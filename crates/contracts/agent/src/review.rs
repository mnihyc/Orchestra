//! Native permission reviews, kept as factual thread items rather than permission requests.

use crate::events::{
    ItemCompleted, ItemContent, ItemStarted, ItemStatus, MAIN_STREAM, OutputRef, RawCall,
    SessionEvent, TokenUsage, ToolCallPayload, ToolPayload,
};
use orch_types::{Json, Time};
use serde_json::json;

pub use orch_api_contract::settings::ApprovalReviewer;

/// The reserved name for a native review item.
pub const REVIEW_TOOL: &str = "harness/approvalReview";

/// The reviewer declared before any native report arrives.
#[must_use]
pub fn declared(model: String) -> ApprovalReviewer {
    ApprovalReviewer {
        model,
        reported: false,
        usage_apart: false,
    }
}

/// One complete native review, kept together so its model, outcome and raw evidence cannot be mixed.
#[derive(Debug)]
pub struct ReportedReview<'a> {
    /// Stable native item reference.
    pub native_ref: String,
    /// The turn the harness associated with it.
    pub turn: Option<crate::session::TurnId>,
    /// When it was reported.
    pub at: Time,
    /// Reported model, or the declared fallback.
    pub model: &'a str,
    /// Whether the harness supplied that model.
    pub model_reported: bool,
    /// The native outcome.
    pub outcome: &'a str,
    /// The harness's reason, when given.
    pub reason: Option<&'a str>,
    /// What was reviewed.
    pub action: Json,
    /// Usage reported apart from the main turn.
    pub usage: Option<TokenUsage>,
    /// The native evidence.
    pub raw: &'a Json,
}

/// A complete review the harness reported. An action with no report creates no item.
#[must_use]
pub fn reported(review: ReportedReview<'_>) -> Vec<SessionEvent> {
    let ReportedReview {
        native_ref,
        turn,
        at,
        model,
        model_reported,
        outcome,
        reason,
        action,
        usage,
        raw,
    } = review;
    let detail = json!({"model":model,"reported":model_reported,"outcome":outcome,"reason":reason,"action":action,"usage":usage});
    let content = ToolCallPayload {
        tool: ToolPayload::Other {
            name: REVIEW_TOOL.to_owned(),
            arguments: None,
            result: Some(detail),
        },
        title: format!("Permission review: {outcome}"),
        started_at: at,
        duration_ms: None,
        raw: RawCall {
            name: REVIEW_TOOL.to_owned(),
            input: OutputRef::of(&action.to_string()),
            output: Some(OutputRef::of(&raw.to_string())),
            is_error: matches!(
                outcome,
                "denied" | "failed" | "timedOut" | "timed_out" | "aborted"
            ),
        },
    };
    let mut events = Vec::new();
    if let Some(turn_id) = turn {
        events.push(SessionEvent::ItemStarted(ItemStarted {
            native_ref: native_ref.clone(),
            turn_id,
            stream: MAIN_STREAM.to_owned(),
            parent_native_ref: None,
            content: ItemContent::ToolCall(content.clone()),
        }));
    }
    events.push(SessionEvent::ItemCompleted(ItemCompleted {
        native_ref,
        status: ItemStatus::Completed,
        payload: serde_json::to_value(content).unwrap_or(Json::Null),
        output_blob: None,
    }));
    events
}
