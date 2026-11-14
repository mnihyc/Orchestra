//! A person's machines: the token a machine enrolls with, listing and reading with whether each is online and what was registered from it, renaming, revoking, asking a machine for its facts again, and its local policy as it reported it with the keys its owner handed over.
//!
//! A machine's page names its owners and other holders; a stranger is told it is not there.
//! An enabled administrator can review its page and explicitly name themself before managing it.
//! Its local policy has the last word. Keys are handed to Orch one by one; the access engine
//! checks who manages what each key limits before changing it. The machine decides what stays local.

use crate::backplane::RATE_WEIGHT;

use crate::certificates::TokenFor;

use crate::certificates::TokenRequest;

use crate::dispatch::RunError;

use crate::middleware::authentication::Authed;

use crate::middleware::authorization;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::access::page::NameBounds;

use crate::routes::nodes::actor_name;

use crate::routes::nodes::invalid;

use crate::routes::nodes::placed;

use crate::routes::nodes::policy_at;

use crate::routes::resources::providers::home_policies;

use crate::routes::resources::providers::refused_making;

use crate::routes::resources::view::accesses_prepared;

use crate::routes::resources::view::agent_refs_from_rows_with_names;

use crate::routes::resources::view::environment_refs_from_rows_with_names;

use crate::routes::resources::view::is_online;

use crate::routes::resources::view::machine_refs;

use crate::routes::resources::view::parse;

use crate::routes::resources::view::prepare_accesses;

use crate::routes::resources::view::provider_refs_from_rows_with_names;

use crate::sign_in::origin;

use orch_access_contract::audit::AuditAction;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;
#[cfg(any(test, feature = "test-support"))]
use orch_api_contract::common::FieldFault;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::Problem;

use orch_api_contract::resources as api;

use orch_api_contract::resources::EnrollmentCommands;

use orch_api_contract::resources::EnrollmentToken;

use orch_api_contract::resources::Machine;

use orch_api_contract::resources::MachinePolicy;

use orch_api_contract::resources::MachinePolicyKey;

use orch_api_contract::resources::MachineRef;

use orch_api_contract::resources::PolicyBounds;

use orch_authz::ownership;

use orch_authz::ownership::Basis;

use orch_authz::ownership::Making;

use orch_authz::resources::KeyAbout;

use orch_authz::resources::Keying;

use orch_authz::resources::may_change_policy_key;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_api_base::certificates::example_address;

use orch_config::ServerConfig;

use orch_db::audit;

use orch_db::audit::Actor;

use orch_db::audit::Entry;

use orch_db::queries::resources as rows;

use orch_db::queries::resources::MachineRow;

use orch_db::queries::resources::routes as queries;

use orch_db::Conn;

use orch_db::DbError;

use orch_link::commands::CommandFailure;

use orch_link_contract::commands::MachinePolicySet;

use orch_link_contract::commands::SerialKey;
#[cfg(any(test, feature = "test-support"))]
use orch_link_contract::policy::MachineDrivers;

use orch_link_contract::policy::Bounds;

use orch_link_contract::policy::LocalPolicy;

use orch_link_contract::policy::PolicyKey;

use orch_link_contract::session::MachineFacts;

use orch_link_contract::enrollment_choices::EnrollmentChoices;

use orch_link_contract::session::Role;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use serde_json::json;

use std::collections::BTreeMap;

pub use orch_api_base::routes::resources::machines::PreparedInventory;

pub use orch_api_base::routes::resources::machines::Registered;
/// The longest name a machine has: what the schema allows.
const MACHINE_NAME_MAX_CHARS: usize = 80;

/// The member of a machine's facts that holds what the link keeps about its certificate; it is
/// never shown.
const LINK_MEMBER: &str = rows::LINK_MEMBER;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    gateway::register(registry);
    reenrollment::mount(registry);
    enrollment::mount(registry);
    registry
        .on_snapshot_read_with_optional_operation(
            api::MachineList,
            crate::key_operations::native::selected::<read::Machines>,
            read::list,
        )
        .on(api::MachineEnrollmentTokenCreate, token_create)
        .on_snapshot_read_with_optional_operation(
            api::MachineGet,
            crate::key_operations::native::selected::<individual::MachineRead>,
            individual::get,
        )
        .on(api::MachineEnrollmentInfoGet, enrollment_info)
        .on_with_optional_operation(
            api::MachineUpdate,
            crate::key_operations::native::stored_deferred::<mutations::Machines, _, _>,
            mutations::update,
        )
        .on_with_optional_operation(
            api::MachineUpdateNow,
            crate::key_operations::native::stored_deferred::<updates::Requests, _, _>,
            updates::update,
        )
        .on_with_optional_operation(
            api::MachineRevoke,
            crate::key_operations::native::stored_deferred::<revocation::Machines, _, _>,
            revocation::revoke,
        )
        .on_with_optional_operation(
            api::MachineProbe,
            crate::key_operations::native::stored_deferred::<probes::Probes, _, _>,
            probes::probe,
        )
        .on_with_operation(api::MachinePolicyGet, read_operation::prepare, policy_get)
        .on_with_optional_operation(
            api::MachinePolicyUpdate,
            crate::key_operations::native::stored_deferred::<policies::Changes, _, _>,
            policies::update,
        );
    programs::register(registry);
    roles::register(registry);
}

/// Whether the machine is the caller's: they are its owner, and the way they ask reaches a thing
/// that goes through no node (a token confined to a subtree does not).
#[must_use]
pub fn is_owner(authed: &Authed, machine: &MachineRow) -> bool {
    machine.owner_id == Some(authed.id())
        && authorization::caller_of(authed).is_ok_and(|caller| caller.reaches(None))
}

/// The machine `id` through its current View permission, or its host's facade.
/// An unreadable or deleted machine is not there for the caller.
pub(crate) async fn own_machine(ctx: &Ctx, conn: &mut Conn, id: Id) -> ApiResult<MachineRow> {
    let row = live_machine(conn, id).await?;
    let principal = authorization::principal(ctx, conn).await?;
    require_machine(conn, &principal, id, Permission::MachineView).await?;
    Ok(row)
}
orch_api_base::__api_bind_c5ec983f9e!();

async fn live_machine(conn: &mut Conn, id: Id) -> ApiResult<MachineRow> {
    rows::machine(conn, id)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)
}

/// The same stored row and ordinary View gate, using this read's current admitted principal.
async fn own_machine_on(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<MachineRow> {
    let row = live_machine(conn, id).await?;
    require_machine(conn, principal, id, Permission::MachineView).await?;
    Ok(row)
}

/// A physical machine's page decides every action; the node role uses its existing host facade.
pub(crate) async fn require_machine(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    permission: Permission,
) -> ApiResult<()> {
    let decision = match orch_authz::resources::check_machine(
        conn,
        principal,
        id,
        permission,
        Policy::default(),
    )
    .await
    {
        Ok(decision) => decision,
        Err(error) => return Err(authorization::engine_problem(conn, principal, error).await),
    };
    if decision.is_allowed() {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}
orch_api_base::__api_bind_55c8b9c49d!();

/// The same native registration door with this source's current admitted principal.
pub(crate) async fn registrable_machine_on(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<MachineRow> {
    let row = own_machine_on(conn, principal, id).await?;
    require_machine(conn, principal, id, Permission::MachineUse).await?;
    Ok(row)
}
orch_api_base::__api_bind_a909d67302!();

fn installation_info(config: &ServerConfig) -> api::MachineEnrollmentInfo {
    let endpoints = config.gateway.advertised_endpoints();
    let public_url = config.system.public_url.trim_end_matches('/').to_owned();
    // Installers accept HTTPS and loopback HTTP for local development.
    let secure = url::Url::parse(&public_url).ok().is_some_and(|url| {
        url.scheme() == "https"
            || (url.scheme() == "http"
                && match url.host() {
                    Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
                    Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                    Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                    None => false,
                })
    });
    let secure = secure && installer_syntax(&public_url);
    let configure = if !secure || example_address(&public_url, false) {
        Some("public_url".to_owned())
    } else if endpoints.is_empty()
        || endpoints.iter().any(|endpoint| {
            example_address(&endpoint.url, true)
                || !installer_syntax(&endpoint.url)
                || !url::Url::parse(&endpoint.url)
                    .is_ok_and(|url| matches!(url.scheme(), "wss" | "https"))
        })
    {
        Some("[[gateway.endpoints]]".to_owned())
    } else {
        None
    };
    api::MachineEnrollmentInfo {
        public_url,
        secure: secure && configure.is_none(),
        configure,
    }
}

async fn enrollment_info(ctx: Ctx, (): (), (): ()) -> ApiResult<api::MachineEnrollmentInfo> {
    Ok(installation_info(&installation_config(&ctx)))
}

/// Commands and readiness use the same effective catalogue as enrollment, including an admin override.
pub(crate) fn installation_config(ctx: &Ctx) -> ServerConfig {
    let mut config = (*ctx.app.config).clone();
    config.gateway.instances.clear();
    config.gateway.endpoints = ctx
        .app
        .link
        .certificates()
        .advertised_endpoints()
        .into_iter()
        .map(|endpoint| orch_config::server::GatewayEndpoint {
            url: endpoint.url,
            trust: match endpoint.trust {
                orch_link_contract::enrollment::Trust::Pinned => "pinned",
                orch_link_contract::enrollment::Trust::Public => "public",
            }
            .into(),
        })
        .collect();
    config
}

/// Never mint a secret for a command the shipped installer will refuse.
pub(crate) fn require_installable(config: &ServerConfig) -> ApiResult<()> {
    if installation_info(config).secure {
        Ok(())
    } else {
        Err(ApiError::invalid(
            installation_info(config)
                .configure
                .as_deref()
                .unwrap_or("public_url"),
            "Configure this setting with a reachable address before enrolling a machine.",
        ))
    }
}

/// The installers take explicit hosts, not browser-normalized IPv4 shorthands or encoded authorities.
/// Keep the integrator's transport eligibility above; check the command's original spelling before minting.
fn installer_syntax(address: &str) -> bool {
    if address
        .chars()
        .any(|letter| letter.is_whitespace() || letter.is_control() || letter == '\\')
        || address.contains('#')
    {
        return false;
    }
    let Ok(uri) = address.parse::<http::Uri>() else {
        return false;
    };
    let Some(authority) = uri.authority() else {
        return false;
    };
    if uri.query().is_some()
        || authority.as_str().contains('@')
        || authority.as_str().ends_with(':')
    {
        return false;
    }
    let host = authority.host();
    if host.contains('%') {
        return false;
    }
    if uri
        .scheme_str()
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http"))
    {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    } else {
        true
    }
}

/// Local HTTP download/enrollment does not turn a node's gateway into a plaintext endpoint.
pub(crate) fn require_node_installable(config: &ServerConfig) -> ApiResult<()> {
    require_installable(config)?;
    let endpoint = node_endpoint(config);
    if installer_syntax(&endpoint)
        && url::Url::parse(&endpoint)
            .ok()
            .is_some_and(|url| matches!(url.scheme(), "https" | "wss"))
    {
        Ok(())
    } else {
        Err(ApiError::invalid(
            "gateway.endpoints",
            "Ask the system operator for a WSS or HTTPS gateway address without credentials, query or fragment before adding a node.",
        ))
    }
}

/// What a machine reports about itself, without what the link keeps.
fn shown_facts(facts: &Json) -> Json {
    let mut facts = facts.clone();
    let policy = facts
        .get("policy")
        .cloned()
        .and_then(|value| serde_json::from_value::<LocalPolicy>(value).ok());
    if let Some(members) = facts.as_object_mut() {
        members.remove(LINK_MEMBER);
        members.remove("installation");
        // Directory observations have their own management projection, never the generic facts blob.
        members.remove("data_directory");
        // Policy acknowledgements update the canonical policy before a later native facts event.
        // Capability support is a fact; its delegated/enabled state always comes from that policy.
        if let Some(capability) = members
            .get_mut("role_management")
            .and_then(Json::as_object_mut)
        {
            capability.insert(
                "delegated".into(),
                json!(
                    policy
                        .as_ref()
                        .is_some_and(|policy| policy.delegates(PolicyKey::MachineRoles))
                ),
            );
            capability.insert(
                "enabled".into(),
                json!(policy.as_ref().is_some_and(|policy| policy.machine_roles)),
            );
        }
        if let Some(capabilities) = members
            .get_mut("programs")
            .and_then(|programs| programs.get_mut("capabilities"))
            .and_then(Json::as_array_mut)
        {
            for capability in capabilities {
                let key = match capability.get("kind").and_then(Json::as_str) {
                    Some("agent") => PolicyKey::AgentPrograms,
                    Some("provider_source") => PolicyKey::ProviderPrograms,
                    _ => continue,
                };
                let supported = capability.get("supported").and_then(Json::as_bool) == Some(true);
                let delegated = policy.as_ref().is_some_and(|policy| policy.delegates(key));
                let enabled = policy
                    .as_ref()
                    .is_some_and(|policy| policy.get(key).as_bool() == Some(true));
                if let Some(capability) = capability.as_object_mut() {
                    capability.insert("delegated".into(), json!(delegated));
                    capability.insert("enabled".into(), json!(enabled));
                    capability.insert("remote".into(), json!(supported && delegated && enabled));
                    if supported && delegated && enabled {
                        capability.insert("reason".into(), Json::Null);
                    }
                }
            }
        }
    }
    facts
}

/// Machines as the API shows them, with what was registered from each, for any number of them in
/// the same statements.
pub async fn machine_views(
    ctx: &Ctx,
    conn: &mut Conn,
    found: Vec<MachineRow>,
) -> ApiResult<Vec<Machine>> {
    let principal = authorization::principal(ctx, conn).await?;
    machine_views_as(ctx, conn, &principal, found).await
}
orch_api_base::__api_bind_b444ed81ee!();

/// The same ordinary projection with the current consumed preparation principal.
/// This does not reconstruct a caller or establish final operation admission.
pub(super) async fn machine_views_as(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    found: Vec<MachineRow>,
) -> ApiResult<Vec<Machine>> {
    let prepared = prepare_inventory(conn, principal, found).await?;
    show_inventory_prepared(ctx, conn, principal, &prepared).await
}

/// Actual registered reference identities from the native selectors, before API conversion.
pub(crate) async fn registered_sources(
    conn: &mut Conn,
    machines: &[Id],
) -> ApiResult<Vec<ObjectRef>> {
    Ok(Registered::load(conn, machines).await?.sources)
}

pub(crate) async fn prepare_inventory(
    conn: &mut Conn,
    principal: &Principal,
    found: Vec<MachineRow>,
) -> ApiResult<PreparedInventory> {
    prepare_inventory_for(conn, principal, found, false).await
}
orch_api_base::__api_bind_63d7556e7e!();

/// The original Machine/Host View predicate, applied before registration/name projection.
pub(crate) async fn prepare_list_inventory(
    conn: &mut Conn,
    principal: &Principal,
    found: Vec<MachineRow>,
) -> ApiResult<PreparedInventory> {
    prepare_inventory_for(conn, principal, found, true).await
}

async fn prepare_inventory_for(
    conn: &mut Conn,
    principal: &Principal,
    mut found: Vec<MachineRow>,
    readable_only: bool,
) -> ApiResult<PreparedInventory> {
    let ids: Vec<Id> = found.iter().map(|row| row.id).collect();
    let mut hosts = rows::hosting::hosts_naming(conn, &ids).await?;
    let mut resource_ids = ids.clone();
    resource_ids.extend(hosts.iter().map(|(_, host, _)| *host));
    let mut effective =
        orch_authz::resources::effective_among(conn, principal, &resource_ids, Policy::default())
            .await
            .map_err(ApiError::internal)?;
    if readable_only {
        let facades: BTreeMap<_, _> = hosts
            .iter()
            .map(|(machine, host, _)| (*machine, *host))
            .collect();
        let visible: BTreeMap<_, _> = effective
            .iter()
            .map(|held| {
                (
                    held.object.id,
                    held.held.iter().any(|permission| {
                        matches!(
                            permission.permission,
                            Permission::MachineView | Permission::HostView
                        )
                    }),
                )
            })
            .collect();
        found.retain(|row| visible.get(facades.get(&row.id).unwrap_or(&row.id)) == Some(&true));
        let physical: std::collections::BTreeSet<_> = found.iter().map(|row| row.id).collect();
        hosts.retain(|(machine, _, _)| physical.contains(machine));
        let retained: std::collections::BTreeSet<_> = physical
            .iter()
            .copied()
            .chain(hosts.iter().map(|(_, host, _)| *host))
            .collect();
        effective.retain(|held| retained.contains(&held.object.id));
    }
    let holdings: Vec<_> = effective
        .iter()
        .map(super::view::Holding::from_effective)
        .collect();
    let access = prepare_accesses(conn, &holdings).await?;
    let mut sources = access.names.clone();
    let selected: Vec<_> = found.iter().map(|row| row.id).collect();
    let registered = Registered::load(conn, &selected).await?;
    let subjects = selected
        .iter()
        .map(|&id| ("machine".to_owned(), id))
        .collect::<Vec<_>>();
    let grouping = super::grouping::Prepared::select(conn, &subjects).await?;
    sources.nodes.extend(grouping.homes());

    sources.nodes.extend(
        orch_authz::resources::address_name_sources(
            conn,
            &registered
                .sources
                .iter()
                .map(|source| source.id)
                .collect::<Vec<_>>(),
        )
        .await
        .map_err(authorization::restoration_error)?,
    );
    Ok(PreparedInventory {
        grouping,
        found,
        hosts,
        effective,
        access,
        sources,
        bounds: NameBounds::default(),
        registered,
    })
}

pub(crate) async fn show_inventory_prepared(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedInventory,
) -> ApiResult<Vec<Machine>> {
    let found = prepared.found.clone();
    let grouping = prepared
        .grouping
        .render(&ctx.app, conn, principal, Some(&prepared.bounds), None)
        .await?;

    let hosts = &prepared.hosts;
    let ids: Vec<_> = found.iter().map(|row| row.id).collect();
    let software_updates = crate::component_updates::views(&ctx.app.link, conn, &ids).await?;
    let holdings: Vec<_> = prepared
        .effective
        .iter()
        .map(super::view::Holding::from_effective)
        .collect();
    let access = accesses_prepared(
        conn,
        principal,
        &holdings,
        &prepared.access,
        prepared.bounds.limited.then_some(&prepared.bounds),
    )
    .await?;
    let owners: Vec<Id> = found.iter().filter_map(|row| row.owner_id).collect();
    let owners = refusals::user_refs(conn, &owners).await?;
    let names = prepared.bounds.limited.then_some(&prepared.bounds);
    let providers =
        provider_refs_from_rows_with_names(conn, prepared.registered.providers.clone(), names)
            .await?;
    let agents =
        agent_refs_from_rows_with_names(&ctx.app, conn, prepared.registered.agents.clone(), names)
            .await?;
    let environments = environment_refs_from_rows_with_names(
        conn,
        prepared.registered.environments.clone(),
        orch_types::Time::new(ctx.now),
        names,
    )
    .await?;
    found
        .into_iter()
        .map(|row| {
            let of_machine = |machine: &Option<Id>| *machine == Some(row.id);
            let mut facts = shown_facts(&row.facts);
            let role_management = facts
                .as_object_mut()
                .and_then(|members| members.remove("role_management"))
                .and_then(|value| serde_json::from_value(value).ok());
            let manager = access
                .get(
                    &hosts
                        .iter()
                        .find(|(machine, _, _)| *machine == row.id)
                        .map_or(row.id, |(_, host, _)| *host),
                )
                .is_some_and(|access| {
                    access.permissions.iter().any(|permission| {
                        matches!(permission.as_str(), "machine.manage" | "host.manage")
                    })
                });
            let data_directory = (manager || principal.is_system_admin())
                .then(|| {
                    row.facts
                        .get("data_directory")
                        .cloned()
                        .and_then(|value| serde_json::from_value(value).ok())
                })
                .flatten();
            Ok(Machine {
                data_directory,
                grouping: grouping
                    .get(&("machine".to_owned(), row.id))
                    .cloned()
                    .unwrap_or_default(),
                revision: row.revision,
                lost_at: row.lost_at.map(orch_types::Time::new),
                role_management,
                installation: row
                    .facts
                    .get("installation")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok()),
                environment_sandboxes: Vec::new(),
                software_update: software_updates.get(&row.id).cloned(),
                access: access
                    .get(
                        &hosts
                            .iter()
                            .find(|(machine, _, _)| *machine == row.id)
                            .map_or(row.id, |(_, host, _)| *host),
                    )
                    .cloned(),
                host: hosts.iter().find(|(machine, _, _)| *machine == row.id).map(
                    |(_, id, name)| api::HostRef {
                        id: *id,
                        name: name.clone(),
                        address: name.clone(),
                    },
                ),
                base: MachineRef {
                    id: row.id,
                    name: row.name.clone(),
                    online: is_online(&ctx.app, row.id, row.online),
                    platform_os: parse(&row.platform_os)?,
                    placement: parse(&row.placement)?,
                    host_id: row.host_id,
                },
                owner: row.owner_id.and_then(|owner| owners.get(&owner).cloned()),
                platform_arch: row.platform_arch.clone(),
                connector_version: row.connector_version.clone(),
                protocol_version: u32::try_from(row.protocol_version).unwrap_or_default(),
                roles: row.roles.clone(),
                status: parse(&row.status)?,
                last_seen_at: row.last_seen_at.map(Time::new),
                last_ip: row.last_ip.clone(),
                cert_not_after: Time::new(row.cert_not_after),
                facts,
                providers: providers
                    .iter()
                    .filter(|(machine, _)| of_machine(machine))
                    .map(|(_, each)| each.clone())
                    .collect(),
                agents: agents
                    .iter()
                    .filter(|(machine, _)| of_machine(machine))
                    .map(|(_, each)| each.clone())
                    .collect(),
                environments: environments
                    .iter()
                    .filter(|(machine, _)| of_machine(machine))
                    .map(|(_, each)| each.clone())
                    .collect(),
            })
        })
        .collect()
}
orch_api_base::__api_bind_c297297595!();

/// A name for a machine as it is kept: without the space around it, and neither empty nor longer
/// than the schema allows.
fn clean_name(path: &str, name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid(path, "a machine needs a name"));
    }
    if name.chars().count() > MACHINE_NAME_MAX_CHARS {
        return Err(invalid(
            path,
            &format!("a name has at most {MACHINE_NAME_MAX_CHARS} characters"),
        ));
    }
    Ok(name.to_owned())
}

/// Which program a token's commands install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Installer {
    /// `install.sh` and `install.ps1`: the connector of a person's machine, enrolled at the
    /// system's address.
    Connector,
    /// `install-node.sh`: `orch-node` as the service of a host, enrolled at the gateway's pinned
    /// endpoint. It is built for Linux alone.
    Node,
}

/// `text` as one word of a POSIX shell: in single quotes, with a single quote inside closed,
/// escaped and opened again, so that nothing in it is expanded or splits it.
fn sh_word(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `text` as one PowerShell string with nothing expanded: in single quotes, with every character
/// PowerShell takes for a single quote (the typographic ones too) doubled.
fn ps_word(text: &str) -> String {
    let mut word = String::with_capacity(text.len() + 2);
    word.push('\'');
    for character in text.chars() {
        if matches!(
            character,
            '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}'
        ) {
            word.push(character);
        }
        word.push(character);
    }
    word.push('\'');
    word
}

/// The system's first advertised address, in its configured order; an empty list cannot mint a command.
fn node_endpoint(config: &ServerConfig) -> String {
    config
        .gateway
        .advertised_endpoints()
        .first()
        .map(|endpoint| endpoint.url.clone())
        .unwrap_or_default()
}

/// The commands that enroll a machine into this system with `token`, ready to paste: every value
/// is quoted for the shell it is pasted into, and each command ends with the installer's own
/// arguments, so that the flag that hands a machine's limits over to Orch (`--delegate-policy`,
/// `-DelegatePolicy`) is added at its end. A host's commands are the node's: the same POSIX
/// command for Linux and macOS (the browser a host is added from may be on a Mac; the host runs
/// Linux, and the installer refuses any other system with the way to add it as a machine), and
/// for Windows a line that says so.
pub(crate) fn enrollment_commands(
    config: &ServerConfig,
    token: &str,
    installer: Installer,
) -> EnrollmentCommands {
    let base = config.system.public_url.trim_end_matches('/');
    match installer {
        Installer::Connector => {
            let posix = format!(
                "curl -fsSL {} | sh -s -- --url {} --token {}",
                sh_word(&format!("{base}/api/v1/downloads/install.sh")),
                sh_word(base),
                sh_word(token)
            );
            EnrollmentCommands {
                linux: posix.clone(),
                macos: posix,
                windows: format!(
                    "& ([scriptblock]::Create((iwr {} -UseBasicParsing))) -Url {} -Token {}",
                    ps_word(&format!("{base}/api/v1/downloads/install.ps1")),
                    ps_word(base),
                    ps_word(token)
                ),
            }
        }
        Installer::Node => {
            // A node is a service of its host, installed as root; it is fetched from the
            // system's address and enrolls at its first advertised machine address.
            let posix = format!(
                "curl -fsSL {} | sh -s -- --from {} --url {} --token {}",
                sh_word(&format!("{base}/api/v1/downloads/install-node.sh")),
                sh_word(base),
                sh_word(&node_endpoint(config)),
                sh_word(token)
            );
            EnrollmentCommands {
                linux: posix.clone(),
                macos: posix,
                windows: format!(
                    "Write-Output {}",
                    ps_word(
                        "A host runs orch-node, which is built for Linux: run the Linux command on the host."
                    )
                ),
            }
        }
    }
}

/// Adds only the exact local consent flags bound to the token, for the installer of that program.
pub(crate) fn enrollment_commands_choices(
    config: &ServerConfig,
    token: &str,
    installer: Installer,
    choices: &EnrollmentChoices,
) -> EnrollmentCommands {
    let mut commands = enrollment_commands(config, token, installer);
    match installer {
        Installer::Connector => {
            let roles = choices
                .roles
                .iter()
                .map(|role| match role {
                    Role::Environment => "environment",
                    Role::Agent => "agent",
                    Role::Provider => "provider",
                    Role::Node => "node",
                })
                .collect::<Vec<_>>()
                .join(",");
            let posix = format!(" --roles {}", sh_word(&roles));
            let windows = format!(" -Roles {}", ps_word(&roles));
            commands.linux.push_str(&posix);
            commands.macos.push_str(&posix);
            commands.windows.push_str(&windows);
        }
        Installer::Node => {}
    }
    if choices.delegate == ["*"] {
        commands.linux.push_str(" --delegate-policy");
        commands.macos.push_str(" --delegate-policy");
        if matches!(installer, Installer::Connector) {
            commands.windows.push_str(" -DelegatePolicy");
        }
    } else if !choices.delegate.is_empty() {
        let sections = choices
            .delegate
            .iter()
            .map(|section| section.trim_end_matches(".*"))
            .collect::<Vec<_>>()
            .join(",");
        let posix = format!(" --delegate {}", sh_word(&sections));
        commands.linux.push_str(&posix);
        commands.macos.push_str(&posix);
        if matches!(installer, Installer::Connector) {
            commands
                .windows
                .push_str(&format!(" -Delegate {}", ps_word(&sections)));
        }
    }
    if choices.dedicated {
        commands.linux.push_str(" --dedicated");
        commands.macos.push_str(" --dedicated");
        if matches!(installer, Installer::Connector) {
            commands.windows.push_str(" -Dedicated");
        }
    }
    commands
}

/// Makes the token a machine enrolls with: single use, and the machine that uses it is the
/// caller's.
async fn token_create(
    ctx: Ctx,
    (): (),
    body: api::MachineEnrollmentTokenCreateBody,
) -> ApiResult<EnrollmentToken> {
    let installation = installation_config(&ctx);
    require_installable(&installation)?;
    let authed = ctx.user()?;
    if !authorization::caller_of(authed)?.reaches(None) {
        return Err(ApiError::not_found());
    }
    let home = body
        .home_id
        .or(authed.user.personal_org_id)
        .or(authed.user.service_org_id)
        .ok_or_else(|| ApiError::invalid("home_id", "choose the org this machine belongs to"))?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let (_, kind) = placed(&mut conn, home).await?;
    if kind != orch_api_contract::nodes::NodeKind::Org {
        return Err(invalid("home_id", "a machine is at home in an org"));
    }
    let policies = home_policies(&mut conn, home).await?;
    let policy = policy_at(&mut conn, home).await?;
    match ownership::may_make(
        &mut conn,
        &principal,
        ResourceKind::Machine,
        Some(home),
        &policies.members_make,
        policy,
    )
    .await
    {
        Ok(Making::Allowed(Basis::Member | Basis::Rules(_) | Basis::SystemAdmin)) => {}
        Ok(Making::Refused(why)) => return Err(refused_making(&mut conn, home, why).await),
        Err(error) => return Err(refusals::problem(&mut conn, error).await),
    }
    drop(conn);
    let name_hint = body
        .name_hint
        .as_deref()
        .map(|hint| clean_name("name_hint", hint))
        .transpose()?;
    let explicit = body.roles.is_some() || body.delegate.is_some() || body.dedicated.is_some();
    let roles: Vec<Role> = body
        .roles
        .unwrap_or_else(|| {
            vec![
                api::MachineEnrollmentRole::Agent,
                api::MachineEnrollmentRole::Environment,
            ]
        })
        .into_iter()
        .map(|role| match role {
            api::MachineEnrollmentRole::Environment => Role::Environment,
            api::MachineEnrollmentRole::Agent => Role::Agent,
            api::MachineEnrollmentRole::Provider => Role::Provider,
        })
        .collect();
    EnrollmentChoices::new(roles.clone(), Vec::new(), false)
        .map_err(|message| ApiError::invalid("roles", message))?;
    let choices = EnrollmentChoices::new(
        roles,
        body.delegate.unwrap_or_default(),
        body.dedicated.unwrap_or(false),
    )
    .map_err(|message| ApiError::invalid("delegate", message))?;
    let request = TokenRequest {
        created_by: authed.id(),
        purpose: TokenFor::Machine,
        name_hint,
    };
    let certificates = ctx.app.link.certificates();
    let made = certificates
        .issue_machine_token_at(&request, explicit.then_some(&choices), home)
        .await
        .map_err(|fault| ApiError::internal(format!("{}: {}", fault.code, fault.detail)))?;
    Ok(EnrollmentToken {
        receipt: Some(made.receipt),
        commands: if explicit {
            enrollment_commands_choices(&installation, &made.token, Installer::Connector, &choices)
        } else {
            enrollment_commands(&installation, &made.token, Installer::Connector)
        },
        token: made.token,
        expires_at: made.expires_at,
    })
}

/// The same stored projection for a read and an acknowledged machine-policy mutation.
pub(crate) async fn machine_with_readiness(
    ctx: &Ctx,
    conn: &mut Conn,
    row: MachineRow,
) -> ApiResult<Machine> {
    let principal = authorization::principal(ctx, conn).await?;
    individual::Prepared::load(conn, &principal, row)
        .await?
        .render(ctx, conn, &principal)
        .await
}
orch_api_base::__api_bind_d2ace19f11!();

/// The original native update observations and local-policy gate, before its first write.
pub(crate) struct PreparedUpdate {
    row: MachineRow,
    view: api::ComponentUpdate,
    enabled: bool,
}

impl PreparedUpdate {
    pub(crate) fn machine(&self) -> Id {
        self.row.id
    }
}

/// Native result, including whether this action actually filed a fresh request.
pub(crate) struct RequestedUpdate {
    pub(crate) fresh: bool,
}

pub(crate) async fn prepare_update_on(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    machine: Id,
) -> ApiResult<PreparedUpdate> {
    if orch_db::queries::transaction_guard::in_transaction(conn) {
        orch_db::queries::component_updates::lock(conn, machine).await?;
        let _ = sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
            .bind(machine)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
        let _ =
            sqlx::query("SELECT machine_id FROM component_updates WHERE machine_id=$1 FOR UPDATE")
                .bind(machine)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?;
    }
    let row = rows::machine(conn, machine)
        .await?
        .filter(|row| row.is_active())
        .ok_or_else(ApiError::not_found)?;
    require_machine(conn, principal, row.id, Permission::MachineManage).await?;
    require_online(ctx, &row)?;
    if row.runtime_id.is_some() {
        return Err(ApiError::new(
            ErrorCode::RequestInvalid,
            "Use this runtime's own Update action.",
        ));
    }
    let view = crate::component_updates::views(&ctx.app.link, conn, &[machine])
        .await?
        .remove(&machine)
        .ok_or_else(ApiError::not_found)?;
    let enabled = reported_policy(conn,&row).await?.ok_or_else(||ApiError::new(ErrorCode::SourceOffline,
        "This machine has not reported update.auto yet; wait for its policy report, then retry Update now."))?.update_auto;
    Ok(PreparedUpdate { row, view, enabled })
}

pub(crate) async fn apply_update_on(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &PreparedUpdate,
    before: impl FnOnce() -> ApiResult<()>,
) -> ApiResult<RequestedUpdate> {
    apply_update_with_origin(ctx, conn, principal, value, None, before).await
}

/// The same native writer can atomically bind an explicit request's real private origin.
pub(crate) async fn apply_update_with_origin(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &PreparedUpdate,
    origin: Option<(Id, &Json)>,
    before: impl FnOnce() -> ApiResult<()>,
) -> ApiResult<RequestedUpdate> {
    let row = &value.row;
    let machine = row.id;
    let enabled = value.enabled;
    let mut view = value.view.clone();
    let mut fresh = false;
    // Native drain/state/physical preparation has completed on this same connection.
    before()?;
    if view.behind {
        fresh = orch_db::queries::component_updates::request_with_origin(
            conn,
            machine,
            &view.recommended_version,
            Some(principal.user()),
            origin,
            ctx.now,
        )
        .await?;
        if !enabled && fresh {
            let detail = format!(
                "update.auto is off on this machine. Its local policy refuses Update now. Run `{}` on the machine to update it locally.",
                view.command
            );
            orch_db::queries::component_updates::finish(
                conn,
                machine,
                "refused",
                Some(&detail),
                ctx.now,
            )
            .await?;
            view.status = "refused".into();
            view.detail = Some(detail);
        } else if fresh {
            view.status = "queued".into();
            view.detail = None;
        } else {
            view = crate::component_updates::views(&ctx.app.link, conn, &[machine])
                .await?
                .remove(&machine)
                .ok_or_else(ApiError::not_found)?;
        }
    }
    audit_machine(ctx,conn,orch_db::audit::action!(AuditAction::MachineUpdateRequested),machine,
        &format!("Update requested for {}",row.name),
        json!({"running":view.running_version,"recommended":view.recommended_version,"status":view.status,"detail":view.detail})).await?;
    Ok(RequestedUpdate { fresh })
}

/// Writes the audit row of an act on a machine, in the caller's transaction. A machine lies in no
/// org, so the row has no place of its own.
pub(crate) async fn audit_machine(
    ctx: &Ctx,
    conn: &mut Conn,
    action: AuditAction,
    machine: Id,
    summary: &str,
    details: Json,
) -> ApiResult<()> {
    let mut entry = Entry::new(
        Actor::User(ctx.user()?.id()),
        action.as_str(),
        action.object_kind().unwrap_or("machine"),
        Some(machine),
        summary,
    );
    entry.history = action.is_history_line();
    entry.details = details;
    audit::append(conn, &entry, &origin(ctx), ctx.now).await?;
    Ok(())
}

/// What went wrong with a command to a machine, as the answer a client gets: a machine that did
/// not take it is offline for the purpose, and a machine that failed it is the log's to tell.
fn command_failed(machine: Id, error: RunError) -> ApiError {
    match error {
        RunError::Dispatch(crate::dispatch::DispatchError::BeforeEffect(error)) => {
            match error.downcast::<ApiError>() {
                Ok(error) => *error,
                Err(error) => ApiError::internal(error),
            }
        }
        RunError::Command(CommandFailure::Expired | CommandFailure::Cancelled) => {
            ApiError::new(ErrorCode::SourceOffline, "the machine did not answer")
        }
        other => ApiError::internal(format!("a command to machine {machine} failed: {other}")),
    }
}

/// The machine must be connected for a request that waits for its answer.
fn require_online(ctx: &Ctx, machine: &MachineRow) -> ApiResult<()> {
    if is_online(&ctx.app, machine.id, machine.online) {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::SourceOffline,
            "the machine is not connected",
        ))
    }
}

/// Keeps the facts a machine reported, with the rates it measured folded into the averages the
/// link keeps.
async fn queries_set_facts(
    conn: &mut Conn,
    id: Id,
    written: &Json,
    facts: &MachineFacts,
    at: chrono::DateTime<chrono::Utc>,
) -> ApiResult<()> {
    rows::set_machine_facts(
        conn,
        id,
        written,
        (facts.rates.up_bps, facts.rates.down_bps),
        RATE_WEIGHT,
        at,
    )
    .await?;
    Ok(())
}

// ---- the local policy ----

/// The policy a machine last reported, if it reported one.
pub(crate) async fn reported_policy(
    conn: &mut Conn,
    machine: &MachineRow,
) -> ApiResult<Option<LocalPolicy>> {
    let facts = orch_db::queries::environment_facts::machine(conn, machine.id, None).await?;
    match facts.as_ref().and_then(|facts| facts.policy.as_ref()) {
        None | Some(Json::Null) => Ok(None),
        Some(policy) => serde_json::from_value(policy.clone())
            .map(Some)
            .map_err(|error| {
                ApiError::internal(format!(
                    "the policy machine {} reported does not read: {error}",
                    machine.id
                ))
            }),
    }
}
orch_api_base::__api_bind_cb869fa787!();

/// What a key limits, as the API says it.
fn bounds_of(key: PolicyKey) -> PolicyBounds {
    match key.bounds() {
        Bounds::Environments => PolicyBounds::Environment,
        Bounds::Providers => PolicyBounds::Provider,
        Bounds::Machine => PolicyBounds::Machine,
    }
}

/// The question the access engine is asked about a key.
fn about(key: PolicyKey) -> KeyAbout {
    orch_engine::commands::policy_key_about(key)
}

/// Whether the machine's owner handed `key` over to be changed from here.
fn is_delegated(policy: &LocalPolicy, key: PolicyKey) -> bool {
    policy.delegates(key)
}

/// Machine viewers see all settings; managers of hosted work see their binding ceilings read-only.
async fn policy_sections(
    conn: &mut Conn,
    principal: &Principal,
    machine: &MachineRow,
) -> ApiResult<Vec<api::MachinePolicySection>> {
    orch_engine::commands::policy_sections(conn, principal, machine.id)
        .await
        .map_err(ApiError::engine)
}

/// Read access never grants the authority to change the physical machine.
async fn may_read_policy(
    _ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    machine: &MachineRow,
) -> ApiResult<bool> {
    Ok(!policy_sections(conn, principal, machine).await?.is_empty())
}

/// The machine `id`, for a caller who reads its policy.
async fn machine_for_policy(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<MachineRow> {
    let row = rows::machine(conn, id)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    if may_read_policy(ctx, conn, principal, &row).await? {
        Ok(row)
    } else {
        Err(ApiError::not_found())
    }
}

/// The policy of the machine as it last reported it, with what the caller may change.
async fn policy_command(conn: &mut Conn, row: &MachineRow) -> ApiResult<String> {
    let physical = orch_db::queries::environment_facts::machine(conn, row.id, None)
        .await?
        .and_then(|facts| facts.machine_id);
    let actual = match physical.filter(|id| *id != row.id) {
        Some(id) => rows::machine(conn, id).await?,
        None => None,
    };
    let row = actual.as_ref().unwrap_or(row);
    Ok(row
        .facts
        .pointer("/installation/commands/policy")
        .and_then(Json::as_str)
        .filter(|command| !command.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if row.roles.iter().any(|role| role == "node") {
                "orch-node policy".into()
            } else {
                "orch-connector policy".into()
            }
        }))
}

/// The current policy and one correct local invocation for every row and role action.
async fn policy_of(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: &MachineRow,
) -> ApiResult<MachinePolicy> {
    let refs = machine_refs(&ctx.app, conn, &[row.id]).await?;
    let machine = refs.get(&row.id).cloned().ok_or_else(ApiError::not_found)?;
    let Some(policy) = reported_policy(conn, row).await? else {
        return Ok(MachinePolicy {
            machine_view: None,
            command: Some(policy_command(conn, row).await?),
            machine,
            reported_at: None,
            keys: Vec::new(),
        });
    };
    // What a caller may change is asked once for each thing a key can limit.
    let mut changeable: BTreeMap<&'static str, bool> = BTreeMap::new();
    let mut keys = Vec::new();
    let roles = row
        .roles
        .iter()
        .map(|role| parse(role))
        .collect::<ApiResult<Vec<orch_link_contract::session::Role>>>()?;
    let readable = policy_sections(conn, principal, row).await?;
    for key in PolicyKey::ALL {
        if !key.applies_to_roles(&roles) {
            continue;
        }
        if !key
            .sections()
            .iter()
            .any(|section| readable.contains(section))
        {
            continue;
        }
        if *key == PolicyKey::ToolsFetch
            && row
                .facts
                .pointer("/build_fetch/supported")
                .and_then(Json::as_bool)
                != Some(true)
        {
            continue;
        }
        if *key == PolicyKey::MachineRoles
            && row
                .facts
                .pointer("/role_management/supported")
                .and_then(Json::as_bool)
                != Some(true)
        {
            continue;
        }
        let delegated = is_delegated(&policy, *key);
        let writable = if delegated {
            let question = about(*key);
            let word = match question {
                KeyAbout::Environments => "environments",
                KeyAbout::Providers => "providers",
                KeyAbout::TheMachine => "machine",
                KeyAbout::TheHandingOver => "never",
            };
            if let Some(known) = changeable.get(word) {
                *known
            } else {
                let keying = may_change_policy_key(
                    conn,
                    principal,
                    row.id,
                    question,
                    true,
                    Policy::default(),
                )
                .await
                .map_err(ApiError::internal)?;
                let allowed = matches!(keying, Keying::Allowed);
                changeable.insert(word, allowed);
                allowed
            }
        } else {
            false
        };
        keys.push(MachinePolicyKey {
            definition: key.definition(),
            key: key.as_str().to_owned(),
            value: policy.get(*key),
            bounds: bounds_of(*key),
            delegated,
            writable,
        });
    }
    Ok(MachinePolicy {
        machine_view: None,
        command: Some(policy_command(conn, row).await?),
        machine,
        reported_at: Some(Time::new(row.updated_at)),
        keys,
    })
}

/// Reads the local policy of a machine as it last reported it.
async fn policy_get(ctx: Ctx, (): (), (): ()) -> ApiResult<MachinePolicy> {
    // This stored read has no native command or response bookkeeping to replay.
    crate::key_operations::snapshot_read::run(ctx, policy_snapshot).await
}

async fn policy_snapshot(ctx: Ctx) -> ApiResult<MachinePolicy> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    // Environment/provider managers retain their existing section-only access. A new
    // MachineView prerequisite here would deny an ordinary supported policy reader.
    let row = machine_for_policy(&ctx, &mut tx, &principal, ctx.id("id")?).await?;
    let shown = policy_of(&ctx, &mut tx, &principal, &row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// Checks changed connection objects against schemas this host actually reported.
#[cfg(any(test, feature = "test-support"))]
fn driver_configuration_faults(
    proposed: &MachineDrivers,
    previous: Option<&MachineDrivers>,
    registered_schemas: &Json,
    descriptors: &Json,
) -> Vec<FieldFault> {
    orch_engine::commands::driver_configuration_faults(
        proposed,
        previous,
        registered_schemas,
        descriptors,
    )
}

/// Host pages synchronize through the same native policy preparation and original Host operation.
pub(crate) async fn policy_update_on(
    ctx: Ctx,
    machine: Id,
    body: api::MachinePolicyUpdateBody,
) -> ApiResult<()> {
    policies::synchronize(ctx, machine, body).await
}

#[path = "machines/acknowledgement.rs"]
pub mod acknowledgement;

#[path = "machines/enrollment.rs"]
pub mod enrollment;

#[path = "machines/individual.rs"]
pub mod individual;

#[path = "machines/mutations.rs"]
pub mod mutations;

#[path = "machines/gateway.rs"]
mod gateway;
#[path = "machines/policies.rs"]
pub mod policies;

#[path = "machines/probes.rs"]
pub mod probes;

#[path = "machines/programs.rs"]
pub mod programs;

#[path = "machines/read.rs"]
pub mod read;

#[path = "machines/read_operation.rs"]
pub mod read_operation;

#[path = "machines/revocation.rs"]
pub mod revocation;

#[path = "machines/roles.rs"]
pub mod roles;

#[path = "machines/updates.rs"]
pub mod updates;

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
    fn changed_driver_connections_report_every_schema_fault_without_echoing_values() {
        let descriptors = json!({"cluster": {"configuration_schema": {
            "type": "object", "additionalProperties": false,
            "required": ["namespace", "storage_class"],
            "properties": {"namespace": {"type": "string"},
                "storage_class": {"type": "string", "minLength": 1}}
        }}});
        let mut proposed = MachineDrivers::default();
        proposed.configurations.insert(
            "cluster".into(),
            json!({"namespace": 7, "storage_class": "", "unwanted": "private-value"}),
        );
        let faults = driver_configuration_faults(&proposed, None, &json!({}), &descriptors);
        assert!(
            faults
                .iter()
                .any(|fault| fault.path.ends_with("/namespace"))
        );
        assert!(
            faults
                .iter()
                .any(|fault| fault.path.ends_with("/storage_class"))
        );
        assert_eq!(faults.len(), 3);
        assert!(!format!("{faults:?}").contains("private-value"));
        let static_schema = json!({"cluster": descriptors["cluster"]["configuration_schema"]});
        assert_eq!(
            driver_configuration_faults(&proposed, None, &static_schema, &json!({})),
            faults
        );
        assert!(
            driver_configuration_faults(&proposed, Some(&proposed), &json!({}), &descriptors)
                .is_empty()
        );
        assert!(driver_configuration_faults(&proposed, None, &json!({}), &json!({})).is_empty());
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_changed_driver_connections_report_every_schema_fault_without_echoing_values() {
        self::changed_driver_connections_report_every_schema_fault_without_echoing_values()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn role_and_program_controls_follow_the_one_policy_before_the_next_machine_report()
    -> Result<(), orch_link_contract::policy::PolicyError> {
        let delegated = vec![
            "machine.roles".into(),
            "agent.programs".into(),
            "provider.programs".into(),
        ];
        let empty = BTreeMap::new();
        let policy = LocalPolicy::resolve(&delegated, &empty, &empty)?;
        for placement in ["connected", "managed"] {
            let mut facts = json!({"placement":placement,"policy":policy,
                "role_management":{"supported":true,"enabled":false,"delegated":false},
                "programs":{"capabilities":[{"kind":"agent","supported":true,"remote":false,"enabled":false,"delegated":false},
                    {"kind":"provider_source","supported":false,"remote":false,"enabled":false,"delegated":false}]}});
            let shown = shown_facts(&facts);
            assert_eq!(shown["role_management"]["enabled"], true);
            assert_eq!(shown["role_management"]["delegated"], true);
            assert_eq!(shown["programs"]["capabilities"][0]["remote"], true);
            assert_eq!(
                shown["programs"]["capabilities"][1]["remote"], false,
                "policy never invents unsupported services"
            );
            let local = BTreeMap::from([
                ("agent.programs".into(), json!(false)),
                ("machine.roles".into(), json!(true)),
            ]);
            facts["policy"] = json!(LocalPolicy::resolve(&delegated, &local, &empty)?);
            let shown = shown_facts(&facts);
            assert_eq!(shown["role_management"]["enabled"], true);
            assert_eq!(
                shown["role_management"]["delegated"], false,
                "a locally kept true value is still kept"
            );
            assert_eq!(shown["programs"]["capabilities"][0]["remote"], false);
        }
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_role_and_program_controls_follow_the_one_policy_before_the_next_machine_report()
    -> Result<(), ::orch_link_contract::policy::PolicyError> {
        self::role_and_program_controls_follow_the_one_policy_before_the_next_machine_report()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn example_or_missing_addresses_name_the_setting_and_prevent_minting_commands() {
        for public in [
            "https://example.com",
            "https://orch.example.com",
            "https://deep.orch.example.com",
        ] {
            let config = config_of(public, &[("wss://gateway.test:9443", "pinned")]);
            assert_eq!(
                installation_info(&config).configure.as_deref(),
                Some("public_url")
            );
            assert!(require_installable(&config).is_err());
        }
        for endpoints in [
            vec![],
            vec![("wss://gateway.example.com:9443", "pinned")],
            vec![("wss://gateway.orch.internal:9443", "pinned")],
        ] {
            let config = config_of("https://orch.test", &endpoints);
            assert_eq!(
                installation_info(&config).configure.as_deref(),
                Some("[[gateway.endpoints]]")
            );
            assert!(require_installable(&config).is_err());
        }
        assert!(
            require_installable(&config_of(
                "https://orch.test",
                &[("wss://orch.test/link", "public")]
            ))
            .is_ok()
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_example_or_missing_addresses_name_the_setting_and_prevent_minting_commands() {
        self::example_or_missing_addresses_name_the_setting_and_prevent_minting_commands()
    }

    /// A configuration whose system is at `public_url` and whose gateway advertises `endpoints`.
    fn config_of(public_url: &str, endpoints: &[(&str, &str)]) -> ServerConfig {
        let mut config = ServerConfig::default();
        config.system.public_url = public_url.to_owned();
        config.gateway.endpoints = endpoints
            .iter()
            .map(|(url, trust)| orch_config::server::GatewayEndpoint {
                url: (*url).to_owned(),
                trust: (*trust).to_owned(),
            })
            .collect();
        config
    }

    #[cfg(any(test, feature = "test-support"))]
    fn enrollment_metadata_names_an_uninstallable_system_before_a_token_is_issued() {
        for url in [
            "http://orch.example:3000",
            "http://10.0.0.1",
            "http://localhost.example",
            "http://128.0.0.1",
            "http://[::2]",
            "not a URL",
        ] {
            let config = config_of(url, &[("wss://gateway.test:9443", "pinned")]);
            assert!(!installation_info(&config).secure);
            assert!(require_installable(&config).is_err());
        }
        let config = config_of(
            "https://orch.example/",
            &[("wss://gateway.test:9443", "pinned")],
        );
        assert!(require_installable(&config).is_ok());
        assert_eq!(
            installation_info(&config).public_url,
            "https://orch.example"
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_enrollment_metadata_names_an_uninstallable_system_before_a_token_is_issued() {
        self::enrollment_metadata_names_an_uninstallable_system_before_a_token_is_issued()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn enrollment_accepts_https_and_the_installers_loopback_http_exception() {
        for address in [
            "https://orch.example",
            "http://localhost:3000",
            "http://LOCALHOST:3000",
            "http://127.0.0.1:3000",
            "http://127.45.1.7",
            "http://[::1]:3000",
            "http://[0:0:0:0:0:0:0:1]",
        ] {
            let config = config_of(address, &[("wss://gateway.test:9443", "pinned")]);
            assert!(installation_info(&config).secure, "{address}");
            assert!(require_installable(&config).is_ok(), "{address}");
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_enrollment_accepts_https_and_the_installers_loopback_http_exception() {
        self::enrollment_accepts_https_and_the_installers_loopback_http_exception()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn minted_commands_follow_the_installers_shared_address_cases() {
        for line in include_str!("../../../../../../../scripts/smoke/installers/addresses.tsv")
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
        {
            let Some((expected, address)) = line.split_once('\t') else {
                panic!("an installer case has no address: {line}")
            };
            assert_eq!(
                require_installable(&config_of(
                    address,
                    &[("wss://gateway.test:9443", "pinned")]
                ))
                .is_ok(),
                expected == "allow",
                "{address}"
            );
        }
        assert!(require_node_installable(&config_of("http://localhost:3000", &[])).is_err());
        let config = config_of(
            "http://localhost:3000",
            &[("wss://localhost:9443", "pinned")],
        );
        assert!(require_node_installable(&config).is_ok());
        let command = enrollment_commands(&config, "oenr_test.pin", Installer::Node);
        assert!(
            command
                .linux
                .contains("--from 'http://localhost:3000' --url 'wss://localhost:9443'")
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_minted_commands_follow_the_installers_shared_address_cases() {
        self::minted_commands_follow_the_installers_shared_address_cases()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn the_commands_that_enroll_a_machine_carry_the_token_and_the_address_of_the_system() {
        let config = config_of("https://orch.example.com/", &[]);
        let commands = enrollment_commands(&config, "oenr_abc.def", Installer::Connector);
        assert_eq!(
            commands.linux,
            "curl -fsSL 'https://orch.example.com/api/v1/downloads/install.sh' | sh -s -- --url 'https://orch.example.com' --token 'oenr_abc.def'"
        );
        assert_eq!(commands.macos, commands.linux);
        assert_eq!(
            commands.windows,
            "& ([scriptblock]::Create((iwr 'https://orch.example.com/api/v1/downloads/install.ps1' -UseBasicParsing))) -Url 'https://orch.example.com' -Token 'oenr_abc.def'"
        );
        // The screen adds the flag that hands the limits over at the end: the installer's
        // arguments come last.
        assert!(commands.linux.ends_with("--token 'oenr_abc.def'"));
        assert!(commands.windows.ends_with("-Token 'oenr_abc.def'"));
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_the_commands_that_enroll_a_machine_carry_the_token_and_the_address_of_the_system()
     {
        self::the_commands_that_enroll_a_machine_carry_the_token_and_the_address_of_the_system()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn an_address_with_a_quote_or_a_shell_character_stays_one_word_in_every_shell() {
        let config = config_of("https://o'rch.example.com/a b;c$(d)", &[]);
        let commands = enrollment_commands(&config, "oenr_abc.def", Installer::Connector);
        assert!(
            commands
                .linux
                .contains(r"--url 'https://o'\''rch.example.com/a b;c$(d)' --token"),
            "{}",
            commands.linux
        );
        assert!(
            commands
                .windows
                .contains("-Url 'https://o''rch.example.com/a b;c$(d)' -Token"),
            "{}",
            commands.windows
        );
        assert_eq!(ps_word("it\u{2019}s"), "'it\u{2019}\u{2019}s'");
        assert_eq!(sh_word(""), "''");
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_an_address_with_a_quote_or_a_shell_character_stays_one_word_in_every_shell() {
        self::an_address_with_a_quote_or_a_shell_character_stays_one_word_in_every_shell()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_host_installer_uses_the_first_advertised_address_in_the_systems_order() {
        let config = config_of(
            "https://orch.example.com",
            &[
                ("wss://orch.example.com/link", "public"),
                ("wss://gateway.example.com:9443", "pinned"),
            ],
        );
        let commands = enrollment_commands(&config, "oenr_abc.def", Installer::Node);
        assert_eq!(
            commands.linux,
            "curl -fsSL 'https://orch.example.com/api/v1/downloads/install-node.sh' | sh -s -- --from 'https://orch.example.com' --url 'wss://orch.example.com/link' --token 'oenr_abc.def'"
        );
        assert_eq!(commands.macos, commands.linux);
        assert!(commands.windows.starts_with("Write-Output '"));
        assert!(!commands.windows.contains("oenr_abc.def"));
        // A public-only list stays that exact listed address, including its link path.
        let public_only = config_of(
            "https://orch.example.com/",
            &[("wss://orch.example.com/link", "public")],
        );
        assert!(
            enrollment_commands(&public_only, "oenr_abc.def", Installer::Node)
                .linux
                .contains("--url 'wss://orch.example.com/link' --token")
        );
        assert!(node_endpoint(&config_of("https://orch.test", &[])).is_empty());
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_host_installer_uses_the_first_advertised_address_in_the_systems_order() {
        self::a_host_installer_uses_the_first_advertised_address_in_the_systems_order()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_key_the_owner_handed_over_singly_or_all_at_once_may_be_changed_from_here_and_the_handing_over_never()
     {
        let mut policy = LocalPolicy::default();
        assert!(!is_delegated(&policy, PolicyKey::McpAdd));
        policy.policy_delegated = vec!["mcp.add".to_owned()];
        assert!(is_delegated(&policy, PolicyKey::McpAdd));
        assert!(!is_delegated(&policy, PolicyKey::ToolsFetch));
        policy.policy_delegated = vec![orch_link_contract::policy::ALL_KEYS.to_owned()];
        assert!(is_delegated(&policy, PolicyKey::ToolsFetch));
        assert!(
            !is_delegated(&policy, PolicyKey::PolicyDelegated),
            "which keys are handed over is changed on the machine and nowhere else"
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_key_the_owner_handed_over_singly_or_all_at_once_may_be_changed_from_here_and_the_handing_over_never()
     {
        self::a_key_the_owner_handed_over_singly_or_all_at_once_may_be_changed_from_here_and_the_handing_over_never()
    }
}

// Enrollment tokens for the same machine, with ownership and registrations retained.

#[path = "machines/reenrollment.rs"]
pub mod reenrollment;
