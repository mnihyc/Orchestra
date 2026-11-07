//! Guarded thread move reviews using the ordinary tree locks, access preview and history procedures.

use super::{NodeKind, Plan, may_move, plan_of, plan_reply, shown};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::forks::PlanBlocker;
use orch_api_contract::nodes::{MoveSetting, Node, NodeOrSystem};
use orch_api_contract::thread_move::{
    ThreadMove, ThreadMoveBody, ThreadMovePlan, ThreadMovePreview, ThreadMovePreviewBody,
    ThreadMoveUnchanged,
};
use orch_api_contract::threads::Thread;
use orch_authz::previews::{Before, Moved, PEOPLE_MOST, Preview, Scope};
use orch_authz::things::Thing;
use orch_authz::{Policy, Principal, epoch};
use orch_db::queries::{hierarchy, jobs, threads};
use orch_db::{Conn, DbError};
use orch_engine::thread_move;
use orch_settings::{Place, resolve_all};
use orch_types::{Hash, Id, Json, Time, new_id};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};

mod operation;
mod projection;
mod writing;

use crate::key_operations::native::effects::{self, FirstEffect};
use orch_access_contract::permissions::ObjectKind;

pub(super) struct Reviewed {
    pub id: Id,
    pub actor: Id,
    pub plan: ThreadMovePlan,
    sources: operation::ReviewSources,
}
pub(super) struct MovedThread {
    pub receipt: orch_api_contract::thread_move::ThreadMoveReceipt,
    original: Option<operation::OriginalGate>,
}

/// This is the ordinary preflight interval, shared with other reviewed changes.
const PLAN_LIFETIME: chrono::Duration = chrono::Duration::minutes(15);
const KIND: &str = "thread_move";

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

async fn trial(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    plan: &Plan,
    lanes: &[threads::ThreadRow],
    first: &mut Option<FirstEffect<'_>>,
    original: Option<&operation::OriginalGate>,
) -> ApiResult<(
    orch_api_contract::access::PagePreview,
    Vec<MoveSetting>,
    Vec<(ObjectKind, Id)>,
)> {
    let mut before = Vec::new();
    for lane in lanes {
        before.push(
            Before::take(
                conn,
                principal,
                Scope::Page(Thing::Node(lane.node_id)),
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?,
        );
    }
    let prior = settings(conn, plan.moved.id).await?;
    sqlx::query("SAVEPOINT thread_move_trial")
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let tested: ApiResult<_> = async {
        thread_move::reparent_before(conn, lanes, plan.moved.id, plan.to.id, ctx.now, || {
            operation::check_first(ctx, first, original)
        })
        .await
        .map_err(ApiError::engine)?;
        let mut access = Preview::default();
        for before in &before {
            let current = before
                .finish(conn, principal)
                .await
                .map_err(ApiError::internal)?;
            access.pages += current.pages;
            access.more += current.more;
            access.gaining.extend(current.gaining);
            access.losing.extend(current.losing);
        }
        // The main and its retained side lanes are distinct pages. A person is named once with
        // their union of permissions and the number of those pages that changes for them.
        access.gaining = combined(access.gaining, &mut access.more);
        access.losing = combined(access.losing, &mut access.more);
        let mut changed = Vec::new();
        let mut sources = Vec::new();
        for setting in resolve_all(conn, Place::Node(plan.moved.id))
            .await
            .map_err(ApiError::internal)?
        {
            let from = prior.get(setting.key()).cloned().unwrap_or(None);
            let to = setting.value().cloned();
            if from == to {
                continue;
            }
            let locked_by = match setting.lock_above().map(|lock| lock.node) {
                None => None,
                Some(None) => Some(NodeOrSystem::SYSTEM),
                Some(Some(id)) => {
                    // The native setting's real owner supplies this source before
                    // formatting; trial ancestry never becomes a scope proof.
                    let source = crate::key_operations::targets::node(conn, id).await?;
                    if let orch_access_contract::key_scope::KeyTargetKind::Object(kind) =
                        source.object.kind
                    {
                        sources.push((kind, id));
                    }
                    let refs =
                        crate::routes::nodes::view::refs(conn, principal, &[id], Policy::default())
                            .await?;
                    Some(
                        refs.get(&id)
                            .cloned()
                            .map_or(NodeOrSystem::SYSTEM, NodeOrSystem::Node),
                    )
                }
            };
            changed.push(MoveSetting {
                key: setting.key().to_owned(),
                from,
                to,
                locked_by,
            });
        }
        changed.sort_by(|one, two| one.key.cmp(&two.key));
        let access = crate::routes::access::previews::worded(conn, access).await?;
        Ok((access, changed, sources))
    }
    .await;
    sqlx::query("ROLLBACK TO SAVEPOINT thread_move_trial")
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("RELEASE SAVEPOINT thread_move_trial")
        .execute(conn)
        .await
        .map_err(DbError::from)?;
    tested
}

/// The identity and expiry belong to the same reviewed preflight.
struct ReviewStamp {
    id: Id,
    until: Time,
}

async fn review(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    target: Id,
    lanes: &[threads::ThreadRow],
    stamp: ReviewStamp,
    first: &mut Option<FirstEffect<'_>>,
    original: Option<&operation::OriginalGate>,
) -> ApiResult<(ThreadMovePlan, Vec<(ObjectKind, Id)>)> {
    let row = ordinary(conn, principal, node).await?;
    let plan = plan_of(ctx, conn, NodeKind::Thread, node, target).await?;
    let mut movement = plan_reply(ctx, conn, principal, &plan).await?;
    if row.topic_id == target {
        movement.blockers.push(PlanBlocker {
            code: "node.wrong_parent".to_owned(),
            message: "choose another topic of this tree".to_owned(),
            missing: None,
        });
    }
    if let Some(busy) = thread_move::busy(conn, lanes)
        .await
        .map_err(ApiError::internal)?
    {
        movement.blockers.push(PlanBlocker {
            code: "thread.not_idle".to_owned(),
            message: format!(
                "a main or side lane is {}; wait for it before moving",
                busy.state
            ),
            missing: None,
        });
    }
    let mut sources = Vec::new();
    if movement.blockers.is_empty() {
        let (access, settings, content) =
            trial(ctx, conn, principal, &plan, lanes, first, original).await?;
        movement.access = access;
        movement.settings = settings;
        sources = content;
    }
    let Node::Thread(thread) = shown(ctx, conn, principal, node).await? else {
        return Err(ApiError::not_found());
    };
    movement.preflight_id = stamp.id;
    movement.expires_at = stamp.until;
    let mut answer = ThreadMovePlan {
        movement,
        expected_revision: thread.base.revision,
        unchanged: ThreadMoveUnchanged {
            owner: thread.base.owner,
            environment: thread.environment,
            directory: thread.directory,
        },
    };
    answer.movement.stamp = fingerprint(conn, &answer).await?;
    Ok((answer, sources))
}

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
