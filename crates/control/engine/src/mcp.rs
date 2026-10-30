//! The MCP servers of a session: presets, servers imported from a harness's own configuration, which run from the copy their machine keeps, and servers an environment was given as its own; found, imported, enabled; and one that is enabled where its definition is not, unavailable with that reason.
//!
//! Enablement is resolved at the environment, with its template's defaults beneath its settings.
//! The definition of an imported server never leaves the connector; only its identity is sent back.

use crate::commands::{Outcome, Outgoing};
use crate::{Engine, EngineError};
use orch_api_contract::environment::{McpFrom, TemplateMark};
use orch_authz::usable::{self, Policies, Usable};
use orch_authz::{Caller, Principal};
use orch_db::{Conn, queries::resources::mcp as rows};
use orch_link_contract::commands::{
    McpStart, McpStartGiven, McpStartLocal, Secret, SerialKey, SessionOpenMcp, TicketSlot, True,
};
use orch_link_contract::tunnels::{Service, TunnelKind};
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};

/// An enabled server with its origin and anything that makes it unavailable here.
#[derive(Debug, Clone)]
pub struct ResolvedServer {
    /// Its safe stored definition or import metadata.
    pub row: rows::McpRow,
    /// The setting or template that enabled it.
    pub from: Option<McpFrom>,
    /// Why it cannot be started here.
    pub reason: Option<String>,
}

/// Resolves every enabled preset, import and own server without starting a process.
pub async fn resolve(
    conn: &mut Conn,
    environment: Id,
    at: Time,
) -> Result<Vec<ResolvedServer>, EngineError> {
    let env = orch_db::queries::environments::hosting::environment(conn, environment)
        .await?
        .ok_or_else(|| EngineError::unreadable("the environment is not there"))?;
    let policy = orch_db::queries::environment_facts::of(conn, environment)
        .await?
        .and_then(|machine| machine.policy);
    let template =
        orch_db::queries::environments::thread_engine::template_spec(conn, environment).await?;
    resolve_where(
        conn,
        &Where {
            environment: Some(environment),
            home: env.home_node_id,
            owner: env.owner_id,
            imports_from: env.machine_id,
            policy: policy.as_ref(),
            template: template.as_ref(),
            own: None,
            granted: &env.granted_capabilities,
        },
        at,
    )
    .await
}

/// Where an environment's servers would run when a plan takes it elsewhere.
pub(crate) enum Elsewhere<'a> {
    /// The same environment on another host: that host's policy holds, and what was imported from the machine
    /// it ran on stays there.
    Moved {
        /// The environment.
        environment: Id,
        /// The host machine it goes to.
        machine: Option<Id>,
    },
    /// An environment that is not made yet, on the host it would be made on.
    New {
        /// Its settings home.
        home: Id,
        /// Its first owner, the writer of its own values.
        owner: Id,
        /// The host machine it would be made on.
        machine: Option<Id>,
        /// Its template's definition.
        template: Option<&'a Json>,
        /// Its own configuration.
        own: &'a Json,
    },
}

/// Resolves the servers an environment would have elsewhere, by the same rules as where it is, without starting
/// anything.
pub(crate) async fn resolve_elsewhere(
    conn: &mut Conn,
    elsewhere: &Elsewhere<'_>,
    at: Time,
) -> Result<Vec<ResolvedServer>, EngineError> {
    let machine = match elsewhere {
        Elsewhere::Moved { machine, .. } | Elsewhere::New { machine, .. } => *machine,
    };
    let policy = match machine {
        Some(machine) => orch_db::queries::resources::machine(conn, machine)
            .await?
            .and_then(|row| row.facts.get("policy").cloned()),
        None => None,
    };
    match elsewhere {
        Elsewhere::Moved { environment, .. } => {
            let env = orch_db::queries::environments::hosting::environment(conn, *environment)
                .await?
                .ok_or_else(|| EngineError::unreadable("the environment is not there"))?;
            let template =
                orch_db::queries::environments::thread_engine::template_spec(conn, *environment)
                    .await?;
            resolve_where(
                conn,
                &Where {
                    environment: Some(*environment),
                    home: env.home_node_id,
                    owner: env.owner_id,
                    // A unit made elsewhere is another machine; nothing imported where it ran is there.
                    imports_from: None,
                    policy: policy.as_ref(),
                    template: template.as_ref(),
                    own: None,
                    granted: &env.granted_capabilities,
                },
                at,
            )
            .await
        }
        Elsewhere::New {
            home,
            owner,
            template,
            own,
            ..
        } => {
            resolve_where(
                conn,
                &Where {
                    environment: None,
                    home: *home,
                    owner: *owner,
                    imports_from: None,
                    policy: policy.as_ref(),
                    template: *template,
                    own: Some(*own),
                    granted: &[],
                },
                at,
            )
            .await
        }
    }
}

/// What one resolution reads: the environment, or what a new one would be.
struct Where<'a> {
    /// The environment, when it exists: its settings, its own servers and its configuration's writer.
    environment: Option<Id>,
    /// Its settings home.
    home: Id,
    /// Its first owner.
    owner: Id,
    /// The machine whose imported servers it may run.
    imports_from: Option<Id>,
    /// The policy of the machine that runs its servers.
    policy: Option<&'a Json>,
    /// Its template's definition.
    template: Option<&'a Json>,
    /// For a new environment, its own configuration.
    own: Option<&'a Json>,
    /// The capabilities granted to it.
    granted: &'a [String],
}

async fn resolve_where(
    conn: &mut Conn,
    at_place: &Where<'_>,
    at: Time,
) -> Result<Vec<ResolvedServer>, EngineError> {
    let def = registry::get(keys::MCP_ENABLED)
        .ok_or_else(|| EngineError::unreadable("MCP enablement is not registered"))?;
    let place = match at_place.environment {
        Some(environment) => Place::Environment(environment),
        None => Place::Node(at_place.home),
    };
    let snapshot = Snapshot::load(conn, place, Wanted::Keys(&[def])).await?;
    let values = snapshot.resolve(def);
    let chain: Vec<Id> = snapshot.chain().iter().map(|link| link.id).collect();
    let writers = rows::settings(conn, &chain).await?;
    let nodes = orch_db::queries::hierarchy::with_ancestors(conn, &[at_place.home]).await?;
    let permits_add = at_place
        .policy
        .and_then(|policy| policy.get("mcp.add"))
        .and_then(Json::as_bool)
        == Some(true);
    let permits_import = at_place
        .policy
        .and_then(|policy| policy.get("mcp.import"))
        .and_then(Json::as_bool)
        == Some(true);
    let mut enabled: HashMap<Id, (bool, Option<McpFrom>, Id, usize)> = HashMap::new();
    for id in at_place
        .template
        .and_then(|spec| spec.get("mcp_server_ids"))
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|id| id.as_str().and_then(|id| id.parse().ok()))
    {
        enabled.insert(
            id,
            (
                true,
                Some(McpFrom::Template(TemplateMark::Template)),
                at_place.owner,
                0,
            ),
        );
    }
    let own_writer = match at_place.environment {
        Some(environment) => {
            orch_db::queries::resources::mounts::configuration_writer(conn, environment)
                .await?
                .unwrap_or(at_place.owner)
        }
        None => at_place.owner,
    };
    for (depth, part) in values.parts.iter().enumerate() {
        let source = part
            .source
            .node
            .and_then(|id| nodes.iter().find(|node| node.id == id))
            .map(|node| {
                Ok::<_, EngineError>(McpFrom::Node(orch_api_contract::nodes::NodeRef {
                    thread_display: None,
                    id: node.id,
                    kind: serde_json::from_value(json!(node.kind))?,
                    name: None,
                    redacted: true,
                }))
            })
            .transpose()?;
        let writer = part
            .source
            .node
            .and_then(|id| {
                writers
                    .iter()
                    .find(|(node, _)| *node == id)
                    .and_then(|(_, writer)| *writer)
            })
            .unwrap_or(own_writer);
        for (id, value) in part.value.as_object().into_iter().flatten() {
            if let (Ok(id), Some(value)) = (id.parse(), value.as_bool()) {
                enabled.insert(id, (value, source.clone(), writer, depth + 1));
            }
        }
    }
    // A new environment's own values come after its home's, as an existing one's do, unless a lock holds them.
    if values.locked_by.is_none()
        && let Some(own) = at_place
            .own
            .and_then(|own| own.get(keys::MCP_ENABLED))
            .and_then(Json::as_object)
    {
        for (id, value) in own {
            if let (Ok(id), Some(value)) = (id.parse(), value.as_bool()) {
                enabled.insert(id, (value, None, at_place.owner, values.parts.len() + 1));
            }
        }
    }
    let own = match at_place.environment {
        Some(environment) => rows::own(conn, environment).await?,
        None => Vec::new(),
    };
    for row in &own {
        enabled
            .entry(row.id)
            .or_insert((true, None, at_place.owner, values.parts.len() + 1));
    }
    let ids: Vec<Id> = enabled
        .iter()
        .filter(|(_, (on, _, _, _))| *on)
        .map(|(id, _)| *id)
        .collect();
    let mut servers = rows::servers(conn, &ids).await?;
    let offerings = rows::offerings(conn, &ids).await?;
    servers.sort_by_key(|row| {
        (
            std::cmp::Reverse(enabled.get(&row.id).map_or(0, |(_, _, _, depth)| *depth)),
            row.id,
        )
    });
    let mut names = HashSet::new();
    let mut result = Vec::new();
    for mut row in servers {
        let offered = offerings
            .iter()
            .filter(|offering| offering.server_id == row.id && offering.state == "ready")
            .collect::<Vec<_>>();
        if row.imported {
            row.machine_id = offered
                .iter()
                .find(|offering| Some(offering.machine_id) == at_place.imports_from)
                .map(|offering| offering.machine_id)
                .or_else(|| offered.first().map(|offering| offering.machine_id));
        }
        let (_, from, writer, _) = enabled
            .get(&row.id)
            .cloned()
            .ok_or_else(|| EngineError::unreadable("the MCP source is missing"))?;
        let mut reason = if !row.enabled {
            Some("this server is switched off".to_owned())
        } else if row
            .environment_id
            .is_some_and(|id| Some(id) != at_place.environment)
        {
            Some("this server belongs to another environment".to_owned())
        } else if row.imported && offered.is_empty() {
            Some("No machine".to_owned())
        } else if row.imported && row.machine_id != at_place.imports_from {
            Some("its imported definition belongs to another machine".to_owned())
        } else if row.environment_id.is_some() && !permits_add {
            Some("the machine permits no MCP server added from here".to_owned())
        } else if row.imported && !permits_import {
            Some("the machine permits no imported MCP server".to_owned())
        } else if !names.insert(row.name.clone()) {
            Some("another enabled server has the same name".to_owned())
        } else {
            None
        };
        if row.environment_id.is_none() {
            let place = match &from {
                Some(McpFrom::Node(node)) => node.id,
                _ => at_place.home,
            };
            if !source_may_use(conn, writer, row.id, place, at).await? {
                reason = Some("whoever enabled it may no longer use it".to_owned());
            }
        }
        if !row.imported {
            let spec: orch_agent_contract::discovery::McpServerSpec =
                serde_json::from_value(row.spec.clone())?;
            if spec
                .requires
                .capabilities
                .iter()
                .any(|capability| !at_place.granted.contains(capability))
            {
                reason =
                    Some("a required capability is not granted to this environment".to_owned());
            }
        }
        result.push(ResolvedServer { row, from, reason });
    }
    Ok(result)
}

/// What becomes of each MCP server of the source and the target when a plan takes an environment elsewhere,
/// read from the two resolutions alone.
#[must_use]
pub(crate) fn plan(
    source: &[ResolvedServer],
    target: &[ResolvedServer],
) -> Vec<orch_api_contract::forks::PlanMcpServer> {
    use orch_api_contract::forks::{McpFate, PlanMcpServer};
    let mut ids: Vec<Id> = source
        .iter()
        .chain(target)
        .map(|server| server.row.id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .filter_map(|id| {
            let before = source.iter().find(|server| server.row.id == id);
            let after = target.iter().find(|server| server.row.id == id);
            let row = &after.or(before)?.row;
            let had = before.is_some_and(|server| server.reason.is_none());
            let (fate, reason) = match after {
                Some(server) if server.reason.is_none() => {
                    (if had { McpFate::Kept } else { McpFate::New }, None)
                }
                // An import stays on the machine it was found on, and an environment's own server with it.
                Some(server) if row.imported || row.environment_id.is_some() => {
                    (McpFate::Lost, server.reason.clone())
                }
                Some(server) => (McpFate::Unavailable, server.reason.clone()),
                None => (
                    McpFate::Lost,
                    Some(if row.environment_id.is_some() {
                        "it is the source environment's own".to_owned()
                    } else {
                        "it is not switched on there".to_owned()
                    }),
                ),
            };
            Some(PlanMcpServer {
                id,
                name: row.name.clone(),
                own: row.environment_id.is_some(),
                imported: row.imported,
                fate,
                reason,
            })
        })
        .collect()
}

/// Checks the setting's author in the setting's work context, including a grant to work there.
pub(crate) async fn source_may_use(
    conn: &mut Conn,
    writer: Id,
    resource: Id,
    place: Id,
    at: Time,
) -> Result<bool, EngineError> {
    // When a writer leaves this tree, its surviving place owner receives the setting's
    // custody as well. A writer who remains a member is still checked under their own rights.
    let writer = orch_authz::ownership::owner_of_new(conn, writer, Some(place)).await?;
    let principal = match Principal::load(conn, Caller::session(writer), at).await {
        Ok(principal) => principal,
        Err(orch_authz::Error::NoSuchUser(_)) => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    Ok(matches!(
        usable::usable(
            conn,
            &principal,
            resource,
            Some(place),
            &Policies::default()
        )
        .await?,
        Usable::Yes(_)
    ))
}

impl Engine {
    /// Fingerprints the effective definitions, rather than per-session tunnel tickets.
    pub(crate) async fn mcp_revision(
        &self,
        environment: Id,
    ) -> Result<crate::harnesses::McpRevision, EngineError> {
        let mut conn = self.db().acquire().await?;
        let servers = resolve(&mut conn, environment, Time::new(self.inner.clock.now())).await?;
        let value: Vec<_> = servers
            .into_iter()
            .map(|server| {
                (
                    server.row.id,
                    server.row.name,
                    server.row.spec,
                    server.reason,
                )
            })
            .collect();
        let states = orch_db::queries::environments::hosting::environment(&mut conn, environment)
            .await?
            .map(|row| row.facts.get("mcp_servers").cloned().unwrap_or(json!([])))
            .unwrap_or(json!([]));
        Ok(crate::harnesses::McpRevision::of(&serde_json::to_vec(&(
            value, states,
        ))?))
    }
    /// Starts each resolved server once per definition and environment generation, then gives its tunnel to a session.
    pub async fn mcp_for_session(
        &self,
        environment: Id,
        thread: Id,
    ) -> Result<Vec<SessionOpenMcp>, EngineError> {
        self.mcp_for_session_inner(environment, thread, true).await
    }

    /// Reissues only tickets for already-running servers; a refresh never starts or changes another server.
    pub(crate) async fn current_mcp_for_session(
        &self,
        environment: Id,
        thread: Id,
    ) -> Result<Vec<SessionOpenMcp>, EngineError> {
        self.mcp_for_session_inner(environment, thread, false).await
    }

    async fn mcp_for_session_inner(
        &self,
        environment: Id,
        thread: Id,
        ensure: bool,
    ) -> Result<Vec<SessionOpenMcp>, EngineError> {
        let (env, servers) = {
            let mut conn = self.db().acquire().await?;
            let env = orch_db::queries::environments::hosting::environment(&mut conn, environment)
                .await?
                .ok_or_else(|| EngineError::unreadable("the environment is not there"))?;
            let servers =
                resolve(&mut conn, environment, Time::new(self.inner.clock.now())).await?;
            (env, servers)
        };
        let Some(machine) = env.machine_id else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        for server in servers.into_iter().filter(|server| server.reason.is_none()) {
            if !ensure
                && !env
                    .facts
                    .get("mcp_servers")
                    .and_then(Json::as_array)
                    .is_some_and(|states| {
                        states.iter().any(|state| {
                            state["name"] == server.row.name && state["state"] == "running"
                        })
                    })
            {
                continue;
            }
            let failed = env
                .facts
                .get("mcp_servers")
                .and_then(Json::as_array)
                .is_some_and(|states| {
                    states
                        .iter()
                        .any(|state| state["name"] == server.row.name && state["state"] == "failed")
                });
            if failed {
                continue;
            }
            let body = if server.row.imported {
                McpStart::Local(McpStartLocal {
                    environment_id: environment,
                    name: server.row.name.clone(),
                    server_id: server.row.id,
                    local: True,
                })
            } else {
                let mut conn = self.db().acquire().await?;
                let mut secrets: BTreeMap<String, Secret> =
                    rows::secret_ids(&mut conn, server.row.id)
                        .await?
                        .into_iter()
                        .map(|(name, id)| (name, Secret::Ref(id)))
                        .collect();
                // A shared preset can name an environment's inherited secret without ever storing its value in the preset.
                let spec: orch_agent_contract::discovery::McpServerSpec =
                    serde_json::from_value(server.row.spec.clone())?;
                // Turning authentication off must not forward a retained, now-unused credential.
                secrets.retain(|name, _| spec.secret_env.contains(name));
                let allowed: Vec<String> = orch_settings::get(
                    &mut conn,
                    Place::Environment(environment),
                    keys::ENV_SECRET_NAMES,
                )
                .await?
                .unwrap_or_default();
                for name in &spec.secret_env {
                    if secrets.contains_key(name) {
                        continue;
                    }
                    if !allowed.contains(name) {
                        continue;
                    }
                    let listed = orch_settings::secrets::list(
                        &mut conn,
                        orch_settings::secrets::Owner::Environment(environment),
                    )
                    .await
                    .map_err(EngineError::unreadable)?;
                    if let Some(info) = listed.iter().find(|info| info.name == *name) {
                        let (kind, owner) = info
                            .node
                            .map_or(("environment", environment), |node| ("setting", node));
                        if let Some(secret) =
                            orch_db::queries::settings::secret(&mut conn, kind, owner, name).await?
                        {
                            secrets.insert(name.clone(), Secret::Ref(secret.id));
                        }
                    }
                }
                McpStart::Given(Box::new(McpStartGiven {
                    server_id: Some(server.row.id),
                    environment_id: environment,
                    name: server.row.name.clone(),
                    spec: serde_json::from_value(server.row.spec.clone())?,
                    secrets,
                }))
            };
            let digest =
                orch_types::Hash::of(serde_json::to_vec(&(&body, &server.row.spec))?.as_slice());
            let command = Outgoing::new(
                crate::ids::derived(
                    "mcp.start",
                    &[
                        environment.as_bytes(),
                        server.row.id.as_bytes(),
                        &env.generation.to_le_bytes(),
                        digest.as_bytes(),
                    ],
                ),
                machine,
                SerialKey::Environment(environment),
                &body,
            )?;
            if ensure
                && !matches!(
                    self.ask_bounded(command).await?,
                    Some(Outcome::Succeeded(_))
                )
            {
                continue;
            }
            let service = Service::Mcp {
                server: server.row.name.clone(),
            };
            let mut ticket = TicketSlot::ask(TunnelKind::Mcp);
            if let TicketSlot::Asked(ask) = &mut ticket {
                ask.ticket
                    .more
                    .insert("service".to_owned(), json!(service.to_string()));
                ask.ticket.more.insert("dst".to_owned(), json!(machine));
                ask.ticket
                    .more
                    .insert("thread_id".to_owned(), json!(thread));
                ask.ticket
                    .more
                    .insert("environment_id".to_owned(), json!(environment));
                ask.ticket
                    .more
                    .insert("environment_generation".to_owned(), json!(env.generation));
            }
            result.push(SessionOpenMcp {
                server_id: Some(server.row.id),
                name: server.row.name,
                ticket,
            });
        }
        Ok(result)
    }
}

impl Engine {
    /// Reloads only open native sessions that declare it. Other sessions take the revision at their next turn.
    pub(crate) async fn reload_mcp(
        &self,
        environment: Id,
        operation: Id,
    ) -> Result<(), EngineError> {
        let mut conn = self.db().acquire().await?;
        let sessions: Vec<(Id,Id,Id,Id,Option<Id>)> = sqlx::query_as("SELECT s.id,s.thread_id,s.provider_id,s.agent_id,s.machine_id FROM segments s JOIN threads t ON t.current_segment_id=s.id WHERE s.environment_id=$1 AND s.ended_at IS NULL")
            .bind(environment).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        drop(conn);
        let revision = self.mcp_revision(environment).await?;
        for (segment, thread, provider, agent, machine) in sessions {
            let Some(machine) = machine else {
                continue;
            };
            let key = crate::harnesses::HarnessKey {
                provider,
                agent,
                machine,
            };
            if !crate::engine::lock(&self.inner.sessions).is_open(segment, key) {
                continue;
            }
            let mut conn = self.db().acquire().await?;
            let descriptor = orch_db::queries::resources::thread_engine::agent(&mut conn, agent)
                .await?
                .map(|agent| crate::harnesses::descriptor_of(&agent))
                .transpose()?
                .flatten();
            drop(conn);
            if !descriptor.is_some_and(|descriptor| descriptor.capabilities.mcp.reload) {
                continue;
            }
            let mcp = self.current_mcp_for_session(environment, thread).await?;
            let outgoing = Outgoing::new(
                crate::ids::derived(
                    "session.mcp.reload",
                    &[operation.as_bytes(), segment.as_bytes()],
                ),
                machine,
                SerialKey::Thread(thread),
                &orch_link_contract::commands::SessionMcpReload {
                    thread_id: thread,
                    segment_id: segment,
                    mcp,
                },
            )?;
            if matches!(
                self.ask_bounded(outgoing).await?,
                Some(Outcome::Succeeded(_))
            ) {
                crate::engine::lock(&self.inner.sessions).mcp_requested(segment, revision);
            } else {
                return Err(EngineError::unreadable(
                    "the server restarted, but an agent's live tool refresh did not finish; its next turn will read the new set",
                ));
            }
        }
        Ok(())
    }
}

/// A single server's startup inputs, with secret references only; a restart job never resolves the other servers' values.
pub(crate) async fn restart_request(
    conn: &mut Conn,
    environment: Id,
    server: &ResolvedServer,
) -> Result<McpStart, EngineError> {
    if server.row.imported {
        return Ok(McpStart::Local(McpStartLocal {
            environment_id: environment,
            name: server.row.name.clone(),
            server_id: server.row.id,
            local: True,
        }));
    }
    let spec: orch_agent_contract::discovery::McpServerSpec =
        serde_json::from_value(server.row.spec.clone())?;
    let mut secrets: BTreeMap<String, Secret> = rows::secret_ids(conn, server.row.id)
        .await?
        .into_iter()
        .map(|(name, id)| (name, Secret::Ref(id)))
        .collect();
    secrets.retain(|name, _| spec.secret_env.contains(name));
    let allowed: Vec<String> = orch_settings::get(
        conn,
        Place::Environment(environment),
        keys::ENV_SECRET_NAMES,
    )
    .await?
    .unwrap_or_default();
    if spec
        .secret_env
        .iter()
        .any(|name| !secrets.contains_key(name) && allowed.contains(name))
    {
        let listed = orch_settings::secrets::list(
            conn,
            orch_settings::secrets::Owner::Environment(environment),
        )
        .await
        .map_err(EngineError::unreadable)?;
        for name in &spec.secret_env {
            if secrets.contains_key(name) || !allowed.contains(name) {
                continue;
            }
            if let Some(info) = listed.iter().find(|info| info.name == *name) {
                let (kind, owner) = info
                    .node
                    .map_or(("environment", environment), |node| ("setting", node));
                if let Some(secret) =
                    orch_db::queries::settings::secret(conn, kind, owner, name).await?
                {
                    secrets.insert(name.clone(), Secret::Ref(secret.id));
                }
            }
        }
    }
    Ok(McpStart::Given(Box::new(McpStartGiven {
        server_id: Some(server.row.id),
        environment_id: environment,
        name: server.row.name.clone(),
        spec,
        secrets,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::forks::McpFate;

    fn server(name: &str, machine: Option<Id>, environment: Option<Id>) -> rows::McpRow {
        rows::McpRow {
            imported: machine.is_some(),
            id: orch_types::new_id(),
            name: name.into(),
            description: String::new(),
            org_id: None,
            home_id: None,
            owner_id: None,
            machine_id: machine,
            environment_id: environment,
            spec: json!({}),
            enabled: true,
            revision: 1,
        }
    }

    fn resolved(row: &rows::McpRow, reason: Option<&str>) -> ResolvedServer {
        ResolvedServer {
            row: row.clone(),
            from: None,
            reason: reason.map(str::to_owned),
        }
    }

    #[test]
    fn a_plan_says_what_each_server_becomes_from_the_two_sides_alone() {
        let machine = orch_types::new_id();
        let environment = orch_types::new_id();
        let preset = server("tmux", None, None);
        let import = server("local-db", Some(machine), None);
        let own = server("trial", None, Some(environment));
        let blocked = server("browser", None, None);
        let fresh = server("search", None, None);
        let source = vec![
            resolved(&preset, None),
            resolved(&import, None),
            resolved(&own, None),
            resolved(&blocked, None),
        ];
        let target = vec![
            resolved(&preset, None),
            resolved(
                &import,
                Some("its imported definition belongs to another machine"),
            ),
            resolved(
                &blocked,
                Some("a required capability is not granted to this environment"),
            ),
            resolved(&fresh, None),
        ];
        let plan = plan(&source, &target);
        let fate = |name: &str| {
            plan.iter()
                .find(|server| server.name == name)
                .map(|server| (server.fate, server.reason.clone()))
                .unwrap()
        };
        assert_eq!(fate("tmux"), (McpFate::Kept, None));
        assert_eq!(
            fate("local-db").0,
            McpFate::Lost,
            "an import stays on its machine"
        );
        assert!(fate("local-db").1.is_some());
        assert_eq!(
            fate("trial").0,
            McpFate::Lost,
            "an environment's own server goes nowhere else"
        );
        assert_eq!(
            fate("browser"),
            (
                McpFate::Unavailable,
                Some("a required capability is not granted to this environment".to_owned())
            )
        );
        assert_eq!(fate("search"), (McpFate::New, None));
        assert!(
            plan.iter()
                .any(|server| server.name == "local-db" && server.imported)
        );
        assert!(
            plan.iter()
                .any(|server| server.name == "trial" && server.own)
        );
    }
}
