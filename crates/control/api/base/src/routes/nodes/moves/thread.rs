//! Guarded thread move reviews using the ordinary tree locks, access preview and history procedures.

pub use super::NodeKind;

use super::Plan;

use super::plan_of;

use super::plan_reply;

use super::shown;

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

use orch_access_contract::audit::AuditAction;

pub use orch_api_contract::common::ErrorCode;

use orch_api_contract::forks::PlanBlocker;

use orch_api_contract::nodes::MoveSetting;

pub use orch_api_contract::nodes::Node;

use orch_api_contract::nodes::NodeOrSystem;

pub use orch_api_contract::thread_move::ThreadMoveBody;

pub use orch_api_contract::thread_move::ThreadMovePlan;

pub use orch_api_contract::thread_move::ThreadMovePreviewBody;

use orch_api_contract::thread_move::ThreadMoveUnchanged;

pub use orch_api_contract::threads::Thread;

use orch_authz::previews::Before;

pub use orch_authz::previews::Preview;

use orch_authz::previews::Scope;

use orch_authz::things::Thing;

pub use orch_authz::Policy;

pub use orch_authz::Principal;

pub use orch_authz::epoch;

pub use orch_db::queries::hierarchy;

pub use orch_db::queries::jobs;

pub use orch_db::queries::threads;

pub use orch_db::Conn;

pub use orch_db::DbError;

pub use orch_engine::thread_move;

pub use orch_settings::Place;

pub use orch_settings::resolve_all;

pub use orch_types::Id;

pub use orch_types::Time;

use orch_types::new_id;

pub use serde_json::json;

use crate::key_operations::native::effects::FirstEffect;

pub use orch_access_contract::permissions::ObjectKind;
pub struct Reviewed {
    pub id: Id,
    pub actor: Id,
    pub plan: ThreadMovePlan,
    pub sources: operation::ReviewSources,
}

pub struct MovedThread {
    pub receipt: orch_api_contract::thread_move::ThreadMoveReceipt,
    pub original: Option<operation::OriginalGate>,
}

/// This is the ordinary preflight interval, shared with other reviewed changes.
pub const PLAN_LIFETIME: chrono::Duration = chrono::Duration::minutes(15);

pub const KIND: &str = "thread_move";

orch_api_base::api_family_callback! {
    bind __api_bind_aae135d331, __api_install_aae135d331, __api_callback_aae135d331;
    slot __ApiCallbackaae135d331, __API_CALLBACK_AAE135D331, __api_set_aae135d331;
    base orch_api_base::routes::nodes::moves::thread;

    async fn ordinary<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_db::queries::threads::ThreadRow>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_08cc559e9a, __api_install_08cc559e9a, __api_callback_08cc559e9a;
    slot __ApiCallback08cc559e9a, __API_CALLBACK_08CC559E9A, __api_set_08cc559e9a;
    base orch_api_base::routes::nodes::moves::thread;

    async fn settings<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<std::collections::BTreeMap<String, Option<orch_types::Json>>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e3bfe7ff85, __api_install_e3bfe7ff85, __api_callback_e3bfe7ff85;
    slot __ApiCallbacke3bfe7ff85, __API_CALLBACK_E3BFE7FF85, __api_set_e3bfe7ff85;
    base orch_api_base::routes::nodes::moves::thread;

    fn combined<'__api>(arg0: Vec<orch_authz::previews::Moved>, arg1: &'__api mut i64) -> Vec<orch_authz::previews::Moved>;
}

pub async fn trial(
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
pub struct ReviewStamp {
    pub id: Id,
    pub until: Time,
}

pub async fn review(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    base: &operation::Base,
    stamp: ReviewStamp,
    first: &mut Option<FirstEffect<'_>>,
    original: Option<&operation::OriginalGate>,
) -> ApiResult<(ThreadMovePlan, Vec<(ObjectKind, Id)>)> {
    let node = base.row.node_id;
    let target = base.target;
    let lanes = base.lanes.as_slice();
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

orch_api_base::api_family_callback! {
    bind __api_bind_12543c587f, __api_install_12543c587f, __api_callback_12543c587f;
    slot __ApiCallback12543c587f, __API_CALLBACK_12543C587F, __api_set_12543c587f;
    base orch_api_base::routes::nodes::moves::thread;

    async fn fingerprint<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_api_contract::thread_move::ThreadMovePlan) -> orch_api_base::middleware::requests::ApiResult<String>;
}

#[path = "thread/operation.rs"]
pub mod operation;

#[path = "thread/writing.rs"]
pub mod writing;
