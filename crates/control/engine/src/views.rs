//! A thread's rows as the wire has them, and what the readers of a thread are given.
//!
//! What the engine writes must read back as the types of the API
//! contract. The rows are therefore turned into JSON in the shape those
//! types are written in and read as them; a row that does not read (a payload
//! an adapter got wrong) is shown as an error item, and a reader is never
//! refused a whole thread for one line.
//!
//! The reading requests ([`Engine::sync`], [`Engine::items`], [`Engine::item`],
//! [`Engine::command_output`]) answer with what the engine owns. The parts of a
//! thread's view that belong to others (its node, with what the caller holds
//! on it; what it is bound to, worked out; its lock) are given by the caller to
//! [`ThreadSyncData::assemble`], which makes the whole [`ThreadSync`].

use crate::engine::Engine;
use crate::error::EngineError;
use chrono::{DateTime, Utc};
use orch_api_contract::environment::EnvironmentRef;
use orch_api_contract::goals::ThreadGoal;
use orch_api_contract::nodes::{JobRef, NodeBase, NodeRef};
use orch_api_contract::threads::continuations::ThreadReadSelector;
use orch_api_contract::threads::{
    BlockedReason, Item, RecordedCost, Segment, SideReading, StartingStep, THREAD_SYNC_ENDED_TURNS,
    THREAD_SYNC_MAX_CHANGES, Thread, ThreadBinding, ThreadBlocked, ThreadCan, ThreadCosts,
    ThreadInput, ThreadInputAccess, ThreadKind, ThreadMemory, ThreadQueue, ThreadSides,
    ThreadState, ThreadSync, ThreadUsage, Turn, TurnRecordedCost,
};
use orch_api_contract::users::UserRef;
use orch_blob_contract::range::ByteRange;
use orch_db::queries::environments::thread_engine::EnvironmentRow;
use orch_db::queries::resources::thread_engine::{self as resources, AgentRow, ProviderRow};
use orch_db::queries::thread_continuations::{self as continuation_rows, reading as history_rows};
use orch_db::queries::threads::{
    self, HistorySegmentRow as SegmentRow, InputRow, ItemRow, TurnRow,
};
use orch_db::{Conn, queries::approvals as approval_queries};
use orch_types::{Hash, Id, Json, Seq, Time};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use unicode_segmentation::UnicodeSegmentation;

/// The most characters of an item's plain-text projection that are kept for
/// search.
pub const SEARCH_TEXT_MAX_CHARS: usize = crate::projections::SEARCH_TEXT_MAX_CHARS;

/// The time as the wire writes it.
#[must_use]
pub fn time_json(at: DateTime<Utc>) -> Json {
    serde_json::to_value(Time::new(at)).unwrap_or(Json::Null)
}

fn optional_time(at: Option<DateTime<Utc>>) -> Json {
    at.map_or(Json::Null, time_json)
}

fn read<T: DeserializeOwned>(value: Json, what: &str) -> Result<T, EngineError> {
    serde_json::from_value(value).map_err(|error| {
        EngineError::unreadable(format!("{what} does not read as it should: {error}"))
    })
}

// ---------------------------------------------------------------- people and things, named

/// The letters an avatar shows: the first extended grapheme cluster of the
/// name, in upper case where it has one.
#[must_use]
pub fn avatar_text(display_name: &str) -> String {
    display_name
        .graphemes(true)
        .next()
        .map(str::to_uppercase)
        .unwrap_or_default()
}

fn person_json(row: &threads::PersonRow) -> Json {
    json!({
        "id": row.id,
        "kind": if row.kind == "service" { "service" } else { "human" },
        "username": row.username,
        "display_name": row.display_name,
        "avatar": { "text": avatar_text(&row.display_name), "color": row.avatar_color },
    })
}

/// A person as the wire names them; a person who is gone is named by what
/// is left of them.
pub async fn user_ref_json(conn: &mut Conn, user: Id) -> Result<Json, EngineError> {
    let people = threads::people(conn, &[user]).await?;
    Ok(people.first().map_or_else(
        || {
            json!({ "id": user, "kind": "human", "username": "", "display_name": "", "avatar": { "text": "", "color": 0 } })
        },
        person_json,
    ))
}

/// The address of a thing: the short names of the orgs down to its home, its
/// first owner's username, and its own name, joined by dots.
pub(crate) async fn address_json(
    conn: &mut Conn,
    home: Option<Id>,
    owner: Option<Id>,
    name: &str,
) -> Result<String, EngineError> {
    let mut parts: Vec<String> = match home {
        Some(home) => threads::org_slugs_above(conn, home).await?,
        None => vec!["system".to_owned()],
    };
    if let Some(owner) = owner
        && let Some(person) = threads::people(conn, &[owner]).await?.first()
    {
        parts.push(person.username.clone());
    }
    parts.push(
        name.to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("-"),
    );
    Ok(parts.join("."))
}

async fn machine_ref_json(conn: &mut Conn, machine: Id) -> Result<Json, EngineError> {
    Ok(resources::machine_name(conn, machine)
        .await?
        .map_or(Json::Null, |row| {
            json!({
                "id": row.id, "name": row.name, "online": row.online,
                "platform_os": row.platform_os, "placement": row.placement,
            })
        }))
}

/// An agent profile as the wire names it.
pub async fn agent_ref_json(conn: &mut Conn, agent: &AgentRow) -> Result<Json, EngineError> {
    let address = address_json(conn, agent.home_id, agent.owner_id, &agent.name).await?;
    let machine = match agent.machine_id {
        Some(machine) => machine_ref_json(conn, machine).await?,
        None => Json::Null,
    };
    let harness = serde_json::to_value(crate::programs::shipped_name(
        &agent.harness_kind,
        Some(&agent.descriptor),
    ))
    .unwrap_or(Json::Null);
    Ok(json!({
        "id": agent.id, "name": agent.name, "address": address, "harness_kind": agent.harness_kind,
        "harness": harness, "machine": machine, "disabled": agent.disabled,
    }))
}

/// A provider as the wire names it.
pub async fn provider_ref_json(
    conn: &mut Conn,
    provider: &ProviderRow,
) -> Result<Json, EngineError> {
    let address = address_json(
        conn,
        Some(provider.home_id),
        Some(provider.owner_id),
        &provider.name,
    )
    .await?;
    let harness = provider
        .harness_kind
        .as_deref()
        .and_then(|kind| serde_json::to_value(crate::programs::shipped_name(kind, None)).ok())
        .unwrap_or(Json::Null);
    Ok(json!({
        "id": provider.id, "name": provider.name, "kind": provider.kind,
        "harness_kind": provider.harness_kind, "harness": harness, "address": address,
        "placement": provider.placement, "status": provider.status,
    }))
}

/// An environment as the wire names it.
pub async fn environment_ref_json(
    conn: &mut Conn,
    environment: &EnvironmentRow,
    now: Time,
) -> Result<Json, EngineError> {
    let address = orch_authz::resources::addresses(conn, &[environment.id])
        .await?
        .into_iter()
        .next()
        .map(|(_, address)| address)
        .unwrap_or_default();
    let owner = user_ref_json(conn, environment.owner_id).await?;
    let exposed = crate::ports::exposures(conn, &[environment.id], now)
        .await
        .map_err(EngineError::unreadable)?;
    Ok(json!({
        "ports_exposed": exposed.contains(&environment.id),
        "id": environment.id, "name": environment.name, "owner": owner, "address": address,
        "placement": environment.placement, "status": environment.status,
    }))
}

// ---------------------------------------------------------------- rows as wire types

/// The people rows name, found once.
#[derive(Debug, Default)]
pub struct People {
    by_id: BTreeMap<Id, Json>,
    approvals: BTreeMap<Id, orch_db::queries::approvals::ApprovalRow>,
}

impl People {
    /// The people `ids` name.
    pub async fn load(
        conn: &mut Conn,
        ids: impl IntoIterator<Item = Id>,
    ) -> Result<Self, EngineError> {
        let wanted: Vec<Id> = ids
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let rows = threads::people(conn, &wanted).await?;
        Ok(Self {
            by_id: rows.iter().map(|row| (row.id, person_json(row))).collect(),
            approvals: BTreeMap::new(),
        })
    }

    /// Loads source outcomes in one batch; source cards keep pointers, never a second decision state.
    pub async fn with_approvals(
        mut self,
        conn: &mut Conn,
        ids: impl IntoIterator<Item = Id>,
    ) -> Result<Self, EngineError> {
        let ids: Vec<_> = ids
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let rows = orch_db::queries::approvals::among(conn, &ids).await?;
        let people: Vec<_> = rows.iter().filter_map(|row| row.decided_by).collect();
        if !people.is_empty() {
            for row in threads::people(conn, &people).await? {
                self.by_id.insert(row.id, person_json(&row));
            }
        }
        self.approvals = rows.into_iter().map(|row| (row.id, row)).collect();
        Ok(self)
    }

    fn outcome(&self, id: Option<Id>) -> Json {
        let Some(row) = id.and_then(|id| self.approvals.get(&id)) else {
            return Json::Null;
        };
        let Some(at) = row.decided_at else {
            return Json::Null;
        };
        let action = match row.kind.as_str() {
            "input" if row.state == "approved" => Some("run"),
            "plan" => row
                .decision
                .as_ref()
                .and_then(|decision| decision.get("option_id"))
                .and_then(Json::as_str)
                .filter(|choice| matches!(*choice, "proceed" | "keep_planning")),
            _ => None,
        };
        json!({"action":action,"state":row.state,"decided_by":self.named_or_null(row.decided_by),"decided_at":time_json(at)})
    }

    fn named(&self, id: Id) -> Json {
        self.by_id.get(&id).cloned().unwrap_or_else(|| {
            json!({ "id": id, "kind": "human", "username": "", "display_name": "", "avatar": { "text": "", "color": 0 } })
        })
    }

    pub(crate) fn named_or_null(&self, id: Option<Id>) -> Json {
        id.map_or(Json::Null, |id| self.named(id))
    }
}

/// Replaces the user ids of a stored list of bypasses by the people they
/// name.
fn bypass_json(stored: Option<&Json>, people: &People) -> Json {
    let entries = stored.and_then(Json::as_array).cloned().unwrap_or_default();
    Json::Array(
        entries
            .into_iter()
            .map(|mut entry| {
                if let Some(by) = entry
                    .get("by")
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse::<Id>().ok())
                {
                    entry["by"] = people.named(by);
                }
                entry
            })
            .collect(),
    )
}

/// The ids of the people a stored list of bypasses names.
pub(crate) fn bypass_people(stored: Option<&Json>) -> Vec<Id> {
    stored
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .get("by")
                .and_then(Json::as_str)
                .and_then(|id| id.parse().ok())
        })
        .collect()
}

/// The canonical source request pointer kept on a plan item.
pub fn source_approval(row: &ItemRow) -> Option<Id> {
    row.payload
        .get("approval_id")
        .and_then(Json::as_str)
        .and_then(|id| id.parse::<Id>().ok())
}

/// An item as the wire has it. An item whose payload does not read as its
/// kind is shown as an error that says so.
#[must_use]
pub fn item_view(row: &ItemRow, people: &People) -> Item {
    let pinned = row.pinned_at.map_or(
        Json::Null,
        |at| json!({ "at": time_json(at), "by": people.named_or_null(row.pinned_by) }),
    );
    let mut payload = row.payload.clone();
    if row.kind == "plan" {
        let approval = payload
            .get("approval_id")
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok());
        if approval.is_some() {
            payload["decision"] = people.outcome(approval);
        }
    }
    let assembled = json!({
        "id": row.id, "thread_id": row.thread_id, "continuation_id":row.continuation_id, "segment_id": row.segment_id, "turn_id": row.turn_id, "model_input_id":row.model_input_id,
        "stream": row.stream, "parent_item_id": row.parent_item_id,
        "kind": row.kind, "payload": payload, "status": row.status,
        "author": people.named_or_null(row.author_id), "before_orch": row.before_orch, "pinned": pinned, "chat_refs": row.chat_refs,
        "seq_created": row.seq_created, "seq_updated": row.seq_updated,
        "created_at": time_json(row.created_at), "updated_at": time_json(row.updated_at),
    });
    match serde_json::from_value::<Item>(assembled) {
        Ok(item) => item,
        Err(error) => {
            tracing::warn!(item = %row.id, %error, "an item does not read as its kind");
            let fallback = json!({
                "id": row.id, "thread_id": row.thread_id, "continuation_id":row.continuation_id, "segment_id": row.segment_id, "turn_id": row.turn_id,
                "stream": row.stream, "parent_item_id": row.parent_item_id,
                "kind": "error",
                "payload": { "code": "unreadable_item", "message": format!("an item of kind {} could not be read", row.kind), "retryable": false },
                "status": "failed", "author": null, "pinned": null, "chat_refs": row.chat_refs,
                "seq_created": row.seq_created, "seq_updated": row.seq_updated,
                "created_at": time_json(row.created_at), "updated_at": time_json(row.updated_at),
            });
            // The fallback is written here, in the shape of the type: it reads.
            serde_json::from_value(fallback).unwrap_or_else(|_| unreachable_item(row))
        }
    }
}

/// An item that is nothing but its place, for the one case that the fallback
/// itself does not read, which would be a change of the type.
fn unreachable_item(row: &ItemRow) -> Item {
    Item {
        continuation_id: Some(row.continuation_id),
        id: row.id,
        thread_id: row.thread_id,
        segment_id: row.segment_id,
        turn_id: row.turn_id,
        model_input_id: row.model_input_id,
        stream: row.stream.clone(),
        parent_item_id: row.parent_item_id,
        payload: orch_api_contract::threads::ItemPayload::Error(
            orch_api_contract::threads::ErrorPayload {
                code: "unreadable_item".to_owned(),
                message: "an item could not be read".to_owned(),
                retryable: false,
            },
        ),
        status: orch_api_contract::threads::ItemStatus::Failed,
        author: None,
        before_orch: row.before_orch,
        pinned: None,
        chat_refs: i64::from(row.chat_refs),
        seq_created: row.seq_created,
        seq_updated: row.seq_updated,
        created_at: Time::new(row.created_at),
        updated_at: Time::new(row.updated_at),
    }
}

/// A turn as the wire has it. `display` is the name its model is shown by.
pub fn turn_view(row: &TurnRow, display: &str, people: &People) -> Result<Turn, EngineError> {
    let error = row.error.clone().map(|mut error| {
        if let Some(object) = error.as_object_mut() {
            let message = orch_api_contract::messages::for_failure(
                object
                    .get("code")
                    .and_then(Json::as_str)
                    .unwrap_or("internal"),
            );
            object.insert("message".to_owned(), Json::String(message));
        }
        error
    });
    let phase = match (&row.phase, row.phase_at) {
        (Some(kind), Some(since)) => json!({ "kind": kind, "since": time_json(since) }),
        _ => Json::Null,
    };
    read(
        json!({
            "id": row.id, "thread_id": row.thread_id, "segment_id": row.segment_id, "ordinal": row.ordinal,
            "state": row.state, "input_id": row.input_id, "model": row.model,
            "before_snapshot_id": row.before_snapshot_id,
            "model_display": display,
            "effort": row.effort, "params": row.params, "error": error, "usage": row.usage,
            // What the harness reported beyond the totals, and what filled the context: a record of another
            // shape (an older one) is shown as none rather than making the turn unreadable.
            "usage_detail": crate::context_fill::readable::<orch_api_contract::context::TurnUsageDetail>(row.usage_detail.as_ref()),
            "context": crate::context_fill::readable::<orch_api_contract::context::ContextFill>(row.context.as_ref()),
            "interrupted_by": people.named_or_null(row.interrupted_by),
            "command_cleanup": row.command_cleanup,
            "limit_bypass": bypass_json(row.limit_bypass.as_ref(), people),
            "phase": phase, "waiting_since": optional_time(row.waiting_since), "waited_ms": row.waited_ms,
            "started_at": optional_time(row.started_at),
            "completed_at": optional_time(row.completed_at), "seq_updated": row.seq_updated,
            "plan": row.plan,
        }),
        "a turn",
    )
}

/// What a person's Stop holds of a thread's queue, as the wire has it.
pub async fn queue_of(
    conn: &mut Conn,
    row: &threads::ThreadRow,
) -> Result<ThreadQueue, EngineError> {
    if !row.queue_held {
        return Ok(ThreadQueue {
            revision: row.queue_revision,
            ..ThreadQueue::default()
        });
    }
    let held_inputs = threads::count_queue_held(conn, row.node_id).await?;
    let by = match row.queue_held_by {
        Some(by) => Some(read::<UserRef>(user_ref_json(conn, by).await?, "a person")?),
        None => None,
    };
    Ok(ThreadQueue {
        held: true,
        by,
        at: row.queue_held_at.map(Time::new),
        held_inputs,
        revision: row.queue_revision,
    })
}

/// An input as the wire has it.
pub fn input_view(row: &InputRow, people: &People) -> Result<ThreadInput, EngineError> {
    read(
        json!({
            "id": row.id, "thread_id": row.thread_id, "continuation_id":row.continuation_id, "author": people.named_or_null(row.author_id),
            "run_as": people.named_or_null(row.run_as),
            "decision": people.outcome(row.approval_id),"approval_id":row.approval_id,
            "plan": row.plan,
            "source": row.source, "content": crate::paste::metadata(&row.content), "native_action":row.native_action, "delivery": row.delivery, "state": row.state,
            "state_detail": row.state_detail,
            "attachment_issues": row.attachment_issues, "queue_held": row.queue_held,
            "limit_bypass": bypass_json(row.limit_bypass.as_ref(), people),
            "position": row.position, "turn_id": row.turn_id, "client_id": row.client_id,
            "created_at": time_json(row.created_at), "delivered_at": optional_time(row.delivered_at),
            "seq_updated": row.seq_updated,
        }),
        "an input",
    )
}

pub(crate) async fn segment_view(
    conn: &mut Conn,
    row: &SegmentRow,
    people: &People,
    now: Time,
) -> Result<Segment, EngineError> {
    let (agent, provider, environment) = match (row.agent_id, row.provider_id, row.environment_id) {
        (None, None, None) => (Json::Null, Json::Null, Json::Null),
        (Some(agent), Some(provider), Some(environment)) => {
            let agent = resources::agent(conn, agent).await?;
            let provider = resources::provider(conn, provider).await?;
            let environment =
                orch_db::queries::environments::thread_engine::environment(conn, environment)
                    .await?;
            let (Some(agent), Some(provider), Some(environment)) = (agent, provider, environment)
            else {
                return Err(EngineError::unreadable(
                    "a segment names an agent profile, a provider or an environment that is gone",
                ));
            };
            (
                agent_ref_json(conn, &agent).await?,
                provider_ref_json(conn, &provider).await?,
                environment_ref_json(conn, &environment, now).await?,
            )
        }
        _ => {
            return Err(EngineError::unreadable(
                "a segment has an incomplete execution binding",
            ));
        }
    };
    read(
        json!({
            "id": row.id, "ordinal": row.ordinal,
            "agent": agent, "provider": provider, "environment": environment,
            "env_generation": row.env_generation, "arrangement": row.arrangement,
            "executor": row.executor, "tool_set": row.tool_set, "credential_path": row.credential_path,
            "started_by": people.named_or_null(row.started_by),
            "started_at": time_json(row.started_at), "ended_at": optional_time(row.ended_at),
            "end_reason": row.end_reason, "import_fidelity": row.import_fidelity,
            "import_report": row.import_report, "seq_updated": row.seq_updated,
        }),
        "a segment",
    )
}

/// The plain-text projection of an item that search looks through; none for
/// a kind that is not searched.
#[must_use]
pub fn search_text(kind: &str, payload: &Json) -> Option<String> {
    // Search: the writer and its rebuild share one extractor.
    crate::projections::plain_item(kind, payload)
}

// ---------------------------------------------------------------- what a reader is given

/// The part of a thread's view that the engine owns.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadFacts {
    pub active_continuation_id: Id,
    pub execution_revision: i64,
    /// The thread's node.
    pub id: Id,
    /// The topic it lies in.
    pub topic_id: Id,
    /// What it is doing.
    pub state: ThreadState,
    /// The server-selected icon for this reader.
    pub display: orch_api_contract::threads::ThreadDisplay,
    /// For a side thread: its parent.
    pub side_of: Option<Id>,
    /// For a side thread: the last turn of its parent it took.
    pub side_from_turn: Option<i64>,
    /// For a side thread: how it reads without acting.
    pub side_reading: Option<SideReading>,
    /// What a starting thread waits for.
    pub starting_step: Option<StartingStep>,
    /// Why it cannot go on.
    pub blocked: Option<ThreadBlocked>,
    /// What a person's Stop holds of its queue.
    pub queue: ThreadQueue,
    /// Where its tools run, named.
    pub environment: Option<EnvironmentRef>,
    /// Its directory there.
    pub directory: String,
    /// Safe source preparation or worktree facts.
    pub source: Option<orch_api_contract::source::SourceInfo>,
    /// The links accepted by an interactor.
    pub links: Vec<orch_api_contract::source::ThreadLink>,
    /// Suggested links from the last completed main answer, separate from accepted links.
    pub suggested_links: Vec<orch_api_contract::source::ThreadLink>,
    /// The last explicitly requested Git work observation.
    pub work: Option<orch_api_contract::source::DirectoryWork>,
    /// Independent work flags and the unresolved last failed turn.
    pub attention: orch_api_contract::source::ThreadAttention,
    /// The model's routing name.
    pub model: Option<String>,
    /// The model's display name.
    pub model_display: Option<String>,
    /// The effort.
    pub effort: Option<String>,
    /// Where the agent starts, absolute.
    pub cwd: String,
    /// The turn under way.
    pub current_turn_id: Option<Id>,
    /// The segment under way.
    pub current_segment_id: Option<Id>,
    /// How many approvals wait.
    pub pending_approvals: i64,
    /// How many inputs wait.
    pub pending_inputs: i64,
    /// What it used in all.
    pub usage: Option<ThreadUsage>,
    /// Recorded credits supplied by the same coherent thread snapshot.
    pub costs: Option<ThreadCosts>,
    /// When something last happened in it.
    pub last_activity_at: Option<Time>,
    /// The highest sequence given out.
    pub seq: Seq,
    /// The sequence of the last change worth attention.
    pub activity_seq: Seq,
    /// How far the viewer has read.
    pub my_last_read_seq: Option<Seq>,
}

fn recovery_word<T: serde::de::DeserializeOwned>(word: Option<&str>) -> Option<T> {
    word.and_then(|word| serde_json::from_value(Json::String(word.to_owned())).ok())
}

fn thread_recovery_display(
    facts: &orch_db::queries::source::ThreadDisplayFacts,
) -> Option<(
    orch_api_contract::threads::ThreadDisplayState,
    orch_api_contract::threads::ThreadDisplayRecovery,
)> {
    use orch_api_contract::threads::{
        ThreadDisplayRecovery, ThreadDisplayState as State, ThreadRecoveryFailure,
        ThreadRecoveryStep,
    };
    if facts.state != "blocked"
        || facts.blocked_reason.as_deref() != Some("error")
        || facts.blocked_detail.as_deref() != Some("call_recovery")
        || facts.recovery_has_lock
        || matches!(
            facts.recovery_stop_reason.as_deref(),
            Some(
                "provider_switched_off" | "model_switched_off" | "request_cancelled" | "returning"
            )
        )
    {
        return None;
    }
    let state = match facts.recovery_state.as_deref()? {
        "scheduled" => State::Queued,
        "running" => State::Running,
        "stopped" | "waiting" => State::Failed,
        _ => return None,
    };
    let step = recovery_word(facts.recovery_step_type.as_deref());
    let attempts = matches!(
        step,
        Some(ThreadRecoveryStep::Retry | ThreadRecoveryStep::Continue)
    )
    .then_some(facts.recovery_step_times)
    .flatten();
    Some((
        state,
        ThreadDisplayRecovery {
            rule_id: facts.recovery_rule_id.clone().unwrap_or_default(),
            step,
            attempt: facts.recovery_attempt,
            attempts,
            due_at: facts.recovery_due_at.map(Time::new),
            continuing: facts.recovery_continuing,
            failure_kind: if facts.recovery_failure_reason.as_deref() == Some("access") {
                ThreadRecoveryFailure::Access
            } else {
                recovery_word(facts.recovery_failure_kind.as_deref()).unwrap_or_default()
            },
            stop_reason: recovery_word(facts.recovery_stop_reason.as_deref()),
            count: facts.recovery_count,
            window_secs: facts.recovery_window_secs,
        },
    ))
}

/// Selects the single icon for a reader from one coherent set of stored facts.
#[must_use]
pub fn thread_display(
    facts: &orch_db::queries::source::ThreadDisplayFacts,
) -> orch_api_contract::threads::ThreadDisplay {
    use orch_api_contract::threads::{ThreadDisplay, ThreadDisplayState as State};
    let recovery = thread_recovery_display(facts);
    // Archived work cannot run or wait. Its lifecycle presentation suppresses stale execution facts.
    let state = if facts.archived {
        State::Archived
    } else if facts.approvals > 0 || facts.waiting_inputs > 0 {
        State::WaitingApproval
    } else if let Some((state, _)) = &recovery {
        *state
    } else if facts.blocked_reason.is_some() || facts.state == "blocked" {
        State::Blocked
    } else if facts.failed {
        State::Failed
    } else if matches!(
        facts.state.as_str(),
        "starting" | "running" | "waiting_approval"
    ) {
        State::Running
    } else if facts.queued_inputs > 0 {
        State::Queued
    } else if facts.queue_held || facts.held_inputs > 0 {
        State::Held
    } else if facts.unread_completion {
        State::Finished
    } else {
        State::Idle
    };
    ThreadDisplay {
        seq: facts.seq,
        read_seq: facts.read_seq,
        state,
        queued_inputs: facts.queued_inputs,
        held_inputs: facts.held_inputs,
        waiting_inputs: facts.waiting_inputs,
        waiting_for_answer: facts.waiting_for_answer,
        blocked_reason: facts
            .blocked_reason
            .as_ref()
            .and_then(|word| serde_json::from_value(Json::String(word.clone())).ok()),
        finished_at: facts.finished_at.map(Time::new),
        recovery: recovery
            .filter(|(selected, _)| *selected == state)
            .map(|(_, details)| Box::new(details)),
    }
}

/// Projects an already authorized batch with no per-thread query or private diagnostic detail.
pub async fn thread_displays(
    conn: &mut Conn,
    ids: &[Id],
    reader: Option<Id>,
    at: Time,
) -> Result<std::collections::HashMap<Id, orch_api_contract::threads::ThreadDisplay>, EngineError> {
    Ok(
        orch_db::queries::source::thread_display_facts(conn, ids, reader, at)
            .await?
            .into_iter()
            .map(|facts| (facts.id, thread_display(&facts)))
            .collect(),
    )
}

/// A thread's projected rows without its view, settings or permission provenance.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadSyncRows {
    pub projection: orch_api_contract::threads::ThreadContinuationProjection,
    pub inputs_next_after: Option<Id>,
    pub inputs_queue_revision: i64,
    /// The thread's current feed sequence.
    pub seq: Seq,
    /// The stamp of the thread's state, for a separately authorized view.
    pub state_stamp: Seq,
    /// Whether every changed row fits this read.
    pub complete: bool,
    /// Conversation items in their wire shape.
    pub items: Vec<Item>,
    /// Turns in their wire shape.
    pub turns: Vec<Turn>,
    /// Inputs in their wire shape.
    pub inputs: Vec<ThreadInput>,
    /// Segments in their wire shape.
    pub segments: Vec<Segment>,
    /// Whether a first load has older items.
    pub has_older: bool,
}

/// What a thread's reader is given by the engine.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadSyncData {
    pub projection: orch_api_contract::threads::ThreadContinuationProjection,
    pub inputs_next_after: Option<Id>,
    pub inputs_queue_revision: i64,
    /// What the thread is, as far as the engine knows.
    pub thread: ThreadFacts,
    /// The highest sequence given out.
    pub seq: Seq,
    /// The sequence the reader asked from.
    pub since: Option<Seq>,
    /// False when more rows changed than a sync from a position holds.
    pub complete: bool,
    /// The items.
    pub items: Vec<Item>,
    /// The turns.
    pub turns: Vec<Turn>,
    /// The inputs.
    pub inputs: Vec<ThreadInput>,
    /// The segments.
    pub segments: Vec<Segment>,
    /// Whether items older than the returned ones exist.
    pub has_older: bool,
}

/// The parts of a thread's view that belong to the node, the resources and
/// the locks, which the caller who has them gives.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadFrame {
    /// The current goal, read with the caller's authorized thread view in the same snapshot.
    pub goal: Option<ThreadGoal>,
    /// What every node has, with what the caller holds on it.
    pub base: NodeBase<ThreadKind>,
    /// What the thread runs on, with what it comes to.
    pub binding: Option<ThreadBinding>,
    /// Its parent, for a side thread.
    pub side_of: Option<NodeRef>,
    /// Its side threads, for a thread that is not one.
    pub sides: Option<ThreadSides>,
    /// Where its agent's notes are kept.
    pub memory: ThreadMemory,
    /// The job that holds it.
    pub lock: Option<JobRef>,
    /// The authorized blocked-state projection, including this reader's sandbox recovery acts.
    pub blocked: Option<ThreadBlocked>,
    /// What the caller holds on its environment: the rows of its two levels and its four doors.
    pub environment_permissions: Vec<String>,
    /// How many other threads work in its directory.
    pub directory_threads: i64,
    /// Whether the caller may give it input.
    pub input_access: ThreadInputAccess,
    /// What the caller may do with it.
    pub can: ThreadCan,
}

impl ThreadSyncData {
    /// The whole sync, with the parts the caller has.
    #[must_use]
    pub fn assemble(self, frame: ThreadFrame) -> ThreadSync {
        let facts = self.thread;
        let mut base = frame.base;
        for reference in &mut base.path {
            if reference.id == facts.id {
                reference.thread_display = Some(Box::new(facts.display.clone()));
            }
        }
        ThreadSync {
            projection: Some(self.projection),
            prompt_summaries: Vec::new(),
            viewed_prompt: None,
            inputs_next_after: self.inputs_next_after,
            inputs_queue_revision: Some(self.inputs_queue_revision),
            thread: Thread {
                goal: frame.goal,
                active_continuation_id: Some(facts.active_continuation_id),
                execution_revision: Some(facts.execution_revision),
                base,
                topic_id: facts.topic_id,
                state: facts.state,
                side_of: frame.side_of,
                side_from_turn: facts.side_from_turn,
                sides: frame.sides,
                side_reading: facts.side_reading,
                starting_step: facts.starting_step,
                blocked: frame.blocked,
                queue: facts.queue,
                binding: frame.binding,
                environment: facts.environment,
                environment_permissions: frame.environment_permissions,
                directory: facts.directory,
                source: facts.source,
                links: facts.links,
                suggested_links: facts.suggested_links,
                work: facts.work,
                attention: facts.attention,
                display: facts.display,
                model: facts.model,
                model_display: facts.model_display,
                effort: facts.effort,
                cwd: facts.cwd,
                directory_threads: frame.directory_threads,
                memory: frame.memory,
                lock: frame.lock,
                input_access: frame.input_access,
                can: frame.can,
                current_turn_id: facts.current_turn_id,
                current_segment_id: facts.current_segment_id,
                pending_approvals: facts.pending_approvals,
                pending_inputs: facts.pending_inputs,
                usage: facts.usage,
                costs: facts.costs,
                last_activity_at: facts.last_activity_at,
                seq: facts.seq,
                activity_seq: facts.activity_seq,
                my_last_read_seq: facts.my_last_read_seq,
            },
            seq: self.seq,
            since: self.since,
            complete: self.complete,
            items: self.items,
            turns: self.turns,
            inputs: self.inputs,
            segments: self.segments,
            has_older: self.has_older,
        }
    }
}

/// Why a thread could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    /// A cursor from another selected history must reset, never be reused by numeric sequence.
    #[error("The viewed continuation changed; reload this history projection.")]
    ProjectionChanged(orch_api_contract::threads::ThreadContinuationProjection),
    /// There is no such thread, or no such item in it.
    #[error("there is no such thing")]
    NotFound,
    /// The request asks for something that cannot be: none or several of the
    /// positions, a limit of nothing.
    #[error("{0}")]
    BadRequest(&'static str),
    /// The output of this command is longer than the item keeps and is not in
    /// the store of content: the command is still running, or this process has
    /// no store.
    #[error("the whole of the output is not in the store of content")]
    OutputInBlobStore,
    /// The item is not a command.
    #[error("the item is not a command")]
    NotACommand,
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<orch_db::DbError> for ReadError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

/// A thread's segments, turns and items, whole.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadExportData {
    /// Exact read-only history identity retained by this export.
    pub projection: orch_api_contract::threads::ThreadContinuationProjection,
    /// Its segments.
    pub segments: Vec<Segment>,
    /// Its turns.
    pub turns: Vec<Turn>,
    /// Its items.
    pub items: Vec<Item>,
}

/// Content choices and the read-only selection of a complete export.
#[derive(Debug, Clone, Copy)]
pub struct ExportSelection {
    /// Permanent turn-ordinal range; loose selected items remain included.
    pub range: (Option<i64>, Option<i64>),
    /// Include reasoning only for an authorized JSON export.
    pub reasoning: bool,
    /// Include sub-agent streams.
    pub sub_streams: bool,
    /// Absence captures the active history at this read snapshot.
    pub continuation: Option<Id>,
    /// Optional identity check for a previously opened history.
    pub revision: Option<i64>,
}

/// Where in a thread's items a page is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Around {
    /// The items made before this sequence.
    Before(Seq),
    /// The items made after this sequence.
    After(Seq),
    /// The items around this item.
    Item(Id),
}

/// A page of items, oldest first.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemsPage {
    pub projection: orch_api_contract::threads::ThreadContinuationProjection,
    /// The items.
    pub items: Vec<Item>,
    /// Turns needed to display this exact window, including old references.
    pub turns: Vec<Turn>,
    /// Segments needed by this window and its turns.
    pub segments: Vec<Segment>,
    /// Whether older ones exist.
    pub has_older: bool,
    /// Whether newer ones exist.
    pub has_newer: bool,
}

/// A stretch of a command's output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputPage {
    /// The bytes asked for, as text.
    pub text: String,
    /// How long the whole output is, in bytes.
    pub total_len: i64,
}

fn thread_state(word: &str) -> Result<ThreadState, EngineError> {
    read(Json::String(word.to_owned()), "a thread's state")
}

fn blocked_of(row: &threads::ThreadRow) -> Result<Option<ThreadBlocked>, EngineError> {
    row.blocked_reason
        .as_deref()
        .map(|reason| {
            Ok(ThreadBlocked {
                sandbox: None,
                reason: read::<BlockedReason>(Json::String(reason.to_owned()), "a blocked reason")?,
                detail: row.blocked_detail.clone(),
            })
        })
        .transpose()
}

/// How a page around an item is shared out: the newer half is rounded down,
/// and the older side takes the odd one, since what led to an item is read
/// before what followed it.
const fn split_around(limit: i64) -> (i64, i64) {
    let newer = limit / 2;
    (limit - newer, newer)
}

impl Engine {
    /// What the thread's reader is given: a window for a first load, or
    /// what changed after `since`. `viewer` is the person reading, whose
    /// place in the thread is told.
    pub async fn sync(
        &self,
        thread: Id,
        viewer: Option<Id>,
        since: Option<Seq>,
        limit: u32,
    ) -> Result<ThreadSyncData, ReadError> {
        let mut tx = orch_db::queries::feeds::begin_reading(&self.inner.db).await?;
        let data = self.sync_in(&mut tx, thread, viewer, since, limit).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(data)
    }

    /// Reads sync facts and rows on the connection the API also uses for personalized projections.
    pub async fn sync_in(
        &self,
        conn: &mut Conn,
        thread: Id,
        viewer: Option<Id>,
        since: Option<Seq>,
        limit: u32,
    ) -> Result<ThreadSyncData, ReadError> {
        self.sync_view_in(
            conn,
            thread,
            viewer,
            since,
            limit,
            ThreadReadSelector::default(),
        )
        .await
    }

    /// The selector is read-only and captured with all rows in the caller's snapshot.
    pub async fn sync_view_in(
        &self,
        conn: &mut Conn,
        thread: Id,
        viewer: Option<Id>,
        since: Option<Seq>,
        limit: u32,
        selection: ThreadReadSelector,
    ) -> Result<ThreadSyncData, ReadError> {
        let ThreadReadSelector {
            continuation_id: continuation,
            projection_revision: revision,
            ..
        } = selection;
        let row = threads::thread(conn, thread)
            .await?
            .ok_or(ReadError::NotFound)?;
        let projection = continuation_rows::projection(conn, thread, continuation)
            .await?
            .ok_or(ReadError::NotFound)?;
        let rows = read_sync_rows(
            conn,
            &row,
            since,
            limit,
            Time::new(self.inner.clock.now()),
            &projection,
            revision,
        )
        .await?;
        let mut facts = thread_facts(conn, &row, viewer, Time::new(self.inner.clock.now())).await?;
        if let Some(viewer) = viewer {
            facts.my_last_read_seq =
                history_rows::last_read(conn, viewer, thread, projection.viewed_continuation_id)
                    .await?;
        }
        Ok(ThreadSyncData {
            projection: rows.projection,
            inputs_next_after: rows.inputs_next_after,
            inputs_queue_revision: rows.inputs_queue_revision,
            thread: facts,
            seq: rows.seq,
            since,
            complete: rows.complete,
            items: rows.items,
            turns: rows.turns,
            inputs: rows.inputs,
            segments: rows.segments,
            has_older: rows.has_older,
        })
    }

    /// Reads changed rows in the same wire shape as sync, leaving the thread view to its caller.
    pub async fn rows_since(&self, thread: Id, since: Seq) -> Result<ThreadSyncRows, ReadError> {
        let mut tx = orch_db::queries::feeds::begin_reading(&self.inner.db).await?;
        let rows = self.rows_since_in(&mut tx, thread, since).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(rows)
    }

    /// Projects a bounded changed-row set inside the caller's existing repeatable-read snapshot.
    pub async fn rows_since_in(
        &self,
        conn: &mut Conn,
        thread: Id,
        since: Seq,
    ) -> Result<ThreadSyncRows, ReadError> {
        self.rows_since_view_in(conn, thread, since, None, None)
            .await
    }

    pub async fn rows_since_view_in(
        &self,
        conn: &mut Conn,
        thread: Id,
        since: Seq,
        continuation: Option<Id>,
        revision: Option<i64>,
    ) -> Result<ThreadSyncRows, ReadError> {
        let row = threads::thread(conn, thread)
            .await?
            .ok_or(ReadError::NotFound)?;
        // The first-load item limit is unused because this entry point always reads from a sequence.
        const UNUSED_FIRST_LOAD_LIMIT: u32 = 0;
        let projection = continuation_rows::projection(conn, thread, continuation)
            .await?
            .ok_or(ReadError::NotFound)?;
        let rows = read_sync_rows(
            conn,
            &row,
            Some(since),
            UNUSED_FIRST_LOAD_LIMIT,
            Time::new(self.inner.clock.now()),
            &projection,
            revision,
        )
        .await?;
        Ok(rows)
    }
}

/// Projects rows once for the whole sync and for a socket's rows-only read.
async fn read_sync_rows(
    conn: &mut Conn,
    row: &threads::ThreadRow,
    since: Option<Seq>,
    limit: u32,
    now: Time,
    projection: &continuation_rows::Projection,
    revision: Option<i64>,
) -> Result<ThreadSyncRows, ReadError> {
    let thread = row.node_id;
    let seq = row.seq();
    let view = projection.viewed_continuation_id;
    let mut complete = projection.accepts_cursor(since.is_some(), revision);
    let mut inputs_next_after = None;
    let (mut segments, mut turns, inputs, items, has_older) = if !complete {
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), false)
    } else if let Some(since) = since {
        if since > seq {
            complete = false;
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), false)
        } else {
            let budget = THREAD_SYNC_MAX_CHANGES + 1;
            let segments =
                history_rows::changed_segments(conn, thread, view, since, budget).await?;
            let turns = history_rows::changed_turns(conn, thread, view, since, budget).await?;
            let inputs = history_rows::changed_inputs(conn, thread, view, since, budget).await?;
            let items = history_rows::changed_items(conn, thread, view, since, budget).await?;
            if i64::try_from(segments.len() + turns.len() + inputs.len() + items.len())
                .unwrap_or(i64::MAX)
                > THREAD_SYNC_MAX_CHANGES
            {
                complete = false;
                (Vec::new(), Vec::new(), Vec::new(), Vec::new(), false)
            } else {
                (segments, turns, inputs, items, false)
            }
        }
    } else {
        let mut items =
            history_rows::items(conn, thread, view, None, false, None, i64::from(limit)).await?;
        let main = history_rows::items(
            conn,
            thread,
            view,
            None,
            false,
            Some("main"),
            i64::from(limit),
        )
        .await?;
        let known: BTreeSet<_> = items.iter().map(|item| item.id).collect();
        items.extend(main.into_iter().filter(|item| !known.contains(&item.id)));
        items.sort_by_key(|item| item.seq_created);
        let has_older = match items.iter().find(|item| item.stream == "main") {
            Some(first) => !history_rows::items(
                conn,
                thread,
                view,
                Some(first.seq_created),
                false,
                Some("main"),
                1,
            )
            .await?
            .is_empty(),
            None => false,
        };
        let input_limit = i64::from(orch_api_contract::threads::ITEMS_DEFAULT_LIMIT);
        let mut inputs =
            history_rows::open_inputs(conn, thread, view, None, input_limit + 1).await?;
        if i64::try_from(inputs.len()).unwrap_or(i64::MAX) > input_limit {
            inputs.pop();
            inputs_next_after = inputs.last().map(|input| input.id);
        }
        (
            Vec::new(),
            history_rows::window_turns(
                conn,
                thread,
                view,
                &items
                    .iter()
                    .filter_map(|item| item.turn_id)
                    .collect::<Vec<_>>(),
                (view == row.active_continuation_id)
                    .then_some(row.current_turn_id)
                    .flatten(),
                i64::from(THREAD_SYNC_ENDED_TURNS),
            )
            .await?,
            inputs,
            items,
            has_older,
        )
    };
    if complete && since.is_some() {
        inputs_next_after = history_rows::queue_next_after(
            conn,
            thread,
            view,
            orch_api_contract::threads::ITEMS_DEFAULT_LIMIT,
        )
        .await?;
    }
    if complete {
        let known: BTreeSet<_> = turns.iter().map(|turn| turn.id).collect();
        let needed: Vec<_> = items
            .iter()
            .filter_map(|item| item.turn_id)
            .chain(inputs.iter().filter_map(|input| input.turn_id))
            .chain(
                (view == row.active_continuation_id)
                    .then_some(row.current_turn_id)
                    .flatten(),
            )
            .filter(|id| !known.contains(id))
            .collect();
        turns.extend(history_rows::turns_among(conn, thread, view, &needed).await?);
        turns.sort_by_key(|turn| turn.ordinal);
        let known: BTreeSet<_> = segments.iter().map(|segment| segment.id).collect();
        let needed: Vec<_> = items
            .iter()
            .map(|item| item.segment_id)
            .chain(turns.iter().map(|turn| turn.segment_id))
            .chain(projection.viewed_segment_id)
            .filter(|id| !known.contains(id))
            .collect();
        segments.extend(history_rows::segments_among(conn, thread, view, &needed).await?);
        segments.sort_by_key(|segment| segment.ordinal);
    }
    let mut who: Vec<Id> = Vec::new();
    for item in &items {
        who.extend(item.author_id);
        who.extend(item.pinned_by);
    }
    for turn in &turns {
        who.extend(turn.interrupted_by);
        who.extend(bypass_people(turn.limit_bypass.as_ref()));
    }
    for input in &inputs {
        who.extend(input.author_id);
        who.extend(input.run_as);
        who.extend(bypass_people(input.limit_bypass.as_ref()));
    }
    for segment in &segments {
        who.extend(segment.started_by);
    }
    let people = People::load(conn, who)
        .await?
        .with_approvals(
            conn,
            items
                .iter()
                .filter_map(source_approval)
                .chain(inputs.iter().filter_map(|input| input.approval_id)),
        )
        .await?;
    let segment_providers: BTreeMap<_, _> = segments
        .iter()
        .filter_map(|segment| segment.provider_id.map(|provider| (segment.id, provider)))
        .collect();
    let mut displays: BTreeMap<(Id, String), String> = BTreeMap::new();
    let mut turn_views = Vec::with_capacity(turns.len());
    for turn in &turns {
        let display = match segment_providers.get(&turn.segment_id) {
            Some(provider) => {
                let key = (*provider, turn.model.clone());
                if let Some(display) = displays.get(&key) {
                    display.clone()
                } else {
                    let display = resources::model(conn, *provider, &turn.model)
                        .await?
                        .map(|model| model.display_name)
                        .unwrap_or_default();
                    displays.insert(key, display.clone());
                    display
                }
            }
            None => String::new(),
        };
        turn_views.push(turn_view(turn, &display, &people)?);
    }
    let mut segment_views = Vec::with_capacity(segments.len());
    for segment in &segments {
        segment_views.push(segment_view(conn, segment, &people, now).await?);
    }
    let item_views = items.iter().map(|item| item_view(item, &people)).collect();
    let input_views = inputs
        .iter()
        .map(|input| input_view(input, &people))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ThreadSyncRows {
        projection: crate::continuations::projection::scope(projection),
        inputs_next_after,
        inputs_queue_revision: continuation_rows::continuation(conn, thread, view)
            .await?
            .ok_or(ReadError::NotFound)?
            .queue_revision,
        seq,
        state_stamp: row.seq_state,
        complete,
        items: item_views,
        turns: turn_views,
        inputs: input_views,
        segments: segment_views,
        has_older,
    })
}

/// What the engine owns of a thread, for a reader.
async fn thread_facts(
    conn: &mut Conn,
    row: &threads::ThreadRow,
    viewer: Option<Id>,
    now: Time,
) -> Result<ThreadFacts, EngineError> {
    {
        let settings = crate::resolved::load(conn, row.node_id).await?;
        let environment = match row.environment_id {
            Some(id) => {
                orch_db::queries::environments::thread_engine::environment(conn, id).await?
            }
            None => None,
        };
        let cwd = environment
            .as_ref()
            .map_or_else(String::new, |environment| {
                crate::sessions::absolute_cwd(
                    &environment.workspace_root,
                    &row.workspace_dir,
                    &settings.cwd,
                )
            });
        let environment_ref = match &environment {
            Some(environment) => Some(read(
                environment_ref_json(conn, environment, now).await?,
                "an environment reference",
            )?),
            None => None,
        };
        let model_display = match (&settings.model, row.provider_id) {
            (Some(model), Some(provider)) => resources::model(conn, provider, model)
                .await?
                .map(|row| row.display_name)
                .filter(|name| !name.is_empty()),
            _ => None,
        };
        let usage = row
            .usage
            .clone()
            .map(|usage| read::<ThreadUsage>(usage, "a thread's usage"))
            .transpose()?;
        let credits =
            orch_db::usage::ledger::status_credits(conn, row.node_id, row.current_turn_id).await?;
        let costs = if credits.credits.is_some() || credits.unrated_tokens > 0 {
            Some(ThreadCosts {
                total: RecordedCost {
                    credits: credits.credits,
                    estimated_credits: credits.estimated_credits,
                    unrated_tokens: credits.unrated_tokens,
                },
                turn: row.current_turn_id.map(|turn_id| TurnRecordedCost {
                    turn_id,
                    amounts: RecordedCost {
                        credits: credits.turn_credits,
                        estimated_credits: credits.turn_estimated_credits,
                        unrated_tokens: credits.turn_unrated_tokens,
                    },
                }),
            })
        } else {
            None
        };
        let pending_inputs = threads::count_pending_inputs(conn, row.node_id).await?;
        let pending_approvals =
            approval_queries::count_pending_of_thread(conn, row.node_id).await?;
        let queue = queue_of(conn, row).await?;
        let my_last_read_seq = match viewer {
            Some(viewer) => threads::last_read(conn, viewer, row.node_id).await?,
            None => None,
        };
        let mut source = row
            .source
            .as_ref()
            .map(|source| {
                read::<orch_api_contract::source::SourceInfo>(
                    source.get("info").unwrap_or(source).clone(),
                    "a thread's source",
                )
            })
            .transpose()?;
        if let Some(source) = &mut source
            && let Some(reference) = &source.job
            && let Some(job) = orch_db::queries::jobs::job(conn, reference.id).await?
        {
            source.job = Some(crate::jobs::source::reference(&job));
        }
        let links = read::<Vec<orch_api_contract::source::ThreadLink>>(
            row.links.clone(),
            "a thread's links",
        )?;
        let suggested_links = orch_db::queries::source::last_agent_message(conn, row.node_id)
            .await?
            .as_ref()
            .map(crate::git_work::suggested_links)
            .unwrap_or_default();
        let mut work = row
            .work_observed
            .clone()
            .map(|work| {
                read::<orch_api_contract::source::DirectoryWork>(
                    work,
                    "a directory's last Git observation",
                )
            })
            .transpose()?;
        if let Some(work) = &mut work {
            let online = match environment
                .as_ref()
                .and_then(|environment| environment.machine_id)
            {
                Some(machine) => orch_db::queries::resources::machine(conn, machine)
                    .await?
                    .is_some_and(|machine| machine.online && machine.is_active()),
                None => false,
            };
            if environment
                .as_ref()
                .is_none_or(|environment| environment.status != "running")
                || !online
            {
                work.stale = true;
                work.reason = Some("source_offline".to_owned());
            }
        }
        let display_facts =
            orch_db::queries::source::thread_display_facts(conn, &[row.node_id], viewer, now)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| EngineError::unreadable("a readable thread has no icon facts"))?;
        let failed = display_facts.failed;
        let display = thread_display(&display_facts);
        let attention = orch_api_contract::source::ThreadAttention {
            uncommitted: work.as_ref().and_then(|work| work.uncommitted),
            unpushed: work.as_ref().and_then(|work| work.unpushed),
            failed,
            as_of: work.as_ref().map(|work| work.as_of),
            stale: work.as_ref().is_some_and(|work| work.stale),
            reason: work.as_ref().and_then(|work| work.reason.clone()),
        };
        Ok(ThreadFacts {
            active_continuation_id: row.active_continuation_id,
            execution_revision: row.execution_revision,
            id: row.node_id,
            topic_id: row.topic_id,
            state: thread_state(&row.state)?,
            display,
            side_of: row.side_of,
            side_from_turn: row.side_from_turn.map(i64::from),
            side_reading: row
                .side_reading
                .as_deref()
                .map(|word| read::<SideReading>(Json::String(word.to_owned()), "a side reading"))
                .transpose()?,
            starting_step: row
                .starting_step
                .as_deref()
                .map(|step| read::<StartingStep>(Json::String(step.to_owned()), "a starting step"))
                .transpose()?,
            blocked: blocked_of(row)?,
            queue,
            environment: environment_ref,
            directory: row.workspace_dir.clone(),
            source,
            links,
            suggested_links,
            work,
            attention,
            model: settings.model,
            model_display,
            effort: settings.effort,
            cwd,
            current_turn_id: row.current_turn_id,
            current_segment_id: row.current_segment_id,
            pending_approvals,
            pending_inputs,
            usage,
            costs,
            last_activity_at: row.last_activity_at.map(Time::new),
            seq: row.seq(),
            activity_seq: row.activity_seq,
            my_last_read_seq,
        })
    }
}

impl Engine {
    /// A page of a thread's items, oldest first, around the position asked.
    pub async fn items(
        &self,
        thread: Id,
        around: Around,
        limit: u32,
        stream: Option<&str>,
    ) -> Result<ItemsPage, ReadError> {
        let mut tx = orch_db::queries::feeds::begin_reading(&self.inner.db).await?;
        let page = self
            .items_in(&mut tx, thread, around, limit, stream)
            .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(page)
    }

    /// Reads one item window and its reference metadata on the supplied coherent snapshot.
    pub async fn items_in(
        &self,
        conn: &mut Conn,
        thread: Id,
        around: Around,
        limit: u32,
        stream: Option<&str>,
    ) -> Result<ItemsPage, ReadError> {
        let projection = continuation_rows::projection(conn, thread, None)
            .await?
            .ok_or(ReadError::NotFound)?;
        self.items_view_in(
            conn,
            thread,
            around,
            limit,
            stream,
            ThreadReadSelector {
                continuation_id: None,
                projection_revision: Some(projection.projection_revision),
                ..Default::default()
            },
        )
        .await
    }

    pub async fn items_view_in(
        &self,
        conn: &mut Conn,
        thread: Id,
        around: Around,
        limit: u32,
        stream: Option<&str>,
        selection: ThreadReadSelector,
    ) -> Result<ItemsPage, ReadError> {
        let ThreadReadSelector {
            continuation_id: continuation,
            projection_revision: revision,
            ..
        } = selection;
        if limit == 0 {
            return Err(ReadError::BadRequest("a page holds at least one item"));
        }
        let projection = continuation_rows::projection(conn, thread, continuation)
            .await?
            .ok_or(ReadError::NotFound)?;
        if !projection.accepts_cursor(true, revision) {
            return Err(ReadError::ProjectionChanged(
                crate::continuations::projection::scope(&projection),
            ));
        }
        let view = projection.viewed_continuation_id;
        let limit = i64::from(limit);
        let rows = match around {
            Around::Before(seq) => {
                history_rows::items(conn, thread, view, Some(seq), false, stream, limit).await?
            }
            Around::After(seq) => {
                history_rows::items(conn, thread, view, Some(seq), true, stream, limit).await?
            }
            Around::Item(id) => {
                let centre = history_rows::item(conn, thread, view, id)
                    .await?
                    .ok_or(ReadError::NotFound)?;
                let (older, newer) = split_around(limit);
                let mut rows = history_rows::items(
                    conn,
                    thread,
                    view,
                    Some(centre.seq_created.saturating_add(1)),
                    false,
                    stream,
                    older,
                )
                .await?;
                rows.extend(
                    history_rows::items(
                        conn,
                        thread,
                        view,
                        Some(centre.seq_created),
                        true,
                        stream,
                        newer,
                    )
                    .await?,
                );
                rows
            }
        };
        let has_older = match rows.first() {
            Some(first) => !history_rows::items(
                conn,
                thread,
                view,
                Some(first.seq_created),
                false,
                stream,
                1,
            )
            .await?
            .is_empty(),
            None => match around {
                Around::After(seq) => !history_rows::items(
                    conn,
                    thread,
                    view,
                    Some(seq.saturating_add(1)),
                    false,
                    stream,
                    1,
                )
                .await?
                .is_empty(),
                _ => false,
            },
        };
        let has_newer = match rows.last() {
            Some(last) => {
                !history_rows::items(conn, thread, view, Some(last.seq_created), true, stream, 1)
                    .await?
                    .is_empty()
            }
            None => match around {
                Around::Before(seq) => !history_rows::items(
                    conn,
                    thread,
                    view,
                    Some(seq.saturating_sub(1)),
                    true,
                    stream,
                    1,
                )
                .await?
                .is_empty(),
                _ => false,
            },
        };
        let mut who: Vec<Id> = Vec::new();
        for item in &rows {
            who.extend(item.author_id);
            who.extend(item.pinned_by);
        }
        let people = People::load(conn, who)
            .await?
            .with_approvals(conn, rows.iter().filter_map(source_approval))
            .await?;
        let turn_ids: Vec<_> = rows.iter().filter_map(|row| row.turn_id).collect();
        let turns = history_rows::turns_among(conn, thread, view, &turn_ids).await?;
        let segment_ids: Vec<_> = rows
            .iter()
            .map(|row| row.segment_id)
            .chain(turns.iter().map(|turn| turn.segment_id))
            .collect();
        let segments = history_rows::segments_among(conn, thread, view, &segment_ids).await?;
        let (turns, segments) = self.project_history_in(conn, &turns, &segments).await?;
        Ok(ItemsPage {
            projection: crate::continuations::projection::scope(&projection),
            items: rows.iter().map(|item| item_view(item, &people)).collect(),
            turns,
            segments,
            has_older,
            has_newer,
        })
    }

    /// One item of a thread.
    pub async fn item(&self, thread: Id, item: Id) -> Result<Item, ReadError> {
        let mut tx = orch_db::queries::feeds::begin_reading(&self.inner.db).await?;
        let item = self.item_in(&mut tx, thread, item).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(item)
    }

    /// Projects an exact item on the same connection as its personalized request cards.
    pub async fn item_in(&self, conn: &mut Conn, thread: Id, item: Id) -> Result<Item, ReadError> {
        self.item_view_in(conn, thread, item, None, None).await
    }

    /// A direct known-item link resolves its actual owner unless a prefix view was explicitly chosen.
    pub async fn item_view_in(
        &self,
        conn: &mut Conn,
        thread: Id,
        item: Id,
        continuation: Option<Id>,
        revision: Option<i64>,
    ) -> Result<Item, ReadError> {
        let owner = match continuation {
            Some(owner) => owner,
            None => history_rows::item_owner(conn, thread, item)
                .await?
                .ok_or(ReadError::NotFound)?,
        };
        let projection = continuation_rows::projection(conn, thread, Some(owner))
            .await?
            .ok_or(ReadError::NotFound)?;
        if !projection.accepts_cursor(false, revision) {
            return Err(ReadError::ProjectionChanged(
                crate::continuations::projection::scope(&projection),
            ));
        }
        let row = history_rows::item(conn, thread, owner, item)
            .await?
            .ok_or(ReadError::NotFound)?;
        let people = People::load(conn, [row.author_id, row.pinned_by].into_iter().flatten())
            .await?
            .with_approvals(conn, source_approval(&row))
            .await?;
        Ok(item_view(&row, &people))
    }

    /// Native turn/segment projection for bounded timeline and historical metadata pages.
    pub async fn project_history_in(
        &self,
        conn: &mut Conn,
        turns: &[TurnRow],
        segments: &[SegmentRow],
    ) -> Result<(Vec<Turn>, Vec<Segment>), ReadError> {
        let who: Vec<_> = turns
            .iter()
            .flat_map(|turn| {
                turn.interrupted_by
                    .into_iter()
                    .chain(bypass_people(turn.limit_bypass.as_ref()))
            })
            .chain(segments.iter().filter_map(|segment| segment.started_by))
            .collect();
        let people = People::load(conn, who).await?;
        let providers: BTreeMap<_, _> = segments
            .iter()
            .filter_map(|segment| segment.provider_id.map(|provider| (segment.id, provider)))
            .collect();
        let mut displays: BTreeMap<(Id, String), String> = BTreeMap::new();
        let mut shown_turns = Vec::with_capacity(turns.len());
        for turn in turns {
            let display = if let Some(provider) = providers.get(&turn.segment_id) {
                let key = (*provider, turn.model.clone());
                if let Some(display) = displays.get(&key) {
                    display.clone()
                } else {
                    let display = resources::model(conn, *provider, &turn.model)
                        .await?
                        .map(|model| model.display_name)
                        .unwrap_or_default();
                    displays.insert(key, display.clone());
                    display
                }
            } else {
                String::new()
            };
            shown_turns.push(turn_view(turn, &display, &people)?);
        }
        let mut shown_segments = Vec::with_capacity(segments.len());
        let now = Time::new(self.inner.clock.now());
        for segment in segments {
            shown_segments.push(segment_view(conn, segment, &people, now).await?);
        }
        Ok((shown_turns, shown_segments))
    }

    /// Everything a thread holds, for an export: its segments, its turns and its
    /// items, oldest first. `from` and `to` (turn numbers) keep the turns between
    /// them and the items made in them; items that belong to no turn stay.
    /// `reasoning` and `sub_streams` say whether those kinds of item are kept.
    pub async fn export(
        &self,
        thread: Id,
        viewer: Option<Id>,
        range: (Option<i64>, Option<i64>),
        reasoning: bool,
        sub_streams: bool,
    ) -> Result<ThreadExportData, ReadError> {
        let mut tx = orch_db::queries::feeds::begin_reading(&self.inner.db).await?;
        let _ = viewer;
        let result = self
            .export_view_in(
                &mut tx,
                thread,
                ExportSelection {
                    range,
                    reasoning,
                    sub_streams,
                    continuation: None,
                    revision: None,
                },
            )
            .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(result)
    }

    /// Snapshot-sharing export for HTTP, archives and other authorized server readers.
    pub async fn export_view_in(
        &self,
        conn: &mut Conn,
        thread: Id,
        how: ExportSelection,
    ) -> Result<ThreadExportData, ReadError> {
        threads::thread(conn, thread)
            .await?
            .ok_or(ReadError::NotFound)?;
        let projection = continuation_rows::projection(conn, thread, how.continuation)
            .await?
            .ok_or(ReadError::NotFound)?;
        if !projection.accepts_cursor(false, how.revision) {
            return Err(ReadError::ProjectionChanged(
                crate::continuations::projection::scope(&projection),
            ));
        }
        let view = projection.viewed_continuation_id;
        let segments = history_rows::segments(conn, thread, view, None, i64::MAX).await?;
        let turns =
            orch_db::queries::thread_history::turns_between(conn, thread, view, 1, i64::MAX)
                .await?;
        let range = how.range;
        let kept_turns: Vec<_> = turns
            .into_iter()
            .filter(|turn| {
                range.0.is_none_or(|from| i64::from(turn.ordinal) >= from)
                    && range.1.is_none_or(|to| i64::from(turn.ordinal) <= to)
            })
            .collect();
        let kept_ids: BTreeSet<Id> = kept_turns.iter().map(|turn| turn.id).collect();
        let narrowed = range.0.is_some() || range.1.is_some();
        let mut rows: Vec<ItemRow> = Vec::new();
        let mut after: Seq = 0;
        loop {
            let page = history_rows::items(
                conn,
                thread,
                view,
                Some(after),
                true,
                None,
                orch_db::queries::thread_history::PAGE,
            )
            .await?;
            let Some(last) = page.last().map(|row| row.seq_created) else {
                break;
            };
            rows.extend(page);
            after = last;
        }
        rows.retain(|row| {
            (how.reasoning || row.kind != "reasoning")
                && (how.sub_streams || !row.stream.starts_with("sub:"))
                && (!narrowed || row.turn_id.is_none_or(|turn| kept_ids.contains(&turn)))
        });
        let mut who: Vec<Id> = Vec::new();
        for row in &rows {
            who.extend(row.author_id);
            who.extend(row.pinned_by);
        }
        for turn in &kept_turns {
            who.extend(turn.interrupted_by);
            who.extend(bypass_people(turn.limit_bypass.as_ref()));
        }
        for segment in &segments {
            who.extend(segment.started_by);
        }
        let people = People::load(conn, who)
            .await?
            .with_approvals(conn, rows.iter().filter_map(source_approval))
            .await?;
        let mut segment_providers: BTreeMap<Id, Id> = BTreeMap::new();
        for segment in &segments {
            if let Some(provider) = segment.provider_id {
                segment_providers.insert(segment.id, provider);
            }
        }
        let mut turn_views = Vec::with_capacity(kept_turns.len());
        for turn in &kept_turns {
            let display = match segment_providers.get(&turn.segment_id) {
                Some(provider) => resources::model(conn, *provider, &turn.model)
                    .await?
                    .map(|model| model.display_name)
                    .unwrap_or_default(),
                None => String::new(),
            };
            turn_views.push(turn_view(turn, &display, &people)?);
        }
        let mut segment_views = Vec::with_capacity(segments.len());
        for segment in &segments {
            segment_views.push(
                segment_view(conn, segment, &people, Time::new(self.inner.clock.now())).await?,
            );
        }
        Ok(ThreadExportData {
            projection: crate::continuations::projection::scope(&projection),
            segments: segment_views,
            turns: turn_views,
            items: rows.iter().map(|row| item_view(row, &people)).collect(),
        })
    }

    /// A stretch of the output of a command: `limit` bytes from `offset`.
    /// The output of a command that is long is kept in the store of content
    /// once the command is over, and is read from there; what the item holds
    /// itself is read now. A command that is still running and has printed
    /// more than the item keeps has its whole nowhere yet.
    pub async fn command_output(
        &self,
        thread: Id,
        item: Id,
        offset: i64,
        limit: i64,
    ) -> Result<OutputPage, ReadError> {
        self.command_output_view(thread, item, offset, limit, None, None)
            .await
    }

    pub async fn command_output_view(
        &self,
        thread: Id,
        item: Id,
        offset: i64,
        limit: i64,
        continuation: Option<Id>,
        revision: Option<i64>,
    ) -> Result<OutputPage, ReadError> {
        let mut tx = orch_db::queries::feeds::begin_reading(&self.inner.db).await?;
        let owner = match continuation {
            Some(owner) => owner,
            None => history_rows::item_owner(&mut tx, thread, item)
                .await?
                .ok_or(ReadError::NotFound)?,
        };
        let projection = continuation_rows::projection(&mut tx, thread, Some(owner))
            .await?
            .ok_or(ReadError::NotFound)?;
        if !projection.accepts_cursor(false, revision) {
            return Err(ReadError::ProjectionChanged(
                crate::continuations::projection::scope(&projection),
            ));
        }
        let row = history_rows::item(&mut tx, thread, owner, item)
            .await?
            .ok_or(ReadError::NotFound)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let tool = row.payload.get("tool");
        if row.kind != "tool_call"
            || tool
                .and_then(|tool| tool.get("kind"))
                .and_then(Json::as_str)
                != Some("command")
        {
            return Err(ReadError::NotACommand);
        }
        let output = tool
            .and_then(|tool| tool.get("output"))
            .cloned()
            .unwrap_or(Json::Null);
        if let Some(hash) = output
            .get("blob")
            .and_then(Json::as_str)
            .and_then(|text| text.parse::<Hash>().ok())
        {
            return self.output_from_store(&hash, offset, limit).await;
        }
        let truncated = output
            .get("truncated")
            .and_then(Json::as_bool)
            .unwrap_or(false);
        if truncated {
            return Err(ReadError::OutputInBlobStore);
        }
        let inline = output
            .get("inline")
            .and_then(Json::as_str)
            .unwrap_or_default();
        let bytes = inline.as_bytes();
        let total = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
        let from = usize::try_from(offset.clamp(0, total)).unwrap_or(bytes.len());
        let to = usize::try_from((offset.max(0).saturating_add(limit.max(0))).clamp(0, total))
            .unwrap_or(bytes.len());
        Ok(OutputPage {
            text: String::from_utf8_lossy(&bytes[from.min(to)..to]).into_owned(),
            total_len: total,
        })
    }

    /// A stretch of an output that is in the store of content as a blob of its own: only the stretch is
    /// read, however long the whole is.
    async fn output_from_store(
        &self,
        hash: &Hash,
        offset: i64,
        limit: i64,
    ) -> Result<OutputPage, ReadError> {
        let store = self.blobs().ok_or(ReadError::OutputInBlobStore)?;
        let total = store
            .size(hash)
            .await
            .map_err(EngineError::unreadable)?
            .ok_or_else(|| EngineError::unreadable("the output is no longer in the store"))?;
        let start = offset.clamp(0, total);
        let end = start.saturating_add(limit.max(0)).min(total);
        let text = if end > start {
            let read = store
                .get(
                    hash,
                    Some(ByteRange::Between {
                        start,
                        end: end - 1,
                    }),
                )
                .await
                .map_err(EngineError::unreadable)?;
            let bytes = orch_blob_contract::store::read_all(read.body)
                .await
                .map_err(EngineError::unreadable)?;
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            String::new()
        };
        Ok(OutputPage {
            text,
            total_len: total,
        })
    }
}

// ---------------------------------------------------------------- a thread as a node shows it

/// What a thread runs on, named: the profile and the provider as the wire
/// has them, and what the thread's current segment says about how they are
/// placed.
#[derive(Debug, Clone, PartialEq)]
pub struct BindingParts {
    /// The agent profile.
    pub agent: orch_api_contract::resources::AgentRef,
    /// The provider.
    pub provider: orch_api_contract::resources::ProviderRef,
    /// The arrangement type the thread took.
    pub arrangement: orch_api_contract::binding::ArrangementType,
    /// What carries out its tool calls.
    pub executor: Option<String>,
    /// The tool set of that executor.
    pub tool_set: Option<String>,
    /// How its harness reaches the model.
    pub credential_path: orch_api_contract::binding::CredentialPath,
    /// What the profile's harness declares of itself.
    pub descriptor: Option<orch_agent_contract::descriptor::AgentDescriptor>,
}

/// What a thread's own view needs beyond what any node has: the engine's
/// facts, what it runs on, and how many other threads share its directory.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadParts {
    /// What the engine owns of the thread.
    pub facts: ThreadFacts,
    /// What it runs on; none until it is bound.
    pub binding: Option<BindingParts>,
    /// How many other threads work in the same directory of the same
    /// environment.
    pub directory_threads: i64,
    /// The resolved `agent.memory` setting.
    pub memory_setting: orch_agent_contract::session::MemorySetting,
}

/// The thread's own part of its view, for a reader (`viewer`, whose place
/// in the thread is told). None where there is no such thread.
pub async fn thread_parts(
    conn: &mut Conn,
    thread: Id,
    viewer: Option<Id>,
    now: Time,
) -> Result<Option<ThreadParts>, EngineError> {
    {
        let Some(row) = threads::thread(conn, thread).await? else {
            return Ok(None);
        };
        let facts = thread_facts(conn, &row, viewer, now).await?;
        let settings = crate::resolved::load(conn, thread).await?;
        let binding = match (row.current_segment_id, row.agent_id, row.provider_id) {
            (Some(segment), Some(agent), Some(provider)) => {
                let segment = threads::segment(conn, segment).await?;
                let agent = resources::agent(conn, agent).await?;
                let provider = resources::provider(conn, provider).await?;
                match (segment, agent, provider) {
                    (Some(segment), Some(agent), Some(provider)) => Some(BindingParts {
                        agent: read(agent_ref_json(conn, &agent).await?, "an agent reference")?,
                        provider: read(
                            provider_ref_json(conn, &provider).await?,
                            "a provider reference",
                        )?,
                        arrangement: read(
                            Json::String(row.arrangement.clone().unwrap_or(segment.arrangement)),
                            "an arrangement type",
                        )?,
                        executor: row.executor.clone().or(segment.executor),
                        tool_set: row.tool_set.clone().or(segment.tool_set),
                        credential_path: read(
                            Json::String(segment.credential_path),
                            "a credential path",
                        )?,
                        // An invalid persisted profile disables its descriptor-derived
                        // controls; it must not hide authoritative turn/state progress.
                        // Execution admission continues to reject that profile strictly.
                        descriptor: match crate::harnesses::descriptor_of(&agent) {
                            Ok(descriptor) => descriptor,
                            Err(_) => {
                                tracing::warn!(agent = %agent.id, "an invalid profile descriptor was omitted from its thread view");
                                None
                            }
                        },
                    }),
                    _ => None,
                }
            }
            _ => None,
        };
        let directory_threads = match row.environment_id {
            Some(environment) => {
                threads::threads_in_directory(conn, environment, &row.workspace_dir, thread).await?
            }
            None => 0,
        };
        Ok(Some(ThreadParts {
            facts,
            binding,
            directory_threads,
            memory_setting: settings.memory,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_letter_of_a_name_is_the_avatar_in_upper_case() {
        assert_eq!(avatar_text("ada lovelace"), "A");
        assert_eq!(avatar_text(""), "");
        assert_eq!(avatar_text("\u{4e2d}\u{6587}"), "\u{4e2d}");
    }

    #[test]
    fn imported_history_has_no_local_execution_references() {
        let segment: Segment = serde_json::from_value(json!({
            "id": orch_types::new_id(), "ordinal": 1, "agent": null, "provider": null, "environment": null,
            "env_generation": 0, "arrangement": "separated", "executor": null, "tool_set": null,
            "credential_path": "native", "started_by": null, "started_at": "2025-01-01T00:00:00Z",
            "ended_at": "2025-01-01T00:00:00Z", "end_reason": null,
            "import_fidelity": "structured", "import_report": null, "seq_updated": 1,
        })).expect("an ended history segment with no local bindings is readable");
        assert!(segment.agent.is_none());
        assert!(segment.provider.is_none());
        assert!(segment.environment.is_none());
        let wire = serde_json::to_value(segment).expect("a segment serializes");
        assert!(wire["agent"].is_null());
        assert!(wire["provider"].is_null());
        assert!(wire["environment"].is_null());
    }

    #[test]
    fn search_looks_through_what_people_would_look_for() {
        let command = json!({
            "title": "Run tests", "tool": { "kind": "command", "command": "cargo test", "output": { "inline": "ok" } }
        });
        assert_eq!(
            search_text("tool_call", &command).as_deref(),
            Some("Run tests\ncargo test")
        );
        let message = json!({ "text": "hello", "text_len": 5 });
        assert_eq!(
            search_text("agent_message", &message).as_deref(),
            Some("hello")
        );
        assert_eq!(search_text("marker", &json!({})), None);
        let long = json!({ "text": "x".repeat(SEARCH_TEXT_MAX_CHARS + 5) });
        assert_eq!(
            search_text("agent_message", &long).map(|text| text.chars().count()),
            Some(SEARCH_TEXT_MAX_CHARS)
        );
    }
}
