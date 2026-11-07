//! Current native approval governors, shared by HTTP scope admission and continuing work.
//!
//! Approval/source IDs remain exact operands. The subject is the ordinary permission object,
//! never an opaque row ID treated as a node or a path retained from a credential.

use super::deciding_permission;
use crate::Engine;
use crate::operation_scope::{
    BindingError, Prepared,
    targets::{self, TargetError},
};
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyIdentityKind, KeyResolvedTarget, KeyTargetKind},
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    approvals::{ApprovalDecide, ApprovalDecision},
    operations::TargetResolver,
    routes::Endpoint,
};
use orch_db::{
    Conn,
    queries::{approvals::ApprovalRow, environments::capabilities as environments},
};
use orch_types::Id;
use serde_json::json;

/// The stored source selects the operation; a credential never supplies this selector.
pub async fn decision_selection(
    conn: &mut Conn,
    row: &ApprovalRow,
) -> Result<&'static str, crate::EngineError> {
    match row.kind.as_str() {
        "harness"
            if row
                .request
                .get("request_kind")
                .and_then(orch_types::Json::as_str)
                != Some("plan") =>
        {
            Ok("ApprovalDecide.harness")
        }
        "harness" | "plan" => {
            let thread = row
                .thread_id
                .ok_or_else(|| crate::EngineError::unreadable("the plan has no thread"))?;
            let permission =
                deciding_permission(conn, thread, &json!({"request_kind":"plan"})).await?;
            Ok(if permission == Permission::ThreadManage {
                "ApprovalDecide.plan_manage"
            } else {
                "ApprovalDecide.plan_interact"
            })
        }
        "input" => Ok("ApprovalDecide.input"),
        "access" => Ok("ApprovalDecide.access"),
        "capability" => Ok("ApprovalDecide.capability"),
        "port" => Ok("ApprovalDecide.port"),
        _ => Err(crate::EngineError::unreadable(
            "the approval has no native authority kind",
        )),
    }
}
fn environment(row: &ApprovalRow) -> Result<Id, TargetError> {
    row.environment_id.ok_or(TargetError::Missing)
}
async fn resource(
    conn: &mut Conn,
    kind: ObjectKind,
    id: Id,
) -> Result<KeyResolvedTarget, TargetError> {
    targets::resource_identity(conn, ObjectRef { kind, id }).await
}
/// The same capability scope used by the native decision writer. A requesting thread is context,
/// not authority over the environment, Host or actual home org.
pub async fn capability_governor(
    conn: &mut Conn,
    environment: Id,
    scope: &str,
) -> Result<KeyResolvedTarget, TargetError> {
    match scope {
        "environment" => resource(conn, ObjectKind::Environment, environment).await,
        "host" => {
            let host = crate::capabilities::capability_host(conn, environment)
                .await?
                .map(|(id, _)| id)
                .ok_or(TargetError::Missing)?;
            resource(conn, ObjectKind::Host, host).await
        }
        "org" => {
            let facts = environments::environment_facts(conn, environment)
                .await?
                .ok_or(TargetError::Missing)?;
            let org = orch_db::queries::threads::tools::nearest_org(conn, facts.home_node_id)
                .await?
                .unwrap_or(facts.org_id);
            let target = targets::node_identity(conn, org).await?;
            if target.object.kind != KeyTargetKind::Object(ObjectKind::Org) {
                return Err(TargetError::Missing);
            }
            Ok(target)
        }
        "system" => Ok(targets::system()),
        _ => Err(TargetError::Missing),
    }
}
/// Exact current governing identity of a typed stored approval. Its source item/input IDs do not
/// become resource ancestry. Missing/unknown kinds do not fall back to thread authority.
pub async fn governor(
    conn: &mut Conn,
    row: &ApprovalRow,
) -> Result<KeyResolvedTarget, TargetError> {
    match orch_authz::approvals::authority(&row.kind) {
        Some(
            orch_authz::approvals::Authority::HarnessThread
            | orch_authz::approvals::Authority::PlanThread
            | orch_authz::approvals::Authority::ModelUseThread,
        ) => {
            let target =
                targets::node_identity(conn, row.thread_id.ok_or(TargetError::Missing)?).await?;
            if target.object.kind != KeyTargetKind::Object(ObjectKind::Thread) {
                return Err(TargetError::Missing);
            }
            Ok(target)
        }
        Some(orch_authz::approvals::Authority::AccessObject) => {
            if crate::membership_requests::is_join(row) {
                return targets::chat_identity(conn, row.object_id.ok_or(TargetError::Missing)?)
                    .await;
            }
            let object = super::access::request_object(conn, row)
                .await?
                .ok_or(TargetError::Missing)?;
            if object.kind.node().is_some() {
                targets::node_identity(conn, object.id).await
            } else if object.kind.is_document() {
                targets::page_object(conn, object).await
            } else {
                resource(conn, object.kind, object.id).await
            }
        }
        Some(orch_authz::approvals::Authority::PortEnvironment) => {
            resource(conn, ObjectKind::Environment, environment(row)?).await
        }
        Some(orch_authz::approvals::Authority::CapabilityScope) => {
            capability_governor(
                conn,
                environment(row)?,
                row.approver_scope.as_deref().unwrap_or("system"),
            )
            .await
        }
        None => Err(TargetError::Missing),
    }
}
fn role(plan: &Prepared, name: &str) -> Result<usize, BindingError> {
    plan.variant().facets.iter().position(|facet|matches!(facet.target,TargetResolver::Resolved(found)|TargetResolver::ResolvedCollection(found) if found==name)).ok_or(BindingError::Refused)
}
/// Populate only effects. HTTP projection alone adds optional card/source Read facets later;
/// a continuing worker neither emits old card content nor borrows its Read authority.
pub async fn prepare_decision(
    conn: &mut Conn,
    row: &ApprovalRow,
    body: &ApprovalDecision,
) -> Result<Prepared, crate::EngineError> {
    let selection = decision_selection(conn, row).await?;
    let subject = governor(conn, row)
        .await
        .map_err(crate::EngineError::unreadable)?;
    let mut plan =
        Prepared::new(ApprovalDecide::ROUTE, selection).map_err(crate::EngineError::unreadable)?;
    plan.bind(
        role(&plan, "approval_subject").map_err(crate::EngineError::unreadable)?,
        subject.object.kind,
        std::slice::from_ref(&subject),
    )
    .map_err(crate::EngineError::unreadable)?;
    if let Ok(slot) = role(&plan, "approval_capability_grants") {
        let grant = matches!(body,ApprovalDecision::Port(choice) if choice.decision==orch_api_contract::ports::PortDecisionChoice::Grant);
        if grant {
            let environment = environment(row).map_err(crate::EngineError::unreadable)?;
            let card: orch_api_contract::ports::PortRequest =
                serde_json::from_value(row.request.clone())?;
            let gate = crate::port_capabilities::gate(conn, environment, card.request.kind)
                .await
                .map_err(crate::EngineError::unreadable)?;
            let target = capability_governor(conn, environment, &gate.approver_scope)
                .await
                .map_err(crate::EngineError::unreadable)?;
            plan.bind(slot, target.object.kind, &[target])
                .map_err(crate::EngineError::unreadable)?;
        } else {
            plan.bind_empty(slot)
                .map_err(crate::EngineError::unreadable)?;
        }
    }
    if let Ok(slot) = role(&plan, "input_native_manage") {
        let input = match row.object_id {
            Some(id) => orch_db::queries::threads::input(conn, id).await?,
            None => None,
        }
        .filter(|input| Some(input.thread_id) == row.thread_id)
        .ok_or_else(|| {
            crate::EngineError::unreadable("the held-input approval has no actual input")
        })?;
        let permission = if let Some(raw) = &input.native_action {
            let selected = serde_json::from_value(raw.clone())?;
            let delivery = match input.delivery.as_str() {
                "steer" => orch_api_contract::threads::Delivery::Steer,
                "interrupt" => orch_api_contract::threads::Delivery::Interrupt,
                _ => orch_api_contract::threads::Delivery::Queue,
            };
            Some(
                crate::native_actions::selected_action(conn, input.thread_id, &selected, delivery)
                    .await
                    .map_err(crate::EngineError::unreadable)?
                    .permission,
            )
        } else {
            None
        };
        if permission == Some(orch_api_contract::native::NativePermission::Manage) {
            plan.bind(
                slot,
                KeyTargetKind::Object(ObjectKind::Thread),
                &[subject.clone()],
            )
            .map_err(crate::EngineError::unreadable)?;
        } else {
            plan.bind_empty(slot)
                .map_err(crate::EngineError::unreadable)?;
        }
    }
    Ok(plan)
}

/// Refresh the same credential and current account facts after the native writer's actual
/// wait. Its original ceiling and second-step hold remain; no session or administrator mode
/// is manufactured for a key. Failure is ordinary refusal, not authorization to fall back.
pub(crate) async fn current_principal(
    engine: &Engine,
    conn: &mut Conn,
    original: &orch_authz::Principal,
    row: &ApprovalRow,
    body: &ApprovalDecision,
) -> Result<Option<orch_authz::Principal>, crate::EngineError> {
    let Some(mut caller) = orch_authz::CapturedCaller::capture(original.caller())
        .restore_current(conn, engine.inner.clock.as_ref())
        .await?
    else {
        return Ok(None);
    };
    if caller.grant().is_some() {
        let plan = prepare_decision(conn, row, body).await?;
        if caller
            .grant()
            .is_some_and(|grant| grant.operation_scope().is_some())
            && original.caller().admitted_operation() != Some(plan.variant().operation.as_str())
        {
            return Ok(None);
        }
        caller = match plan.admit(&caller, false) {
            Ok(caller) => caller,
            Err(BindingError::Refused) => return Ok(None),
            Err(error) => return Err(crate::EngineError::unreadable(error)),
        };
    }
    let current = orch_authz::Principal::load(
        conn,
        caller,
        orch_types::Time::new(engine.inner.clock.now()),
    )
    .await?;
    if !current
        .caller()
        .key_deadline_allows(engine.inner.clock.as_ref())
    {
        return Ok(None);
    }
    Ok(Some(match original.second_step_hold() {
        Some(hold) => current.held_for_second_step(hold.clone()),
        None => current,
    }))
}

/// Current native facts and serialized footprint, never authorization or wire input.
/// Only the capability path carries its existing materialized application choice.
pub struct DecisionLocks {
    row: ApprovalRow,
    pub(crate) capability: crate::capabilities::PreparedCapabilityGrant,
    pub(crate) body: ApprovalDecision,
}
impl DecisionLocks {
    pub fn row(&self) -> &ApprovalRow {
        &self.row
    }
    pub(crate) fn into_parts(
        self,
    ) -> (
        ApprovalRow,
        crate::capabilities::PreparedCapabilityGrant,
        ApprovalDecision,
    ) {
        (self.row, self.capability, self.body)
    }
}

/// Serialize the existing native decision's first-effect footprint in its native order.
/// API calls its full HTTP checkpoint afterwards; native callers still apply their ordinary
/// authority and body checks. A lock is never permission and this prepares no external effect.
pub async fn lock_decision(
    engine: &Engine,
    conn: &mut Conn,
    id: Id,
    body: &ApprovalDecision,
) -> Result<Option<DecisionLocks>, crate::EngineError> {
    use orch_db::queries::{approvals as rows, chats, ports, threads};
    let Some(initial) = rows::approval(conn, id).await? else {
        return Ok(None);
    };
    if initial.state != "pending" {
        return Ok(Some(DecisionLocks {
            row: initial,
            capability: crate::capabilities::PreparedCapabilityGrant::None,
            body: body.clone(),
        }));
    }
    match orch_authz::approvals::authority(&initial.kind) {
        Some(
            orch_authz::approvals::Authority::HarnessThread
            | orch_authz::approvals::Authority::PlanThread
            | orch_authz::approvals::Authority::ModelUseThread,
        ) => {
            let Some(thread) = initial.thread_id else {
                return Ok(None);
            };
            if threads::lock(conn, thread).await?.is_none() {
                return Ok(None);
            }
        }
        Some(orch_authz::approvals::Authority::AccessObject)
            if crate::membership_requests::is_join(&initial) =>
        {
            let Some(chat) = initial.object_id else {
                return Ok(None);
            };
            // The membership writer and its expiry path already use Chat, then Approval.
            if chats::lock(conn, chat).await?.is_none() {
                return Ok(None);
            }
        }
        Some(orch_authz::approvals::Authority::PortEnvironment) => {
            let ApprovalDecision::Port(decision) = body else {
                return Ok(Some(DecisionLocks {
                    row: initial,
                    capability: crate::capabilities::PreparedCapabilityGrant::None,
                    body: body.clone(),
                }));
            };
            let Some(opening) = ports::opening_of_approval(conn, id).await? else {
                return Ok(None);
            };
            if decision.decision != orch_api_contract::ports::PortDecisionChoice::Deny {
                if !ports::lock_environment(conn, opening.environment_id).await? {
                    return Ok(None);
                }
                let port = u16::try_from(opening.port).map_err(crate::EngineError::unreadable)?;
                ports::lock_target(conn, opening.environment_id, port).await?;
                if opening.kind == "host"
                    && let Ok(card) = serde_json::from_value::<orch_api_contract::ports::PortRequest>(
                        initial.request.clone(),
                    )
                    && card.request.kind == orch_api_contract::ports::PortOpeningKind::Host
                    && let Some(candidate) = card.plan.candidate
                {
                    // The native Host writer validates this exact saved candidate, then updates
                    // the opening and Approval. Keep Machine→Opening→Approval in that same order.
                    // Malformed/missing candidates keep the existing native refusal below.
                    ports::lock_publisher_machine(conn, candidate.machine.id).await?;
                }
            }
            // Deny has only the opening/refusal path; it receives no invented Env/target lock.
            if !ports::lock_opening(conn, opening.id).await? {
                return Ok(None);
            }
        }
        Some(
            orch_authz::approvals::Authority::CapabilityScope
            | orch_authz::approvals::Authority::AccessObject,
        ) => {}
        None => {
            return Ok(Some(DecisionLocks {
                row: initial,
                capability: crate::capabilities::PreparedCapabilityGrant::None,
                body: body.clone(),
            }));
        }
    }
    let Some(current) = rows::lock(conn, id).await? else {
        return Ok(None);
    };
    if current.kind != initial.kind
        || current.thread_id != initial.thread_id
        || current.environment_id != initial.environment_id
        || current.object_id != initial.object_id
        || current.object_kind != initial.object_kind
    {
        return Err(crate::EngineError::unreadable(
            "the approval's immutable source identity changed",
        ));
    }
    if current.kind == "port" && current.request != initial.request {
        // The publisher lock came from this exact saved card. A refreshed candidate must
        // start a fresh preparation instead of acquiring another late publisher after Guard.
        return Err(orch_db::DbError::Retry.into());
    }
    if current.kind == "access"
        && !crate::membership_requests::is_join(&current)
        && body
            .as_access_decision()
            .is_some_and(|decision| decision.approve)
        && current.state == "pending"
    {
        if let Some(object) = super::access::request_object(conn, &current).await? {
            let target = if object.kind.node().is_some() {
                orch_authz::pages::Target::Live(object.id)
            } else {
                orch_authz::pages::Target::Resource(object)
            };
            // Reuse the exact page writer prefix, including a document's container scope.
            // Decline never grants a page and therefore takes none of these locks.
            orch_authz::pages::lock_change_target(conn, target).await?;
        }
    }
    let mut capability = if current.kind == "capability"
        && current.state == "pending"
        && matches!(body, ApprovalDecision::Capability(choice) if choice.approve)
    {
        if let (Some(environment), Some(key)) = (
            current.environment_id,
            current
                .request
                .get("capability")
                .and_then(orch_types::Json::as_str),
        ) {
            engine
                .prepare_capability_grant(conn, environment, key)
                .await?
        } else {
            crate::capabilities::PreparedCapabilityGrant::None
        }
    } else {
        crate::capabilities::PreparedCapabilityGrant::None
    };
    if current.kind == "capability" && current.state == "pending" {
        match body {
            ApprovalDecision::Capability(choice) if choice.approve => {
                // Stored grants only: freeze the same marker source, locking its Thread
                // only when its existing Some-segment path will stamp an item.
                capability.prepare_marker(conn, current.thread_id).await?;
            }
            ApprovalDecision::Capability(_) => {
                // Native denial's system_input unconditionally locks this exact Thread.
                if let Some(thread) = current.thread_id {
                    let _ = threads::lock(conn, thread).await?;
                }
            }
            _ => {}
        }
    }
    if current.state == "pending" && matches!(current.kind.as_str(), "plan" | "input") {
        if let (Some(thread), Some(source)) = (current.thread_id, current.object_id) {
            let source = if current.kind == "plan" {
                orch_db::queries::notifications::inbox::RequestSource::Plan(source)
            } else {
                orch_db::queries::notifications::inbox::RequestSource::Input(source)
            };
            // Share the direct source writer's exact late recipient/Inbox prefix too.
            // Approval keeps its own operation/actor; this is only actual native locks.
            engine
                .lock_source_control(conn, thread, source, true)
                .await?;
        }
    }
    Ok(Some(DecisionLocks {
        row: current,
        capability,
        body: body.clone(),
    }))
}

/// Preserve the capability decision's explicit native system-admin alternative.
/// This returns eligibility only; it never creates an administrator Principal or lets
/// this credential use a different resource/private-inspection operation.
pub(crate) async fn capability_administrator(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    row: &ApprovalRow,
) -> Result<bool, crate::EngineError> {
    use orch_access_contract::key_scope::{KeyAction, KeyFacet};
    if principal.is_system_admin() {
        return Ok(true);
    }
    let caller = principal.caller();
    if !principal.has_system_admin_standing()
        || row.kind != "capability"
        || caller.as_admin()
        || caller.grant().is_none_or(|grant| {
            grant.operation_scope().is_none() || grant.requires_legacy_compatibility()
        })
    {
        return Ok(false);
    }
    let Some(selection) = caller.admitted_operation() else {
        return Ok(false);
    };
    let target = governor(conn, row)
        .await
        .map_err(crate::EngineError::unreadable)?;
    let permits = |action, resource_kind| {
        caller.permits_facets(&[KeyFacet {
            operation: selection.to_owned(),
            action,
            resource_kind,
            target: target.clone(),
        }])
    };
    // Read eligibility is only preparation of the common card. A native decision still
    // passes the same complete required-effect producer before any write.
    Ok(match selection {
        "ApprovalGet" | "ApprovalList" => permits(
            KeyAction::Read,
            KeyTargetKind::Identity(KeyIdentityKind::Approval),
        ),
        "ApprovalDecide.capability" => {
            permits(KeyAction::Manage, target.object.kind)
                || permits(
                    KeyAction::Read,
                    KeyTargetKind::Identity(KeyIdentityKind::Approval),
                )
        }
        _ => false,
    })
}
