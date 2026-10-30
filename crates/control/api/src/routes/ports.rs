//! Guarded stable port targets, reviewed wider admissions and safe outside listings.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Answer, Ctx, Registry};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::common::{DEFAULT_LIMIT, ErrorCode, MAX_LIMIT, Page};
use orch_api_contract::ports::{self as api, *};
use orch_authz::{Policy, Principal};
use orch_db::Conn;
use orch_db::audit::Actor;
use orch_db::queries::environments::routes::{self as env_rows, EnvironmentFull};
use orch_db::queries::ports as rows;
use orch_engine::port_capabilities;
use orch_engine::ports::{self as core, Requester};
use orch_types::{Id, Json, Time, new_id};
use std::collections::BTreeSet;

/// Binds the target, policy and admission core; entry and agent decisions join separately.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::EnvironmentPortList, list)
        .on(api::EnvironmentPortGet, get)
        .on(api::EnvironmentPortProgramUpdate, program)
        .on(api::EnvironmentPortOpeningPreflight, preflight)
        .on(api::EnvironmentPortOpeningCreate, create)
        .on(api::PortOpeningGet, opening)
        .on(api::PortOpeningUpdate, update)
        .on(api::PortOpeningDelete, close)
        .on(api::EnvironmentPortConnectionList, connections)
        .on(api::NodePortOpeningList, outside)
        .on(api::EnvironmentPortCapabilityGrant, capability_grant)
        .on(api::EnvironmentPortUseCreate, use_port);
}

/// Maps an ordinary access refusal with its current holders, or a port-specific policy failure.
pub(crate) async fn failure(conn: &mut Conn, error: core::Error) -> ApiError {
    match error {
        core::Error::Access(error) => refusals::problem(conn, error).await,
        core::Error::NotFound => ApiError::not_found(),
        core::Error::Invalid(reason) => ApiError::invalid("request", &reason),
        core::Error::NotAllowed(reason) => ApiError::new(ErrorCode::PortsNotAllowed, reason),
        core::Error::TooMany => ApiError::new(
            ErrorCode::PortsTooMany,
            "the distinct wider-port limit is reached",
        ),
        core::Error::Conflict => ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the reviewed port or request changed",
        ),
        core::Error::BodyConflict => ApiError::new(
            ErrorCode::ConflictIdempotencyMismatch,
            "this client reference was used with a different reviewed request",
        ),
        core::Error::ProgramConflict => ApiError::new(
            ErrorCode::PortsProgramConflict,
            "the current TCP receiver requires a fresh program review",
        ),
        core::Error::Unavailable(reason) => ApiError::new(ErrorCode::PortsUnavailable, reason),
        error => ApiError::internal(error),
    }
}
fn port(ctx: &Ctx) -> ApiResult<u16> {
    ctx.param("port")?
        .parse::<u16>()
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(|| ApiError::invalid("port", "a port is from 1 through 65535"))
}
pub(crate) fn parse<T: serde::de::DeserializeOwned>(value: &str) -> ApiResult<T> {
    serde_json::from_value(Json::String(value.into()))
        .map_err(|_| ApiError::internal("a stored port value cannot be read"))
}
fn limit(value: Option<u32>) -> usize {
    value.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize
}
fn cursor<T: std::str::FromStr>(value: Option<&str>) -> ApiResult<Option<T>> {
    value
        .map(|v| {
            v.parse()
                .map_err(|_| ApiError::invalid("cursor", "the continuation is invalid"))
        })
        .transpose()
}

/// Loads the current environment; a port target never selects another resource.
pub(crate) async fn environment(conn: &mut Conn, id: Id) -> ApiResult<EnvironmentFull> {
    env_rows::environments(conn, &[id])
        .await?
        .into_iter()
        .next()
        .ok_or_else(ApiError::not_found)
}
/// Computes the same safe exposure sign for every environment reference under current admission.
pub async fn exposures(conn: &mut Conn, ids: &[Id], now: Time) -> ApiResult<BTreeSet<Id>> {
    core::exposures(conn, ids, now)
        .await
        .map_err(ApiError::internal)
}

async fn require(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    permission: Permission,
) -> ApiResult<()> {
    crate::routes::resources::require(conn, principal, ResourceKind::Environment, id, permission)
        .await
}

/// The public name is independent of discovery, runtime generation and caller authority.
#[must_use]
pub fn address(ctx: &Ctx, environment: Id, port: u16) -> Option<String> {
    let domain = ctx.app.config.edge.ports_domain.as_deref()?.trim();
    let domain = domain.trim_end_matches('.');
    if domain.is_empty() {
        return None;
    }
    Some(format!(
        "https://e{}-p{port}.{domain}",
        environment.simple()
    ))
}
pub(crate) fn entries(
    ctx: &Ctx,
    env: Id,
    port: u16,
    methods: &[PortEntryMethod],
    transports: &[PortTransport],
    needs: PortEntryNeeds,
    program: &PortProgram,
) -> Vec<PortEntry> {
    let transport = if transports.contains(&PortTransport::Tcp) {
        "tcp"
    } else {
        "udp"
    };
    methods.iter().filter_map(|method| {
        match method {
            PortEntryMethod::Browser if transports.contains(&PortTransport::Tcp)=>address(ctx,env,port).map(|address|PortEntry {
                method:*method,transports:vec![PortTransport::Tcp],address:Some(address),command:None,needs,
                identity_methods:vec![PortIdentityMethod::HttpHeaders,PortIdentityMethod::PeerLookup],
            }),
            PortEntryMethod::Helper=>Some(PortEntry {
                method:*method,transports:transports.to_vec(),address:Some(ctx.app.config.system.public_url.clone()),
                command:Some(if needs==PortEntryNeeds::Link {format!("orch-connector port forward --link-stdin --transport {transport}")}
                    else {format!("orch-connector port forward --environment {env} --port {port} --transport {transport} --token-stdin --url {}", ctx.app.config.system.public_url.trim_end_matches('/'))}),
                needs,identity_methods:if program.proxy_protocol && transports.contains(&PortTransport::Tcp) {vec![PortIdentityMethod::ProxyV2Tcp,PortIdentityMethod::PeerLookup]} else {vec![PortIdentityMethod::PeerLookup]},
            }),
            PortEntryMethod::Browser|PortEntryMethod::Host=>None,
        }
    }).collect()
}
pub(crate) fn observed(ctx: &Ctx, env: &EnvironmentFull) -> PortSnapshot {
    let mut snapshot = ctx
        .app
        .cache
        .ports()
        .snapshot(env.id, i64::from(env.generation))
        .unwrap_or_else(|| PortSnapshot {
            environment_id: env.id,
            generation: i64::from(env.generation),
            as_of: Time::new(ctx.now),
            activity_at: None,
            discovery: PortDiscovery::Offline,
            reason: Some("no current connector measurement is available".into()),
            listeners: Vec::new(),
            udp_dropped: PortSnapshotUdpDropped::default(),
        });
    if env.status == "running"
        && env
            .machine_id
            .is_none_or(|machine| !ctx.app.link.is_online(machine))
    {
        snapshot.discovery = PortDiscovery::Offline;
        snapshot.reason = Some("the current connector is offline".into());
    }
    snapshot
}
fn state(
    env: &EnvironmentFull,
    snapshot: &PortSnapshot,
    port: u16,
) -> (PortTargetState, Option<String>) {
    if env.status != "running" {
        return (
            PortTargetState::Stopped,
            Some(format!("the environment is {}", env.status)),
        );
    }
    if snapshot.discovery != PortDiscovery::Native
        || snapshot.generation != i64::from(env.generation)
    {
        return (PortTargetState::Unavailable, snapshot.reason.clone());
    }
    let listeners = snapshot
        .listeners
        .iter()
        .filter(|l| l.port == port)
        .collect::<Vec<_>>();
    if listeners
        .iter()
        .any(|l| !l.internal && l.unavailable_reason.is_none())
    {
        (PortTargetState::Observed, None)
    } else if listeners.is_empty() {
        (
            PortTargetState::NotListening,
            Some("nothing is listening on this port".into()),
        )
    } else {
        (
            PortTargetState::Unavailable,
            Some("the current socket is Internal or has no proved environment owner".into()),
        )
    }
}

pub(crate) async fn opening_view(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: &rows::Opening,
    env: &EnvironmentFull,
    audience_only: bool,
) -> ApiResult<PortOpening> {
    let people = [row.requested_by, row.allowed_by]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let users = refusals::user_refs(conn, &people).await?;
    let refs = crate::routes::nodes::view::refs(
        conn,
        principal,
        &[row.place_id, row.requested_by_thread]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    let port = u16::try_from(row.port).map_err(ApiError::internal)?;
    let (program, _) = rows::program(conn, env.id, port).await?;
    let kind = parse(&row.kind)?;
    let transports = row
        .transports
        .iter()
        .map(|value| parse(value))
        .collect::<ApiResult<Vec<PortTransport>>>()?;
    let methods = row
        .entry_methods
        .iter()
        .map(|value| parse(value))
        .collect::<ApiResult<Vec<PortEntryMethod>>>()?;
    let mut state = parse::<PortOpeningState>(&row.state)?;
    let mut reason = row.reason.clone();
    if !matches!(
        state,
        PortOpeningState::Closed | PortOpeningState::Refused | PortOpeningState::Waiting
    ) {
        let policy = core::policies(conn, env.id)
            .await
            .map_err(ApiError::internal)?;
        // The kind's capability (neither left out by the template nor denied) is asked by `authority`.
        let effective = policy.enabled
            && core::local_enabled(conn, env)
                .await
                .map_err(ApiError::internal)?
            && row.until_at.is_none_or(|end| end > principal.now().get())
            && core::authority(conn, row, principal.now())
                .await
                .map_err(ApiError::internal)?;
        if !effective {
            state = PortOpeningState::Closed;
            reason = Some("current admission has ended".into());
        } else if kind != PortOpeningKind::Host || state == PortOpeningState::Open {
            // A host opening's number is held apart from its target's availability: one its node serves
            // shows the target's state as every opening does, and one its node could not serve keeps the
            // node's reason.
            let (target, why) = self::state(env, &observed(ctx, env), port);
            state = match target {
                PortTargetState::Observed => PortOpeningState::Open,
                PortTargetState::Stopped => PortOpeningState::Stopped,
                PortTargetState::NotListening => PortOpeningState::NotListening,
                PortTargetState::Unavailable => PortOpeningState::Unavailable,
            };
            reason = why;
        }
    }
    let (host, host_entry, outside) = if kind == PortOpeningKind::Host {
        host_parts(conn, row, &program, &transports).await?
    } else {
        (None, None, None)
    };
    if reason.is_none() && state == PortOpeningState::Open {
        reason = outside;
    }
    let manage = if audience_only {
        false
    } else {
        core::holds(conn, principal, env.id, Permission::PortsManage)
            .await
            .map_err(ApiError::internal)?
    };
    let admitted = !matches!(
        state,
        PortOpeningState::Closed | PortOpeningState::Refused | PortOpeningState::Waiting
    );
    let can_use = admitted
        && (core::holds(conn, principal, env.id, Permission::PortsView)
            .await
            .map_err(ApiError::internal)?
            || (kind == PortOpeningKind::Place
                && if let Some(place) = row.place_id {
                    core::place_view(conn, principal, place)
                        .await
                        .unwrap_or(false)
                } else {
                    false
                }));
    let admission = match &row.admission {
        Some(admission) => admission_view(conn, principal, admission).await?,
        None => None,
    };
    Ok(PortOpening {
        id: row.id,
        environment_id: row.environment_id,
        port,
        kind,
        place: row.place_id.and_then(|id| refs.get(&id).cloned()),
        label: row.label.clone(),
        transports: transports.clone(),
        entry_methods: methods.clone(),
        entries: entries(
            ctx,
            env.id,
            port,
            &methods,
            &transports,
            if kind == PortOpeningKind::Link {
                PortEntryNeeds::Link
            } else if kind == PortOpeningKind::Host {
                PortEntryNeeds::None
            } else {
                PortEntryNeeds::SignIn
            },
            &program,
        )
        .into_iter()
        .chain(host_entry.filter(|_| admitted))
        .collect(),
        requested_by: row.requested_by.and_then(|id| users.get(&id).cloned()),
        requested_by_thread: row
            .requested_by_thread
            .and_then(|id| refs.get(&id).cloned()),
        allowed_by: row.allowed_by.and_then(|id| users.get(&id).cloned()),
        admission,
        opened_at: Time::new(row.opened_at),
        until: row.until_at.map(Time::new),
        state,
        reason,
        program,
        host,
        publisher: match row.publish_machine_id {
            Some(machine) => {
                crate::routes::resources::view::machine_refs(&ctx.app, conn, &[machine])
                    .await?
                    .remove(&machine)
            }
            None => None,
        },
        host_port: row.host_port.and_then(|p| u16::try_from(p).ok()),
        fixed_host_port: row.fixed_host_port,
        host_allocated: row.host_allocated,
        revision: row.revision,
        link_recipient: row
            .link_recipient
            .as_ref()
            .map(|value| parse(value))
            .transpose()?,
        counts: None,
        can: PortOpeningCan {
            r#use: can_use,
            close: manage,
            change: manage && state != PortOpeningState::Closed,
        },
    })
}

/// The mark of a host opening whose number the host's saved ranges no longer cover: a narrowed page keeps an
/// acknowledged number until its opening closes, and admits no new one outside them.
pub(crate) const OUTSIDE_DECLARED_RANGES: &str =
    "its number lies outside the host's declared ranges now: it serves until it is closed";

/// Where a caller reaches a host number: `address:port`, an IPv6 address in brackets.
fn host_address(incoming: &str, port: u16) -> String {
    if incoming.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{incoming}]:{port}")
    } else {
        format!("{incoming}:{port}")
    }
}

/// What a host opening's view adds: its host, the entry at the host's incoming address while its number is held,
/// and [`OUTSIDE_DECLARED_RANGES`] where the saved ranges no longer cover that number.
async fn host_parts(
    conn: &mut Conn,
    row: &rows::Opening,
    program: &PortProgram,
    transports: &[PortTransport],
) -> ApiResult<(
    Option<orch_api_contract::resources::HostRef>,
    Option<PortEntry>,
    Option<String>,
)> {
    let Some(machine) = row.publish_machine_id else {
        return Ok((None, None, None));
    };
    let Some(publisher) = orch_db::queries::environment_facts::publisher(conn, machine).await?
    else {
        return Ok((None, None, None));
    };
    let reference = if let Some(host) = publisher.host_id {
        let address = crate::routes::resources::view::addresses(conn, &[host])
            .await?
            .remove(&host)
            .unwrap_or_default();
        Some(orch_api_contract::resources::HostRef {
            id: host,
            name: publisher.name,
            address,
        })
    } else {
        None
    };
    let ports = publisher.ports.and_then(|ports| {
        serde_json::from_value::<orch_link_contract::policy::MachinePorts>(ports).ok()
    });
    let declared = ports.and_then(|ports| {
        if ports.permits(PortOpeningKind::Host) {
            ports.publish
        } else {
            None
        }
    });
    let number = row.host_port.and_then(|port| u16::try_from(port).ok());
    let (Some(declared), Some(number)) = (declared, number) else {
        return Ok((reference, None, None));
    };
    let outside = (!declared
        .ranges
        .iter()
        .any(|range| range.from <= number && number <= range.to))
    .then(|| OUTSIDE_DECLARED_RANGES.to_owned());
    let entry = row.host_allocated.then(|| PortEntry {
        method: PortEntryMethod::Host,
        transports: transports.to_vec(),
        address: Some(host_address(&declared.incoming_address, number)),
        command: None,
        needs: PortEntryNeeds::None,
        // A host caller is known by its address alone, which the node gives the program in a PROXY header
        // where the program asked for one; the connection's own records name the host, not the caller.
        identity_methods: if program.proxy_protocol && transports.contains(&PortTransport::Tcp) {
            vec![PortIdentityMethod::ProxyV2Tcp]
        } else {
            Vec::new()
        },
    });
    Ok((reference, entry, outside))
}

/// Projects the same stable target for API and subsequent admitted tool/local readers.
pub async fn target(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    env: &EnvironmentFull,
    port: u16,
) -> ApiResult<PortTarget> {
    let snapshot = observed(ctx, env);
    let (program, revision) = rows::program(conn, env.id, port).await?;
    let (state, reason) = self::state(env, &snapshot, port);
    let mut listeners = snapshot
        .listeners
        .iter()
        .filter(|l| l.port == port)
        .cloned()
        .collect::<Vec<_>>();
    let thread_refs = crate::routes::nodes::view::refs(
        conn,
        principal,
        &listeners
            .iter()
            .filter_map(|l| l.process.as_ref()?.thread_id)
            .collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    for listener in &mut listeners {
        if let Some(process) = &mut listener.process
            && process
                .thread_id
                .is_some_and(|id| !thread_refs.contains_key(&id))
        {
            process.thread_id = None;
            process.item_id = None;
        }
    }
    let mut openings = Vec::new();
    for row in rows::openings(conn, env.id).await?.iter().filter(|row| {
        row.port == i32::from(port) && !matches!(row.state.as_str(), "closed" | "refused")
    }) {
        let view = opening_view(ctx, conn, principal, row, env, false).await?;
        if view.state != PortOpeningState::Closed {
            openings.push(view);
        }
    }
    Ok(PortTarget {
        environment_id: env.id,
        port,
        address: address(ctx, env.id, port),
        as_of: snapshot.as_of,
        discovery: snapshot.discovery,
        state,
        reason,
        own_entries: entries(
            ctx,
            env.id,
            port,
            &[PortEntryMethod::Browser, PortEntryMethod::Helper],
            &[PortTransport::Tcp, PortTransport::Udp],
            PortEntryNeeds::SignIn,
            &program,
        ),
        program,
        revision,
        listeners,
        openings,
        counts: None,
    })
}

pub(crate) async fn list(
    ctx: Ctx,
    query: EnvironmentPortListQuery,
    (): (),
) -> ApiResult<EnvironmentPorts> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let env = environment(&mut conn, id).await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    require(&mut conn, &principal, id, Permission::PortsView).await?;
    if query.look == Some(true) {
        drop(conn);
        let _ = ctx.app.cache.ports().look(&ctx, &env).await;
        conn = ctx.app.db.acquire().await?;
    }
    let principal = authorization::principal(&ctx, &mut conn).await?;
    require(&mut conn, &principal, id, Permission::PortsView).await?;
    let env = environment(&mut conn, id).await?;
    list_view(&ctx, &mut conn, &principal, &env, query).await
}

/// Canonical projection after the entry point proved either Ports View or an exact inside-machine scope.
pub(crate) async fn list_view(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    env: &EnvironmentFull,
    query: EnvironmentPortListQuery,
) -> ApiResult<EnvironmentPorts> {
    let id = env.id;
    let snapshot = observed(ctx, env);
    let policy = core::policies(conn, id).await.map_err(ApiError::internal)?;
    let mut openings = Vec::new();
    for row in rows::openings(conn, id)
        .await?
        .iter()
        .filter(|r| !matches!(r.state.as_str(), "closed" | "refused"))
    {
        let view = opening_view(ctx, conn, principal, row, env, false).await?;
        if view.state != PortOpeningState::Closed {
            openings.push(view);
        }
    }
    let mut ports = snapshot
        .listeners
        .iter()
        .map(|l| l.port)
        .chain(openings.iter().map(|o| o.port))
        .collect::<BTreeSet<_>>();
    if let Some(port) = query.port {
        if port == 0 {
            return Err(ApiError::invalid("port", "a port is nonzero"));
        }
        ports = BTreeSet::from([port]);
    }
    let after = cursor::<u16>(query.cursor.as_deref())?;
    let ports = ports
        .into_iter()
        .filter(|p| after.is_none_or(|after| *p > after))
        .collect::<Vec<_>>();
    let truncated = ports.len() > limit(query.limit);
    let mut targets = Vec::new();
    for port in ports.into_iter().take(limit(query.limit)) {
        targets.push(target(ctx, conn, principal, env, port).await?);
    }
    let cursor = if truncated {
        targets.last().map(|t| t.port.to_string())
    } else {
        None
    };
    let mut allowed = Vec::new();
    let mut capabilities = Vec::new();
    let gates = port_capabilities::gates(conn, id)
        .await
        .map_err(ApiError::internal)?;
    let host_listeners = gates
        .iter()
        .find(|gate| gate.kind == PortOpeningKind::Host)
        .is_some_and(|gate| gate.driver_support.supported);
    for gate in &gates {
        let local = core::local_allows(conn, env, Some(gate.kind))
            .await
            .map_err(ApiError::internal)?;
        let refused = (!gate.driver_support.supported)
            .then(|| gate.driver_support.reason.clone())
            .flatten()
            .or_else(|| refused_because(&policy, local, env, gate));
        let (scope, holders) = match &refused {
            Some(_) => policy_holders(conn, principal, env, &policy, local, gate).await?,
            None => (None, Vec::new()),
        };
        allowed.push(EnvironmentPortsAllowed {
            kind: gate.kind,
            allowed: refused.is_none(),
            reason: refused,
            scope,
            holders,
        });
        capabilities.push(capability_view(conn, principal, env, gate).await?);
    }
    Ok(EnvironmentPorts {
        environment_id: id,
        snapshot,
        targets,
        used: rows::used(conn, id).await?,
        limit: policy.max_open,
        host_ports: orch_db::queries::environment_facts::publisher_of(conn, id)
            .await?
            .and_then(|publisher| publisher.ports)
            .and_then(|ports| {
                serde_json::from_value::<orch_link_contract::policy::MachinePorts>(ports).ok()
            })
            .and_then(|ports| {
                if ports.permits(PortOpeningKind::Host) && host_listeners {
                    ports.publish
                } else {
                    None
                }
            }),
        ports_exposed: openings
            .iter()
            .any(|o| o.state != PortOpeningState::Waiting),
        openings,
        allowed,
        capabilities,
        cursor,
        truncated,
    })
}

/// Why nobody may open this audience here now, or none where it may be opened (by a person's
/// decision, or an agent's without a card where the environment holds it or the policy allows it).
fn refused_because(
    policy: &core::Policies,
    local: bool,
    _env: &EnvironmentFull,
    gate: &port_capabilities::Gate,
) -> Option<String> {
    if !policy.enabled {
        Some("Ports is off at this environment's place (ports.enabled)".to_owned())
    } else if !local {
        Some("ports (environment role): the machine's operator or managers have not allowed this audience and its reachable address".to_owned())
    } else if gate.kind == PortOpeningKind::Host && !gate.offered {
        Some("a host opening needs an allowed audience and a reachable address declared by this machine".to_owned())
    } else if !gate.offered {
        Some(format!(
            "this machine's ports policy or template does not allow \"{}\" ({})",
            gate.label, gate.key
        ))
    } else if gate.decision == "deny" {
        Some(format!(
            "\"{}\" ({}) is denied by capability.policy",
            gate.label, gate.key
        ))
    } else {
        None
    }
}

/// The place whose setting keeps an audience out, and the people who may change it there: where
/// `ports.enabled` is off, or where `capability.policy` denies the audience's capability. A template
/// that leaves it out, a machine's own ceiling and a person's machine are changed elsewhere, and name
/// nobody here.
async fn policy_holders(
    conn: &mut Conn,
    principal: &Principal,
    env: &EnvironmentFull,
    policy: &core::Policies,
    local: bool,
    gate: &port_capabilities::Gate,
) -> ApiResult<(
    Option<orch_api_contract::nodes::NodeRef>,
    Vec<orch_api_contract::access::Holder>,
)> {
    use orch_settings_contract::provenance::SourceScope;
    use orch_settings_contract::registry::{self, keys};
    let (system, node) = if !policy.enabled {
        let resolved = policy.snapshot.resolve(
            registry::get(keys::PORTS_ENABLED)
                .ok_or_else(|| ApiError::internal("the port policy is absent"))?,
        );
        let system = resolved
            .locked_by
            .as_ref()
            .is_some_and(|lock| lock.node.is_none())
            || resolved.source.scope == SourceScope::System;
        let node = if system {
            None
        } else {
            resolved
                .locked_by
                .and_then(|lock| lock.node)
                .or(resolved.source.node)
                .or(Some(env.home_node_id))
        };
        (system, node)
    } else if local && gate.offered && gate.decision == "deny" {
        // The definition's own default is the system's to change, as the policy's row there is.
        match gate.policy_node {
            Some(node) => (false, Some(node)),
            None => (true, None),
        }
    } else {
        return Ok((None, Vec::new()));
    };
    let mut ancestors = Vec::new();
    let mut orgs = Vec::new();
    let mut scope = None;
    if let Some(node) = node {
        let facts = orch_db::queries::hierarchy::with_ancestors(conn, &[node]).await?;
        if let Some(found) = facts.iter().find(|found| found.id == node) {
            ancestors = found.path.clone();
        }
        orgs = facts
            .iter()
            .filter(|node| node.kind == "org")
            .map(|node| node.id)
            .collect();
        scope = crate::routes::nodes::view::refs(conn, principal, &[node], Policy::default())
            .await?
            .remove(&node)
            .filter(|node| !node.redacted);
    }
    let candidates =
        orch_db::queries::access::admin_people(conn, &ancestors, principal.now().get(), system)
            .await?;
    let candidates = orch_db::queries::access::active_among(conn, &candidates, &orgs).await?;
    let candidates = candidates
        .into_iter()
        .take(orch_api_contract::access::ASK_MAX_PEOPLE as usize)
        .collect::<Vec<_>>();
    let refs = refusals::user_refs(conn, &candidates).await?;
    let holders = candidates
        .into_iter()
        .filter_map(|id| refs.get(&id).cloned())
        .map(|user| orch_api_contract::access::Holder::User { user })
        .collect();
    Ok((scope, holders))
}
async fn get(ctx: Ctx, query: EnvironmentPortGetQuery, (): ()) -> ApiResult<PortTarget> {
    let id = ctx.id("id")?;
    let port = port(&ctx)?;
    let mut conn = ctx.app.db.acquire().await?;
    let env = environment(&mut conn, id).await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    require(&mut conn, &principal, id, Permission::PortsView).await?;
    if query.look == Some(true) {
        drop(conn);
        let _ = ctx.app.cache.ports().look(&ctx, &env).await;
        conn = ctx.app.db.acquire().await?;
    }
    let principal = authorization::principal(&ctx, &mut conn).await?;
    require(&mut conn, &principal, id, Permission::PortsView).await?;
    let env = environment(&mut conn, id).await?;
    target(&ctx, &mut conn, &principal, &env, port).await
}

async fn program(
    ctx: Ctx,
    (): (),
    body: EnvironmentPortProgramUpdateBody,
) -> ApiResult<PortTarget> {
    let id = ctx.id("id")?;
    let port = port(&ctx)?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    require(&mut tx, guard.principal(), id, Permission::PortsManage).await?;
    core::validate_program(&body.program)
        .map_err(|e| ApiError::invalid("program", &e.to_string()))?;
    rows::lock_environment(&mut tx, id).await?;
    rows::lock_target(&mut tx, id, port).await?;
    let (old, revision) = rows::program(&mut tx, id, port).await?;
    if revision != body.expected_revision {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the target program changed",
        ));
    }
    let env = environment(&mut tx, id).await?;
    check_program(&ctx, &mut tx, &env, port, &old, &body.program).await?;
    if !rows::set_program(&mut tx, id, port, &body.program, revision).await? {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the target program changed",
        ));
    }
    record_program(&ctx, &mut tx, &env, port, guard.now()).await?;
    let result = target(&ctx, &mut tx, guard.principal(), &env, port).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(result)
}

/// Checks the reviewed receiver prefix against current TCP uses under the authority guard.
pub async fn check_program(
    ctx: &Ctx,
    conn: &mut Conn,
    env: &EnvironmentFull,
    port: u16,
    old: &PortProgram,
    desired: &PortProgram,
) -> ApiResult<()> {
    if old.proxy_protocol == desired.proxy_protocol {
        return Ok(());
    }
    let current = rows::openings(conn, env.id).await?;
    let has_audience = current.iter().any(|o| {
        o.port == i32::from(port)
            && (!matches!(o.state.as_str(), "closed" | "refused") || o.host_allocated)
            && o.transports.iter().any(|t| t == "tcp")
    });
    if has_audience {
        return Err(ApiError::new(
            ErrorCode::PortsProgramConflict,
            "close the current TCP audiences before changing the receiver prefix",
        ));
    }
    if env.status == "running"
        && let Some(machine) = env.machine_id
    {
        let current = ctx
            .app
            .link
            .port_connections(
                machine,
                env.id,
                i64::from(env.generation),
                EnvironmentPortConnectionListQuery {
                    port: Some(port),
                    transport: Some(PortTransport::Tcp),
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| {
                ApiError::new(
                    ErrorCode::PortsUnavailable,
                    "the current TCP uses cannot be established",
                )
            })?;
        if current.counts.active > 0 {
            return Err(ApiError::new(
                ErrorCode::PortsProgramConflict,
                "close the current TCP uses before changing the receiver prefix",
            ));
        }
    }
    Ok(())
}
async fn record_program(
    ctx: &Ctx,
    conn: &mut Conn,
    env: &EnvironmentFull,
    port: u16,
    now: Time,
) -> ApiResult<()> {
    let mut entry = orch_db::audit::Entry::new(
        Actor::User(ctx.user()?.id()),
        orch_db::audit::action!(AuditAction::PortsUpdated).as_str(),
        "environment",
        Some(env.id),
        "The port program options changed.",
    );
    entry.history = true;
    entry.environment_id = Some(env.id);
    entry.node_id = Some(env.home_node_id);
    entry.org_id = Some(env.org_id);
    let (program, revision) = rows::program(conn, env.id, port).await?;
    entry.details = serde_json::json!({"port":port,"program":program,"program_revision":revision});
    orch_db::audit::append(conn, &entry, &crate::sign_in::origin(ctx), now.get()).await?;
    Ok(())
}

async fn preflight(ctx: Ctx, (): (), body: PortOpeningRequest) -> ApiResult<PortOpeningPlan> {
    let id = ctx.id("id")?;
    let port = port(&ctx)?;
    if body.port != port {
        return Err(ApiError::invalid(
            "port",
            "body and path must name the same target",
        ));
    }
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    require(&mut tx, guard.principal(), id, Permission::PortsManage).await?;
    let env = environment(&mut tx, id).await?;
    let request = match core::resolve_request(
        &mut tx,
        guard.principal(),
        &env,
        &body,
        address(&ctx, id, port).is_some(),
    )
    .await
    {
        Ok(request) => request,
        Err(error) => return Err(failure(&mut tx, error).await),
    };
    let disclosure = (body.kind == PortOpeningKind::Link).then(|| {
        "The link is given to you once; its proof is absent from general lists".to_owned()
    });
    let plan = plan_of(
        &ctx,
        &mut tx,
        guard.principal(),
        &env,
        port,
        request,
        disclosure,
    )
    .await?;
    let stored = serde_json::json!({"plan":plan,"body":body});
    orch_db::queries::jobs::forks::save_preflight(
        &mut tx,
        &orch_db::queries::jobs::forks::PreflightRow {
            id: plan.preflight_id,
            kind: "port_opening".into(),
            node_id: None,
            environment_id: Some(id),
            created_by: guard.principal().user(),
            plan: stored,
            expires_at: plan.expires_at.get(),
            created_at: guard.now().get(),
        },
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(plan)
}

/// The review of one resolved request: its target, audience, what it discloses and what blocks it
/// now. A person's preflight stores it; a thread's request shows it on its card.
pub(crate) async fn plan_of(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    env: &EnvironmentFull,
    port: u16,
    request: PortOpeningRequest,
    disclosure: Option<String>,
) -> ApiResult<PortOpeningPlan> {
    let id = env.id;
    let policy = core::policies(conn, id).await.map_err(ApiError::internal)?;
    let mut blockers = Vec::new();
    if let Err(error) = core::allowed(conn, env, &request, &policy, principal.now()).await {
        let error = failure(conn, error).await;
        blockers.push(PortOpeningPlanBlocker {
            code: error.code().as_str().into(),
            message: error.problem.detail.clone(),
        });
    }
    let wider = rows::openings(conn, id).await?;
    let used = wider
        .iter()
        .filter(|o| !matches!(o.state.as_str(), "closed" | "refused"))
        .map(|o| o.port)
        .collect::<BTreeSet<_>>();
    if !used.contains(&i32::from(port)) && used.len() as i64 >= policy.max_open {
        blockers.push(PortOpeningPlanBlocker {
            code: ErrorCode::PortsTooMany.as_str().into(),
            message: "the distinct wider-port limit is reached".into(),
        });
    }
    let mut candidate = None;
    if request.kind == PortOpeningKind::Host {
        match host_candidate(&ctx.app, conn, env, &request, principal.user()).await? {
            Ok(found) => candidate = Some(found),
            Err(blocker) => blockers.push(blocker),
        }
    }
    let refs = crate::routes::nodes::view::refs(
        conn,
        principal,
        &request.place_id.into_iter().collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    let place = request.place_id.and_then(|id| refs.get(&id).cloned());
    let sentence = match request.kind {
        PortOpeningKind::Place => format!(
            "Everyone who may view {}, including guests",
            place
                .as_ref()
                .and_then(|p| p.name.as_deref())
                .unwrap_or("the named place")
        ),
        PortOpeningKind::Link => "Anybody holding this link may enter until its lapse".into(),
        PortOpeningKind::Host => {
            "Anyone who reaches this host port may enter without signing in".into()
        }
    };
    let target = target(ctx, conn, principal, env, port).await?;
    Ok(PortOpeningPlan {
        preflight_id: new_id(),
        expires_at: Time::new(
            principal.now().get() + chrono::Duration::seconds(core::PLAN_SECONDS),
        ),
        program_revision: target.revision,
        target,
        request,
        audience: PortOpeningPlanAudience {
            place,
            count: None,
            sentence,
        },
        candidate,
        disclosure,
        blockers,
    })
}

/// The host number a host opening would take: the requester's own (`host_port`) where it lies within the
/// host's saved declaration and no other opening holds it, else the lowest such number of the declaration;
/// what blocks it otherwise. Nothing is reserved by a review.
pub(crate) async fn host_candidate(
    app: &crate::state::App,
    conn: &mut Conn,
    env: &EnvironmentFull,
    request: &PortOpeningRequest,
    _caller: Id,
) -> ApiResult<Result<PortOpeningPlanCandidate, PortOpeningPlanBlocker>> {
    let blocked = |code: ErrorCode, message: String| {
        Ok(Err(PortOpeningPlanBlocker {
            code: code.as_str().into(),
            message,
        }))
    };
    let support = core::host_listener_support(conn, env.id)
        .await
        .map_err(ApiError::internal)?;
    if !support.supported {
        return blocked(
            ErrorCode::PortsNotAllowed,
            support
                .reason
                .unwrap_or_else(|| "this driver does not offer host listeners".into()),
        );
    }
    let Some(publisher) = orch_db::queries::environment_facts::publisher_of(conn, env.id).await?
    else {
        return blocked(
            ErrorCode::PortsNoRange,
            "ports (environment role): no publishing machine has been selected".into(),
        );
    };
    let machine = publisher.machine_id;
    let name = publisher.name;
    let policy = publisher.ports.and_then(|ports| {
        serde_json::from_value::<orch_link_contract::policy::MachinePorts>(ports).ok()
    });
    let Some(policy) = policy.filter(|ports| ports.permits(PortOpeningKind::Host)) else {
        return blocked(
            ErrorCode::PortsNotAllowed,
            format!(
                "ports (environment role): {name}'s operator or machine managers have not allowed publishing"
            ),
        );
    };
    let Some(declared) = policy.publish.as_ref() else {
        return blocked(
            ErrorCode::PortsNoRange,
            format!(
                "ports (environment role): {name}'s operator or machine managers must declare a reachable address, listen address and ranges"
            ),
        );
    };
    let held: BTreeSet<u16> = rows::publisher_ports_held(conn, machine)
        .await?
        .into_iter()
        .filter_map(|port| u16::try_from(port).ok())
        .collect();
    let within = |port: u16| {
        declared
            .ranges
            .iter()
            .any(|range| range.from <= port && port <= range.to)
    };
    let (host_port, fixed) = match request.host_port {
        Some(wanted) if !within(wanted) => {
            return blocked(
                ErrorCode::PortsNoRange,
                format!("{wanted} lies outside the ranges {name} declares"),
            );
        }
        Some(wanted) if held.contains(&wanted) => {
            return blocked(
                ErrorCode::PortsPortInUse,
                format!("{wanted} is held by another opening on {name}"),
            );
        }
        Some(wanted) => (wanted, true),
        None => {
            let mut ranges = declared.ranges.clone();
            ranges.sort_by_key(|range| range.from);
            let free = ranges
                .iter()
                .flat_map(|range| range.from..=range.to)
                .find(|port| *port > 0 && !held.contains(port));
            match free {
                Some(port) => (port, false),
                None => {
                    return blocked(
                        ErrorCode::PortsNoRange,
                        format!("every number {name} declares is held by an opening"),
                    );
                }
            }
        }
    };
    let host = if let Some(host) = publisher.host_id {
        let address = crate::routes::resources::view::addresses(conn, &[host])
            .await?
            .remove(&host)
            .unwrap_or_default();
        Some(orch_api_contract::resources::HostRef {
            id: host,
            name,
            address,
        })
    } else {
        None
    };
    let machine_ref = crate::routes::resources::view::machine_refs(app, conn, &[machine])
        .await?
        .remove(&machine)
        .ok_or_else(ApiError::not_found)?;
    Ok(Ok(PortOpeningPlanCandidate {
        host,
        machine: machine_ref,
        host_port,
        fixed,
        host_revision: policy.revision(),
        local_limits_revision: policy.limits.as_ref().map_or(0, |limits| limits.revision),
    }))
}

async fn create(
    ctx: Ctx,
    (): (),
    body: EnvironmentPortOpeningCreateBody,
) -> ApiResult<Answer<PortOpenReply>> {
    let id = ctx.id("id")?;
    let port = port(&ctx)?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    require(&mut tx, guard.principal(), id, Permission::PortsManage).await?;
    let stored = orch_db::queries::jobs::forks::preflight(&mut tx, body.preflight_id)
        .await?
        .filter(|p| {
            p.kind == "port_opening"
                && p.environment_id == Some(id)
                && p.created_by == guard.principal().user()
        })
        .ok_or_else(ApiError::not_found)?;
    let plan: PortOpeningPlan = serde_json::from_value(
        stored
            .plan
            .get("plan")
            .cloned()
            .ok_or_else(ApiError::not_found)?,
    )
    .map_err(ApiError::internal)?;
    if plan.request.port != port {
        return Err(ApiError::invalid(
            "preflight_id",
            "the plan names another target",
        ));
    }
    let full = stored
        .plan
        .get("body")
        .cloned()
        .ok_or_else(ApiError::not_found)?;
    let prior = rows::request(
        &mut tx,
        id,
        Some(guard.principal().user()),
        None,
        body.client_id,
    )
    .await?;
    if prior.is_none() && stored.expires_at <= guard.now().get() {
        return Err(ApiError::new(
            ErrorCode::PreflightExpired,
            "review the current port again",
        ));
    }
    let env = environment(&mut tx, id).await?;
    core::reconcile(&mut tx, id, guard.now())
        .await
        .map_err(ApiError::internal)?;
    let (old_program, _) = rows::program(&mut tx, id, port).await?;
    if prior.is_none()
        && let Some(desired) = &plan.request.program
    {
        check_program(&ctx, &mut tx, &env, port, &old_program, desired).await?;
    }
    let mut proof = None;
    let preparation = if plan.request.kind == PortOpeningKind::Link {
        use base64::Engine;
        let opening_id = new_id();
        let random: [u8; 32] = rand::random();
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random);
        let sealed = crate::routes::resources::sealing::master_key()?
            .seal(token.as_bytes(), opening_id.as_bytes())
            .map_err(ApiError::internal)?;
        let digest = orch_types::Hash::of(token.as_bytes()).as_bytes().to_vec();
        proof = Some(token);
        core::Preparation::Link {
            id: opening_id,
            digest,
            sealed,
            recipient: PortLinkRecipient::Person,
            person: Some(guard.principal().user()),
        }
    } else if plan.request.kind == PortOpeningKind::Host {
        // The reviewed candidate, exactly: a number taken since is a fresh review, never a substitute.
        let candidate = plan.candidate.as_ref().ok_or_else(|| {
            ApiError::new(
                ErrorCode::PortsNoRange,
                "the review found no host number: review the opening again",
            )
        })?;
        core::Preparation::Host {
            host: candidate.host.as_ref().map(|host| host.id),
            machine: candidate.machine.id,
            port: candidate.host_port,
            revision: candidate.host_revision,
        }
    } else {
        core::Preparation::Place
    };
    let produced = match core::produce_prepared(
        &mut tx,
        guard.principal(),
        &env,
        Requester::Person(guard.principal().user()),
        body.client_id,
        &full,
        &plan.request,
        plan.program_revision,
        &preparation,
        &crate::sign_in::origin(&ctx),
    )
    .await
    {
        Ok(produced) => produced,
        Err(error) => return Err(failure(&mut tx, error).await),
    };
    let opening = opening_view(
        &ctx,
        &mut tx,
        guard.principal(),
        &produced.opening,
        &env,
        false,
    )
    .await?;
    let link = if produced.created
        && let Some(proof) = proof
    {
        let helper = format!(
            "{}/api/v1/ports/links/{}#{proof}",
            ctx.app.config.system.public_url.trim_end_matches('/'),
            opening.id
        );
        let browser = if opening.entry_methods.contains(&PortEntryMethod::Browser) {
            address(&ctx, id, port).map(|address| format!("{address}/?orch_token={proof}"))
        } else {
            None
        };
        rows::delivered(&mut tx, opening.id).await?;
        Some(PortLinkDelivery {
            recipient: PortLinkRecipient::Person,
            url_once: Some(helper.clone()),
            browser_url_once: browser,
            helper_link_once: opening
                .entry_methods
                .contains(&PortEntryMethod::Helper)
                .then_some(helper),
            delivered_to: "the creating person, once".into(),
        })
    } else {
        None
    };
    let mut replaced = None;
    if produced.created
        && plan.request.close_replaced == Some(true)
        && let Some(old) = plan.request.replacement_of
    {
        let row = rows::opening(&mut tx, old)
            .await?
            .filter(|row| {
                row.environment_id == id
                    && row.port == i32::from(port)
                    && row.kind == "link"
                    && !matches!(row.state.as_str(), "closed" | "refused")
            })
            .ok_or_else(|| {
                ApiError::invalid(
                    "replacement_of",
                    "a replacement must name a current link on this target",
                )
            })?;
        rows::close(&mut tx, old, "explicitly_replaced", guard.now().get()).await?;
        core::record(
            &mut tx,
            &env,
            &row,
            Actor::User(guard.principal().user()),
            AuditAction::PortsClosed,
            &crate::sign_in::origin(&ctx),
            guard.now(),
        )
        .await
        .map_err(ApiError::internal)?;
        replaced = Some(old);
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    if let Some(old) = replaced {
        ctx.app.cache.ports().close_opening(old);
        ctx.app.link.terminals().close_port_opening(old);
        crate::host_ports::unpublish_in_background(&ctx.app, &[old]);
    }
    // A host opening is bound by its node after the commit; the opening says when it is open.
    if produced.created && plan.request.kind == PortOpeningKind::Host {
        crate::host_ports::publish_in_background(&ctx.app, opening.id);
    }
    let mut answer = Answer::from(PortOpenReply {
        opening,
        request: None,
        link,
    });
    if !produced.created {
        answer.status = Some(http::StatusCode::OK);
    }
    Ok(answer)
}

async fn opening(ctx: Ctx, (): (), (): ()) -> ApiResult<PortOpening> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let row = rows::opening(&mut conn, ctx.id("opening_id")?)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let env = environment(&mut conn, row.environment_id).await?;
    let door = core::holds(&mut conn, &principal, env.id, Permission::PortsView)
        .await
        .map_err(ApiError::internal)?;
    if !door
        && !(row.kind == "place"
            && row.place_id.is_some()
            && core::place_view(
                &mut conn,
                &principal,
                row.place_id.ok_or_else(ApiError::not_found)?,
            )
            .await
            .unwrap_or(false))
    {
        return Err(ApiError::not_found());
    }
    opening_view(&ctx, &mut conn, &principal, &row, &env, !door).await
}
async fn close(ctx: Ctx, (): (), body: PortRevisionBody) -> ApiResult<PortOpening> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let row = rows::opening(&mut tx, ctx.id("opening_id")?)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let env = environment(&mut tx, row.environment_id).await?;
    require(&mut tx, guard.principal(), env.id, Permission::PortsManage).await?;
    rows::lock_environment(&mut tx, env.id).await?;
    rows::lock_target(&mut tx, env.id, row.port as u16).await?;
    let current = rows::opening(&mut tx, row.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if current.state != "closed" && current.revision != body.expected_revision {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the opening changed",
        ));
    }
    if rows::close(&mut tx, row.id, "closed_by_manager", guard.now().get()).await? {
        core::record(
            &mut tx,
            &env,
            &row,
            Actor::User(guard.principal().user()),
            AuditAction::PortsClosed,
            &crate::sign_in::origin(&ctx),
            guard.now(),
        )
        .await
        .map_err(ApiError::internal)?;
    }
    let current = rows::opening(&mut tx, row.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let result = opening_view(&ctx, &mut tx, guard.principal(), &current, &env, false).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    ctx.app.cache.ports().close_opening(row.id);
    ctx.app.link.terminals().close_port_opening(row.id);
    crate::host_ports::unpublish_in_background(&ctx.app, &[row.id]);
    Ok(result)
}
async fn update(ctx: Ctx, (): (), body: PortOpeningUpdateBody) -> ApiResult<PortOpening> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let row = rows::opening(&mut tx, ctx.id("opening_id")?)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let env = environment(&mut tx, row.environment_id).await?;
    require(&mut tx, guard.principal(), env.id, Permission::PortsManage).await?;
    rows::lock_environment(&mut tx, env.id).await?;
    let until = body
        .until
        .ok_or_else(|| ApiError::invalid("until", "a link lapse is required"))?;
    let policy = core::policies(&mut tx, env.id)
        .await
        .map_err(ApiError::internal)?;
    // A link's capability (neither left out by the template nor denied) is asked by `authority`.
    if until <= guard.now()
        || until.get() > row.opened_at + chrono::Duration::seconds(policy.link_max_secs)
        || !policy.enabled
        || !core::local_enabled(&mut tx, &env)
            .await
            .map_err(ApiError::internal)?
        || !core::authority(&mut tx, &row, guard.now())
            .await
            .map_err(ApiError::internal)?
    {
        return Err(ApiError::new(
            ErrorCode::PortsNotAllowed,
            "the link cannot be extended beyond its current policy or ended authority",
        ));
    }
    if !rows::lapse(&mut tx, row.id, body.expected_revision, until.get()).await? {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the link ended or changed",
        ));
    }
    let current = rows::opening(&mut tx, row.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    core::record(
        &mut tx,
        &env,
        &current,
        Actor::User(guard.principal().user()),
        AuditAction::PortsUpdated,
        &crate::sign_in::origin(&ctx),
        guard.now(),
    )
    .await
    .map_err(ApiError::internal)?;
    let result = opening_view(&ctx, &mut tx, guard.principal(), &current, &env, false).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(result)
}

/// Current own-use or named-place admission for exactly one port and transport.
pub(crate) async fn may_use(
    conn: &mut Conn,
    principal: &Principal,
    env: &EnvironmentFull,
    port: u16,
    transport: PortTransport,
    opening: Option<Id>,
) -> ApiResult<()> {
    if let Some(id) = opening {
        let row = rows::opening(conn, id)
            .await?
            .filter(|r| {
                r.environment_id == env.id
                    && r.port == i32::from(port)
                    && r.kind == "place"
                    && !matches!(r.state.as_str(), "closed" | "refused" | "waiting")
            })
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::PortsNotAllowed,
                    "the named-place opening is not current",
                )
            })?;
        if !row.transports.contains(
            &if transport == PortTransport::Tcp {
                "tcp"
            } else {
                "udp"
            }
            .into(),
        ) || !core::authority(conn, &row, principal.now())
            .await
            .map_err(ApiError::internal)?
            || !core::place_view(
                conn,
                principal,
                row.place_id.ok_or_else(ApiError::not_found)?,
            )
            .await
            .unwrap_or(false)
        {
            return Err(ApiError::new(
                ErrorCode::PortsNotAllowed,
                "current audience View and its admitted transport are required",
            ));
        }
    } else {
        require(conn, principal, env.id, Permission::PortsView).await?;
    }
    carried(conn, env, port, transport).await
}

/// What every use needs beside its caller's admission: Ports on at the home and on the machine,
/// a running environment, a connector that reported this transport, and no dedicated PROXY
/// receiver that only its own entry may reach. A link's own capability is its admission's.
async fn carried(
    conn: &mut Conn,
    env: &EnvironmentFull,
    port: u16,
    transport: PortTransport,
) -> ApiResult<()> {
    let policy = core::policies(conn, env.id)
        .await
        .map_err(ApiError::internal)?;
    if !policy.enabled
        || !core::local_enabled(conn, env)
            .await
            .map_err(ApiError::internal)?
    {
        return Err(ApiError::new(
            ErrorCode::PortsNotAllowed,
            "Ports is disabled by current home or local policy",
        ));
    }
    if env.status != "running" {
        return Err(ApiError::new(
            ErrorCode::PortsUnavailable,
            "the environment is stopped or unavailable",
        ));
    }
    let supported = env
        .facts
        .get("ports")
        .and_then(|ports| ports.get("transports"))
        .and_then(Json::as_array)
        .is_some_and(|transports| {
            transports.iter().any(|value| {
                value.as_str()
                    == Some(if transport == PortTransport::Tcp {
                        "tcp"
                    } else {
                        "udp"
                    })
            })
        });
    if !supported {
        return Err(ApiError::new(
            ErrorCode::PortsUnavailable,
            "this connector has not reported support for the selected carrier transport",
        ));
    }
    let (program, _) = rows::program(conn, env.id, port).await?;
    if transport == PortTransport::Tcp && program.proxy_protocol {
        return Err(ApiError::new(
            ErrorCode::PortsUnavailable,
            "the dedicated receiver requires its PROXY entry owner",
        ));
    }
    Ok(())
}

/// The word a stored opening keeps for `transport`.
pub(crate) fn transport_word(transport: PortTransport) -> &'static str {
    match transport {
        PortTransport::Tcp => "tcp",
        PortTransport::Udp => "udp",
    }
}

/// Whether a link's opening admits one use of `transport` now: still admitted and not lapsed,
/// carrying that transport, its decider still holding what the decision took, and the home and
/// the machine still allowing links. Answers the caller the use is made as.
pub(crate) async fn may_use_link(
    conn: &mut Conn,
    env: &EnvironmentFull,
    row: &rows::Opening,
    transport: PortTransport,
    now: Time,
) -> ApiResult<PortCaller> {
    let port = u16::try_from(row.port).map_err(|_| ApiError::not_found())?;
    let admitted = row.environment_id == env.id
        && row.kind == "link"
        && !matches!(row.state.as_str(), "closed" | "refused" | "waiting")
        && row.until_at.is_none_or(|end| end > now.get())
        && row
            .transports
            .iter()
            .any(|word| word == transport_word(transport));
    if !admitted
        || !core::authority(conn, row, now)
            .await
            .map_err(ApiError::internal)?
    {
        return Err(ApiError::new(
            ErrorCode::PortsNotAllowed,
            "the link has lapsed, was closed or does not carry this transport",
        ));
    }
    carried(conn, env, port, transport).await?;
    Ok(PortCaller::Link {
        id: row.id,
        label: row.label.clone(),
    })
}

/// The opening and the proof of a link as its holder has it: the whole helper address
/// (`…/ports/links/<opening>#<proof>`), or the opening and the proof joined by `#`.
pub(crate) fn parse_link(link: &str) -> ApiResult<(Id, String)> {
    let last = link.trim().rsplit('/').next().unwrap_or_default();
    let (opening, proof) = last.split_once('#').ok_or_else(ApiError::not_found)?;
    let opening = opening.parse::<Id>().map_err(|_| ApiError::not_found())?;
    if proof.is_empty() {
        return Err(ApiError::not_found());
    }
    Ok((opening, proof.to_owned()))
}

/// Whether `proof` is the link of `row`, compared in time independent of where they differ.
pub(crate) fn proof_matches(row: &rows::Opening, proof: &str) -> bool {
    let Some(kept) = &row.secret_hash else {
        return false;
    };
    let offered = orch_types::Hash::of(proof.as_bytes());
    let offered = offered.as_bytes();
    kept.len() == offered.len()
        && kept
            .iter()
            .zip(offered.iter())
            .fold(0u8, |differs, (a, b)| differs | (a ^ b))
            == 0
}

/// Whether the environment serves `transport` on `port` now, as its last scoped observation says:
/// no use is admitted to a target nothing owned listens on.
pub(crate) fn listening(
    ctx: &Ctx,
    env: &EnvironmentFull,
    port: u16,
    transport: PortTransport,
) -> ApiResult<()> {
    let snapshot = observed(ctx, env);
    if snapshot.discovery != PortDiscovery::Native {
        return Err(ApiError::new(
            ErrorCode::PortsUnavailable,
            "current scoped discovery is unavailable",
        ));
    }
    if !snapshot.listeners.iter().any(|l| {
        l.port == port && l.transport == transport && !l.internal && l.unavailable_reason.is_none()
    }) {
        return Err(ApiError::new(
            ErrorCode::PortsNotListening,
            "this transport has no observed current owned service",
        ));
    }
    Ok(())
}

/// A fresh entry proof. It authorizes the one API entry of a use; the carrying ticket is signed
/// anew when that entry reports its actually observed socket peer.
pub(crate) fn entry_proof() -> String {
    use base64::Engine;
    const ENTRY_PROOF_BYTES: usize = 32;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(rand::random::<[u8; ENTRY_PROOF_BYTES]>())
}

async fn use_port(ctx: Ctx, (): (), body: EnvironmentPortUseCreateBody) -> ApiResult<PortUse> {
    let id = ctx.id("id")?;
    let port = port(&ctx)?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let env = environment(&mut tx, id).await?;
    may_use(
        &mut tx,
        guard.principal(),
        &env,
        port,
        body.transport,
        body.opening_id,
    )
    .await?;
    listening(&ctx, &env, port, body.transport)?;
    let use_id = new_id();
    let expires =
        Time::new(guard.now().get() + chrono::Duration::seconds(crate::ports_runtime::USE_SECONDS));
    let ticket = entry_proof();
    tx.commit().await.map_err(orch_db::DbError::from)?;
    ctx.app.cache.ports().admit(
        &ctx,
        &env,
        port,
        body.transport,
        body.opening_id,
        ticket,
        expires,
        use_id,
    )
}

pub(crate) async fn connections(
    ctx: Ctx,
    query: EnvironmentPortConnectionListQuery,
    (): (),
) -> ApiResult<PortConnectionList> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let env = environment(&mut conn, id).await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    require(&mut conn, &principal, id, Permission::PortsView).await?;
    drop(conn);
    let mut result = ctx
        .app
        .link
        .port_connections(
            env.machine_id.ok_or_else(|| {
                ApiError::new(ErrorCode::PortsUnavailable, "no current connector")
            })?,
            id,
            i64::from(env.generation),
            query,
        )
        .await
        .map_err(|_| {
            ApiError::new(
                ErrorCode::PortsUnavailable,
                "the live carrier record is unavailable",
            )
        })?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    require(&mut conn, &principal, id, Permission::PortsView).await?;
    if !core::holds(&mut conn, &principal, id, Permission::PortsManage)
        .await
        .map_err(ApiError::internal)?
    {
        result.connections = None;
        result.totals = None;
    }
    Ok(result)
}
async fn outside(
    ctx: Ctx,
    query: NodePortOpeningListQuery,
    (): (),
) -> ApiResult<Page<PortOpening>> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let graph = orch_authz::engine::Graph::load(&mut conn, &[id])
        .await
        .map_err(ApiError::internal)?;
    let site = graph.site_of(id).ok_or_else(ApiError::not_found)?;
    let frame = orch_authz::engine::Frame::load(
        &mut conn,
        &principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &[],
    )
    .await
    .map_err(ApiError::internal)?;
    if !frame.administers(&site) || !principal.caller().reaches(site.path.as_deref()) {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "an administrator of this place lists its outside openings",
        ));
    }
    let rows = rows::outside(
        &mut conn,
        id,
        cursor(query.cursor.as_deref())?,
        (limit(query.limit) + 1) as i64,
    )
    .await?;
    let mut items = Vec::new();
    for row in rows.iter().take(limit(query.limit)) {
        let env = environment(&mut conn, row.environment_id).await?;
        let view = opening_view(&ctx, &mut conn, &principal, row, &env, false).await?;
        if view.state != PortOpeningState::Closed
            && query.kind.is_none_or(|k| k == view.kind)
            && query.state.is_none_or(|s| s == view.state)
        {
            items.push(view);
        }
    }
    Ok(Page {
        items,
        next_cursor: if rows.len() > limit(query.limit) {
            rows.get(limit(query.limit) - 1).map(|r| r.id.to_string())
        } else {
            None
        },
    })
}
/// One audience's capability on an environment as this reader is shown it, with what they may change.
pub(crate) async fn capability_view(
    conn: &mut Conn,
    principal: &Principal,
    env: &EnvironmentFull,
    gate: &port_capabilities::Gate,
) -> ApiResult<PortCapability> {
    let manage = core::holds(conn, principal, env.id, Permission::PortsManage)
        .await
        .map_err(ApiError::internal)?;
    let can_grant = manage
        && gate.permits()
        && !gate.granted
        && port_capabilities::may_grant(conn, principal, env.id, &gate.approver_scope)
            .await
            .map_err(ApiError::internal)?;
    let admission = match gate.admission() {
        Some(stored) => admission_view(conn, principal, &stored).await?,
        None => None,
    };
    Ok(PortCapability {
        key: gate.key.clone(),
        kind: gate.kind,
        label: gate.label.clone(),
        risk: parse(&gate.risk)?,
        offered: gate.offered,
        granted: gate.granted,
        from_template: gate.from_template,
        decision: parse(&gate.decision)?,
        approver_scope: parse(&gate.approver_scope)?,
        admission,
        can_grant,
        can_revoke: manage && gate.granted,
    })
}

/// What admitted an agent's opening, as this reader is shown it: the policy's place only where they
/// may see it, and whose decision it was.
pub(crate) async fn admission_view(
    conn: &mut Conn,
    principal: &Principal,
    stored: &Json,
) -> ApiResult<Option<PortOpeningAdmission>> {
    let Some(capability) = stored.get("capability").and_then(Json::as_str) else {
        return Ok(None);
    };
    let source = match stored.get("source").and_then(Json::as_str) {
        Some("grant") => PortAdmissionSource::Grant,
        Some("template") => PortAdmissionSource::Template,
        Some("policy") => PortAdmissionSource::Policy,
        _ => return Ok(None),
    };
    let id_of = |member: &str| {
        stored
            .get(member)
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok())
    };
    let scope = match id_of("node_id") {
        Some(node) => crate::routes::nodes::view::refs(conn, principal, &[node], Policy::default())
            .await?
            .remove(&node)
            .filter(|node| !node.redacted),
        None => None,
    };
    let by = match id_of("by_user_id") {
        Some(user) => refusals::user_refs(conn, &[user]).await?.remove(&user),
        None => None,
    };
    Ok(Some(PortOpeningAdmission {
        capability: capability.to_owned(),
        source,
        scope,
        by,
    }))
}

/// The audience the capability key in the path names: one of the three port keys.
fn kind_in_path(ctx: &Ctx) -> ApiResult<PortOpeningKind> {
    port_capabilities::kind_of_key(ctx.param("key")?).ok_or_else(|| {
        ApiError::invalid(
            "key",
            "a port capability is port_place, port_link or port_host",
        )
    })
}

/// `PUT /environments/{id}/ports/capabilities/{key}`: grants the environment one audience's
/// capability, for a holder of Ports Manage among the capability's approvers by its policy. Agents'
/// requests of that kind then open without a card; nothing of the unit changes. Giving what is held
/// changes nothing.
async fn capability_grant(ctx: Ctx, (): (), (): ()) -> ApiResult<PortCapability> {
    let id = ctx.id("id")?;
    let kind = kind_in_path(&ctx)?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    require(&mut tx, guard.principal(), id, Permission::PortsManage).await?;
    let env = environment(&mut tx, id).await?;
    rows::lock_environment(&mut tx, id).await?;
    if let Err(error) = port_capabilities::grant(
        &mut tx,
        guard.principal(),
        id,
        kind,
        &crate::sign_in::origin(&ctx),
    )
    .await
    {
        return Err(failure(&mut tx, error).await);
    }
    let gate = port_capabilities::gate(&mut tx, id, kind)
        .await
        .map_err(ApiError::internal)?;
    let result = capability_view(&mut tx, guard.principal(), &env, &gate).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(result)
}
