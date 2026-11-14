//! The built-in dialogue program, hosted in the control plane without an environment.
//!
//! The transport supplies the exact admitted principal through a private TurnHost.
//! It reuses the API's operation implementation for tools; this engine owns the
//! model/tool loop, durable provider commands and ordinary usage attribution.

pub mod admission;
pub mod command;
pub mod context;
pub mod model;

use crate::commands::{Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::turns::{OwnPurpose, OwnUse};
use async_trait::async_trait;
use orch_link_contract::commands::{ProviderGenerate, ProviderGenerateResult, SerialKey};
use orch_model_contract::generation::{
    GenerationInput, GenerationMessage, GenerationOutput, GenerationPart, GenerationRole,
    GenerationStop,
};
use orch_types::Id;

/// The built-in program's stable identity. It uses API providers and declares no
/// environment, native process, workspace, terminal or credential delivery.
pub const PROGRAM: &str = orch_api_contract::threads::BuiltinThreadProgramId::Orch.as_str();

/// Declared execution needs, shared by creation and kept-thread placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Program {
    /// Stable built-in identity.
    pub name: &'static str,
    /// Whether placement requires an environment.
    pub environment: bool,
    /// Whether a native harness process must be started.
    pub native_harness: bool,
}

/// The program can run in a personal hidden thread or an ordinary Private thread.
pub const BUILTIN: Program = Program {
    name: PROGRAM,
    environment: orch_api_contract::threads::BuiltinThreadProgramId::Orch.requires_environment(),
    native_harness: orch_api_contract::threads::BuiltinThreadProgramId::Orch
        .requires_native_harness(),
};

/// Inherited bounds, resolved before a turn starts. Hitting a bound leaves the
/// retained conversation intact and the person can ask a narrower follow-up.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Model steps in one user turn.
    pub steps: u32,
    /// Total tool invocations in that turn.
    pub tools: u32,
    /// Bytes from any one tool result.
    pub tool_bytes: usize,
    /// Total tool-result bytes sent during that turn, at most: a model whose window leaves less holds less
    /// ([`Step::tool_bytes`]).
    pub turn_tool_bytes: usize,
}

/// An authorized, retained call prepared using current model, payer and caps.
#[derive(Debug, Clone)]
pub struct Step {
    /// A tool-free context summary, counted through this same Ask usage path.
    pub summarizing: bool,
    /// Retained turn that owns this request.
    pub turn: Id,
    /// Its no-environment segment.
    pub segment: Id,
    /// Durable command identity, chosen once by the host.
    pub id: Id,
    /// The fixed governing root organization.
    pub root: Id,
    /// Hidden or kept thread.
    pub thread: Id,
    /// Asking person, independent of the payer.
    pub person: Id,
    /// Credential-keeping machine.
    pub machine: Id,
    /// Fully constrained model request, without a credential.
    pub request: ProviderGenerate,
    /// The most bytes of read-tool results this turn may hold for the model of this step: the reserve its
    /// request was fitted to, which a turn that holds more would overrun.
    pub tool_bytes: usize,
}

/// A completed tool response; its operation/reference witness is retained by
/// the host so that every resumed model request checks its source again.
#[derive(Debug, Clone)]
pub struct ToolAnswer {
    /// Bounded untrusted material and canonical source links.
    pub content: String,
    /// Whether the tool was refused or failed.
    pub is_error: bool,
}

/// The API's request-local authority and durable conversation adapter.
/// Implementations must not mint a token or reconstruct a caller from a user ID.
#[async_trait]
pub trait TurnHost: Send + Sync {
    /// Serialize the selected turn with Stop through the actual queue insert,
    /// using the original request's native before-effect boundary.
    async fn issue(&self, engine: &Engine, command: Outgoing) -> Result<(), EngineError>;
    /// Restore the original admitted identity, refresh every earlier tool result,
    /// then resolve today's model, payer, constraints, Use and usage caps. Retain
    /// the prepared command before returning, so provider deltas have an owner.
    async fn prepare(&self, input: &GenerationInput) -> Result<Step, EngineError>;
    /// Stop is checked before every tool as well as before each model step.
    async fn check_active(&self) -> Result<(), EngineError>;
    /// Persist the finished assistant step before executing its requested tools.
    async fn assistant(&self, step: &Step, output: &GenerationOutput) -> Result<(), EngineError>;
    /// Retain a visible summary before preparing the next step from the native journal.
    async fn summary(&self, step: &Step, output: &GenerationOutput) -> Result<(), EngineError>;
    /// Dispatch through the API's own read implementation as the admitted caller.
    async fn tool(
        &self,
        id: &str,
        name: &str,
        arguments: &orch_types::Json,
    ) -> Result<ToolAnswer, EngineError>;
    /// Retain the answer and its source witness for reauthorization on resume.
    async fn tool_answer(&self, id: &str, answer: &ToolAnswer) -> Result<Option<Id>, EngineError>;
}

/// Apply the same UTF-8 byte bound to first delivery and to resumed current
/// projections, retaining an explicit indication that a narrower read is needed.
pub fn bound_tool_result(content: &mut String, limit: usize) {
    if content.len() <= limit {
        return;
    }
    const NOTE: &str = "\n[Result shortened; use its canonical link or a narrower read.]";
    if limit < NOTE.len() {
        content.clear();
        content.push_str(&NOTE[..limit]);
        return;
    }
    let mut end = limit - NOTE.len();
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    content.truncate(end);
    content.push_str(NOTE);
}

/// Recover a bounded retained dialogue at user-turn boundaries. A stopped turn
/// can have a completed assistant tool request without all of its tool answers;
/// carry an explicit interruption answer rather than replaying those operations.
/// Leading fragments from an older journal page have no complete user turn and
/// are omitted. This is dialogue repair, never a source-read authorization.
#[must_use]
pub fn resume_messages(messages: Vec<GenerationMessage>) -> Vec<GenerationMessage> {
    fn close(out: &mut Vec<GenerationMessage>, pending: &mut Vec<String>) {
        if pending.is_empty() {
            return;
        }
        out.push(GenerationMessage {
            role: GenerationRole::User,
            content: pending
                .drain(..)
                .map(|call_id| GenerationPart::ToolResult {
                    call_id,
                    content: "This earlier operation was interrupted; no result is available."
                        .into(),
                    is_error: true,
                    witness: None,
                })
                .collect(),
            continuation: vec![],
        });
    }
    let mut out = Vec::new();
    let mut pending = Vec::new();
    let mut started = false;
    for mut message in messages {
        let user_turn = message.role == GenerationRole::User
            && message.content.iter().any(|part| {
                matches!(
                    part,
                    GenerationPart::Text { .. }
                        | GenerationPart::ImageRef { .. }
                        | GenerationPart::Image { .. }
                )
            });
        if !started && !user_turn {
            continue;
        }
        started = true;
        if user_turn || message.role == GenerationRole::Assistant {
            close(&mut out, &mut pending);
        }
        if message.role == GenerationRole::Assistant {
            for part in &message.content {
                if let GenerationPart::ToolCall { id, .. } = part {
                    pending.push(id.clone());
                }
            }
        } else {
            message.content.retain(|part| {
                if let GenerationPart::ToolResult { call_id, .. } = part {
                    let Some(index) = pending.iter().position(|id| id == call_id) else {
                        return false;
                    };
                    pending.remove(index);
                }
                true
            });
        }
        if !message.content.is_empty() || !message.continuation.is_empty() {
            out.push(message);
        }
    }
    close(&mut out, &mut pending);
    out
}

/// Preserve readable conversation when opaque state is no longer valid for its reader or sources.
/// Tool history becomes untrusted assistant-side transcript, without native call/signature pairs.
pub fn portable_messages(input: &mut GenerationInput) {
    for message in &mut input.messages {
        let results_only = !message.content.is_empty()
            && message
                .content
                .iter()
                .all(|part| matches!(part, GenerationPart::ToolResult { .. }));
        message.continuation.clear();
        message.content = std::mem::take(&mut message.content)
            .into_iter()
            .map(|part| match part {
                GenerationPart::ToolCall {
                    name, arguments, ..
                } => GenerationPart::Text {
                    text: format!("Earlier read request {name}: {arguments}"),
                },
                GenerationPart::ToolResult {
                    content, is_error, ..
                } => GenerationPart::Text {
                    text: format!(
                        "Untrusted {}source result: {content}",
                        if is_error { "unavailable " } else { "" }
                    ),
                },
                other => other,
            })
            .collect();
        if results_only {
            message.role = GenerationRole::Assistant;
        }
    }
    input.messages.retain(|message| !message.content.is_empty());
}

impl Engine {
    /// Installs the control-plane lifetime sweep once for this engine instance.
    pub fn install_ask_runtime(&self) -> bool {
        self.inner
            .ask_runtime
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
    }

    /// Whether the API host should stop renewing control-plane runner leases.
    #[must_use]
    pub fn ask_runtime_stopped(&self) -> bool {
        self.is_stopped()
    }

    /// The existing command writer keeps the caller's once-only final check.
    pub async fn queue_ask_command(
        &self,
        command: Outgoing,
        before: crate::commands::BeforeCommand<'_>,
    ) -> Result<(), EngineError> {
        self.inner.commands.issue_before(command, before).await?;
        Ok(())
    }
    /// Uses the same provider starter as ordinary turns without preparing an environment.
    pub async fn start_ask_provider(&self, thread: Id, provider: Id) -> Result<(), EngineError> {
        match self.ensure_provider_started(thread, provider).await? {
            crate::providers::Keeping::Up => Ok(()),
            _ => Err(EngineError::unreadable(
                "the Ask provider could not be started",
            )),
        }
    }

    /// Stop uses the exact retained command IDs, never a process or credential-wide match.
    pub async fn cancel_ask_calls(&self, calls: &[Id]) -> Result<(), EngineError> {
        for call in calls {
            if self.inner.commands.status(*call).await?.is_some() {
                self.inner.commands.cancel(*call).await?;
            }
        }
        Ok(())
    }

    /// Reduce an already admitted interruption of a real built-in turn.
    /// The caller's native stop/change writer marked it first; no new directing authority is created.
    pub(crate) async fn finish_marked_ask_interrupt(
        &self,
        thread: Id,
        turn: &orch_db::queries::threads::TurnRow,
    ) -> Result<bool, EngineError> {
        use orch_db::queries::{ask as rows, threads};
        let mut tx = self.inner.db.begin().await?;
        if rows::segment_binding(&mut tx, thread, turn.segment_id)
            .await?
            .is_none()
        {
            return Ok(false);
        }
        let Some(row) = rows::get(&mut tx, thread, true).await? else {
            return Ok(true);
        };
        let current = threads::turn(&mut tx, turn.id).await?;
        if row.turn != Some(turn.id)
            || row.state != "running"
            || current
                .as_ref()
                .is_none_or(|turn| !turn.is_active() || turn.interrupted_by.is_none())
        {
            return Ok(true);
        }
        let calls = rows::calls(&mut tx, turn.id).await?;
        let now = self.inner.clock.now();
        let mut unit = crate::unit::Unit::new(&mut tx, now);
        let seq = unit.stamp(thread).await?;
        threads::finish_turn(unit.conn, turn.id, "interrupted", None, now, seq).await?;
        threads::settle_items_of_turn(unit.conn, turn.id, "cancelled", now, seq).await?;
        rows::finish(unit.conn, thread, turn.id).await?;
        // The native history projection supplies settled items; a reset carries no stale preview.
        rows::emit(unit.conn, thread, &serde_json::json!({"kind":"reset"})).await?;
        unit.wake(thread);
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        self.forget_cap_check(thread);
        self.room_may_have_been_made();
        self.cancel_ask_calls(&calls).await?;
        Ok(true)
    }

    /// A cap ends the exact built-in turn without a native harness or lease.
    pub(crate) async fn stop_ask_for_cap(
        &self,
        thread: Id,
        turn: Id,
        message: &str,
    ) -> Result<(), EngineError> {
        use orch_db::queries::{ask as rows, threads};
        use serde_json::json;
        let mut tx = self.inner.db.begin().await?;
        let Some(row) = rows::get(&mut tx, thread, true).await? else {
            return Ok(());
        };
        if row.turn != Some(turn) || row.state != "running" {
            return Ok(());
        }
        let Some(current) = threads::turn(&mut tx, turn)
            .await?
            .filter(|turn| turn.is_active())
        else {
            return Ok(());
        };
        let calls = rows::calls(&mut tx, turn).await?;
        let now = self.inner.clock.now();
        let mut unit = crate::unit::Unit::new(&mut tx, now);
        let seq = unit.stamp(thread).await?;
        threads::set_interrupt_cause(unit.conn, turn, "usage_limit").await?;
        let error = crate::turns::error_json(crate::caps::CAP_REACHED, message, false);
        threads::finish_turn(unit.conn, turn, "interrupted", Some(&error), now, seq).await?;
        threads::settle_items_of_turn(unit.conn, turn, "cancelled", now, seq).await?;
        crate::ingest::add_error_item(
            &mut unit,
            thread,
            current.segment_id,
            Some(turn),
            crate::caps::CAP_REACHED,
            message,
            false,
        )
        .await?;
        rows::finish(unit.conn, thread, turn).await?;
        rows::emit(
            unit.conn,
            thread,
            &json!({"kind":"error","code":crate::caps::CAP_REACHED,"message":message}),
        )
        .await?;
        // A snapshot rechecks current retention and governance before the API renders its header.
        rows::emit(unit.conn, thread, &json!({"kind":"reset"})).await?;
        unit.stamp(thread).await?;
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        self.forget_cap_check(thread);
        self.room_may_have_been_made();
        self.cancel_ask_calls(&calls).await
    }

    /// Uses the same masking boundary as other owned model calls.
    pub async fn mask_ask(
        &self,
        conn: &mut orch_db::Conn,
        root: Id,
        input: &mut GenerationInput,
    ) -> Result<bool, EngineError> {
        // Wire discriminators, tool schemas and signed continuation are protocol
        // control data. Mask conversation data without corrupting those fields.
        let values: Vec<_> = input
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .map(|part| match part {
                GenerationPart::Text { text }
                | GenerationPart::ToolResult { content: text, .. } => {
                    orch_types::Json::String(text.clone())
                }
                GenerationPart::ToolCall { arguments, .. } => arguments.clone(),
                GenerationPart::ImageRef { .. } | GenerationPart::Image { .. } => {
                    orch_types::Json::Null
                }
            })
            .collect();
        let original = orch_types::Json::Array(values);
        let masked = crate::raw_events::mask_summary(conn, root, root, None, original.clone())
            .await?
            .ok_or_else(|| EngineError::unreadable("Ask redaction is unavailable"))?;
        let changed = masked != original;
        let mut values = masked
            .as_array()
            .ok_or_else(|| EngineError::unreadable("Ask redaction changed its shape"))?
            .iter();
        for part in input
            .messages
            .iter_mut()
            .flat_map(|message| &mut message.content)
        {
            let value = values
                .next()
                .ok_or_else(|| EngineError::unreadable("Ask redaction lost a part"))?;
            match part {
                GenerationPart::Text { text }
                | GenerationPart::ToolResult { content: text, .. } => {
                    *text = value
                        .as_str()
                        .ok_or_else(|| {
                            EngineError::unreadable("Ask text redaction changed its shape")
                        })?
                        .into()
                }
                GenerationPart::ToolCall { arguments, .. } => *arguments = value.clone(),
                GenerationPart::ImageRef { .. } | GenerationPart::Image { .. } => {}
            }
        }
        Ok(changed)
    }

    pub(crate) async fn on_ask_delta(
        &self,
        unit: &mut crate::unit::Unit<'_>,
        provider: Id,
        delta: &orch_link_contract::events::ProviderGenerationDelta,
    ) -> Result<(), EngineError> {
        if let Some((thread, item)) = orch_db::queries::ask::delta(
            unit.conn,
            provider,
            delta.command_id,
            delta.offset,
            &delta.text,
        )
        .await?
        {
            let data = orch_api_contract::ask::AskEventData::Delta {
                message_id: item,
                offset: delta.offset,
                text: delta.text.clone(),
            };
            orch_db::queries::ask::emit(unit.conn, thread, &serde_json::to_value(data)?).await?;
        }
        Ok(())
    }

    /// Execute a bounded user turn of the built-in program. No environment or
    /// native harness enters this path; inference still runs at the key keeper.
    pub async fn run_ask(
        &self,
        host: &dyn TurnHost,
        mut input: GenerationInput,
        limits: Limits,
    ) -> Result<GenerationStop, EngineError> {
        if limits.steps == 0
            || limits.tools == 0
            || limits.tool_bytes == 0
            || limits.turn_tool_bytes < limits.tool_bytes
        {
            return Err(EngineError::unreadable("the Ask loop limits are invalid"));
        }
        let mut calls = 0u32;
        let mut tool_bytes = 0usize;
        for _ in 0..limits.steps {
            host.check_active().await?;
            let step = host.prepare(&input).await?;
            if (!step.summarizing && step.request.input.tools != input.tools)
                || (step.summarizing && !step.request.input.tools.is_empty())
            {
                return Err(EngineError::unreadable(
                    "Ask tool declarations changed during preparation",
                ));
            }
            let outgoing = Outgoing::new(step.id, step.machine, SerialKey::None, &step.request)
                .map_err(|error| EngineError::unreadable(error.to_string()))?;
            host.issue(self, outgoing).await?;
            let outcome = self
                .inner
                .commands
                .wait(step.id)
                .await
                .map_err(|error| EngineError::unreadable(error.to_string()))?;
            let answer: ProviderGenerateResult = match outcome {
                Outcome::Succeeded(value) => serde_json::from_value(value)?,
                Outcome::Failed(error) => {
                    self.count_failed_ask_step(&step, &error).await?;
                    return Err(EngineError::unreadable(error.detail));
                }
                Outcome::Cancelled => return Err(EngineError::unreadable("Ask was stopped")),
                Outcome::Expired => {
                    return Err(EngineError::unreadable("the Ask provider request expired"));
                }
            };
            let may_continue = self.count_ask_step(&step, &answer).await?;
            answer
                .output
                .validate()
                .map_err(|_| EngineError::unreadable("the model returned invalid tool calls"))?;
            host.check_active().await?;
            if step.summarizing {
                host.summary(&step, &answer.output).await?;
            } else {
                host.assistant(&step, &answer.output).await?;
            }
            if !may_continue {
                return Err(EngineError::unreadable("An Ask usage cap has been reached"));
            }
            if answer.output.stop == GenerationStop::Refused {
                return Err(EngineError::unreadable("The model refused this question"));
            }
            if step.summarizing {
                continue;
            }
            input.messages.push(answer.output.message.clone());
            if answer.output.stop != GenerationStop::Tools {
                return Ok(answer.output.stop);
            }
            let mut results = Vec::new();
            // Provider call IDs belong to one assistant step; the host journals
            // a separate logical witness when a later step reuses the same ID.
            let mut call_ids = std::collections::HashSet::new();
            for part in &answer.output.message.content {
                let GenerationPart::ToolCall {
                    id,
                    name,
                    arguments,
                } = part
                else {
                    continue;
                };
                if !call_ids.insert(id.clone()) {
                    return Err(EngineError::unreadable(
                        "the model reused a tool call identity",
                    ));
                }
                host.check_active().await?;
                calls = calls
                    .checked_add(1)
                    .ok_or_else(|| EngineError::unreadable("the Ask tool budget ended"))?;
                if calls > limits.tools {
                    return Err(EngineError::unreadable("the Ask tool budget ended"));
                }
                let mut result = if input.tools.iter().any(|tool| tool.name == *name) {
                    host.tool(id, name, arguments).await?
                } else {
                    ToolAnswer {
                        content: "This read tool is not available.".into(),
                        is_error: true,
                    }
                };
                bound_tool_result(&mut result.content, limits.tool_bytes);
                tool_bytes = tool_bytes.saturating_add(result.content.len());
                if tool_bytes > limits.turn_tool_bytes.min(step.tool_bytes) {
                    return Err(EngineError::unreadable("the Ask source budget ended"));
                }
                let witness = host.tool_answer(id, &result).await?;
                results.push(GenerationPart::ToolResult {
                    call_id: id.clone(),
                    content: result.content,
                    is_error: result.is_error,
                    witness,
                });
            }
            input.messages.push(GenerationMessage {
                role: GenerationRole::User,
                content: results,
                continuation: vec![],
            });
        }
        Err(EngineError::unreadable(
            "the Ask step budget ended; continue with a narrower question",
        ))
    }

    async fn count_ask_step(
        &self,
        step: &Step,
        answer: &ProviderGenerateResult,
    ) -> Result<bool, EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let room = self.count_ask_step_in(&mut tx, step, answer).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(room)
    }

    /// Count the actual retained reply even after its controller outcome became
    /// cancelled or expired. The recipient and all attribution come from the
    /// original private journal, never from the replying machine's claimed IDs.
    pub async fn reconcile_ask_reply(
        &self,
        conn: &mut orch_db::Conn,
        machine: Id,
        id: Id,
        reply: &Result<orch_types::Json, orch_link_contract::commands::CommandError>,
    ) -> Result<(), EngineError> {
        let Some(call) = orch_db::queries::ask::call(conn, id, machine).await? else {
            return Ok(());
        };
        let request: ProviderGenerate = serde_json::from_value(call.request)?;
        if request.provider_id != call.provider_id {
            return Err(EngineError::unreadable(
                "the Ask provider journal no longer matches its issuer",
            ));
        }
        let answer = match reply {
            Ok(value) => Some(serde_json::from_value::<ProviderGenerateResult>(
                value.clone(),
            )?),
            Err(error) => failed_answer(error)?,
        };
        let Some(answer) = answer else {
            return Ok(());
        };
        let step = Step {
            summarizing: false,
            id: call.id,
            root: call.root_org_id,
            thread: call.thread_id,
            turn: call.turn_id,
            segment: call.segment_id,
            person: call.person_id,
            machine: call.machine_id,
            request,
            // This step only counts an answer that came after its turn; it holds no tool results.
            tool_bytes: 0,
        };
        self.count_ask_step_in(conn, &step, &answer)
            .await
            .map(|_| ())
    }

    async fn count_ask_step_in(
        &self,
        conn: &mut orch_db::Conn,
        step: &Step,
        answer: &ProviderGenerateResult,
    ) -> Result<bool, EngineError> {
        // Factual accounting of an already issued request survives soft deletion
        // of its governing root; this opens no new model or source operation.
        orch_db::queries::trace::read_scope(
            &[step.root],
            self.count_ask_step_retained(conn, step, answer),
        )
        .await
    }

    async fn count_ask_step_retained(
        &self,
        conn: &mut orch_db::Conn,
        step: &Step,
        answer: &ProviderGenerateResult,
    ) -> Result<bool, EngineError> {
        let tokens = answer.usage;
        self.count_own_use_context(
            conn,
            &OwnUse {
                purpose: OwnPurpose::Ask,
                timing: Some(&answer.timing),
                id: step.id,
                at: answer
                    .timing
                    .end
                    .map_or_else(|| self.inner.clock.now(), |time| time.get()),
                topic: step.root,
                thread: Some(step.thread),
                provider: step.request.provider_id,
                entry: None,
                person: Some(step.person),
                model: &step.request.model_name,
                protocol: Some(&step.request.protocol),
                tokens: orch_db::usage::ledger::Tokens {
                    input: tokens.input_tokens,
                    cached_input: tokens.cached_input_tokens,
                    cache_write: tokens.cache_write_tokens,
                    output: tokens.output_tokens,
                    reasoning: tokens.reasoning_tokens,
                    total: tokens.total_tokens,
                },
                request_ref: answer.request_ref.as_deref(),
            },
            Some((step.turn, step.segment)),
        )
        .await?;
        // Count, warn and audit in the same transaction. This also covers a
        // final answer which has no following model step to perform admission.
        let tier = orch_model_contract::parameters::requested_tier_for_caps(
            &step.request.protocol,
            step.request
                .params
                .as_ref()
                .unwrap_or(&orch_types::Json::Null),
        );
        let room = self
            .room_for_ask(
                conn,
                step.root,
                step.request.provider_id,
                &step.request.model_name,
                tier.as_deref(),
                step.person,
                Some(step.turn),
            )
            .await?;
        Ok(room)
    }

    async fn count_failed_ask_step(
        &self,
        step: &Step,
        error: &orch_link_contract::commands::CommandError,
    ) -> Result<(), EngineError> {
        let Some(answer) = failed_answer(error)? else {
            return Ok(());
        };
        self.count_ask_step(step, &answer).await.map(|_| ())
    }
}

pub(crate) fn failed_answer(
    error: &orch_link_contract::commands::CommandError,
) -> Result<Option<ProviderGenerateResult>, EngineError> {
    let Some(raw) = error
        .details
        .as_ref()
        .and_then(|details| details.get("model_request"))
    else {
        return Ok(None);
    };
    let request: orch_agent_contract::requests::UsageRequest = serde_json::from_value(raw.clone())?;
    request.check().map_err(EngineError::unreadable)?;
    let mut timing = match error
        .details
        .as_ref()
        .and_then(|details| details.get("request_timing"))
    {
        Some(timing) => serde_json::from_value(timing.clone())?,
        None => request.measured(),
    };
    if let Some(quotes) = raw.get("native_prices") {
        timing.native_prices = serde_json::from_value(quotes.clone())?;
        if timing.native_prices.contains_key("web_search") && timing.web_searches.is_unsupported() {
            timing.web_searches = orch_api_contract::usage::UsageMeasurement::Empty;
        }
        if timing.native_prices.contains_key("page_fetch") && timing.web_fetches.is_unsupported() {
            timing.web_fetches = orch_api_contract::usage::UsageMeasurement::Empty;
        }
    }
    if let Some(tier) = raw.get("requested_service_tier") {
        timing.requested_service_tier = tier.as_str().map(str::to_owned);
    }
    if let Some(value) = raw.get("reported_service_tier") {
        timing.reported_service_tier =
            value
                .as_str()
                .map_or(orch_api_contract::usage::UsageMeasurement::Empty, |value| {
                    orch_api_contract::usage::UsageMeasurement::Told {
                        value: value.into(),
                    }
                });
    }
    let answer = ProviderGenerateResult {
        native_result_blob: None,
        output: GenerationOutput {
            native_result: serde_json::Value::Null,
            message: GenerationMessage {
                role: GenerationRole::Assistant,
                content: vec![],
                continuation: vec![],
            },
            stop: GenerationStop::Complete,
        },
        usage: request.tokens.usage(),
        timing,
        request_ref: request.provider_request_id,
    };
    Ok(Some(answer))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a deciding case fails by panicking"
)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_measured_error_keeps_selected_price_terms_without_turning_an_unknown_search_count_free() {
        let mut request = orch_agent_contract::requests::UsageRequest {
            id: "priced-error".into(),
            ..Default::default()
        };
        request.finish("harness_error", Some("native response was cut"), None);
        let mut raw = json!(request);
        raw["native_prices"] = json!({"web_search":null});
        let error = orch_link_contract::commands::CommandError::new(
            orch_link_contract::commands::ErrorCode::Internal,
            "native response was cut",
        )
        .with_details(json!({"model_request":raw}));
        let answer = failed_answer(&error).unwrap().unwrap();
        assert_eq!(answer.timing.native_prices.get("web_search"), Some(&None));
        assert_eq!(
            answer.timing.web_searches,
            orch_api_contract::usage::UsageMeasurement::Empty
        );
    }

    #[test]
    fn failed_native_result_upload_keeps_the_exact_measured_counter_and_tier_envelope() {
        let mut request = orch_agent_contract::requests::UsageRequest {
            id: "generation".into(),
            model: Some("actual-model".into()),
            ..Default::default()
        };
        request.finish("harness_error", Some("captured result upload failed"), None);
        let mut timing = request.measured();
        timing.requested_service_tier = Some("priority".into());
        timing.web_fetches = orch_api_contract::usage::UsageMeasurement::Told { value: 1 };
        timing.declared_counters.insert(
            "actual_native_calls".into(),
            orch_api_contract::usage::UsageMeasurement::Told { value: 2 },
        );
        let error = orch_link_contract::commands::CommandError::new(
            orch_link_contract::commands::ErrorCode::Internal,
            "captured result upload failed",
        )
        .with_details(json!({"model_request":request,"request_timing":timing}));
        assert_eq!(failed_answer(&error).unwrap().unwrap().timing, timing);
    }

    fn message(role: GenerationRole, content: Vec<GenerationPart>) -> GenerationMessage {
        GenerationMessage {
            role,
            content,
            continuation: vec![],
        }
    }
    fn question() -> GenerationMessage {
        message(
            GenerationRole::User,
            vec![GenerationPart::Text {
                text: "Question".into(),
            }],
        )
    }
    fn call(id: &str) -> GenerationPart {
        GenerationPart::ToolCall {
            id: id.into(),
            name: "open".into(),
            arguments: json!({"reference":"topic:example"}),
        }
    }
    fn result(id: &str) -> GenerationPart {
        GenerationPart::ToolResult {
            call_id: id.into(),
            content: "Checked source".into(),
            is_error: false,
            witness: None,
        }
    }

    #[test]
    fn unicode_tool_results_fit_the_byte_budget_with_the_shortening_note() {
        let mut result = "来源😀".repeat(1000);
        bound_tool_result(&mut result, 128);
        assert!(result.len() <= 128);
        assert!(result.starts_with("来源😀"));
        assert!(result.ends_with("a narrower read.]"));
    }

    #[test]
    fn a_cancelled_provider_reply_keeps_its_actual_usage_and_outcome() {
        use orch_link_contract::commands::{CommandError, ErrorCode};
        let mut report = orch_agent_contract::requests::UsageRequest {
            id: "generation".into(),
            model: Some("question-model".into()),
            asked: Some("2026-11-08T00:00:00Z".parse().unwrap()),
            first_output: Some("2026-11-08T00:00:01Z".parse().unwrap()),
            end: Some("2026-11-08T00:00:03Z".parse().unwrap()),
            tokens: orch_api_contract::threads::TokenUsage {
                input_tokens: 90,
                output_tokens: 12,
                total_tokens: 102,
                ..Default::default()
            }
            .into(),
            output_streamed: Some(true),
            provider_request_id: Some("actual-request".into()),
            ..Default::default()
        };
        report.finish("stream_interrupted", Some("generation was stopped"), None);
        let error = CommandError::new(ErrorCode::Conflict, "generation was stopped")
            .with_details(json!({"model_request":report}));
        let answer = failed_answer(&error).unwrap().unwrap();
        assert_eq!(answer.usage.total_tokens, 102);
        assert_eq!(answer.request_ref.as_deref(), Some("actual-request"));
        assert_eq!(
            answer.timing.outcome.value().map(String::as_str),
            Some("stream_interrupted")
        );
        assert_eq!(answer.timing.output_streamed, Some(true));
        assert!(answer.output.message.content.is_empty());
    }

    #[test]
    fn resuming_an_interrupted_tool_batch_closes_only_missing_answers() {
        let prior = vec![
            question(),
            message(GenerationRole::Assistant, vec![call("a"), call("b")]),
            message(GenerationRole::User, vec![result("a")]),
            question(),
        ];
        let resumed = resume_messages(prior);
        assert_eq!(resumed.len(), 5);
        assert_eq!(resumed[2].content, vec![result("a")]);
        let [
            GenerationPart::ToolResult {
                call_id, is_error, ..
            },
        ] = resumed[3].content.as_slice()
        else {
            panic!("the unfinished operation needs one explicit interruption result");
        };
        assert_eq!(call_id, "b");
        assert!(*is_error);
        assert_eq!(resumed[4], question());
    }

    #[test]
    fn a_retained_page_does_not_start_inside_an_older_tool_exchange() {
        let resumed = resume_messages(vec![
            message(GenerationRole::Assistant, vec![call("old")]),
            message(GenerationRole::User, vec![result("old")]),
            question(),
            message(GenerationRole::User, vec![result("orphan")]),
        ]);
        assert_eq!(resumed, vec![question()]);
    }
}
