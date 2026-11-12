//! The Node panel's own current root, independently admitted scopes and optional native reads.

use super::*;

use crate::key_operations::targets;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use crate::routes::access::operations::NameRoles;

use crate::routes::access::operations::projection_names;

use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::operations::TargetResolver;

use orch_api_contract::summaries::SummaryTarget;

use orch_authz::Caller;

use orch_authz::CapturedCaller;
const CHAT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Chat);

const NAMES: NameRoles = NameRoles {
    groups: None,
    nodes: [
        (ObjectKind::Org, "opening_names_orgs"),
        (ObjectKind::Project, "opening_names_projects"),
        (ObjectKind::Topic, "opening_names_topics"),
        (ObjectKind::Thread, "opening_names_threads"),
    ],
    containers: None,
};

pub(super) struct PreparedNodeOpening {
    scopes: Vec<Scope>,
    opening: Option<read_operation::PreparedRead>,
    defaults: Option<defaults::PreparedDefaults>,
    names: NameBounds,
    summaries: Option<std::sync::Arc<crate::routes::summaries::embedded::Embedded>>,
}

struct Scope {
    node: Id,
    kind: NodeKind,
    chat: TreeChat,
}

pub(super) fn slot(plan: &OperationPlan, role: &'static str) -> ApiResult<usize> {
    plan.variant()
        .facets
        .iter()
        .position(|facet| {
            matches!(facet.target,
            TargetResolver::Resolved(name)|TargetResolver::ResolvedCollection(name) if name==role)
        })
        .ok_or_else(|| ApiError::internal("the Node opening source is not declared"))
}

pub(super) fn bind(
    plan: &mut OperationPlan,
    role: &'static str,
    kind: KeyTargetKind,
    targets: &[KeyResolvedTarget],
) -> ApiResult<()> {
    let index = slot(plan, role)?;
    if targets.is_empty() {
        plan.bind_empty(index)
    } else {
        plan.bind(index, kind, targets)
    }
}

pub(super) fn permits(
    plan: &OperationPlan,
    caller: &Caller,
    role: &'static str,
    target: &KeyResolvedTarget,
) -> ApiResult<bool> {
    plan.permits_binding(
        caller,
        slot(plan, role)?,
        target.object.kind,
        std::slice::from_ref(target),
    )
}

pub(super) async fn current(ctx: &Ctx, conn: &mut Conn, caller: &Caller) -> ApiResult<Caller> {
    CapturedCaller::capture(caller)
        .restore(conn, orch_types::Time::new(ctx.app.clock.now()))
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthInvalidToken,
                "the credential is no longer valid",
            )
        })
}

pub(super) async fn error(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    error: ApiError,
) -> ApiError {
    match current(ctx, conn, caller).await {
        Ok(caller)
            if caller
                .grant()
                .is_none_or(|grant| grant.operation_scope().is_none()) =>
        {
            error
        }
        Ok(_) => authorization::problem_without_content(error),
        Err(error) => error,
    }
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: NodeChatOpeningGetQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        match prepare_inner(ctx, conn, caller, query).await {
            Ok(plan) => Ok(plan),
            Err(failed) => Err(error(ctx, conn, caller, failed).await),
        }
    })
}

async fn prepare_inner(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: NodeChatOpeningGetQuery,
) -> ApiResult<OperationPlan> {
    let root = ctx.id("id")?;
    let target = targets::node(conn, root).await?;
    let root_kind = match target.object.kind {
        KeyTargetKind::Object(ObjectKind::Org) => NodeKind::Org,
        KeyTargetKind::Object(ObjectKind::Project) => NodeKind::Project,
        KeyTargetKind::Object(ObjectKind::Topic) => NodeKind::Topic,
        KeyTargetKind::Object(ObjectKind::Thread) => NodeKind::Thread,
        _ => return Err(ApiError::not_found()),
    };
    let caller = current(ctx, conn, caller).await?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    bind(
        &mut plan,
        "opening_node",
        target.object.kind,
        std::slice::from_ref(&target),
    )?;
    let principal = plan.preparation_principal(ctx, conn, &caller).await?;
    let bounded = principal
        .caller()
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some());
    // This is the route's existing native NodeView gate, not an additional ordinary right.
    authorization::require_projected(
        conn,
        &principal,
        root,
        authorization::VIEW,
        Act::Use,
        Policy::default(),
        bounded,
    )
    .await?;
    let work = Work {
        projection_ctx: ctx.clone(),
        now: principal.now().get(),
        principal,
        origin: origin(ctx),
    };
    // The same normal View/path producer as the native node pages. Full node data is
    // projected only if Defaults asks for it, after its actual sources have been bound.
    let mut paths = view::viewable_ways(conn, &work.principal, &[root], Policy::default()).await?;
    let path = paths.remove(&root).ok_or_else(ApiError::not_found)?;
    let wanted: &[NodeKind] = match root_kind {
        NodeKind::Org => &[NodeKind::Org],
        NodeKind::Project => &[NodeKind::Project],
        NodeKind::Topic => &[NodeKind::Topic, NodeKind::Project],
        NodeKind::Thread => &[NodeKind::Thread, NodeKind::Topic, NodeKind::Project],
    };
    let candidates: Vec<Id> = wanted
        .iter()
        .filter_map(|kind| {
            path.iter()
                .rev()
                .find(|node| node.kind == *kind && !node.redacted)
                .map(|node| node.id)
        })
        .collect();
    let visible =
        view::viewable_ways(conn, &work.principal, &candidates, Policy::default()).await?;
    let mut scopes = Vec::new();
    let mut loaded = Vec::new();
    let mut chat_targets = Vec::new();
    // Retain the existing nearest-kind order rather than HashMap/database order.
    for id in candidates {
        if !visible.contains_key(&id) {
            continue;
        }
        let Some(node) = path.iter().find(|node| node.id == id) else {
            continue;
        };
        let Some(row) = rows::chat_of_node(conn, id).await? else {
            continue;
        };
        let chat_target = targets::from_chat(conn, &row).await?;
        if !permits(
            &plan,
            work.principal.caller(),
            "opening_scope_chats",
            &chat_target,
        )? {
            continue;
        }
        match load(conn, &work, row.id, Mode::Read).await {
            Ok(readable) => {
                scopes.push(Scope {
                    node: id,
                    kind: node.kind,
                    chat: TreeChat {
                        id: row.id,
                        seq: readable.row.seq(),
                    },
                });
                loaded.push(readable);
                chat_targets.push(chat_target);
            }
            Err(error) if matches!(error.problem.status, 403 | 404) => {}
            Err(error) => return Err(error),
        }
    }
    bind(&mut plan, "opening_scope_chats", CHAT, &chat_targets)?;
    let chosen = query
        .chat_id
        .and_then(|id| scopes.iter().position(|scope| scope.chat.id == id))
        .or_else(|| {
            query
                .scope
                .and_then(|kind| scopes.iter().position(|scope| scope.kind == kind))
        })
        .or_else(|| (!scopes.is_empty()).then_some(0));
    let selected = chosen.map(|index| &chat_targets[index]);
    bind(
        &mut plan,
        "node_chat",
        CHAT,
        &selected.into_iter().cloned().collect::<Vec<_>>(),
    )?;
    let mut names = NameSources::default();
    names.nodes.extend(target.node_path().into_iter().flatten());
    if query.settings_only
        && (!query.defaults
            || query.secret_names
            || query.default_provider_name
            || query.embeddings_provider_name)
    {
        return Err(ApiError::invalid(
            "settings_only",
            "must request only the settings consumer",
        ));
    }
    let defaults = if query.defaults {
        Some(
            defaults::prepare(
                ctx,
                conn,
                &work.principal,
                &mut plan,
                root,
                root_kind,
                &query,
                &mut names,
            )
            .await?,
        )
    } else {
        defaults::empty(&mut plan)?;
        None
    };
    let names = projection_names(conn, work.principal.caller(), &mut plan, names, &NAMES).await?;
    let (mut plan, opening) = match chosen {
        Some(index) => {
            let selected = loaded
                .into_iter()
                .nth(index)
                .ok_or_else(|| ApiError::internal("the chosen Chat has no native read"))?;
            let (plan, prepared) =
                read_operation::prepare_opening_with_plan(conn, &work, plan, selected, query.limit)
                    .await?;
            (plan, Some(prepared))
        }
        None => {
            read_operation::empty_opening(&mut plan)?;
            (plan, None)
        }
    };
    let summary_slots = [
        slot(&plan, "opening_summary_targets")?,
        slot(&plan, "opening_summary_inputs")?,
        slot(&plan, "opening_summary_providers")?,
    ];
    // The place's own summary is independent of which chat is selected or readable.
    let summaries = crate::routes::summaries::embedded::prepare(
        ctx,
        conn,
        &caller,
        plan.shared(),
        opening.as_ref().map(|read| read.loaded.row.id),
        match root_kind {
            NodeKind::Topic => Some(SummaryTarget::Topic { id: root }),
            NodeKind::Thread => Some(SummaryTarget::Thread { id: root }),
            NodeKind::Org | NodeKind::Project => None,
        },
        summary_slots,
    )
    .await
    .ok()
    .flatten();
    let summary_facts = summaries
        .as_ref()
        .map(|value| value.facts())
        .unwrap_or_default();
    for (index, facts) in summary_slots.into_iter().zip(summary_facts) {
        let mut groups = std::collections::BTreeMap::<_, Vec<_>>::new();
        for fact in facts {
            let group = groups.entry(fact.object.kind).or_default();
            if !group.contains(&fact) {
                group.push(fact);
            }
        }
        if groups.is_empty() {
            plan.bind_empty(index)?;
        } else {
            plan.bind_collection(index, &groups.into_iter().collect::<Vec<_>>())?;
        }
    }
    Ok(plan.with_data(PreparedNodeOpening {
        scopes,
        opening,
        defaults,
        names,
        summaries,
    }))
}

pub(super) async fn show(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: Principal,
    prepared: &PreparedNodeOpening,
) -> ApiResult<NodeChatOpening> {
    let work = Work {
        projection_ctx: ctx.clone(),
        now: principal.now().get(),
        principal,
        origin: origin(ctx),
    };
    let ids: Vec<_> = prepared.scopes.iter().map(|scope| scope.node).collect();
    let refs = prepared.names.refs(conn, &work.principal, &ids).await?;
    let scopes = prepared
        .scopes
        .iter()
        .map(|scope| NodeChatScope {
            // Scope choices keep their existing compact wire; a Thread's live
            // display belongs to its native projection, not this selector row.
            node: NodeRef {
                thread_display: None,
                ..refs.get(&scope.node).cloned().unwrap_or(NodeRef {
                    id: scope.node,
                    kind: scope.kind,
                    name: None,
                    redacted: true,
                    thread_display: None,
                })
            },
            chat: scope.chat.clone(),
        })
        .collect();
    let opening = match &prepared.opening {
        Some(read) => {
            note_opened(conn, &work, &read.loaded).await?;
            let projected = read_operation::project(conn, &work.principal, read).await?;
            Some(read_operation::opening_view(read, projected)?)
        }
        None => None,
    };
    let defaults = match &prepared.defaults {
        Some(read) => {
            Some(defaults::show(ctx, conn, &work.principal, read, &prepared.names).await?)
        }
        None => None,
    };
    if let Some(summaries) = &prepared.summaries {
        crate::routes::summaries::embedded::note_read(conn, &work.principal, summaries).await?;
        crate::routes::summaries::embedded::retain(ctx, &work.principal, summaries)?;
    }
    Ok(NodeChatOpening {
        scopes,
        opening,
        defaults,
        summaries: prepared
            .summaries
            .as_ref()
            .map_or_else(Vec::new, |value| value.snapshots()),
        summary_range: prepared.summaries.as_ref().and_then(|value| value.range),
    })
}
