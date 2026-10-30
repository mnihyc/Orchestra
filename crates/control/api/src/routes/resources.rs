//! Providers, listed by what the caller owns, may use and manages, with their setup, sign-in by each declared method, allowed arrangement types and runtime (start, stop, update plans), models, profiles, hosts with their stored metrics, machines with their policy, templates of both kinds with their import, drivers, MCP presets and imported servers, and mounts; the owners of a resource.
//!
//! Every decision is the access engine's: who may see a resource, use it, manage it, make one of a
//! kind in a place, and what a refusal says is missing. What is written here is the translation to
//! and from the wire, the rows each request reads and writes, and the audit row of every change,
//! made in the transaction of the change. A key, a secret header and every other secret that a
//! request brings is sealed before it is stored and is returned by no request.
//!
//! The requests are bound by kind of resource, each in a module of its own below.

pub mod agents;
pub mod host_delete;
pub mod host_maintenance;
pub mod host_storage;
pub mod hosts;
pub mod impact;
pub mod library;
pub mod machines;
pub mod mcp;
pub mod mcp_readers;
pub mod models;
pub mod mounts;
pub mod page;
pub mod provider_login;
pub mod provider_plan;
mod provider_recovery;
pub mod provider_resets;
pub mod provider_runtime;
pub(crate) mod provider_sources;
pub mod provider_updates;
pub mod providers;
pub mod sealing;
pub mod setup;
pub mod skills;
pub mod templates;
pub mod upstreams;
pub(crate) mod validation;
pub mod view;

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::invalid;
use crate::sign_in::origin;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_authz::{Policy, Principal};
use orch_db::Conn;
use orch_db::audit::{self, Actor, Entry};
use orch_types::{Id, Json};

/// The most characters of the name of a resource: what the schema allows.
pub const NAME_MAX_CHARS: usize = 80;

/// How many resources a list shows. The lists of the contract have no cursor, so a list holds the
/// first this many that the caller may see, in the order of their identifiers; a place with more
/// resources of one kind than this narrows its lists with `org_id`.
pub const LIST_LIMIT: usize = 500;

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

/// Where a resource lies, as the audit row of an act on it needs it: the org or node it is at home
/// in and the root org of that.
#[derive(Debug, Clone, Copy, Default)]
pub struct Home {
    /// The place its page follows; none for a resource of the system.
    pub node: Option<Id>,
    /// The root org of that place.
    pub org: Option<Id>,
}

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

/// Turns a failure of the engine into the answer a client gets.
pub async fn engine_failed(conn: &mut Conn, error: orch_authz::Error) -> ApiError {
    refusals::problem(conn, error).await
}
