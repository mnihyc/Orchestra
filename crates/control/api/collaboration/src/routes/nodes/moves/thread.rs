//! Guarded thread move reviews using the ordinary tree locks, access preview and history procedures.

use super::NodeKind;

use super::may_move;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::nodes::Node;

use orch_api_contract::thread_move::ThreadMove;

use orch_api_contract::thread_move::ThreadMoveBody;

use orch_api_contract::thread_move::ThreadMovePlan;

use orch_api_contract::thread_move::ThreadMovePreview;

use orch_api_contract::thread_move::ThreadMovePreviewBody;

use orch_api_contract::threads::Thread;

use orch_authz::previews::Moved;

use orch_authz::previews::PEOPLE_MOST;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::epoch;

use orch_db::queries::hierarchy;

use orch_db::queries::jobs;

use orch_db::queries::threads;

use orch_db::Conn;

use orch_db::DbError;

use orch_engine::thread_move;

use orch_settings::Place;

use orch_settings::resolve_all;

use orch_types::Hash;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use serde_json::json;

use std::collections::BTreeMap;

use std::collections::HashMap;

use crate::key_operations::native::effects;

use orch_access_contract::permissions::ObjectKind;

pub use orch_api_base::routes::nodes::moves::thread::Reviewed;

pub use orch_api_base::routes::nodes::moves::thread::MovedThread;

pub use orch_api_base::routes::nodes::moves::thread::PLAN_LIFETIME;

pub use orch_api_base::routes::nodes::moves::thread::KIND;

pub use orch_api_base::routes::nodes::moves::thread::trial;

pub use orch_api_base::routes::nodes::moves::thread::ReviewStamp;

pub use orch_api_base::routes::nodes::moves::thread::review;
/// Binds the main thread's preview and confirmation.
pub fn mount(registry: &mut Registry) {
    registry
        .on_with_operation(
            ThreadMovePreview,
            effects::prepare::<operation::Preview>,
            writing::preview,
        )
        .on_with_operation(
            ThreadMove,
            effects::prepare::<operation::Move>,
            writing::move_thread,
        );
}

async fn ordinary(
    conn: &mut Conn,
    principal: &Principal,
    main: Id,
) -> ApiResult<threads::ThreadRow> {
    let row = threads::thread(conn, main)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if let Some(parent) = row.side_of {
        return Err(ApiError::new(
            ErrorCode::NodeWrongParent,
            "a side lane moves only with its main conversation",
        )
        .with("main_thread_id", parent));
    }
    let kind: String = sqlx::query_scalar("SELECT kind FROM users WHERE id=$1")
        .bind(principal.user())
        .fetch_one(conn)
        .await
        .map_err(DbError::from)?;
    if kind == "service" {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "a service account cannot move a conversation",
        )
        .with("requestable", false));
    }
    Ok(row)
}
orch_api_base::__api_bind_aae135d331!();

async fn locks(
    conn: &mut Conn,
    root: Id,
    main: Id,
    target: Id,
) -> ApiResult<Vec<threads::ThreadRow>> {
    hierarchy::lock_tree_for_moves(conn, root).await?;
    let lanes = thread_move::lock_lanes(conn, main)
        .await
        .map_err(ApiError::internal)?;
    let mut nodes: Vec<_> = lanes
        .iter()
        .map(|row| row.node_id)
        .chain(std::iter::once(target))
        .collect();
    nodes.sort_unstable();
    nodes.dedup();
    for node in nodes {
        hierarchy::lock(conn, node).await?;
    }
    Ok(lanes)
}

async fn settings(conn: &mut Conn, node: Id) -> ApiResult<BTreeMap<String, Option<Json>>> {
    Ok(resolve_all(conn, Place::Node(node))
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|setting| (setting.key().to_owned(), setting.value().cloned()))
        .collect())
}
orch_api_base::__api_bind_08cc559e9a!();

fn combined(people: Vec<Moved>, more: &mut i64) -> Vec<Moved> {
    let mut by_person: HashMap<Id, Moved> = HashMap::new();
    for person in people {
        match by_person.entry(person.user) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(person);
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let tally = entry.get_mut();
                tally.pages += person.pages;
                tally.permissions.extend(person.permissions);
                tally.permissions.sort_unstable();
                tally.permissions.dedup();
            }
        }
    }
    let mut people: Vec<_> = by_person.into_values().collect();
    people.sort_by(|one, two| {
        two.pages
            .cmp(&one.pages)
            .then_with(|| one.user.cmp(&two.user))
    });
    *more += i64::try_from(people.len().saturating_sub(PEOPLE_MOST)).unwrap_or(i64::MAX);
    people.truncate(PEOPLE_MOST);
    people
}
orch_api_base::__api_bind_e3bfe7ff85!();

async fn fingerprint(conn: &mut Conn, plan: &ThreadMovePlan) -> ApiResult<String> {
    // The existing epoch binds all page/group/token changes; settings and node revisions bind values
    // that do not move it. A changed authorization epoch conservatively requires a fresh review.
    let epoch = epoch::current(conn).await.map_err(ApiError::internal)?;
    let source = settings(conn, plan.movement.node.id).await?;
    let destination = settings(conn, plan.movement.to.id).await?;
    let ancestors =
        hierarchy::with_ancestors(conn, &[plan.movement.node.id, plan.movement.to.id]).await?;
    let mut revisions: Vec<_> = ancestors
        .into_iter()
        .map(|node| (node.id, node.revision))
        .collect();
    revisions.sort_unstable();
    let mut movement = serde_json::to_value(&plan.movement).map_err(ApiError::internal)?;
    if let Some(fields) = movement.as_object_mut() {
        fields.remove("stamp");
    }
    let value = json!({"epoch":epoch,"source":source,"destination":destination,"revisions":revisions,"movement":movement,"unchanged":plan.unchanged});
    Ok(Hash::of(&serde_json::to_vec(&value).map_err(ApiError::internal)?).to_string())
}
orch_api_base::__api_bind_12543c587f!();

#[path = "thread/operation.rs"]
pub mod operation;

#[path = "thread/projection.rs"]
pub mod projection;

#[path = "thread/writing.rs"]
pub mod writing;
