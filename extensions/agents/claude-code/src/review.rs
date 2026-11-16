//! Auto's reviewer policy, native permission-denial hook and refusal of a Manual fallback.

use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::review::{self, ApprovalReviewer};

/// The documented classifier default, when this person may use it.
pub const CLAUDE_REVIEW_MODEL: &str = "claude-sonnet-5";
use orch_agent_contract::session::SessionSettings;
use orch_types::Json;
use serde_json::json;

/// The default process maps the native Sonnet alias to the selected model. This is a declaration,
/// not a claim that the native review itself reported a model or separately accounted usage.
pub fn reviewer(settings: &SessionSettings) -> Result<Option<ApprovalReviewer>, AdapterError> {
    let environment = crate::parameters::model_environment(&settings.model, None)
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    reviewer_in_environment(settings, &environment)
}

/// Resolve the preferred native alias against the exact process environment, after profile overrides.
/// Preserve profile values and the native fallback within the existing allowed-model list.
pub(crate) fn reviewer_in_environment(
    settings: &SessionSettings,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<Option<ApprovalReviewer>, AdapterError> {
    if settings.approval_mode != ApprovalMode::Auto {
        return Ok(None);
    }
    if !settings.available_models.contains(&settings.model) {
        return Err(AdapterError::Unsupported(
            "Auto needs the session's model in this person's availableModels".to_owned(),
        ));
    }
    // The pinned process's native Sonnet alias is the selected model by default, but a profile
    // may intentionally override it. Native availableModels excludes an unavailable preferred
    // alias and falls back to the selected model; keep the existing process configuration intact.
    // Only the exact canonical client override established by the pinned native trial is
    // declared here. Native eligibility is not the same as supportsAutoMode or membership in
    // availableModels: the native client rejects some otherwise allowed model tiers for this
    // input. Do not duplicate that vendor policy or assume arbitrary aliases are eligible.
    // Server-configured classifier choices can still outrank this client declaration; a native
    // reported model, if supplied, remains authoritative in denial_for_reviewer.
    let observed_override = environment
        .get("CLAUDE_CODE_AUTO_MODE_MODEL")
        .filter(|model| model.as_str() == CLAUDE_REVIEW_MODEL)
        .filter(|model| settings.available_models.contains(model));
    let preferred = observed_override
        .or_else(|| environment.get("ANTHROPIC_DEFAULT_SONNET_MODEL"))
        .filter(|model| !model.trim().is_empty())
        .map_or(settings.model.as_str(), String::as_str);
    let model = if settings
        .available_models
        .iter()
        .any(|allowed| allowed == preferred)
    {
        preferred
    } else {
        settings.model.as_str()
    };
    Ok(Some(review::declared(model.to_owned())))
}

/// Read-only hooks. Their replies contain no decision or additional context.
#[must_use]
pub fn hooks() -> Json {
    json!({"PermissionDenied":[{"hookCallbackIds":["orch-permission-review"]}],"InstructionsLoaded":[{"hookCallbackIds":["orch-instructions-loaded"]}]})
}

/// The harness must confirm Auto rather than silently starting in Manual.
pub fn confirmed(mode: Option<&str>, reason: Option<&str>) -> Result<(), AdapterError> {
    if mode.is_some_and(|mode| mode.eq_ignore_ascii_case("auto")) {
        return Ok(());
    }
    Err(AdapterError::Unsupported(format!(
        "Auto was requested but Claude Code reported {}: {}",
        mode.unwrap_or("no permission mode"),
        reason.unwrap_or(
            "its reviewer is unavailable for this model, provider or organisation policy"
        )
    )))
}

/// PermissionDenied fires only for Auto's denials, never a person's denial or a matching deny rule.
#[must_use]
pub fn denial(
    input: &Json,
    settings: &SessionSettings,
    turn: Option<orch_types::Id>,
    at: orch_types::Time,
) -> Vec<orch_agent_contract::events::SessionEvent> {
    let reviewer = reviewer(settings).ok().flatten();
    denial_for_reviewer(input, reviewer.as_ref(), turn, at)
}

/// Project a fact using the declaration captured for the actual running process. A later desired
/// model or allowlist must not relabel the process that made this native decision.
pub(crate) fn denial_for_reviewer(
    input: &Json,
    reviewer: Option<&ApprovalReviewer>,
    turn: Option<orch_types::Id>,
    at: orch_types::Time,
) -> Vec<orch_agent_contract::events::SessionEvent> {
    if input.get("hook_event_name").and_then(Json::as_str) != Some("PermissionDenied") {
        return Vec::new();
    }
    let Some(reviewer) = reviewer else {
        return Vec::new();
    };
    let reported_model = input.get("model").and_then(Json::as_str);
    let reason = input.get("reason").and_then(Json::as_str);
    let outcome = if reason.is_some_and(|reason| {
        reason == "Classifier unavailable" || reason.starts_with("Auto mode could not evaluate")
    }) {
        "failed"
    } else {
        "denied"
    };
    review::reported(review::ReportedReview {
        native_ref: format!(
            "approval-review:{}",
            input
                .get("tool_use_id")
                .and_then(Json::as_str)
                .unwrap_or("unknown")
        ),
        turn,
        at,
        model: reported_model.unwrap_or(&reviewer.model),
        model_reported: reported_model.is_some(),
        outcome,
        reason,
        action: json!({"tool":input.get("tool_name"),"input":input.get("tool_input")}),
        usage: None,
        raw: input,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    use orch_agent_contract::events::SessionEvent;

    fn settings() -> SessionSettings {
        serde_json::from_value(json!({"agent_id":orch_types::new_id(),"thread_id":orch_types::new_id(),"segment_id":orch_types::new_id(),"mode":"new","native_session_ref":null,"upto_turn_ordinal":null,"workspace_root":"/workspace","instruction_root_marker":false,"directory":"/workspace","cwd":"/workspace","mounts":[],"model":"claude-opus-5","effort":null,"params":{},"approval_mode":"auto","available_models":["claude-opus-5"],"sandbox_mode":"workspace","instructions":"","memory":null,"arrangement":"together","orch_tools":[],"output_capture_bytes":67108864, "web_action_wait_secs":120,"raw_events":false})).unwrap()
    }

    #[test]
    fn permitting_another_model_does_not_change_the_existing_native_model_pin() {
        let mut settings = settings();
        assert_eq!(reviewer(&settings).unwrap().unwrap().model, "claude-opus-5");
        settings
            .available_models
            .push(CLAUDE_REVIEW_MODEL.to_owned());
        assert_eq!(reviewer(&settings).unwrap().unwrap().model, "claude-opus-5");
        settings.available_models.clear();
        assert!(reviewer(&settings).is_err());
    }

    #[test]
    fn denied_action_shows_reason_and_model_without_a_permission_card() {
        let mut settings = settings();
        settings
            .available_models
            .push(CLAUDE_REVIEW_MODEL.to_owned());
        let at = orch_types::Time::new(chrono::Utc::now());
        let events = denial(
            &json!({"hook_event_name":"PermissionDenied","permission_mode":"auto","tool_name":"Bash","tool_input":{"command":"rm -rf /workspace"},"tool_use_id":"denial-1","reason":"[Irreversible Local Destruction]"}),
            &settings,
            Some(orch_types::new_id()),
            at,
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
        );
        assert!(events.iter().any(|event| matches!(event,SessionEvent::ItemCompleted(item) if item.payload.pointer("/tool/result/model")==Some(&json!("claude-opus-5")) && item.payload.pointer("/tool/result/reason")==Some(&json!("[Irreversible Local Destruction]")))));
        assert!(
            events
                .iter()
                .any(|event| matches!(event,SessionEvent::ItemCompleted(item)
            if item.payload.pointer("/tool/result/reported") == Some(&json!(false))
                && item.payload.pointer("/tool/result/usage") == Some(&Json::Null)))
        );
        // Ordinary actions and a person's denial are not invented reviewer reports.
        assert!(
            denial(
                &json!({"hook_event_name":"PostToolUse"}),
                &settings,
                None,
                at
            )
            .is_empty()
        );
    }

    #[test]
    fn manual_fallback_is_refused_with_the_harness_mode_and_reason() {
        let error = confirmed(Some("Manual"), Some("disabled by the organisation")).unwrap_err();
        assert!(error.to_string().contains("Manual"));
        assert!(error.to_string().contains("disabled by the organisation"));
        assert!(confirmed(Some("auto"), None).is_ok());
        assert!(confirmed(None, None).is_err());
    }

    #[test]
    fn auto_is_not_declared_before_the_real_trial_and_ask_rules_are_still_cards() {
        let descriptor = crate::descriptor::descriptor("2.1.296", false);
        assert!(!descriptor.approval_modes.contains(&ApprovalMode::Auto));
        assert_eq!(
            crate::approvals::permission_mode(ApprovalMode::Auto),
            "auto"
        );
        assert_eq!(
            crate::approvals::permission_mode(ApprovalMode::Bypass),
            "bypassPermissions"
        );
        let frame = crate::stream::Frame::parse(&json!({"type":"control_request","request_id":"ask-rule","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"deploy"}}}).to_string()).unwrap();
        assert!(crate::approvals::PermissionAsk::from_frame(&frame).is_some());
    }
    #[test]
    fn a_running_process_review_fact_keeps_its_declaration_after_desired_settings_change() {
        let mut settings = settings();
        settings
            .available_models
            .push(CLAUDE_REVIEW_MODEL.to_owned());
        let actual = reviewer(&settings).unwrap().unwrap();
        settings.model = CLAUDE_REVIEW_MODEL.to_owned();
        settings.available_models = vec![settings.model.clone()];
        assert_eq!(
            reviewer(&settings).unwrap().unwrap().model,
            CLAUDE_REVIEW_MODEL
        );
        let events = denial_for_reviewer(
            &json!({"hook_event_name":"PermissionDenied","tool_name":"Bash","tool_input":{"command":"node marker"},"tool_use_id":"running-call","reason":"Classifier unavailable"}),
            Some(&actual),
            None,
            orch_types::Time::new(chrono::Utc::now()),
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event,SessionEvent::ItemCompleted(item)
            if item.payload.pointer("/tool/result/model") == Some(&json!("claude-opus-5"))
                && item.payload.pointer("/tool/result/reported") == Some(&json!(false))
                && item.payload.pointer("/tool/result/usage") == Some(&Json::Null)))
        );
    }
    #[test]
    fn observed_classifier_override_is_declared_only_when_the_exact_model_is_allowed() {
        let mut settings = settings();
        settings
            .available_models
            .push(CLAUDE_REVIEW_MODEL.to_owned());
        let env = std::collections::BTreeMap::from([
            (
                "ANTHROPIC_DEFAULT_SONNET_MODEL".to_owned(),
                settings.model.clone(),
            ),
            (
                "CLAUDE_CODE_AUTO_MODE_MODEL".to_owned(),
                CLAUDE_REVIEW_MODEL.to_owned(),
            ),
        ]);
        let declared = reviewer_in_environment(&settings, &env).unwrap().unwrap();
        assert_eq!(declared.model, CLAUDE_REVIEW_MODEL);
        assert!(!declared.reported && !declared.usage_apart);
        settings
            .available_models
            .retain(|model| model != CLAUDE_REVIEW_MODEL);
        assert_eq!(
            reviewer_in_environment(&settings, &env)
                .unwrap()
                .unwrap()
                .model,
            settings.model
        );
        settings.available_models.clear();
        assert!(reviewer_in_environment(&settings, &env).is_err());
    }

    #[test]
    fn allowed_main_model_capability_does_not_certify_an_env_classifier_override() {
        let mut settings = settings();
        let opus = settings.model.clone();
        settings.model = CLAUDE_REVIEW_MODEL.to_owned();
        settings.available_models = vec![CLAUDE_REVIEW_MODEL.to_owned(), opus.clone()];
        let env = std::collections::BTreeMap::from([
            (
                "ANTHROPIC_DEFAULT_SONNET_MODEL".to_owned(),
                CLAUDE_REVIEW_MODEL.to_owned(),
            ),
            ("CLAUDE_CODE_AUTO_MODE_MODEL".to_owned(), opus),
        ]);
        // The pinned native env validator rejects Opus5 here, although initialize advertises
        // supportsAutoMode=true for it. An allowed-model check alone must not relabel this fact.
        let declared = reviewer_in_environment(&settings, &env).unwrap().unwrap();
        assert_eq!(declared.model, CLAUDE_REVIEW_MODEL);
        assert!(!declared.reported && !declared.usage_apart);
    }
}
