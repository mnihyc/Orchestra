//! Exact share operands, bounded by the shared operation gate and consumed by the ordinary writer.

use super::*;

use crate::key_operations::targets;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::operations::TargetResolver;

use orch_api_contract::share::ShareTo;

use orch_api_contract::share::ShareWhat;

use orch_api_contract::threads::ShareMode;

use orch_authz::Caller;

use orch_engine::sharing::PreparedAgentShare;
const NODES: [(ObjectKind, &str); 4] = [
    (ObjectKind::Org, "orgs"),
    (ObjectKind::Project, "projects"),
    (ObjectKind::Topic, "topics"),
    (ObjectKind::Thread, "threads"),
];

fn indexes(plan: &OperationPlan, role: &str) -> Vec<usize> {
    plan.variant()
        .facets
        .iter()
        .enumerate()
        .filter_map(|(i, f)| match f.target {
            TargetResolver::Resolved(name) | TargetResolver::ResolvedCollection(name)
                if name == role =>
            {
                Some(i)
            }
            _ => None,
        })
        .collect()
}

fn bind(
    plan: &mut OperationPlan,
    role: &str,
    kind: KeyTargetKind,
    targets: &[KeyResolvedTarget],
) -> ApiResult<()> {
    for i in indexes(plan, role) {
        if targets.is_empty() {
            plan.bind_empty(i)?;
        } else {
            plan.bind(i, kind, targets)?;
        }
    }
    Ok(())
}

async fn source(conn: &mut Conn, what: &ShareWhat) -> ApiResult<KeyResolvedTarget> {
    match what {
        ShareWhat::Node(n) => targets::node(conn, n.node_id).await,
        ShareWhat::Chat(c) => targets::chat(conn, c.chat_id).await,
        ShareWhat::Document(d) => {
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Document,
                    id: d.document_id,
                },
            )
            .await
        }
    }
}

fn source_role(kind: KeyTargetKind) -> ApiResult<&'static str> {
    Ok(match kind {
        KeyTargetKind::Object(ObjectKind::Org) => "snapshot_source_orgs",
        KeyTargetKind::Object(ObjectKind::Project) => "snapshot_source_projects",
        KeyTargetKind::Object(ObjectKind::Topic) => "snapshot_source_topics",
        KeyTargetKind::Object(ObjectKind::Thread) => "snapshot_source_threads",
        KeyTargetKind::Object(ObjectKind::Document) => "snapshot_source_documents",
        KeyTargetKind::Identity(KeyIdentityKind::Chat) => "snapshot_source_chats",
        _ => return Err(ApiError::not_found()),
    })
}

async fn failure(ctx: &Ctx, conn: &mut Conn, error: ShareError) -> ApiError {
    let limited = ctx.key_receipt_required()
        || ctx
            .user()
            .ok()
            .and_then(|user| authorization::caller_of(user).ok())
            .is_some_and(|caller| {
                caller
                    .grant()
                    .is_some_and(|grant| grant.operation_scope().is_some())
            });
    failed_projected(conn, error, limited).await
}

async fn name_bindings(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    plan: &OperationPlan,
    ids: &[Id],
) -> ApiResult<Vec<KeyResolvedTarget>> {
    let mut out = Vec::new();
    // One ordinary graph supplies each actual candidate and its qualified ancestors.
    // Missing/deleted names stay absent; normal reader-specific projection remains below.
    let graph = orch_authz::engine::Graph::load(conn, ids)
        .await
        .map_err(ApiError::internal)?;
    let mut named: Vec<_> = graph.nodes().filter(|node| !node.deleted).collect();
    named.sort_by_key(|node| node.id);
    for node in named {
        let site = graph.site_of(node.id).ok_or_else(ApiError::not_found)?;
        let target = targets::from_site(&graph, &site)?;
        let KeyTargetKind::Object(kind) = target.object.kind else {
            continue;
        };
        let Some((_, suffix)) = NODES.iter().find(|(k, _)| *k == kind) else {
            continue;
        };
        let role = format!("share_name_{suffix}");
        let slots = indexes(plan, &role);
        let mut allowed = !slots.is_empty();
        for index in slots {
            allowed &= plan.permits_binding(
                caller,
                index,
                target.object.kind,
                std::slice::from_ref(&target),
            )?;
        }
        if allowed {
            out.push(target);
        }
    }
    // Ordinary visibility is still decided by the existing reader-specific projection.
    let _ = ctx;
    Ok(out)
}

fn bind_names(plan: &mut OperationPlan, names: &[KeyResolvedTarget]) -> ApiResult<()> {
    for (kind, suffix) in NODES {
        let selected: Vec<_> = names
            .iter()
            .filter(|t| t.object.kind == KeyTargetKind::Object(kind))
            .cloned()
            .collect();
        bind(
            plan,
            &format!("share_name_{suffix}"),
            KeyTargetKind::Object(kind),
            &selected,
        )?;
    }
    Ok(())
}

pub(super) fn resolve<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: ShareWithAgents,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let selected = match ctx
            .app
            .engine
            .agent_share_selection(conn, caller.user(), ctx.now, &body)
            .await
        {
            Ok(selected) => selected,
            Err(error) => return Err(failure(ctx, conn, error).await),
        };
        let mut plan = OperationPlan::new(ctx.route, selected)?;
        let target = source(conn, &body.what).await?;
        bind(
            &mut plan,
            "response_source",
            target.object.kind,
            std::slice::from_ref(&target),
        )?;
        if body.mode == ShareMode::Live {
            bind(
                &mut plan,
                "source",
                target.object.kind,
                std::slice::from_ref(&target),
            )?;
        }
        let anchor = match &body.what {
            orch_api_contract::share::ShareWhat::Node(node) => Some(node.node_id),
            _ => None,
        };
        let disclosure = crate::thread_read_tools::sharing::Before::take(
            &ctx.app,
            conn,
            anchor,
            &("agents", &body),
            body.parts
                .as_ref()
                .is_some_and(|parts| parts.timeline.is_some()),
            body.inside.unwrap_or(true),
        )
        .await?;
        let (plan, prepared) = prepare_agents(ctx, conn, caller, &body, plan).await?;
        let selection = plan.variant().operation.clone();
        Ok(plan.with_data(PreparedShare {
            disclosure,
            share: prepared,
            selection,
        }))
    })
}

/// Fresh HTTP result plus its original selected operation; never persisted or client-supplied.
pub(super) struct PreparedShare {
    pub disclosure: crate::thread_read_tools::sharing::Before,
    pub share: PreparedAgentShare,
    pub selection: String,
}

const SOURCES: [(&str, KeyTargetKind); 8] = [
    (
        "snapshot_source_orgs",
        KeyTargetKind::Object(ObjectKind::Org),
    ),
    (
        "snapshot_source_projects",
        KeyTargetKind::Object(ObjectKind::Project),
    ),
    (
        "snapshot_source_topics",
        KeyTargetKind::Object(ObjectKind::Topic),
    ),
    (
        "snapshot_source_threads",
        KeyTargetKind::Object(ObjectKind::Thread),
    ),
    (
        "snapshot_source_documents",
        KeyTargetKind::Object(ObjectKind::Document),
    ),
    (
        "snapshot_source_document_containers",
        KeyTargetKind::Object(ObjectKind::DocumentContainer),
    ),
    (
        "snapshot_source_environments",
        KeyTargetKind::Object(ObjectKind::Environment),
    ),
    (
        "snapshot_source_chats",
        KeyTargetKind::Identity(KeyIdentityKind::Chat),
    ),
];

fn bind_copy_sources(
    plan: &mut OperationPlan,
    sources: &[orch_engine::copies::SourceBinding],
) -> ApiResult<()> {
    for (role, kind) in SOURCES {
        let mut selected = Vec::new();
        for b in sources.iter().filter(|b| b.role == role) {
            if !selected
                .iter()
                .any(|t: &KeyResolvedTarget| t.object == b.target.object)
            {
                selected.push(b.target.clone());
            }
        }
        bind(plan, role, kind, &selected)?;
    }
    for (role, kind) in [
        (
            "message_source_chats",
            KeyTargetKind::Identity(KeyIdentityKind::Chat),
        ),
        (
            "message_source_threads",
            KeyTargetKind::Object(ObjectKind::Thread),
        ),
        (
            "message_source_documents",
            KeyTargetKind::Object(ObjectKind::Document),
        ),
        (
            "message_source_environments",
            KeyTargetKind::Object(ObjectKind::Environment),
        ),
        (
            "message_retained_uploads",
            KeyTargetKind::Identity(KeyIdentityKind::Upload),
        ),
    ] {
        bind(plan, role, kind, &[])?;
    }
    Ok(())
}

/// Actual creation propagates ordinary refusals; previews retain them in their existing trial envelope.
pub(crate) async fn prepare_agents(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: &ShareWithAgents,
    plan: OperationPlan,
) -> ApiResult<(OperationPlan, PreparedAgentShare)> {
    let (plan, prepared) = prepare_agents_trial(ctx, conn, caller, body, plan).await?;
    Ok((plan, prepared?))
}

/// Shared source/recipient preparation retains the original operation and ordinary refusal semantics.
pub(crate) async fn prepare_agents_trial(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: &ShareWithAgents,
    mut plan: OperationPlan,
) -> ApiResult<(OperationPlan, Result<PreparedAgentShare, ApiError>)> {
    let target = source(conn, &body.what).await?;
    let snapshot = body.mode == ShareMode::Snapshot;
    let mut initial = plan.clone();
    if snapshot {
        bind(
            &mut initial,
            source_role(target.object.kind)?,
            target.object.kind,
            std::slice::from_ref(&target),
        )?;
    }
    let principal = initial.preparation_principal(ctx, conn, caller).await?;
    let requested: Vec<_> = match &body.to {
        ShareTo::Threads(to) => to.iter().map(|to| to.thread_id).collect(),
        ShareTo::Nodes(to) => to.iter().map(|to| to.node_id).collect(),
    };
    let mut recipients = Vec::new();
    for id in requested {
        match targets::node(conn, id).await {
            Ok(target) => recipients.push(target),
            Err(error) if error.problem.code == ErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let selection = match ctx
        .app
        .engine
        .agent_share_selection(conn, caller.user(), principal.now().get(), body)
        .await
    {
        Ok(selected) => selected,
        Err(error) => return Err(failure(ctx, conn, error).await),
    };
    let control = selection.contains("live_read_first");
    for (kind, suffix) in NODES {
        let targets: Vec<_> = recipients
            .iter()
            .filter(|t| t.object.kind == KeyTargetKind::Object(kind))
            .cloned()
            .collect();
        bind(
            &mut plan,
            &format!("share_recipient_{suffix}"),
            KeyTargetKind::Object(kind),
            if snapshot { &[] } else { &targets },
        )?;
        bind(
            &mut plan,
            &format!("share_control_{suffix}"),
            KeyTargetKind::Object(kind),
            if control { &targets } else { &[] },
        )?;
        bind(
            &mut plan,
            &format!("response_recipient_{suffix}"),
            KeyTargetKind::Object(kind),
            &targets,
        )?;
        if kind == ObjectKind::Thread {
            bind(
                &mut plan,
                "snapshot_recipient_threads",
                KeyTargetKind::Object(kind),
                if snapshot { &targets } else { &[] },
            )?;
        }
    }
    let viewers = body.viewers_read == Some(true) && !snapshot;
    bind(
        &mut plan,
        "viewer_grant_sources",
        KeyTargetKind::Object(ObjectKind::Document),
        if viewers {
            std::slice::from_ref(&target)
        } else {
            &[]
        },
    )?;
    let principal = plan
        .preparation_principal(ctx, conn, principal.caller())
        .await?;
    let mut source_bindings = Vec::new();
    if snapshot {
        source_bindings.push(orch_engine::copies::SourceBinding {
            role: source_role(target.object.kind)?,
            kind: target.object.kind,
            target: target.clone(),
        });
    }
    let prepared = ctx
        .app
        .engine
        .prepare_agent_share_in(conn, &principal, &origin(ctx), body)
        .await;
    let mut prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            bind_copy_sources(&mut plan, &source_bindings)?;
            bind_names(&mut plan, &[])?;
            return Ok((plan, Err(failure(ctx, conn, error).await)));
        }
    };
    // A node chat's default title is read from its containing node, while an
    // explicit chat name is Chat content. Keep that distinction for both the
    // prepared copy/index and its history marker, before any bytes are written.
    if snapshot && let ShareWhat::Chat(chat) = &body.what {
        let row = orch_db::queries::chats::chat(conn, chat.chat_id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if row.name.is_none()
            && let Some(node) = row.node_id
        {
            let target = targets::node(conn, node).await?;
            let role = source_role(target.object.kind)?;
            let slots = indexes(&plan, role);
            let mut allowed = !slots.is_empty();
            for index in slots {
                allowed &= plan.permits_binding(
                    principal.caller(),
                    index,
                    target.object.kind,
                    std::slice::from_ref(&target),
                )?;
            }
            if allowed {
                source_bindings.push(orch_engine::copies::SourceBinding {
                    role,
                    kind: target.object.kind,
                    target,
                });
            } else {
                prepared.found.name = "Chat".to_owned();
            }
        }
    }
    prepared.checked_copy = true;
    if snapshot {
        let missing = prepared
            .missing_copy_recipients(conn, body)
            .await
            .map_err(ApiError::internal)?;
        if !missing.is_empty() {
            let copied = ctx
                .app
                .engine
                .files_of_copy_scoped(
                    conn,
                    &principal,
                    &prepared.found,
                    &prepared.parts,
                    principal.now(),
                    orch_engine::copies::Scope {
                        plan: plan.shared(),
                        caller: principal.caller(),
                    },
                )
                .await;
            let copied = match copied {
                Ok(p) => p,
                Err(error) => {
                    bind_copy_sources(&mut plan, &source_bindings)?;
                    bind_names(&mut plan, &[])?;
                    return Ok((plan, Err(failure(ctx, conn, error).await)));
                }
            };
            source_bindings.extend(copied.sources.iter().cloned());
            for id in missing {
                let writings = ctx
                    .app
                    .engine
                    .copy_writings_scoped(
                        conn,
                        id,
                        &copied.files,
                        orch_engine::copies::Scope {
                            plan: plan.shared(),
                            caller: principal.caller(),
                        },
                    )
                    .await;
                let writings = match writings {
                    Ok(p) => p,
                    Err(error) => {
                        bind_copy_sources(&mut plan, &source_bindings)?;
                        bind_names(&mut plan, &[])?;
                        return Ok((plan, Err(failure(ctx, conn, error).await)));
                    }
                };
                source_bindings.extend(writings.sources.iter().cloned());
                prepared.writings.insert(id, writings);
            }
            prepared.copies = Some(copied);
        }
    }
    bind_copy_sources(&mut plan, &source_bindings)?;
    prepared.source_name_visible = if snapshot {
        true
    } else {
        let slots = indexes(&plan, "response_source");
        if slots.is_empty() {
            true
        } else {
            let mut allowed = true;
            for index in slots {
                allowed &= plan.permits_binding(
                    principal.caller(),
                    index,
                    target.object.kind,
                    std::slice::from_ref(&target),
                )?;
            }
            allowed
        }
    };
    let mut names = Vec::new();
    for ancestor in &target.node_ancestors {
        names.extend(ancestor.id);
    }
    if matches!(
        target.object.kind,
        KeyTargetKind::Object(
            ObjectKind::Org | ObjectKind::Project | ObjectKind::Topic | ObjectKind::Thread
        )
    ) {
        names.extend(target.object.id);
    }
    for (id, _) in &prepared.recipients {
        names.push(*id);
    }
    names.sort_unstable();
    names.dedup();
    let allowed = name_bindings(ctx, conn, principal.caller(), &plan, &names).await?;
    bind_names(&mut plan, &allowed)?;
    Ok((plan, Ok(prepared)))
}
orch_api_base::__api_bind_d61a4f1c27!();

fn what_of(subject: Subject) -> ShareWhat {
    match subject {
        Subject::Node(node_id) => {
            ShareWhat::Node(orch_api_contract::share::ShareWhatNode { node_id })
        }
        Subject::Chat(chat_id) => {
            ShareWhat::Chat(orch_api_contract::share::ShareWhatChat { chat_id })
        }
        Subject::Document(document_id) => {
            ShareWhat::Document(orch_api_contract::share::ShareWhatDocument { document_id })
        }
    }
}

#[path = "operation/creation.rs"]
pub mod creation;

#[path = "operation/records.rs"]
pub mod records;
