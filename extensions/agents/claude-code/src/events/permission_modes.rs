//! Report a native approval mode separately from the policy requested for this turn.
use super::*;
use orch_agent_contract::events::UsageMeasurement;

fn value(mode: &str) -> Json {
    json!({"native_mode":mode})
}

impl EventMapper {
    pub(crate) fn expect_permission_mode(&mut self, mode: &str) {
        self.requested_permission_mode = Some(mode.to_owned());
    }

    pub(crate) fn forget_permission_report(&mut self) {
        self.reported_permission_mode = None;
    }

    pub(crate) fn permission_mode_answer(&mut self, frame: &Frame) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        if frame.kind() == FrameKind::ControlResponse {
            self.observe_permission_mode(frame, &mut out);
        }
        out
    }

    pub(super) fn observe_permission_mode(&mut self, frame: &Frame, out: &mut Vec<SessionEvent>) {
        if self.native_session.as_deref().is_some_and(|session| {
            frame
                .text("session_id")
                .is_some_and(|actual| session != actual)
        }) {
            return;
        }
        let mode = if frame.kind() == FrameKind::ControlResponse {
            frame
                .get("response")
                .filter(|answer| answer["subtype"] == "success")
                .and_then(|answer| answer.get("response"))
                .and_then(|answer| answer.get("current_permission_mode"))
                .and_then(Json::as_str)
        } else {
            frame.text("permissionMode")
        };
        let Some(mode) = mode.filter(|mode| !mode.is_empty()) else {
            return;
        };
        if frame.get("parent_tool_use_id").is_none_or(Json::is_null) {
            self.reported_permission_mode = Some((frame.clone(), mode.to_owned()));
            self.report_permission_mode(out);
        } else {
            self.native_marker(frame, "approval_mode", None, value(mode), false, out);
        }
    }

    pub(super) fn report_permission_mode(&mut self, out: &mut Vec<SessionEvent>) {
        let Some((frame, actual)) = self.reported_permission_mode.clone() else {
            return;
        };
        let expected = self.requested_permission_mode.clone();
        let diverged = expected
            .as_ref()
            .is_some_and(|expected| expected != &actual);
        let emitted = self.native_observation(
            &frame,
            native_changes::Observation {
                change: "approval_mode",
                subject: None,
                value: value(&actual),
                explicit_change: false,
                turn: None,
                expected: expected
                    .as_deref()
                    .map(|mode| UsageMeasurement::Told { value: value(mode) })
                    .unwrap_or(UsageMeasurement::Unsupported),
            },
            out,
        );
        if !diverged {
            return;
        }
        let Some((reference, turn)) = emitted else {
            return;
        };
        let native_ref = format!("{reference}:notice");
        let error = ErrorPayload {
            details: Some(json!({"turn_id":turn,"change":"approval_mode","next_turn":false})),
            window: None,
            resets_at: None,
            code: orch_agent_contract::events::turn_error::HARNESS_DIVERGED.into(),
            message: format!(
                "Claude Code reported permission mode {actual} instead of {}.",
                expected.as_deref().unwrap_or_default()
            ),
            retryable: false,
        };
        if let Some(turn_id) = turn {
            out.push(SessionEvent::ItemStarted(ItemStarted {
                turn_id,
                native_ref: native_ref.clone(),
                stream: MAIN_STREAM.into(),
                parent_native_ref: None,
                content: ItemContent::Error(error.clone()),
            }));
        }
        if let Ok(payload) = serde_json::to_value(error) {
            out.push(SessionEvent::ItemCompleted(ItemCompleted {
                native_ref,
                status: ItemStatus::Completed,
                payload,
                output_blob: None,
            }));
        }
    }
}

#[cfg(test)]
mod tests;
