//! Current thread command rosters, reviewed exact End requests and factual outcomes.

use crate::routes::Access::Permission;
use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A stopped turn's factual native command cleanup counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandCleanup {
    /// Confirmed native ends.
    pub ended: i64,
    /// Commands known still to run.
    pub still_running: i64,
    /// Ends that remain unconfirmed.
    pub unknown: i64,
}

/// Availability of current native observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundState {
    /// Fresh facts of an existing native session.
    Live,
    /// The machine or session is disconnected.
    Offline,
    /// The native facts are unavailable.
    Unavailable,
}

/// Where a command executes, with private directories cut for the current reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundPlace {
    /// The thread's environment.
    Environment,
    /// Its provider's runtime.
    Provider,
}

/// One current command with its durable source links and optional native facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BackgroundCommand {
    /// An opaque command identity scoped to its captured native generation.
    pub id: String,
    /// The retained segment that owns it.
    pub segment_id: Id,
    /// A bounded redacted preview, when readable.
    pub command: Option<String>,
    /// Whether the preview was shortened.
    pub command_truncated: bool,
    /// Its directory, when reported and readable.
    pub cwd: Option<String>,
    /// Whether the reader's rights hid that directory.
    pub cwd_redacted: bool,
    /// Its execution place.
    pub place: BackgroundPlace,
    /// Its native start time, if reported.
    pub since: Option<Time>,
    /// Its recorded source turn, if known.
    pub turn_id: Option<Id>,
    /// Its recorded source item, if known.
    pub item_id: Option<Id>,
    /// Its native CPU use, if reported.
    pub cpu_percent: Option<f64>,
    /// Its native memory use, if reported.
    pub memory_bytes: Option<i64>,
}

/// A live roster lives only in memory and is never a durable sync row or job result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadBackground {
    /// Native listing and exact End are both supported.
    pub supported: bool,
    /// The observation's moment, when known.
    pub as_of: Option<Time>,
    /// Whether the observation is current and available.
    pub state: BackgroundState,
    /// A safe availability or partial-list explanation.
    pub reason: Option<String>,
    /// This page's bounded commands.
    pub items: Vec<BackgroundCommand>,
    /// The next page of this captured snapshot.
    pub next_cursor: Option<String>,
    /// The full count only when this snapshot is complete.
    pub total: Option<i64>,
    /// This caller's opaque review, where End is available.
    pub list_ref: Option<String>,
    /// Thirty seconds after this review's most recent active transfer.
    pub expires_at: Option<Time>,
    /// Current thread Interact and native support permit End.
    pub can_end: bool,
}

/// A reviewed selected set, or every member of one complete captured snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum BackgroundEndRequest {
    /// End at most two hundred explicitly reviewed commands.
    Selected(BackgroundEndSelected),
    /// End the complete reviewed count, without including later commands.
    All(BackgroundEndAll),
}

/// Explicit exact command selection from a review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BackgroundEndSelected {
    /// The caller's current captured review.
    pub list_ref: String,
    /// The opaque identities the caller reviewed.
    pub command_ids: Vec<String>,
}

/// Confirmation of a complete reviewed set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BackgroundEndAll {
    /// The caller's current captured review.
    pub list_ref: String,
    /// Must be true to confirm every reviewed member.
    pub all: bool,
    /// The exact complete reviewed count.
    pub count: i64,
}

/// The factual end of an exact command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundEndState {
    /// Native control confirmed the end.
    Ended,
    /// This exact command had already ended.
    AlreadyEnded,
    /// The command is known still to run.
    StillRunning,
    /// Its end could not be proved.
    Unknown,
}

/// A persistent mutation result contains only identities, outcomes and safe source links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BackgroundEndResultEntry {
    /// The requested opaque identity.
    pub id: String,
    /// Its factual end outcome.
    pub state: BackgroundEndState,
    /// A safe explanation, where needed.
    pub reason: Option<String>,
    /// The durable source item, when known.
    pub item_id: Option<Id>,
}

/// End outcomes for the accepted targets; clients read a fresh roster afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BackgroundEndResult {
    /// One factual outcome for each captured target.
    pub results: Vec<BackgroundEndResultEntry>,
}

/// Paging a captured current native list renews its idle review deadline.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadBackgroundGetQuery {
    /// The next page of the same snapshot, if one was returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Fifty by default, at most two hundred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

crate::endpoints! {
    /// Reads the current native commands of this thread's existing sessions.
    ThreadBackgroundGet: GET "/threads/{id}/background", Permission("thread.view"), query ThreadBackgroundGetQuery, none, json ThreadBackground, 200;
    /// Ends exactly the reviewed commands through an attributed job.
    ThreadBackgroundEnd: POST "/threads/{id}/background/end", Permission("thread.interact"), none, json BackgroundEndRequest, json crate::workspace::Job, 202;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_end_request_cannot_expand_a_selected_review_into_all_commands() {
        assert!(
            serde_json::from_value::<BackgroundEndRequest>(serde_json::json!({
                "list_ref": "review", "command_ids": ["one"], "all": true, "count": 2
            }))
            .is_err()
        );
        let all: BackgroundEndRequest = serde_json::from_value(serde_json::json!({
            "list_ref": "review", "all": true, "count": 250
        }))
        .unwrap();
        assert!(matches!(all, BackgroundEndRequest::All(_)));
    }
}
