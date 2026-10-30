//! Tools components.
//!
//! The tools of the `orch` server, as the engine answers them. An agent's harness calls one through the connector
//! that hosts it, which turns the call into an `orch.tool_call` event of the thread's session; the engine takes the
//! call, checks it, does what it asks, and answers it once with an `orch.tool_result` command to the machine that
//! sent it ([`Engine::answer_tool_call`]). The definitions of the tools are [`definitions`], and each has its
//! behaviour here: the four that take from the shared tree ([`shared`], [`read`], [`search`], [`download`]) over the
//! tree of [`tree`], document edits ([`edit`]), one `send_message` interface with the existing chat and thread
//! handlers ([`chat_message`], [`thread_message`]), document-tree organization with preview and confirmation
//! only for deletion ([`manage`]), [`announce`] and [`request_capability`]. [`definitions::BUILT`] lists
//! them.
//!
//! **A call is checked at the moment it is made**, against the thread's settings as they are then: the tool must
//! be one the thread is offered (the resolved `orch_tools.enabled`, what is built, and no `announce` in a side
//! thread), the thread's agent must be within the rate, and the arguments must be what the tool's schema allows.
//! Each check that fails is a refusal: a result marked as an error that begins with its code and says what to do
//! instead, which the model can read and correct itself by.
//!
//! Fresh calls belong to the exact selected running turn, segment and continuation. [`source`]
//! checks that identity before dispatch and under the source-thread lock of every write. A read
//! crossing selection is refused before its answer is retained. Native effects enter the ordinary
//! durable command queue under the same lock and take part in continuation quiescence. Preparation
//! can wait on machines or blobs only outside a transaction. Completed canonical answers replay
//! before these fresh-work gates; factual late native items and usage use their ordinary ingress.
//!
//! **A call is answered once.** The same call asked again (a spool that is replayed, an adapter that says it twice)
//! is the same call: what it makes, it makes under an identifier derived from the call, so that it is made once,
//! and its completed answer is retained as that command's protected body. An explicit connector retry keeps
//! call_ref and sends that saved body under an attempt-specific command id; replaying one attempt sends once. A call that is lost
//! (the process stopped between the event and the answer) gets a connector timeout before the native deadline,
//! with a warning that a changing call may still have taken effect.
//!
//! **The item of a call is the harness's own**: the adapter writes it, as it writes every step of the agent's work,
//! and completes it with the answer. What the engine writes into threads is what the call does to them: the
//! announcement that another agent's call put there.

pub mod announce;
pub mod browser;
pub mod browser_definition;
pub mod chat_message;
pub mod definitions;
pub mod display;
pub mod documents;
pub mod download;
pub mod edit;
mod edit_import;
mod edit_source;
pub mod goal;
pub mod manage;
mod message;
pub mod ports;
pub mod read;
pub mod references;
pub mod request_capability;
pub mod search;
pub mod shared;
pub mod source;
pub mod texts;
pub mod thread_message;
pub mod tree;
pub(crate) mod workspace_files;

use crate::commands::Outgoing;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use definitions::{Code, Refusal};
use orch_agent_contract::events::OrchTool;
use orch_db::Conn;
use orch_db::queries::threads::ThreadRow;
use orch_link_contract::commands::{OrchToolResult, SerialKey, ToolError};
use orch_link_contract::events::OrchToolCall;
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, SettingDef, keys};
use orch_types::{Id, Json};
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;

/// What a call comes to: what the tool returned, or why it was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// The result of the table in the tool's definition.
    Done(Json),
    /// A refusal, with what to do instead.
    Refused(Refusal),
}

impl From<Refusal> for Answer {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

/// The code of the answer to a call whose handling failed on Orch's side. It is no refusal of the table: the model
/// cannot correct a call that was right, and is told so.
pub const UNAVAILABLE: &str = "unavailable";

/// The sentence that goes with [`UNAVAILABLE`].
const UNAVAILABLE_MESSAGE: &str = "Orch did not confirm this call's result. A change may have been carried out. Inspect the target before repeating it.";

/// The settings of a thread's place that every call is checked against.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSettings {
    /// The resolved `orch_tools.enabled`.
    pub enabled: Vec<String>,
    /// The resolved `orch_tools.calls_per_minute`.
    pub calls_per_minute: i64,
    /// The resolved `agent.messages`.
    pub messages: BTreeMap<String, String>,
}

/// One setting of a snapshot as the type the caller knows it to be.
pub(crate) fn setting<T: DeserializeOwned>(
    snapshot: &Snapshot,
    key: &str,
) -> Result<Option<T>, EngineError> {
    let def = definition_of(key)?;
    match snapshot.resolve(def).value {
        None | Some(Json::Null) => Ok(None),
        Some(found) => serde_json::from_value(found).map(Some).map_err(|error| {
            EngineError::unreadable(format!(
                "the value of {key} is not what it should be: {error}"
            ))
        }),
    }
}

/// The definition of a setting in the registry.
pub(crate) fn definition_of(key: &str) -> Result<&'static SettingDef, EngineError> {
    registry::get(key)
        .ok_or_else(|| EngineError::unreadable(format!("the setting {key} is not in the registry")))
}

impl ToolSettings {
    /// Reads them for a thread.
    pub async fn load(conn: &mut Conn, thread: Id) -> Result<Self, EngineError> {
        let wanted = [
            definition_of(keys::ORCH_TOOLS_ENABLED)?,
            definition_of(keys::ORCH_TOOLS_CALLS_PER_MINUTE)?,
            definition_of(keys::AGENT_MESSAGES)?,
        ];
        let snapshot = Snapshot::load(conn, Place::Node(thread), Wanted::Keys(&wanted)).await?;
        Ok(Self {
            enabled: setting(&snapshot, keys::ORCH_TOOLS_ENABLED)?.unwrap_or_default(),
            calls_per_minute: setting(&snapshot, keys::ORCH_TOOLS_CALLS_PER_MINUTE)?
                .unwrap_or_default(),
            messages: setting(&snapshot, keys::AGENT_MESSAGES)?.unwrap_or_default(),
        })
    }
}

/// The tools a thread's session is opened with: those the thread's settings enable and the engine has built, and
/// none where its harness cannot take an MCP server or its environment switched them off for the agents that work
/// there.
pub async fn offered_for_session(
    conn: &mut Conn,
    thread: &ThreadRow,
    harness_takes_mcp: bool,
    now: orch_types::Time,
) -> Result<Vec<OrchTool>, EngineError> {
    if !harness_takes_mcp {
        return Ok(Vec::new());
    }
    if let Some(environment) = thread.environment_id {
        let wanted = [definition_of(keys::EXEC_ORCH_TOOLS)?];
        let snapshot =
            Snapshot::load(conn, Place::Environment(environment), Wanted::Keys(&wanted)).await?;
        if setting::<bool>(&snapshot, keys::EXEC_ORCH_TOOLS)? == Some(false) {
            return Ok(Vec::new());
        }
    }
    let settings = ToolSettings::load(conn, thread.node_id).await?;
    let mut offered = definitions::offered(&settings.enabled, thread.side_of.is_some());
    if !browser::offered(conn, thread.environment_id).await? {
        offered.retain(|tool| *tool != OrchTool::UseBrowser);
    }
    if !display::offered(conn, thread.environment_id).await? {
        offered.retain(|tool| *tool != OrchTool::UseDisplay);
    }
    ports::narrow(&mut offered, ports::offered(conn, thread, now).await?);
    // The goal tool is the agent's way of reporting on a goal: only a thread that has one is given it.
    if crate::goals::offers_tool(conn, thread.node_id).await? {
        offered.push(OrchTool::Goal);
    }
    Ok(offered)
}

/// What a call needs to be answered.
#[derive(Debug, Clone)]
pub struct CallContext {
    /// The machine the call came from, and the answer goes back to.
    pub machine: Id,
    /// The thread whose agent made it.
    pub thread: Id,
    /// The turn it was made in.
    pub turn: Id,
    /// The harness's name for the call.
    pub call_ref: String,
    /// The delivery attempt, while the call_ref and effect identity stay fixed.
    pub attempt: Option<Id>,
    /// Which tool.
    pub tool: OrchTool,
    /// What it was called with.
    pub arguments: Json,
}

impl Engine {
    /// A saved answer is read only by the machine for which its delivery was first retained.
    async fn retained_tool_answer(
        &self,
        id: Id,
        machine: Id,
    ) -> Result<Option<OrchToolResult>, EngineError> {
        let Some(retained) = self.inner.commands.retained_command(id).await? else {
            return Ok(None);
        };
        if retained.machine != machine {
            return Err(EngineError::StaleToolSource);
        }
        Ok(Some(serde_json::from_value(retained.body)?))
    }

    /// Takes a call of the agent of a thread, as the event that carries it is applied. The call is answered after
    /// the transaction that applies the event has had time to commit, in a task of its own.
    pub(crate) fn take_tool_call(&self, machine: Id, thread: Id, call: &OrchToolCall) {
        // The link and the adapters' contract write the names alike.
        let Some(tool) = serde_json::to_value(call.tool)
            .ok()
            .and_then(|word| word.as_str().and_then(OrchTool::parse))
        else {
            tracing::warn!(%thread, "a call of a tool that is none of those there are");
            return;
        };
        let context = CallContext {
            machine,
            thread,
            turn: call.turn_id,
            call_ref: call.call_ref.clone(),
            attempt: call.attempt,
            tool,
            arguments: call.arguments.clone(),
        };
        let engine = self.clone();
        let grace = self.inner.config.post_commit_grace;
        #[cfg(test)]
        let task_identity = (context.thread, context.call_ref.clone());
        let task = tokio::spawn(async move {
            tokio::time::sleep(grace).await;
            if engine.is_stopped() {
                return;
            }
            if let Err(error) = engine.answer_tool_call(&context).await {
                tracing::warn!(%error, thread = %context.thread, call = %context.call_ref, "a call of an orch tool could not be answered");
            }
        });
        #[cfg(test)]
        source::tests::track_task(task_identity, task);
        #[cfg(not(test))]
        drop(task);
    }

    /// Answers a call: checks it, does what it asks, and sends the answer to the machine it came from. Asked twice
    /// for one call it does the work once; each explicit retry can receive its retained answer once.
    pub async fn answer_tool_call(&self, call: &CallContext) -> Result<(), EngineError> {
        let canonical = ids::tool_result(call.thread, &call.call_ref);
        if let Some(body) = self.retained_tool_answer(canonical, call.machine).await?
            && body.thread_id == call.thread
            && body.call_ref == call.call_ref
        {
            return self
                .send_tool_result(call.machine, body, call.attempt)
                .await;
        }
        let answer = match orch_db::audit::scope_agent_turn(
            call.thread,
            call.turn,
            // Ordinary content tools share the barrier. Granting a capability or a port opening
            // declares authority first; document creation/management choose their own write class.
            orch_db::queries::transaction_guard::request_scope(
                if matches!(
                    call.tool,
                    OrchTool::RequestCapability | OrchTool::OpenPort | OrchTool::ClosePort
                ) {
                    orch_db::queries::transaction_guard::Scope::Authority
                } else {
                    orch_db::queries::transaction_guard::Scope::Shared
                },
                // The dispatcher contains every tool's future. Allocate it once
                // before the nested task-local scopes, so they carry its pointer
                // rather than copying that large state onto each poll frame.
                Box::pin(self.decide_tool_call(call)),
            ),
        )
        .await
        {
            Ok(answer) => answer,
            Err(error @ (EngineError::StaleToolSource | EngineError::ToolCallIdentity)) => {
                Answer::Refused(Refusal::new(Code::Changed, error.to_string()))
            }
            Err(error) => {
                // A failed database step is an internal failure too: settle the native waiter
                // with the same safe refusal rather than leaving it waiting for its deadline.
                tracing::warn!(%error, thread = %call.thread, "a call of an orch tool failed");
                let body = result_of(
                    call,
                    Err(ToolError {
                        code: UNAVAILABLE.to_owned(),
                        message: UNAVAILABLE_MESSAGE.to_owned(),
                    }),
                );
                return self
                    .send_tool_result(call.machine, body, call.attempt)
                    .await;
            }
        };
        shared_measure(call, &answer);
        let body = match answer {
            Answer::Done(result) => result_of(call, Ok(result)),
            Answer::Refused(refusal) => result_of(
                call,
                Err(ToolError {
                    code: refusal.code.as_str().to_owned(),
                    message: refusal.message,
                }),
            ),
        };
        self.send_tool_result(call.machine, body, call.attempt)
            .await
    }

    /// Sends the answer to a call, under an identifier derived from the call.
    async fn send_tool_result(
        &self,
        machine: Id,
        body: OrchToolResult,
        attempt: Option<Id>,
    ) -> Result<(), EngineError> {
        // Always keep the canonical completed answer before its first delivery attempt. Its body survives expiry.
        let canonical = ids::tool_result(body.thread_id, &body.call_ref);
        let command = Outgoing::new(canonical, machine, SerialKey::Thread(body.thread_id), &body)?;
        self.inner.commands.issue(command).await?;
        // A racing first execution also uses the canonical winner's exact answer.
        let saved = self
            .retained_tool_answer(canonical, machine)
            .await?
            .ok_or_else(|| {
                EngineError::unreadable("The canonical tool answer has not been retained.")
            })?;
        #[cfg(test)]
        source::tests::answer_retained(saved.thread_id, canonical).await;
        if let Some(attempt) = attempt
            && saved.attempt != Some(attempt)
        {
            let id = ids::derived(
                "orch.tool_result.attempt",
                &[
                    canonical.as_bytes().as_slice(),
                    attempt.as_bytes().as_slice(),
                ],
            );
            let mut saved = saved;
            saved.attempt = Some(attempt);
            self.inner
                .commands
                .issue(Outgoing::new(
                    id,
                    machine,
                    SerialKey::Thread(saved.thread_id),
                    &saved,
                )?)
                .await?;
        }
        if source::answered(self, canonical).await.is_err() {
            // Delivery is already durably retained. An incomplete admitted receipt keeps
            // its protections; the bounded source reaper rejoins this exact canonical body.
            // Failure before receipt admission has no metadata/effect obligation to release.
            tracing::warn!(
                phase = "tool_answer_checkpoint",
                "A retained native tool answer awaits its durable receipt checkpoint."
            );
        }
        Ok(())
    }

    /// What a call comes to, the checks first and the tool after.
    async fn decide_tool_call(&self, call: &CallContext) -> Result<Answer, EngineError> {
        let (source, thread) = {
            let mut tx = self.inner.db.begin().await?;
            let admitted = source::Source::load(&mut tx, call).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            admitted
        };
        source
            // The captured-source scope is another owner of the operation
            // future; keep its task-local poll frames independent of its size.
            .scope(Box::pin(async {
                let answer = self.decide_admitted_tool_call(call, &thread).await?;
                // Reads may perform several bounded metadata/blob/native observations. If selection
                // changed between them, none of that newer projection becomes this old call's answer.
                if reading_call(call) && matches!(&answer, Answer::Done(_)) {
                    #[cfg(test)]
                    source::tests::pause(call, source::tests::Point::Read).await;
                    let mut tx = self.inner.db.begin().await?;
                    source::check(&mut tx).await?;
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                }
                Ok(answer)
            }))
            .await
    }

    async fn decide_admitted_tool_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
    ) -> Result<Answer, EngineError> {
        let settings = {
            let mut conn = self.inner.db.acquire().await?;
            ToolSettings::load(&mut conn, call.thread).await?
        };
        let mut offered = definitions::offered(&settings.enabled, thread.side_of.is_some());
        let mut conn = self.inner.db.acquire().await?;
        ports::narrow(
            &mut offered,
            ports::offered(&mut conn, thread, self.inner.clock.time()).await?,
        );
        if crate::goals::offers_tool(&mut conn, call.thread).await? {
            offered.push(OrchTool::Goal);
        }
        drop(conn);
        if !offered.contains(&call.tool) {
            return Ok(Refusal::invalid(not_offered(call.tool, &offered)).into());
        }
        let admitted = crate::engine::lock(&self.inner.call_rate).admit(
            call.thread,
            &call.call_ref,
            self.inner.clock.now(),
            settings.calls_per_minute,
        );
        if let Err(seconds) = admitted {
            return Ok(Refusal::new(
                Code::RateLimited,
                format!(
                    "You have made more calls of these tools than a minute allows ({}). Wait {seconds} seconds, and try again.",
                    settings.calls_per_minute
                ),
            )
            .into());
        }
        if let Err(refusal) = definitions::check_arguments(call.tool, &call.arguments) {
            return Ok(refusal.into());
        }
        source::admit(self, call).await?;
        // Each tool's future is boxed: the dispatcher's own state and poll frame stay the size of a pointer
        // per tool rather than of the largest tool (debug builds copy that state through the stack).
        match call.tool {
            OrchTool::UseBrowser => Box::pin(self.use_browser_call(call, thread)).await,
            OrchTool::UseDisplay => Box::pin(self.use_display_call(call,thread)).await,
            OrchTool::ListShared => Box::pin(self.list_shared_call(call, thread)).await,
            OrchTool::ReadShared => Box::pin(self.read_shared_call(call, thread)).await,
            OrchTool::SearchShared => Box::pin(self.search_shared_call(call, thread)).await,
            OrchTool::DownloadShared => Box::pin(self.download_shared_call(call, thread)).await,
            OrchTool::SendMessage => Box::pin(self.send_message_call(call, thread, &settings)).await,
            OrchTool::EditDocument => Box::pin(self.edit_document_call(call, thread)).await,
            OrchTool::ManageShared => Box::pin(self.manage_shared_call(call, thread)).await,
            OrchTool::Announce => Box::pin(self.announce_call(call, thread, &settings)).await,
            OrchTool::SendChatMessage | OrchTool::SendThreadMessage => {
                Ok(Refusal::invalid("Use send_message with target.kind and target.path. The old message names are retained only in conversation history.").into())
            }
            OrchTool::RequestCapability => {
                Box::pin(self.request_capability_call(call, thread, &settings)).await
            }
            OrchTool::Goal => Box::pin(self.goal_call(call, thread)).await,
            // What these do is the API's, reached through the hook it attaches (`ports`); a thread is offered them
            // only once they are in `definitions::BUILT`.
            OrchTool::ListPorts
            | OrchTool::OpenPort
            | OrchTool::ClosePort
            | OrchTool::ListPortConnections => Box::pin(self.port_tool_call(call, thread)).await,
        }
    }
}

fn reading_call(call: &CallContext) -> bool {
    matches!(
        call.tool,
        OrchTool::ListShared
            | OrchTool::ReadShared
            | OrchTool::SearchShared
            | OrchTool::ListPorts
            | OrchTool::ListPortConnections
    ) || matches!(call.tool, OrchTool::UseBrowser | OrchTool::UseDisplay)
        && call.arguments.get("command").and_then(Json::as_str) == Some("list")
        || call.tool == OrchTool::ManageShared
            && call.arguments.get("command").and_then(Json::as_str) == Some("delete")
            && call.arguments.get("confirm").is_none_or(Json::is_null)
}

fn shared_measure(call: &CallContext, answer: &Answer) {
    let tool = match call.tool {
        OrchTool::ListShared => "list",
        OrchTool::ReadShared => "read",
        OrchTool::SearchShared => "search",
        OrchTool::DownloadShared => "download",
        OrchTool::EditDocument => {
            if call.arguments.get("command").and_then(Json::as_str) == Some("create") {
                "write"
            } else {
                "edit"
            }
        }
        OrchTool::ManageShared => "write",
        OrchTool::SendMessage | OrchTool::Announce => "send",
        _ => return,
    };
    let result = match answer {
        Answer::Done(body) if body.get("truncated").and_then(Json::as_bool) == Some(true) => {
            "truncated"
        }
        Answer::Done(_) => "ok",
        Answer::Refused(refusal) if refusal.code == Code::RateLimited => "rate_limited",
        Answer::Refused(_) => "refused",
    };
    metrics::counter!("orch_shared_calls_total","tool"=>tool,"result"=>result).increment(1);
}

/// What is said to a model that calls a tool its thread does not have.
fn not_offered(tool: OrchTool, offered: &[OrchTool]) -> String {
    let names: Vec<&str> = offered.iter().map(|tool| tool.name()).collect();
    if names.is_empty() {
        format!(
            "{} is not a tool you have here: you have none of Orch's tools in this thread.",
            tool.name()
        )
    } else {
        format!(
            "{} is not a tool you have here. You have: {}.",
            tool.name(),
            names.join(", ")
        )
    }
}

fn result_of(call: &CallContext, outcome: Result<Json, ToolError>) -> OrchToolResult {
    match outcome {
        Ok(result) => OrchToolResult {
            thread_id: call.thread,
            call_ref: call.call_ref.clone(),
            attempt: call.attempt,
            result: Some(result),
            error: None,
        },
        Err(error) => OrchToolResult {
            thread_id: call.thread,
            call_ref: call.call_ref.clone(),
            attempt: call.attempt,
            result: None,
            error: Some(error),
        },
    }
}
