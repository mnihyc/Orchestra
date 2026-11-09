//! MCP preset resources, environment-owned servers and imports whose definitions stay on their machine.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::invalid;
use crate::routes::resources::{Home, record, require, sealing};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_agent_contract::discovery::{McpAuthentication, McpServerSpec, McpTransport};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{self as api, McpServer};
use orch_authz::Principal;
use orch_db::queries::resources::mcp::{self as rows, McpRow};
use orch_db::{Conn, DbError};
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::BTreeMap;

/// Names are machine-facing components under the database's two-to-thirty-two-character rule.
const NAME_MIN: usize = 2;
const NAME_MAX: usize = 32;
/// The start command has a sixty-second deadline, so a definition's startup wait cannot exceed it.
const STARTUP_MAX_SECS: u32 = 60;

pub(crate) mod creation;
mod delete;
/// The same native deletion writer, entered only with the authenticated link's machine identity.
pub(crate) async fn forget_from_machine(
    app: &crate::state::App,
    machine: Id,
    server: Id,
    request: Id,
) -> ApiResult<bool> {
    delete::from_machine(app, machine, server, request).await
}
mod imports;
mod mutations;
pub(crate) mod projection;

/// Binds the preset's resource requests and reimport.
pub fn mount(registry: &mut Registry) {
    registry
        .on_with_operation(
            api::McpServerCreate,
            crate::key_operations::native::effects::prepare::<creation::Presets>,
            creation::create,
        )
        .creation(api::McpServerCreate, creation::binding())
        .on_with_operation(
            api::McpServerUpdate,
            crate::key_operations::native::effects::prepare::<mutations::Servers>,
            mutations::update,
        )
        .on_with_operation(
            api::McpServerDelete,
            crate::key_operations::native::effects::prepare::<delete::Delete>,
            delete::run,
        )
        .on_with_operation(
            api::McpServerReimport,
            crate::key_operations::native::effects::prepare::<
                imports::Imports<orch_engine::environment_mcp::existing::Reimport>,
            >,
            imports::reimport,
        )
        .on_with_operation(
            api::McpOfferingUse,
            crate::key_operations::native::effects::prepare::<
                imports::Imports<orch_engine::environment_mcp::existing::Offering>,
            >,
            imports::offering,
        );
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
    require_subject(conn, principal, row.id, row.environment_id, manages).await
}

/// The original guard on the stored subject, before reading a selected server's full spec.
pub(crate) async fn require_subject(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    environment: Option<Id>,
    manages: bool,
) -> ApiResult<()> {
    if let Some(environment) = environment {
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
            id,
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
    let prepared =
        projection::prepare(conn, principal, &[id], None, projection::Style::Individual).await?;
    projection::render(ctx, conn, principal, &prepared, None)
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)
}

/// Seals a server's own secret immediately; neither its spec nor its audit ever receives the value.
pub(crate) async fn put_secret(
    ctx: &Ctx,
    conn: &mut Conn,
    server: Id,
    name: &str,
    value: &str,
) -> ApiResult<()> {
    let sealed = seal_secret(server, name, value)?;
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

/// Prepare supplied secret bytes before the native writer reaches its first effect.
fn seal_secret(server: Id, name: &str, value: &str) -> ApiResult<Vec<u8>> {
    if value.len() > sealing::SECRET_MAX_BYTES {
        return Err(invalid(
            "secrets",
            "a secret exceeds the stored-value limit",
        ));
    }
    sealing::master_key()?
        .seal(
            value.as_bytes(),
            &sealing::binding("mcp_server", server, name),
        )
        .map_err(|_| ApiError::internal("the secret could not be sealed"))
}
