//! Preset hosting, servers imported from a harness's own configuration and run from the connector's copy, servers an environment was given as its own, relay, tool filters and restart limits.
//!
//! This is the registry of the tool servers of an environment: which are known, where each came
//! from, whether it runs and where a tunnel reaches it, which of its tools are offered, and how
//! often it may crash before it is given up on. Starting a server and relaying a session's calls
//! to it are done elsewhere; what is here is what they need to know and the rules that need no
//! process to be stated.
//!
//! A server is one of three kinds. A **preset** is hosted for the environment from a definition
//! the system gives. An **imported** server is one a harness had configured on the machine: the
//! definition was read out through that harness's adapter and a copy of it is kept in a file of
//! the connector's data directory, readable by the connector's user alone; the system never has
//! the definition, only its name, its transport and a digest that changes when it does. An
//! environment's **own** server is one somebody added from Orch: on a person's machine that is a
//! command that runs as that person, so it takes the machine's permission for it.
//!
//! A crashing server is restarted, up to a limit in a window of time, and then reported failed.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use orch_agent_contract::discovery::{McpDefinition, McpFound, McpServerSpec};
use orch_link_contract::commands::{McpImport, McpImportResult};
use orch_link_contract::events::EnvironmentFactsMcp;
use orch_types::{Clock, Id};

use crate::error::{HostError, Result};

/// Where the copies of imported definitions are kept, below the connector's data directory.
const IMPORT_DIR: &str = "mcp";

/// The permission bits of a copy of a definition: it may hold secrets, and the connector's user is
/// the only one who reads it.
#[cfg(unix)]
const IMPORT_FILE_MODE: u32 = 0o600;

/// The name of the service a tunnel to a server names.
#[must_use]
pub fn service_of(name: &str) -> String {
    format!("mcp/{name}")
}

/// What kind of server this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Hosted for the environment from a preset the system gives.
    Preset,
    /// Added from Orch for this environment alone.
    Own,
    /// Imported from a harness's configuration on the machine; run from the connector's copy.
    Imported(Id),
}

/// Where a server stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerState {
    /// Being started.
    Starting,
    /// Running, and reached by the service named.
    Running {
        /// The service a tunnel names.
        service: String,
    },
    /// Given up on, for the reason given.
    Failed {
        /// Why.
        reason: String,
    },
    /// A requirement is missing, so no server process can be started.
    Unavailable {
        /// The missing requirement.
        reason: String,
    },
    /// Stopped on purpose.
    Stopped,
}

/// What to do after a server crashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restart {
    /// Start it again.
    Again,
    /// It crashed too often: report it failed.
    GiveUp,
}

/// Counts crashes in a window of time.
#[derive(Debug, Clone)]
pub struct RestartBudget {
    limit: u32,
    window: chrono::Duration,
    crashes: VecDeque<DateTime<Utc>>,
}

impl RestartBudget {
    /// A budget of `limit` restarts in `window`: the crash after the last of them gives up.
    #[must_use]
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window: chrono::Duration::from_std(window).unwrap_or(chrono::Duration::MAX),
            crashes: VecDeque::new(),
        }
    }

    /// Notes a crash at `now` and says what to do.
    pub fn crashed(&mut self, now: DateTime<Utc>) -> Restart {
        while self
            .crashes
            .front()
            .is_some_and(|first| now.signed_duration_since(*first) > self.window)
        {
            self.crashes.pop_front();
        }
        self.crashes.push_back(now);
        if self.crashes.len() as u64 > u64::from(self.limit) {
            Restart::GiveUp
        } else {
            Restart::Again
        }
    }

    /// Forgets every crash, as when somebody starts the server again on purpose.
    pub fn reset(&mut self) {
        self.crashes.clear();
    }
}

/// A server of the environment.
#[derive(Debug, Clone)]
pub struct Server {
    /// Stable Orch membership, separate from a reusable local name.
    pub server_id: Option<Id>,
    /// Starts of this one server, used to refresh only offered tools after a restart.
    pub revision: u64,
    /// The name it is offered under.
    pub name: String,
    /// Where it came from.
    pub origin: Origin,
    /// How it is started and which of its tools are offered; its secrets are not here.
    pub spec: McpServerSpec,
    /// Where it stands.
    pub state: ServerState,
    budget: RestartBudget,
}

/// What a machine's owner has switched on for tool servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpGates {
    /// Whether a server may be imported from a harness's configuration on the machine.
    pub import: bool,
    /// Whether a server may be added from Orch.
    pub add: bool,
}

/// The tool servers of one environment.
pub struct McpServers {
    clock: Arc<dyn Clock>,
    data_dir: PathBuf,
    limit: u32,
    window: Duration,
    gates: Mutex<McpGates>,
    servers: Mutex<BTreeMap<String, Server>>,
    found: Mutex<Vec<McpFound>>,
}

impl std::fmt::Debug for McpServers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServers")
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

/// Whether a tool is offered, given the lists a definition names: when there is an allow list, only
/// the tools on it; and never one on the deny list.
#[must_use]
pub fn tool_offered(spec: &McpServerSpec, tool: &str) -> bool {
    let allowed = spec
        .tools_allow
        .as_ref()
        .is_none_or(|list| list.iter().any(|name| name == tool));
    let denied = spec
        .tools_deny
        .as_ref()
        .is_some_and(|list| list.iter().any(|name| name == tool));
    allowed && !denied
}

impl McpServers {
    /// The registry of an environment whose connector keeps its files in `data_dir`. A server may
    /// crash `limit` times in `window` and be restarted each time.
    #[must_use]
    pub fn new(
        clock: Arc<dyn Clock>,
        data_dir: PathBuf,
        limit: u32,
        window: Duration,
        gates: McpGates,
    ) -> Self {
        Self {
            clock,
            data_dir,
            limit,
            window,
            gates: Mutex::new(gates),
            servers: Mutex::new(BTreeMap::new()),
            found: Mutex::new(Vec::new()),
        }
    }

    fn servers(&self) -> MutexGuard<'_, BTreeMap<String, Server>> {
        self.servers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces what the machine's owner has switched on.
    pub fn set_gates(&self, gates: McpGates) {
        *self.gates.lock().unwrap_or_else(PoisonError::into_inner) = gates;
    }

    fn gates(&self) -> McpGates {
        *self.gates.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the machine lets the tool servers its harnesses have configured be listed and imported.
    #[must_use]
    pub fn import_allowed(&self) -> bool {
        self.gates().import
    }

    // ------------------------------------------------------------------ the servers

    /// Notes a server that is being started. A name is used once; a server of the environment's
    /// own kind takes the machine's permission to add one.
    pub fn add(&self, name: &str, origin: Origin, spec: McpServerSpec) -> Result<()> {
        if origin == Origin::Own && !self.gates().add {
            return Err(HostError::McpAddDisabled);
        }
        let mut servers = self.servers();
        if let Some(there) = servers.get(name)
            && !matches!(
                there.state,
                ServerState::Stopped | ServerState::Failed { .. } | ServerState::Unavailable { .. }
            )
        {
            return Err(HostError::Exists(format!(
                "a server called {name:?} is there already"
            )));
        }
        let server_id = match &origin {
            Origin::Imported(id) => Some(*id),
            _ => None,
        };
        servers.insert(
            name.to_owned(),
            Server {
                server_id,
                revision: 0,
                name: name.to_owned(),
                origin,
                spec,
                state: ServerState::Starting,
                budget: RestartBudget::new(self.limit, self.window),
            },
        );
        Ok(())
    }

    /// Retains the controller's stable identity after a given definition is accepted.
    pub fn identify(&self, name: &str, id: Option<Id>) -> Result<()> {
        let mut servers = self.servers();
        let server = servers
            .get_mut(name)
            .ok_or_else(|| HostError::NotFound(format!("there is no server {name:?}")))?;
        server.server_id = id;
        Ok(())
    }

    /// Notes that the server is running and reached by its service.
    pub fn started(&self, name: &str) -> Result<String> {
        let mut servers = self.servers();
        let server = servers
            .get_mut(name)
            .ok_or_else(|| HostError::NotFound(format!("there is no server {name:?}")))?;
        let service = service_of(name);
        server.revision = server.revision.saturating_add(1);
        server.state = ServerState::Running {
            service: service.clone(),
        };
        Ok(service)
    }

    /// Notes that the server crashed, and says whether to start it again. After the limit it is
    /// reported failed.
    pub fn crashed(&self, name: &str, reason: &str) -> Result<Restart> {
        let now = self.clock.now();
        let mut servers = self.servers();
        let server = servers
            .get_mut(name)
            .ok_or_else(|| HostError::NotFound(format!("there is no server {name:?}")))?;
        let decision = server.budget.crashed(now);
        server.state = match decision {
            Restart::Again => ServerState::Starting,
            Restart::GiveUp => ServerState::Failed {
                reason: format!("it crashed too often: {reason}"),
            },
        };
        Ok(decision)
    }

    /// Notes that the server could not be started, or was given up on, for `reason`.
    pub fn failed(&self, name: &str, reason: &str) {
        if let Some(server) = self.servers().get_mut(name) {
            server.state = ServerState::Failed {
                reason: reason.to_owned(),
            };
        }
    }

    /// Keeps a missing startup requirement apart from a crashed or failed process.
    pub fn unavailable(&self, name: &str, reason: &str) {
        if let Some(server) = self.servers().get_mut(name) {
            server.state = ServerState::Unavailable {
                reason: reason.to_owned(),
            };
        }
    }

    /// Notes that the server was stopped on purpose.
    /// Restarts the same authorized definition without adding a new server or asking an add/import gate again.
    pub fn restart(&self, name: &str) -> Result<()> {
        let mut servers = self.servers();
        let server = servers
            .get_mut(name)
            .ok_or_else(|| HostError::NotFound(format!("there is no server {name:?}")))?;
        server.budget.reset();
        server.state = ServerState::Starting;
        Ok(())
    }

    /// Notes that the server was stopped on purpose.
    pub fn stopped(&self, name: &str) -> Result<()> {
        let mut servers = self.servers();
        let server = servers
            .get_mut(name)
            .ok_or_else(|| HostError::NotFound(format!("there is no server {name:?}")))?;
        server.state = ServerState::Stopped;
        server.budget.reset();
        Ok(())
    }

    /// Forgets the server altogether.
    pub fn remove(&self, name: &str) {
        self.servers().remove(name);
    }

    /// The server called `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Server> {
        self.servers().get(name).cloned()
    }

    /// Every server.
    #[must_use]
    pub fn list(&self) -> Vec<Server> {
        self.servers().values().cloned().collect()
    }

    /// Whether at least one server is enabled (running or being started): a tool set made of the
    /// servers alone is offered only then.
    #[must_use]
    pub fn any_enabled(&self) -> bool {
        self.servers().values().any(|server| {
            matches!(
                server.state,
                ServerState::Running { .. } | ServerState::Starting
            )
        })
    }

    /// Whether the tool `tool` of the server `name` is offered.
    #[must_use]
    pub fn offers_tool(&self, name: &str, tool: &str) -> bool {
        self.servers()
            .get(name)
            .is_some_and(|server| tool_offered(&server.spec, tool))
    }

    // ------------------------------------------------------------------ what harnesses have configured

    /// Remembers what the adapters found configured on the machine. Only names and shapes are
    /// kept, never a definition.
    pub fn set_found(&self, found: Vec<McpFound>) {
        *self.found.lock().unwrap_or_else(PoisonError::into_inner) = found;
    }

    /// What is found, as the environment's facts say it: where each is configured and how it is
    /// reached, and nothing that says how it is started.
    #[must_use]
    pub fn found(&self) -> Vec<EnvironmentFactsMcp> {
        self.found
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|found| EnvironmentFactsMcp {
                name: found.name.clone(),
                source: found.source.clone(),
                scope: wire_word(&found.scope),
                directory: found.directory.clone(),
                transport: wire_word(&found.transport),
                digest: found.digest.clone(),
            })
            .collect()
    }

    // ------------------------------------------------------------------ copies of imported definitions

    fn copy_path(&self, server_id: Id) -> PathBuf {
        copy_path_in(&self.data_dir, server_id)
    }

    /// Keeps a copy of `definition`, which an adapter read out of a harness's configuration, under
    /// the id the system gave the server. The answer says how the server is reached and a digest of
    /// the definition, and nothing of the definition itself.
    pub fn import(
        &self,
        request: &McpImport,
        definition: &McpDefinition,
        tools: Option<i64>,
    ) -> Result<McpImportResult> {
        if !self.gates().import {
            return Err(HostError::McpImportDisabled);
        }
        definition.check().map_err(|faults| {
            HostError::Unsupported(format!("that definition is not complete: {faults:?}"))
        })?;
        if request
            .expected_fingerprint
            .is_some_and(|expected| definition.definition_fingerprint() != expected)
        {
            return Err(HostError::Stale { current: None });
        }
        keep_copy(&self.data_dir, request.server_id, definition, tools)
    }

    /// The definition kept under `server_id`, to start the server from.
    pub fn load(&self, server_id: Id) -> Result<McpDefinition> {
        let text = std::fs::read(self.copy_path(server_id)).map_err(|error| {
            HostError::io(format!("reading the copy of server {server_id}"), error)
        })?;
        serde_json::from_slice(&text).map_err(|error| {
            HostError::Internal(format!(
                "the copy of server {server_id} is damaged: {error}"
            ))
        })
    }

    /// Deletes the copy kept under `server_id`.
    pub fn forget(&self, server_id: Id) -> Result<()> {
        forget_copy(&self.data_dir, server_id)
    }

    /// Whether the copy kept under `server_id` is the definition with this digest: a server whose
    /// harness configuration has changed since differs from its copy.
    #[must_use]
    pub fn copy_has_digest(&self, server_id: Id, digest: &str) -> bool {
        self.load(server_id)
            .is_ok_and(|definition| definition.digest().to_string() == digest)
    }
}

fn keep_copy(
    data_dir: &std::path::Path,
    server_id: Id,
    definition: &McpDefinition,
    tools: Option<i64>,
) -> Result<McpImportResult> {
    let path = copy_path_in(data_dir, server_id);
    let dir = path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    std::fs::create_dir_all(&dir)
        .map_err(|error| HostError::io("making the directory of the copies", error))?;
    let text = serde_json::to_vec(definition)?;
    let temp = dir.join(format!(".{}.tmp", orch_types::new_id().simple()));
    write_private(&temp, &text)
        .map_err(|error| HostError::io("keeping a copy of a definition", error))?;
    std::fs::rename(&temp, &path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        HostError::io("keeping a copy of a definition", error)
    })?;
    Ok(result_of(definition, tools))
}
/// A managed stored snapshot has a source UUID and exact digest, never a replacement discovery result.
pub(crate) fn copy_stored(
    data_dir: &std::path::Path,
    server_id: Id,
    source: &orch_link_contract::commands::McpStoredCopy,
) -> Result<McpImportResult> {
    if server_id == source.server_id {
        return Err(HostError::Unsupported(
            "a duplicate needs a new server identity".into(),
        ));
    }
    let definition = stored_snapshot(data_dir, source)?;
    keep_snapshot(data_dir, server_id, &definition)
}
/// Native metadata of this exact definition, with no command, address or private values.
pub(crate) fn result_of(definition: &McpDefinition, tools: Option<i64>) -> McpImportResult {
    let imported = definition.imported(tools);
    McpImportResult {
        snapshot: None,
        definition_fingerprint: imported.definition_fingerprint,
        transport: wire_word(&imported.transport),
        digest: imported.digest,
        tools: imported.tools,
    }
}
// Interpolation defaults and standard URL userinfo are credential values too. Preserve
// placeholder names/configuration while leaving no private fallback literal in a nonowner file.
fn without_url_credentials(value: &str, interpolate: bool) -> String {
    let mut rendered = String::new();
    let mut rest = value;
    if interpolate {
        while let Some(start) = rest.find("${") {
            rendered.push_str(&rest[..start]);
            let tail = &rest[start + 2..];
            let Some(end) = tail.find('}') else {
                rendered.push_str(&rest[start..]);
                rest = "";
                break;
            };
            let name = tail[..end]
                .split_once(":-")
                .map_or(&tail[..end], |(name, _)| name);
            rendered.push_str("${");
            rendered.push_str(name);
            rendered.push('}');
            rest = &tail[end + 1..];
        }
    }
    rendered.push_str(rest);
    if let Some(scheme) = rendered.find("://")
        && (rendered[..scheme].eq_ignore_ascii_case("http")
            || rendered[..scheme].eq_ignore_ascii_case("https"))
    {
        let start = scheme + 3;
        let end = rendered[start..]
            .find(['/', '?', '#'])
            .map_or(rendered.len(), |end| start + end);
        if let Some(userinfo) = rendered[start..end].rfind('@') {
            rendered.replace_range(start..start + userinfo + 1, "");
        }
    }
    rendered
}
/// The same original managed snapshot read and redaction for local and sealed machine copies.
pub(crate) fn stored_snapshot(
    data_dir: &std::path::Path,
    source: &orch_link_contract::commands::McpStoredCopy,
) -> Result<McpDefinition> {
    let bytes = std::fs::read(copy_path_in(data_dir, source.server_id))
        .map_err(|error| HostError::io("reading the exact stored source definition", error))?;
    let mut definition: McpDefinition = serde_json::from_slice(&bytes)?;
    if definition.digest().to_string() != source.expected_digest {
        return Err(HostError::Stale { current: None });
    }
    definition.check().map_err(|faults| {
        HostError::Unsupported(format!("the stored source is incomplete: {faults:?}"))
    })?;
    if !source.include_secrets {
        for value in definition.secrets.values_mut() {
            *value = orch_plugin_contract::Secret::new(String::new());
        }
        if let Some(http) = &mut definition.http_credentials {
            if http.interpolate {
                let mut names = Vec::new();
                for text in http
                    .headers
                    .values()
                    .map(orch_plugin_contract::Secret::expose)
                    .chain(definition.spec.url.iter().map(String::as_str))
                {
                    let mut rest = text;
                    while let Some(start) = rest.find("${") {
                        let tail = &rest[start + 2..];
                        let Some(end) = tail.find('}') else {
                            break;
                        };
                        names.push(
                            tail[..end]
                                .split_once(":-")
                                .map_or(&tail[..end], |(name, _)| name)
                                .to_owned(),
                        );
                        rest = &tail[end + 1..];
                    }
                }
                http.blocked_env.extend(names);
            }
            for value in http.headers.values_mut() {
                *value = orch_plugin_contract::Secret::new(String::new());
            }
            http.blocked_env.extend(http.env_headers.values().cloned());
            http.blocked_env
                .extend(http.bearer_token_env_var.iter().cloned());
            http.blocked_env
                .extend(definition.spec.secret_env.iter().cloned());
            http.blocked_env.sort();
            http.blocked_env.dedup();
        }
    }
    if !source.include_secrets {
        let interpolate = definition
            .http_credentials
            .as_ref()
            .is_some_and(|http| http.interpolate);
        if let Some(url) = &mut definition.spec.url {
            *url = without_url_credentials(url, interpolate);
        }
    }
    Ok(definition)
}
/// A new copy may replay its own exact file, but never overwrite a different existing snapshot.
pub(crate) fn keep_snapshot(
    data_dir: &std::path::Path,
    server_id: Id,
    definition: &McpDefinition,
) -> Result<McpImportResult> {
    definition.check().map_err(|faults| {
        HostError::Unsupported(format!("the copied definition is incomplete: {faults:?}"))
    })?;
    match std::fs::read(copy_path_in(data_dir, server_id)) {
        Ok(bytes) => {
            let existing: McpDefinition = serde_json::from_slice(&bytes)?;
            if existing != *definition {
                return Err(HostError::Exists(
                    "a different native copy already occupies this identity".into(),
                ));
            }
            Ok(result_of(&existing, None))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            keep_copy(data_dir, server_id, definition, None)
        }
        Err(error) => Err(HostError::io("reading an existing native copy", error)),
    }
}

/// Where the copy of the definition of `server_id` is kept below the connector's data directory.
fn copy_path_in(data_dir: &std::path::Path, server_id: Id) -> PathBuf {
    data_dir.join(IMPORT_DIR).join(format!("{server_id}.json"))
}

/// Deletes the copy of the definition of `server_id` kept below `data_dir`, which does not belong
/// to one environment: a copy is the machine's. A copy that is not there is not an error.
pub fn forget_copy(data_dir: &std::path::Path, server_id: Id) -> Result<()> {
    match std::fs::remove_file(copy_path_in(data_dir, server_id)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(HostError::io("deleting the copy of a definition", error)),
    }
}

/// A value as the wire writes it, as a bare word.
fn wire_word<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(IMPORT_FILE_MODE)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Windows has no permission bits to restrict: the connector's data directory lies in the user's
/// own profile, whose access list already keeps other accounts out.
#[cfg(not(unix))]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::discovery::{McpRequires, McpTransport, McpWorkingDir};
    use orch_types::ManualClock;

    fn spec(allow: Option<&[&str]>, deny: Option<&[&str]>) -> McpServerSpec {
        McpServerSpec {
            transport: McpTransport::Stdio,
            command: Some(vec!["github-mcp".into(), "--stdio".into()]),
            url: None,
            authentication: None,
            env: BTreeMap::new(),
            secret_env: vec![],
            working_dir: McpWorkingDir::Workspace,
            startup_timeout_secs: 30,
            tools_allow: allow.map(|l| l.iter().map(|s| (*s).to_owned()).collect()),
            tools_deny: deny.map(|l| l.iter().map(|s| (*s).to_owned()).collect()),
            requires: McpRequires::default(),
        }
    }

    fn registry(dir: &std::path::Path, gates: McpGates) -> (McpServers, ManualClock) {
        let clock = ManualClock::new(DateTime::from_timestamp(1_000_000, 0).unwrap());
        (
            McpServers::new(
                Arc::new(clock.clone()),
                dir.to_path_buf(),
                3,
                Duration::from_secs(600),
                gates,
            ),
            clock,
        )
    }

    const ALL: McpGates = McpGates {
        import: true,
        add: true,
    };

    #[test]
    fn a_crashing_server_is_restarted_three_times_in_ten_minutes_and_then_reported_failed() {
        let dir = tempfile::tempdir().unwrap();
        let (servers, clock) = registry(dir.path(), ALL);
        servers
            .add("github", Origin::Preset, spec(None, None))
            .unwrap();
        servers.started("github").unwrap();
        for _ in 0..3 {
            assert_eq!(servers.crashed("github", "exit 1").unwrap(), Restart::Again);
            clock.advance(chrono::Duration::seconds(30));
        }
        assert_eq!(
            servers.crashed("github", "exit 1").unwrap(),
            Restart::GiveUp,
            "the fourth crash in the window"
        );
        match servers.get("github").unwrap().state {
            ServerState::Failed { reason } => assert!(reason.contains("exit 1")),
            other => panic!("{other:?}"),
        }
        // Crashes that are long past do not count.
        servers.stopped("github").unwrap();
        servers
            .add("github", Origin::Preset, spec(None, None))
            .unwrap();
        assert_eq!(servers.crashed("github", "x").unwrap(), Restart::Again);
        clock.advance(chrono::Duration::seconds(601));
        for _ in 0..3 {
            assert_eq!(servers.crashed("github", "x").unwrap(), Restart::Again);
        }
    }

    #[test]
    fn a_name_is_used_by_one_running_server_and_the_tools_offered_follow_the_definitions_lists() {
        let dir = tempfile::tempdir().unwrap();
        let (servers, _) = registry(dir.path(), ALL);
        servers
            .add(
                "files",
                Origin::Own,
                spec(Some(&["read", "list"]), Some(&["list"])),
            )
            .unwrap();
        assert!(servers.add("files", Origin::Own, spec(None, None)).is_err());
        assert!(servers.any_enabled(), "being started counts");
        assert!(servers.offers_tool("files", "read"));
        assert!(!servers.offers_tool("files", "list"), "the deny list wins");
        assert!(
            !servers.offers_tool("files", "write"),
            "only what the allow list names"
        );
        assert!(!servers.offers_tool("nobody", "read"));
        assert_eq!(servers.started("files").unwrap(), "mcp/files");
        servers.stopped("files").unwrap();
        assert!(!servers.any_enabled());
        servers.add("files", Origin::Own, spec(None, None)).unwrap();
        servers.remove("files");
        assert!(servers.get("files").is_none());
    }

    #[test]
    fn a_server_added_from_orch_takes_the_machines_permission_and_a_preset_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let (servers, _) = registry(
            dir.path(),
            McpGates {
                import: true,
                add: false,
            },
        );
        assert!(matches!(
            servers.add("own", Origin::Own, spec(None, None)),
            Err(HostError::McpAddDisabled)
        ));
        assert!(
            servers
                .add("preset", Origin::Preset, spec(None, None))
                .is_ok()
        );
        servers.set_gates(ALL);
        assert!(servers.add("own", Origin::Own, spec(None, None)).is_ok());
    }

    fn definition(secret: &str) -> McpDefinition {
        serde_json::from_value(serde_json::json!({
            "spec": { "transport": "stdio", "command": ["github-mcp"], "secret_env": ["GITHUB_TOKEN"] },
            "secrets": { "GITHUB_TOKEN": secret }
        }))
        .unwrap()
    }

    fn request(server_id: Id) -> McpImport {
        McpImport {
            expected_fingerprint: None,
            stored: None,
            transfer: None,
            environment_id: Some(orch_types::new_id()),
            server_id,
            source: "codex".into(),
            scope: "user".into(),
            name: "github".into(),
            directory: None,
        }
    }

    #[test]
    fn a_different_definition_is_refused_before_creating_or_replacing_a_protected_copy() {
        let dir = tempfile::tempdir().unwrap();
        let (servers, _) = registry(dir.path(), ALL);
        let id = orch_types::new_id();
        let original = definition("first-secret");
        let mut different = definition("second-secret");
        different.spec.command = Some(vec!["different-server".into()]);
        let mut guarded = request(id);
        guarded.expected_fingerprint = Some(original.definition_fingerprint());
        assert!(matches!(
            servers.import(&guarded, &different, None),
            Err(HostError::Stale { .. })
        ));
        assert!(
            servers.load(id).is_err(),
            "merely finding a different definition creates no protected import"
        );
        servers.import(&request(id), &original, None).unwrap();
        assert!(matches!(
            servers.import(&guarded, &different, None),
            Err(HostError::Stale { .. })
        ));
        assert_eq!(
            servers.load(id).unwrap().definition_fingerprint(),
            original.definition_fingerprint(),
            "a failed observation never replaces the imported thing"
        );
        assert!(
            servers
                .import(&guarded, &definition("rotated-secret"), None)
                .is_ok(),
            "variable and secret values are excluded from structural identity"
        );
    }

    #[test]
    fn an_imported_definition_is_kept_where_only_the_connector_reads_it_and_the_system_gets_only_its_shape()
     {
        let dir = tempfile::tempdir().unwrap();
        let (servers, _) = registry(dir.path(), ALL);
        let id = orch_types::new_id();
        let answer = servers
            .import(&request(id), &definition("ghp_very_secret"), Some(12))
            .unwrap();
        assert_eq!(
            (answer.transport.as_str(), answer.tools),
            ("stdio", Some(12))
        );
        let wire = serde_json::to_string(&answer).unwrap();
        assert!(
            !wire.contains("ghp_very_secret") && !wire.contains("github-mcp"),
            "the answer says nothing of the definition: {wire}"
        );

        let path = dir.path().join("mcp").join(format!("{id}.json"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(servers.load(id).unwrap(), definition("ghp_very_secret"));
        assert!(servers.copy_has_digest(id, &answer.digest));
        // The harness's own file changes and is imported again: the digest changes with the definition.
        let again = servers
            .import(&request(id), &definition("another"), None)
            .unwrap();
        assert_ne!(again.digest, answer.digest);
        assert!(
            !servers.copy_has_digest(id, &answer.digest),
            "the server's page can say that it differs"
        );
        servers.forget(id).unwrap();
        assert!(!path.exists());
        servers.forget(id).unwrap();
        assert!(servers.load(id).is_err());
    }

    #[test]
    fn nothing_is_imported_where_the_machine_lets_none_be_and_an_incomplete_definition_is_refused()
    {
        let dir = tempfile::tempdir().unwrap();
        let (servers, _) = registry(
            dir.path(),
            McpGates {
                import: false,
                add: true,
            },
        );
        let id = orch_types::new_id();
        let refused = servers
            .import(&request(id), &definition("s"), None)
            .unwrap_err();
        assert_eq!(refused.command_error().code, "policy.mcp_import_disabled");
        assert!(!dir.path().join("mcp").exists());
        servers.set_gates(ALL);
        let mut broken = definition("s");
        broken.spec.command = None;
        assert!(servers.import(&request(id), &broken, None).is_err());
    }

    #[test]
    fn what_harnesses_configured_is_reported_by_name_and_place_and_never_how_it_is_started() {
        let dir = tempfile::tempdir().unwrap();
        let (servers, _) = registry(dir.path(), ALL);
        servers.set_found(vec![serde_json::from_value(serde_json::json!({
            "name": "github", "source": "codex", "scope": "directory", "directory": "web", "transport": "http", "digest": "d1"
        }))
        .unwrap()]);
        let found = servers.found();
        assert_eq!(found.len(), 1);
        assert_eq!(
            (
                found[0].scope.as_str(),
                found[0].transport.as_str(),
                found[0].directory.as_deref()
            ),
            ("directory", "http", Some("web"))
        );
        let wire = serde_json::to_string(&found).unwrap();
        assert!(
            !wire.contains("command") && !wire.contains("url") && !wire.contains("secret"),
            "{wire}"
        );
    }
    #[test]
    fn a_stored_snapshot_copy_keeps_its_original_and_never_reselects_a_changed_source() {
        let dir = tempfile::tempdir().unwrap();
        let (servers, _) = registry(dir.path(), ALL);
        let original_id = orch_types::new_id();
        let original = definition("synthetic original credential");
        let first = servers
            .import(&request(original_id), &original, None)
            .unwrap();
        let copy_id = orch_types::new_id();
        let source = orch_link_contract::commands::McpStoredCopy {
            server_id: original_id,
            expected_digest: first.digest.clone(),
            include_secrets: true,
        };
        let copied = copy_stored(dir.path(), copy_id, &source).unwrap();
        assert_eq!(copied.digest, first.digest);
        assert_eq!(servers.load(copy_id).unwrap(), original);
        assert_eq!(servers.load(original_id).unwrap(), original);
        let changed = definition("a later credential");
        servers
            .import(&request(original_id), &changed, None)
            .unwrap();
        assert!(matches!(
            copy_stored(dir.path(), copy_id, &source),
            Err(HostError::Stale { .. })
        ));
        assert_eq!(
            servers.load(copy_id).unwrap(),
            original,
            "a stale snapshot cannot replace an accepted copy"
        );
        assert!(copy_stored(dir.path(), original_id, &source).is_err());
    }

    #[test]
    fn a_nonowner_snapshot_copy_empties_declared_and_native_http_values_and_blocks_machine_variable_fallbacks()
     {
        let dir = tempfile::tempdir().unwrap();
        let (servers, _) = registry(dir.path(), ALL);
        let original_id = orch_types::new_id();
        let original:McpDefinition=serde_json::from_value(serde_json::json!({
            "spec":{"transport":"http","url":"https://synthetic-user:synthetic-password@mcp.example.test/${URL_TOKEN:-synthetic-default}","secret_env":["AUTH"],"authentication":{"kind":"bearer","secret":"AUTH"}},
            "secrets":{"AUTH":"synthetic bearer"},
            "http_credentials":{"headers":{"Authorization":"synthetic header","X-Variable":"${MACHINE_TOKEN:-synthetic-fallback}"},"env_headers":{"X-Tenant":"TENANT_TOKEN"},"bearer_token_env_var":"ACCESS_TOKEN","interpolate":true}
        })).unwrap();
        let first = servers
            .import(&request(original_id), &original, None)
            .unwrap();
        let copy_id = orch_types::new_id();
        let reply = copy_stored(
            dir.path(),
            copy_id,
            &orch_link_contract::commands::McpStoredCopy {
                server_id: original_id,
                expected_digest: first.digest,
                include_secrets: false,
            },
        )
        .unwrap();
        let copied = servers.load(copy_id).unwrap();
        copied.check().unwrap();
        assert!(
            copied
                .secrets
                .values()
                .all(|value| value.expose().is_empty())
        );
        let http = copied.http_credentials.as_ref().unwrap();
        assert!(http.headers.values().all(|value| value.expose().is_empty()));
        for name in [
            "AUTH",
            "URL_TOKEN",
            "MACHINE_TOKEN",
            "TENANT_TOKEN",
            "ACCESS_TOKEN",
        ] {
            assert!(
                http.blocked_env.iter().any(|blocked| blocked == name),
                "{name}"
            );
        }
        let private_copy = serde_json::to_string(&copied).unwrap();
        for value in [
            "synthetic-default",
            "synthetic-password",
            "synthetic-user",
            "synthetic-fallback",
            "synthetic bearer",
            "synthetic header",
        ] {
            assert!(
                !private_copy.contains(value),
                "a nonowner's native file cannot retain a credential fallback: {value}"
            );
        }
        assert_eq!(servers.load(original_id).unwrap(), original);
        assert_ne!(reply.digest, original.digest().to_string());
        let public = serde_json::to_string(&reply).unwrap();
        for value in ["synthetic bearer", "synthetic header", "synthetic-fallback"] {
            assert!(!public.contains(value));
        }
    }
}
