//! Private authored input schedules, civil timing and visible occurrence outcomes.

use crate::common::{Page, PageQuery};
use crate::nodes::{CreateThread, NodeRef};
use crate::routes::Access::{Permission, Rule, User};
use crate::threads::ContentPart;
use crate::users::UserRef;
use orch_types::Json;
use orch_types::{Id, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A selection against one exact loaded native catalogue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeInvocation {
    /// Opaque identity of the artifact the adapter can actually invoke.
    pub action_id: String,
    /// The loaded mapping which must still hold through dispatch.
    pub revision: String,
    /// Conversation content; never copied into audit details.
    pub arguments: Json,
}

/// A fixed instant, or an explicit civil daily/weekly time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputTiming {
    /// One UTC instant with the editor's chosen IANA zone.
    Once {
        /// The future instant.
        at: Time,
        /// The chosen IANA zone.
        zone: String,
    },
    /// A civil occurrence; ambiguous times use the earlier instant once.
    Repeat {
        /// Daily or weekly.
        frequency: InputFrequency,
        /// HH:MM local time.
        time: String,
        /// IANA zone.
        zone: String,
        /// ISO weekdays, 1..7; empty for daily.
        weekdays: Vec<u8>,
    },
}

/// A repeating civil frequency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputFrequency {
    /// Every local day.
    Daily,
    /// The selected local weekdays.
    Weekly,
}

/// The ordinary input's destination, never another trigger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputScheduleTarget {
    /// An existing thread of the topic.
    Thread {
        /// The destination thread.
        thread_id: Id,
    },
    /// A new thread at each occurrence.
    NewThread {
        /// Choices resolved again when due; first_input is refused.
        config: Box<CreateThread>,
    },
}

/// The future-work lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputScheduleState {
    /// Future occurrences are enabled.
    Active,
    /// Future occurrences are paused.
    Paused,
    /// A single occurrence is over.
    Finished,
    /// Future work was removed; queued input is unchanged.
    Deleted,
}

/// The author's private frozen draft, shown to nobody else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputScheduleDefinition {
    /// Ordinary destination choices.
    pub target: InputScheduleTarget,
    /// Frozen authored parts.
    pub content: Vec<ContentPart>,
    /// Optional verified native selection.
    pub native_action: Option<NativeInvocation>,
}

/// Redacted destination metadata without the private creation definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputScheduleDestination {
    /// Existing destination, subject to current View.
    pub thread: Option<NodeRef>,
    /// Each occurrence makes a new thread.
    pub new_thread: bool,
}

/// Safe metadata and, for its author only, its prepared definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputSchedule {
    /// Schedule identity.
    pub id: Id,
    /// Topic, redacted where unreadable.
    pub topic: Option<NodeRef>,
    /// The person whose current authority will be used.
    pub author: UserRef,
    /// Authored name.
    pub name: String,
    /// Current redacted destination.
    pub destination: InputScheduleDestination,
    /// Author-only draft.
    pub definition: Option<InputScheduleDefinition>,
    /// Explicit civil timing.
    pub timing: InputTiming,
    /// Future-work state.
    pub state: InputScheduleState,
    /// Next due instant, if any.
    pub next_run_at: Option<Time>,
    /// Compare-and-set revision.
    pub revision: i64,
    /// Latest occurrence, where any.
    pub last_run: Option<InputScheduleRun>,
    /// Creation time.
    pub created_at: Time,
    /// Latest edit.
    pub updated_at: Time,
    /// Topic feed stamp.
    pub topic_seq: Seq,
}

/// What happened to one distinct occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputScheduleRunState {
    /// Ordinary input is queued.
    Queued,
    /// Ordinary input is held.
    Blocked,
    /// Its ordinary turn started.
    Started,
    /// It could not start or was cancelled.
    Failed,
    /// A nonexistent civil time or a previous pending occurrence.
    Skipped,
}

/// One safe occurrence outcome, with names subject to current View.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputScheduleRun {
    /// Run identity.
    pub id: Id,
    /// Stable instant or civil date/time key.
    pub occurrence: String,
    /// Null only for a nonexistent civil time.
    pub scheduled_for: Option<Time>,
    /// Actual queue/turn outcome.
    pub state: InputScheduleRunState,
    /// Current ordinary thread reference.
    pub thread: Option<NodeRef>,
    /// The accepted input, where any.
    pub input_id: Option<Id>,
    /// Its turn, when started.
    pub turn_id: Option<Id>,
    /// Safe explanation without the prompt.
    pub reason: Option<String>,
    /// Occurrence creation time.
    pub created_at: Time,
    /// Actual start, if any.
    pub started_at: Option<Time>,
}

/// A browser-authored future input; it reserves no future authority or credits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputScheduleCreateBody {
    /// Optional name; a generated name otherwise.
    pub name: Option<String>,
    /// Existing or new destination.
    pub target: InputScheduleTarget,
    /// Frozen authored parts.
    pub content: Vec<ContentPart>,
    /// Optional native selection.
    pub native_action: Option<NativeInvocation>,
    /// Explicit future timing.
    pub timing: InputTiming,
}

/// A revision-checked edit; native_action null removes that selection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputScheduleUpdateBody {
    /// Revision shown by the editor.
    pub revision: i64,
    /// New name.
    pub name: Option<String>,
    /// New destination.
    pub target: Option<InputScheduleTarget>,
    /// New frozen parts.
    pub content: Option<Vec<ContentPart>>,
    /// Omitted preserves, null removes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::common::absent_or_null"
    )]
    pub native_action: Option<Option<NativeInvocation>>,
    /// New timing.
    pub timing: Option<InputTiming>,
    /// Active or paused only.
    pub state: Option<InputScheduleState>,
}

/// A topic's readable schedules, next due first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputScheduleListQuery {
    /// Ordinary paging.
    #[serde(flatten)]
    pub page: PageQuery,
    /// Optional lifecycle filter.
    pub state: Option<InputScheduleState>,
}

crate::endpoints! {
    /// Lists readable metadata, and each author's own draft.
    TopicInputScheduleList: GET "/topics/{id}/input-schedules", Permission("topic.view"), query InputScheduleListQuery, none, json Page<InputSchedule>, 200;
    /// Freezes a future input, without starting a turn.
    TopicInputScheduleCreate: POST "/topics/{id}/input-schedules", User, none, json InputScheduleCreateBody, json InputSchedule, 201;
    /// Reads a schedule as author or current destination reader.
    InputScheduleGet: GET "/input-schedules/{id}", User, none, none, json InputSchedule, 200;
    /// Lists readable occurrence outcomes, newest first.
    InputScheduleRunList: GET "/input-schedules/{id}/runs", User, query PageQuery, none, json Page<InputScheduleRun>, 200;
    /// Edits an authored definition or controls future occurrences.
    InputScheduleUpdate: PATCH "/input-schedules/{id}", Rule("author for definition; author or current destination controller for state"), none, json InputScheduleUpdateBody, json InputSchedule, 200;
    /// Stops future occurrences, leaving accepted inputs in the ordinary tray.
    InputScheduleDelete: DELETE "/input-schedules/{id}", Rule("author or current destination controller"), none, none, none, 204;
}
