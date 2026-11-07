//! The existing provider setup reader, with its current management root and actual offers.

use crate::key_operations::targets;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationFuture, OperationPlan};
use crate::routes::access::operations::{NameRoles, projection_names};
use crate::routes::access::page::NameSources;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyObjectRef, KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::operations::TargetResolver;
use orch_api_contract::resources::{ProviderSetup, ProviderSetupGetQuery, SourceBy};
use orch_authz::{Caller, Principal, engine::Graph, resources};
use orch_db::Conn;
use orch_types::Id;
use std::collections::{HashMap, HashSet};

const PROVIDER: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Provider);
const NAMES: NameRoles = NameRoles {
    groups: None,
    nodes: [
        (ObjectKind::Org, "setup_names_orgs"),
        (ObjectKind::Project, "setup_names_projects"),
        (ObjectKind::Topic, "setup_names_topics"),
        (ObjectKind::Thread, "setup_names_threads"),
    ],
    containers: None,
};

struct PreparedSetup {
    response: OperationPlan,
    reply: ProviderSetup,
}

fn scoped(caller: &Caller) -> bool {
    caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
}

/// Stored offer identity is factual; the ordinary setup reader alone decides whether
/// the selected keeper/template declaration remains available through a retained home.
fn offer_target(graph: &Graph, row: &resources::Resource) -> Option<KeyResolvedTarget> {
    let node_ancestors = match row.home {
        Some(home) => {
            orch_engine::operation_scope::targets::from_node_identity(graph, home)?.node_ancestors
        }
        None => Vec::new(),
    };
    Some(KeyResolvedTarget {
        object: KeyObjectRef {
            kind: KeyTargetKind::Object(row.kind.into()),
            id: Some(row.id),
        },
        node_ancestors,
        document_ancestors: Vec::new(),
        credential_account: None,
    })
}

/// Scope only selects actual ordinary offer payloads. It grants no resource permission.
/// The closed source roles cannot be used as a generic partial-caller or visibility fallback.
async fn selected(
    conn: &mut Conn,
    caller: &Caller,
    plan: &mut OperationPlan,
    kind: ObjectKind,
    ids: &[Id],
) -> ApiResult<Vec<Id>> {
    let role = match kind {
        ObjectKind::Agent => "setup_agents",
        ObjectKind::Machine => "setup_machines",
        ObjectKind::Template => "setup_templates",
        _ => return Err(ApiError::internal("the setup offer source is not declared")),
    };
    let slot = plan
        .variant()
        .facets
        .iter()
        .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
        .ok_or_else(|| ApiError::internal("the setup offer role is absent"))?;
    // Hosted physical machines use their existing Host permission-page identity. The
    // returned DTO still names the physical machine; this source facet remains MachineRead.
    let facades = if kind == ObjectKind::Machine {
        orch_db::queries::resources::hosting::machine_hosts(conn, ids)
            .await?
            .into_iter()
            .map(|(machine, host, _)| (host, machine))
            .collect::<HashMap<_, _>>()
    } else {
        HashMap::new()
    };
    let mut source_ids = ids.to_vec();
    source_ids.extend(facades.keys().copied());
    let mut graph = Graph::default();
    let rows = resources::read_into(conn, &mut graph, &source_ids)
        .await
        .map_err(ApiError::internal)?;
    let mut targets = HashMap::new();
    for row in rows {
        let selected_id = if ObjectKind::from(row.kind) == kind {
            Some(row.id)
        } else if row.kind == orch_access_contract::permissions::ResourceKind::Host {
            facades.get(&row.id).copied()
        } else {
            None
        };
        let Some(id) = selected_id else {
            continue;
        };
        if let Some(target) = offer_target(&graph, &row) {
            targets.insert(id, target);
        }
    }
    let resource = KeyTargetKind::Object(kind);
    let mut chosen = Vec::new();
    let mut facts = Vec::new();
    for id in ids {
        let Some(target) = targets.remove(id) else {
            continue;
        };
        if plan.permits_binding(caller, slot, resource, std::slice::from_ref(&target))? {
            chosen.push(*id);
            facts.push(target);
        }
    }
    if facts.is_empty() {
        plan.bind_empty(slot)?;
    } else {
        plan.bind(slot, resource, &facts)?;
    }
    Ok(chosen)
}

/// Profiles are selected before any profile-derived response or catalogue suppression.
pub(super) async fn agents(
    conn: &mut Conn,
    caller: &Caller,
    plan: &mut OperationPlan,
    ids: &[Id],
) -> ApiResult<Vec<Id>> {
    if !scoped(caller) {
        // An ordinary/legacy request does not need a new source-scope proof. Keep its
        // original reader and lifecycle policy, including already-selected declarations.
        return Ok(ids.to_vec());
    }
    selected(conn, caller, plan, ObjectKind::Agent, ids).await
}

async fn finish(
    conn: &mut Conn,
    principal: &Principal,
    caller: &Caller,
    plan: &mut OperationPlan,
    existing: bool,
    mut reply: ProviderSetup,
) -> ApiResult<ProviderSetup> {
    if !scoped(caller) {
        return Ok(reply);
    }
    // A selected existing keeper reference is intrinsic root context. Additional machine
    // capability choices and program declarations are actual external offer payloads.
    let mut machines: Vec<_> = reply
        .machines
        .iter()
        .flatten()
        .map(|choice| choice.machine.id)
        .collect();
    machines.extend(
        reply
            .sources
            .iter()
            .filter_map(|source| source.machine.as_ref().map(|m| m.id)),
    );
    if !existing {
        machines.extend(reply.key_machines.iter().map(|machine| machine.id));
    }
    let machines: HashSet<_> = selected(conn, caller, plan, ObjectKind::Machine, &machines)
        .await?
        .into_iter()
        .collect();
    if let Some(choices) = &mut reply.machines {
        choices.retain(|choice| machines.contains(&choice.machine.id));
    }
    if !existing {
        reply
            .key_machines
            .retain(|machine| machines.contains(&machine.id));
    }
    // The default subscription's simple TemplateRef stays intrinsic setup context,
    // like ProviderGet's runtime reference. Full selectable template/program offers bind.
    let mut templates: Vec<_> = reply
        .runtime_templates
        .iter()
        .map(|template| template.id)
        .collect();
    templates.extend(
        reply
            .sources
            .iter()
            .filter_map(|source| source.template.as_ref().map(|t| t.id)),
    );
    let templates: HashSet<_> = selected(conn, caller, plan, ObjectKind::Template, &templates)
        .await?
        .into_iter()
        .collect();
    reply
        .runtime_templates
        .retain(|template| templates.contains(&template.id));
    reply.sources.retain(|source| match source.by {
        SourceBy::Builtin => true,
        SourceBy::Program => {
            let machine = source.machine.as_ref().map(|m| machines.contains(&m.id));
            let template = source.template.as_ref().map(|t| templates.contains(&t.id));
            (machine.is_some() || template.is_some())
                && machine.unwrap_or(true)
                && template.unwrap_or(true)
        }
        SourceBy::Default | SourceBy::Address | SourceBy::None => false,
    });
    let ids: Vec<_> = reply
        .subscriptions_withheld
        .iter()
        .chain(&reply.subscriptions_unshared)
        .filter_map(|policy| policy.scope.as_ref().map(|node| node.id))
        .collect();
    let names = NameSources {
        nodes: ids.iter().copied().collect(),
        ..NameSources::default()
    };
    let bounds = projection_names(conn, caller, plan, names, &NAMES).await?;
    let refs = bounds.refs(conn, principal, &ids).await?;
    for policy in reply
        .subscriptions_withheld
        .iter_mut()
        .chain(&mut reply.subscriptions_unshared)
    {
        if let Some(node) = &mut policy.scope {
            *node = refs.get(&node.id).cloned().unwrap_or_else(|| {
                let mut hidden = node.clone();
                hidden.name = None;
                hidden.redacted = true;
                hidden.thread_display = None;
                hidden
            });
        }
    }
    Ok(reply)
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: ProviderSetupGetQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let existing = query.provider_id.is_some();
        let target = match query.provider_id {
            Some(id) => {
                targets::page_object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Provider,
                        id,
                    },
                )
                .await?
            }
            None => {
                targets::page_object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Org,
                        id: query.org_id,
                    },
                )
                .await?
            }
        };
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        plan.bind(0, PROVIDER, &[target])?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        let reply = Box::pin(super::setup::prepare_reply(
            ctx, conn, &principal, caller, &mut plan, query,
        ))
        .await?;
        let reply = finish(conn, &principal, caller, &mut plan, existing, reply).await?;
        let response = plan.clone();
        Ok(plan.with_data(PreparedSetup { response, reply }))
    })
}

pub(super) async fn get(
    ctx: Ctx,
    _query: ProviderSetupGetQuery,
    (): (),
) -> ApiResult<ProviderSetup> {
    // Only this effect-free database reader opts into the existing one-fresh-snapshot
    // retry. The original request and accepted/current credential ceilings are retained.
    crate::key_operations::snapshot_read::run(ctx, setup_snapshot).await
}

async fn setup_snapshot(ctx: Ctx) -> ApiResult<ProviderSetup> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<PreparedSetup>()?;
    if !scoped(principal.caller()) {
        // The same freshly prepared ordinary projection needs no operation-key response
        // binding. Unresolved optional sources are never described as falsely empty.
        let reply = prepared.reply.clone();
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(reply);
    }
    let reply = prepared
        .response
        .clone()
        .read_response(&ctx, &mut tx, principal.caller(), move |_conn, _reader| {
            Box::pin(async move { Ok(prepared.reply.clone()) })
        })
        .await?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the provider setup response content has not been admitted",
            )
        })?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(reply)
}
