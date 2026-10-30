//! External AgentAdapter bridge.
//!
//! [`AgentBridge`] is an [`AgentAdapter`] whose implementation is a program that sits beside the connector, with
//! the reach of the user it runs as. The methods of the transport are the agent-role commands of the link, by the
//! names of [`orch_agent_contract::adapter::method`], and what the program emits are the events of
//! [`AdapterEvent`], written as the contract states ([`AdapterEvent::to_wire`]).
//!
//! | Method | Required | Parameters | Result |
//! |--------|----------|------------|--------|
//! | `agent.start` | yes | what the connector gives: `provider_id`, `agent_id`, `harness_kind`, `profile`, `home`, `provider_config`, `loopback`, `binary` | `harness_version` and the `descriptor`; asked again, it is `probe` |
//! | `agent.stop` | yes | `provider_id`, `agent_id` | none |
//! | `session.open` | yes | the session spec: the settings of the session, its `session_key`, the fetched `archive` and `history`, `provider`, `exec`, `mcp` | `native_session_ref`, `resumed`, `import_report` |
//! | `session.close` | yes | `thread_id` | none |
//! | `turn.start` | yes | the turn: `thread_id`, `turn_id`, `text`, `images`, `model`, `effort`, `params`, `approval_mode`, `sandbox_mode` | none |
//! | `turn.steer` | no | `thread_id`, `turn_id`, `text`, `images` | none |
//! | `turn.interrupt` | no | `thread_id`, `turn_id` | none |
//! | `approval.resolve` | no | `thread_id`, `request_ref`, `decision` | none |
//! | `orch.tool_result` | no | `thread_id`, `call_ref`, and `result` or `error`: `code`, `message` | none |
//! | `session.export` | no | `thread_id`, `kind` (`native` or `neutral`), `upto_turn_ordinal` | the archive: `format`, `path`, `size` |
//! | `session.resync` | no | `thread_id` | `items`: how many were emitted again |
//! | `agent.discover_sessions` | no | `provider_id`, `agent_id` | `sessions` |
//! | `mcp.discover` | no | the search: `directories` | `servers` |
//! | `mcp.import` | no | the request: `server_id`, `source`, `scope`, `name`, `directory` | the definition of the server |
//! | `provider.login.start` | no | `provider_id`, `agent_id`, `method`, `values`, `config` | none; progress arrives as `provider.login` events |
//! | `provider.login.input` | no | `provider_id`, `agent_id`, `values` | none |
//! | `provider.login.cancel`, `provider.logout` | no | `provider_id`, `agent_id` | none |
//! | `provider.refresh` | no | `provider_id` | `facts`, beside the provider's status |
//! | `provider.discover_models` | no | `provider_id` | `suggestions` |
//! | `provider.credential.issue` | no | `provider_id`, `form` | the credential: `form`, `credential`, `expires_at` |
//! | `provider.credential.accept` | no | `provider_id` and the credential | none |
//! | `agent.credential.set` | no | `provider_id`, `agent_id` and the credential | none |
//! | `agent.credential.return` | no | `provider_id`, `agent_id` | the credential |
//!
//! **One program, many harness processes.** A plugin is started once and kept; the connector makes an adapter for
//! each harness process it starts, and every one of them is a bridge to the same program ([`AdapterFactory`]). The
//! events of a session go to the bridge that opened it: it is known by the thread, and is entered before
//! `session.open` is sent, since the program may emit before it answers. The other events (`provider.login`,
//! `provider.facts`, `harness.descriptor`, `harness.status`) go to the bridge of the harness process they name
//! with `agent_id`, a member the program adds when it serves several, and otherwise to every harness process that
//! is started. When the program is gone, every harness process that was started on it is reported offline.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use orch_agent_contract::adapter::{
    AdapterError, AdapterEvent, AgentAdapter, AgentStart, AgentStarted, EventSink, WireError,
    method,
};
use orch_agent_contract::credentials::{GivenCredential, IssuedCredential};
use orch_agent_contract::descriptor::AgentDescriptor;
use orch_agent_contract::discovery::{
    McpDefinition, McpFound, McpImportRequest, McpSearch, ModelSuggestion, NativeSession,
};
use orch_agent_contract::facts::{HarnessState, HarnessStatus, ProviderFacts};
use orch_agent_contract::history::{ExportKind, SessionArchive};
use orch_agent_contract::login::{LoginRequest, LoginValues};
use orch_agent_contract::session::{
    ApprovalDecision, SessionOpened, SessionSpec, ThreadId, ToolError, TurnId, TurnStart, TurnSteer,
};
use orch_plugin_contract::PluginKind;
use orch_plugin_contract::messages::Event;
use orch_types::{Id, Json};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use super::{BridgeError, Described, adapter_error, interned, member, params, result};
use crate::requests::{CallError, Observer};
use crate::supervisor::Plugin;

/// One bridge's place in the registry that routes the events of the program.
struct Slot {
    sink: EventSink,
    agent_id: Option<Id>,
    threads: BTreeSet<ThreadId>,
    started: bool,
}

/// The bridges of one program, and where each one's events go. It is the observer of the plugin.
#[derive(Default)]
struct Registry {
    slots: Mutex<BTreeMap<u64, Slot>>,
    next: AtomicU64,
    installed: AtomicBool,
}

impl Registry {
    /// The registry of `plugin`, made and made its observer on the first ask.
    fn of(plugin: &Plugin) -> Arc<Self> {
        let registry = plugin.shared_state::<Self>();
        if !registry.installed.swap(true, Ordering::AcqRel) {
            plugin.set_observer(Arc::clone(&registry) as Arc<dyn Observer>);
        }
        registry
    }

    fn slots(&self) -> MutexGuard<'_, BTreeMap<u64, Slot>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn new_slot(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// The bridge `slot` is starting its harness process: from now on it hears.
    fn begin(&self, slot: u64, sink: EventSink, agent_id: Id) {
        let mut slots = self.slots();
        let threads = slots
            .remove(&slot)
            .map(|old| old.threads)
            .unwrap_or_default();
        slots.insert(
            slot,
            Slot {
                sink,
                agent_id: Some(agent_id),
                threads,
                started: true,
            },
        );
    }

    fn forget(&self, slot: u64) {
        self.slots().remove(&slot);
    }

    fn open_thread(&self, slot: u64, thread: ThreadId) {
        if let Some(slot) = self.slots().get_mut(&slot) {
            slot.threads.insert(thread);
        }
    }

    fn close_thread(&self, slot: u64, thread: &ThreadId) {
        if let Some(slot) = self.slots().get_mut(&slot) {
            slot.threads.remove(thread);
        }
    }

    /// The sinks an event goes to.
    fn sinks_for(&self, event: &AdapterEvent, agent_id: Option<Id>) -> Vec<EventSink> {
        let slots = self.slots();
        let started: Vec<&Slot> = slots.values().filter(|slot| slot.started).collect();
        let chosen: Vec<&Slot> = match event {
            AdapterEvent::Session { thread_id, .. } => {
                let owners: Vec<&Slot> = started
                    .iter()
                    .copied()
                    .filter(|slot| slot.threads.contains(thread_id))
                    .collect();
                if owners.is_empty() && started.len() == 1 {
                    // A session nobody opened here, with only one harness process to hear it.
                    started
                } else {
                    owners
                }
            }
            AdapterEvent::ModelRequest(report) => started
                .into_iter()
                .filter(|slot| slot.threads.contains(&report.thread_id))
                .collect(),
            _ => match agent_id {
                Some(agent) => started
                    .into_iter()
                    .filter(|slot| slot.agent_id == Some(agent))
                    .collect(),
                None => started,
            },
        };
        chosen.into_iter().map(|slot| slot.sink.clone()).collect()
    }
}

#[async_trait::async_trait]
impl Observer for Registry {
    async fn event(&self, event: Event) {
        let agent_id = event
            .params
            .get("agent_id")
            .and_then(|id| serde_json::from_value::<Id>(id.clone()).ok());
        let name = event.event;
        let heard = match AdapterEvent::from_wire(&name, event.params) {
            Ok(heard) => heard,
            Err(WireError::UnknownEvent(_)) => {
                // Both sides ignore what they do not know.
                tracing::debug!(event = %name, "an event of the adapter is not known and was ignored");
                return;
            }
            Err(error) => {
                tracing::warn!(event = %name, %error, "an event of the adapter is not what it should be and was ignored");
                return;
            }
        };
        for sink in self.sinks_for(&heard, agent_id) {
            // A host that has stopped listening has nothing more to learn.
            let _ = sink.emit(heard.clone()).await;
        }
    }

    async fn lost(&self, reason: &str) {
        let sinks: Vec<_> = {
            let mut slots = self.slots();
            slots
                .values_mut()
                .filter(|slot| slot.started)
                .map(|slot| {
                    slot.started = false;
                    slot.threads.clear();
                    slot.sink.clone()
                })
                .collect()
        };
        for sink in sinks {
            let _ = sink
                .emit(AdapterEvent::Status(HarnessStatus {
                    status: HarnessState::Offline,
                    detail: Some(reason.to_owned()),
                    harness_version: None,
                    sessions_open: 0,
                }))
                .await;
        }
    }
}

/// An adapter that is a program, for one harness process.
pub struct AgentBridge {
    plugin: Plugin,
    descriptor: Described<AgentDescriptor>,
    registry: Arc<Registry>,
    kind: &'static str,
    slot: u64,
    started: Option<AgentStart>,
}

impl AgentBridge {
    /// An adapter for one harness process, made by `plugin`. The plugin must be of the kind and have said hello.
    pub fn new(plugin: Plugin) -> Result<Self, BridgeError> {
        let descriptor = Described::<AgentDescriptor>::new(plugin.clone(), PluginKind::Adapter)?;
        let kind = interned(&descriptor.get().kind);
        let registry = Registry::of(&plugin);
        Ok(Self {
            slot: registry.new_slot(),
            plugin,
            descriptor,
            registry,
            kind,
            started: None,
        })
    }

    /// What the program declared of its harness when it said hello, as of the last time it did. The adapter
    /// answers the same again when the harness is started ([`AgentAdapter::start`]), with what the harness is
    /// now.
    #[must_use]
    pub fn declared(&self) -> AgentDescriptor {
        (*self.descriptor.get()).clone()
    }

    /// Asks the program `method` and waits for its result. A method that was not declared is not asked: the
    /// adapter does not have the capability that `capability` names.
    async fn ask(
        &self,
        method: &'static str,
        capability: &str,
        sent: Json,
    ) -> Result<Json, AdapterError> {
        if !self.plugin.supports(method) {
            return Err(AdapterError::Unsupported(capability.to_owned()));
        }
        self.call(method, sent).await
    }

    async fn call(&self, method: &'static str, sent: Json) -> Result<Json, AdapterError> {
        self.plugin.call(method, sent).await.map_err(adapter_error)
    }

    /// The provider and the agent of the harness process, once it was started.
    fn harness(&self) -> Result<(Id, Id), AdapterError> {
        self.started
            .as_ref()
            .map(|start| (start.provider_id, start.agent_id))
            .ok_or_else(|| {
                AdapterError::HarnessUnavailable("the harness has not been started".to_owned())
            })
    }

    /// The members that name the harness process, and then `more`.
    fn about_harness(&self, more: Json) -> Result<Json, AdapterError> {
        let (provider_id, agent_id) = self.harness()?;
        let mut members = json!({ "provider_id": provider_id, "agent_id": agent_id });
        merge(&mut members, more);
        Ok(members)
    }
}

impl Drop for AgentBridge {
    fn drop(&mut self) {
        // A bridge that is gone hears nothing more.
        self.registry.forget(self.slot);
    }
}

/// Puts the members of `more`, if it is an object, into `into`.
fn merge(into: &mut Json, more: Json) {
    if let (Json::Object(into), Json::Object(more)) = (into, more) {
        into.extend(more);
    }
}

fn written<T: Serialize>(method: &str, value: &T) -> Result<Json, AdapterError> {
    params(method, value).map_err(adapter_error)
}

fn read<T: DeserializeOwned>(method: &str, value: Json) -> Result<T, AdapterError> {
    result(method, value).map_err(adapter_error)
}

fn read_member<T: DeserializeOwned>(
    method: &str,
    value: Json,
    name: &str,
) -> Result<T, AdapterError> {
    member(method, value, name).map_err(|error: CallError| adapter_error(error))
}

#[async_trait]
impl AgentAdapter for AgentBridge {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn manifest(&self) -> Option<orch_agent_programs::ProgramManifest> {
        // The program's manifest is the `manifest` member of the descriptor it sent when it said hello.
        let negotiated = self.plugin.negotiated()?;
        let manifest = negotiated.descriptor.get("manifest")?;
        serde_json::from_value::<orch_agent_programs::ProgramManifest>(manifest.clone())
            .ok()
            .filter(|manifest| manifest.check().is_ok() && manifest.kind == self.kind)
    }

    async fn start(
        &mut self,
        start: AgentStart,
        sink: EventSink,
    ) -> Result<AgentStarted, AdapterError> {
        // The program may emit before it answers, so the place for its events is made first.
        self.registry.begin(self.slot, sink, start.agent_id);
        let sent = written(method::AGENT_START, &start)?;
        match self.call(method::AGENT_START, sent).await {
            Ok(answer) => {
                self.started = Some(start);
                read(method::AGENT_START, answer)
            }
            Err(error) => {
                self.registry.forget(self.slot);
                Err(error)
            }
        }
    }

    async fn stop(&mut self) -> Result<(), AdapterError> {
        let Some(start) = self.started.take() else {
            self.registry.forget(self.slot);
            return Ok(());
        };
        let sent = json!({ "provider_id": start.provider_id, "agent_id": start.agent_id });
        let stopped = self.call(method::AGENT_STOP, sent).await;
        self.registry.forget(self.slot);
        stopped.map(|_| ())
    }

    async fn probe(&mut self) -> Result<AgentDescriptor, AdapterError> {
        let start = self.started.clone().ok_or_else(|| {
            AdapterError::HarnessUnavailable("the harness has not been started".to_owned())
        })?;
        let sent = written(method::AGENT_START, &start)?;
        let answer = self.call(method::AGENT_START, sent).await?;
        Ok(read::<AgentStarted>(method::AGENT_START, answer)?.descriptor)
    }

    async fn open_session(&mut self, spec: SessionSpec) -> Result<SessionOpened, AdapterError> {
        let thread = spec.settings.thread_id;
        let sent = written(method::SESSION_OPEN, &spec)?;
        self.registry.open_thread(self.slot, thread);
        let opened = self.call(method::SESSION_OPEN, sent).await;
        match opened {
            Ok(answer) => read(method::SESSION_OPEN, answer),
            Err(error) => {
                self.registry.close_thread(self.slot, &thread);
                Err(error)
            }
        }
    }

    async fn close_session(&mut self, thread: ThreadId) -> Result<(), AdapterError> {
        let closed = self
            .call(method::SESSION_CLOSE, json!({ "thread_id": thread }))
            .await;
        self.registry.close_thread(self.slot, &thread);
        closed.map(|_| ())
    }

    async fn inspect_input_prefix(
        &mut self,
        source: orch_agent_contract::history::NativePrefixSource,
        before_input_ref: &str,
    ) -> Result<orch_agent_contract::history::NativeInputPrefix, AdapterError> {
        let answer = self
            .call(
                method::SESSION_PREFIX_INSPECT,
                json!({"source":source,"before_input_ref":before_input_ref}),
            )
            .await?;
        read(method::SESSION_PREFIX_INSPECT, answer)
    }

    async fn open_input_prefix(
        &mut self,
        spec: SessionSpec,
        prefix: orch_agent_contract::history::NativeInputPrefix,
    ) -> Result<orch_agent_contract::history::NativePrefixOpened, AdapterError> {
        let thread = spec.settings.thread_id;
        self.registry.open_thread(self.slot, thread);
        let result = self
            .call(
                method::SESSION_PREFIX_OPEN,
                json!({"spec":spec,"prefix":prefix}),
            )
            .await;
        match result {
            Ok(result) => read(method::SESSION_PREFIX_OPEN, result),
            Err(error) => {
                self.registry.close_thread(self.slot, &thread);
                Err(error)
            }
        }
    }

    async fn resume_continuation(
        &mut self,
        spec: SessionSpec,
        source: orch_agent_contract::history::NativePrefixSource,
        replace_segment_id: Option<Id>,
    ) -> Result<SessionOpened, AdapterError> {
        let thread = spec.settings.thread_id;
        self.registry.open_thread(self.slot, thread);
        let result = self
            .call(
                method::SESSION_CONTINUATION_RESUME,
                json!({"spec":spec,"source":source,"replace_segment_id":replace_segment_id}),
            )
            .await;
        match result {
            Ok(result) => read(method::SESSION_CONTINUATION_RESUME, result),
            Err(error) => {
                self.registry.close_thread(self.slot, &thread);
                Err(error)
            }
        }
    }

    async fn start_turn(&mut self, turn: TurnStart) -> Result<(), AdapterError> {
        let sent = written(method::TURN_START, &turn)?;
        self.call(method::TURN_START, sent).await.map(|_| ())
    }

    async fn native_actions(
        &mut self,
        thread: ThreadId,
        refresh: bool,
    ) -> Result<orch_agent_contract::native::NativeCatalog, AdapterError> {
        let answer = self
            .call(
                method::SESSION_NATIVE_ACTIONS,
                json!({"thread_id":thread,"refresh":refresh}),
            )
            .await?;
        read(method::SESSION_NATIVE_ACTIONS, answer)
    }

    async fn start_native_turn(
        &mut self,
        turn: TurnStart,
        selection: orch_agent_contract::native::NativeInvocation,
    ) -> Result<(), AdapterError> {
        let mut sent = written(method::TURN_START, &turn)?;
        sent["native_action"] = written(method::TURN_START, &selection)?;
        self.call(method::TURN_START, sent).await.map(|_| ())
    }

    async fn steer_native(
        &mut self,
        steer: TurnSteer,
        selection: orch_agent_contract::native::NativeInvocation,
    ) -> Result<(), AdapterError> {
        let mut sent = written(method::TURN_STEER, &steer)?;
        sent["native_action"] = written(method::TURN_STEER, &selection)?;
        self.call(method::TURN_STEER, sent).await.map(|_| ())
    }

    async fn steer(&mut self, steer: TurnSteer) -> Result<(), AdapterError> {
        let sent = written(method::TURN_STEER, &steer)?;
        self.ask(method::TURN_STEER, "steer", sent)
            .await
            .map(|_| ())
    }

    async fn interrupt(&mut self, thread: ThreadId, turn: TurnId) -> Result<(), AdapterError> {
        let sent = json!({ "thread_id": thread, "turn_id": turn });
        self.ask(method::TURN_INTERRUPT, "interrupt", sent)
            .await
            .map(|_| ())
    }

    async fn resolve_approval(
        &mut self,
        thread: ThreadId,
        request_ref: &str,
        decision: ApprovalDecision,
    ) -> Result<(), AdapterError> {
        let sent = json!({
            "thread_id": thread, "request_ref": request_ref,
            "decision": written(method::APPROVAL_RESOLVE, &decision)?
        });
        self.ask(method::APPROVAL_RESOLVE, "approvals", sent)
            .await
            .map(|_| ())
    }

    async fn tool_result(
        &mut self,
        thread: ThreadId,
        call_ref: &str,
        outcome: Result<Json, ToolError>,
    ) -> Result<(), AdapterError> {
        let mut sent = json!({ "thread_id": thread, "call_ref": call_ref });
        match outcome {
            Ok(value) => sent["result"] = value,
            Err(error) => sent["error"] = written(method::ORCH_TOOL_RESULT, &error)?,
        }
        self.ask(method::ORCH_TOOL_RESULT, "orch_tools", sent)
            .await
            .map(|_| ())
    }

    async fn export_session(
        &mut self,
        thread: ThreadId,
        kind: ExportKind,
        upto_turn_ordinal: Option<u32>,
    ) -> Result<SessionArchive, AdapterError> {
        let capability = match kind {
            ExportKind::Native => "export_native",
            ExportKind::Neutral => "export_neutral",
        };
        let sent =
            json!({ "thread_id": thread, "kind": kind, "upto_turn_ordinal": upto_turn_ordinal });
        let answer = self.ask(method::SESSION_EXPORT, capability, sent).await?;
        read(method::SESSION_EXPORT, answer)
    }

    async fn resync_session(&mut self, thread: ThreadId) -> Result<u32, AdapterError> {
        let answer = self
            .ask(
                method::SESSION_RESYNC,
                "resync",
                json!({ "thread_id": thread }),
            )
            .await?;
        read_member(method::SESSION_RESYNC, answer, "items")
    }

    async fn discover_sessions(&mut self) -> Result<Vec<NativeSession>, AdapterError> {
        let sent = self.about_harness(json!({}))?;
        let answer = self
            .ask(method::AGENT_DISCOVER_SESSIONS, "discover_sessions", sent)
            .await?;
        read_member(method::AGENT_DISCOVER_SESSIONS, answer, "sessions")
    }

    async fn discover_mcp(&mut self, search: McpSearch) -> Result<Vec<McpFound>, AdapterError> {
        let sent = written(method::MCP_DISCOVER, &search)?;
        let answer = self
            .ask(method::MCP_DISCOVER, "mcp_discovery", sent)
            .await?;
        read_member(method::MCP_DISCOVER, answer, "servers")
    }

    async fn read_mcp(&mut self, request: McpImportRequest) -> Result<McpDefinition, AdapterError> {
        let sent = written(method::MCP_IMPORT, &request)?;
        let answer = self.ask(method::MCP_IMPORT, "mcp_discovery", sent).await?;
        read(method::MCP_IMPORT, answer)
    }

    async fn reload_mcp(
        &mut self,
        thread: ThreadId,
        servers: Vec<String>,
    ) -> Result<(), AdapterError> {
        self.ask(
            method::MCP_RELOAD,
            "mcp.reload",
            json!({"thread_id":thread,"servers":servers}),
        )
        .await
        .map(|_| ())
    }

    async fn login(&mut self, request: LoginRequest) -> Result<(), AdapterError> {
        let sent = self.about_harness(written(method::PROVIDER_LOGIN_START, &request)?)?;
        self.ask(method::PROVIDER_LOGIN_START, "native_provider", sent)
            .await
            .map(|_| ())
    }

    async fn login_input(&mut self, values: LoginValues) -> Result<(), AdapterError> {
        let sent = self
            .about_harness(json!({ "values": written(method::PROVIDER_LOGIN_INPUT, &values)? }))?;
        self.ask(method::PROVIDER_LOGIN_INPUT, "native_provider", sent)
            .await
            .map(|_| ())
    }

    async fn cancel_login(&mut self) -> Result<(), AdapterError> {
        let sent = self.about_harness(json!({}))?;
        self.ask(method::PROVIDER_LOGIN_CANCEL, "native_provider", sent)
            .await
            .map(|_| ())
    }

    async fn logout(&mut self) -> Result<(), AdapterError> {
        let sent = self.about_harness(json!({}))?;
        self.ask(method::PROVIDER_LOGOUT, "native_provider", sent)
            .await
            .map(|_| ())
    }

    async fn provider_facts(&mut self) -> Result<ProviderFacts, AdapterError> {
        let (provider_id, _) = self.harness()?;
        let answer = self
            .ask(
                method::PROVIDER_REFRESH,
                "provider_facts",
                json!({ "provider_id": provider_id }),
            )
            .await?;
        read_member(method::PROVIDER_REFRESH, answer, "facts")
    }

    async fn discover_models(&mut self) -> Result<Vec<ModelSuggestion>, AdapterError> {
        let (provider_id, _) = self.harness()?;
        let answer = self
            .ask(
                method::PROVIDER_DISCOVER_MODELS,
                "model_discovery",
                json!({ "provider_id": provider_id }),
            )
            .await?;
        read_member(method::PROVIDER_DISCOVER_MODELS, answer, "suggestions")
    }

    async fn issue_credential(&mut self, form: &str) -> Result<IssuedCredential, AdapterError> {
        let (provider_id, _) = self.harness()?;
        let answer = self
            .ask(
                method::PROVIDER_CREDENTIAL_ISSUE,
                "credentials",
                json!({ "provider_id": provider_id, "form": form }),
            )
            .await?;
        read(method::PROVIDER_CREDENTIAL_ISSUE, answer)
    }

    async fn set_credential(&mut self, credential: GivenCredential) -> Result<(), AdapterError> {
        let sent = self.about_harness(written(method::AGENT_CREDENTIAL_SET, &credential)?)?;
        self.ask(method::AGENT_CREDENTIAL_SET, "credentials", sent)
            .await
            .map(|_| ())
    }

    async fn return_credential(&mut self) -> Result<GivenCredential, AdapterError> {
        let sent = self.about_harness(json!({}))?;
        let answer = self
            .ask(method::AGENT_CREDENTIAL_RETURN, "credentials", sent)
            .await?;
        read(method::AGENT_CREDENTIAL_RETURN, answer)
    }

    async fn accept_credential(&mut self, credential: GivenCredential) -> Result<(), AdapterError> {
        let (provider_id, _) = self.harness()?;
        let mut sent = json!({ "provider_id": provider_id });
        merge(
            &mut sent,
            written(method::PROVIDER_CREDENTIAL_ACCEPT, &credential)?,
        );
        self.ask(method::PROVIDER_CREDENTIAL_ACCEPT, "credentials", sent)
            .await
            .map(|_| ())
    }
}

/// What makes an adapter for each harness process out of a plugin: a connector asks the factory for one when it
/// starts a harness. They all talk to the one program.
#[derive(Debug, Clone)]
pub struct AdapterFactory {
    plugin: Plugin,
    descriptor: Described<AgentDescriptor>,
}

impl AdapterFactory {
    /// The factory of the adapters that `plugin` is. The plugin must be of the kind and have said hello.
    pub fn new(plugin: Plugin) -> Result<Self, BridgeError> {
        let descriptor = Described::new(plugin.clone(), PluginKind::Adapter)?;
        // The place where the events of every bridge of this program are routed is made now, so that no bridge
        // can miss the first one.
        Registry::of(&plugin);
        Ok(Self { plugin, descriptor })
    }

    /// The name the plugin is registered by.
    #[must_use]
    pub fn name(&self) -> &str {
        self.plugin.name()
    }

    /// The harness kind the adapter drives, as its descriptor states it.
    #[must_use]
    pub fn harness_kind(&self) -> Option<String> {
        let negotiated = self.plugin.negotiated()?;
        negotiated
            .descriptor
            .get("kind")
            .and_then(Json::as_str)
            .map(str::to_owned)
    }

    /// An adapter for one harness process.
    pub fn create(&self) -> Result<AgentBridge, BridgeError> {
        Ok(self.adapter())
    }

    /// Makes an adapter from the validated declaration, retaining it while the program restarts.
    #[must_use]
    pub fn adapter(&self) -> AgentBridge {
        let registry = Registry::of(&self.plugin);
        AgentBridge {
            plugin: self.plugin.clone(),
            descriptor: self.descriptor.clone(),
            kind: interned(&self.descriptor.get().kind),
            slot: registry.new_slot(),
            registry,
            started: None,
        }
    }
}
