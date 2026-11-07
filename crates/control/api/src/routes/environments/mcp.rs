//! The MCP servers an environment enables, imports copied on their machine and servers written for this environment alone.

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{invalid, view as node_view};
use crate::routes::resources::view::{addresses, machine_refs_from_rows};
use crate::routes::resources::{Home, begin_mutation, mcp as resources, record, require};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::{
    self as api, EnvironmentMcp, McpFrom, McpImportReason, McpImportRight, McpState,
};
use orch_api_contract::resources::McpServer;
use orch_authz::ownership::{self, NewAccess};
use orch_authz::{Policy, Principal};
use orch_db::queries::environments::hosting as environments;
use orch_db::queries::resources::mcp::{self as rows, McpRow};
use orch_db::{Conn, DbError};
use orch_link_contract::commands::{McpForget, McpImportResult, SerialKey};
use orch_link_contract::policy::LocalPolicy;
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::{BTreeSet, HashMap};

mod create_sources;
mod import_sources;
mod read_sources;

/// Binds the environment's enabled set, import and own-server requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_snapshot_read_with_optional_operation(
            api::EnvironmentMcpGet,
            crate::key_operations::native::selected::<read_sources::Servers>,
            crate::key_operations::native::selected_read::<read_sources::Servers>,
        )
        .on(api::EnvironmentMcpServerRestart, restart)
        .on_with_operation(
            api::EnvironmentMcpImportCreate,
            crate::key_operations::native::effects::prepare::<import_sources::Imports>,
            import,
        )
        .on_with_operation(
            api::EnvironmentMcpServerCreate,
            crate::key_operations::native::effects::prepare::<create_sources::OwnServers>,
            create,
        );
}

/// Maps the connector's directory scope into the API's project scope without exposing a definition.
pub(super) fn found_view(found: &Json) -> Option<api::McpFound> {
    let mut found = found.clone();
    if found["scope"] == "directory" {
        found["scope"] = json!("project");
    }
    serde_json::from_value(found).ok()
}

fn policy_error(error: orch_engine::environment_mcp::Error) -> ApiError {
    match error {
        orch_engine::environment_mcp::Error::NotFound => ApiError::not_found(),
        orch_engine::environment_mcp::Error::Invalid { field, message } => invalid(field, message),
        orch_engine::environment_mcp::Error::Offline => ApiError::new(
            ErrorCode::SourceOffline,
            "the environment has no connected machine",
        ),
        orch_engine::environment_mcp::Error::Internal(error) => ApiError::internal(error),
    }
}

async fn machine_policy(
    conn: &mut Conn,
    environment: Id,
) -> ApiResult<(
    environments::EnvironmentState,
    Option<orch_db::queries::resources::MachineRow>,
    LocalPolicy,
)> {
    orch_engine::environment_mcp::machine_policy(conn, environment)
        .await
        .map_err(policy_error)
}

/// The native producer is also used by retained command sources, with no permission copy.
pub(crate) async fn may_import(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
) -> ApiResult<bool> {
    orch_engine::environment_mcp::may_import(conn, principal, environment)
        .await
        .map_err(policy_error)
}

/// The same native whole-Environment reading door as the original server view.
async fn require_get(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<()> {
    if super::reading::decide(conn, principal, id).await? != super::reading::Reading::Whole {
        require(
            conn,
            principal,
            ResourceKind::Environment,
            id,
            Permission::EnvironmentView,
        )
        .await?;
    }
    Ok(())
}

/// Selected native definitions and observations, before public references are constructed.
pub(super) struct PreparedGet {
    environment: Id,
    resolved: Vec<orch_engine::mcp::ResolvedServer>,
    addresses: HashMap<Id, String>,
    machines: Vec<orch_db::queries::resources::routes::MachineNameRow>,
    node_ids: BTreeSet<Id>,
    node_disclosure: node_view::ReferenceDisclosure,
    states: Vec<(String, String)>,
    runtime_states: Vec<orch_link_contract::events::EnvironmentFactsMcpServer>,
    status: String,
    applies: api::McpApplies,
    library: Vec<orch_engine::resource_facts::RawFacts>,
    library_subjects: BTreeSet<Id>,
    library_visibility: orch_engine::resource_facts::PreparedVisibility,
    import_allowed: bool,
    import_permitted: bool,
    found: Vec<api::McpFound>,
}

async fn prepare_get(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    resolved: Vec<orch_engine::mcp::ResolvedServer>,
) -> ApiResult<PreparedGet> {
    let server_ids: Vec<Id> = resolved
        .iter()
        .filter(|server| server.row.environment_id.is_none())
        .map(|server| server.row.id)
        .collect();
    let addresses = addresses(conn, &server_ids).await?;
    let machine_ids: Vec<Id> = resolved
        .iter()
        .filter_map(|server| server.row.machine_id)
        .collect();
    let machines = orch_db::queries::resources::routes::machine_names(conn, &machine_ids).await?;
    let node_ids: BTreeSet<_> = resolved
        .iter()
        .filter_map(|server| match &server.from {
            Some(McpFrom::Node(node)) => Some(node.id),
            _ => None,
        })
        .collect();
    let node_disclosure = node_view::reference_disclosure(
        conn,
        principal,
        &node_ids.iter().copied().collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    let states = rows::states(conn, environment).await?;
    let (env, _, policy) = machine_policy(conn, environment).await?;
    let runtime_states =
        serde_json::from_value(env.facts.get("mcp_servers").cloned().unwrap_or(json!([])))
            .unwrap_or_default();
    let applies = if rows::reload_available(conn, environment).await? {
        api::McpApplies::Immediately
    } else {
        api::McpApplies::NextTurn
    };
    let objects: Vec<_> = resolved
        .iter()
        .map(|server| ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::McpServer,
            id: server.row.id,
        })
        .collect();
    let readable = crate::routes::resources::library::readable(conn, principal, &objects).await?;
    let subjects: Vec<_> = objects
        .into_iter()
        .filter(|object| readable.contains(object))
        .map(|object| orch_api_contract::resource_facts::FactSubject::Resource { object })
        .collect();
    let library_subjects = subjects
        .iter()
        .filter_map(|subject| match subject {
            orch_api_contract::resource_facts::FactSubject::Resource { object } => Some(object.id),
            _ => None,
        })
        .collect();
    let mut library = orch_engine::resource_facts::read_with(
        conn,
        &subjects,
        Time::new(ctx.now),
        Some(&orch_engine::resource_facts::library::RecordedUses),
    )
    .await
    .map_err(ApiError::internal)?;
    crate::routes::everything::library::select_relations(&mut library);
    let library_visibility = orch_engine::resource_facts::prepare_projection(
        conn,
        principal,
        Policy::default(),
        &library,
    )
    .await
    .map_err(ApiError::internal)?;
    let import_allowed = may_import(conn, principal, environment).await?;
    let found = policy
        .mcp_import
        .then_some(&env.facts)
        .and_then(|facts| facts.get("mcp_found"))
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(found_view)
        .collect();
    Ok(PreparedGet {
        environment,
        resolved,
        addresses,
        machines,
        node_ids,
        node_disclosure,
        states,
        runtime_states,
        status: env.status,
        applies,
        library,
        library_subjects,
        library_visibility,
        import_allowed,
        import_permitted: policy.mcp_import,
        found,
    })
}

impl PreparedGet {
    async fn render(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<EnvironmentMcp> {
        let machines = machine_refs_from_rows(&ctx.app, self.machines.clone())?;
        let nodes = node_view::refs(
            conn,
            principal,
            &self.node_ids.iter().copied().collect::<Vec<_>>(),
            Policy::default(),
        )
        .await?;
        let mut library: HashMap<_, _> = self
            .library_visibility
            .project(&self.library)
            .into_iter()
            .filter_map(|facts| match &facts.subject {
                orch_api_contract::resource_facts::FactSubject::Resource { object } => {
                    Some((object.id, facts))
                }
                _ => None,
            })
            .collect();
        let mut servers = Vec::new();
        for server in &self.resolved {
            let hosted = self
                .runtime_states
                .iter()
                .find(|state| state.name == server.row.name);
            let state = if server.reason.is_some() {
                McpState::Unavailable
            } else if self.status != "running" {
                McpState::Stopped
            } else if let Some(hosted) = hosted {
                match hosted.state.as_str() {
                    "starting" => McpState::Starting,
                    "running" => McpState::Running,
                    "failed" => McpState::Failed,
                    _ => McpState::Stopped,
                }
            } else {
                match self
                    .states
                    .iter()
                    .find(|(name, _)| *name == server.row.name)
                    .map(|(_, state)| state.as_str())
                {
                    Some("succeeded") => McpState::Running,
                    Some("failed" | "expired" | "cancelled") => McpState::Failed,
                    _ => McpState::Stopped,
                }
            };
            let from = match server.from.clone() {
                Some(McpFrom::Node(node)) => nodes.get(&node.id).cloned().map(McpFrom::Node),
                other => other,
            };
            servers.push(api::EnvironmentMcpServer {
                library: library.remove(&server.row.id),
                server: api::EnvironmentMcpServerRef {
                    id: server.row.id,
                    name: server.row.name.clone(),
                    address: self
                        .addresses
                        .get(&server.row.id)
                        .cloned()
                        .unwrap_or_default(),
                    machine: server
                        .row
                        .machine_id
                        .and_then(|machine| machines.get(&machine).cloned()),
                    own: server.row.environment_id == Some(self.environment),
                },
                from,
                state,
                reason: server.reason.clone().or_else(|| {
                    (state == McpState::Failed).then(|| {
                        hosted
                            .and_then(|hosted| hosted.reason.clone())
                            .unwrap_or_else(|| "the last start attempt failed".to_owned())
                    })
                }),
                tools: server
                    .row
                    .spec
                    .pointer("/imported/tools")
                    .and_then(Json::as_i64),
            });
        }
        Ok(EnvironmentMcp {
            servers,
            applies: self.applies,
            found: self.found.clone(),
            import: McpImportRight {
                allowed: self.import_allowed,
                reason: if self.import_allowed {
                    None
                } else if !self.import_permitted {
                    Some(McpImportReason::Policy)
                } else {
                    Some(McpImportReason::NotOwner)
                },
            },
        })
    }
}

async fn check_import(conn: &mut Conn, principal: &Principal, environment: Id) -> ApiResult<()> {
    require(
        conn,
        principal,
        ResourceKind::Environment,
        environment,
        Permission::EnvironmentManage,
    )
    .await?;
    let (_, _, policy) = machine_policy(conn, environment).await?;
    if !policy.mcp_import {
        return Err(ApiError::new(
            ErrorCode::PolicyMcpImportDisabled,
            "the machine permits no MCP imports",
        ));
    }
    if !may_import(conn, principal, environment).await? {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "only this machine's owner or its delegated environment managers import definitions",
        ));
    }
    Ok(())
}

/// The exact harness configuration entry copied by one import, independently of its Orch identifiers.
struct ImportOrigin<'a> {
    source: &'a str,
    scope: Json,
    directory: Option<String>,
    name: &'a str,
}

async fn imported(
    ctx: &Ctx,
    conn: &mut Conn,
    environment: Id,
    server: Id,
    origin: ImportOrigin<'_>,
) -> ApiResult<Json> {
    let ImportOrigin {
        source,
        scope,
        directory,
        name,
    } = origin;
    let selected = orch_engine::environment_mcp::select_import(
        conn,
        environment,
        source,
        scope,
        directory,
        name,
    )
    .await
    .map_err(policy_error)?;
    let reply: McpImportResult = ctx
        .app
        .link
        .dispatcher()
        .run(
            selected.machine(),
            &SerialKey::Environment(environment),
            &selected.command(server),
        )
        .await
        .map_err(|error| {
            if let crate::dispatch::RunError::Command(orch_link::commands::CommandFailure::Failed(
                refused,
            )) = &error
                && refused.code == "policy.mcp_import_disabled"
            {
                return ApiError::new(
                    ErrorCode::PolicyMcpImportDisabled,
                    "the machine now permits no MCP imports",
                );
            }
            ApiError::new(
                ErrorCode::SourceOffline,
                "the machine could not copy this MCP definition",
            )
        })?;
    import_spec(ctx, &selected, reply)
}

/// The native safe import record, shared by ordinary and operation-owned copies.
fn import_spec(
    ctx: &Ctx,
    selected: &orch_engine::environment_mcp::ImportSelection,
    reply: McpImportResult,
) -> ApiResult<Json> {
    Ok(selected.imported_spec(reply, ctx.user()?.id(), Time::new(ctx.now)))
}

async fn import(
    ctx: Ctx,
    (): (),
    _body: api::EnvironmentMcpImportCreateBody,
) -> ApiResult<api::EnvironmentMcpCreateResponse> {
    let (tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done =
        crate::key_operations::native::effects::apply_owned::<import_sources::Imports>(&ctx, tx)
            .await?;
    created_reply(&done.ctx, &done.output.accepted, done.output.projection).await
}

/// Command-line import after the live machine's actual owner was pinned.
/// The transaction's authority lock covers both filing the copy and assigning its page.
pub(crate) async fn import_in(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    body: api::EnvironmentMcpImportCreateBody,
    id: Id,
    command_line: bool,
) -> ApiResult<McpServer> {
    let name = resources::name(body.as_name.as_deref().unwrap_or(&body.name))?;
    check_import(conn, principal, environment).await?;
    let env = environments::lock_environment(conn, environment)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let machine = env.machine_id.ok_or_else(|| {
        ApiError::new(
            ErrorCode::SourceOffline,
            "the environment has no connected machine",
        )
    })?;
    let owner = orch_db::queries::resources::machine(conn, machine)
        .await?
        .and_then(|machine| machine.owner_id)
        .unwrap_or(principal.user());
    if command_line && let Some(existing) = rows::lock(conn, id).await? {
        let origin = &existing.spec["imported"];
        if existing.owner_id != Some(principal.user())
            || existing.machine_id != Some(machine)
            || origin["command_request"] != id.to_string()
            || origin["source"] != body.source
            || origin["name"] != body.name
            || origin["scope"] != serde_json::to_value(body.scope).map_err(ApiError::internal)?
            || origin["directory"].as_str() != body.directory.as_deref()
        {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "that import key already belongs to another request",
            ));
        }
        return resources::show(ctx, conn, principal, id).await;
    }
    let mut spec = imported(
        ctx,
        conn,
        environment,
        id,
        ImportOrigin {
            source: &body.source,
            scope: serde_json::to_value(body.scope).map_err(ApiError::internal)?,
            directory: body.directory,
            name: &body.name,
        },
    )
    .await?;
    if command_line {
        spec["imported"]["command_request"] = json!(id);
    }
    let row = McpRow {
        imported: true,
        id,
        name,
        description: String::new(),
        org_id: Some(env.org_id),
        home_id: Some(env.org_id),
        owner_id: Some(owner),
        machine_id: Some(machine),
        environment_id: None,
        spec,
        enabled: true,
        revision: 1,
    };
    if let Err(error) = file_import(
        ctx,
        conn,
        principal,
        Filing {
            environment,
            row: &row,
            enable: body.enable.unwrap_or(false),
            command_line,
        },
    )
    .await
    {
        if matches!(&error, FilingError::Copy(_)) {
            let _ = ctx
                .app
                .link
                .dispatcher()
                .run(machine, &SerialKey::Machine, &McpForget { server_id: id })
                .await;
        }
        return Err(error.problem());
    }
    resources::show(ctx, conn, principal, id).await
}

/// The native filing operands after the connector produced this one copy.
struct Filing<'a> {
    environment: Id,
    row: &'a McpRow,
    enable: bool,
    command_line: bool,
}

/// Preserve the ordinary import's two existing immediate-copy cleanup cases.
enum FilingError {
    Copy(ApiError),
    Other(ApiError),
}
impl FilingError {
    fn problem(self) -> ApiError {
        match self {
            Self::Copy(error) | Self::Other(error) => error,
        }
    }
}
impl From<ApiError> for FilingError {
    fn from(error: ApiError) -> Self {
        Self::Other(error)
    }
}
impl From<DbError> for FilingError {
    fn from(error: DbError) -> Self {
        Self::Other(error.into())
    }
}

/// One native row/offer/page/enable/audit writer for web and command-line copies.
async fn file_import(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    filing: Filing<'_>,
) -> Result<(), FilingError> {
    let Filing {
        environment,
        row,
        enable,
        command_line,
    } = filing;
    let id = row.id;
    let machine = row
        .machine_id
        .ok_or_else(|| ApiError::internal("the imported server has no native machine"))?;
    let owner = row
        .owner_id
        .ok_or_else(|| ApiError::internal("the imported server has no native owner"))?;
    rows::insert(conn, row, ctx.now)
        .await
        .map_err(|error| FilingError::Copy(error.into()))?;
    rows::offer(
        conn,
        id,
        machine,
        Some(environment),
        &row.spec["imported"],
        ctx.now,
    )
    .await?;
    ownership::start_resource(
        conn,
        ObjectRef {
            kind: ResourceKind::McpServer.into(),
            id,
        },
        owner,
        NewAccess::Defaults,
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::internal)?;
    if enable {
        require(
            conn,
            principal,
            ResourceKind::McpServer,
            id,
            Permission::McpUse,
        )
        .await
        .map_err(FilingError::Copy)?;
        rows::enable(conn, environment, id, true, ctx.now).await?;
    }
    record(ctx, conn, AuditAction::McpImported, id, Home { node: row.home_id, org: row.org_id },
        if command_line {"Imported an MCP definition from the command line"} else {"Imported an MCP definition"},
        json!({"server":id,"machine":machine,"environment":environment,"via":if command_line {"command_line"} else {"web"}}),
    ).await?;
    Ok(())
}

/// A friendly one-server job; the environment's lifecycle and all turns remain as they are.
async fn restart(ctx: Ctx, (): (), (): ()) -> ApiResult<orch_api_contract::workspace::Job> {
    let environment = ctx.id("id")?;
    let name = ctx.param("name")?.to_owned();
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    require(
        &mut tx,
        &principal,
        ResourceKind::Environment,
        environment,
        Permission::EnvironmentManage,
    )
    .await?;
    let (env, machine, _) = machine_policy(&mut tx, environment).await?;
    if env.status != "running" || machine.is_none_or(|machine| !machine.online) {
        return Err(ApiError::new(
            ErrorCode::SourceOffline,
            "the environment must be online to restart its tool server",
        ));
    }
    let enabled = orch_engine::mcp::resolve(&mut tx, environment, Time::new(ctx.now))
        .await
        .map_err(ApiError::internal)?;
    let server = enabled
        .iter()
        .find(|server| server.row.name == name)
        .ok_or_else(ApiError::not_found)?;
    if let Some(reason) = &server.reason {
        return Err(ApiError::new(ErrorCode::CapabilityUnsupported, reason));
    }
    let row = ctx
        .app
        .engine
        .queue_mcp_restart(&mut tx, environment, &name, principal.user())
        .await
        .map_err(ApiError::internal)?;
    let job = super::jobs::view(&mut tx, &row).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(job)
}

async fn create(
    ctx: Ctx,
    (): (),
    _body: api::EnvironmentMcpServerCreateBody,
) -> ApiResult<api::EnvironmentMcpCreateResponse> {
    let (mut tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done =
        crate::key_operations::native::effects::apply::<create_sources::OwnServers>(&ctx, &mut tx)
            .await?;
    tx.commit().await.map_err(DbError::from)?;
    created_reply(&done.ctx, &done.accepted, done.output).await
}

/// Both native creators publish their actual saved identity through the same optional reader.
async fn created_reply(
    ctx: &Ctx,
    accepted: &orch_authz::Caller,
    created: create_sources::Created,
) -> ApiResult<api::EnvironmentMcpCreateResponse> {
    let receipt = api::EnvironmentMcpCreateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        server_id: created.server,
        environment_id: created.environment,
    };
    crate::key_operations::publication::receipt(ctx, &receipt)?;
    match crate::key_operations::native::resource_reply::with_input::<create_sources::SavedServers>(
        ctx, accepted, created,
    )
    .await?
    {
        Some(orch_api_contract::resources::Resource::McpServer(server)) => {
            Ok(api::EnvironmentMcpCreateResponse::Full(Box::new(server)))
        }
        Some(_) => Err(ApiError::internal(
            "the created native MCP projection changed resource kind",
        )),
        None => Ok(api::EnvironmentMcpCreateResponse::Receipt(receipt)),
    }
}
