//! Favorites, recents and read/subscription state; what the interface keeps for a person.
//!
//! Everything here acts on the caller and on nobody else. A favorite or a
//! recent is a kind/id the caller may view; one they lost the right to is no
//! longer listed. The interface state is kept under nine keys and no
//! others, whole, and no value is larger than the limit; the last view of a
//! project is also kept one project at a time, beside the others. Drafts and the
//! chat states belong to the threads and the chats, and the read position
//! of a thread to the threads routes: none of the three is bound here.

pub(crate) mod viewed;

use crate::middleware::authorization::{self, VIEW};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{invalid, placed, view};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::personal::{
    Favorite, MeFavoriteDelete, MeFavoriteList, MeFavoriteSet, MeFavoriteSetBody,
    MeItemFavoriteDelete, MeItemFavoriteSet, MeItemFavoriteSetBody, MeProjectViewDelete,
    MeProjectViewSet, MeReadStateList, MeReadStateListQuery, MeRecentCreate, MeRecentCreateBody,
    MeStateGet, MeStateSet, MeSubscription, MeSubscriptionList, MeSubscriptionListQuery,
    MeVisitCreate, PERSONAL_STATE_MAX_BYTES, PersonalState, PersonalStateKey, PersonalTarget,
    ProjectView, ReadState, ShortcutState, SubscriptionSource,
};
use orch_authz::{Act, Policy, Principal};
use orch_db::queries::hierarchy::{self, Placing};
use orch_db::queries::identity;
use orch_types::{Id, Json, Time};

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(MeFavoriteList, favorite_list)
        .on(MeFavoriteSet, favorite_set)
        .on(MeFavoriteDelete, favorite_delete)
        .on(MeRecentCreate, recent_create)
        .on(MeItemFavoriteSet, item_favorite_set)
        .on(MeItemFavoriteDelete, item_favorite_delete)
        .on(MeVisitCreate, visit_create)
        .on(MeReadStateList, read_state_list)
        .on(MeSubscriptionList, subscription_list)
        .on(MeStateGet, state_get)
        .on(MeStateSet, state_set)
        .on(MeProjectViewSet, project_view_set)
        .on(MeProjectViewDelete, project_view_delete);
}

/// The caller's favorites in order, each with the way down to it, as far as
/// they may still view them.
async fn favorite_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<Favorite>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    favorites_for(&mut conn, &principal, ctx.now).await
}

/// The ordered personal stars, shared by the individual read and the shell bootstrap.
pub(crate) async fn favorites_for(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<Vec<Favorite>> {
    let rows = hierarchy::favorites_of(conn, principal.user()).await?;
    let ids: Vec<Id> = rows.iter().map(|row| row.node_id).collect();
    let nodes = view::visible(conn, principal, &ids, now, Policy::default()).await?;
    let ways = view::ways(conn, principal, &ids, Policy::default()).await?;
    let viewable: std::collections::HashSet<Id> =
        nodes.iter().map(crate::routes::nodes::node_id).collect();
    Ok(rows
        .into_iter()
        .filter(|row| viewable.contains(&row.node_id))
        .filter_map(|row| {
            let mut path = ways.get(&row.node_id)?.clone();
            let node: NodeRef = path.pop()?;
            Some(Favorite {
                node,
                path,
                position: row.position,
                // A view that no longer reads as one stars the place alone.
                view: row.view.and_then(|view| serde_json::from_value(view).ok()),
            })
        })
        .collect())
}

/// Makes a node a favorite, or moves it within the list.
async fn favorite_set(ctx: Ctx, (): (), body: MeFavoriteSetBody) -> ApiResult<()> {
    let node = ctx.id("node_id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let user = ctx.user()?.id();
    let placing = match body.after_node_id {
        None => Placing::Last,
        Some(None) => Placing::First,
        Some(Some(after)) => Placing::After(after),
    };
    let view = match body.view {
        None => None,
        Some(None) => Some(None),
        Some(Some(view)) => {
            // A view is kept for a project alone: it is how its topics were looked at.
            let (_, kind) = placed(&mut conn, node).await?;
            if kind != NodeKind::Project {
                return Err(invalid("view", "is kept for a project alone"));
            }
            Some(Some(
                serde_json::to_value(view).map_err(ApiError::internal)?,
            ))
        }
    };
    if !hierarchy::put_favorite(&mut conn, user, node, placing, ctx.now).await? {
        return Err(invalid(
            "after_node_id",
            "is not one of the favorites it can go after",
        ));
    }
    // Left out, the view a favorite has stays; null stars the place alone.
    if let Some(view) = view {
        hierarchy::set_favorite_view(&mut conn, user, node, view.as_ref()).await?;
    }
    Ok(())
}

/// Takes a node out of the favorites.
async fn favorite_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let node = ctx.id("node_id")?;
    let mut conn = ctx.app.db.acquire().await?;
    hierarchy::delete_favorite(&mut conn, ctx.user()?.id(), node).await?;
    Ok(())
}

/// Notes that the caller went to a node they may view.
async fn recent_create(ctx: Ctx, (): (), body: MeRecentCreateBody) -> ApiResult<()> {
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    let principal = guard.principal();
    authorization::require(
        &mut conn,
        principal,
        body.node_id,
        VIEW,
        Act::Use,
        Policy::default(),
    )
    .await?;
    hierarchy::touch_recent(
        &mut conn,
        principal.user(),
        body.node_id,
        guard.now().get(),
        i64::from(ctx.app.config.limits.recents_per_user),
    )
    .await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

fn personal_target(ctx: &Ctx) -> ApiResult<PersonalTarget> {
    let kind = serde_json::from_value(Json::String(ctx.param("kind")?.to_owned()))
        .map_err(|_| invalid("kind", "is not a searchable kind"))?;
    Ok(PersonalTarget {
        kind,
        id: ctx.id("id")?,
    })
}

/// Exact identity lookups use the same bulk authorization as names, without searching for another person's data.
async fn require_target(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    target: &PersonalTarget,
    at: Time,
) -> ApiResult<()> {
    let ids = [target.id];
    let kinds = [target.kind.as_str().to_owned()];
    let candidates = orch_db::queries::search::names(
        conn,
        &orch_db::queries::search::Names {
            identifiers: Some(&ids),
            starred: false,
            at: at.get(),
            user: principal.user(),
            trees: principal.member_of(),
            roots: &[],
            system_admin: principal.is_system_admin(),
            text: "",
            kinds: &kinds,
            org: None,
            offset: 0,
            limit: 1,
        },
    )
    .await?;
    let visible = orch_engine::projections::visible_names(conn, principal, &candidates)
        .await
        .map_err(ApiError::internal)?;
    if !visible.contains(&(target.kind.as_str().to_owned(), target.id)) {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            "that thing is not available",
        ));
    }
    Ok(())
}

async fn item_favorite_set(ctx: Ctx, (): (), body: MeItemFavoriteSetBody) -> ApiResult<()> {
    let target = personal_target(&ctx)?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    require_target(&mut tx, guard.principal(), &target, guard.now()).await?;
    let placing = match &body.after {
        None => hierarchy::ItemPlacing::Last,
        Some(None) => hierarchy::ItemPlacing::First,
        Some(Some(after)) => hierarchy::ItemPlacing::After {
            kind: after.kind.as_str(),
            id: after.id,
        },
    };
    if !hierarchy::put_item_favorite(
        &mut tx,
        guard.principal().user(),
        target.kind.as_str(),
        target.id,
        placing,
        guard.now().get(),
    )
    .await?
    {
        return Err(invalid("after", "is not another favorite in your list"));
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

async fn item_favorite_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let target = personal_target(&ctx)?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    hierarchy::delete_item_favorite(
        &mut tx,
        guard.principal().user(),
        target.kind.as_str(),
        target.id,
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

async fn visit_create(ctx: Ctx, (): (), target: PersonalTarget) -> ApiResult<()> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    require_target(&mut tx, guard.principal(), &target, guard.now()).await?;
    hierarchy::touch_item_recent(
        &mut tx,
        guard.principal().user(),
        target.kind.as_str(),
        target.id,
        guard.now().get(),
        i64::from(ctx.app.config.limits.recents_per_user),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// How far the caller has read their threads.
async fn read_state_list(
    ctx: Ctx,
    query: MeReadStateListQuery,
    (): (),
) -> ApiResult<Vec<ReadState>> {
    let mut conn = ctx.app.db.acquire().await?;
    read_states_for(&mut conn, ctx.user()?.id(), query.org_id).await
}

pub(crate) async fn read_states_for(
    conn: &mut orch_db::Conn,
    user: Id,
    org: Option<Id>,
) -> ApiResult<Vec<ReadState>> {
    Ok(hierarchy::read_states_of(conn, user, org)
        .await?
        .into_iter()
        .map(|(thread_id, continuation_id, last_read_seq)| ReadState {
            thread_id,
            continuation_id: Some(continuation_id),
            last_read_seq,
        })
        .collect())
}

/// The caller's subscriptions.
async fn subscription_list(
    ctx: Ctx,
    query: MeSubscriptionListQuery,
    (): (),
) -> ApiResult<Vec<MeSubscription>> {
    let mut conn = ctx.app.db.acquire().await?;
    subscriptions_for(&mut conn, ctx.user()?.id(), query.org_id).await
}

pub(crate) async fn subscriptions_for(
    conn: &mut orch_db::Conn,
    user: Id,
    org: Option<Id>,
) -> ApiResult<Vec<MeSubscription>> {
    Ok(hierarchy::subscriptions_of(conn, user, org)
        .await?
        .into_iter()
        .filter_map(|row| {
            Some(MeSubscription {
                node_id: row.node_id,
                level: serde_json::from_value(Json::String(row.level)).ok()?,
                source: if row.source == "manual" {
                    SubscriptionSource::Manual
                } else {
                    SubscriptionSource::Auto
                },
            })
        })
        .collect())
}

/// What the caller's interface remembers for them.
async fn state_get(ctx: Ctx, (): (), (): ()) -> ApiResult<PersonalState> {
    let mut conn = ctx.app.db.acquire().await?;
    Ok(identity::interface_state(&mut conn, ctx.user()?.id())
        .await?
        .into_iter()
        .collect())
}

/// Replaces the whole value of one key of that state.
async fn state_set(ctx: Ctx, (): (), mut value: Json) -> ApiResult<()> {
    let key = ctx.param("key")?;
    let known: Option<PersonalStateKey> = serde_json::from_value(Json::String(key.to_owned())).ok();
    if known.is_none() {
        return Err(invalid(
            "key",
            "is none of help, layouts, viewers, timeline, sidebar, project_views, keys, reactions and inbox",
        ));
    }
    // A person's keys are kept only as keys of actions there are, none of them meeting another's.
    if known == Some(PersonalStateKey::Keys) {
        let keys: ShortcutState = serde_json::from_value(value.clone())
            .map_err(|error| invalid("keys", &error.to_string()))?;
        keys.check()
            .map_err(|(member, why)| invalid(&member, &why))?;
    }
    let size = serde_json::to_vec(&value)
        .map_err(ApiError::internal)?
        .len();
    if i64::try_from(size).unwrap_or(i64::MAX) > PERSONAL_STATE_MAX_BYTES {
        return Err(
            ApiError::new(ErrorCode::RequestTooLarge, "the value is too large")
                .with("limit", PERSONAL_STATE_MAX_BYTES),
        );
    }
    if known == Some(PersonalStateKey::Reactions) {
        // The same glyph format as ordinary reactions; one platform grapheme, no custom-emoji
        // object or image. Preserve newest-first order while bounding distinct remembered choices.
        use unicode_segmentation::UnicodeSegmentation;
        const RECENT_REACTIONS: usize = 8;
        let entries = value
            .as_array()
            .ok_or_else(|| invalid("reactions", "must be a list of emoji"))?;
        let mut recent = Vec::new();
        for entry in entries {
            let text = entry
                .as_str()
                .ok_or_else(|| invalid("reactions", "each choice is an emoji"))?;
            let emoji = orch_documents::comments::clean_emoji(text)
                .map_err(|_| invalid("reactions", "each choice is one emoji"))?;
            if emoji.graphemes(true).count() != 1 {
                return Err(invalid("reactions", "each choice is one emoji"));
            }
            if recent.len() < RECENT_REACTIONS && !recent.contains(&emoji) {
                recent.push(emoji);
            }
        }
        value = serde_json::to_value(recent).map_err(ApiError::internal)?;
    }
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    let user = guard.principal().user();
    identity::put_interface_state(&mut conn, user, key, &value, guard.now().get()).await?;
    orch_db::sequences::notify_user_event(
        &mut conn,
        &[user],
        &serde_json::json!({"what": "state", "key": key}),
    )
    .await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Keeps the caller's last view of one project they may view, beside the views of the others: two
/// devices that save the views of two projects at once keep both. The value as it then is holds to
/// the bound of a key, or nothing is written.
async fn project_view_set(ctx: Ctx, (): (), body: ProjectView) -> ApiResult<()> {
    let project = ctx.id("project_id")?;
    let mut conn = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let (_, kind) = placed(&mut conn, project).await?;
    if kind != NodeKind::Project {
        return Err(ApiError::not_found());
    }
    authorization::require(
        &mut conn,
        &principal,
        project,
        VIEW,
        Act::Use,
        Policy::default(),
    )
    .await?;
    let view = serde_json::to_value(&body).map_err(ApiError::internal)?;
    let user = principal.user();
    let size = identity::put_project_view(&mut conn, user, project, &view, ctx.now).await?;
    if size > PERSONAL_STATE_MAX_BYTES {
        return Err(
            ApiError::new(ErrorCode::RequestTooLarge, "the value is too large")
                .with("limit", PERSONAL_STATE_MAX_BYTES),
        );
    }
    orch_db::sequences::notify_user_event(
        &mut conn,
        &[user],
        &serde_json::json!({"what": "state", "key": "project_views"}),
    )
    .await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Forgets the caller's last view of one project. It asks for nothing of the project: what one saved
/// for a place one can no longer see is one's own to clear.
async fn project_view_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let project = ctx.id("project_id")?;
    let mut conn = ctx.app.db.begin().await?;
    let user = ctx.user()?.id();
    identity::delete_project_view(&mut conn, user, project, ctx.now).await?;
    orch_db::sequences::notify_user_event(
        &mut conn,
        &[user],
        &serde_json::json!({"what": "state", "key": "project_views"}),
    )
    .await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}
