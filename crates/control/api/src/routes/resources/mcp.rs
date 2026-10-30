//! MCP preset resources, environment-owned servers and imports whose definitions stay on their machine.

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{invalid, placed, policy_at, view as node_view};
use crate::routes::resources::providers::{home_policies, refused_making};
use crate::routes::resources::view::{
    Holding, accesses, addresses, environment_refs, machine_refs,
};
use crate::routes::resources::{Home, begin_mutation, record, require, sealing};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_agent_contract::discovery::{McpAuthentication, McpServerSpec, McpTransport};
use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::NodeKind;
use orch_api_contract::resources::{self as api, McpServer};
use orch_authz::ownership::{self, Making, NewAccess};
use orch_authz::{Policy, Principal, resources};
use orch_db::queries::resources::mcp::{self as rows, McpRow};
use orch_db::{Conn, DbError};
use orch_link_contract::commands::{McpForget, SerialKey};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::BTreeMap;

/// Names are machine-facing components under the database's two-to-thirty-two-character rule.
const NAME_MIN: usize = 2;
const NAME_MAX: usize = 32;
/// The start command has a sixty-second deadline, so a definition's startup wait cannot exceed it.
const STARTUP_MAX_SECS: u32 = 60;

/// Binds the preset's resource requests and reimport.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::McpServerCreate, create)
        .on(api::McpServerUpdate, update)
        .on(api::McpServerDelete, delete)
        .on(api::McpServerReimport, reimport)
        .on(api::McpOfferingUse, use_offering);
}

/// Checks the name a harness and a machine use for a server.
pub(crate) fn name(value: &str) -> ApiResult<String> {
    let value = value.trim();
    if (NAME_MIN..=NAME_MAX).contains(&value.len())
        && value.starts_with(|c: char| c.is_ascii_lowercase())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
    {
        Ok(value.to_owned())
    } else {
        Err(invalid(
            "name",
            "use 2 to 32 lower case letters, digits, - or _, beginning with a letter",
        ))
    }
}

/// Refuses an incomplete transport, unknown fields and secret values disguised as public variables.
pub(crate) fn spec(value: &Json) -> ApiResult<Json> {
    const FIELDS: &[&str] = &[
        "transport",
        "command",
        "url",
        "authentication",
        "env",
        "secret_env",
        "working_dir",
        "startup_timeout_secs",
        "tools_allow",
        "tools_deny",
        "requires",
    ];
    if !value
        .as_object()
        .is_some_and(|value| value.keys().all(|field| FIELDS.contains(&field.as_str())))
    {
        return Err(invalid(
            "spec",
            "the server definition is an object with only declared fields",
        ));
    }
    if let Some(requires) = value.get("requires")
        && !requires.as_object().is_some_and(|value| {
            value
                .keys()
                .all(|key| matches!(key.as_str(), "commands" | "capabilities"))
        })
    {
        return Err(invalid(
            "spec.requires",
            "only commands and capabilities are declared",
        ));
    }
    let spec: McpServerSpec = serde_json::from_value(value.clone())
        .map_err(|_| invalid("spec", "a server field has the wrong type"))?;
    if spec.startup_timeout_secs == 0 || spec.startup_timeout_secs > STARTUP_MAX_SECS {
        return Err(invalid(
            "spec.startup_timeout_secs",
            "the startup wait is between 1 and 60 seconds",
        ));
    }
    if spec
        .secret_env
        .iter()
        .any(|name| spec.env.contains_key(name))
    {
        return Err(invalid(
            "spec.env",
            "a secret variable's value is supplied as a secret, never as a plain variable",
        ));
    }
    if let Some(McpAuthentication::Bearer { secret }) = &spec.authentication
        && (spec.transport != McpTransport::Http || !spec.secret_env.contains(secret))
    {
        return Err(invalid(
            "spec.authentication",
            "bearer authentication is for an HTTP server and names a declared secret",
        ));
    }
    match spec.transport {
        McpTransport::Stdio
            if spec
                .command
                .as_ref()
                .is_none_or(|argv| argv.first().is_none_or(String::is_empty)) =>
        {
            return Err(invalid("spec.command", "a stdio server names its program"));
        }
        McpTransport::Http => {
            let url = spec
                .url
                .as_deref()
                .and_then(|url| url::Url::parse(url).ok())
                .ok_or_else(|| {
                    invalid("spec.url", "an HTTP server names an HTTP or HTTPS address")
                })?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(invalid(
                    "spec.url",
                    "use HTTP or HTTPS without embedded credentials",
                ));
            }
        }
        _ => {}
    }
    serde_json::to_value(spec).map_err(ApiError::internal)
}

/// A public definition declares names, never values; replacements are sealed only after authority is checked.
fn secret_values(spec: &Json, values: Option<&BTreeMap<String, String>>) -> ApiResult<()> {
    let Some(values) = values else {
        return Ok(());
    };
    let declared = spec.get("secret_env").and_then(Json::as_array);
    for (name, value) in values {
        if !declared.is_some_and(|names| {
            names
                .iter()
                .any(|entry| entry.as_str() == Some(name.as_str()))
        }) {
            return Err(invalid(
                "secrets",
                "a secret value must belong to a declared secret variable",
            ));
        }
        if value.is_empty() || value.len() > sealing::SECRET_MAX_BYTES {
            return Err(invalid(
                "secrets",
                "enter a secret within the stored-value limit, or omit it to keep its saved value",
            ));
        }
    }
    Ok(())
}

fn home(row: &McpRow) -> Home {
    Home {
        node: row.home_id,
        org: row.org_id,
    }
}

/// Requires the server's own page, or the environment's page for one of its own servers.
pub(crate) async fn require_row(
    conn: &mut Conn,
    principal: &Principal,
    row: &McpRow,
    manages: bool,
) -> ApiResult<()> {
    if let Some(environment) = row.environment_id {
        if !manages
            && super::super::environments::reading::decide(conn, principal, environment).await?
                == super::super::environments::reading::Reading::Whole
        {
            return Ok(());
        }
        require(
            conn,
            principal,
            ResourceKind::Environment,
            environment,
            if manages {
                Permission::EnvironmentManage
            } else {
                Permission::EnvironmentView
            },
        )
        .await
    } else {
        require(
            conn,
            principal,
            ResourceKind::McpServer,
            row.id,
            if manages {
                Permission::McpManage
            } else {
                Permission::McpView
            },
        )
        .await
    }
}

/// Reads the server with no secret values; an import returns only its transport and safe origin.
pub(crate) async fn show(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<McpServer> {
    let row = rows::servers(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let people = row
        .owner_id
        .into_iter()
        .chain(
            row.spec
                .pointer("/imported/by")
                .and_then(Json::as_str)
                .and_then(|id| id.parse().ok()),
        )
        .collect::<Vec<_>>();
    let people = refusals::user_refs(conn, &people).await?;
    let homes = node_view::refs(
        conn,
        principal,
        &row.home_id.into_iter().collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    let addresses = if row.environment_id.is_none() {
        addresses(conn, &[id]).await?
    } else {
        Default::default()
    };
    let environment = if let Some(environment) = row.environment_id {
        environment_refs(conn, &[environment], &[], Time::new(ctx.now))
            .await?
            .into_iter()
            .next()
            .map(|(_, reference)| reference)
    } else {
        None
    };
    let current_offerings = rows::offerings(conn, &[id]).await?;
    let offering_machines = machine_refs(
        &ctx.app,
        conn,
        &current_offerings
            .iter()
            .map(|offering| offering.machine_id)
            .collect::<Vec<_>>(),
    )
    .await?;
    // An environment's own servers inherit its page and have no native offerings.
    let can_manage = !current_offerings.is_empty()
        && row.environment_id.is_none()
        && resources::check(
            conn,
            principal,
            ObjectRef {
                kind: ResourceKind::McpServer.into(),
                id,
            },
            Permission::McpManage,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .is_allowed();
    let mut can_use = BTreeMap::new();
    for offering in &current_offerings {
        let allowed = if can_manage && (offering.state == "found" || offering.proposal.is_some()) {
            if let Some(environment) = offering
                .proposal
                .as_ref()
                .and_then(|proposal| proposal["environment"].as_str())
                .and_then(|id| id.parse::<Id>().ok())
                .or(offering.environment_id)
            {
                super::super::environments::mcp::may_import(conn, principal, environment).await?
            } else {
                false
            }
        } else {
            false
        };
        can_use.insert(offering.machine_id, allowed);
    }
    let offerings = current_offerings
        .iter()
        .filter_map(|offering| {
            offering_machines
                .get(&offering.machine_id)
                .cloned()
                .map(|machine| api::McpServerOffering {
                    different_definition: offering.proposal.is_some()
                        || offering.reason.as_deref() == Some("different_definition"),

                    definition_fingerprint: offering
                        .proposal
                        .as_ref()
                        .map(|proposal| &proposal["source"])
                        .unwrap_or(&offering.source)
                        .get("definition_fingerprint")
                        .cloned()
                        .and_then(|value| serde_json::from_value(value).ok()),
                    can_use_definition: can_use.get(&offering.machine_id).copied().unwrap_or(false),
                    machine,
                    state: match offering.state.as_str() {
                        "found" => api::McpOfferingState::Found,
                        "ready" => api::McpOfferingState::Ready,
                        "pending" => api::McpOfferingState::Pending,
                        _ => api::McpOfferingState::Failed,
                    },
                    offered_at: offering.offered_at.map(Time::new),
                    seen_at: Time::new(offering.seen_at),
                    reason: offering
                        .proposal
                        .as_ref()
                        .and_then(|proposal| proposal["reason"].as_str())
                        .map(str::to_owned)
                        .or_else(|| offering.reason.clone()),
                })
        })
        .collect();
    let machine = if let Some(machine) = row.machine_id {
        machine_refs(&ctx.app, conn, &[machine])
            .await?
            .remove(&machine)
    } else {
        None
    };
    let access = if let Some(environment) = row.environment_id {
        let held = resources::effective(
            conn,
            principal,
            ObjectRef {
                kind: ResourceKind::Environment.into(),
                id: environment,
            },
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?;
        accesses(
            conn,
            principal,
            &[Holding {
                admin_only: &held.admin_only,
                admin_access: held.admin_access,
                admin_place: held.admin_place,
                participating: held.participating,
                id: environment,
                owner: held.owner,
                administers: held.administers,
                held: &held.held,
            }],
        )
        .await?
        .remove(&environment)
    } else {
        let held = resources::effective(
            conn,
            principal,
            ObjectRef {
                kind: ResourceKind::McpServer.into(),
                id,
            },
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?;
        accesses(
            conn,
            principal,
            &[Holding {
                admin_only: &held.admin_only,
                admin_access: held.admin_access,
                admin_place: held.admin_place,
                participating: held.participating,
                id,
                owner: held.owner,
                administers: held.administers,
                held: &held.held,
            }],
        )
        .await?
        .remove(&id)
    }
    .ok_or_else(|| ApiError::internal("the server's access was not read"))?;
    let imported = if let Some(imported) = row.spec.get("imported") {
        let by: Id = serde_json::from_value(imported["by"].clone()).map_err(ApiError::internal)?;
        Some(api::McpServerImported {
            source: imported["source"].as_str().unwrap_or_default().to_owned(),
            scope: serde_json::from_value(imported["scope"].clone()).map_err(ApiError::internal)?,
            directory: imported["directory"].as_str().map(str::to_owned),
            name: imported["name"].as_str().unwrap_or_default().to_owned(),
            at: serde_json::from_value(imported["at"].clone()).map_err(ApiError::internal)?,
            by: people.get(&by).cloned().ok_or_else(ApiError::not_found)?,
            stale: import_is_stale(conn, &row).await?,
        })
    } else {
        None
    };
    let address = addresses
        .get(&id)
        .cloned()
        .or_else(|| {
            environment
                .as_ref()
                .map(|environment| format!("{}.{}", environment.address, row.name))
        })
        .unwrap_or_default();
    let enabled_places: i64 = if !row.enabled {
        0
    } else if row.environment_id.is_some() {
        1
    } else {
        sqlx::query_scalar("SELECT
          (SELECT count(*) FROM settings s JOIN nodes n ON n.id=s.node_id AND n.deleted_at IS NULL WHERE s.key='mcp.enabled' AND s.value->>$1::uuid::text='true') +
          (SELECT count(*) FROM environments WHERE deleted_at IS NULL AND config->'mcp.enabled'->>$1::uuid::text='true')")
            .bind(id).fetch_one(&mut *conn).await.map_err(DbError::from)?
    };
    Ok(McpServer {
        offerings,
        enabled_places: Some(enabled_places),
        id,
        name: row.name,
        description: row.description,
        org_id: row.org_id,
        owner: row.owner_id.and_then(|owner| people.get(&owner).cloned()),
        home: row.home_id.and_then(|home| homes.get(&home).cloned()),
        address,
        machine,
        environment,
        imported,
        spec: if row.imported {
            json!({"transport":row.spec["transport"]})
        } else {
            row.spec
        },
        enabled: row.enabled,
        revision: row.revision,
        access,
    })
}

async fn import_is_stale(conn: &mut Conn, row: &McpRow) -> ApiResult<bool> {
    let offerings = rows::offerings(conn, &[row.id]).await?;
    for offering in offerings
        .into_iter()
        .filter(|offering| offering.state == "ready")
    {
        let origin = &offering.source;
        if rows::found_on_machine(conn, offering.machine_id)
            .await?
            .iter()
            .any(|found| {
                found["source"] == origin["source"]
                    && (found["scope"] == origin["scope"]
                        || (found["scope"] == "directory" && origin["scope"] == "project"))
                    && found["name"] == origin["name"]
                    && found["directory"] == origin["directory"]
                    && found["digest"] != origin["digest"]
            })
        {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn create(ctx: Ctx, (): (), body: api::McpServerCreateBody) -> ApiResult<McpServer> {
    let name = name(&body.name)?;
    let spec = spec(&body.spec)?;
    secret_values(&spec, body.secrets.as_ref())?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let (org, members, policy) = match body.home_id {
        Some(home) => {
            let (place, kind) = placed(&mut tx, home).await?;
            if kind != NodeKind::Org {
                return Err(invalid("home_id", "a preset is at home in an org"));
            }
            (
                Some(place.org),
                home_policies(&mut tx, home).await?.members_make,
                policy_at(&mut tx, home).await?,
            )
        }
        None => (None, Vec::new(), Policy::default()),
    };
    match ownership::may_make(
        &mut tx,
        &principal,
        ResourceKind::McpServer,
        body.home_id,
        &members,
        policy,
    )
    .await
    {
        Ok(Making::Allowed(_)) => {}
        Ok(Making::Refused(why)) => {
            return Err(match body.home_id {
                Some(home) => refused_making(&mut tx, home, why).await,
                None => {
                    refusals::system_admin_only("only a system admin makes a system MCP preset")
                }
            });
        }
        Err(error) => return Err(refusals::problem(&mut tx, error).await),
    }
    let row = McpRow {
        imported: false,
        id: orch_types::new_id(),
        name,
        description: body.description.unwrap_or_default().trim().to_owned(),
        org_id: org,
        home_id: body.home_id,
        owner_id: Some(principal.user()),
        machine_id: None,
        environment_id: None,
        spec,
        enabled: true,
        revision: 1,
    };
    rows::insert(&mut tx, &row, ctx.now).await?;
    for (name, value) in body.secrets.as_ref().into_iter().flatten() {
        put_secret(&ctx, &mut tx, row.id, name, value).await?;
    }
    ownership::start_resource(
        &mut tx,
        ObjectRef {
            kind: ResourceKind::McpServer.into(),
            id: row.id,
        },
        principal.user(),
        match body.access {
            None | Some(NewResourceAccess::Defaults) => NewAccess::Defaults,
            Some(NewResourceAccess::OnlyMe) => NewAccess::OnlyMe,
            Some(NewResourceAccess::Home) => NewAccess::Home,
        },
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::internal)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::McpCreated),
        row.id,
        home(&row),
        "Added an MCP preset",
        json!({"server":row.id,"name":row.name}),
    )
    .await?;
    let result = show(&ctx, &mut tx, &principal, row.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(result)
}

async fn update(ctx: Ctx, (): (), body: api::McpServerUpdateBody) -> ApiResult<McpServer> {
    let clean_name = body.name.as_deref().map(name).transpose()?;
    let clean_spec = body.spec.as_ref().map(spec).transpose()?;
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let mut row = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    require_row(&mut tx, &principal, &row, true).await?;
    if row.revision != body.expected_revision {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the server changed since it was read",
        )
        .with("current", show(&ctx, &mut tx, &principal, id).await?));
    }
    if row.imported && (body.spec.is_some() || body.secrets.is_some()) {
        return Err(invalid(
            "spec",
            "an imported definition changes only by reimporting it from its machine",
        ));
    }
    if let Some(name) = clean_name {
        row.name = name;
    }
    if let Some(description) = body.description {
        row.description = description.trim().to_owned();
    }
    if let Some(spec) = clean_spec {
        row.spec = spec;
    }
    secret_values(&row.spec, body.secrets.as_ref())?;
    for (name, value) in body.secrets.as_ref().into_iter().flatten() {
        put_secret(&ctx, &mut tx, row.id, name, value).await?;
    }
    let was_enabled = row.enabled;
    if let Some(enabled) = body.enabled {
        row.enabled = enabled;
    }
    rows::update(&mut tx, &row, ctx.now).await?;
    if !was_enabled && row.enabled {
        record(
            &ctx,
            &mut tx,
            orch_db::audit::action!(AuditAction::McpEnabled),
            id,
            home(&row),
            "Enabled an MCP server",
            json!({"server":id,"name":row.name}),
        )
        .await?;
    }
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::McpUpdated),
        id,
        home(&row),
        "Changed an MCP server",
        json!({"server":id,"name":row.name,"definition":body.spec.is_some(),"enabled":row.enabled}),
    )
    .await?;
    let result = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(result)
}

async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    rows::lock_definition(&mut tx, id).await?;
    let row = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    require_row(&mut tx, &principal, &row, true).await?;
    for offering in rows::offerings(&mut tx, &[id]).await? {
        if orch_db::queries::resources::machine(&mut tx, offering.machine_id)
            .await?
            .is_some_and(|machine| machine.online && machine.is_active())
        {
            let _ = ctx
                .app
                .link
                .dispatcher()
                .run(
                    offering.machine_id,
                    &SerialKey::Machine,
                    &McpForget { server_id: id },
                )
                .await;
        }
    }
    rows::delete(&mut tx, id, ctx.now).await?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::McpDeleted),
        id,
        home(&row),
        "Deleted an MCP server",
        json!({"server":id,"name":row.name}),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

async fn reimport(ctx: Ctx, (): (), (): ()) -> ApiResult<McpServer> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    rows::lock_definition(&mut tx, id).await?;
    let mut row = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    require_row(&mut tx, &principal, &row, true).await?;
    if !row.imported {
        return Err(invalid("id", "this server was not imported from a machine"));
    }
    let offering = rows::offerings(&mut tx, &[id])
        .await?
        .into_iter()
        .find(|offering| Some(offering.machine_id) == row.machine_id)
        .or(rows::offerings(&mut tx, &[id]).await?.into_iter().next())
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::SourceOffline,
                "No machine offers this imported server",
            )
        })?;
    if offering
        .proposal
        .as_ref()
        .is_some_and(|proposal| proposal["reason"] == "different_definition")
    {
        return Err(ApiError::new(
            ErrorCode::PlanBlocked,
            "Review the different definition and choose Use this machine's",
        ));
    }
    let machine = offering.machine_id;
    row.machine_id = Some(machine);
    if offering.source.is_object() {
        let mut origin = offering.source;
        if origin["scope"] == "directory" {
            origin["scope"] = json!("project");
        }
        let old = row
            .spec
            .get("imported")
            .cloned()
            .unwrap_or_else(|| json!({}));
        for key in ["by", "at"] {
            if origin.get(key).is_none() {
                origin[key] = old[key].clone();
            }
        }
        row.spec["imported"] = origin;
    }
    let mut chosen = None;
    for environment in rows::on_machine(&mut tx, machine).await? {
        if super::super::environments::mcp::may_import(&mut tx, &principal, environment).await? {
            chosen = Some(environment);
            break;
        }
    }
    let environment = chosen.ok_or_else(|| {
        ApiError::new(
            ErrorCode::AuthzDenied,
            "no environment of this machine permits this caller to reimport",
        )
    })?;
    super::super::environments::mcp::reimport(&ctx, &mut tx, &principal, row, environment).await?;
    let result = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(result)
}

/// Seals a server's own secret immediately; neither its spec nor its audit ever receives the value.
pub(crate) async fn put_secret(
    ctx: &Ctx,
    conn: &mut Conn,
    server: Id,
    name: &str,
    value: &str,
) -> ApiResult<()> {
    if value.len() > sealing::SECRET_MAX_BYTES {
        return Err(invalid(
            "secrets",
            "a secret exceeds the stored-value limit",
        ));
    }
    let sealed = sealing::master_key()?
        .seal(
            value.as_bytes(),
            &sealing::binding("mcp_server", server, name),
        )
        .map_err(|_| ApiError::internal("the secret could not be sealed"))?;
    orch_db::queries::settings::put_secret(
        conn,
        "mcp_server",
        server,
        name,
        &sealed,
        Some(ctx.user()?.id()),
        ctx.now,
    )
    .await?;
    Ok(())
}

async fn use_offering(ctx: Ctx, (): (), body: api::McpOfferingUseBody) -> ApiResult<McpServer> {
    let id = ctx.id("id")?;
    let machine = ctx.id("machine_id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    rows::lock_definition(&mut tx, id).await?;
    let mut row = rows::lock(&mut tx, id)
        .await?
        .filter(|row| row.imported)
        .ok_or_else(ApiError::not_found)?;
    require_row(&mut tx, &principal, &row, true).await?;
    if row.revision != body.expected_revision {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "The server changed; review its offerings again",
        ));
    }
    let offering = rows::offerings(&mut tx, &[id])
        .await?
        .into_iter()
        .find(|offering| {
            offering.machine_id == machine
                && (offering.state == "found" || offering.proposal.is_some())
        })
        .ok_or_else(ApiError::not_found)?;
    let candidate = offering
        .proposal
        .as_ref()
        .map(|proposal| proposal["source"].clone())
        .unwrap_or_else(|| offering.source.clone());
    let source_environment = offering
        .proposal
        .as_ref()
        .and_then(|proposal| proposal["environment"].as_str())
        .and_then(|id| id.parse::<Id>().ok())
        .or(offering.environment_id);
    let environment = source_environment.ok_or_else(|| {
        ApiError::new(
            ErrorCode::SourceOffline,
            "The discovered environment is no longer registered",
        )
    })?;
    if !super::super::environments::mcp::may_import(&mut tx, &principal, environment).await? {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "Importing this machine's definition needs authority over its environment",
        ));
    }
    let reported = candidate
        .get("definition_fingerprint")
        .cloned()
        .and_then(|value| serde_json::from_value::<Option<orch_types::Hash>>(value).ok())
        .flatten();
    if reported != body.expected_fingerprint {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "The reported definition changed; review it again",
        ));
    }
    let reply: orch_link_contract::commands::McpImportResult = ctx
        .app
        .link
        .dispatcher()
        .run(
            machine,
            &SerialKey::Environment(environment),
            &orch_link_contract::commands::McpImport {
                expected_fingerprint: reported,
                environment_id: environment,
                server_id: id,
                source: candidate["source"].as_str().unwrap_or_default().into(),
                scope: candidate["scope"].as_str().unwrap_or_default().into(),
                name: candidate["name"].as_str().unwrap_or_default().into(),
                directory: candidate["directory"].as_str().map(str::to_owned),
            },
        )
        .await
        .map_err(|_| {
            ApiError::new(
                ErrorCode::SourceOffline,
                "The machine could not copy this definition",
            )
        })?;
    if reported.is_some() && reply.definition_fingerprint != reported {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "The definition changed during import; review it again",
        ));
    }
    let old = row
        .spec
        .pointer("/imported/definition_fingerprint")
        .cloned();
    let mut origin = candidate;
    if origin["scope"] == "directory" {
        origin["scope"] = json!("project");
    }
    origin["definition_fingerprint"] = json!(reply.definition_fingerprint);
    origin["digest"] = json!(reply.digest);
    origin["tools"] = json!(reply.tools);
    origin["at"] = json!(Time::new(ctx.now));
    origin["by"] = json!(principal.user());
    row.spec = json!({"transport":reply.transport,"imported":origin});
    rows::select_definition(&mut tx, &row, machine, environment, ctx.now).await?;
    record(&ctx,&mut tx,orch_db::audit::action!(AuditAction::McpImported),id,home(&row),"Selected a discovered MCP definition",json!({"machine":machine,"definition_before":old,"definition_after":row.spec.pointer("/imported/definition_fingerprint")})).await?;
    let result = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(result)
}
