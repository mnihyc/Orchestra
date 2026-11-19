//! Overview, the environments a topic's threads use, places and announcements.
//!
//! Bound here: the environments of a topic (those its threads run in and those whose home it is, the
//! default first, each as far as the caller may see it), the one that new threads join, the
//! places where one more could be made, and the state behind the topic's feed, which is in `sync`.
//!
//! What people tell the threads of a topic or of a project is in the module `announcements` beside this one.

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::environments::{environments_of, places_for};
use crate::routes::nodes::view as node_view;
use orch_api_contract::environment::{Environment, Place, PlaceListQuery};
use orch_api_contract::nodes::{Node, NodeKind, Topic};
use orch_api_contract::topics::{self as api, TopicDefaultEnvironmentSetBody};
use orch_authz::Policy;
use orch_db::DbError;
use orch_db::audit::Actor;
use orch_db::queries::environments::hosting as rows;
use orch_settings::write::{Clear, Write, clear, write};
use orch_settings::{Place as SettingsPlace, Target};
use orch_settings_contract::registry::keys;
use orch_types::Id;
use serde_json::json;

mod announcements;
pub(crate) mod sync;

pub use announcements::mount_announcements;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::TopicEnvironmentList, environments)
        .on(api::TopicDefaultEnvironmentSet, default_environment_set)
        .on(api::TopicPlaceList, places);
    sync::mount(registry);
}

/// The environments a topic's threads run in and those whose home it is, the default first, each as
/// far as the caller may see it.
async fn environments(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<Environment>> {
    let topic = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let default = default_of(&mut conn, topic).await?;
    let mut ids = rows::environments_under(&mut conn, topic).await?;
    if let Some(default) = default
        && !ids.contains(&default)
    {
        ids.push(default);
    }
    let mut listed = environments_of(&ctx, &mut conn, &principal, &ids).await?;
    listed.sort_by_key(|environment| Some(environment.base.id) != default);
    Ok(listed)
}

/// The environment new threads of a topic join, if one was chosen.
async fn default_of(conn: &mut orch_db::Conn, topic: Id) -> ApiResult<Option<Id>> {
    orch_settings::get(conn, SettingsPlace::Node(topic), keys::ENV_DEFAULT_ID)
        .await
        .map_err(ApiError::internal)
}

/// Writes or clears the ordinary inherited default choice. A thread's admission still checks its creator's access to it.
async fn default_environment_set(
    ctx: Ctx,
    (): (),
    body: TopicDefaultEnvironmentSetBody,
) -> ApiResult<Topic> {
    let topic = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let (_, kind) = crate::routes::nodes::placed(&mut tx, topic).await?;
    if kind != NodeKind::Topic {
        return Err(ApiError::not_found());
    }
    let may_lock = orch_authz::nodes::check(
        &mut tx,
        &principal,
        topic,
        orch_access_contract::permissions::Permission::SettingsManage,
        orch_authz::Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .is_allowed();
    // This older convenience request writes the ordinary inherited setting, including its locks and audit.
    let written = match body.environment_id {
        Some(id) => {
            write(
                &mut tx,
                &Write {
                    actor: Actor::User(principal.user()),
                    target: Target::Node(topic),
                    key: keys::ENV_DEFAULT_ID,
                    value: json!(id),
                    locked: None,
                    expected_revision: None,
                    may_lock,
                    origin: &crate::sign_in::origin(&ctx),
                },
                ctx.now,
            )
            .await
        }
        None => {
            clear(
                &mut tx,
                &Clear {
                    actor: Actor::User(principal.user()),
                    target: Target::Node(topic),
                    key: keys::ENV_DEFAULT_ID,
                    expected_revision: None,
                    may_lock,
                    origin: &crate::sign_in::origin(&ctx),
                },
                ctx.now,
            )
            .await
        }
    };
    if let Err(error) = written {
        return Err(crate::routes::settings::write_failed(&mut tx, &principal, topic, error).await);
    }
    orch_db::queries::hierarchy::stamp(&mut tx, topic).await?;
    let shown = match node_view::one(&mut tx, &principal, topic, ctx.now, Policy::default()).await?
    {
        Some(Node::Topic(mut shown)) => {
            let mut ids = rows::environments_under(&mut tx, topic).await?;
            if let Some(default) = shown.default_environment_id
                && !ids.contains(&default)
            {
                ids.push(default);
            }
            shown.environments = environments_of(&ctx, &mut tx, &principal, &ids)
                .await?
                .into_iter()
                .map(|environment| environment.base)
                .collect();
            shown
        }
        _ => return Err(ApiError::not_found()),
    };
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// Where a thread of this topic could run, for the caller: the hosts they may use for a new
/// environment, then the environments they manage.
async fn places(ctx: Ctx, query: PlaceListQuery, (): ()) -> ApiResult<Vec<Place>> {
    let topic = ctx.id("id")?;
    places_for(&ctx, topic, &query).await
}
