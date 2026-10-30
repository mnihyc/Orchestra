//! Read-only prompt versions and explicit, replay-bound execution selection.

use super::{ContentPart, ThreadInput};
use crate::nodes::JobRef;
use crate::users::UserRef;
use orch_types::{Id, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The history chosen for this read, independently of the selected execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadContinuationProjection {
    /// The immutable-prefix/owned-suffix projection returned by this read.
    pub viewed_continuation_id: Id,
    /// The one continuation whose future queue may execute.
    pub active_continuation_id: Id,
    /// Compare-and-set revision of execution selection.
    pub execution_revision: i64,
    /// Identity of this read projection; a different value requires a reset.
    pub projection_revision: i64,
    /// True when the reader omitted an explicit continuation selector.
    pub follows_active: bool,
}

/// A factual limit on editing this actual input as the current reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromptEditReason {
    /// Words belong to another author.
    NotAuthor,
    /// This is an imported, system or announcement input, not an authored main input.
    NotUserInput,
    /// An unsent input uses its existing pending-input edit action.
    PendingInput,
    /// This input has no supported exact native boundary, including mid-turn guidance.
    NativeBoundaryUnsupported,
    /// Its exact retained source cannot be prepared from its machine/native lane.
    NativeSourceUnavailable,
    /// Current interaction or use of the current provider/model/effort is missing.
    AccessRequired,
    /// Another accepted execution transition holds the thread.
    TransitionInProgress,
    /// The thread or an ancestor is archived.
    Archived,
}

/// Current-reader availability, batched with the visible prompt references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PromptEditAvailability {
    /// Whether current stored facts and authority allow requesting preparation.
    pub available: bool,
    /// The factual deciding reason, or null when available.
    pub reason: Option<PromptEditReason>,
    /// Safe detail of the deciding fact; preparation still rechecks native state.
    pub detail: Option<String>,
}

/// No delivered-input collection or per-message version request is needed to show the controls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadPromptSummary {
    /// The actual UserMessagePayload.input_id; never inferred from text or sequence.
    pub input_id: Id,
    /// The permanent original-input identity for these versions.
    pub group_id: Id,
    /// The continuation which owns this version's actual input.
    pub continuation_id: Id,
    /// Stable one-based version ordinal.
    pub ordinal: i64,
    /// Number of retained versions in the group.
    pub total: i64,
    /// The actual author; null never grants an editable human identity.
    pub author: Option<UserRef>,
    /// Availability under the current reader's authority and source facts.
    pub edit: PromptEditAvailability,
}

/// Native preparation is explicit; an accepted job is not a delivered turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadContinuationStatus {
    /// Its exact native source is being prepared.
    Preparing,
    /// The native result was verified and the continuation can be selected.
    Ready,
    /// Preparation failed; the former execution/history remain intact.
    Failed,
}

/// One version neighbor; full history and native work remain lazy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadPromptVersion {
    /// Permanent original-input identity; also present when this version is returned alone.
    pub group_id: Id,
    /// Retained version count from the same bounded metadata cut.
    pub total: i64,
    /// The version's actual input identity.
    pub id: Id,
    /// The actual input, also used to locate its history row.
    pub input_id: Id,
    /// Its actual delivered user-message item in this version's history; null before delivery.
    /// This is a positioned timeline anchor, never an ordinal or an execution request.
    #[serde(default)]
    pub item_id: Option<Id>,
    /// The version's owning continuation.
    pub continuation_id: Id,
    /// Permanent one-based ordinal within its prompt group.
    pub ordinal: i64,
    /// Its actual author, without borrowed owner authority.
    pub author: Option<UserRef>,
    /// Acceptance time of these words.
    pub created_at: Time,
    /// Whether its native lane was prepared successfully.
    pub status: ThreadContinuationStatus,
    /// Preparation/recovery progress, when there is an operation for this version.
    pub job: Option<JobRef>,
    /// The same current-reader decision as the bounded prompt summary.
    pub edit: PromptEditAvailability,
}

/// Adjacent/explicit browsing is read-only and uses stable indexed ordinals.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadPromptVersionListQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Source view for a prompt inherited by more than one retained continuation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    /// Scope of the source view; positioned neighbor reads retain this identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadPromptVersionsPage {
    /// The read-only source view, independent of execution selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<ThreadContinuationProjection>,
    pub group_id: Id,
    pub rows: Vec<ThreadPromptVersion>,
    pub total: i64,
    pub selected_version: i64,
    pub active_continuation_id: Id,
    pub next_before: Option<i64>,
    pub next_after: Option<i64>,
    pub seq: Seq,
}

/// A policy applies only to the exact running turn the person chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationRunningAction {
    Wait,
    Interrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContinuationActiveTurn {
    pub turn_id: Id,
    pub action: ContinuationRunningAction,
}

/// Creates revised words under the actual author's current interaction/provider rights.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadPromptVersionCreateBody {
    pub client_id: String,
    pub source_continuation_id: Id,
    pub content: Vec<ContentPart>,
    pub expected_active_continuation_id: Id,
    pub expected_execution_revision: i64,
    pub expected_queue_revision: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn: Option<ContinuationActiveTurn>,
}

/// Selects future execution; it never reruns the old last prompt or restores workspace files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadContinuationActivateBody {
    pub client_id: String,
    pub expected_active_continuation_id: Id,
    pub expected_execution_revision: i64,
    pub expected_queue_revision: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn: Option<ContinuationActiveTurn>,
    /// New ordinary content bound atomically to this exact target; omission authors nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ContentPart>>,
}

/// Replay returns these same durable identities, projected under current read authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadContinuationAccepted {
    pub operation_id: Id,
    pub continuation_id: Id,
    pub input_id: Option<Id>,
    pub job: JobRef,
    pub seq: Seq,
}

/// Lazy pending/failed queue pages; delivered history remains in the bounded timeline.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadInputListQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// A positioned queue page must match this exact ordering revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_revision: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadInputPage {
    pub projection: ThreadContinuationProjection,
    pub inputs: Vec<ThreadInput>,
    pub next_after: Option<Id>,
    pub queue_revision: i64,
    pub seq: Seq,
}

/// An explicit history selector is read-only, including for a direct known-item link.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadReadSelector {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const THREAD: &str = "01950000-0000-7000-8000-000000000001";
    const INPUT: &str = "01950000-0000-7000-8000-000000000002";

    #[test]
    fn browsing_a_version_carries_its_read_scope_and_never_an_execution_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let written =
            json!({"before":3,"limit":1,"continuation_id":THREAD,"projection_revision":7});
        let query: ThreadPromptVersionListQuery = serde_json::from_value(written.clone())?;
        assert_eq!(serde_json::to_value(query)?, written);
        let mut forbidden = written;
        forbidden["activate"] = json!(true);
        assert!(serde_json::from_value::<ThreadPromptVersionListQuery>(forbidden).is_err());
        assert_eq!(
            serde_json::to_value(ThreadReadSelector::default())?,
            json!({})
        );
        Ok(())
    }

    #[test]
    fn editing_names_the_exact_running_turn_and_preserves_all_compare_and_set_values()
    -> Result<(), Box<dyn std::error::Error>> {
        for action in ["wait", "interrupt"] {
            let written = json!({"client_id":"synthetic-edit-1","source_continuation_id":THREAD,
                "content":[{"type":"text","text":"Revised words"}],"expected_active_continuation_id":THREAD,
                "expected_execution_revision":7,"expected_queue_revision":11,
                "active_turn":{"turn_id":INPUT,"action":action}});
            let request: ThreadPromptVersionCreateBody = serde_json::from_value(written.clone())?;
            assert_eq!(serde_json::to_value(request)?, written);
        }
        let written = json!({"client_id":"synthetic-activation-1","expected_active_continuation_id":THREAD,
            "expected_execution_revision":7,"expected_queue_revision":11});
        let request: ThreadContinuationActivateBody = serde_json::from_value(written.clone())?;
        assert_eq!(request.content, None);
        assert_eq!(request.active_turn, None);
        assert_eq!(serde_json::to_value(request)?, written);
        Ok(())
    }

    #[test]
    fn a_prompt_summary_joins_the_actual_input_without_loading_delivered_words()
    -> Result<(), Box<dyn std::error::Error>> {
        let written = json!({"input_id":INPUT,"group_id":INPUT,"continuation_id":THREAD,"ordinal":1,"total":2,
            "author":null,"edit":{"available":false,"reason":"not_author","detail":"These words have another author."}});
        let summary: ThreadPromptSummary = serde_json::from_value(written.clone())?;
        assert_eq!(serde_json::to_value(summary)?, written);
        Ok(())
    }

    #[test]
    fn a_version_delivers_only_an_actual_item_anchor_and_old_missing_members_read_as_null()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut written = json!({"group_id":INPUT,"total":2,"id":INPUT,"input_id":INPUT,"continuation_id":THREAD,
            "ordinal":1,"author":null,"created_at":"2026-10-17T00:00:00.000Z","status":"ready","job":null,
            "edit":{"available":false,"reason":"not_author","detail":"These words have another author."}});
        let old: ThreadPromptVersion = serde_json::from_value(written.clone())?;
        assert_eq!(old.item_id, None);
        written["item_id"] = serde_json::Value::Null;
        assert_eq!(serde_json::to_value(old)?, written);
        written["item_id"] = json!(THREAD);
        let delivered: ThreadPromptVersion = serde_json::from_value(written.clone())?;
        assert_eq!(delivered.item_id, Some(THREAD.parse()?));
        assert_eq!(serde_json::to_value(delivered)?, written);
        written["item_id"] = json!(17);
        assert!(serde_json::from_value::<ThreadPromptVersion>(written).is_err());
        Ok(())
    }
}
