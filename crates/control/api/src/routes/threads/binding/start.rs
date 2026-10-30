//! Binding choices before a topic's first thread, judged in an existing environment or the resolved template.

use super::*;
use orch_api_contract::binding::{NodeBindingOptionsGet, NodeBindingOptionsGetQuery};
use orch_db::queries::hierarchy;
use orch_types::Json;
use serde_json::json;

pub(super) fn mount(registry: &mut Registry) {
    registry.on(NodeBindingOptionsGet, options);
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

async fn template_ground(
    conn: &mut Conn,
    principal: &Principal,
    node: &hierarchy::NodeDetail,
) -> ApiResult<Option<(environments::EnvironmentRow, EnvironmentOffers)>> {
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
    Ok(Some((environment, offers)))
}

async fn options(ctx: Ctx, query: NodeBindingOptionsGetQuery, (): ()) -> ApiResult<BindingOptions> {
    if query.environment_new && query.environment_id.is_some() {
        return Err(invalid(
            "environment_id",
            "choose either an existing environment or a new one",
        ));
    }
    let id = ctx.id("id")?;
    let mut timing = PreviewTiming::new(&ctx, id);
    let mut connection = ctx.app.db.acquire().await?;
    timing.phase("connection");
    let conn = &mut *connection;
    let principal = authorization::principal(&ctx, conn).await?;
    let principal = &principal;
    let node = hierarchy::details(conn, &[id], ctx.now)
        .await?
        .into_iter()
        .next()
        .filter(|node| node.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    let permission = match node.kind.as_str() {
        "topic" => Permission::TopicStartThreads,
        "project" => Permission::ProjectCreateTopics,
        _ => {
            return Err(invalid(
                "id",
                "a thread starts in a topic, project or folder",
            ));
        }
    };
    let decision = nodes::check(conn, principal, id, permission, Act::Use, Policy::default())
        .await
        .map_err(ApiError::internal)?;
    allowed(conn, decision).await?;
    timing.phase("authority");
    let settings = resolved::load(conn, id).await.map_err(ApiError::internal)?;
    let environment_default =
        match setting::<String>(conn, Place::Node(id), keys::ENV_THREAD_DEFAULT)
            .await
            .map_err(ApiError::internal)?
            .as_deref()
        {
            Some("fresh") => EnvironmentDefault::Fresh,
            _ => EnvironmentDefault::Shared,
        };
    let mut environments = environments_of(conn, principal, &ctx.app, id, id).await?;
    let existing = match query.environment_id {
        Some(environment) => Some(chosen_environment(conn, principal, environment, id).await?),
        None if !query.environment_new && environment_default == EnvironmentDefault::Shared => {
            match setting::<Id>(conn, Place::Node(id), keys::ENV_DEFAULT_ID)
                .await
                .map_err(ApiError::internal)?
            {
                Some(environment) => {
                    // Environments have Manage rather than Use. The shared list already
                    // includes only the caller's manageable environments and their readiness.
                    if environments
                        .iter()
                        .any(|entry| entry.environment.id == environment && entry.usable)
                    {
                        Some(chosen_environment(conn, principal, environment, id).await?)
                    } else {
                        None
                    }
                }
                None => None,
            }
        }
        None => None,
    };
    let judged = match existing {
        Some(environment) => {
            let offers = binding::environment_offers(conn, &environment)
                .await
                .map_err(ApiError::internal)?;
            Some((environment, offers))
        }
        None => template_ground(conn, principal, &node).await?,
    };
    let environment = judged.as_ref().map(|(environment, _)| environment);
    let offers = judged.as_ref().map(|(_, offers)| offers);
    let mut orch_tools = OrchToolsOf::read(
        conn,
        id,
        environment.filter(|row| !row.id.is_nil()).map(|row| row.id),
    )
    .await?;
    if let Some(row) = environment.filter(|row| row.id.is_nil()) {
        orch_tools.environment_off = row.config["exec.orch_tools"] == false;
    }
    let scope = match binding::allowed_set_at(conn, id)
        .await
        .map_err(ApiError::internal)?
    {
        Some(node) => node_view::refs(conn, principal, &[node], Policy::default())
            .await?
            .remove(&node),
        None => None,
    };
    let defaults = defaults_of(conn, principal, id).await?;
    timing.phase("settings_and_environment");
    let mut providers = providers_of(
        conn,
        principal,
        &ctx.app,
        &Ground {
            thread: id,
            settings: &settings,
            environment,
            offers,
            orch_tools: &orch_tools,
            scope: scope.as_ref(),
        },
    )
    .await?;
    timing.phase("providers");
    if node.kind == "project" {
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
    let (access, new_environment) = access_of(conn, principal, id).await?;
    timing.phase("defaults_and_access");
    let can_create = can_create_in(conn, principal, id, id).await?;
    let hosts = hosts_for(&ctx, conn, principal, id, &can_create).await;
    if environment.is_some_and(|row| row.id.is_nil())
        && can_create.allowed
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
    let mut answer = BindingOptions {
        defaults,
        providers,
        environments,
        environment_default,
        source: source_for(conn, principal, id).await?,
        access,
        new_environment,
        can_create,
        hosts,
        machines: Vec::new(),
    };
    answer.machines = machines_for(&ctx, conn, principal, &answer.environments).await?;
    timing.phase("placement_and_source");
    Ok(answer)
}
