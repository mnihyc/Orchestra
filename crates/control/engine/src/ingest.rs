//! Idempotent application of durable machine events.
//!
//! A machine's events arrive in one stream for each session and each
//! harness, numbered without gaps, and are applied by [`Engine::apply_events`]
//! inside the transaction that moves the stream's cursor, so a stream that is
//! cut at any point is taken in once and in order. Applying is also
//! idempotent by what each event is about, because the guarantee should not
//! rest on one mechanism alone: an item is made once for a `(segment,
//! native_ref)`; text is appended only where the event's offset equals what
//! the item holds, so a stretch that was applied is skipped and a stretch
//! that skips ahead waits for the completed item that carries the whole;
//! a turn leaves a state once. An event that is merely late (for a turn that
//! is over, an item that is complete, an approval that was decided) changes
//! nothing and is not an error: a failure of the transaction would send the
//! same events again for ever.
//!
//! What an event does to a thread's rows is stamped with the thread's
//! sequence, one number for each row changed, so that a reader that missed
//! the announcement reads everything after the last number it has.
//!
//! The streams of providers are applied in `providers` (a provider's status
//! and facts, the requests a lease counted, the requests it refused).
//!
//! **Not done here.** Text is not masked for secrets (redaction is a later
//! part's); `raw.frame` is kept for diagnosis, masked, by `raw_events`; `usage.updated` records the turn's and the
//! thread's figures (the turn's never below what the forwarder counted for it)
//! and writes no row of the usage ledger, since on the path `forwarded` the
//! forwarder is the meter; once a delivery has been applied, the usage caps of
//! the turns whose usage it reported are looked at again (`caps`); an agent's
//! call of one of Orch's tools is answered by `tools`, apart from the
//! transaction; `credential.needed`
//! is recorded in the log only.

use crate::engine::{Engine, lock};
use crate::error::EngineError;
use crate::harnesses::HarnessKey;
use crate::live::LiveDelta;
use crate::providers;
use crate::turns::{InputAfter, TurnEnd, error_json, turn_error_json};
use crate::unit::Unit;
use crate::views;
use orch_db::Conn;
use orch_db::queries::resources::thread_engine as resources;
use orch_db::queries::threads::{
    self, InputChange, InputRow, ItemChange, ItemRow, NewItem, SegmentRow, ThreadRow, TurnRow,
    input_state, thread_state, turn_state,
};
use orch_db::usage::ledger;
use orch_link::streams::{DeliveredEvent, Delivery};
use orch_link_contract::commands::SerialKey;
use orch_link_contract::events::{
    DeltaField, EmittedItemKind, Event, ItemCompleted, ItemDelta, ItemStarted, ItemUpdated,
    OrchToolCall, SessionLost, SessionOpened, SessionTitle, TurnCompleted, TurnOutcome, TurnPhase,
    TurnPhaseKind, TurnStarted, UsageUpdated,
};
use orch_link_contract::streams::{ResetReason, Stream};
use orch_types::{Id, Json, new_id};
use serde_json::json;

/// How much of the end of a command's output an item keeps in its row: the
/// last 16 KiB, which a screen shows at a glance. The whole is kept as a
/// blob.
pub const OUTPUT_INLINE_MAX_BYTES: usize = 16 * 1024;
/// The longest title a harness may give a thread, in characters.
pub const TITLE_MAX_CHARS: usize = 120;

// ---------------------------------------------------------------- making items

/// An item to add to a thread's timeline.
#[derive(Debug, Clone)]
pub struct ItemSpec<'a> {
    /// The thread.
    pub thread: Id,
    /// The segment.
    pub segment: Id,
    /// The turn.
    pub turn: Option<Id>,
    /// `main`, or `sub:…`.
    pub stream: &'a str,
    /// The item it is made under.
    pub parent: Option<Id>,
    /// The kind.
    pub kind: &'a str,
    /// The status.
    pub status: &'a str,
    /// Who it is by.
    pub author: Option<Id>,
    /// What it holds.
    pub payload: &'a Json,
    /// The adapter's name for it, or a key of the engine's own, which makes
    /// the item once however often it is asked for.
    pub native_ref: Option<&'a str>,
}

/// Whether making an item of this kind moves what a reader's attention is
/// worth.
fn moves_activity(kind: &str) -> bool {
    matches!(
        kind,
        "user_message" | "announcement" | "marker" | "error" | "approval"
    )
}

/// The text an item is found by in a search. The part that forks and switches: the items of a side thread
/// are not indexed at all, so that an aside is never found by those who look for what the thread said.
async fn indexed_text(
    conn: &mut Conn,
    thread: Id,
    kind: &str,
    payload: &Json,
) -> Result<Option<String>, EngineError> {
    // Search: the same projector serves ingestion and repair, including side-thread exclusion.
    crate::projections::item_text(conn, thread, kind, payload).await
}

/// Adds an item. None where the segment has one with this `native_ref`
/// already, and nothing was written.
pub async fn add_item(
    unit: &mut Unit<'_>,
    spec: &ItemSpec<'_>,
) -> Result<Option<ItemRow>, EngineError> {
    // An item that is there already costs nothing: no number is spent.
    if let Some(native_ref) = spec.native_ref
        && threads::item_by_native_ref(&mut *unit.conn, spec.segment, native_ref)
            .await?
            .is_some()
    {
        return Ok(None);
    }
    let seq = unit.stamp(spec.thread).await?;
    let now = unit.now();
    let search = indexed_text(&mut *unit.conn, spec.thread, spec.kind, spec.payload).await?;
    let row = threads::insert_item(
        &mut *unit.conn,
        &NewItem {
            id: new_id(),
            thread_id: spec.thread,
            segment_id: spec.segment,
            turn_id: spec.turn,
            stream: spec.stream,
            parent_item_id: spec.parent,
            kind: spec.kind,
            status: spec.status,
            author_id: spec.author,
            payload: spec.payload,
            native_ref: spec.native_ref,
            search_text: search.as_deref(),
            at: now,
        },
        seq,
    )
    .await?;
    if spec.kind == "tool_call"
        && let (Some(item), Some(turn), Some(reference)) = (&row, spec.turn, spec.native_ref)
    {
        // A native request may arrive before its tool item, or the tool may be recovered at completion.
        orch_db::queries::approvals::link_content_source(
            unit.conn,
            spec.thread,
            turn,
            reference,
            item.id,
            now,
            seq,
        )
        .await?;
    }
    if row.is_some()
        && orch_db::queries::thread_history::selected_segment(unit.conn, spec.thread, spec.segment)
            .await?
    {
        threads::touch(&mut *unit.conn, spec.thread, now).await?;
        if moves_activity(spec.kind) {
            threads::mark_activity(&mut *unit.conn, spec.thread, seq, now).await?;
        }
    }
    Ok(row)
}

/// Adds a line of history to a thread's timeline. `key` makes it once.
pub async fn add_marker(
    unit: &mut Unit<'_>,
    thread: Id,
    segment: Id,
    turn: Option<Id>,
    payload: &Json,
    key: &str,
) -> Result<(), EngineError> {
    add_item(
        unit,
        &ItemSpec {
            thread,
            segment,
            turn,
            stream: "main",
            parent: None,
            kind: "marker",
            status: "completed",
            author: None,
            payload,
            native_ref: Some(key),
        },
    )
    .await?;
    Ok(())
}

/// Adds an error to a thread's timeline.
pub async fn add_error_item(
    unit: &mut Unit<'_>,
    thread: Id,
    segment: Id,
    turn: Option<Id>,
    code: &str,
    message: &str,
    retryable: bool,
) -> Result<(), EngineError> {
    let payload = error_json(code, message, retryable);
    let key = turn.map(|turn| format!("error:{code}:{turn}"));
    add_item(
        unit,
        &ItemSpec {
            thread,
            segment,
            turn,
            stream: "main",
            parent: None,
            kind: "error",
            status: "failed",
            author: None,
            payload: &payload,
            native_ref: key.as_deref(),
        },
    )
    .await?;
    Ok(())
}

/// Adds what a person said to the timeline, once, when their input is
/// delivered.
pub async fn add_user_message(
    unit: &mut Unit<'_>,
    thread: Id,
    segment: Id,
    turn: Option<Id>,
    input: &InputRow,
) -> Result<(), EngineError> {
    // Words whose place was kept before they were sent (a steer) are confirmed where they stand.
    if confirm_user_message(unit, thread, input.id).await? {
        return Ok(());
    }
    write_user_message(unit, thread, segment, turn, input, "completed").await
}

/// Keeps the place of an input's words in the timeline before they are sent into a turn that runs, as an item
/// `in_progress`: an answer of the harness that comes before the acknowledgement then never stands above the words
/// it answers. The place is kept once, through a send made again and a restart; [`add_user_message`] confirms it.
pub async fn reserve_user_message(
    unit: &mut Unit<'_>,
    thread: Id,
    segment: Id,
    turn: Option<Id>,
    input: &InputRow,
) -> Result<(), EngineError> {
    write_user_message(unit, thread, segment, turn, input, "in_progress").await
}

/// Confirms the words of an input whose place was kept: the item is `completed` where it stands. Says whether
/// there was such a place.
pub async fn confirm_user_message(
    unit: &mut Unit<'_>,
    thread: Id,
    input: Id,
) -> Result<bool, EngineError> {
    settle_user_message(unit, thread, input, "completed").await
}

/// Settles the kept place of an input's words as `status` (`completed` once sent, `cancelled` when the input
/// was taken back before it was). Says whether there was such a place.
pub async fn settle_user_message(
    unit: &mut Unit<'_>,
    thread: Id,
    input: Id,
    status: &str,
) -> Result<bool, EngineError> {
    let key = user_message_key(input);
    let Some(item) = threads::user_message_kept(&mut *unit.conn, thread, &key).await? else {
        return Ok(false);
    };
    let seq = unit.stamp(thread).await?;
    let now = unit.now();
    threads::set_item_status(&mut *unit.conn, item.id, &["in_progress"], status, now, seq).await?;
    Ok(true)
}

/// The engine's own key of the item that holds an input's words.
fn user_message_key(input: Id) -> String {
    format!("user:{input}")
}

async fn write_user_message(
    unit: &mut Unit<'_>,
    thread: Id,
    segment: Id,
    turn: Option<Id>,
    input: &InputRow,
    status: &str,
) -> Result<(), EngineError> {
    // What another agent or a person announced is in the timeline already, as the announcement that the thread
    // was given when it was made: saying it again as if somebody had typed it would show it twice.
    if input.source == "announcement" {
        return Ok(());
    }
    let mut payload = json!({
        "parts": crate::paste::metadata(&input.content), "input_id": input.id, "delivery": input.delivery, "source": input.source,
    });
    // The input of a goal's loop shows the goal's own words in its place: its turn, its limit, what it has used.
    if input.source == "goal"
        && let Some(facts) = orch_db::queries::goals::input_facts(unit.conn, input.id).await?
    {
        payload["goal"] = facts;
    }
    if input
        .client_id
        .as_deref()
        .is_some_and(|key| key.starts_with("recovery:"))
        && let Some(recovery) = crate::calls::read(
            unit.conn,
            thread,
            orch_provider_contract::calls::Purpose::Agent,
        )
        .await?
        && recovery.context["input_id"] == json!(input.id)
    {
        payload["automatic"] = json!({"rule_id":recovery.rule["id"],"kind":recovery.failure["code"],"attempt":recovery.action.as_ref().and_then(|action|action.get("attempt"))});
    }
    let key = user_message_key(input.id);
    let model_input:Option<Id>=sqlx::query_scalar("SELECT id FROM model_inputs WHERE node_id=$1 AND input_id=$2 AND purpose='input' ORDER BY seq_created DESC LIMIT 1")
        .bind(thread).bind(input.id).fetch_optional(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
    add_item(
        unit,
        &ItemSpec {
            thread,
            segment,
            turn,
            stream: "main",
            parent: None,
            kind: "user_message",
            status,
            author: input.author_id,
            payload: &payload,
            native_ref: Some(&key),
        },
    )
    .await?;
    if let Some(model_input) = model_input {
        sqlx::query("UPDATE items SET model_input_id=$3 WHERE thread_id=$1 AND native_ref=$2")
            .bind(thread)
            .bind(&key)
            .bind(model_input)
            .execute(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- payloads

pub(crate) fn code_points(text: &str) -> i64 {
    i64::try_from(text.chars().count()).unwrap_or(i64::MAX)
}

/// An item's payload with the members the wire needs of its kind, so that a
/// harness that leaves one out still gives an item that reads.
fn normalized(kind: &str, payload: &Json) -> Json {
    let mut payload = if payload.is_object() {
        payload.clone()
    } else {
        json!({})
    };
    let set_default = |payload: &mut Json, member: &str, value: Json| {
        if payload.get(member).is_none() {
            payload[member] = value;
        }
    };
    match kind {
        "agent_message" => {
            set_default(&mut payload, "text", json!(""));
            let length = code_points(payload["text"].as_str().unwrap_or_default());
            payload["text_len"] = json!(length);
            set_default(&mut payload, "phase", Json::Null);
        }
        "reasoning" => {
            set_default(&mut payload, "summary", json!(""));
            let length = code_points(payload["summary"].as_str().unwrap_or_default());
            payload["text_len"] = json!(length);
            set_default(&mut payload, "hidden", json!(false));
        }
        "tool_call"
            // Display credentials are carried only by orch.tool_call, never by the public timeline item.
            if payload["tool"]["kind"]=="orch" && payload["tool"]["tool"]=="use_display" => {
                if let Some(arguments)=payload["tool"]["arguments"].as_object_mut() {arguments.remove("password");}
                let input=payload["tool"]["arguments"].to_string();
                let length=code_points(&input);
                payload["raw"]["input"]=json!({"inline":input,"total_len":length,"truncated":false,"blob":null});
            }
        _ => {}
    }
    payload
}

/// What kind of item a completed payload is, for an item that was never
/// started because its start was lost.
fn infer_kind(payload: &Json) -> Option<&'static str> {
    let has = |member: &str| payload.get(member).is_some();
    if has("tool") {
        Some("tool_call")
    } else if has("steps") {
        Some("plan")
    } else if has("summary") {
        Some("reasoning")
    } else if has("text") {
        Some("agent_message")
    } else if has("code") && has("message") {
        Some("error")
    } else if has("marker") {
        Some("marker")
    } else {
        None
    }
}

/// Merges `patch` into `target`: objects member by member, anything else
/// replaces.
/// Whether `patch` only tells a sub-agent's card its state and its model, which a completed card still takes:
/// the sub-agent goes on after the call that started it has completed.
fn subagent_state_only(kind: &str, payload: &Json, patch: &Json) -> bool {
    kind == "tool_call"
        && payload.pointer("/tool/kind").and_then(Json::as_str) == Some("subagent")
        && patch.as_object().is_some_and(|members| members.len() == 1)
        && patch
            .get("tool")
            .and_then(Json::as_object)
            .is_some_and(|tool| {
                !tool.is_empty() && tool.keys().all(|key| key == "state" || key == "model")
            })
}

fn merge(target: &mut Json, patch: &Json) {
    match (target.as_object_mut(), patch.as_object()) {
        (Some(into), Some(members)) => {
            for (name, value) in members {
                match into.get_mut(name) {
                    Some(existing) => merge(existing, value),
                    None => {
                        into.insert(name.clone(), value.clone());
                    }
                }
            }
        }
        _ => *target = patch.clone(),
    }
}

/// The last `max_bytes` of `text`, cut at a character.
pub(crate) fn tail_of(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut from = text.len() - max_bytes;
    while !text.is_char_boundary(from) {
        from += 1;
    }
    &text[from..]
}

// ---------------------------------------------------------------- applying a delivery

/// Joins each run of deltas that continue one another (the same item and field, each starting where the one before
/// ends) into one, so that the pieces a frame carries of one item are one piece to hold and to hand to live readers.
/// A machine joins only what waits in its own queue, which drains as fast as its spool takes it; while the system is
/// behind, the machine's window of unacknowledged events is full and its next frame is made of everything that
/// waited in its spool, so the further behind the system is, the fewer pieces it handles.
fn joined_deltas<R>(events: Vec<(R, Event)>) -> Vec<(R, Event)> {
    let mut joined: Vec<(R, Event)> = Vec::with_capacity(events.len());
    for (raw, event) in events {
        if let (Some((_, Event::ItemDelta(before))), Event::ItemDelta(delta)) =
            (joined.last_mut(), &event)
            && before.native_ref == delta.native_ref
            && before.field == delta.field
            && before.offset + before.len == delta.offset
        {
            before.data.push_str(&delta.data);
            before.len += delta.len;
            continue;
        }
        joined.push((raw, event));
    }
    joined
}

/// What an event is about, for the log.
fn describe(event: &DeliveredEvent) -> String {
    format!("{} #{}", event.kind, event.seq)
}

impl Engine {
    /// Applies the events of one delivery, in their order, inside the
    /// transaction of the caller, which moves the stream's cursor in the same
    /// transaction and commits both. Streams the engine does not own are left
    /// to whoever does.
    pub async fn apply_events(
        &self,
        conn: &mut Conn,
        machine: Id,
        delivery: &Delivery,
    ) -> Result<(), EngineError> {
        self.apply_stream_events(conn, machine, &delivery.stream, &delivery.events)
            .await
    }

    /// Applies the events of a stream, in their order. Applying the same
    /// events again changes nothing: what each event does is done once.
    pub async fn apply_stream_events(
        &self,
        conn: &mut Conn,
        machine: Id,
        stream: &Stream,
        events: &[DeliveredEvent],
    ) -> Result<(), EngineError> {
        // Direct callers obey the same order as the backplane: authority before any report effects.
        orch_db::queries::transaction_guard::lock_authority(conn).await?;
        // Ask the source before object locks, restores, tools or diagnostics, including nested host objects.
        if !orch_db::queries::machine_reports::accepts(
            conn,
            machine,
            stream,
            &events
                .iter()
                .map(|event| (event.kind.as_str(), &event.body))
                .collect::<Vec<_>>(),
        )
        .await?
        {
            tracing::warn!(%machine, stream_kind = stream.family().prefix(), count = events.len(), "a report from another machine was refused");
            return Ok(());
        }
        // A shell that ended while nobody was attached to it is told on its environment's stream.
        if let Stream::Env { environment_id } = *stream {
            crate::browsers::apply_reports(self, conn, machine, environment_id, events).await?;
            self.shells()
                .apply_exits(conn, environment_id, events)
                .await?;
        }
        let mut unit = Unit::new(conn, self.inner.clock.now());
        match *stream {
            Stream::Session {
                thread_id,
                segment_id,
            } => {
                self.apply_session(&mut unit, machine, thread_id, segment_id, events)
                    .await?;
            }
            Stream::Provider { provider_id } => {
                self.apply_provider_events(&mut unit, machine, provider_id, events)
                    .await?;
            }
            Stream::Harness {
                provider_id,
                agent_id,
            } => {
                for event in events {
                    match Event::decode(&event.kind, event.body.clone()) {
                        Ok(Event::HarnessStatus(status)) => {
                            self.apply_harness_status(
                                &mut unit,
                                machine,
                                provider_id,
                                agent_id,
                                &status,
                            )
                            .await?;
                        }
                        Ok(Event::HarnessDescriptor(descriptor)) => {
                            self.apply_harness_descriptor(
                                &mut unit,
                                machine,
                                provider_id,
                                agent_id,
                                &serde_json::to_value(&descriptor)?,
                            )
                            .await?;
                        }
                        Ok(_) => {}
                        Err(error) => {
                            tracing::warn!(%error, event = %describe(event), "an event of a harness does not read")
                        }
                    }
                }
            }
            Stream::Node => {
                self.apply_node_events(&mut unit, machine, events).await?;
            }
            Stream::Env { environment_id } => {
                self.apply_env_events(&mut unit, machine, environment_id, events)
                    .await?;
            }
            Stream::Machine => {
                for event in events {
                    if let Ok(Event::PortState(report)) =
                        Event::decode(&event.kind, event.body.clone())
                    {
                        crate::ports::machine_port_state(&mut *unit.conn, machine, &report).await?;
                    }
                }
            }
        }
        // The usage caps of the turns whose usage was reported are looked at again, at most as often as
        // `[engine] cap_check_secs` says for a thread.
        self.caps_after_usage(&mut unit, stream, events).await?;
        let finished = unit.finish().await?;
        for thread in finished.wakes {
            self.wake(thread);
        }
        Ok(())
    }

    /// The machine said that events of a stream are gone. The cursor was set
    /// in the same transaction; a session stream is repaired by the machine
    /// reporting the session again, which is applied like anything else.
    pub async fn reset_stream(
        &self,
        conn: &mut Conn,
        machine: Id,
        stream: &Stream,
        reason: ResetReason,
    ) -> Result<(), EngineError> {
        if !orch_db::queries::machine_reports::owns(conn, machine, stream).await? {
            return Ok(());
        }
        tracing::warn!(%machine, %stream, ?reason, "events of a stream are gone");
        Ok(())
    }

    async fn apply_session(
        &self,
        unit: &mut Unit<'_>,
        machine: Id,
        thread_id: Id,
        segment_id: Id,
        events: &[DeliveredEvent],
    ) -> Result<(), EngineError> {
        let Some(thread) = threads::lock(&mut *unit.conn, thread_id).await? else {
            tracing::warn!(%thread_id, "events of a thread that is gone");
            return Ok(());
        };
        let Some(segment) = threads::segment(&mut *unit.conn, segment_id)
            .await?
            .filter(|segment| segment.thread_id == thread_id)
        else {
            tracing::warn!(%thread_id, %segment_id, "events of a segment the thread does not have");
            return Ok(());
        };
        // Events from the machine of a thread that was blocked as offline are
        // the machine's return.
        if thread.current_segment_id == Some(segment.id)
            && thread.active_continuation_id == segment.continuation_id
        {
            self.restore_in(unit, &thread).await?;
        }
        let mut batch = Batch {
            machine,
            thread_id,
            frames: crate::raw_events::Keeper::new(&thread, segment.id),
            segment,
        };
        let mut decoded = Vec::with_capacity(events.len());
        for event in events {
            match Event::decode(&event.kind, event.body.clone()) {
                Ok(event_of) => decoded.push((event, event_of)),
                Err(error) => {
                    tracing::warn!(%error, event = %describe(event), "an event of a session does not read");
                }
            }
        }
        // Whatever else the session says reads its items with all the text they were told.
        let mut written = false;
        for (event, decoded) in joined_deltas(decoded) {
            if matches!(decoded, Event::ItemDelta(_)) {
                written = false;
            } else if !written {
                self.write_held_of(unit, thread_id).await?;
                written = true;
            }
            self.apply_one(unit, &mut batch, event, decoded).await?;
        }
        Ok(())
    }

    async fn apply_one(
        &self,
        unit: &mut Unit<'_>,
        batch: &mut Batch,
        raw: &DeliveredEvent,
        event: Event,
    ) -> Result<(), EngineError> {
        match event {
            Event::SessionOpened(opened) => self.on_session_opened(unit, batch, &opened).await,
            Event::InstructionSources(sources) => {
                self.record_instruction_sources(
                    unit,
                    batch.thread_id,
                    batch.segment.id,
                    &sources.sources,
                )
                .await
            }
            Event::SessionTitle(title) => self.on_session_title(unit, batch, &title).await,
            Event::SessionLost(lost) => self.on_session_lost(unit, batch, &lost).await,
            Event::TurnStarted(started) => self.on_turn_started(unit, batch, &started).await,
            Event::ItemStarted(started) => self.on_item_started(unit, batch, &started).await,
            Event::ItemDelta(delta) => self.on_item_delta(unit, batch, &delta).await,
            Event::ItemUpdated(updated) => self.on_item_updated(unit, batch, &updated).await,
            Event::ItemCompleted(completed) => {
                self.on_item_completed(unit, batch, &completed).await
            }
            Event::ApprovalRequested(requested) => {
                self.on_approval_requested(
                    unit,
                    batch.thread_id,
                    batch.segment.id,
                    batch.machine,
                    &requested,
                )
                .await
            }
            Event::ApprovalClosed(closed) => {
                self.on_approval_closed(unit, batch.thread_id, &closed)
                    .await
            }
            Event::TurnPhase(phase) => self.on_turn_phase(unit, batch, &phase).await,
            Event::UsageUpdated(usage) => {
                self.on_usage_updated(unit, batch, &usage, raw.at.get())
                    .await
            }
            Event::ModelRequest(report) => {
                if let Err(error) = report.request.check() {
                    tracing::warn!(%error,"an invalid model request observation was not counted");
                    return Ok(());
                }
                if let Some(turn) = self.turn_of(unit, batch, report.turn_id).await? {
                    self.count_adapter_requests(
                        unit,
                        turn.id,
                        std::slice::from_ref(&report.request),
                    )
                    .await?;
                }
                Ok(())
            }
            Event::TurnCompleted(completed) => {
                self.on_turn_completed(unit, batch, &completed).await
            }
            Event::TurnCommandCleanup(cleanup) => {
                if let Some(turn) = self.turn_of(unit, batch, cleanup.turn_id).await?
                    && (turn.state == turn_state::INTERRUPTED || turn.interrupted_by.is_some())
                {
                    let seq = unit.stamp(batch.thread_id).await?;
                    sqlx::query("UPDATE turns SET command_cleanup=$2,seq_updated=$3 WHERE id=$1")
                        .bind(turn.id)
                        .bind(serde_json::to_value(cleanup.result)?)
                        .bind(seq)
                        .execute(&mut *unit.conn)
                        .await
                        .map_err(orch_db::DbError::from)?;
                }
                Ok(())
            }
            Event::OrchToolCall(call) => self.on_tool_call(batch, &call).await,
            Event::CredentialNeeded(need) => {
                // A harness that runs on a copy of the credential says it lacks one (the part that keeps
                // providers): the schedule issues another, and fails the turn if it asks too often.
                let lease = threads::open_lease_of(&mut *unit.conn, batch.thread_id).await?;
                match lease.filter(|lease| lease.mode == "shared") {
                    Some(lease) => lock(&self.inner.credentials).needs(lease.id, need.reason),
                    None => {
                        tracing::warn!(thread = %batch.thread_id, ?need, "a harness needs a credential, and none was shared with it");
                    }
                }
                Ok(())
            }
            Event::RawFrame(frame) => {
                batch
                    .frames
                    .keep(&mut *unit.conn, &frame, raw.at.get())
                    .await
            }
            _ => {
                tracing::debug!(event = %describe(raw), "an event that does not belong to a session");
                Ok(())
            }
        }
    }

    async fn thread_row(&self, unit: &mut Unit<'_>, thread: Id) -> Result<ThreadRow, EngineError> {
        threads::thread(&mut *unit.conn, thread)
            .await?
            .ok_or_else(|| EngineError::unreadable(format!("there is no thread {thread}")))
    }

    // ------------------------------------------------------------ session

    async fn on_session_opened(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
        opened: &SessionOpened,
    ) -> Result<(), EngineError> {
        let held = threads::segment(&mut *unit.conn, batch.segment.id)
            .await?
            .and_then(|segment| segment.native_session_ref);
        if held.is_none() {
            let seq = unit.stamp(batch.thread_id).await?;
            threads::set_native_session_ref(
                &mut *unit.conn,
                batch.segment.id,
                &opened.native_session_ref,
                seq,
            )
            .await?;
        }
        let (agent, provider) = (
            resources::agent(&mut *unit.conn, batch.segment.agent_id).await?,
            resources::provider(&mut *unit.conn, batch.segment.provider_id).await?,
        );
        if let (Some(agent), Some(provider)) = (agent, provider) {
            let payload = json!({
                "marker": "segment_started",
                "data": {
                    "segment_id": batch.segment.id, "ordinal": batch.segment.ordinal,
                    "agent": views::agent_ref_json(&mut *unit.conn, &agent).await?,
                    "provider": views::provider_ref_json(&mut *unit.conn, &provider).await?,
                    "resumed": opened.resumed,
                },
            });
            add_marker(
                unit,
                batch.thread_id,
                batch.segment.id,
                None,
                &payload,
                &format!("segment_started:{}", batch.segment.id),
            )
            .await?;
        }
        lock(&self.inner.sessions).opened(
            batch.segment.id,
            batch.thread_id,
            HarnessKey {
                provider: batch.segment.provider_id,
                agent: batch.segment.agent_id,
                machine: batch.machine,
            },
        );
        Ok(())
    }

    async fn on_session_title(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
        title: &SessionTitle,
    ) -> Result<(), EngineError> {
        if !orch_db::queries::thread_history::selected_segment(
            unit.conn,
            batch.thread_id,
            batch.segment.id,
        )
        .await?
        {
            return Ok(());
        }
        let mode: Option<String> = orch_settings::get(
            &mut *unit.conn,
            orch_settings::Place::Node(batch.thread_id),
            orch_settings_contract::registry::keys::AUTO_TITLE,
        )
        .await
        .map_err(|error| EngineError::unreadable(error.to_string()))?;
        if mode.as_deref() != Some("harness") {
            return Ok(());
        }
        let name: String = title.title.trim().chars().take(TITLE_MAX_CHARS).collect();
        if name.is_empty() {
            return Ok(());
        }
        let now = unit.now();
        if threads::rename_if_automatic(&mut *unit.conn, batch.thread_id, &name, now).await? {
            let seq = unit.stamp(batch.thread_id).await?;
            threads::set_seq_state(&mut *unit.conn, batch.thread_id, seq).await?;
        }
        Ok(())
    }

    async fn on_session_lost(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
        lost: &SessionLost,
    ) -> Result<(), EngineError> {
        // A per-conversation CLI can end while its adapter remains able to serve other conversations.
        sqlx::query("UPDATE harnesses SET last_error_at=$4,last_error_detail=$5,updated_at=$4 WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3")
            .bind(batch.segment.provider_id).bind(batch.segment.agent_id).bind(batch.machine).bind(unit.now()).bind(&lost.reason)
            .execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        lock(&self.inner.sessions).closed(batch.segment.id);
        let thread = self.thread_row(unit, batch.thread_id).await?;
        let Some(turn) = threads::active_turn(&mut *unit.conn, batch.thread_id).await? else {
            if thread.current_segment_id == Some(batch.segment.id) {
                add_error_item(
                    unit,
                    batch.thread_id,
                    batch.segment.id,
                    None,
                    "session_lost",
                    &format!("The harness lost the session: {}", lost.reason),
                    true,
                )
                .await?;
            }
            return Ok(());
        };
        if turn.segment_id != batch.segment.id {
            return Ok(());
        }
        let message = format!("The harness lost the session: {}", lost.reason);
        add_error_item(
            unit,
            batch.thread_id,
            turn.segment_id,
            Some(turn.id),
            "session_lost",
            &message,
            true,
        )
        .await?;
        self.end_turn(
            unit,
            &thread,
            &turn,
            TurnEnd {
                state: if turn.state == turn_state::RUNNING {
                    turn_state::INTERRUPTED
                } else {
                    turn_state::FAILED
                },
                error: Some(error_json("session_lost", &message, true)),
                input: InputAfter::Keep,
                then_block: None,
            },
        )
        .await?;
        Ok(())
    }

    // ------------------------------------------------------------ turns

    /// A turn that words prove to be running is marked so.
    pub(crate) async fn begin_turn_for(
        &self,
        unit: &mut Unit<'_>,
        thread_id: Id,
        turn: &TurnRow,
    ) -> Result<(), EngineError> {
        self.begin_turn(unit, thread_id, turn, None).await
    }

    /// The turn is running: `starting` → `running`, its input delivered.
    async fn begin_turn(
        &self,
        unit: &mut Unit<'_>,
        thread_id: Id,
        turn: &TurnRow,
        native_turn_ref: Option<&str>,
    ) -> Result<(), EngineError> {
        if turn.state != turn_state::STARTING {
            return Ok(());
        }
        let selected = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM threads h JOIN turns t ON t.id=$2
          WHERE h.node_id=$1 AND h.current_turn_id=t.id AND h.current_segment_id=t.segment_id
            AND h.active_continuation_id=t.continuation_id)",
        )
        .bind(thread_id)
        .bind(turn.id)
        .fetch_one(&mut *unit.conn)
        .await
        .map_err(orch_db::DbError::from)?;
        let seq = unit.stamp(thread_id).await?;
        let now = unit.now();
        if !threads::start_running(&mut *unit.conn, turn.id, native_turn_ref, now, seq).await? {
            return Ok(());
        }
        crate::resource_facts::library::capture_turn(unit.conn, turn.id).await?;
        let moved = threads::change_input(
            &mut *unit.conn,
            turn.input_id,
            &[input_state::DELIVERING],
            &InputChange {
                state: input_state::DELIVERED,
                turn_id: Some(turn.id),
                delivered_at: Some(now),
                ..InputChange::default()
            },
            seq,
        )
        .await?;
        if let Some(input) = threads::input(&mut *unit.conn, turn.input_id).await?
            && (moved || input.state == input_state::DELIVERED)
        {
            add_user_message(unit, thread_id, turn.segment_id, Some(turn.id), &input).await?;
            crate::native_actions::invoked(unit, turn, input.id).await?;
            // Automatic first-input titles use the delivered words; no provider work runs here.
            if selected {
                crate::summaries::first_input(unit, thread_id).await?;
            }
            // What of where the agent is went with the turn's input is what it was given.
            threads::context_given(&mut *unit.conn, turn.segment_id, &turn.id.to_string()).await?;
        }
        if !selected {
            return Ok(());
        }
        let waiting =
            orch_db::queries::approvals::count_pending_of_thread(&mut *unit.conn, thread_id)
                .await?
                > 0;
        let state = if waiting {
            thread_state::WAITING_APPROVAL
        } else {
            thread_state::RUNNING
        };
        threads::set_state(
            &mut *unit.conn,
            thread_id,
            &[thread_state::STARTING, thread_state::BLOCKED],
            &threads::StateChange::plain(state),
            seq,
        )
        .await?;
        threads::touch(&mut *unit.conn, thread_id, now).await?;
        Ok(())
    }

    async fn turn_of(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
        turn: Id,
    ) -> Result<Option<TurnRow>, EngineError> {
        Ok(threads::turn(&mut *unit.conn, turn).await?.filter(|turn| {
            turn.thread_id == batch.thread_id && turn.segment_id == batch.segment.id
        }))
    }

    async fn on_turn_started(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
        started: &TurnStarted,
    ) -> Result<(), EngineError> {
        let Some(turn) = self.turn_of(unit, batch, started.turn_id).await? else {
            tracing::warn!(thread = %batch.thread_id, turn = %started.turn_id, "a harness started a turn the thread does not have");
            return Ok(());
        };
        if turn.is_active() {
            return self
                .begin_turn(
                    unit,
                    batch.thread_id,
                    &turn,
                    started.native_turn_ref.as_deref(),
                )
                .await;
        }
        // The engine gave this turn up (it was not accepted in time, or it
        // was interrupted before it began), and the harness runs it after all:
        // ask it to stop.
        tracing::warn!(thread = %batch.thread_id, turn = %turn.id, "a harness began a turn that is over; it is asked to stop");
        self.stop_stray(batch, turn.id);
        Ok(())
    }

    /// Asks the harness to stop a turn it began after the engine gave it up. The command is a row written
    /// on a pooled connection: it is issued on a task of its own, never inside the transaction of the
    /// delivery that heard of the turn, which must wait for no other connection. The delivery does not
    /// await this task. Its identifier is the turn's, so a second ask is the same
    /// command, and one whose delivery was undone stops a turn that is over all the same.
    fn stop_stray(&self, batch: &Batch, turn: Id) {
        let command = crate::commands::Outgoing::new(
            crate::ids::turn_interrupt(turn),
            batch.machine,
            SerialKey::Thread(batch.thread_id),
            &orch_link_contract::commands::TurnInterrupt {
                thread_id: batch.thread_id,
                turn_id: turn,
            },
        );
        let Ok(command) = command else { return };
        let commands = std::sync::Arc::clone(&self.inner.commands);
        tokio::spawn(async move {
            if let Err(error) = commands.issue(command).await {
                tracing::warn!(%error, "a stray turn could not be interrupted");
            }
        });
    }

    async fn on_turn_phase(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
        phase: &TurnPhase,
    ) -> Result<(), EngineError> {
        let Some(turn) = self.turn_of(unit, batch, phase.turn_id).await? else {
            return Ok(());
        };
        if !turn.is_active() {
            return Ok(());
        }
        let word = match phase.kind {
            TurnPhaseKind::Working => "working",
            TurnPhaseKind::Compacting => "compacting",
        };
        if turn.phase.as_deref() == Some(word) {
            return Ok(());
        }
        let seq = unit.stamp(batch.thread_id).await?;
        let changed =
            threads::set_turn_phase(&mut *unit.conn, turn.id, word, phase.at.get(), seq).await?;
        // The harness shortens the conversation: where the agent is is told whole again at the next input.
        if changed && word == "compacting" {
            threads::set_context_sent(&mut *unit.conn, turn.segment_id, &json!({ "resend": true }))
                .await?;
        }
        if changed
            && turn.phase.as_deref() == Some("compacting")
            && word == "working"
            && let Some(since) = turn.phase_at
        {
            let took = (phase.at.get() - since).num_milliseconds().max(0);
            add_marker(
                unit,
                batch.thread_id,
                turn.segment_id,
                Some(turn.id),
                &json!({ "marker": "compacted", "data": { "duration_ms": took } }),
                &format!("compacted:{}:{}", turn.id, since.timestamp_millis()),
            )
            .await?;
        }
        Ok(())
    }

    async fn on_usage_updated(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
        usage: &UsageUpdated,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), EngineError> {
        let Some(turn) = self.turn_of(unit, batch, usage.turn_id).await? else {
            return Ok(());
        };
        let thread = self.thread_row(unit, batch.thread_id).await?;
        // What the program says it reports decides what its adapter's figures mean: an adapter puts zero in the
        // totals of a program that counts nothing, and zero is not a count.
        let (reports_tokens, reports_context) = self.usage_reports(unit, batch).await?;
        let mut whole = if reports_tokens {
            serde_json::to_value(usage.thread)?
        } else {
            json!({ "tokens_reported": false })
        };
        if let Some(window) = usage.context_window {
            orch_db::queries::provider_facts::learn_model_window(
                &mut *unit.conn,
                batch.segment.provider_id,
                &turn.model,
                window,
                at,
            )
            .await?;
        }
        if let Some(object) = whole.as_object_mut() {
            let previous = thread.usage.as_ref();
            let keep = |member: &str| {
                previous
                    .and_then(|usage| usage.get(member))
                    .cloned()
                    .unwrap_or(Json::Null)
            };
            object.insert(
                "context_window".to_owned(),
                usage
                    .context_window
                    .map_or_else(|| keep("context_window"), Json::from),
            );
            object.insert(
                "context_used".to_owned(),
                usage
                    .context_used
                    .map_or_else(|| keep("context_used"), Json::from),
            );
            object.insert("updated_at".to_owned(), views::time_json(at));
        }
        // Where the harness shortens the conversation, as it last said.
        crate::context_fill::with_compact_at(&mut whole, usage, thread.usage.as_ref());
        // What the forwarder counted for the turn is not lost when the harness reports less: the
        // turn's figures are the larger of the two, part by part.
        let counted = ledger::counted_request_totals(&mut *unit.conn, turn.id).await?;
        let turn_usage = if reports_tokens {
            let turn_report = serde_json::to_value(usage.turn)?;
            let reported = providers::tokens_of(&turn_report);
            Some(if counted.total > reported.total {
                providers::usage_json(&providers::larger(&reported, &counted))
            } else {
                turn_report
            })
        } else if counted.total > 0 {
            // Only what the forwarder counted is a count of the turn; without it the turn's use is not known.
            Some(providers::usage_json(&counted))
        } else {
            None
        };
        let turn_changed = turn_usage
            .as_ref()
            .is_some_and(|figures| turn.usage.as_ref() != Some(figures));
        let selected =
            orch_db::queries::thread_history::selected_turn(unit.conn, batch.thread_id, turn.id)
                .await?;
        // A program that reports neither tokens nor context leaves the thread's usage as it is: unknown.
        let thread_changed = selected
            && (reports_tokens || reports_context)
            && thread.usage.as_ref() != Some(&whole);
        if turn_changed || thread_changed {
            let seq = unit.stamp(batch.thread_id).await?;
            if let Some(figures) = turn_usage.as_ref().filter(|_| turn_changed) {
                threads::set_turn_usage(&mut *unit.conn, turn.id, figures, seq).await?;
            }
            if thread_changed {
                threads::set_usage(&mut *unit.conn, batch.thread_id, &whole, seq).await?;
            }
        }
        // What the harness reported beyond the totals, kept with the turn.
        self.keep_usage_report(unit, batch.thread_id, &turn, usage)
            .await?;
        Ok(())
    }

    /// Whether the program of the segment's agent reports token totals, and whether it reports how full the context
    /// is, as its descriptor says (with the overrides of the system). A program that never described itself is
    /// taken to report both, as before.
    async fn usage_reports(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
    ) -> Result<(bool, bool), EngineError> {
        let agent = orch_db::queries::resources::thread_engine::agent(
            &mut *unit.conn,
            batch.segment.agent_id,
        )
        .await?;
        let descriptor = agent
            .as_ref()
            .map(crate::harnesses::descriptor_of)
            .transpose()?
            .flatten();
        Ok(descriptor.map_or((true, true), |descriptor| {
            (
                descriptor.capabilities.usage_tokens,
                descriptor.capabilities.usage_context.reported(),
            )
        }))
    }

    async fn on_turn_completed(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
        completed: &TurnCompleted,
    ) -> Result<(), EngineError> {
        let Some(turn) = self.turn_of(unit, batch, completed.turn_id).await? else {
            return Ok(());
        };
        // Native completion is proof for this exact source turn, including after a forced Stop.
        sqlx::query("UPDATE turns SET native_end_confirmed=true WHERE id=$1 AND segment_id=$2")
            .bind(turn.id)
            .bind(batch.segment.id)
            .execute(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
        if !turn.is_active() {
            if turn.state == turn_state::INTERRUPTED
                && turn
                    .error
                    .as_ref()
                    .is_some_and(|error| error["code"] == "forced")
            {
                let seq = unit.stamp(batch.thread_id).await?;
                sqlx::query("UPDATE turns SET seq_updated=$2 WHERE id=$1")
                    .bind(turn.id)
                    .bind(seq)
                    .execute(&mut *unit.conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
                let thread = self.thread_row(unit, batch.thread_id).await?;
                if orch_db::queries::thread_history::selected_turn(
                    unit.conn,
                    batch.thread_id,
                    turn.id,
                )
                .await?
                    && thread.blocked_reason.as_deref() == Some("error")
                    && thread.blocked_detail.as_deref() == Some(crate::background::NATIVE_BUSY)
                    && !crate::background::native_busy(&mut *unit.conn, batch.thread_id).await?
                {
                    threads::set_state(
                        &mut *unit.conn,
                        batch.thread_id,
                        &[thread_state::BLOCKED],
                        &threads::StateChange::plain(thread_state::IDLE),
                        seq,
                    )
                    .await?;
                    unit.wake(batch.thread_id);
                }
            }
            return Ok(());
        }
        // A turn that ended without a word of having begun began and ended.
        self.begin_turn(unit, batch.thread_id, &turn, None).await?;
        let (state, mut error) = match completed.status {
            TurnOutcome::Completed => (turn_state::COMPLETED, None),
            TurnOutcome::Interrupted => (
                turn_state::INTERRUPTED,
                completed.error.as_ref().map(turn_error_json),
            ),
            TurnOutcome::Failed => (
                turn_state::FAILED,
                Some(completed.error.as_ref().map_or_else(
                    || error_json("harness_error", "the turn failed", false),
                    turn_error_json,
                )),
            ),
        };
        if let Some(failure) = completed.error.as_ref()
            && completed.status == TurnOutcome::Failed
        {
            // What passes (`provider_busy`, `provider_unreachable`) and every failure that no row knows leave
            // the provider as it was. A limit of the plan closes it until the time the vendor names; a refused
            // credential puts it in `error`; a limit that covers one family of models or the spending, which
            // another model may get round, closes nothing.
            match failure.code.as_str() {
                "provider_exhausted" if !failure.retryable => {
                    self.close_provider_by_limit(unit, batch.segment.provider_id, failure)
                        .await?;
                }
                "provider_unauthorized" => {
                    self.reject_provider(unit, batch.segment.provider_id, &failure.message)
                        .await?;
                }
                _ => {}
            }
        }
        if completed.status == TurnOutcome::Completed {
            self.accept_provider(unit, batch.segment.provider_id)
                .await?;
        }
        let thread = self.thread_row(unit, batch.thread_id).await?;
        let turn = self
            .turn_of(unit, batch, completed.turn_id)
            .await?
            .unwrap_or(turn);
        let recovery = if completed.status == TurnOutcome::Failed {
            if let Some(failure) = error.as_ref() {
                self.recover_agent_in(unit, &thread, &turn, failure).await?
            } else {
                None
            }
        } else {
            None
        };
        if let (Some(recovery), Some(error)) = (recovery.as_ref(), error.as_mut()) {
            error["recovery"] = json!({"state":recovery.state,"rule":recovery.rule["id"],
                "automatic": recovery.state == "scheduled" || recovery.state == "stopped"});
        }
        self.end_turn(
            unit,
            &thread,
            &turn,
            TurnEnd {
                state,
                error,
                input: InputAfter::Keep,
                then_block: None,
            },
        )
        .await?;
        if let Some(recovery) = recovery {
            let seq = unit.stamp(thread.node_id).await?;
            threads::set_state(
                unit.conn,
                thread.node_id,
                &[thread_state::IDLE, thread_state::BLOCKED],
                &threads::StateChange::blocked("error", Some("call_recovery")),
                seq,
            )
            .await?;
            if recovery.state == "stopped" {
                crate::calls::notify_stop(unit, &thread, &recovery).await?;
            }
        } else if matches!(
            completed.status,
            TurnOutcome::Completed | TurnOutcome::Failed
        ) {
            sqlx::query("UPDATE model_call_recovery SET state='completed',due_at=NULL,context=context-'manual_stop'-'manual_request',updated_at=$2 WHERE node_id=$1 AND purpose='agent' AND (state='running' OR context->>'manual_stop'='true')")
                .bind(thread.node_id).bind(unit.now()).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        }
        if turn.plan && completed.status == TurnOutcome::Completed {
            let proposed:Option<Id>=sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND turn_id=$2 AND kind='plan' ORDER BY seq_created DESC,id DESC LIMIT 1").bind(batch.thread_id).bind(turn.id).fetch_optional(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
            if let Some(item) = proposed {
                Box::pin(self.source_requested(
                    unit,
                    &thread,
                    orch_db::queries::notifications::inbox::RequestSource::Plan(item),
                    Some(turn.id),
                ))
                .await?;
            }
        }
        Ok(())
    }

    /// A call of one of Orch's tools by the agent of the thread: answered once, apart from this transaction (see
    /// [`crate::tools`]).
    async fn on_tool_call(&self, batch: &Batch, call: &OrchToolCall) -> Result<(), EngineError> {
        self.take_tool_call(batch.machine, batch.thread_id, call);
        Ok(())
    }

    // ------------------------------------------------------------ items

    async fn item_of(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
        native_ref: &str,
    ) -> Result<Option<ItemRow>, EngineError> {
        Ok(threads::item_by_native_ref(&mut *unit.conn, batch.segment.id, native_ref).await?)
    }

    async fn on_item_started(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
        started: &ItemStarted,
    ) -> Result<(), EngineError> {
        let turn = self.turn_of(unit, batch, started.turn_id).await?;
        if let Some(turn) = &turn {
            // Words of a turn are proof that it runs.
            self.begin_turn(unit, batch.thread_id, turn, None).await?;
            if turn.is_active()
                && matches!(
                    started.kind,
                    EmittedItemKind::AgentMessage | EmittedItemKind::Reasoning
                )
            {
                self.accept_provider(unit, batch.segment.provider_id)
                    .await?;
            }
        }
        let parent = match &started.parent_native_ref {
            Some(native) => self.item_of(unit, batch, native).await?.map(|item| item.id),
            None => None,
        };
        let kind = match started.kind {
            EmittedItemKind::AgentMessage => "agent_message",
            EmittedItemKind::Reasoning => "reasoning",
            EmittedItemKind::Plan => "plan",
            EmittedItemKind::ToolCall => "tool_call",
            EmittedItemKind::Marker => "marker",
            EmittedItemKind::Error => "error",
        };
        let payload = normalized(kind, &serde_json::to_value(&started.payload)?);
        let status = if started.kind == EmittedItemKind::Error {
            "failed"
        } else {
            "in_progress"
        };
        add_item(
            unit,
            &ItemSpec {
                thread: batch.thread_id,
                segment: batch.segment.id,
                turn: turn.as_ref().map(|turn| turn.id),
                stream: &started.stream,
                parent,
                kind,
                status,
                author: None,
                payload: &payload,
                native_ref: Some(&started.native_ref),
            },
        )
        .await?;
        Ok(())
    }

    /// A piece of an item's text: held, and handed to whoever has the thread open, until it is written to the
    /// item ([`crate::streaming`]); with no interval to hold it for, written to the item at once.
    async fn on_item_delta(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
        delta: &ItemDelta,
    ) -> Result<(), EngineError> {
        if self.inner.config.stream_flush.is_zero() {
            return self.write_delta(unit, batch, delta).await;
        }
        let key = (batch.segment.id, delta.native_ref.clone());
        if self.inner.growing.expects(&key, delta.field).is_none() {
            let Some(item) = self.item_of(unit, batch, &delta.native_ref).await? else {
                return Ok(());
            };
            if item.status != "in_progress" {
                return Ok(());
            }
            let base = crate::streaming::length_of(&item.payload, delta.field);
            self.inner.growing.begin(&key, &item, delta.field, base);
        }
        // A stretch that was taken in already is skipped; one that skips ahead waits for the completed item, which
        // carries the whole.
        let Some(held) = self.inner.growing.add(&key, delta) else {
            return Ok(());
        };
        self.inner.live.delta(&LiveDelta {
            thread_id: batch.thread_id,
            item_id: held.item,
            turn_id: held.turn,
            stream: held.stream,
            field: delta.field,
            offset: delta.offset,
            len: delta.len,
            data: delta.data.clone(),
        });
        Ok(())
    }

    /// A piece of an item's text, written to the item as it comes.
    async fn write_delta(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
        delta: &ItemDelta,
    ) -> Result<(), EngineError> {
        let Some(item) = self.item_of(unit, batch, &delta.native_ref).await? else {
            return Ok(());
        };
        if item.status != "in_progress" {
            return Ok(());
        }
        let mut payload = item.payload.clone();
        let appended = match delta.field {
            DeltaField::Text | DeltaField::Summary => {
                let member = if delta.field == DeltaField::Text {
                    "text"
                } else {
                    "summary"
                };
                let held = payload
                    .get(member)
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if delta.offset != code_points(&held) {
                    // A stretch that was applied is skipped; one that skips ahead
                    // waits for the completed item, which carries the whole.
                    return Ok(());
                }
                let grown = format!("{held}{}", delta.data);
                payload[member] = json!(grown);
                payload["text_len"] = json!(code_points(&grown));
                true
            }
            DeltaField::Output => {
                let output = payload
                    .pointer("/tool/output")
                    .cloned()
                    .unwrap_or(Json::Null);
                let total = output.get("total_len").and_then(Json::as_i64).unwrap_or(0);
                if delta.offset != total {
                    return Ok(());
                }
                let inline = output
                    .get("inline")
                    .and_then(Json::as_str)
                    .unwrap_or_default();
                let joined = format!("{inline}{}", delta.data);
                let kept = tail_of(&joined, OUTPUT_INLINE_MAX_BYTES).to_owned();
                let new_total = total + delta.len;
                let blob = output.get("blob").cloned().unwrap_or(Json::Null);
                let whole = i64::try_from(kept.len()).unwrap_or(i64::MAX) >= new_total;
                payload["tool"]["output"] = json!({
                    "inline": kept, "total_len": new_total, "truncated": !whole, "blob": blob,
                });
                true
            }
        };
        if !appended {
            return Ok(());
        }
        let seq = unit.stamp(batch.thread_id).await?;
        let now = unit.now();
        threads::change_item(
            &mut *unit.conn,
            item.id,
            &ItemChange {
                status: None,
                payload: &payload,
                search_text: None,
                at: now,
            },
            seq,
        )
        .await?;
        if orch_db::queries::thread_history::selected_segment(
            unit.conn,
            batch.thread_id,
            batch.segment.id,
        )
        .await?
        {
            threads::touch(&mut *unit.conn, batch.thread_id, now).await?;
        }
        self.inner.live.delta(&LiveDelta {
            thread_id: batch.thread_id,
            item_id: item.id,
            turn_id: item.turn_id,
            stream: item.stream.clone(),
            field: delta.field,
            offset: delta.offset,
            len: delta.len,
            data: delta.data.clone(),
        });
        Ok(())
    }

    async fn on_item_updated(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
        updated: &ItemUpdated,
    ) -> Result<(), EngineError> {
        let Some(item) = self.item_of(unit, batch, &updated.native_ref).await? else {
            return Ok(());
        };
        // A sub-agent goes on after the call that started it has completed: its card still takes its state.
        if item.status != "in_progress"
            && !subagent_state_only(&item.kind, &item.payload, &updated.payload)
        {
            return Ok(());
        }
        let mut payload = item.payload.clone();
        merge(&mut payload, &updated.payload);
        let payload = normalized(&item.kind, &payload);
        if payload == item.payload {
            return Ok(());
        }
        let seq = unit.stamp(batch.thread_id).await?;
        let now = unit.now();
        threads::change_item(
            &mut *unit.conn,
            item.id,
            &ItemChange {
                status: None,
                payload: &payload,
                search_text: None,
                at: now,
            },
            seq,
        )
        .await?;
        self.subagent_moved(unit, batch, &item, &payload).await
    }

    /// Where an item that is a sub-agent's card now says the sub-agent ended, and said it ran before, the
    /// thread's people are told as its place asks (`notify.subagents`).
    async fn subagent_moved(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
        item: &ItemRow,
        after: &Json,
    ) -> Result<(), EngineError> {
        if item.kind != "tool_call"
            || after.pointer("/tool/kind").and_then(Json::as_str) != Some("subagent")
        {
            return Ok(());
        }
        let before = item.payload.pointer("/tool/state").and_then(Json::as_str);
        let now = after.pointer("/tool/state").and_then(Json::as_str);
        if before == now || !matches!(now, Some("completed" | "failed" | "interrupted")) {
            return Ok(());
        }
        let thread = self.thread_row(unit, batch.thread_id).await?;
        crate::notifications::subagent_ended(unit, &thread, item.id, after).await
    }

    async fn on_item_completed(
        &self,
        unit: &mut Unit<'_>,
        batch: &Batch,
        completed: &ItemCompleted,
    ) -> Result<(), EngineError> {
        let status = match completed.status {
            orch_agent_contract::events::ItemStatus::Failed => "failed",
            orch_agent_contract::events::ItemStatus::Cancelled => "cancelled",
            _ => "completed",
        };
        let complete_payload = serde_json::to_value(&completed.payload)?;
        // The completed item carries its whole text: what is held of it is written by this.
        self.inner
            .growing
            .forget(&(batch.segment.id, completed.native_ref.clone()));
        let existing = self.item_of(unit, batch, &completed.native_ref).await?;
        let Some(kind) = existing
            .as_ref()
            .map(|item| item.kind.clone())
            .or_else(|| infer_kind(&complete_payload).map(str::to_owned))
        else {
            tracing::warn!(native_ref = %completed.native_ref, "a completed item of a kind that cannot be told");
            return Ok(());
        };
        let mut payload = normalized(&kind, &complete_payload);
        if kind == "plan" {
            if let Some(object) = payload.as_object_mut() {
                object.remove("decision");
            }
            if let Some(approval) = existing
                .as_ref()
                .and_then(|item| item.payload.get("approval_id"))
            {
                payload["approval_id"] = approval.clone();
            }
        }
        if kind == "plan" {
            let mut texts: Vec<String> = payload
                .get("explanation")
                .and_then(Json::as_str)
                .map(str::to_owned)
                .into_iter()
                .collect();
            texts.extend(
                payload
                    .get("steps")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|step| step.get("text").and_then(Json::as_str).map(str::to_owned)),
            );
            let resolution = Box::pin(self.resolve_references(
                &mut *unit.conn,
                batch.thread_id,
                &texts.join("\n"),
            ))
            .await?;
            if let Some(explanation) = payload.get("explanation").and_then(Json::as_str) {
                payload["explanation"] = json!(resolution.store(explanation));
            }
            for step in payload
                .get_mut("steps")
                .and_then(Json::as_array_mut)
                .into_iter()
                .flatten()
            {
                if let Some(text) = step.get("text").and_then(Json::as_str) {
                    step["text"] = json!(resolution.store(text));
                }
            }
        }
        if kind == "agent_message"
            && let Some(text) = payload.get("text").and_then(Json::as_str)
        {
            let resolution =
                Box::pin(self.resolve_references(&mut *unit.conn, batch.thread_id, text)).await?;
            payload["text"] = json!(resolution.store(text));
        }
        if existing
            .as_ref()
            .is_none_or(|item| item.status == "in_progress" || item.payload != payload)
        {
            self.record_review_item(
                unit,
                batch.thread_id,
                batch.segment.id,
                &completed.native_ref,
                existing.as_ref().and_then(|item| item.turn_id),
                &payload,
            )
            .await?;
        }
        if kind == "tool_call"
            && status != "cancelled"
            && existing
                .as_ref()
                .is_some_and(|item| item.status == "in_progress")
        {
            let source_turn = existing.as_ref().and_then(|item| item.turn_id);
            let at = orch_types::Time::new(unit.now());
            crate::resource_facts::library::tool_item(
                unit.conn,
                batch.segment.id,
                source_turn,
                &completed.native_ref,
                &payload,
                at,
            )
            .await?;
        }
        if let Some(blob) = &completed.output_blob
            && payload.pointer("/tool/output").is_some()
        {
            payload["tool"]["output"]["blob"] = json!(blob.hash);
            payload["tool"]["output"]["truncated"] = json!(true);
        }
        match existing {
            None => {
                // The start was lost to a gap; the item is made whole.
                let turn = threads::active_turn(&mut *unit.conn, batch.thread_id)
                    .await?
                    .filter(|turn| turn.segment_id == batch.segment.id);
                let source_turn = if kind == "tool_call" {
                    orch_db::queries::approvals::content_source_turn(
                        unit.conn,
                        batch.thread_id,
                        batch.segment.id,
                        &completed.native_ref,
                    )
                    .await?
                } else {
                    None
                };
                add_item(
                    unit,
                    &ItemSpec {
                        thread: batch.thread_id,
                        segment: batch.segment.id,
                        turn: source_turn.or_else(|| turn.map(|turn| turn.id)),
                        stream: "main",
                        parent: None,
                        kind: &kind,
                        status,
                        author: None,
                        payload: &payload,
                        native_ref: Some(&completed.native_ref),
                    },
                )
                .await?;
                Ok(())
            }
            Some(item) => {
                if item.status != "in_progress" && item.payload == payload {
                    return Ok(());
                }
                let seq = unit.stamp(batch.thread_id).await?;
                let now = unit.now();
                let search =
                    indexed_text(&mut *unit.conn, batch.thread_id, &kind, &payload).await?;
                threads::change_item(
                    &mut *unit.conn,
                    item.id,
                    &ItemChange {
                        status: Some(status),
                        payload: &payload,
                        search_text: search.as_deref(),
                        at: now,
                    },
                    seq,
                )
                .await?;
                // Search: None also clears text that is now excluded; deltas still leave it alone.
                crate::projections::keep_item_text(
                    &mut *unit.conn,
                    item.id,
                    &payload,
                    status,
                    search.as_deref(),
                )
                .await?;
                let final_message = kind == "agent_message"
                    && matches!(
                        payload.get("phase").and_then(Json::as_str),
                        None | Some("final")
                    );
                if orch_db::queries::thread_history::selected_segment(
                    unit.conn,
                    batch.thread_id,
                    batch.segment.id,
                )
                .await?
                {
                    if final_message && item.status == "in_progress" {
                        threads::mark_activity(&mut *unit.conn, batch.thread_id, seq, now).await?;
                    }
                    threads::touch(&mut *unit.conn, batch.thread_id, now).await?;
                }
                self.subagent_moved(unit, batch, &item, &payload).await
            }
        }
    }
}

/// What the events of one delivery share.
#[derive(Debug)]
struct Batch {
    machine: Id,
    thread_id: Id,
    segment: SegmentRow,
    /// Keeps the frames of the harness's own protocol that the delivery holds.
    frames: crate::raw_events::Keeper,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_completed_sub_agent_card_takes_its_state_and_model_and_nothing_else() {
        let card = json!({ "tool": { "kind": "subagent", "child_stream": "sub:thr", "state": "running" } });
        assert!(subagent_state_only(
            "tool_call",
            &card,
            &json!({ "tool": { "state": "completed" } })
        ));
        assert!(subagent_state_only(
            "tool_call",
            &card,
            &json!({ "tool": { "state": "failed", "model": "gpt-6-mini" } })
        ));
        assert!(!subagent_state_only(
            "tool_call",
            &card,
            &json!({ "tool": { "prompt": "other" } })
        ));
        assert!(!subagent_state_only(
            "tool_call",
            &card,
            &json!({ "tool": {} })
        ));
        assert!(!subagent_state_only(
            "tool_call",
            &card,
            &json!({ "tool": { "state": "completed" }, "title": "x" })
        ));
        let command = json!({ "tool": { "kind": "command", "state": "running" } });
        assert!(!subagent_state_only(
            "tool_call",
            &command,
            &json!({ "tool": { "state": "completed" } })
        ));
        assert!(!subagent_state_only(
            "agent_message",
            &card,
            &json!({ "tool": { "state": "completed" } })
        ));
    }

    #[test]
    fn an_agent_message_that_leaves_members_out_gets_the_ones_the_wire_needs() {
        let normal = normalized("agent_message", &json!({ "text": "héllo" }));
        assert_eq!(normal["text_len"], 5, "counted in characters");
        assert!(normal["phase"].is_null());
        let reasoning = normalized("reasoning", &json!({}));
        assert_eq!(reasoning["summary"], "");
        assert_eq!(reasoning["hidden"], false);
    }

    #[test]
    fn what_a_resync_reports_without_its_start_still_has_a_kind() {
        assert_eq!(infer_kind(&json!({ "text": "x" })), Some("agent_message"));
        assert_eq!(
            infer_kind(&json!({ "tool": {}, "title": "t" })),
            Some("tool_call")
        );
        assert_eq!(infer_kind(&json!({ "steps": [] })), Some("plan"));
        assert_eq!(
            infer_kind(&json!({ "code": "c", "message": "m" })),
            Some("error")
        );
        assert_eq!(infer_kind(&json!({ "nothing": 1 })), None);
    }

    #[test]
    fn a_patch_replaces_what_it_names_and_leaves_the_rest() {
        let mut target =
            json!({ "tool": { "command": "ls", "output": { "inline": "a" } }, "title": "t" });
        merge(&mut target, &json!({ "tool": { "process_id": "42" } }));
        assert_eq!(target["tool"]["command"], "ls");
        assert_eq!(target["tool"]["process_id"], "42");
        merge(&mut target, &json!({ "title": "u" }));
        assert_eq!(target["title"], "u");
    }

    #[test]
    fn deltas_that_continue_one_another_are_applied_as_one() {
        let delta = |item: &str, field: DeltaField, offset: i64, data: &str| {
            Event::ItemDelta(ItemDelta {
                native_ref: item.to_owned(),
                field,
                offset,
                len: i64::try_from(data.chars().count()).unwrap(),
                data: data.to_owned(),
            })
        };
        let joined = joined_deltas(vec![
            (1, delta("a", DeltaField::Text, 0, "Hél")),
            (2, delta("a", DeltaField::Text, 3, "lo")),
            (3, delta("a", DeltaField::Summary, 0, "s")),
            (4, delta("b", DeltaField::Text, 0, "x")),
            (5, delta("b", DeltaField::Text, 1, "y")),
            (6, delta("b", DeltaField::Text, 5, "z")),
            (7, delta("a", DeltaField::Text, 5, "!")),
        ]);
        let seen: Vec<(i32, String, i64, i64, String)> = joined
            .into_iter()
            .filter_map(|(raw, event)| match event {
                Event::ItemDelta(d) => Some((raw, d.native_ref, d.offset, d.len, d.data)),
                _ => None,
            })
            .collect();
        let at = |raw: i32, item: &str, offset: i64, len: i64, data: &str| {
            (raw, item.to_owned(), offset, len, data.to_owned())
        };
        assert_eq!(
            seen,
            vec![
                at(1, "a", 0, 5, "Héllo"),
                at(3, "a", 0, 1, "s"),
                at(4, "b", 0, 2, "xy"),
                at(6, "b", 5, 1, "z"),
                at(7, "a", 5, 1, "!"),
            ],
            "a run joins only while item, field and place continue"
        );
    }

    #[test]
    fn the_end_of_a_long_output_is_kept_cut_at_a_character() {
        let text = "é".repeat(10);
        let tail = tail_of(&text, 5);
        assert!(tail.len() <= 5 && text.ends_with(tail));
        assert_eq!(tail_of("short", 100), "short");
    }
}
