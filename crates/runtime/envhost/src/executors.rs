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
}

fn tool_set_name<'a>(offer: &'a str, executor: &str) -> Option<&'a str> {
    match offer.split_once(':') {
        Some((owner, name)) => (owner == executor).then_some(name),
        None => Some(offer),
    }
}

fn policy_allows(kind: &str, descriptor: &ExecutorDescriptor, allowed: Option<&[String]>) -> bool {
    allowed.is_none_or(|list| {
        list.iter().any(|name| {
            name == kind
                || tool_set_name(name, kind).is_some_and(|set| descriptor.has_tool_set(set))
        })
    })
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
            let mut fact = availability.clone().into_fact(kind);
            if context.tools_dir.is_none()
                && entry
                    .descriptor
                    .program
                    .as_ref()
                    .is_some_and(|program| context.local_programs.contains_key(program))
                && entry.running.as_ref().is_some_and(|running| {
                    !availability.available
                        || availability.version.as_deref() != Some(running.version.as_str())
                })
                && let Some(previous) = entry.running.take()
                && let Err(error) = entry.executor.stop().await
            {
                fact.available = false;
                fact.version.clone_from(&previous.version);
                fact.reason = Some(format!(
                    "the previous executor could not be stopped: {error}"
                ));
                entry.running = Some(previous);
            }
            // A tool set is offered only if it is one the executor declared.
            if let Some(sets) = &mut fact.tool_sets {
                sets.retain(|name| entry.descriptor.has_tool_set(name));
            }
            if (fact.version.is_empty() || fact.version == "unknown")
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
        if !policy_allows(kind, &entry.descriptor, allowed) {
            return Err(HostError::ExecNotOffered(format!(
                "this machine's owner does not let its exec node offer {kind:?}"
            )));
        }
        let availability = entry.executor.available(context).await;
        if !availability.available {
            return Err(HostError::Unsupported(format!(
                "{kind:?} cannot start here: {}",
                availability
                    .reason
                    .unwrap_or_else(|| "no reason was given".to_owned())
            )));
        }
        let endpoint = entry
            .executor
            .start(context)
            .await
            .map_err(|error| HostError::Internal(format!("{kind:?} did not start: {error}")))?;
        endpoint.port().map_err(|error| {
            HostError::Internal(format!(
                "{kind:?} gave an address that is not its own: {error}"
            ))
        })?;
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
        Ok(result)
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
        if let Some(endpoint) = self.endpoint(kind).await {
            let current = {
                let mut entries = self.entries.lock().await;
                match entries.get_mut(kind) {
                    Some(entry) if policy_allows(kind, &entry.descriptor, allowed) => {
                        let available = entry.executor.available(context).await;
                        let local_program = context.tools_dir.is_none()
                            && entry.descriptor.program.as_ref().is_some_and(|program| {
                                context.local_programs.contains_key(program)
                            });
                        available.available
                            && (!local_program
                                || available.version.as_deref() == Some(endpoint.version.as_str()))
                    }
                    _ => false,
                }
            };
            if current && answers(&endpoint).await {
                return Ok(endpoint);
            }
            let _ = self.stop(kind).await;
        }
        self.start(kind, allowed, context).await?;
        self.endpoint(kind)
            .await
            .ok_or_else(|| HostError::Internal(format!("{kind:?} was started and is not there")))
    }

    /// Stops the executor `kind`, if it runs.
    pub async fn stop(&self, kind: &str) -> Result<()> {
        let mut entries = self.entries.lock().await;
        let entry = entries
            .get_mut(kind)
            .ok_or_else(|| HostError::NotFound(format!("there is no executor {kind:?}")))?;
        if entry.running.take().is_some() {
            entry
                .executor
                .stop()
                .await
                .map_err(|error| HostError::Internal(format!("{kind:?} did not stop: {error}")))?;
        }
        Ok(())
    }

    /// Stops running executors whose locally authorized program was removed or replaced.
    pub async fn stop_changed_programs(
        &self,
        before: &BTreeMap<String, String>,
        after: &BTreeMap<String, String>,
    ) {
        let kinds: Vec<String> = self
            .entries
            .lock()
            .await
            .iter()
            .filter_map(|(kind, entry)| {
                let program = entry.descriptor.program.as_ref()?;
                (entry.running.is_some() && before.get(program) != after.get(program))
                    .then(|| kind.clone())
            })
            .collect();
        for kind in kinds {
            if let Err(error) = self.stop(&kind).await {
                tracing::warn!(%kind, %error, "an executor of a withdrawn program could not be stopped");
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

    struct LocalProgram;

    #[async_trait]
    impl Executor for LocalProgram {
        fn kind(&self) -> &'static str {
            "local-program"
        }
        fn describe(&self) -> ExecutorDescriptor {
            serde_json::from_value(
                json!({"kind":"local-program","title":"Local program","program":"codex",
                "protocol":"harness","sandbox_modes":["open"]}),
            )
            .unwrap()
        }
        async fn available(&self, env: &EnvironmentContext) -> ExecutorAvailability {
            if env.local_programs.contains_key("codex") {
                ExecutorAvailability::available(
                    env.variables
                        .get("TEST_VERSION")
                        .map_or("1.0.0", String::as_str),
                    vec![SandboxMode::Open],
                )
            } else {
                ExecutorAvailability::unavailable("the program is no longer allowed")
            }
        }
        async fn start(
            &mut self,
            _env: &EnvironmentContext,
        ) -> std::result::Result<ExecutorEndpoint, AdapterError> {
            Ok(ExecutorEndpoint {
                address: "127.0.0.1:41777".into(),
                token: None,
                version: "1.0.0".into(),
                tool_sets: vec![],
                sandbox_modes: vec![SandboxMode::Open],
            })
        }
        async fn stop(&mut self) -> std::result::Result<(), AdapterError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn removing_a_confirmed_program_stops_its_executor_without_stopping_other_tools() {
        let registry = registry().await;
        registry.register(Box::new(LocalProgram)).await.unwrap();
        let mut env = context(false);
        env.local_programs
            .insert("codex".into(), "/local/codex".into());
        registry.start("local-program", None, &env).await.unwrap();
        registry.start("orch-exec", None, &env).await.unwrap();
        registry
            .stop_changed_programs(&env.local_programs, &BTreeMap::new())
            .await;
        assert!(registry.endpoint("local-program").await.is_none());
        assert!(registry.endpoint("orch-exec").await.is_some());
        env.local_programs.clear();
        assert!(registry.start("local-program", None, &env).await.is_err());
    }

    #[tokio::test]
    async fn an_updated_program_retires_the_old_executor_before_reporting_the_new_version() {
        let registry = registry().await;
        registry.register(Box::new(LocalProgram)).await.unwrap();
        let mut env = context(false);
        env.local_programs
            .insert("codex".into(), "/local/codex".into());
        registry.start("local-program", None, &env).await.unwrap();
        env.variables.insert("TEST_VERSION".into(), "2.0.0".into());
        let facts = registry.facts(&env).await;
        assert!(registry.endpoint("local-program").await.is_none());
        let changed = facts
            .iter()
            .find(|fact| fact.kind == "local-program")
            .unwrap();
        assert!(changed.available);
        assert_eq!(changed.version, "2.0.0");
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
