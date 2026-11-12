//! Existing invitation governance and projections under the same accepted caller.

use super::*;

use crate::key_operations::admission::binding_error;

use crate::key_operations::targets;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use crate::registry::OptionalOperationFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::operations::TargetResolver;

use orch_api_contract::routes::Endpoint;

use orch_authz::CapturedOperation;

use std::collections::BTreeSet;

use std::collections::HashMap;
const INVITATION: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Invitation);

const CHAT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Chat);

const NAMES: [(ObjectKind, &str); 4] = [
    (ObjectKind::Org, "invitation_names_orgs"),
    (ObjectKind::Project, "invitation_names_projects"),
    (ObjectKind::Topic, "invitation_names_topics"),
    (ObjectKind::Thread, "invitation_names_threads"),
];

pub(super) struct Governed {
    pub(super) target: KeyResolvedTarget,
}

pub(super) async fn lock_governing(
    conn: &mut Conn,
    governing: &KeyResolvedTarget,
) -> ApiResult<()> {
    match (governing.object.kind, governing.object.id) {
        (KeyTargetKind::Identity(KeyIdentityKind::System), None) => Ok(()),
        (KeyTargetKind::Object(kind), Some(id)) if kind.node().is_some() => {
            if hierarchy::lock(conn, id).await? {
                Ok(())
            } else {
                Err(ApiError::not_found())
            }
        }
        (KeyTargetKind::Identity(KeyIdentityKind::Chat), Some(id)) => {
            orch_db::queries::chats::lock(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            Ok(())
        }
        _ => Err(ApiError::internal(
            "an invitation has an invalid governing subject",
        )),
    }
}

fn role(plan: &OperationPlan, name: &str) -> ApiResult<usize> {
    plan.variant()
        .facets
        .iter()
        .position(|facet| matches!(facet.target,TargetResolver::Resolved(found)|TargetResolver::ResolvedCollection(found) if found == name))
        .ok_or_else(|| ApiError::internal("the invitation source role is undeclared"))
}

fn optional_role(plan: &OperationPlan, name: &str) -> Option<usize> {
    plan.variant().facets.iter().position(|facet| {
        matches!(facet.target,TargetResolver::Resolved(found)|TargetResolver::ResolvedCollection(found) if found == name)
    })
}

fn bind(
    plan: &mut OperationPlan,
    name: &str,
    kind: KeyTargetKind,
    targets: &[KeyResolvedTarget],
) -> ApiResult<()> {
    let slot = role(plan, name)?;
    if targets.is_empty() {
        plan.bind_empty(slot)
    } else {
        plan.bind(slot, kind, targets)
    }
}

/// A real invitation is governed by its stored destination, not creator/account
/// ancestry. The row id and bearer secret remain the ordinary offer witnesses.
pub(super) async fn governing_row(
    conn: &mut Conn,
    row: &InvitationRow,
) -> ApiResult<KeyResolvedTarget> {
    governing_rows(conn, std::slice::from_ref(row))
        .await?
        .into_iter()
        .next()
        .flatten()
        .ok_or_else(ApiError::not_found)
}

/// Node ancestry is loaded once for a list, and each distinct chat uses the
/// existing canonical chat-identity supplier once, not once per offer.
async fn governing_rows(
    conn: &mut Conn,
    rows: &[InvitationRow],
) -> ApiResult<Vec<Option<KeyResolvedTarget>>> {
    let mut ids: Vec<_> = rows
        .iter()
        .flat_map(|row| row.org_id.into_iter().chain(row.node_id))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let graph = orch_authz::engine::Graph::load(conn, &ids)
        .await
        .map_err(ApiError::internal)?;
    let mut chats = HashMap::new();
    for id in rows.iter().filter_map(|row| row.chat_id) {
        if let std::collections::hash_map::Entry::Vacant(entry) = chats.entry(id) {
            let target = match targets::chat_identity(conn, id).await {
                Ok(target) => Some(target),
                Err(error) if error.code() == ErrorCode::NotFound => None,
                Err(error) => return Err(error),
            };
            entry.insert(target);
        }
    }
    rows.iter().map(|row| match (row.kind.as_str(),row.org_id,row.node_id,row.chat_id) {
        ("system",None,None,None) => Ok(Some(targets::system())),
        (kind @ ("org"|"node"),org,node,None) => {
            let id = match (kind,org,node) {
                ("org",Some(id),None)|("node",None,Some(id)) => id,
                _ => return Err(ApiError::not_found()),
            };
            let Some(target) = orch_engine::operation_scope::targets::from_node_identity(&graph,id) else { return Ok(None) };
            if !matches!(target.object.kind, KeyTargetKind::Object(actual) if actual.node().is_some() && (actual == ObjectKind::Org) == (kind == "org")) {
                return Err(ApiError::not_found());
            }
            Ok(Some(target))
        }
        ("chat",None,None,Some(id)) => Ok(chats.get(&id).cloned().flatten()),
        _ => Err(ApiError::not_found()),
    }).collect()
}

async fn governing_body(
    conn: &mut Conn,
    body: &InvitationCreateBody,
) -> ApiResult<KeyResolvedTarget> {
    invitation_scope(body)?;
    match body.kind {
        InvitationKind::System => Ok(targets::system()),
        InvitationKind::Org => {
            let id = body
                .org_id
                .ok_or_else(|| invalid("org_id", "is needed to invite into an org"))?;
            let target = targets::node(conn, id).await?;
            if target.object.kind != KeyTargetKind::Object(ObjectKind::Org) {
                return Err(invalid("org_id", "is no org"));
            }
            Ok(target)
        }
        InvitationKind::Node => {
            let id = body
                .node_id
                .ok_or_else(|| invalid("node_id", "is needed to invite to a node"))?;
            let target = targets::node(conn, id).await?;
            if target.object.kind == KeyTargetKind::Object(ObjectKind::Org) {
                return Err(invalid("node_id", "an org is invited into, not to"));
            }
            Ok(target)
        }
        InvitationKind::Chat => {
            let id = body
                .chat_id
                .ok_or_else(|| invalid("chat_id", "is needed to invite to a group chat"))?;
            targets::chat_identity(conn, id).await
        }
    }
}

fn base(ctx: &Ctx, governing: &KeyResolvedTarget) -> ApiResult<OperationPlan> {
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    plan.bind(0, INVITATION, std::slice::from_ref(governing))?;
    Ok(plan)
}

pub(super) fn create<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: InvitationCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        plan.shared().compatible(caller).map_err(binding_error)?;
        invitation_scope(&body)?;
        body.email.as_deref().map(clean_email).transpose()?;
        if body
            .note
            .as_ref()
            .is_some_and(|note| note.chars().count() > NOTE_MAX_CHARS)
        {
            return Err(invalid(
                "note",
                &format!("must be at most {NOTE_MAX_CHARS} characters"),
            ));
        }
        let governing = governing_body(conn, &body).await?;
        Ok(base(ctx, &governing)?.with_data(Governed { target: governing }))
    })
}

pub(super) fn delete<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        plan.shared().compatible(caller).map_err(binding_error)?;
        let row = identity::invitation_by_id(conn, ctx.id("id")?)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let governing = governing_row(conn, &row).await?;
        Ok(base(ctx, &governing)?.with_data(Governed { target: governing }))
    })
}

pub(super) fn accept<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: InvitationAcceptBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        plan.shared().compatible(caller).map_err(binding_error)?;
        // Scope needs the actual retained offer/destination. Native redemption
        // alone checks availability, so a post-effect guard cannot consume or
        // reject an already accepted final seat a second time.
        let row = identity::invitation_by_hash(conn, &token_hash(&body.token))
            .await?
            .ok_or_else(gone)?;
        let governing = governing_row(conn, &row).await?;
        Ok(base(ctx, &governing)?.with_data(Governed { target: governing }))
    })
}

pub(super) fn capture(ctx: &Ctx, principal: &Principal) -> ApiResult<Option<serde_json::Value>> {
    if principal.caller().grant().is_none() {
        return Ok(None);
    }
    if !principal.caller().grant().is_some_and(|grant| {
        let mut ids = grant.token_ids();
        ids.next().is_some_and(|id| !id.is_nil()) && ids.all(|id| !id.is_nil())
    }) {
        return Err(ApiError::internal(
            "the invitation key origin has no stored credential",
        ));
    }
    if principal
        .caller()
        .grant()
        .and_then(|grant| grant.expires_at())
        .is_some_and(|end| end.get() <= ctx.app.clock.now())
    {
        return Err(ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the invitation issuer credential has expired",
        ));
    }
    let origin = CapturedOperation::capture(principal.caller()).map_err(ApiError::internal)?;
    if origin.operation() != InvitationCreate::ROUTE.name || origin.user() != principal.user() {
        return Err(ApiError::internal(
            "the invitation issuer changed its accepted operation",
        ));
    }
    serde_json::to_value(origin)
        .map(Some)
        .map_err(ApiError::internal)
}

pub(super) async fn restore_issuer(
    conn: &mut Conn,
    row: &InvitationRow,
    clock: &dyn orch_types::Clock,
) -> ApiResult<Option<Principal>> {
    let Some(raw) = &row.issuer_origin else {
        return Ok(None);
    };
    let origin: CapturedOperation =
        serde_json::from_value(raw.clone()).map_err(|_| issuer_gone())?;
    if origin.operation() != InvitationCreate::ROUTE.name || origin.user() != row.created_by {
        return Err(issuer_gone());
    }
    let Some(caller) = origin
        .restore(conn, clock.time())
        .await
        .map_err(authorization::restoration_error)?
    else {
        return Err(issuer_gone());
    };
    if caller.grant().is_none() {
        // Known key-origin metadata cannot turn into native browser mode.
        return Err(issuer_gone());
    }
    if caller
        .grant()
        .and_then(|grant| grant.expires_at())
        .is_some_and(|end| end.get() <= clock.now())
    {
        return Err(issuer_gone());
    }
    let governing = governing_row(conn, row).await.map_err(issuer_refusal)?;
    let mut plan = OperationPlan::new(InvitationCreate::ROUTE, InvitationCreate::ROUTE.name)?;
    plan.bind(0, INVITATION, &[governing])?;
    let caller = plan
        .shared()
        .admit(&caller, false)
        .map_err(|_| issuer_gone())?;
    let principal = Principal::load(conn, caller, clock.time())
        .await
        .map_err(authorization::restoration_error)?;
    if principal
        .caller()
        .grant()
        .and_then(|grant| grant.expires_at())
        .is_some_and(|end| end.get() <= clock.now())
    {
        return Err(issuer_gone());
    }
    if principal.suspension().applies(&[]) {
        return Err(issuer_gone());
    }
    Ok(Some(principal))
}

pub(super) fn system_issuer(principal: &Principal) -> ApiResult<bool> {
    crate::key_operations::admin::accepted_system_administration(
        principal,
        &InvitationCreate::ROUTE,
    )
}

pub(super) fn system_operation(ctx: &Ctx, principal: &Principal) -> ApiResult<bool> {
    crate::key_operations::admin::accepted_system_administration(principal, &ctx.route)
}

pub(super) struct Listed {
    plan: OperationPlan,
    values: Vec<Invitation>,
}

pub(super) fn list<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: InvitationListQuery,
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if caller
            .grant()
            .is_none_or(|grant| grant.operation_scope().is_none())
        {
            // Native author history keeps its existing behavior, including an
            // old offer whose destination has since been removed.
            return Ok(None);
        }
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        plan.shared().compatible(caller).map_err(binding_error)?;
        let raw =
            authorization::load_principal(conn, caller.clone(), Time::new(ctx.app.clock.now()))
                .await?;
        if let Some(org) = query.org_id {
            let (_, kind) = placed(conn, org).await?;
            if kind != NodeKind::Org {
                return Err(invalid("org_id", "is no org"));
            }
        }
        // Eligibility discovers possible identities only. A key receives no
        // system inventory until actual scoped rows and current standing pass.
        let candidates = identity::invitations_of(
            conn,
            query.org_id,
            raw.has_system_admin_standing(),
            raw.user(),
        )
        .await?;
        let facts = governing_rows(conn, &candidates).await?;
        let mut selected = Vec::new();
        let mut governors = Vec::new();
        for (row, governing) in candidates.into_iter().zip(facts) {
            // A rule key cannot invent placement for a purged historical target.
            // Omit that unscopable row while preserving the other current rows.
            let Some(governing) = governing else { continue };
            if plan.permits_binding(caller, 0, INVITATION, std::slice::from_ref(&governing))? {
                if !governors.contains(&governing) {
                    governors.push(governing.clone());
                }
                selected.push((row, governing));
            }
        }
        if governors.is_empty() {
            plan.bind_empty(0)?;
        } else {
            plan.bind(0, INVITATION, &governors)?;
        }
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        if let Some(org) = query.org_id {
            authorization::require_projected(
                conn,
                &principal,
                org,
                exact(Permission::MembersManage),
                Act::Use,
                Policy::default(),
                true,
            )
            .await?;
        }
        let system = system_operation(ctx, &principal)?;
        selected.retain(|(row, _)| {
            query.org_id.is_some()
                || row.created_by == principal.user()
                || system && row.kind == "system"
        });
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        let mut governors = Vec::new();
        for (_, governing) in &selected {
            if !governors.contains(governing) {
                governors.push(governing.clone());
            }
        }
        if governors.is_empty() {
            plan.bind_empty(0)?;
        } else {
            plan.bind(0, INVITATION, &governors)?;
        }
        let rows: Vec<_> = selected.into_iter().map(|(row, _)| row).collect();
        let mut values = invitations_shown(conn, &principal, &rows).await?;
        bound_invitation_refs(conn, principal.caller(), &plan, &mut values).await?;
        let mut sources = Sources::default();
        for value in &values {
            sources.extend(Sources::invitation(value));
        }
        bind_sources(conn, &mut plan, sources).await?;
        let response = plan.clone();
        Ok(Some(plan.with_data(Listed {
            plan: response,
            values,
        })))
    })
}

pub(super) async fn listed(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
) -> ApiResult<Vec<Invitation>> {
    let prepared = ctx.operation_data::<Listed>()?;
    let values = prepared.values.clone();
    prepared
        .plan
        .clone()
        .read_response(ctx, conn, principal.caller(), move |_, _| {
            Box::pin(async move { Ok(values) })
        })
        .await?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the invitation list content has not been admitted",
            )
        })
}

#[derive(Default)]
pub(super) struct Sources {
    objects: BTreeSet<(ObjectKind, Id)>,
    chats: BTreeSet<Id>,
}

impl Sources {
    fn extend(&mut self, other: Self) {
        self.objects.extend(other.objects);
        self.chats.extend(other.chats);
    }
    fn node(&mut self, node: &NodeRef) {
        if node.name.is_some() || node.thread_display.is_some() {
            let kind = match node.kind {
                NodeKind::Org => ObjectKind::Org,
                NodeKind::Project => ObjectKind::Project,
                NodeKind::Topic => ObjectKind::Topic,
                NodeKind::Thread => ObjectKind::Thread,
            };
            self.objects.insert((kind, node.id));
        }
    }
    fn chat(&mut self, chat: &orch_api_contract::chats::ChatRef) {
        if chat.name.is_some() {
            self.chats.insert(chat.id);
        }
        if let Some(node) = &chat.node {
            self.node(node);
        }
    }
    pub(super) fn invitation(invitation: &Invitation) -> Self {
        let mut sources = Self::default();
        for node in invitation.org.iter().chain(&invitation.node) {
            sources.node(node);
        }
        if let Some(chat) = &invitation.chat {
            sources.chat(chat);
        }
        sources
    }
    pub(super) fn accepted(reply: &InvitationAcceptReply) -> ApiResult<Self> {
        let mut sources = Self::default();
        if let Some(org) = &reply.org {
            sources.objects.extend(
                crate::routes::nodes::read_sources::Sources::of(&Node::Org(org.clone()))?
                    .objects()
                    .map(|object| (object.kind, object.id)),
            );
        }
        if let Some(node) = &reply.node {
            sources.node(node);
        }
        if let Some(chat) = &reply.chat {
            sources.chat(chat);
        }
        Ok(sources)
    }

    fn problem(error: &ApiError) -> ApiResult<Self> {
        if error.problem.extra.keys().any(|key| {
            !matches!(
                key.as_str(),
                "org"
                    | "scope"
                    | "domain"
                    | "archived_node"
                    | "locked_by"
                    | "role_from"
                    | "chat"
                    | "missing"
                    | "required"
                    | "object"
                    | "requestable"
                    | "override"
                    | "ask"
                    | "ask_more"
                    | "fields"
                    | "form"
                    | "max_days"
                    | "missing_permissions"
                    | "reason"
            )
        }) {
            return Err(ApiError::internal(
                "the invitation error projection is undeclared",
            ));
        }
        let mut sources = Self::default();
        for name in [
            "org",
            "scope",
            "domain",
            "archived_node",
            "locked_by",
            "role_from",
        ] {
            if let Some(value) = error
                .problem
                .extra
                .get(name)
                .filter(|value| !value.is_null())
            {
                let node: NodeRef =
                    serde_json::from_value(value.clone()).map_err(ApiError::internal)?;
                sources.node(&node);
            }
        }
        if let Some(value) = error
            .problem
            .extra
            .get("chat")
            .filter(|value| !value.is_null())
        {
            let chat: orch_api_contract::chats::ChatRef =
                serde_json::from_value(value.clone()).map_err(ApiError::internal)?;
            sources.chat(&chat);
        }
        if let Some(value) = error.problem.extra.get("missing") {
            let missing: Vec<orch_api_contract::access::MissingAccess> =
                serde_json::from_value(value.clone()).map_err(ApiError::internal)?;
            for missing in missing {
                if missing.object.name.is_some() {
                    let kind: ObjectKind = serde_json::from_value(json!(missing.object.kind))
                        .map_err(ApiError::internal)?;
                    sources.objects.insert((kind, missing.object.id));
                }
            }
        }
        if let Some(value) = error.problem.extra.get("object") {
            let object: orch_api_contract::access::MissingAccessObject =
                serde_json::from_value(value.clone()).map_err(ApiError::internal)?;
            if object.name.is_some() {
                let kind: ObjectKind =
                    serde_json::from_value(json!(object.kind)).map_err(ApiError::internal)?;
                sources.objects.insert((kind, object.id));
            }
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
            .map(|&(kind, id)| ObjectRef { kind, id }),
    );
    let targets = typed.targets(conn).await?;
    for (kind, name) in NAMES {
        let rows: Vec<_> = targets
            .iter()
            .filter(|(object, _)| object.kind == kind)
            .map(|(_, target)| target.clone())
            .collect();
        bind(plan, name, KeyTargetKind::Object(kind), &rows)?;
    }
    let documents: Vec<_> = targets
        .iter()
        .filter(|(object, _)| object.kind == ObjectKind::Document)
        .map(|(_, target)| target.clone())
        .collect();
    if optional_role(plan, "invitation_documents").is_some() {
        bind(
            plan,
            "invitation_documents",
            KeyTargetKind::Object(ObjectKind::Document),
            &documents,
        )?;
    } else if !documents.is_empty() {
        return Err(ApiError::internal(
            "an invitation projection has an undeclared document source",
        ));
    }
    if targets
        .keys()
        .any(|object| object.kind.node().is_none() && object.kind != ObjectKind::Document)
    {
        return Err(ApiError::internal(
            "an invitation projection has an undeclared resource source",
        ));
    }
    let mut chats = Vec::new();
    for chat in sources.chats {
        chats.push(targets::chat_identity(conn, chat).await?);
    }
    bind(plan, "invitation_chats", CHAT, &chats)
}

pub(super) async fn bound_invitation_refs(
    conn: &mut Conn,
    caller: &Caller,
    plan: &OperationPlan,
    values: &mut [Invitation],
) -> ApiResult<()> {
    if caller
        .grant()
        .is_none_or(|grant| grant.operation_scope().is_none())
    {
        return Ok(());
    }
    let mut sources = Sources::default();
    for value in values.iter() {
        sources.extend(Sources::invitation(value));
    }
    let objects = crate::routes::nodes::read_sources::Sources::from_objects(
        sources
            .objects
            .iter()
            .map(|&(kind, id)| ObjectRef { kind, id }),
    );
    let targets = objects.targets(conn).await?;
    let mut allowed = BTreeSet::new();
    for (object, target) in targets {
        let Some((_, name)) = NAMES.iter().find(|(kind, _)| *kind == object.kind) else {
            continue;
        };
        if plan.permits_binding(caller, role(plan, name)?, target.object.kind, &[target])? {
            allowed.insert(object.id);
        }
    }
    let mut chats = BTreeSet::new();
    for chat in sources.chats {
        let target = targets::chat_identity(conn, chat).await?;
        if plan.permits_binding(caller, role(plan, "invitation_chats")?, CHAT, &[target])? {
            chats.insert(chat);
        }
    }
    let bound_node = |node: &mut NodeRef| {
        if (node.name.is_some() || node.thread_display.is_some()) && !allowed.contains(&node.id) {
            node.name = None;
            node.thread_display = None;
            node.redacted = true;
        }
    };
    for value in values {
        for node in value.org.iter_mut().chain(&mut value.node) {
            bound_node(node);
        }
        if let Some(chat) = &mut value.chat {
            if chat.name.is_some() && !chats.contains(&chat.id) {
                chat.name = None;
            }
            if let Some(node) = &mut chat.node {
                bound_node(node);
            }
        }
    }
    Ok(())
}

fn wants_payload(ctx: &Ctx, current: &Principal, governing: &KeyResolvedTarget) -> ApiResult<bool> {
    let plan = base(ctx, governing)?;
    plan.permits_binding(
        current.caller(),
        role(&plan, "invitation_payload")?,
        INVITATION,
        std::slice::from_ref(governing),
    )
}

pub(super) async fn creation_reply(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    governing: &KeyResolvedTarget,
    row: &InvitationRow,
    receipt: InvitationCreateReceipt,
) -> ApiResult<InvitationCreateResponse> {
    if receipt.invitation_id != row.id {
        return Err(ApiError::internal(
            "the invitation capability changed its offer",
        ));
    }
    let full = if wants_payload(ctx, current, governing)? {
        let mut values = invitations_shown(conn, current, std::slice::from_ref(row)).await?;
        let plan = base(ctx, governing)?;
        bound_invitation_refs(conn, current.caller(), &plan, &mut values).await?;
        let invitation = values
            .into_iter()
            .next()
            .ok_or_else(|| ApiError::internal("an invitation was made and is not shown"))?;
        let sources = Sources::invitation(&invitation);
        project(
            ctx,
            conn,
            current,
            governing,
            InvitationCreateReply {
                invitation,
                token: receipt.token.clone(),
                url: receipt.url.clone(),
                sent: receipt.sent,
                already_created: receipt.already_created,
            },
            sources,
        )
        .await?
    } else {
        None
    };
    Ok(match full {
        Some(full) => InvitationCreateResponse::full(full),
        None => InvitationCreateResponse::Receipt(receipt),
    })
}

pub(super) async fn acceptance_reply(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    governing: &KeyResolvedTarget,
    row: &InvitationRow,
) -> ApiResult<InvitationAcceptResponse> {
    let full = if wants_payload(ctx, current, governing)? {
        let reply = accepted_reply(ctx, conn, current, row).await?;
        let sources = Sources::accepted(&reply)?;
        project(ctx, conn, current, governing, reply, sources).await?
    } else {
        None
    };
    Ok(match full {
        Some(full) => InvitationAcceptResponse::full(full),
        None => InvitationAcceptResponse::Receipt(InvitationAcceptReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            invitation_id: row.id,
        }),
    })
}

pub(super) async fn project<T: Send + Sync + 'static>(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    governing: &KeyResolvedTarget,
    value: T,
    sources: Sources,
) -> ApiResult<Option<T>> {
    let mut plan = base(ctx, governing)?;
    bind(
        &mut plan,
        "invitation_payload",
        INVITATION,
        std::slice::from_ref(governing),
    )?;
    bind_sources(conn, &mut plan, sources).await?;
    plan.read_response(ctx, conn, current.caller(), move |_, _| {
        Box::pin(async move { Ok(value) })
    })
    .await
}

pub(super) fn minimally_projected(error: ApiError) -> ApiError {
    let required = error
        .problem
        .extra
        .get("required")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let mut result = authorization::problem_without_content(error);
    if let Some(required) = required {
        result
            .problem
            .extra
            .insert("required".into(), json!(required));
    }
    result
}

/// Scope the native typed Problem's actual reference fields. Unknown error
/// shapes retain their stable code/required in the existing minimal envelope.
/// Configured text is never searched or rewritten as if it were a source id.
pub(super) async fn project_problem(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    governing: &KeyResolvedTarget,
    error: ApiError,
) -> ApiResult<ApiError> {
    if error.code() == orch_api_contract::common::ErrorCode::InvitationTermsNotAccepted {
        // These exact terms are public to the holder of this already validated invitation token.
        return Ok(error);
    }
    if current
        .caller()
        .grant()
        .is_none_or(|grant| grant.operation_scope().is_none())
    {
        return Ok(error);
    }
    let actual = match (governing.object.kind, governing.object.id) {
        (KeyTargetKind::Identity(KeyIdentityKind::System), None) => Ok(targets::system()),
        (KeyTargetKind::Object(kind), Some(id)) if kind.node().is_some() => {
            targets::node_identity(conn, id).await
        }
        (KeyTargetKind::Identity(KeyIdentityKind::Chat), Some(id)) => {
            targets::chat_identity(conn, id).await
        }
        _ => Err(ApiError::not_found()),
    };
    let actual = match actual {
        Ok(actual) if actual.object == governing.object => actual,
        Ok(_) => return Ok(minimally_projected(error)),
        Err(missing) if missing.code() == ErrorCode::NotFound => {
            return Ok(minimally_projected(error));
        }
        Err(failure) => return Err(failure),
    };
    let Ok(sources) = Sources::problem(&error) else {
        return Ok(minimally_projected(error));
    };
    let original = error.clone();
    Ok(project(ctx, conn, current, &actual, error, sources)
        .await?
        .unwrap_or_else(|| minimally_projected(original)))
}

#[path = "invitation_operation/creation_replay.rs"]
pub mod creation_replay;
