//! Native Auto review completions and the provider model policy that bounds their session.

use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::credentials::CredentialPath;
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::review::{self, ApprovalReviewer};

/// The sign-in provider's preferred reviewer; forwarded catalogues select the parent model.
pub const CODEX_REVIEW_MODEL: &str = "codex-auto-review";
use orch_agent_contract::session::SessionSettings;
use orch_types::Json;

/// Admit the preferred reviewer of a native sign-in.
pub fn reviewer(settings: &SessionSettings) -> Result<Option<ApprovalReviewer>, AdapterError> {
    reviewer_for(settings, CredentialPath::Native)
}

/// Admit the actual reviewer declared for this session's access and model.
pub fn reviewer_for(
    settings: &SessionSettings,
    path: CredentialPath,
) -> Result<Option<ApprovalReviewer>, AdapterError> {
    if settings.approval_mode != ApprovalMode::Auto {
        return Ok(None);
    }
    let support = crate::descriptor::capabilities()
        .approval_reviewer
        .ok_or_else(|| {
            AdapterError::Unsupported("Auto is unavailable: no native reviewer was declared".into())
        })?;
    let model = support.model_for(path, &settings.model);
    if !settings
        .available_models
        .iter()
        .any(|allowed| allowed == model)
    {
        return Err(AdapterError::Unsupported(format!(
            "Auto is unavailable: this person's provider policy does not allow {model}"
        )));
    }
    Ok(Some(review::declared(model.to_owned())))
}

/// A completed native review (the pinned app-server's item/autoApprovalReview/completed).
#[must_use]
pub fn completed(
    params: &Json,
    turn: Option<orch_types::Id>,
    at: orch_types::Time,
) -> Vec<orch_agent_contract::events::SessionEvent> {
    observed(params, turn, at, false, CODEX_REVIEW_MODEL)
}

/// A native review start opens the same factual card its verdict will complete.
#[must_use]
pub fn started(
    params: &Json,
    turn: Option<orch_types::Id>,
    at: orch_types::Time,
) -> Vec<orch_agent_contract::events::SessionEvent> {
    observed(params, turn, at, true, CODEX_REVIEW_MODEL)
}

pub(crate) fn with_model(
    params: &Json,
    turn: Option<orch_types::Id>,
    at: orch_types::Time,
    starting: bool,
    model: &str,
) -> Vec<orch_agent_contract::events::SessionEvent> {
    observed(params, turn, at, starting, model)
}

fn observed(
    params: &Json,
    turn: Option<orch_types::Id>,
    at: orch_types::Time,
    starting: bool,
    fallback_model: &str,
) -> Vec<orch_agent_contract::events::SessionEvent> {
    let Some(id) = params
        .get("reviewId")
        .and_then(Json::as_str)
        .filter(|id| !id.is_empty())
    else {
        return Vec::new();
    };
    let Some(body) = params.get("review") else {
        return Vec::new();
    };
    let Some(outcome) = body.get("status").and_then(Json::as_str) else {
        return Vec::new();
    };
    if starting != (outcome == "inProgress") {
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
    let report = review::ReportedReview {
        native_ref: format!("approval-review:{id}"),
        turn,
        at,
        model: model.unwrap_or(fallback_model),
        model_reported: model.is_some(),
        outcome,
        reason: body.get("rationale").and_then(Json::as_str),
        action: params.get("action").cloned().unwrap_or(Json::Null),
        usage,
        raw: params,
    };
    if starting {
        review::started(report)
    } else {
        review::reported(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::events::SessionEvent;
    use serde_json::json;

    #[test]
    fn every_native_review_verdict_is_a_factual_card_not_a_person_request() {
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
            "web_action_wait_secs":120,
            "arrangement":"together","orch_tools":[],"raw_events":false
        }))?;
        assert!(
            reviewer_for(&settings, CredentialPath::Native).is_err(),
            "the session model cannot stand in for the native reviewer"
        );
        let forwarded = reviewer_for(&settings, CredentialPath::Forwarded)?
            .ok_or("forwarded Auto must declare its actual parent reviewer")?;
        assert_eq!(forwarded.model, "mock-large");
        settings
            .available_models
            .push(CODEX_REVIEW_MODEL.to_owned());
        let declared = reviewer_for(&settings, CredentialPath::Native)?
            .ok_or("Auto must declare its native reviewer")?;
        assert_eq!(declared.model, CODEX_REVIEW_MODEL);
        assert!(!declared.reported && !declared.usage_apart);
        settings
            .available_models
            .retain(|model| model != "mock-large");
        assert!(
            reviewer_for(&settings, CredentialPath::Forwarded).is_err(),
            "permission for the old preferred name does not admit the actual parent reviewer"
        );
        settings.available_models.push("mock-large".into());
        settings
            .available_models
            .retain(|model| model != CODEX_REVIEW_MODEL);
        assert!(
            reviewer_for(&settings, CredentialPath::Native).is_err(),
            "removing native reviewer access closes Auto again"
        );
        Ok(())
    }

    #[test]
    fn an_unreported_review_keeps_its_selected_parent_model_without_inventing_native_evidence() {
        let events = with_model(
            &json!({"reviewId":"parent-review","review":{"status":"approved","rationale":"Allowed"}}),
            None,
            orch_types::Time::new(chrono::Utc::now()),
            false,
            "admitted-parent",
        );
        assert!(events.iter().any(|event| matches!(event,
            SessionEvent::ItemCompleted(item)
            if item.payload["tool"]["result"]["model"] == "admitted-parent"
                && item.payload["tool"]["result"]["reported"] == false
                && item.payload["tool"]["result"]["usage"].is_null())));
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
            crate::descriptor::descriptor("0.167.2")
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
