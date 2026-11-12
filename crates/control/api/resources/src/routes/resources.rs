//! Providers, listed by what the caller owns, may use and manages, with their setup, sign-in by each declared method, allowed arrangement types and runtime (start, stop, update plans), models, profiles, hosts with their stored metrics, machines with their policy, templates of both kinds with their import, drivers, MCP presets and imported servers, and mounts; the owners of a resource.
//!
//! Every decision is the access engine's: who may see a resource, use it, manage it, make one of a
//! kind in a place, and what a refusal says is missing. What is written here is the translation to
//! and from the wire, the rows each request reads and writes, and the audit row of every change,
//! made in the transaction of the change. A key, a secret header and every other secret that a
//! request brings is sealed before it is stored and is returned by no request.
//!
//! The requests are bound by kind of resource, each in a module of its own below.

use crate::middleware::authorization;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::nodes::invalid;

use crate::sign_in::origin;

use orch_access_contract::audit::AuditAction;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::audit;

use orch_db::audit::Actor;

use orch_db::audit::Entry;

use orch_types::Id;

use orch_types::Json;

pub use orch_api_base::routes::resources::LIST_LIMIT;

pub use orch_api_base::routes::resources::Home;
/// The most characters of the name of a resource: what the schema allows.
pub const NAME_MAX_CHARS: usize = 80;

/// The View permission which makes a resource's content readable.
pub(super) const fn view_permission(kind: ResourceKind) -> Permission {
    match kind {
        ResourceKind::Provider => Permission::ProviderView,
        ResourceKind::Agent => Permission::AgentView,
        ResourceKind::Environment => Permission::EnvironmentView,
        ResourceKind::Host => Permission::HostView,
        ResourceKind::Machine => Permission::MachineView,
        ResourceKind::Template => Permission::TemplateView,
        ResourceKind::McpServer => Permission::McpView,
        ResourceKind::Mount => Permission::MountView,
        ResourceKind::Skill => Permission::SkillView,
    }
}

/// Separates private permission-page destinations from readable resources before content views
/// load runtime, configuration or activity. All owner names are fetched together.
pub(crate) async fn hidden_items(
    conn: &mut Conn,
    principal: &Principal,
    seen: &mut Vec<orch_authz::resources::Seen>,
) -> ApiResult<Vec<orch_api_contract::common::HiddenItem>> {
    let hidden: Vec<_> = seen
        .iter()
        .filter(|each| {
            !each
                .held
                .iter()
                .any(|held| held.permission == view_permission(each.resource.kind))
        })
        .collect();
    let owners: Vec<_> = hidden
        .iter()
        .filter_map(|each| each.resource.first_owner)
        .collect();
    let owners = refusals::user_refs(conn, &owners).await?;
    let mut items: Vec<_> = hidden
        .into_iter()
        .map(|each| orch_api_contract::common::HiddenItem {
            id: each.resource.id,
            parent_id: each.resource.home,
            kind: each.resource.kind.as_str().to_owned(),
            name: each.resource.name.clone(),
            owner: each
                .resource
                .first_owner
                .and_then(|id| owners.get(&id).cloned()),
            content_hidden: true,
            private: true,
        })
        .collect();
    items.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    seen.retain(|each| {
        each.held
            .iter()
            .any(|held| held.permission == view_permission(each.resource.kind))
    });
    // The principal's scopes were applied by visible(); no admin flag widens them here.
    let _ = principal;
    Ok(items)
}

/// Keeps the ordinary readable rows followed by the private permission-page rows, which have no
/// work status or activity with which to order or filter them.
pub(crate) fn listed<T>(
    content: Vec<T>,
    private: Vec<orch_api_contract::common::HiddenItem>,
) -> Vec<orch_api_contract::common::Listed<T>> {
    content
        .into_iter()
        .map(orch_api_contract::common::Listed::visible)
        .chain(
            private
                .into_iter()
                .map(orch_api_contract::common::Listed::hidden),
        )
        .collect()
}

/// Opens a resource action with current authority, identity and time for its rules and audit.
pub(super) async fn begin_mutation(ctx: &Ctx) -> ApiResult<(Ctx, orch_db::Tx, Principal)> {
    let (tx, guard) = crate::middleware::transaction_guard::begin(ctx).await?;
    let mut current = ctx.clone();
    current.caller =
        crate::middleware::authentication::Caller::User(std::sync::Arc::new(guard.user().clone()));
    current.now = guard.now().get();
    Ok((current, tx, guard.into_principal()))
}
orch_api_base::__api_bind_b76d4af877!();

/// Binds the requests of every kind of resource that has a handler.
pub fn mount(registry: &mut Registry) {
    library::mount(registry);
    impact::mount(registry);
    machines::mount(registry);
    agents::mount(registry);
    providers::mount(registry);
    provider_runtime::mount(registry);
    provider_updates::mount(registry);
    provider_login::mount(registry);
    provider_resets::mount(registry);
    provider_sources::mount(registry);
    models::mount(registry);
    mcp_readers::mount(registry);
    setup::mount(registry);
    templates::mount(registry);
    mounts::mount(registry);
    mcp::mount(registry);
    hosts::mount(registry);
    host_maintenance::mount(registry);
    host_storage::mount(registry);
    host_delete::mount(registry);
    skills::mount(registry);
}
orch_api_base::__api_bind_8cb26d9e4c!();

/// A name for a resource as it is kept: without the space around it, and neither empty nor
/// longer than the schema allows.
pub fn clean_name(path: &str, name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid(path, "it needs a name"));
    }
    if name.chars().count() > NAME_MAX_CHARS {
        return Err(invalid(
            path,
            &format!("a name has at most {NAME_MAX_CHARS} characters"),
        ));
    }
    Ok(name.to_owned())
}
orch_api_base::__api_bind_08053fcda3!();

/// Requires `permission` of the caller on the resource `id`: the access engine decides, and a
/// refusal says what is missing and who can give it.
pub async fn require(
    conn: &mut Conn,
    principal: &Principal,
    kind: ResourceKind,
    id: Id,
    permission: Permission,
) -> ApiResult<()> {
    authorization::require_resource(conn, principal, kind, id, permission, Policy::default()).await
}
orch_api_base::__api_bind_6286b1a27a!();

/// Writes the audit row of an act of the caller on the resource `object`, in the caller's
/// transaction. The row stands beside the resource's home, and is a line of its history where the
/// action is one.
pub async fn record(
    ctx: &Ctx,
    conn: &mut Conn,
    action: AuditAction,
    object: Id,
    home: Home,
    summary: &str,
    details: Json,
) -> ApiResult<()> {
    let mut entry = Entry::new(
        Actor::User(ctx.user()?.id()),
        action.as_str(),
        action.object_kind().unwrap_or("resource"),
        Some(object),
        summary,
    );
    entry.history = action.is_history_line();
    entry.node_id = home.node;
    entry.org_id = home.org;
    entry.details = details;
    if matches!(
        action,
        AuditAction::ProviderCreated | AuditAction::ProviderUpdated
    ) {
        let upstreams: Option<Json> =
            sqlx::query_scalar("SELECT upstreams FROM providers WHERE id=$1")
                .bind(object)
                .fetch_optional(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
        let hosts: std::collections::BTreeSet<String> = upstreams
            .as_ref()
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(|endpoint| endpoint.get("base_url").and_then(Json::as_str))
            .filter_map(|address| url::Url::parse(address).ok())
            .filter_map(|address| address.host().map(|host| host.to_string()))
            .collect();
        entry.details["endpoint_hosts"] =
            serde_json::to_value(hosts).map_err(ApiError::internal)?;
    }
    audit::append(conn, &entry, &origin(ctx), ctx.now).await?;
    Ok(())
}
orch_api_base::__api_bind_824140da0a!();

/// Turns a failure of the engine into the answer a client gets.
pub async fn engine_failed(conn: &mut Conn, error: orch_authz::Error) -> ApiError {
    refusals::problem(conn, error).await
}

#[path = "resources/agents.rs"]
pub mod agents;

#[path = "resources/capability_backends.rs"]
pub mod capability_backends;

#[path = "resources/collection.rs"]
pub mod collection;

#[path = "resources/context.rs"]
pub mod context;

#[path = "resources/creation_home.rs"]
pub mod creation_home;

#[path = "resources/environment_fleet.rs"]
pub mod environment_fleet;

#[path = "resources/environment_places.rs"]
pub mod environment_places;

#[path = "resources/environment_read.rs"]
pub mod environment_read;

#[path = "resources/figures.rs"]
pub mod figures;

#[path = "resources/grouping.rs"]
pub mod grouping;

#[path = "resources/host_delete.rs"]
pub mod host_delete;

#[path = "resources/host_maintenance.rs"]
pub mod host_maintenance;

#[path = "resources/host_storage.rs"]
pub mod host_storage;

#[path = "resources/hosts.rs"]
pub mod hosts;

#[path = "resources/impact.rs"]
pub mod impact;

#[path = "resources/library.rs"]
pub mod library;

#[path = "resources/machines.rs"]
pub mod machines;

#[path = "resources/mcp.rs"]
pub mod mcp;

#[path = "resources/mcp_readers.rs"]
pub mod mcp_readers;

#[path = "resources/models.rs"]
pub mod models;

#[path = "resources/mounts.rs"]
pub mod mounts;

#[path = "resources/page.rs"]
pub mod page;

#[path = "resources/projection.rs"]
pub mod projection;

#[path = "resources/provider_aux_readers.rs"]
pub mod provider_aux_readers;

#[path = "resources/provider_creation.rs"]
pub mod provider_creation;

#[path = "resources/provider_handover.rs"]
pub mod provider_handover;

#[path = "resources/provider_list.rs"]
pub mod provider_list;

#[path = "resources/provider_login.rs"]
pub mod provider_login;

#[path = "resources/provider_plan.rs"]
pub mod provider_plan;

#[path = "resources/provider_projection.rs"]
pub mod provider_projection;

#[path = "resources/provider_recovery.rs"]
pub mod provider_recovery;

#[path = "resources/provider_resets.rs"]
pub mod provider_resets;

#[path = "resources/provider_runtime.rs"]
pub mod provider_runtime;

#[path = "resources/provider_services.rs"]
pub mod provider_services;

#[path = "resources/provider_setup_operation.rs"]
pub mod provider_setup_operation;

#[path = "resources/provider_sources.rs"]
pub mod provider_sources;

#[path = "resources/provider_updates.rs"]
pub mod provider_updates;

#[path = "resources/providers.rs"]
pub mod providers;

#[path = "resources/read.rs"]
pub mod read;

#[path = "resources/sealing.rs"]
pub mod sealing;

#[path = "resources/setup.rs"]
pub mod setup;

#[path = "resources/skills.rs"]
pub mod skills;

#[path = "resources/templates.rs"]
pub mod templates;

#[path = "resources/upstreams.rs"]
pub mod upstreams;

#[path = "resources/validation.rs"]
pub mod validation;

#[path = "resources/view.rs"]
pub mod view;
