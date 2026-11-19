//! Native Auto review completions and the provider model policy that bounds their session.

use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::review::{self, ApprovalReviewer};

/// The reviewer the pinned build names, until a native review reports a model of its own.
pub const CODEX_REVIEW_MODEL: &str = "codex-auto-review";
use orch_agent_contract::session::SessionSettings;
use orch_types::Json;

/// The pinned build's reviewer until a native review reports a model of its own.
pub fn reviewer(settings: &SessionSettings) -> Result<Option<ApprovalReviewer>, AdapterError> {
    if settings.approval_mode != ApprovalMode::Auto {
        return Ok(None);
    }
    if !settings
        .available_models
        .iter()
        .any(|model| model == CODEX_REVIEW_MODEL)
    {
        return Err(AdapterError::Unsupported(format!(
            "Auto is unavailable: this person's provider policy does not allow {CODEX_REVIEW_MODEL}"
        )));
    }
    Ok(Some(review::declared(CODEX_REVIEW_MODEL.to_owned())))
}

/// A completed native review (the pinned app-server's item/autoApprovalReview/completed).
#[must_use]
pub fn completed(
    params: &Json,
    turn: Option<orch_types::Id>,
    at: orch_types::Time,
) -> Vec<orch_agent_contract::events::SessionEvent> {
    let Some(id) = params.get("reviewId").and_then(Json::as_str) else {
        return Vec::new();
    };
    let Some(body) = params.get("review") else {
        return Vec::new();
    };
    let Some(outcome) = body.get("status").and_then(Json::as_str) else {
        return Vec::new();
    };
    if outcome == "inProgress" {
        return Vec::new();
    }
    let model = body
        .get("model")
        .or_else(|| params.get("model"))
        .and_then(Json::as_str);
    let usage = body
        .get("usage")
        .or_else(|| params.get("usage"))
        .cloned()
        .and_then(|usage| serde_json::from_value(usage).ok());
    review::reported(review::ReportedReview {
        native_ref: format!("approval-review:{id}"),
        turn,
        at,
        model: model.unwrap_or(CODEX_REVIEW_MODEL),
        model_reported: model.is_some(),
        outcome,
        reason: body.get("rationale").and_then(Json::as_str),
        action: params.get("action").cloned().unwrap_or(Json::Null),
        usage,
        raw: params,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::events::SessionEvent;
    use serde_json::json;

    #[test]
    fn an_allowed_action_has_no_card_and_a_denial_shows_its_model_and_reason() {
        let at = orch_types::Time::new(chrono::Utc::now());
        for status in ["approved", "denied", "timedOut", "aborted"] {
            let events = completed(
                &json!({"reviewId":"review-1","threadId":"thread","review":{"status":status,"model":"codex-auto-review","rationale":"The request would send private files away"},"action":{"type":"command","command":"upload"}}),
                Some(orch_types::new_id()),
                at,
            );
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
            );
            assert!(events.iter().any(|event| matches!(event,SessionEvent::ItemCompleted(item) if item.payload.pointer("/tool/result/model")==Some(&json!("codex-auto-review")) && item.payload.pointer("/tool/result/outcome")==Some(&json!(status)) && item.payload.pointer("/tool/result/reason")==Some(&json!("The request would send private files away")))));
        }
    }

    #[test]
    fn no_model_report_uses_the_declared_default_without_claiming_a_report() {
        let events = completed(
            &json!({"reviewId":"review-2","review":{"status":"denied","rationale":"Policy denies the action"}}),
            None,
            orch_types::Time::new(chrono::Utc::now()),
        );
        assert!(events.iter().any(|event| matches!(event,SessionEvent::ItemCompleted(item) if item.payload.pointer("/tool/result/reported")==Some(&json!(false)))));
    }

    #[test]
    fn activated_auto_still_requires_the_fixed_reviewer_in_available_models()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut settings: SessionSettings = serde_json::from_value(json!({
            "agent_id":orch_types::new_id(),"thread_id":orch_types::new_id(),"segment_id":orch_types::new_id(),
            "mode":"new","native_session_ref":null,"upto_turn_ordinal":null,
            "workspace_root":"/workspace","instruction_root_marker":false,"directory":"/workspace","cwd":"/workspace",
            "mounts":[],"model":"mock-large","effort":null,"params":{},"approval_mode":"auto",
            "available_models":["mock-large"],"sandbox_mode":"workspace","instructions":"","memory":null,
            "arrangement":"together","orch_tools":[],"raw_events":false
        }))?;
        assert!(
            reviewer(&settings).is_err(),
            "the session model cannot stand in for the native reviewer"
        );
        settings
            .available_models
            .push(CODEX_REVIEW_MODEL.to_owned());
        let declared = reviewer(&settings)?.ok_or("Auto must declare its native reviewer")?;
        assert_eq!(declared.model, CODEX_REVIEW_MODEL);
        assert!(!declared.reported && !declared.usage_apart);
        settings
            .available_models
            .retain(|model| model != CODEX_REVIEW_MODEL);
        assert!(
            reviewer(&settings).is_err(),
            "removing native reviewer access closes Auto again"
        );
        Ok(())
    }

    #[test]
    fn verified_auto_uses_the_native_reviewer_and_preserves_remaining_person_requests() {
        assert_eq!(
            crate::parameters::approval_policy(ApprovalMode::Auto),
            "on-request"
        );
        assert_eq!(
            crate::parameters::approval_reviewer(ApprovalMode::Auto),
            "auto_review"
        );
        assert_eq!(
            crate::parameters::approval_policy(ApprovalMode::Bypass),
            "never"
        );
        assert!(
            !crate::parameters::definitions()
                .iter()
                .any(|parameter| parameter.key == "approvals_reviewer")
        );
        assert!(
            crate::descriptor::descriptor("0.165.0")
                .approval_modes
                .contains(&ApprovalMode::Auto)
        );
        assert!(crate::descriptor::auto_proven());
        assert!(orch_agent_programs::Programs::builtin().auto_possible());
        assert!(!orch_agent_programs::shipped("claude-code").proves("approval_modes.auto"));
        let declared = crate::descriptor::capabilities().approval_reviewer;
        assert!(
            declared
                .is_some_and(|reviewer| reviewer.model == CODEX_REVIEW_MODEL && !reviewer.settable)
        );
        let asked = crate::approvals::translate(
            "item/commandExecution/requestApproval",
            &json!({"threadId":"thread","itemId":"ask-rule","command":"deploy","cwd":"/workspace","reason":"An explicit ask rule matches"}),
            Vec::new(),
        );
        assert!(
            asked.is_some(),
            "a request the harness still raises remains a card"
        );
    }
}
