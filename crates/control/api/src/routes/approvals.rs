//! Approval listing and decisions.
//!
//! What a harness asks before it acts waits here as an approval, and whoever
//! manages its thread answers it: the first answer to reach the database
//! wins, and the harness is told after the commit. A person's own list holds
//! what they can decide now, what they asked for, or what is attached to a
//! node they may view. Who may decide is the access engine's to say, asked of
//! it for the thread of each approval, the same way for one as for a page of
//! them. A request for a capability is decided on the machine's side, and a
//! request for access by whoever could change the page of what it asks for
//! (`routes/access/requests`); both are listed and read here with the rest.

use crate::middleware::authorization::{self, MANAGE, VIEW, exact, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::access::requests;
use crate::routes::nodes::view;
use orch_access_contract::permissions::Permission;
use orch_api_contract::access::ObjectRef;
use orch_api_contract::approvals::{
    Approval, ApprovalCancel, ApprovalDecide, ApprovalDecision, ApprovalDecisionReply, ApprovalGet,
    ApprovalList, ApprovalListQuery, ApprovalListState, ApprovalRequest, ApprovalRequester,
    ApprovalState,
};
use orch_api_contract::common::{ErrorCode, Page, PageQuery};
use orch_api_contract::threads::ThreadMention;
use orch_authz::{Act, Policy, Principal, nodes};
use orch_db::Conn;
use orch_db::queries::approvals::{self as rows, ApprovalRow, Listing};
use orch_engine::approvals::ApprovalError;
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::{HashMap, HashSet};

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(ApprovalList, list)
        .on(ApprovalGet, get)
        .on(ApprovalDecide, decide)
        .on(ApprovalCancel, cancel);
}

/// How many rows of a list are read at once while some of them are left out
/// because the caller may not see them: a page of mostly visible approvals
/// is found in one round.
const READ_AHEAD: usize = 1;

/// How many waiting approvals are read at a time while those of the caller's own list are counted:
/// the waiting approvals of a whole system usually fit in one round, and a round holds no more
/// rows than this in memory, whatever is waiting.
const WAITING_PER_ROUND: i64 = 500;

/// The states a list is wanted in.
fn states_of(wanted: ApprovalListState) -> &'static [&'static str] {
    match wanted {
        ApprovalListState::Pending => &["pending"],
        ApprovalListState::Decided => &["approved", "answered", "denied", "cancelled", "expired"],
    }
}

/// Approvals as the caller sees them: what each asks, who asked, where it
/// stands, and whether the caller can decide it.
pub(crate) async fn views(
    conn: &mut Conn,
    principal: &Principal,
    found: &[ApprovalRow],
    now: chrono::DateTime<chrono::Utc>,
    engine: &orch_engine::Engine,
) -> ApiResult<Vec<Approval>> {
    Ok(views_routed(conn, principal, found, now, engine).await?.0)
}

/// [`views`], and which of the requests for access among them were sent to the reader: they are
/// among the nearest people who could give what each asks.
/// For the pending plans among `found`: what deciding each thread's plans takes, and the threads whose plans the
/// caller may answer as a writer there.
async fn plan_deciders(
    conn: &mut Conn,
    principal: &Principal,
    found: &[ApprovalRow],
) -> ApiResult<(HashMap<Id, Permission>, HashSet<Id>)> {
    let mut plan_threads: HashMap<Id, Permission> = HashMap::new();
    for row in found
        .iter()
        .filter(|row| row.kind == "harness" && row.state == "pending")
    {
        if let Some(thread) = row.thread_id
            && row.request.get("request_kind").and_then(Json::as_str) == Some("plan")
        {
            let permission =
                orch_engine::approvals::deciding_permission(conn, thread, &row.request)
                    .await
                    .map_err(ApiError::internal)?;
            plan_threads.insert(thread, permission);
        }
    }
    let writing: Vec<Id> = plan_threads
        .iter()
        .filter(|(_, permission)| **permission == Permission::ThreadInteract)
        .map(|(thread, _)| *thread)
        .collect();
    let writers: HashSet<Id> = if writing.is_empty() {
        HashSet::new()
    } else {
        nodes::holding(
            conn,
            principal,
            &writing,
            exact(Permission::ThreadInteract),
            Act::Use,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|listed| listed.id)
        .collect()
    };
    Ok((plan_threads, writers))
}

async fn views_routed(
    conn: &mut Conn,
    principal: &Principal,
    found: &[ApprovalRow],
    now: chrono::DateTime<chrono::Utc>,
    engine: &orch_engine::Engine,
) -> ApiResult<(Vec<Approval>, HashSet<Id>)> {
    let mut source_deciders = HashSet::new();
    for row in found
        .iter()
        .filter(|row| matches!(row.kind.as_str(), "plan" | "input") && row.state == "pending")
    {
        if engine
            .may_decide_source_request(conn, principal, row)
            .await
            .map_err(ApiError::internal)?
        {
            source_deciders.insert(row.id);
        }
    }
    // A request for a capability is decided by the people of its scope, which only the engine can say.
    let mut capability_deciders: HashSet<Id> = HashSet::new();
    for row in found
        .iter()
        .filter(|row| row.kind == "capability" && row.state == "pending")
    {
        if engine
            .may_decide_capability_request(principal.caller(), row)
            .await
            .unwrap_or(false)
        {
            capability_deciders.insert(row.id);
        }
    }
    // A port request is decided by whoever holds Ports Manage on its environment now.
    let mut port_deciders: HashSet<Id> = HashSet::new();
    for row in found
        .iter()
        .filter(|row| row.kind == "port" && row.state == "pending")
    {
        if let Some(environment) = row.environment_id
            && orch_engine::ports::holds(conn, principal, environment, Permission::PortsManage)
                .await
                .unwrap_or(false)
        {
            port_deciders.insert(row.id);
        }
    }
    let mut threads: Vec<Id> = found.iter().filter_map(|row| row.thread_id).collect();
    threads.sort_unstable();
    threads.dedup();
    let deciders: HashSet<Id> = if threads.is_empty() {
        HashSet::new()
    } else {
        nodes::holding(
            conn,
            principal,
            &threads,
            exact(Permission::ThreadManage),
            Act::Use,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|listed| listed.id)
        .collect()
    };
    // A plan an agent proposes is answered by whoever may write to the agent, unless its place keeps plans
    // to the thread's managers. Boxed: the view of a page of approvals is awaited deep in a request's stack.
    let (plan_threads, writers) = Box::pin(plan_deciders(conn, principal, found)).await?;
    let mut people: Vec<Id> = Vec::new();
    for row in found {
        people.extend(row.requester_user_id);
        people.extend(row.decided_by);
    }
    let people = refusals::user_refs(conn, &people).await?;
    let mut nodes_named: Vec<Id> = found
        .iter()
        .filter_map(|row| row.node_id)
        .chain(threads.iter().copied())
        .collect();
    nodes_named.sort_unstable();
    nodes_named.dedup();
    let refs = view::refs(conn, principal, &nodes_named, Policy::default()).await?;
    // Names and their visibility come from this one batch, including for the org home's summary.
    let names: HashMap<Id, String> = threads
        .iter()
        .filter_map(|thread| {
            refs.get(thread)
                .and_then(|node| node.name.clone())
                .map(|name| (*thread, name))
        })
        .collect();
    // A request for access is shown by the part that raises it: what it asks, with the names the
    // reader may know, where it stands, and whether the reader may decide it.
    let mut access = requests::views_of(conn, principal, engine, found, now).await?;
    let ids: Vec<_> = found.iter().map(|row| row.id).collect();
    let mut sent_to_them: HashSet<Id> = sqlx::query_scalar(
        "SELECT DISTINCT approval_id FROM notifications WHERE user_id=$1 AND approval_id=ANY($2)",
    )
    .bind(principal.user())
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?
    .into_iter()
    .collect();
    for row in found
        .iter()
        .filter(|row| matches!(row.kind.as_str(), "plan" | "input") && row.state == "pending")
    {
        if engine
            .approval_recipients(conn, row, Time::new(now))
            .await
            .map_err(ApiError::internal)?
            .contains(&principal.user())
        {
            sent_to_them.insert(row.id);
        }
    }
    let mut approvals = Vec::with_capacity(found.len());
    for row in found {
        let quick_grant_durations = if row.kind == "access" {
            let place = row
                .node_id
                .map_or(orch_settings::Place::System, orch_settings::Place::Node);
            orch_settings::get(
                conn,
                place,
                orch_settings_contract::registry::keys::ACCESS_QUICK_GRANT_DURATIONS,
            )
            .await
            .map_err(ApiError::internal)?
        } else {
            None
        };
        let shown = access.remove(&row.id);
        let request = match &shown {
            Some(shown) => ApprovalRequest::Access(shown.request.clone()),
            None => serde_json::from_value(json!({ "kind": row.kind, "request": row.request }))
                .map_err(ApiError::internal)?,
        };
        let requester = match (
            row.requester_user_id,
            row.requester_thread_id.or(row.thread_id),
        ) {
            (Some(user), _) if people.contains_key(&user) => {
                ApprovalRequester::User(people[&user].clone())
            }
            (_, Some(thread)) => ApprovalRequester::Thread(ThreadMention {
                id: thread,
                name: names.get(&thread).cloned().unwrap_or_default(),
            }),
            _ => return Err(ApiError::internal("an approval nobody asked for")),
        };
        approvals.push(Approval {
            id: row.id,
            request,
            state: match &shown {
                Some(shown) => shown.state,
                None => serde_json::from_value(json!(row.state)).map_err(ApiError::internal)?,
            },
            title: if shown
                .as_ref()
                .is_some_and(|shown| shown.object.name.is_empty())
            {
                "Access request".to_owned()
            } else {
                row.title.clone()
            },
            node: if shown
                .as_ref()
                .is_some_and(|shown| shown.object.name.is_empty())
            {
                None
            } else {
                row.node_id.and_then(|node| refs.get(&node).cloned())
            },
            object: match &shown {
                Some(shown) => Some(shown.object.clone()),
                None => None::<ObjectRef>.or_else(|| {
                    row.object_kind
                        .as_ref()
                        .zip(row.object_id)
                        .map(|(kind, id)| ObjectRef {
                            kind: kind.clone(),
                            id,
                            name: String::new(),
                        })
                }),
            },
            thread_id: row.thread_id,
            item_id: row.item_id,
            requester,
            required_permission: row.required_permission.clone(),
            approver_scope: row
                .approver_scope
                .as_ref()
                .and_then(|scope| serde_json::from_value(json!(scope)).ok()),
            batch_id: row.batch_id,
            can_decide: row.state == "pending"
                && match orch_authz::approvals::authority(&row.kind) {
                    Some(orch_authz::approvals::Authority::HarnessThread) => {
                        row.thread_id.is_some_and(|thread| {
                            let plan = row.request.get("request_kind").and_then(Json::as_str)
                                == Some("plan");
                            if plan
                                && plan_threads.get(&thread) == Some(&Permission::ThreadInteract)
                            {
                                writers.contains(&thread)
                            } else {
                                deciders.contains(&thread)
                            }
                        })
                    }
                    Some(orch_authz::approvals::Authority::CapabilityScope) => {
                        capability_deciders.contains(&row.id)
                    }
                    Some(orch_authz::approvals::Authority::PortEnvironment) => {
                        port_deciders.contains(&row.id)
                    }
                    Some(orch_authz::approvals::Authority::AccessObject) => {
                        shown.as_ref().is_some_and(|shown| shown.can_decide)
                    }
                    Some(
                        orch_authz::approvals::Authority::PlanThread
                        | orch_authz::approvals::Authority::ModelUseThread,
                    ) => source_deciders.contains(&row.id),
                    None => false,
                },
            decided_by: row.decided_by.and_then(|by| people.get(&by).cloned()),
            quick_grant_durations,
            decided_automatically: matches!(row.state.as_str(), "approved" | "denied")
                && row.decided_by.is_none()
                && row.decided_at.is_some(),
            decided_at: row.decided_at.map(Time::new),
            decision: row.decision.clone(),
            expires_at: row.expires_at.map(Time::new),
            created_at: Time::new(row.created_at),
        });
    }
    Ok((approvals, sent_to_them))
}

/// Which approvals a list is of.
enum Scope {
    /// What the caller can decide now.
    Mine,
    /// What the caller may decide that was sent to other people.
    Others,
    /// What the caller asked for.
    Requested,
    /// What is attached to a node or to anything inside it.
    Under(Id),
}

fn scope_of(asked: Option<&str>) -> ApiResult<Scope> {
    match asked {
        None | Some("mine") => Ok(Scope::Mine),
        Some("others") => Ok(Scope::Others),
        Some("requested") => Ok(Scope::Requested),

        Some(other) => other
            .strip_prefix("node:")
            .and_then(|id| id.parse().ok())
            .map(Scope::Under)
            .ok_or_else(|| {
                ApiError::invalid(
                    "scope",
                    "is `mine`, `others`, `requested` or `node:` and a node",
                )
            }),
    }
}

/// Approvals, newest first: those the caller can decide, those they asked for,
/// or those attached to a node they may view.
async fn list(ctx: Ctx, query: ApprovalListQuery, (): ()) -> ApiResult<Page<Approval>> {
    let scope = scope_of(query.scope.as_deref())?;
    let page = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = usize::try_from(page.limit()).unwrap_or(usize::MAX);
    let mut before: Option<Id> = query
        .cursor
        .as_deref()
        .map(|cursor| {
            cursor
                .parse()
                .map_err(|_| ApiError::invalid("cursor", "is not a cursor of this list"))
        })
        .transpose()?;
    let kind = query
        .kind
        .map(|kind| serde_json::to_value(kind).ok())
        .map(|value| value.and_then(|value| value.as_str().map(str::to_owned)));
    let kind = kind.flatten();
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let caller = principal.user();
    let under = match scope {
        Scope::Under(node) => {
            authorization::require(
                &mut conn,
                &principal,
                node,
                VIEW,
                Act::Use,
                Policy::default(),
            )
            .await?;
            Some(node)
        }
        _ => None,
    };
    let batch = i64::try_from(limit + READ_AHEAD).unwrap_or(i64::MAX);
    let mut kept: Vec<Approval> = Vec::new();
    loop {
        let found = rows::list(
            &mut conn,
            &Listing {
                states: states_of(query.state.unwrap_or_default()),
                kind: kind.as_deref(),
                requested_by: matches!(scope, Scope::Requested).then_some(caller),
                under,
                before,
                limit: batch,
            },
        )
        .await?;
        let exhausted = i64::try_from(found.len()).is_ok_and(|count| count < batch);
        let Some(last) = found.last().map(|row| row.id) else {
            break;
        };
        before = Some(last);
        let (shown, sent) =
            views_routed(&mut conn, &principal, &found, ctx.now, &ctx.app.engine).await?;
        let visible = visible_to(
            &mut conn,
            &principal,
            &scope,
            shown,
            &Routing {
                sent: &sent,
                all: query.all.unwrap_or(false),
            },
        )
        .await?;
        // A request that ran out while nobody looked reads as expired, and is not among those waiting.
        let waiting = query.state.unwrap_or_default() == ApprovalListState::Pending;
        kept.extend(
            visible
                .into_iter()
                .filter(|approval| (approval.state == ApprovalState::Pending) == waiting),
        );
        if kept.len() > limit || exhausted {
            break;
        }
    }
    let more = kept.len() > limit;
    kept.truncate(limit);
    Ok(Page {
        next_cursor: more
            .then(|| kept.last().map(|approval| approval.id.to_string()))
            .flatten(),
        items: kept,
    })
}

/// How many approvals wait in the caller's own list (`scope=mine`, waiting, as sent to them): the
/// number of the Approvals badge. It is counted the way that list is made, so that the badge and the
/// list never differ: a request for access, a port or a capability is counted as a harness's is.
pub(crate) async fn waiting_for(
    conn: &mut Conn,
    principal: &Principal,
    now: chrono::DateTime<chrono::Utc>,
    engine: &orch_engine::Engine,
) -> ApiResult<i64> {
    let mut counted = 0_i64;
    let mut before: Option<Id> = None;
    loop {
        let found = rows::list(
            conn,
            &Listing {
                states: states_of(ApprovalListState::Pending),
                kind: None,
                requested_by: None,
                under: None,
                before,
                limit: WAITING_PER_ROUND,
            },
        )
        .await?;
        let exhausted = i64::try_from(found.len()).is_ok_and(|count| count < WAITING_PER_ROUND);
        let Some(last) = found.last().map(|row| row.id) else {
            return Ok(counted);
        };
        before = Some(last);
        // Boxed: the badge is counted deep in a request's or a feed's stack.
        let (shown, sent) = Box::pin(views_routed(conn, principal, &found, now, engine)).await?;
        let visible = visible_to(
            conn,
            principal,
            &Scope::Mine,
            shown,
            &Routing {
                sent: &sent,
                all: false,
            },
        )
        .await?;
        // A request that ran out while nobody looked reads as expired, as in the list.
        let waiting = visible
            .iter()
            .filter(|approval| approval.state == ApprovalState::Pending)
            .count();
        counted = counted.saturating_add(i64::try_from(waiting).unwrap_or(i64::MAX));
        if exhausted {
            return Ok(counted);
        }
    }
}

/// To whom the requests for access in a list were sent, and whether the list is of everything
/// the caller could decide.
struct Routing<'a> {
    /// The requests for access sent to the caller.
    sent: &'a HashSet<Id>,
    /// Everything the caller could decide, not only what was sent to them.
    all: bool,
}

/// The approvals of a batch that belong in the list of this scope: what the
/// caller can decide or could have decided, for their own list; what they asked for; and for a
/// place, what lies in threads they may read.
async fn visible_to(
    conn: &mut Conn,
    principal: &Principal,
    scope: &Scope,
    shown: Vec<Approval>,
    routing: &Routing<'_>,
) -> ApiResult<Vec<Approval>> {
    match scope {
        Scope::Mine | Scope::Others => {
            let threads: Vec<Id> = shown
                .iter()
                .filter_map(|approval| approval.thread_id)
                .collect();
            let managed: HashSet<Id> = if threads.is_empty() {
                HashSet::new()
            } else {
                nodes::holding(
                    conn,
                    principal,
                    &threads,
                    exact(Permission::ThreadManage),
                    Act::Use,
                    Policy::default(),
                )
                .await
                .map_err(ApiError::internal)?
                .into_iter()
                .map(|listed| listed.id)
                .collect()
            };
            Ok(shown
                .into_iter()
                .filter(|approval| {
                    // A harness's request is sent to whoever manages its thread; a request for a capability to
                    // the people of its scope, which owning the thread does not make one of.
                    if matches!(scope, Scope::Others) {
                        return approval.can_decide && !routing.sent.contains(&approval.id);
                    }
                    (approval.can_decide && (routing.all || routing.sent.contains(&approval.id)))
                        || (approval.state != ApprovalState::Pending
                            && (routing.sent.contains(&approval.id)
                                || (routing.all
                                    && approval.request.kind()
                                        == orch_api_contract::approvals::ApprovalKind::Harness
                                    && approval
                                        .thread_id
                                        .is_some_and(|thread| managed.contains(&thread)))))
                })
                .collect())
        }
        Scope::Requested => Ok(shown),

        Scope::Under(_) => {
            let threads: Vec<Id> = shown
                .iter()
                .filter_map(|approval| approval.thread_id)
                .collect();
            let readable: HashSet<Id> = if threads.is_empty() {
                HashSet::new()
            } else {
                nodes::holding(
                    conn,
                    principal,
                    &threads,
                    exact(Permission::ThreadView),
                    Act::Use,
                    Policy::default(),
                )
                .await
                .map_err(ApiError::internal)?
                .into_iter()
                .map(|listed| listed.id)
                .collect()
            };
            Ok(shown
                .into_iter()
                .filter(|approval| {
                    // The thread a request for access names is why it was made, not where it is
                    // attached.
                    approval.request.kind() == orch_api_contract::approvals::ApprovalKind::Access
                        || approval
                            .thread_id
                            .is_none_or(|thread| readable.contains(&thread))
                })
                .collect())
        }
    }
}

/// One approval as the caller sees it; the caller may be anybody who asked
/// for it, who can decide it, or who may read its thread.
pub(crate) async fn one_approval(ctx: &Ctx, conn: &mut Conn, id: Id) -> ApiResult<Approval> {
    let principal = authorization::principal(ctx, conn).await?;
    let row = rows::approval(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let decides = require_decision_authority(ctx, conn, &principal, &row)
        .await
        .is_ok();
    if row.requester_user_id != Some(principal.user()) && !decides {
        match row.thread_id {
            Some(thread) => {
                authorization::require(
                    conn,
                    &principal,
                    thread,
                    exact(Permission::ThreadView),
                    Act::Use,
                    Policy::default(),
                )
                .await?;
            }
            None => return Err(ApiError::not_found()),
        }
    }
    views(conn, &principal, &[row], ctx.now, &ctx.app.engine)
        .await?
        .into_iter()
        .next()
        .ok_or_else(ApiError::not_found)
}

/// One approval.
async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Approval> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    one_approval(&ctx, &mut conn, id).await
}

/// `409 approval.already_decided`, with the approval as it stands.
pub(crate) async fn already_decided(ctx: &Ctx, id: Id) -> ApiError {
    let mut conn = match ctx.app.db.acquire().await {
        Ok(conn) => conn,
        Err(error) => return error.into(),
    };
    let current = one_approval(ctx, &mut conn, id).await.ok();
    ApiError::new(
        ErrorCode::ApprovalAlreadyDecided,
        "that approval is not waiting any more",
    )
    .with("current", current)
}

/// Decides a request for a capability: by whoever decides on the machine's side, as the scope the policy names
/// says, and by nobody else.
async fn decided_capability(
    ctx: &Ctx,
    tx: orch_db::Tx,
    principal: Principal,
    id: Id,
    body: &ApprovalDecision,
) -> ApiResult<Approval> {
    match ctx
        .app
        .engine
        .decide_capability_guarded(tx, principal.clone(), id, body)
        .await
    {
        Ok(row) => {
            let mut conn = ctx.app.db.acquire().await?;
            views(&mut conn, &principal, &[row], ctx.now, &ctx.app.engine)
                .await?
                .into_iter()
                .next()
                .ok_or_else(ApiError::not_found)
        }
        Err(ApprovalError::NotFound) => Err(ApiError::not_found()),
        Err(ApprovalError::AlreadyDecided) => Err(already_decided(ctx, id).await),
        Err(ApprovalError::Denied(refusal)) => {
            let mut conn = ctx.app.db.acquire().await?;
            Err(refusals::denied(&mut conn, *refusal).await?)
        }
        Err(ApprovalError::Invalid(message)) => Err(ApiError::invalid("", &message)),
        Err(ApprovalError::Internal(error)) => Err(ApiError::internal(error)),
    }
}

/// Decides an approval. The decider's credential and authority are asked again once the body has come, in
/// the transaction that then decides, whichever part raised the approval: a request for a capability is
/// decided on the machine's side, a request for access by whoever could change the page of what it asks for,
/// and an approval of a harness by whoever may manage its thread, as the access engine says for that thread.
/// The single authority check: a harness uses its thread; boundary requests use their own scope.
async fn require_decision_authority(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: &ApprovalRow,
) -> ApiResult<()> {
    use orch_authz::approvals::Authority;
    match orch_authz::approvals::authority(&row.kind) {
        Some(Authority::HarnessThread) => {
            let thread = row.thread_id.ok_or_else(ApiError::not_found)?;
            let permission =
                orch_engine::approvals::deciding_permission(conn, thread, &row.request)
                    .await
                    .map_err(ApiError::internal)?;
            authorization::require(
                conn,
                principal,
                thread,
                exact(permission),
                Act::Use,
                Policy::default(),
            )
            .await
        }
        Some(Authority::CapabilityScope) => {
            if row.approver_scope.as_deref() == Some("environment") {
                return authorization::require_resource(
                    conn,
                    principal,
                    orch_access_contract::permissions::ResourceKind::Environment,
                    row.environment_id.ok_or_else(ApiError::not_found)?,
                    Permission::PortsManage,
                    Policy::default(),
                )
                .await;
            }
            match ctx
                .app
                .engine
                .capability_decision_authority(conn, principal, row)
                .await
                .map_err(ApiError::internal)?
            {
                Ok(()) => Ok(()),
                Err(refusal) => Err(refusals::denied(conn, *refusal).await?),
            }
        }
        Some(Authority::PortEnvironment) => {
            authorization::require_resource(
                conn,
                principal,
                orch_access_contract::permissions::ResourceKind::Environment,
                row.environment_id.ok_or_else(ApiError::not_found)?,
                Permission::PortsManage,
                Policy::default(),
            )
            .await
        }
        Some(Authority::AccessObject) => {
            if ctx
                .app
                .engine
                .access_decision_authority(conn, principal, row)
                .await
                .map_err(ApiError::internal)?
                .may_decide
            {
                Ok(())
            } else {
                Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "only an approver of the requested thing may decide this access request",
                )
                .with("required", "access.manage"))
            }
        }
        Some(Authority::PlanThread | Authority::ModelUseThread) => {
            if ctx
                .app
                .engine
                .may_decide_source_request(conn, principal, row)
                .await
                .map_err(ApiError::internal)?
            {
                Ok(())
            } else {
                Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "Current permission to answer this request is required",
                )
                .with("required", row.required_permission.clone()))
            }
        }
        None => Err(ApiError::new(
            ErrorCode::CapabilityUnsupported,
            "this approval kind has no decision authority",
        )),
    }
}

async fn decide(mut ctx: Ctx, (): (), body: ApprovalDecision) -> ApiResult<ApprovalDecisionReply> {
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.into_principal();
    let row = rows::approval(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if row.state != "pending" {
        drop(tx);
        return Err(already_decided(&ctx, id).await);
    }
    require_decision_authority(&ctx, &mut tx, &principal, &row).await?;
    let kind = row.kind.clone();
    match kind.as_str() {
        "capability" => {
            return decided_capability(&ctx, tx, principal, id, &body)
                .await
                .map(ApprovalDecisionReply::from);
        }
        "access" => {
            return requests::decide(&ctx, tx, principal, id, &body)
                .await
                .map(ApprovalDecisionReply::from);
        }
        // A port request answers with its opening's one result as well.
        "port" => {
            return crate::routes::port_requests::decide(&ctx, tx, principal, id, &body).await;
        }
        "harness" => {}
        "plan" | "input" => {
            let source = row.object_id.ok_or_else(ApiError::not_found)?;
            let thread = row.thread_id.ok_or_else(ApiError::not_found)?;
            let result = if kind == "plan" {
                let ApprovalDecision::Option(choice) = &body else {
                    return Err(ApiError::invalid(
                        "",
                        "a plan is answered with Proceed or Keep planning",
                    ));
                };
                let proceed = match choice.option_id.as_str() {
                    "proceed" => true,
                    "keep_planning" => false,
                    _ => return Err(ApiError::invalid("option_id", "is not a plan response")),
                };
                ctx.app
                    .engine
                    .decide_plan_guarded(
                        tx,
                        principal,
                        thread,
                        source,
                        &orch_api_contract::threads::ThreadPlanDecideBody {
                            proceed,
                            text: choice.note.clone(),
                        },
                    )
                    .await
                    .map(|_| ())
            } else {
                let ApprovalDecision::Input(choice) = &body else {
                    return Err(ApiError::invalid(
                        "",
                        "a held message is answered with Run it",
                    ));
                };
                if !choice.run {
                    return Err(ApiError::invalid(
                        "run",
                        "must be true; cancel a message from its queue",
                    ));
                }
                ctx.app
                    .engine
                    .run_input_guarded(tx, principal, thread, source)
                    .await
                    .map(|_| ())
            };
            return match result {
                Ok(()) => {
                    let mut conn = ctx.app.db.acquire().await?;
                    one_approval(&ctx, &mut conn, id)
                        .await
                        .map(ApprovalDecisionReply::from)
                }
                Err(orch_engine::inputs::InputError::AlreadyDecided(_)) => {
                    Err(already_decided(&ctx, id).await)
                }
                Err(error) => {
                    let mut conn = ctx.app.db.acquire().await?;
                    Err(crate::routes::threads::input_failed(&mut conn, thread, error).await)
                }
            };
        }
        _ => {
            return Err(ApiError::new(
                ErrorCode::CapabilityUnsupported,
                "the approval has an unrecognized kind",
            )
            .with("capability", format!("decide.{kind}")));
        }
    }
    let _ = MANAGE;
    match ctx
        .app
        .engine
        .decide_approval_guarded(tx, principal.clone(), id, &body)
        .await
    {
        Ok(row) => {
            let mut conn = ctx.app.db.acquire().await?;
            views(&mut conn, &principal, &[row], ctx.now, &ctx.app.engine)
                .await?
                .into_iter()
                .next()
                .map(ApprovalDecisionReply::from)
                .ok_or_else(ApiError::not_found)
        }
        Err(ApprovalError::NotFound) => Err(ApiError::not_found()),
        Err(ApprovalError::AlreadyDecided) => Err(already_decided(&ctx, id).await),
        Err(ApprovalError::Denied(refusal)) => {
            let mut conn = ctx.app.db.acquire().await?;
            Err(refusals::denied(&mut conn, *refusal).await?)
        }
        Err(ApprovalError::Invalid(message)) => Err(ApiError::invalid("", &message)),
        Err(ApprovalError::Internal(error)) => Err(ApiError::internal(error)),
    }
}

/// Takes back a request for access, which only the person who asked can do.
async fn cancel(ctx: Ctx, (): (), (): ()) -> ApiResult<Approval> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let person = ctx.user()?.id();
    let row = rows::approval(&mut conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if row.kind != "access" {
        return Err(ApiError::invalid(
            "id",
            "only a request for access is taken back",
        ));
    }
    if row.requester_user_id != Some(person) {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "only the person who asked takes a request back",
        )
        .with("required", "approval.requester")
        .with("object", json!({ "kind": "approval", "id": id }))
        .with("requestable", false)
        .with("override", false)
        .with(
            "missing",
            Vec::<orch_api_contract::access::MissingAccess>::new(),
        ));
    }
    if !rows::cancel_request(&mut conn, id, person, ctx.now).await? {
        drop(conn);
        return Err(already_decided(&ctx, id).await);
    }
    // Nobody has anything left to do about it: what asked them to decide is read.
    orch_db::queries::notifications::inbox::settle_approval(&mut conn, id, ctx.now).await?;
    one_approval(&ctx, &mut conn, id).await
}
