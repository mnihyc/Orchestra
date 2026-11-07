//! Scope the existing common approval producer, without changing its ordinary read or decision doors.
use super::*;
use crate::key_operations::{admission, resolver::Disclosure, targets};
use crate::registry::{OperationPlan, OptionalOperationFuture};
use orch_access_contract::{
    decisions::ObjectRef as NativeObject,
    key_scope::{KeyAction, KeyIdentityKind, KeyResolvedTarget, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{
    operations::TargetResolver,
    ports::{PortOpening, PortTarget},
};
use orch_authz::Caller;
use orch_engine::operation_scope::targets::TargetError;
use std::collections::BTreeSet;

const APPROVAL: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Approval);
const OBJECTS: [(ObjectKind, &str); 15] = [
    (ObjectKind::Org, "approval_nodes_orgs"),
    (ObjectKind::Project, "approval_nodes_projects"),
    (ObjectKind::Topic, "approval_nodes_topics"),
    (ObjectKind::Thread, "approval_nodes_threads"),
    (ObjectKind::Document, "approval_documents"),
    (
        ObjectKind::DocumentContainer,
        "approval_document_containers",
    ),
    (ObjectKind::Provider, "approval_providers"),
    (ObjectKind::Agent, "approval_agents"),
    (ObjectKind::Environment, "approval_environments"),
    (ObjectKind::Host, "approval_hosts"),
    (ObjectKind::Machine, "approval_machines"),
    (ObjectKind::Template, "approval_templates"),
    (ObjectKind::McpServer, "approval_mcp_servers"),
    (ObjectKind::Mount, "approval_mounts"),
    (ObjectKind::Skill, "approval_skills"),
];
fn scoped(caller: &Caller) -> bool {
    caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
}
pub(super) fn is_scoped(ctx: &Ctx) -> bool {
    ctx.admitted_caller().is_some_and(scoped)
}
fn denied() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the key does not allow this approval operation",
    )
}
fn role(plan: &OperationPlan, name: &str) -> ApiResult<usize> {
    plan.variant().facets.iter().position(|facet|matches!(facet.target,TargetResolver::Resolved(found)|TargetResolver::ResolvedCollection(found) if found==name)).ok_or_else(||ApiError::internal("the approval source role is undeclared"))
}
fn bind(
    plan: &mut OperationPlan,
    name: &str,
    kind: KeyTargetKind,
    values: &[KeyResolvedTarget],
) -> ApiResult<()> {
    let slot = role(plan, name)?;
    if values.is_empty() {
        plan.bind_empty(slot)
    } else {
        plan.bind(slot, kind, values)
    }
}
pub(crate) async fn governor(conn: &mut Conn, row: &ApprovalRow) -> ApiResult<KeyResolvedTarget> {
    match orch_engine::approvals::operation::governor(conn, row).await {
        Ok(target) => Ok(target),
        Err(TargetError::Missing) => Err(ApiError::not_found()),
        Err(TargetError::Db(error)) => Err(error.into()),
        Err(TargetError::Authz(error)) => Err(refusals::problem(conn, error).await),
    }
}
fn read_plan(ctx: &Ctx, target: &KeyResolvedTarget) -> ApiResult<OperationPlan> {
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    bind(
        &mut plan,
        "approval_scope",
        APPROVAL,
        std::slice::from_ref(target),
    )?;
    Ok(plan)
}
async fn effect_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    row: &ApprovalRow,
    body: Option<&ApprovalDecision>,
) -> ApiResult<OperationPlan> {
    let target = governor(conn, row).await?;
    let mut plan = if let Some(body) = body {
        OperationPlan::from_shared(
            orch_engine::approvals::operation::prepare_decision(conn, row, body)
                .await
                .map_err(ApiError::engine)?,
        )
    } else {
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        bind(
            &mut plan,
            "approval_subject",
            APPROVAL,
            std::slice::from_ref(&target),
        )?;
        plan
    };
    bind(&mut plan, "approval_scope", APPROVAL, &[target])?;
    Ok(plan)
}
pub(super) fn get<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !scoped(caller) {
            return Ok(None);
        }
        let row = rows::approval(conn, ctx.id("id")?)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let target = governor(conn, &row).await?;
        Ok(Some(read_plan(ctx, &target)?))
    })
}
pub(super) fn decide<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: ApprovalDecision,
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !scoped(caller) {
            return Ok(None);
        }
        let row = rows::approval(conn, ctx.id("id")?)
            .await?
            .ok_or_else(ApiError::not_found)?;
        Ok(Some(effect_plan(ctx, conn, &row, Some(&body)).await?))
    })
}
pub(super) fn cancel<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !scoped(caller) {
            return Ok(None);
        }
        let row = rows::approval(conn, ctx.id("id")?)
            .await?
            .ok_or_else(ApiError::not_found)?;
        Ok(Some(effect_plan(ctx, conn, &row, None).await?))
    })
}
/// Find a real scope-covered candidate only to prepare the ordinary effect-free list.
/// If no candidate matches, this has fully scanned to factual exhaustion before empty admission.
async fn list_principal(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &ApprovalListQuery,
) -> ApiResult<Principal> {
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    let scope = scope_of(query.scope.as_deref())?;
    let under = match scope {
        Scope::Under(id) => Some(id),
        _ => None,
    };
    let kind = query
        .kind
        .map(serde_json::to_value)
        .transpose()
        .map_err(ApiError::internal)?;
    let kind = kind.as_ref().and_then(Json::as_str);
    let mut before = query
        .cursor
        .as_deref()
        .map(|cursor| {
            cursor
                .parse::<Id>()
                .map_err(|_| ApiError::invalid("cursor", "is not a cursor of this list"))
        })
        .transpose()?;
    let batch = i64::from(
        PageQuery {
            limit: query.limit,
            cursor: None,
        }
        .limit(),
    ) + i64::try_from(READ_AHEAD).map_err(ApiError::internal)?;
    loop {
        let found = rows::list(
            conn,
            &Listing {
                states: states_of(query.state.unwrap_or_default()),
                kind,
                requested_by: matches!(scope, Scope::Requested).then_some(caller.user()),
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
        if before.is_some_and(|old| last >= old) {
            return Err(ApiError::internal(
                "the native approval cursor did not advance",
            ));
        }
        before = Some(last);
        for row in &found {
            let target = match governor(conn, row).await {
                Ok(target) => target,
                Err(error) if error.code() == ErrorCode::NotFound => continue,
                Err(error) => return Err(error),
            };
            if plan.permits_binding(caller, 0, APPROVAL, std::slice::from_ref(&target))? {
                plan.bind(0, APPROVAL, &[target])?;
                return plan.preparation_principal(ctx, conn, caller).await;
            }
        }
        if exhausted {
            break;
        }
    }
    plan.bind_empty(0)?;
    plan.preparation_principal(ctx, conn, caller).await
}
pub(super) fn list<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: ApprovalListQuery,
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !scoped(caller) {
            return Ok(None);
        }
        // A known current candidate proves only this preparation. No candidate is presented
        // or used as effect authority; the final reader binds every actual returned row.
        let principal = list_principal(ctx, conn, caller, &query).await?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        if let Some(facets) = principal.caller().admitted_facets() {
            let targets: Vec<_> = facets
                .iter()
                .filter(|facet| facet.resource_kind == APPROVAL)
                .map(|facet| facet.target.clone())
                .collect();
            if targets.is_empty() {
                plan.bind_empty(0)?;
            } else {
                plan.bind(0, APPROVAL, &targets)?;
            }
        } else {
            plan.bind_empty(0)?;
        }
        Ok(Some(plan))
    })
}

#[derive(Default)]
struct Sources {
    objects: BTreeSet<(ObjectKind, Id)>,
    chats: BTreeSet<Id>,
}
impl Sources {
    fn object(&mut self, kind: ObjectKind, id: Id) {
        if !id.is_nil() {
            self.objects.insert((kind, id));
        }
    }
    fn node(&mut self, node: &orch_api_contract::nodes::NodeRef) {
        if node.name.is_some() || node.thread_display.is_some() {
            let kind = match node.kind {
                orch_api_contract::nodes::NodeKind::Org => ObjectKind::Org,
                orch_api_contract::nodes::NodeKind::Project => ObjectKind::Project,
                orch_api_contract::nodes::NodeKind::Topic => ObjectKind::Topic,
                orch_api_contract::nodes::NodeKind::Thread => ObjectKind::Thread,
            };
            self.object(kind, node.id);
        }
    }
    fn opening(&mut self, opening: &PortOpening) {
        self.object(ObjectKind::Environment, opening.environment_id);
        if let Some(node) = &opening.place {
            self.node(node);
        }
        if let Some(node) = &opening.requested_by_thread {
            self.node(node);
        }
        if let Some(scope) = opening
            .admission
            .as_ref()
            .and_then(|admission| admission.scope.as_ref())
        {
            self.node(scope);
        }
        if let Some(host) = &opening.host {
            self.object(ObjectKind::Host, host.id);
        }
        if let Some(machine) = &opening.publisher {
            self.object(ObjectKind::Machine, machine.id);
        }
    }
    fn port_target(&mut self, target: &PortTarget) {
        self.object(ObjectKind::Environment, target.environment_id);
        for opening in &target.openings {
            self.opening(opening);
        }
    }
    fn approval(&mut self, approval: &Approval) -> ApiResult<()> {
        if let Some(node) = &approval.node {
            self.node(node);
        }
        if let ApprovalRequester::Thread(thread) = &approval.requester {
            if !thread.name.is_empty() {
                self.object(ObjectKind::Thread, thread.id);
            }
        }
        if let Some(object) = &approval.object {
            if !object.name.is_empty() {
                if object.kind == "chat" {
                    self.chats.insert(object.id);
                } else {
                    let kind = crate::routes::access::subject::kind_of_word(&object.kind)
                        .ok_or_else(|| {
                            ApiError::internal("the approval has an untyped named object")
                        })?;
                    self.object(kind, object.id);
                }
            }
        }
        match &approval.request {
            ApprovalRequest::Capability(request) => {
                self.object(ObjectKind::Environment, request.environment.id);
                self.object(ObjectKind::Host, request.host.id);
            }
            ApprovalRequest::Access(request) => {
                if let Some(node) = &request.for_thread {
                    self.node(node);
                }
                match &request.holder {
                    orch_api_contract::access::Holder::User { .. } => {}
                    orch_api_contract::access::Holder::Node { node } => self.node(node),
                    // The native access-request producer accepts selected people or work only.
                    // A later producer extension must declare its actual Group/role sources;
                    // the broad shared Holder type is not an implicit disclosure grant.
                    orch_api_contract::access::Holder::Members { .. }
                    | orch_api_contract::access::Holder::Role { .. }
                    | orch_api_contract::access::Holder::Group { .. }
                    | orch_api_contract::access::Holder::Agents { .. }
                    | orch_api_contract::access::Holder::Everyone => {
                        return Err(ApiError::internal(
                            "the native approval has an undeclared holder source",
                        ));
                    }
                }
            }
            ApprovalRequest::Port(request) => {
                self.object(ObjectKind::Environment, request.environment.id);
                self.opening(&request.opening);
                self.port_target(&request.plan.target);
                if let Some(node) = &request.plan.audience.place {
                    self.node(node);
                }
                if let Some(candidate) = &request.plan.candidate {
                    if let Some(host) = &candidate.host {
                        self.object(ObjectKind::Host, host.id);
                    }
                    self.object(ObjectKind::Machine, candidate.machine.id);
                }
            }
            ApprovalRequest::Harness(_) | ApprovalRequest::Plan(_) | ApprovalRequest::Input(_) => {}
        }
        Ok(())
    }
    fn reply(reply: &ApprovalDecisionReply) -> ApiResult<Self> {
        let mut sources = Self::default();
        sources.approval(&reply.approval)?;
        if let Some(result) = &reply.opening_result {
            sources.opening(&result.opening);
        }
        Ok(sources)
    }
}
async fn bind_sources(
    conn: &mut Conn,
    plan: &mut OperationPlan,
    sources: Sources,
) -> ApiResult<()> {
    let typed = crate::routes::nodes::read_sources::Sources::from_objects(
        sources
            .objects
            .iter()
            .filter(|(kind, _)| {
                kind.node().is_some() || kind.is_document() || *kind == ObjectKind::Machine
            })
            .map(|&(kind, id)| NativeObject { kind, id }),
    );
    let mut targets = typed.targets(conn).await?;
    // A retained native card can contain a resource reference after its unit is removed.
    // Its stored current identity/home is still required; this is scope attribution only,
    // and does not add an ordinary Host/Env/etc View prerequisite to the shared card.
    for &(kind, id) in &sources.objects {
        if kind.node().is_none() && !kind.is_document() && kind != ObjectKind::Machine {
            let object = NativeObject { kind, id };
            let target = match orch_engine::operation_scope::targets::resource_identity(
                conn, object,
            )
            .await
            {
                Ok(target) => target,
                Err(TargetError::Missing) => return Err(ApiError::not_found()),
                Err(TargetError::Db(error)) => return Err(error.into()),
                Err(TargetError::Authz(error)) => return Err(refusals::problem(conn, error).await),
            };
            targets.insert(object, target);
        }
    }
    if targets
        .keys()
        .any(|object| !OBJECTS.iter().any(|(kind, _)| *kind == object.kind))
    {
        return Err(ApiError::internal(
            "the approval projection has an undeclared typed source",
        ));
    }
    for (kind, name) in OBJECTS {
        let values: Vec<_> = targets
            .iter()
            .filter(|(object, _)| object.kind == kind)
            .map(|(_, target)| target.clone())
            .collect();
        bind(plan, name, KeyTargetKind::Object(kind), &values)?;
    }
    let mut chats = Vec::new();
    for chat in sources.chats {
        chats.push(targets::chat_identity(conn, chat).await?);
    }
    bind(
        plan,
        "approval_chats",
        KeyTargetKind::Identity(KeyIdentityKind::Chat),
        &chats,
    )
}
/// Reuse the actual native common-card source walk in request-batch producers too.
pub(crate) async fn bind_card_sources(
    conn: &mut Conn,
    plan: &mut OperationPlan,
    cards: &[Approval],
) -> ApiResult<()> {
    let mut sources = Sources::default();
    for card in cards {
        sources.approval(card)?;
    }
    bind_sources(conn, plan, sources).await
}

/// The exact returned card is either disclosed in full under its real sources, or omitted.
/// No literal/configuration text is searched or rewritten to pretend a reference disappeared.
pub(super) async fn selected_cards(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    approvals: Vec<Approval>,
) -> ApiResult<Vec<Approval>> {
    let mut selected = Vec::new();
    for approval in approvals {
        let row = rows::approval(conn, approval.id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let target = match governor(conn, &row).await {
            Ok(target) => target,
            Err(error) if error.code() == ErrorCode::NotFound => continue,
            Err(error) => return Err(error),
        };
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        if !plan.permits_binding(caller, 0, APPROVAL, std::slice::from_ref(&target))? {
            continue;
        }
        plan.bind(0, APPROVAL, &[target])?;
        let mut sources = Sources::default();
        sources.approval(&approval)?;
        bind_sources(conn, &mut plan, sources).await?;
        if plan.shared().response_coverage(caller) == Some(true) {
            selected.push(approval);
        }
    }
    Ok(selected)
}
async fn current_caller(
    ctx: &Ctx,
    conn: &mut Conn,
) -> ApiResult<crate::middleware::transaction_guard::CurrentHttpIdentity> {
    let accepted = ctx.admitted_caller().ok_or_else(denied)?;
    admission::response_caller(
        ctx,
        conn,
        accepted,
        &ctx.route.name,
        Time::new(ctx.app.clock.now()),
    )
    .await?
    .ok_or_else(denied)
}
pub(super) async fn get_reply(ctx: Ctx) -> ApiResult<Approval> {
    crate::key_operations::snapshot_read::run(ctx, |ctx| async move {
        let mut tx = ctx.app.db.begin_snapshot().await?;
        let identity = current_caller(&ctx, &mut tx).await?;
        let ctx = ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
        let caller = identity.caller.clone();
        let row = rows::approval(&mut tx, ctx.id("id")?)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let target = governor(&mut tx, &row).await?;
        let mut plan = read_plan(&ctx, &target)?;
        let principal = plan.preparation_principal(&ctx, &mut tx, &caller).await?;
        let ctx = ctx.with_admitted_caller(principal.caller().clone())?;
        let approval = super::one_approval_in(&ctx, &mut tx, &principal, row).await?;
        let mut sources = Sources::default();
        sources.approval(&approval)?;
        bind_sources(&mut tx, &mut plan, sources).await?;
        let shown = plan
            .read_response(&ctx, &mut tx, principal.caller(), move |_, _| {
                Box::pin(async move { Ok(approval) })
            })
            .await?
            .ok_or_else(denied)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(shown)
    })
    .await
}
pub(super) async fn list_reply(ctx: Ctx, query: ApprovalListQuery) -> ApiResult<Page<Approval>> {
    crate::key_operations::snapshot_read::run(ctx, move |ctx| {
        let query = query.clone();
        async move {
            let mut tx = ctx.app.db.begin_snapshot().await?;
            let identity = current_caller(&ctx, &mut tx).await?;
            let ctx = ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
            let caller = identity.caller.clone();
            let principal = list_principal(&ctx, &mut tx, &caller, &query).await?;
            let ctx = ctx.with_admitted_caller(principal.caller().clone())?;
            let page = super::list_in(&ctx, &mut tx, &principal, &query, Some(&caller)).await?;
            let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
            let mut governors = Vec::new();
            let mut sources = Sources::default();
            for approval in &page.items {
                let row = rows::approval(&mut tx, approval.id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                governors.push(governor(&mut tx, &row).await?);
                sources.approval(approval)?;
            }
            bind(&mut plan, "approval_scopes", APPROVAL, &governors)?;
            bind_sources(&mut tx, &mut plan, sources).await?;
            let shown = plan
                .read_response(&ctx, &mut tx, principal.caller(), move |_, _| {
                    Box::pin(async move { Ok(page) })
                })
                .await?
                .ok_or_else(denied)?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            Ok(shown)
        }
    })
    .await
}
/// Safe accepted facts from the ordinary writer. Protected card projection is separate,
/// so a failed optional read cannot masquerade as an uncommitted decision.
pub(super) struct Outcome {
    pub(super) receipt: orch_api_contract::approvals::ApprovalWriteReceipt,
    opening_result: Option<orch_api_contract::ports::PortOpenReply>,
}
impl Outcome {
    pub(super) fn row(row: &ApprovalRow) -> ApiResult<Self> {
        let state = serde_json::from_value(json!(row.state)).map_err(ApiError::internal)?;
        Ok(Self {
            receipt: orch_api_contract::approvals::ApprovalWriteReceipt {
                projection: orch_api_contract::common::ReceiptProjection::Receipt,
                approval_id: row.id,
                state,
                decided_by: row.decided_by,
                decided_at: row.decided_at.map(Time::new),
                opening_id: None,
                link: None,
            },
            opening_result: None,
        })
    }
    pub(super) fn reply(reply: ApprovalDecisionReply) -> Self {
        let receipt = orch_api_contract::approvals::ApprovalWriteReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            approval_id: reply.id,
            state: reply.state,
            decided_by: reply.decided_by.as_ref().map(|user| user.id),
            decided_at: reply.decided_at,
            opening_id: reply
                .opening_result
                .as_ref()
                .map(|result| result.opening.id),
            link: reply
                .opening_result
                .as_ref()
                .and_then(|result| result.link.clone()),
        };
        Self {
            receipt,
            opening_result: reply.opening_result,
        }
    }
    pub(super) fn source(
        id: Id,
        status: &orch_api_contract::approvals::RequestDecisionStatus,
    ) -> Self {
        Self {
            receipt: orch_api_contract::approvals::ApprovalWriteReceipt {
                projection: orch_api_contract::common::ReceiptProjection::Receipt,
                approval_id: id,
                state: status.state,
                decided_by: status.decided_by.as_ref().map(|user| user.id),
                decided_at: Some(status.decided_at),
                opening_id: None,
                link: None,
            },
            opening_result: None,
        }
    }
    /// The run writer reports success only after accepting this exact held-input approval.
    /// Its actor/state are known; an unreturned decision timestamp stays absent, never invented.
    pub(super) fn input(id: Id, actor: Id) -> Self {
        Self {
            receipt: orch_api_contract::approvals::ApprovalWriteReceipt {
                projection: orch_api_contract::common::ReceiptProjection::Receipt,
                approval_id: id,
                state: ApprovalState::Approved,
                decided_by: Some(actor),
                decided_at: None,
                opening_id: None,
                link: None,
            },
            opening_result: None,
        }
    }
}
/// A private, effect-free Read-root preparation. The declared current approval Read
/// is checked first; every additional actual source is still bound and admitted before
/// serialization. This principal never enters a native mutation or captured work.
/// The same exact-operation common-card Read root, reused by its batch producers.
/// This caller is only for effect-free projection and is never captured for a write.
pub(crate) fn card_read_caller(
    caller: &Caller,
    plan: &OperationPlan,
    name: &'static str,
    targets: &[KeyResolvedTarget],
) -> ApiResult<Option<Caller>> {
    let slot = role(plan, name)?;
    let metadata = &plan.variant().facets[slot];
    if metadata.action != KeyAction::Read || metadata.resources != [APPROVAL] {
        return Err(ApiError::internal(
            "the common card Read role metadata changed",
        ));
    }
    if !plan.permits_binding(caller, slot, APPROVAL, targets)? {
        return Ok(None);
    }
    let facets = orch_engine::operation_scope::facets(
        &plan.variant().operation,
        metadata,
        APPROVAL,
        targets,
    )
    .map_err(crate::key_operations::admission::binding_error)?;
    Ok(Some(
        caller
            .admit_facets(&facets)
            .map_err(|_| denied())?
            .into_caller(),
    ))
}

async fn projection_principal(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    plan: &OperationPlan,
    target: &KeyResolvedTarget,
) -> ApiResult<Option<Principal>> {
    let Some(caller) =
        card_read_caller(caller, plan, "approval_scope", std::slice::from_ref(target))?
    else {
        return Ok(None);
    };
    let principal =
        authorization::load_principal(conn, caller, Time::new(ctx.app.clock.now())).await?;
    Ok(Some(
        crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await?,
    ))
}
async fn decision_projection_in(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: ApprovalRow,
    opening_result: Option<orch_api_contract::ports::PortOpenReply>,
) -> ApiResult<ApprovalDecisionReply> {
    let approval = super::one_approval_in(ctx, conn, principal, row).await?;
    let opening_result = if let Some(result) = opening_result {
        // The ordinary opening read door stays unchanged; its intended proof is never re-read.
        let opening =
            crate::routes::ports::read_opening_in(ctx, conn, principal, result.opening.id).await?;
        Some(orch_api_contract::ports::PortOpenReply {
            opening,
            request: result.request,
            link: result.link,
        })
    } else {
        None
    };
    Ok(ApprovalDecisionReply {
        approval,
        opening_result,
    })
}

async fn native_accepted_projection(
    ctx: Ctx,
    original: Caller,
    id: Id,
    opening_result: Option<orch_api_contract::ports::PortOpenReply>,
) -> ApiResult<Option<ApprovalDecisionReply>> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let mut identity = crate::middleware::transaction_guard::current_http_identity(
        &ctx,
        &mut tx,
        crate::middleware::transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    identity.caller = identity
        .caller
        .intersect_restrictions(&original)
        .map_err(ApiError::internal)?;
    identity.user.as_admin = identity.caller.as_admin();
    let current = ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
    // A newly stored rule-mode key cannot borrow an old unclassified native Full path.
    if scoped(&identity.caller) {
        return Ok(None);
    }
    // Keep errors private through the snapshot commit and final shared lifetime cut too.
    let result = async {
        let principal = authorization::admitted_principal(
            &mut tx,
            &identity.caller,
            &identity.user,
            identity.now,
        )
        .await?;
        let row = rows::approval(&mut tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let value =
            decision_projection_in(&current, &mut tx, &principal, row, opening_result).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Some(value))
    }
    .await;
    identity.check_deadline(&ctx.app)?;
    result
}

async fn accepted_projection(
    ctx: Ctx,
    original: Caller,
    id: Id,
    opening_result: Option<orch_api_contract::ports::PortOpenReply>,
) -> ApiResult<Option<ApprovalDecisionReply>> {
    if !scoped(&original) {
        return native_accepted_projection(ctx, original, id, opening_result).await;
    }
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let row = rows::approval(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let (ctx, principal, mut plan) = {
        let selection = original.admitted_operation().ok_or_else(denied)?;
        let Some(identity) = admission::response_caller(
            &ctx,
            &mut tx,
            &original,
            selection,
            Time::new(ctx.app.clock.now()),
        )
        .await?
        else {
            return Ok(None);
        };
        let ctx = ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
        let target = governor(&mut tx, &row).await?;
        let mut plan = OperationPlan::new(ctx.route, selection)?;
        bind(
            &mut plan,
            "approval_scope",
            APPROVAL,
            std::slice::from_ref(&target),
        )?;
        let Some(principal) =
            projection_principal(&ctx, &mut tx, &identity.caller, &plan, &target).await?
        else {
            return Ok(None);
        };
        let ctx = ctx.with_admitted_caller(principal.caller().clone())?;
        (ctx, principal, Some(plan))
    };
    let value = decision_projection_in(&ctx, &mut tx, &principal, row, opening_result).await?;
    let Some(mut plan) = plan.take() else {
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(Some(value));
    };
    bind_sources(&mut tx, &mut plan, Sources::reply(&value)?).await?;
    let shown = plan
        .read_response(&ctx, &mut tx, &original, move |_, _| {
            Box::pin(async move { Ok(value) })
        })
        .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(shown)
}
/// An accepted mutation remains successful if its optional response authority disappears.
/// The intended generated once-result belongs to that outcome, not old card content.
pub(super) async fn write_reply(
    ctx: &Ctx,
    current: &Principal,
    outcome: Outcome,
) -> ApiResult<orch_api_contract::approvals::ApprovalWriteResponse> {
    if is_scoped(ctx) && ctx.disclosure() == Disclosure::Denied {
        return Ok(orch_api_contract::approvals::ApprovalWriteResponse::Receipt(outcome.receipt));
    }
    let original = current.caller().clone();
    let id = outcome.receipt.approval_id;
    let full = crate::key_operations::completed_projection::run(
        ctx,
        accepted_projection(ctx.clone(), original, id, outcome.opening_result.clone()),
    )
    .await;
    match full {
        Ok(Some(full)) => Ok(orch_api_contract::approvals::ApprovalWriteResponse::full(
            full,
        )),
        Ok(None) | Err(_) => {
            Ok(orch_api_contract::approvals::ApprovalWriteResponse::Receipt(outcome.receipt))
        }
    }
}

/// A conflict may disclose a current card only through this same original operation's Read.
pub(super) async fn conflict_current(ctx: &Ctx, id: Id) -> Option<Approval> {
    if ctx.disclosure() == Disclosure::Denied {
        return None;
    }
    let original = ctx.admitted_caller()?.clone();
    match accepted_projection(ctx.clone(), original, id, None).await {
        Ok(Some(shown)) => Some(shown.approval),
        _ => None,
    }
}

/// Existing error codes and public permission hints remain useful without admitting
/// unrelated protected error labels. Unknown rich error shapes use the canonical minimal
/// envelope; the family conflict's current card has its separate typed Read projection.
pub(super) fn protect_error<T>(ctx: &Ctx, result: ApiResult<T>) -> ApiResult<T> {
    match result {
        Err(error)
            if is_scoped(ctx)
                && ctx.disclosure() != Disclosure::Full
                && error.code() != ErrorCode::ApprovalAlreadyDecided =>
        {
            let required = error
                .problem
                .extra
                .get("required")
                .and_then(Json::as_str)
                .map(str::to_owned);
            let mut error = authorization::problem_without_content(error);
            if let Some(required) = required {
                error
                    .problem
                    .extra
                    .insert("required".into(), json!(required));
            }
            Err(error)
        }
        other => other,
    }
}
