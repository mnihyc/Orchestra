//! Executor availability and lifecycle; what the exec node offers, in its order, and the tool set a session gets from it.
//!
//! The exec node of an environment is the connector in its environment role. It knows the
//! executors it can run (each one declares itself) and, for the environment it serves, which of
//! them can start there and which tool sets they can offer. This module keeps that registry:
//! which executors are known, which are running and where they are reached, the variables
//! through which what they start carries its thread and tool call, and the list of **offers**.
//!
//! An offer is a name: an executor kind, or the name of a tool set of an executor that has
//! several. The environment lists the offers it makes, in the order in which to try them; the
//! machine's owner may allow fewer; and an offer is made only while the executor can start there.
//! Which offer a thread takes is not decided here: the first one its harness can work through,
//! each time a session opens, which is the system's to work out from this list.
//!
//! Starting and stopping an executor is the executor's own business, behind the trait of the
//! executor contract; the host only asks, checks the machine's policy first, and remembers.

use std::collections::{BTreeMap, BTreeSet};

use orch_executor_contract::context::EnvironmentContext;
use orch_executor_contract::descriptor::{
    Attribution, ExecutorAvailability, ExecutorDescriptor, ExecutorProtocol,
};
use orch_executor_contract::executor::{Executor, ExecutorEndpoint};
use orch_link_contract::commands::ExecStartResult;
use orch_link_contract::events::EnvironmentFactsExecutor;
use orch_link_contract::tunnels::Service;
use tokio::sync::Mutex;

use crate::error::{HostError, Result};

/// What an offer comes to: the executor that makes it and, for an executor with several, the tool set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// The name the offer is listed under.
    pub name: String,
    /// The executor that carries it out.
    pub executor: String,
    /// The tool set, when the executor has several.
    pub tool_set: Option<String>,
}

struct Entry {
    descriptor: ExecutorDescriptor,
    executor: Box<dyn Executor>,
    running: Option<ExecutorEndpoint>,
    local_program: Option<String>,
}

impl Entry {
    fn selected_program<'a>(&self, context: &'a EnvironmentContext) -> Option<&'a str> {
        if context.tools_dir.is_some() || self.descriptor.protocol != ExecutorProtocol::Harness {
            return None;
        }
        context
            .local_programs
            .get(self.descriptor.program.as_ref()?)
            .map(String::as_str)
    }

    async fn retire_changed_program(
        &mut self,
        context: &EnvironmentContext,
        available: &ExecutorAvailability,
    ) -> std::result::Result<bool, orch_agent_contract::adapter::AdapterError> {
        let changed = self.local_program.is_some()
            && self.running.as_ref().is_some_and(|running| {
                self.selected_program(context) != self.local_program.as_deref()
                    || !available.available
                    || available.version.as_deref() != Some(running.version.as_str())
            });
        if changed {
            self.running = None;
            self.local_program = None;
            self.executor.stop().await?;
        }
        Ok(changed)
    }
}

async fn start_entry(
    kind: &str,
    entry: &mut Entry,
    allowed: Option<&[String]>,
    context: &EnvironmentContext,
    required_version: Option<&str>,
) -> Result<ExecStartResult> {
    if let Some(list) = allowed {
        let offered = list.iter().any(|name| {
            name == kind
                || tool_set_name(name, kind).is_some_and(|set| entry.descriptor.has_tool_set(set))
        });
        if !offered {
            return Err(HostError::ExecNotOffered(format!(
                "this machine's owner does not let its exec node offer {kind:?}"
            )));
        }
    }
    let availability = entry.executor.available(context).await;
    entry
        .retire_changed_program(context, &availability)
        .await
        .map_err(|error| {
            HostError::Internal(format!(
                "{kind:?} did not retire its previous program: {error}"
            ))
        })?;
    if !availability.available {
        return Err(HostError::Unsupported(format!(
            "{kind:?} cannot start here: {}",
            availability
                .reason
                .unwrap_or_else(|| "no reason was given".to_owned())
        )));
    }
    let local_program = entry.selected_program(context).map(str::to_owned);
    if local_program.is_some()
        && required_version.is_some_and(|version| availability.version.as_deref() != Some(version))
    {
        return Err(HostError::Unsupported(
            "the confirmed program is not the executor build admitted for these tools".into(),
        ));
    }
    let endpoint = match entry.executor.start(context).await {
        Ok(endpoint) => endpoint,
        Err(error) => {
            if entry.local_program.take().is_some() {
                entry.running = None;
            }
            return Err(HostError::Internal(format!(
                "{kind:?} did not start: {error}"
            )));
        }
    };
    endpoint.port().map_err(|error| {
        HostError::Internal(format!(
            "{kind:?} gave an address that is not its own: {error}"
        ))
    })?;
    if local_program.is_some()
        && required_version.is_some_and(|version| endpoint.version != version)
    {
        let _ = entry.executor.stop().await;
        entry.running = None;
        entry.local_program = None;
        return Err(HostError::Unsupported(
            "the executor started at a different build than these tools admit".into(),
        ));
    }
    let result = ExecStartResult {
        service: service_of(kind, entry.descriptor.protocol),
        version: endpoint.version.clone(),
        tool_sets: if endpoint.tool_sets.is_empty() {
            entry
                .descriptor
                .tool_sets
                .iter()
                .map(|set| set.name.clone())
                .collect()
        } else {
            endpoint.tool_sets.clone()
        },
    };
    entry.running = Some(endpoint);
    entry.local_program = local_program;
    Ok(result)
}

fn tool_set_name<'a>(offer: &'a str, executor: &str) -> Option<&'a str> {
    match offer.split_once(':') {
        Some((owner, name)) => (owner == executor).then_some(name),
        None => Some(offer),
    }
}

/// The executors a connector can run.
#[derive(Default)]
pub struct Executors {
    entries: Mutex<BTreeMap<String, Entry>>,
}

impl std::fmt::Debug for Executors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executors").finish_non_exhaustive()
    }
}

/// The name an executor that offers its tools as an MCP server gives the server: Orch's own is `exec`.
pub const MCP_SERVER_NAME: &str = "exec";

/// The name of the service a tunnel to an executor names: an executor that carries a protocol of its
/// harness's own is reached by an `exec` tunnel, one that offers its tools as an MCP server by an `mcp`
/// tunnel to the server that is called `exec`.
#[must_use]
pub fn service_of(kind: &str, protocol: ExecutorProtocol) -> String {
    match protocol {
        ExecutorProtocol::Harness => Service::Exec {
            executor: kind.to_owned(),
        }
        .to_string(),
        ExecutorProtocol::Mcp => Service::Mcp {
            server: MCP_SERVER_NAME.to_owned(),
        }
        .to_string(),
    }
}

impl Executors {
    /// A registry with no executor in it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes an executor known. Its declaration is checked, and a kind is registered once.
    pub async fn register(&self, executor: Box<dyn Executor>) -> Result<()> {
        let descriptor = executor.describe();
        descriptor.check().map_err(|faults| {
            HostError::Unsupported(format!(
                "the declaration of executor {:?} is not consistent: {faults:?}",
                descriptor.kind
            ))
        })?;
        let mut entries = self.entries.lock().await;
        if entries.contains_key(&descriptor.kind) {
            return Err(HostError::Exists(format!(
                "executor {:?} is registered already",
                descriptor.kind
            )));
        }
        entries.insert(
            descriptor.kind.clone(),
            Entry {
                descriptor,
                executor,
                running: None,
                local_program: None,
            },
        );
        Ok(())
    }

    /// The variables through which the commands the known executors start carry their ids.
    pub async fn attributions(&self) -> Vec<Attribution> {
        self.entries
            .lock()
            .await
            .values()
            .filter_map(|entry| entry.descriptor.attribution.clone())
            .collect()
    }

    /// What each executor reports of itself in this environment, as the environment's facts say it.
    pub async fn facts(&self, context: &EnvironmentContext) -> Vec<EnvironmentFactsExecutor> {
        let mut entries = self.entries.lock().await;
        let mut facts = Vec::new();
        // An executor is `Send` and not `Sync`, so it is looked at through a unique borrow.
        for (kind, entry) in entries.iter_mut() {
            let availability = entry.executor.available(context).await;
            if let Err(error) = entry.retire_changed_program(context, &availability).await {
                tracing::warn!(%kind, %error, "an executor of a changed program could not be stopped");
            }
            let mut fact = availability.clone().into_fact(kind);
            // A tool set is offered only if it is one the executor declared.
            if let Some(sets) = &mut fact.tool_sets {
                sets.retain(|name| entry.descriptor.has_tool_set(name));
            }
            if fact.version.is_empty()
                && let Some(running) = &entry.running
            {
                fact.version.clone_from(&running.version);
            }
            facts.push(to_link_fact(fact));
        }
        facts
    }

    /// The offers the environment lists, in its order, kept to those the machine allows (`allowed`
    /// is its policy's list, or `None` for no limit) and to those that can be had here now.
    pub async fn offers(
        &self,
        listed: &[String],
        allowed: Option<&[String]>,
        context: &EnvironmentContext,
    ) -> Vec<Offer> {
        let mut entries = self.entries.lock().await;
        let mut available: BTreeMap<String, ExecutorAvailability> = BTreeMap::new();
        for (kind, entry) in entries.iter_mut() {
            available.insert(kind.clone(), entry.executor.available(context).await);
        }
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        for name in listed {
            for (kind, entry) in entries.iter() {
                let Some(availability) = available.get(kind).filter(|a| a.available) else {
                    continue;
                };
                if kind == name {
                    // An executor with several tool sets is offered by the name of a set, not by its own.
                    if entry.descriptor.tool_sets.is_empty()
                        && allowed.is_none_or(|list| list.contains(name))
                        && seen.insert((kind.clone(), None))
                    {
                        out.push(Offer {
                            name: name.clone(),
                            executor: kind.clone(),
                            tool_set: None,
                        });
                    }
                } else if let Some(set) = tool_set_name(name, kind)
                    && entry.descriptor.has_tool_set(set)
                    && availability
                        .tool_sets
                        .as_ref()
                        .is_none_or(|sets| sets.iter().any(|name| name == set))
                    && allowed.is_none_or(|list| {
                        list.iter()
                            .any(|name| tool_set_name(name, kind) == Some(set))
                    })
                    && seen.insert((kind.clone(), Some(set.to_owned())))
                {
                    out.push(Offer {
                        name: name.clone(),
                        executor: kind.clone(),
                        tool_set: Some(set.to_owned()),
                    });
                }
            }
        }
        out
    }

    /// Starts the executor `kind` in this environment and says where it is reached. Refused when
    /// the machine's policy does not let its exec node offer it (`allowed` lists what it does).
    pub async fn start(
        &self,
        kind: &str,
        allowed: Option<&[String]>,
        context: &EnvironmentContext,
    ) -> Result<ExecStartResult> {
        let mut entries = self.entries.lock().await;
        let entry = entries
            .get_mut(kind)
            .ok_or_else(|| HostError::NotFound(format!("there is no executor {kind:?}")))?;
        start_entry(kind, entry, allowed, context, None).await
    }

    /// Where the running executor `kind` is reached, for the tunnels that end at it.
    pub async fn endpoint(&self, kind: &str) -> Option<ExecutorEndpoint> {
        self.entries
            .lock()
            .await
            .get(kind)
            .and_then(|entry| entry.running.clone())
    }

    /// The executors that run, each with the protocol it speaks to a harness and where it is reached.
    pub async fn running(&self) -> Vec<(String, ExecutorProtocol, ExecutorEndpoint)> {
        self.entries
            .lock()
            .await
            .iter()
            .filter_map(|(kind, entry)| {
                entry
                    .running
                    .clone()
                    .map(|endpoint| (kind.clone(), entry.descriptor.protocol, endpoint))
            })
            .collect()
    }

    /// The protocol an executor of `kind` speaks to a harness, if it is known.
    pub async fn protocol_of(&self, kind: &str) -> Option<ExecutorProtocol> {
        self.entries
            .lock()
            .await
            .get(kind)
            .map(|entry| entry.descriptor.protocol)
    }

    /// Where the executor `kind` is reached, started now if it is not running or no longer answers at its
    /// address (it died since it was started).
    pub async fn ensure(
        &self,
        kind: &str,
        allowed: Option<&[String]>,
        context: &EnvironmentContext,
    ) -> Result<ExecutorEndpoint> {
        self.ensure_at_version(kind, allowed, context, None, None)
            .await
    }

    /// A fresh ticket may name a build; a retry without one retains the exact registered endpoint.
    pub async fn ensure_at_version(
        &self,
        kind: &str,
        allowed: Option<&[String]>,
        context: &EnvironmentContext,
        version: Option<&str>,
        registered: Option<&ExecutorEndpoint>,
    ) -> Result<ExecutorEndpoint> {
        let mut entries = self.entries.lock().await;
        let entry = entries
            .get_mut(kind)
            .ok_or_else(|| HostError::NotFound(format!("there is no executor {kind:?}")))?;
        if registered.is_some_and(|expected| entry.running.as_ref() != Some(expected)) {
            return Err(HostError::Unsupported(
                "the registered executor was retired".into(),
            ));
        }
        if entry.selected_program(context).is_none() && entry.local_program.is_none() {
            drop(entries);
            return self.ensure_legacy(kind, allowed, context).await;
        }
        let expected = version
            .map(str::to_owned)
            .or_else(|| {
                entry
                    .running
                    .as_ref()
                    .map(|endpoint| endpoint.version.clone())
            })
            .ok_or_else(|| {
                HostError::Unsupported(
                    "a local executor needs an admitted or previously running build".into(),
                )
            })?;
        let availability = entry.executor.available(context).await;
        let retired = entry
            .retire_changed_program(context, &availability)
            .await
            .map_err(|error| {
                HostError::Internal(format!(
                    "{kind:?} did not retire its previous program: {error}"
                ))
            })?;
        if retired && (registered.is_some() || version.is_none()) {
            return Err(HostError::Unsupported(
                "the registered executor's program changed".into(),
            ));
        }
        if !availability.available || availability.version.as_deref() != Some(expected.as_str()) {
            return Err(HostError::Unsupported(
                "the confirmed program is not the executor build admitted for these tools".into(),
            ));
        }
        if let Some(endpoint) = &entry.running {
            if endpoint.version != expected {
                return Err(HostError::Unsupported(
                    "the running executor is not the build admitted for these tools".into(),
                ));
            }
            if answers(endpoint).await {
                return Ok(endpoint.clone());
            }
        }
        if entry.running.take().is_some() {
            entry.local_program = None;
            entry
                .executor
                .stop()
                .await
                .map_err(|error| HostError::Internal(format!("{kind:?} did not stop: {error}")))?;
        }
        start_entry(kind, entry, allowed, context, Some(&expected)).await?;
        entry
            .running
            .clone()
            .ok_or_else(|| HostError::Internal(format!("{kind:?} was started and is not there")))
    }

    async fn ensure_legacy(
        &self,
        kind: &str,
        allowed: Option<&[String]>,
        context: &EnvironmentContext,
    ) -> Result<ExecutorEndpoint> {
        if let Some(endpoint) = self.endpoint(kind).await {
            if answers(&endpoint).await {
                return Ok(endpoint);
            }
            let _ = self.stop(kind).await;
        }
        self.start(kind, allowed, context).await?;
        self.endpoint(kind)
            .await
            .ok_or_else(|| HostError::Internal(format!("{kind:?} was started and is not there")))
    }

    /// Whether this executor currently uses a confirmed machine program rather than a held cache.
    pub async fn uses_local_program(&self, kind: &str, context: &EnvironmentContext) -> bool {
        self.entries.lock().await.get(kind).is_some_and(|entry| {
            entry.selected_program(context).is_some() || entry.local_program.is_some()
        })
    }

    /// Stops the executor `kind`, if it runs.
    pub async fn stop(&self, kind: &str) -> Result<()> {
        let mut entries = self.entries.lock().await;
        let entry = entries
            .get_mut(kind)
            .ok_or_else(|| HostError::NotFound(format!("there is no executor {kind:?}")))?;
        if entry.running.take().is_some() {
            entry.local_program = None;
            entry
                .executor
                .stop()
                .await
                .map_err(|error| HostError::Internal(format!("{kind:?} did not stop: {error}")))?;
        }
        Ok(())
    }

    /// Retires executors started from a removed or replaced confirmed program.
    pub async fn stop_changed_programs(
        &self,
        after: &BTreeMap<String, String>,
        context: &EnvironmentContext,
    ) {
        if context.tools_dir.is_some() {
            return;
        }
        let mut entries = self.entries.lock().await;
        for (kind, entry) in entries.iter_mut() {
            if entry.descriptor.protocol != ExecutorProtocol::Harness {
                continue;
            }
            let Some(program) = &entry.descriptor.program else {
                continue;
            };
            let Some(started) = entry.local_program.as_deref() else {
                continue;
            };
            if after.get(program).map(String::as_str) != Some(started)
                && entry.running.take().is_some()
            {
                entry.local_program = None;
                if let Err(error) = entry.executor.stop().await {
                    tracing::warn!(%kind, %error, "an executor of a withdrawn program could not be stopped");
                }
            }
        }
    }

    /// Stops every running executor, as when the environment is detached.
    pub async fn stop_all(&self) {
        let kinds: Vec<String> = self
            .entries
            .lock()
            .await
            .iter()
            .filter(|(_, entry)| entry.running.is_some())
            .map(|(kind, _)| kind.clone())
            .collect();
        for kind in kinds {
            let _ = self.stop(&kind).await;
        }
    }
}

/// How long a look at whether an executor answers waits: the address is on the machine itself.
const ANSWER_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Whether something listens at the address an executor was started at.
async fn answers(endpoint: &ExecutorEndpoint) -> bool {
    let Ok(port) = endpoint.port() else {
        return false;
    };
    matches!(
        tokio::time::timeout(
            ANSWER_WAIT,
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        )
        .await,
        Ok(Ok(_))
    )
}

/// `ExecutorAvailability::into_fact` gives the agent contract's fact; the link's is the same
/// shape with the link's own words for the sandbox modes.
fn to_link_fact(fact: orch_agent_contract::facts::ExecutorFact) -> EnvironmentFactsExecutor {
    EnvironmentFactsExecutor {
        kind: fact.kind,
        version: fact.version,
        available: fact.available,
        reason: fact.reason,
        sandbox_modes: fact.sandbox_modes.into_iter().map(sandbox).collect(),
        tool_sets: fact.tool_sets,
    }
}

fn sandbox(
    mode: orch_agent_contract::descriptor::SandboxMode,
) -> orch_link_contract::commands::SandboxMode {
    use orch_agent_contract::descriptor::SandboxMode as Agent;
    use orch_link_contract::commands::SandboxMode as Link;
    match mode {
        Agent::ReadOnly => Link::ReadOnly,
        Agent::Workspace => Link::Workspace,
        Agent::Open => Link::Open,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use orch_agent_contract::adapter::AdapterError;
    use orch_agent_contract::descriptor::SandboxMode;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn context(mcp: bool) -> EnvironmentContext {
        serde_json::from_value(json!({
            "environment_id": "0199abcd-1234-7000-8000-0000000000e1", "generation": 1, "data_dir": "/d",
            "workspace_root": "/w", "home": null, "tools_dir": null, "tools": [], "mounts": [], "user": "agent",
            "shell": { "name": "sh", "path": "/bin/sh" }, "os": "linux", "arch": "x86_64", "path_style": "posix",
            "limits": {}, "variables": {}, "mcp_servers_enabled": mcp
        }))
        .unwrap()
    }

    /// An executor with three tool sets, of which `custom` needs a tool server.
    struct Orch {
        started: bool,
    }

    #[async_trait]
    impl Executor for Orch {
        fn kind(&self) -> &'static str {
            "orch-exec"
        }
        fn describe(&self) -> ExecutorDescriptor {
            serde_json::from_value(json!({
                "kind": "orch-exec", "title": "Orch's own tools", "protocol": "mcp",
                "tool_sets": [
                    { "name": "claude-code", "summary": "Claude Code's names." },
                    { "name": "plain", "summary": "Eight plain tools." },
                    { "name": "custom", "summary": "None of its own." } ],
                "sandbox_modes": ["open"],
                "attribution": { "thread_id": "ORCH_THREAD_ID", "tool_call_id": "ORCH_ITEM_ID" }
            }))
            .unwrap()
        }
        async fn available(&self, env: &EnvironmentContext) -> ExecutorAvailability {
            let mut sets = vec!["claude-code".to_owned(), "plain".to_owned()];
            if env.mcp_servers_enabled {
                sets.push("custom".to_owned());
            }
            ExecutorAvailability {
                tool_sets: Some(sets),
                ..ExecutorAvailability::available("0.1.0", vec![SandboxMode::Open])
            }
        }
        async fn start(
            &mut self,
            _env: &EnvironmentContext,
        ) -> std::result::Result<ExecutorEndpoint, AdapterError> {
            self.started = true;
            Ok(ExecutorEndpoint {
                address: "127.0.0.1:41777".into(),
                token: None,
                version: "0.1.0".into(),
                tool_sets: vec![],
                sandbox_modes: vec![SandboxMode::Open],
            })
        }
        async fn stop(&mut self) -> std::result::Result<(), AdapterError> {
            self.started = false;
            Ok(())
        }
    }

    /// An executor of a harness's own protocol, which is not there.
    struct Missing;

    struct LocalProgram {
        stops: Arc<AtomicUsize>,
        starts: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Executor for LocalProgram {
        fn kind(&self) -> &'static str {
            "local-program"
        }
        fn describe(&self) -> ExecutorDescriptor {
            serde_json::from_value(json!({
                "kind":"local-program","title":"Local program","program":"codex",
                "protocol":"harness","sandbox_modes":["open"]
            }))
            .unwrap()
        }
        async fn available(&self, env: &EnvironmentContext) -> ExecutorAvailability {
            ExecutorAvailability::available(
                env.variables
                    .get("TEST_VERSION")
                    .map_or("1.0.0", String::as_str),
                vec![SandboxMode::Open],
            )
        }
        async fn start(
            &mut self,
            env: &EnvironmentContext,
        ) -> std::result::Result<ExecutorEndpoint, AdapterError> {
            let count = self.starts.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(ExecutorEndpoint {
                address: "127.0.0.1:41777".into(),
                token: Some(orch_plugin_contract::Secret::new(format!(
                    "controlled-executor-{count}"
                ))),
                version: env
                    .variables
                    .get("TEST_VERSION")
                    .map_or("1.0.0", String::as_str)
                    .into(),
                tool_sets: Vec::new(),
                sandbox_modes: vec![SandboxMode::Open],
            })
        }
        async fn stop(&mut self) -> std::result::Result<(), AdapterError> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn removing_or_replacing_a_local_path_retires_its_executor_at_the_same_version() {
        for after in [
            BTreeMap::new(),
            BTreeMap::from([("codex".into(), "/b/codex".into())]),
        ] {
            let registry = registry().await;
            let stops = Arc::new(AtomicUsize::new(0));
            registry
                .register(Box::new(LocalProgram {
                    stops: Arc::clone(&stops),
                    starts: Default::default(),
                }))
                .await
                .unwrap();
            let mut env = context(false);
            env.local_programs.insert("codex".into(), "/a/codex".into());
            registry.start("local-program", None, &env).await.unwrap();
            registry.start("orch-exec", None, &env).await.unwrap();
            registry.stop_changed_programs(&after, &env).await;
            assert_eq!(stops.load(Ordering::SeqCst), 1);
            assert!(registry.endpoint("local-program").await.is_none());
            assert!(registry.endpoint("orch-exec").await.is_some());
        }
    }

    #[tokio::test]
    async fn unchanged_selections_and_held_cache_builds_are_not_retired_by_local_path_changes() {
        for cached in [false, true] {
            let registry = registry().await;
            let stops = Arc::new(AtomicUsize::new(0));
            registry
                .register(Box::new(LocalProgram {
                    stops: Arc::clone(&stops),
                    starts: Default::default(),
                }))
                .await
                .unwrap();
            let mut env = context(false);
            env.local_programs.insert("codex".into(), "/a/codex".into());
            if cached {
                env.tools_dir = Some("/held-cache".into());
            }
            registry.start("local-program", None, &env).await.unwrap();
            let mut after = env.local_programs.clone();
            after.insert("claude-code".into(), "/claude".into());
            registry.stop_changed_programs(&after, &env).await;
            assert_eq!(stops.load(Ordering::SeqCst), 0);
            assert!(registry.endpoint("local-program").await.is_some());
            if cached {
                registry.stop_changed_programs(&BTreeMap::new(), &env).await;
                assert_eq!(stops.load(Ordering::SeqCst), 0);
                assert!(registry.endpoint("local-program").await.is_some());
            }
        }
    }

    #[tokio::test]
    async fn an_executor_already_started_from_the_new_path_survives_that_update() {
        let registry = registry().await;
        let stops = Arc::new(AtomicUsize::new(0));
        registry
            .register(Box::new(LocalProgram {
                stops: Arc::clone(&stops),
                starts: Default::default(),
            }))
            .await
            .unwrap();
        let mut env = context(false);
        env.local_programs.insert("codex".into(), "/b/codex".into());
        registry.start("local-program", None, &env).await.unwrap();
        let current = registry.endpoint("local-program").await.unwrap();
        registry
            .stop_changed_programs(&env.local_programs, &env)
            .await;
        assert_eq!(registry.endpoint("local-program").await, Some(current));
        assert_eq!(stops.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn current_program_version_facts_retire_the_old_executor_but_keep_unrelated_tools() {
        let registry = registry().await;
        let stops = Arc::new(AtomicUsize::new(0));
        registry
            .register(Box::new(LocalProgram {
                stops: Arc::clone(&stops),
                starts: Default::default(),
            }))
            .await
            .unwrap();
        let mut env = context(false);
        env.local_programs.insert("codex".into(), "/a/codex".into());
        registry.start("local-program", None, &env).await.unwrap();
        registry.start("orch-exec", None, &env).await.unwrap();
        env.variables.insert("TEST_VERSION".into(), "2.0.0".into());
        let facts = registry.facts(&env).await;
        let changed = facts
            .iter()
            .find(|fact| fact.kind == "local-program")
            .unwrap();
        assert!(changed.available);
        assert_eq!(changed.version, "2.0.0");
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert!(registry.endpoint("local-program").await.is_none());
        assert!(registry.endpoint("orch-exec").await.is_some());
    }

    #[tokio::test]
    async fn a_fresh_local_executor_requires_the_exact_admitted_known_or_unknown_build() {
        for version in ["1.0.0", "unknown"] {
            let registry = registry().await;
            let starts = Arc::new(AtomicUsize::new(0));
            let stops = Arc::new(AtomicUsize::new(0));
            registry
                .register(Box::new(LocalProgram {
                    starts: Arc::clone(&starts),
                    stops: Arc::clone(&stops),
                }))
                .await
                .unwrap();
            let mut env = context(false);
            env.local_programs.insert("codex".into(), "/a/codex".into());
            env.variables.insert("TEST_VERSION".into(), version.into());
            for requested in [None, Some("another-build")] {
                assert!(
                    registry
                        .ensure_at_version("local-program", None, &env, requested, None)
                        .await
                        .is_err()
                );
                assert_eq!(starts.load(Ordering::SeqCst), 0);
                assert!(registry.endpoint("local-program").await.is_none());
            }
            let endpoint = registry
                .ensure_at_version("local-program", None, &env, Some(version), None)
                .await
                .unwrap();
            assert_eq!(endpoint.version, version);
            assert_eq!(starts.load(Ordering::SeqCst), 1);
            assert!(
                registry
                    .ensure_at_version("local-program", None, &env, Some("another-build"), None)
                    .await
                    .is_err()
            );
            assert_eq!(registry.endpoint("local-program").await, Some(endpoint));
            assert_eq!(starts.load(Ordering::SeqCst), 1);
            assert_eq!(
                stops.load(Ordering::SeqCst),
                0,
                "a stale ticket does not kill a current executor"
            );
        }
    }

    #[tokio::test]
    async fn a_ticketless_retry_does_not_start_a_changed_build_and_a_fresh_ticket_must_name_it() {
        let registry = registry().await;
        let starts = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        registry
            .register(Box::new(LocalProgram {
                starts: Arc::clone(&starts),
                stops: Arc::clone(&stops),
            }))
            .await
            .unwrap();
        let mut env = context(false);
        env.local_programs.insert("codex".into(), "/a/codex".into());
        registry.start("local-program", None, &env).await.unwrap();
        let original = registry.endpoint("local-program").await.unwrap();
        env.variables.insert("TEST_VERSION".into(), "2.0.0".into());
        assert!(
            registry
                .ensure_at_version("local-program", None, &env, None, Some(&original))
                .await
                .is_err()
        );
        assert_eq!(
            starts.load(Ordering::SeqCst),
            1,
            "the changed program was never started by the retry"
        );
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert!(registry.endpoint("local-program").await.is_none());
        assert!(
            registry
                .ensure_at_version("local-program", None, &env, Some("1.0.0"), None)
                .await
                .is_err()
        );
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        let replacement = registry
            .ensure_at_version("local-program", None, &env, Some("2.0.0"), None)
            .await
            .unwrap();
        assert_eq!(replacement.version, "2.0.0");
        assert_eq!(starts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_retired_registered_endpoint_is_not_revived_after_a_same_build_path_replacement() {
        let registry = registry().await;
        let starts = Arc::new(AtomicUsize::new(0));
        registry
            .register(Box::new(LocalProgram {
                starts: Arc::clone(&starts),
                stops: Default::default(),
            }))
            .await
            .unwrap();
        let mut env = context(false);
        env.local_programs.insert("codex".into(), "/a/codex".into());
        registry.start("local-program", None, &env).await.unwrap();
        let original = registry.endpoint("local-program").await.unwrap();
        env.local_programs.insert("codex".into(), "/b/codex".into());
        registry
            .stop_changed_programs(&env.local_programs, &env)
            .await;
        assert!(
            registry
                .ensure_at_version("local-program", None, &env, None, Some(&original))
                .await
                .is_err()
        );
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        let replacement = registry
            .ensure_at_version("local-program", None, &env, Some("1.0.0"), None)
            .await
            .unwrap();
        assert_ne!(replacement, original);
        assert!(
            registry
                .ensure_at_version("local-program", None, &env, None, Some(&original))
                .await
                .is_err()
        );
        assert_eq!(registry.endpoint("local-program").await, Some(replacement));
        assert_eq!(starts.load(Ordering::SeqCst), 2);
    }

    #[async_trait]
    impl Executor for Missing {
        fn kind(&self) -> &'static str {
            "codex-exec-server"
        }
        fn describe(&self) -> ExecutorDescriptor {
            serde_json::from_value(json!({
                "kind": "codex-exec-server", "title": "Codex's", "protocol": "harness", "sandbox_modes": ["open"],
                "attribution": { "thread_id": "CODEX_THREAD_ID", "tool_call_id": "CODEX_TOOL_CALL_ID" }
            }))
            .unwrap()
        }
        async fn available(&self, _env: &EnvironmentContext) -> ExecutorAvailability {
            ExecutorAvailability::unavailable("the build is not in the tool cache")
        }
        async fn start(
            &mut self,
            _env: &EnvironmentContext,
        ) -> std::result::Result<ExecutorEndpoint, AdapterError> {
            Err(AdapterError::HarnessUnavailable("no build".into()))
        }
        async fn stop(&mut self) -> std::result::Result<(), AdapterError> {
            Ok(())
        }
    }

    async fn registry() -> Executors {
        let registry = Executors::new();
        registry
            .register(Box::new(Orch { started: false }))
            .await
            .unwrap();
        registry.register(Box::new(Missing)).await.unwrap();
        registry
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[tokio::test]
    async fn the_offers_are_the_environments_list_in_its_order_kept_to_what_can_be_had_here() {
        let registry = registry().await;
        let listed = names(&["codex-exec-server", "claude-code", "plain", "custom"]);
        let without_servers = registry.offers(&listed, None, &context(false)).await;
        assert_eq!(
            without_servers
                .iter()
                .map(|o| o.name.as_str())
                .collect::<Vec<_>>(),
            ["claude-code", "plain"],
            "codex is not there, and `custom` needs a tool server"
        );
        assert_eq!(without_servers[0].tool_set.as_deref(), Some("claude-code"));
        assert_eq!(without_servers[0].executor, "orch-exec");
        let with_servers = registry.offers(&listed, None, &context(true)).await;
        assert_eq!(
            with_servers
                .iter()
                .map(|o| o.name.as_str())
                .collect::<Vec<_>>(),
            ["claude-code", "plain", "custom"]
        );
        let reordered = registry
            .offers(
                &names(&["plain", "claude-code", "plain"]),
                None,
                &context(false),
            )
            .await;
        assert_eq!(
            reordered
                .iter()
                .map(|o| o.name.as_str())
                .collect::<Vec<_>>(),
            ["plain", "claude-code"],
            "in the environment's order, each once"
        );
    }

    #[tokio::test]
    async fn the_machines_owner_can_allow_fewer_offers_than_the_environment_lists() {
        let registry = registry().await;
        let listed = names(&["claude-code", "plain"]);
        let allowed = names(&["plain"]);
        let offers = registry
            .offers(&listed, Some(&allowed), &context(false))
            .await;
        assert_eq!(
            offers.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(),
            ["plain"]
        );
        // An offer outside the policy is refused when it is asked for.
        let refused = registry
            .start(
                "orch-exec",
                Some(&names(&["codex-exec-server"])),
                &context(false),
            )
            .await
            .unwrap_err();
        assert!(matches!(refused, HostError::ExecNotOffered(_)));
        assert_eq!(refused.command_error().code, "policy.exec_not_offered");
        assert!(
            registry.endpoint("orch-exec").await.is_none(),
            "nothing was started"
        );
    }

    #[tokio::test]
    async fn namespaced_offers_keep_the_same_exact_owner_limit_and_executor_identity() {
        let executors = registry().await;
        let allowed = names(&["orch-exec:plain"]);
        let offered = executors
            .offers(
                &names(&["orch-exec:claude-code", "orch-exec:plain"]),
                Some(&allowed),
                &context(false),
            )
            .await;
        assert_eq!(
            offered,
            vec![Offer {
                name: "orch-exec:plain".into(),
                executor: "orch-exec".into(),
                tool_set: Some("plain".into()),
            }]
        );
        assert!(
            executors
                .start("orch-exec", Some(&allowed), &context(false))
                .await
                .is_ok()
        );
        executors.stop_all().await;
    }

    #[tokio::test]
    async fn legacy_and_namespaced_names_are_one_offer_without_widening_the_allowed_set() {
        let executors = registry().await;
        let offered = executors
            .offers(
                &names(&["plain", "orch-exec:plain", "orch-exec:claude-code"]),
                Some(&names(&["orch-exec:plain"])),
                &context(false),
            )
            .await;
        assert_eq!(
            offered,
            vec![Offer {
                name: "plain".into(),
                executor: "orch-exec".into(),
                tool_set: Some("plain".into()),
            }]
        );
    }

    #[tokio::test]
    async fn another_executor_or_an_unknown_set_cannot_authorize_starting_this_executor() {
        for allowed in [
            names(&["other-exec:plain"]),
            names(&["orch-exec:unknown"]),
            Vec::new(),
        ] {
            let executors = registry().await;
            assert!(
                executors
                    .offers(
                        &names(&["orch-exec:plain"]),
                        Some(&allowed),
                        &context(false)
                    )
                    .await
                    .is_empty()
            );
            assert!(matches!(
                executors
                    .start("orch-exec", Some(&allowed), &context(false))
                    .await,
                Err(HostError::ExecNotOffered(_))
            ));
            assert!(
                executors.endpoint("orch-exec").await.is_none(),
                "no refused executor may start"
            );
        }
    }

    #[tokio::test]
    async fn an_executor_is_started_where_it_can_be_and_its_address_is_remembered_until_it_stops() {
        let registry = registry().await;
        let started = registry
            .start("orch-exec", Some(&names(&["plain"])), &context(false))
            .await
            .unwrap();
        assert_eq!(
            started.service, "mcp/exec",
            "an executor that offers MCP tools is reached by the server that is called exec"
        );
        assert_eq!(started.version, "0.1.0");
        assert_eq!(
            started.tool_sets,
            ["claude-code", "plain", "custom"],
            "an executor that does not say lists those it declared"
        );
        assert_eq!(
            registry.endpoint("orch-exec").await.unwrap().address,
            "127.0.0.1:41777"
        );
        let failed = registry
            .start("codex-exec-server", None, &context(false))
            .await
            .unwrap_err();
        assert!(failed.to_string().contains("tool cache"), "{failed}");
        assert!(matches!(
            registry.start("unknown", None, &context(false)).await,
            Err(HostError::NotFound(_))
        ));
        registry.stop_all().await;
        assert!(registry.endpoint("orch-exec").await.is_none());
    }

    #[tokio::test]
    async fn the_environments_facts_say_what_each_executor_can_do_and_the_variables_for_attribution_are_known()
     {
        let registry = registry().await;
        let facts = registry.facts(&context(false)).await;
        let orch = facts.iter().find(|f| f.kind == "orch-exec").unwrap();
        assert!(orch.available && orch.reason.is_none());
        assert_eq!(
            orch.tool_sets.as_deref(),
            Some(&names(&["claude-code", "plain"])[..]),
            "`custom` only while a tool server is enabled"
        );
        let codex = facts
            .iter()
            .find(|f| f.kind == "codex-exec-server")
            .unwrap();
        assert!(!codex.available);
        assert_eq!(
            codex.reason.as_deref(),
            Some("the build is not in the tool cache")
        );
        let attributions = registry.attributions().await;
        assert_eq!(attributions.len(), 2);
        assert!(attributions.iter().any(|a| a.thread_id == "CODEX_THREAD_ID" && a.tool_call_id == "CODEX_TOOL_CALL_ID"));
        assert!(
            registry.register(Box::new(Missing)).await.is_err(),
            "a kind is registered once"
        );
    }
}
