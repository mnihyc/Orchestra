//! Shared passive compatibility observations in the existing marker and approval vocabulary.
//!
//! The program-specific actor parses its protocol, owns the request id and maps the answer.
//! These helpers never execute a tool, change settings or retain an unknown frame body in history.

use crate::events::{
    ApprovalOption, HarnessRequest, HarnessRequestDetail, HarnessRequestKind, ItemCompleted,
    ItemContent, ItemStarted, ItemStatus, MAIN_STREAM, MarkerPayload, OptionStyle, SessionEvent,
};
use orch_api_contract::threads::MarkerHarnessChanged;
use orch_api_contract::usage::UsageMeasurement;
use orch_types::{Id, Time};
use serde_json::json;

/// A request with no known offered answer can only be cancelled, never silently allowed.
pub const CANCEL_UNRECOGNIZED_APPROVAL: &str = "cancel_unrecognized_approval";

/// Count one unknown frame once. Sequence is the native session's monotonic observation id.
pub fn unrecognized_event(
    program: &str,
    name: &str,
    sequence: u64,
    turn: Option<Id>,
    at: Time,
) -> Result<Vec<SessionEvent>, serde_json::Error> {
    let native_ref = format!("unrecognized-event-{sequence}");
    let content = MarkerPayload::HarnessChanged(Box::new(MarkerHarnessChanged {
        program: program.to_owned(),
        change: "unrecognized_event".into(),
        subject: Some(name.to_owned()),
        previous: UsageMeasurement::Unsupported,
        value: json!({"count":1}),
        expected: UsageMeasurement::Unsupported,
        changed: false,
        diverged: false,
        next_turn: false,
        turn_id: turn,
        reported_at: at,
    }));
    let mut events = Vec::new();
    if let Some(turn_id) = turn {
        events.push(SessionEvent::ItemStarted(ItemStarted {
            turn_id,
            native_ref: native_ref.clone(),
            stream: MAIN_STREAM.into(),
            parent_native_ref: None,
            content: ItemContent::Marker(content.clone()),
        }));
    }
    events.push(SessionEvent::ItemCompleted(ItemCompleted {
        native_ref,
        status: ItemStatus::Completed,
        payload: serde_json::to_value(content)?,
        output_blob: None,
    }));
    Ok(events)
}

/// Keep unknown native approvals in the existing generic card with their real offered answers.
/// The actor passes a safe raw value and retains the native response/cancellation mapping.
pub fn generic_approval(
    summary: String,
    safe_raw: orch_types::Json,
    mut options: Vec<ApprovalOption>,
) -> HarnessRequest {
    if options.is_empty() {
        options.push(ApprovalOption {
            id: CANCEL_UNRECOGNIZED_APPROVAL.into(),
            label: "Cancel request".into(),
            style: OptionStyle::Deny,
            scope: None,
            rule: None,
        });
    }
    HarnessRequest {
        request_kind: HarnessRequestKind::Other,
        options,
        detail: HarnessRequestDetail::Other {
            summary,
            raw: safe_raw,
        },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "fixture assertions")]
    use super::*;

    #[test]
    fn unknown_frames_are_counted_markers_without_a_new_setting_or_invented_turn() {
        let at: Time = serde_json::from_value(json!("2026-10-08T14:00:00Z")).unwrap();
        for turn in [None, Some(Id::from_u128(1))] {
            let events =
                unrecognized_event("Fourth program", "future/update", 7, turn, at).unwrap();
            assert_eq!(events.len(), if turn.is_some() { 2 } else { 1 });
            let event = serde_json::to_value(events.last().unwrap()).unwrap();
            assert_eq!(event["kind"], "item.completed");
            assert_eq!(event["body"]["payload"]["marker"], "harness_changed");
            let data = &event["body"]["payload"]["data"];
            assert_eq!(data["program"], "Fourth program");
            assert_eq!(data["subject"], "future/update");
            assert_eq!(data["value"]["count"], 1);
            assert_eq!(data["diverged"], false);
            assert_eq!(data["turn_id"], json!(turn));
            let next = unrecognized_event("Fourth program", "future/update", 8, turn, at).unwrap();
            assert_ne!(events.last(), next.last());
        }
    }

    #[test]
    fn a_generic_approval_preserves_real_answers_or_exposes_only_native_cancellation() {
        let native = ApprovalOption {
            id: "vendor-once".into(),
            label: "Allow once".into(),
            style: OptionStyle::Allow,
            scope: None,
            rule: None,
        };
        let request = generic_approval(
            "Future permission".into(),
            json!({"kind":"future"}),
            vec![native.clone()],
        );
        assert_eq!(request.request_kind, HarnessRequestKind::Other);
        assert_eq!(request.options, vec![native]);
        let no_options = generic_approval("Future permission".into(), json!({}), vec![]);
        assert!(no_options.first_allowing().is_none());
        assert_eq!(no_options.options[0].id, CANCEL_UNRECOGNIZED_APPROVAL);
    }
}
