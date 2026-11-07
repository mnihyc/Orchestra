//! Binding choices before a topic's first thread, judged in an existing environment or the resolved template.

use super::*;
use orch_api_contract::binding::{NodeBindingOptionsGet, NodeBindingOptionsGetQuery};
use orch_db::queries::hierarchy;
use orch_types::Json;
use serde_json::json;

mod operation;
mod prepared;
mod scope;

pub(super) fn mount(registry: &mut Registry) {
    registry.on_snapshot_read_with_optional_operation(
        NodeBindingOptionsGet,
        crate::key_operations::native::selected::<operation::Options>,
        crate::key_operations::native::selected_read::<operation::Options>,
    );
}

fn bound_ref(node: &mut NodeRef, bounds: &crate::routes::access::page::NameBounds) {
    if bounds.limited && !bounds.nodes.contains(&node.id) {
        node.name = None;
        node.thread_display = None;
        node.redacted = true;
    }
}
fn bound_summary(summary: &mut AccessSummary, bounds: &crate::routes::access::page::NameBounds) {
    if let Some(NodeOrSystem::Node(node)) = &mut summary.from {
        bound_ref(node, bounds);
    }
}

async fn render_selected(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &operation::Selected,
) -> ApiResult<BindingOptions> {
    let context = &value.context;
    let id = context.node.id;
    let environment = context.judged.as_ref().map(|(environment, _)| environment);
    let offers = context.judged.as_ref().map(|(_, offers)| offers);
    let scope = match context.policy_scope {
        Some(id) => value
            .projection
            .names
            .refs(conn, principal, &[id])
            .await?
            .remove(&id),
        None => None,
    };
    let defaults = defaults_of(conn, principal, id).await?;
    let mut providers = providers_from(
        conn,
        principal,
        &ctx.app,
        &Ground {
            thread: id,
            settings: &context.settings,
            environment,
            offers,
            orch_tools: &context.orch_tools,
            scope: scope.as_ref(),
        },
        &value.providers,
        Some(&value.inputs),
        Some(&value.projection),
    )
    .await?;
    let mut environments = environments_from(
        conn,
        principal,
        &ctx.app,
        id,
        Permission::EnvironmentUse,
        &value.environments,
        Some(&value.projection),
    )
    .await?;
    if context.node.kind == "project" {
        for entry in &mut environments {
            entry.is_default = false;
            let used_here =
                orch_db::queries::binding_start::used_in_project(conn, entry.environment.id, id)
                    .await?;
            entry.used = if used_here {
                EnvironmentUse::Project
            } else if entry.threads > 0 {
                EnvironmentUse::Elsewhere
            } else {
                EnvironmentUse::Unused
            };
        }
    }
    let (mut access, mut new_environment) = access_of(conn, principal, id).await?;
    bound_summary(&mut access.managed, &value.projection.names);
    bound_summary(&mut access.connected, &value.projection.names);
    bound_summary(&mut new_environment, &value.projection.names);
    let mut hosts = value.hosts.clone();
    crate::routes::environments::places::name_host_choices(
        conn,
        &mut hosts,
        Some(&value.projection.names),
    )
    .await?;
    let hosts = hosts.into_iter().map(|host| host.place).collect::<Vec<_>>();
    if environment.is_some_and(|row| row.id.is_nil())
        && value.can_create.allowed
        && !hosts.iter().any(|host| host.usable)
    {
        for provider in &mut providers {
            for agent in &mut provider.agents {
                if agent.usable {
                    agent.usable = false;
                    agent.reason = Some("no_host".to_owned());
                }
            }
        }
    }
    let mut source = source_for(conn, principal, id).await?;
    if let Some(node) = &mut source.from {
        bound_ref(node, &value.projection.names);
    }
    let ids = value
        .machines
        .iter()
        .map(|machine| machine.id)
        .collect::<Vec<_>>();
    let refs = machine_refs(&ctx.app, conn, &ids).await?;
    let machines = value
        .machines
        .iter()
        .filter_map(|row| refs.get(&row.id).cloned())
        .map(|machine| BindingMachine {
            environments: environments
                .iter()
                .filter(|entry| entry.machine.as_ref().is_some_and(|on| on.id == machine.id))
                .map(|entry| entry.environment.id)
                .collect(),
            machine,
        })
        .collect();
    Ok(BindingOptions {
        defaults,
        providers,
        environments,
        environment_default: context.environment_default,
        source,
        access,
        agent_access: Some(
            creation::availability(conn, principal, id, &value.query, environment).await?,
        ),
        new_environment,
        can_create: value.can_create.clone(),
        hosts,
        machines,
    })
}

async fn allowed(conn: &mut Conn, decision: Decision) -> ApiResult<()> {
    match decision {
        Decision::Allowed { .. } => Ok(()),
        Decision::Refused(refusal) => Err(refusals::denied(conn, refusal).await?),
    }
}

async fn chosen_environment(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    node: Id,
) -> ApiResult<environments::EnvironmentRow> {
    let row = environments::environment(conn, id)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.remove_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    let object = orch_access_contract::decisions::ObjectRef {
        kind: ObjectKind::Environment,
        id,
    };
    let decision = resources::needs(
        conn,
        principal,
        &resources::Needs {
            required: Permission::EnvironmentUse,
            anchor: object,
            through: Some(node),
            items: vec![resources::Need {
                object,
                permissions: vec![Permission::EnvironmentUse],
                options: None,
            }],
        },
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    allowed(conn, decision).await?;
    if matches!(row.status.as_str(), "failed" | "disabled" | "removed") {
        return Err(ApiError::new(
            ErrorCode::EnvironmentNoneUsable,
            "This environment is unavailable. Choose another environment or retry its start.",
        ));
    }
    Ok(row)
}

fn strings(value: Option<&Json>) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

async fn template_ground_source(
    conn: &mut Conn,
    principal: &Principal,
    node: &hierarchy::NodeDetail,
) -> ApiResult<Option<(environments::EnvironmentRow, EnvironmentOffers, Id)>> {
    let id = setting::<Id>(conn, Place::Node(node.id), keys::ENV_TEMPLATE_ID)
        .await
        .map_err(ApiError::internal)?
        .unwrap_or(orch_db::seeds::DEFAULT_TEMPLATE_ID);
    let template = orch_db::queries::resources::hosting::template(conn, id)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.kind == "environment");
    let Some(template) = template else {
        return Ok(None);
    };
    let offers = EnvironmentOffers {
        offers: strings(
            template
                .spec
                .get("exec")
                .and_then(|exec| exec.get("offers")),
        ),
        colocated: orch_agent_programs::Programs::colocated_for(&template.spec),
    };
    // No row is made for a preview. Nil identifies this template promise to the shared projector.
    let environment = environments::EnvironmentRow {
        id: Id::nil(),
        template_id: Some(id),
        name: String::new(),
        owner_id: principal.user(),
        org_id: node.org_id,
        placement: "managed".to_owned(),
        home_node_id: node.id,
        workspace_root: String::new(),
        config: json!({ "exec.orch_tools": template.spec.get("exec").and_then(|exec| exec.get("orch_tools")).cloned().unwrap_or(Json::Bool(true)) }),
        machine_id: None,
        generation: 0,
        status: "none".to_owned(),
        status_detail: None,
        facts: json!({}),
        remove_at: None,
        deleted_at: None,
    };
    Ok(Some((environment, offers, template.id)))
}
