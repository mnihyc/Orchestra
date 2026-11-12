//! Native reviewer protocol, effects and exact model admission on the existing controlled model.
//! The adapter supplies observed wire facts. A result here never substitutes for the real-plan trial.

use super::*;
use orch_agent_contract::conformance::{
    NativeAutoReview, NativeNotificationReview, NativeRequestPattern,
};
use orch_agent_contract::credentials::CredentialPath;
use orch_agent_contract::review::REVIEW_TOOL;

const MARKER_TEXT: &str = "orch-auto-native";

pub(super) async fn exercise(
    adapter: &mut dyn AgentAdapter,
    fixture: &Fixture<'_>,
    events: &mut (impl Stream<Item = AdapterEvent> + Unpin),
    started: &AgentStarted,
    check: &NativeCheck,
    protocol: &str,
) -> Result<(), String> {
    match check
        .auto_review
        .as_ref()
        .ok_or("the adapter declares no observed native reviewer recipe")?
    {
        NativeAutoReview::Notifications(recipe) => {
            exercise_notifications(adapter, fixture, events, started, check, protocol, recipe).await
        }
        NativeAutoReview::Classifier(recipe) => {
            super::auto_classifier::exercise(adapter, fixture, started, check, protocol, recipe)
                .await
        }
    }
}

async fn exercise_notifications(
    adapter: &mut dyn AgentAdapter,
    fixture: &Fixture<'_>,
    events: &mut (impl Stream<Item = AdapterEvent> + Unpin),
    started: &AgentStarted,
    check: &NativeCheck,
    protocol: &str,
    recipe: &NativeNotificationReview,
) -> Result<(), String> {
    let control = fixture
        .control_model
        .ok_or("the native fixture cannot control its scripted model")?;
    let declared = started
        .descriptor
        .capabilities
        .approval_reviewer
        .as_ref()
        .ok_or("the adapter declares no reviewer model")?;
    let review_model = recipe.model.as_deref().unwrap_or(MODEL);
    let valid_selection = match &recipe.model {
        Some(model) => declared.model == *model,
        None => declared
            .session_model_paths
            .contains(&CredentialPath::Forwarded),
    };
    if declared.settable || !valid_selection {
        return Err("this case requires its declared native reviewer selection".into());
    }
    if !started
        .descriptor
        .approval_modes
        .contains(&ApprovalMode::Auto)
    {
        return Err(
            "Auto is unavailable for this build; the native reviewer case cannot run".into(),
        );
    }

    // This must reach the existing model policy check, not merely the closed feature gate.
    let mut excluded = session(fixture, check, protocol);
    excluded.settings.approval_mode = ApprovalMode::Auto;
    excluded
        .settings
        .available_models
        .retain(|model| model != review_model);
    if recipe.model.is_none() {
        excluded
            .settings
            .available_models
            .push(declared.model.clone());
    }
    let before = (fixture.requests)().len();
    let error = adapter
        .open_session(excluded)
        .await
        .err()
        .ok_or("a session admitted its excluded native reviewer model")?;
    if error.code() != "unsupported"
        || !error.to_string().contains(review_model)
        || (fixture.requests)().len() != before
    {
        return Err(format!(
            "excluded reviewer did not fail before a model request: {error}"
        ));
    }

    for decision in ["allow", "deny", "unavailable"] {
        control(ModelControl::Reset)?;
        let root = fixture.workspace.join(format!("auto-review-{decision}"));
        let work = root.join("work");
        std::fs::create_dir(&root).map_err(|error| error.to_string())?;
        std::fs::create_dir(&work).map_err(|error| error.to_string())?;
        let marker = root.join("marker");
        let mut spec = session(fixture, check, protocol);
        spec.settings.approval_mode = ApprovalMode::Auto;
        spec.settings.sandbox_mode = SandboxMode::Workspace;
        spec.settings.directory = work.display().to_string();
        spec.settings.cwd = work.display().to_string();
        spec.settings.workspace_root = work.display().to_string();
        if !spec
            .settings
            .available_models
            .iter()
            .any(|model| model == review_model)
        {
            spec.settings.available_models.push(review_model.to_owned());
        }
        // Exact native review correlation is evidence even when the caller does not print raw frames.
        spec.settings.raw_events = true;
        let opened = adapter
            .open_session(spec.clone())
            .await
            .map_err(|error| error.to_string())?;
        if !opened
            .approval_reviewer
            .as_ref()
            .is_some_and(|reviewer| reviewer.model == review_model)
        {
            return Err("the opened session lost its admitted reviewer identity".into());
        }
        let mut arguments = recipe
            .command_arguments
            .as_object()
            .cloned()
            .ok_or("native review command arguments are not an object")?;
        arguments.insert(
            "command".to_owned(),
            json!(format!("printf {MARKER_TEXT} > ../marker")),
        );
        control(ModelControl::Queue {
            model: MODEL.to_owned(),
            script: tool_line(fixture, "command", &Json::Object(arguments))?,
        })?;
        if decision != "unavailable" {
            let reply = if decision == "allow" {
                &recipe.allow_response
            } else {
                &recipe.deny_response
            };
            control(ModelControl::QueueMatching {
                model: Some(review_model.to_owned()),
                script: format!("say {reply}"),
                times: 1,
                pattern: NativeRequestPattern {
                    equals: std::collections::BTreeMap::from([(
                        recipe.schema_pointer.clone(),
                        recipe.response_schema.clone(),
                    )]),
                    ..NativeRequestPattern::default()
                },
            })?;
        }
        control(ModelControl::Default {
            script:
                "error permission Native Auto conformance deliberately refuses the reviewer model"
                    .to_owned(),
        })?;
        let input = turn(
            &spec,
            "Run the one requested scratch-marker command. Request native approval if needed. Do not change it, use another tool, or retry.",
        );
        let turn_id = input.turn_id;
        adapter
            .start_turn(input)
            .await
            .map_err(|error| error.to_string())?;
        // Do not use completed(): its ordinary-case helper can answer a permission request.
        let seen = until(events, |event| {
            matches!(
                event,
                SessionEvent::ApprovalRequested(_) | SessionEvent::TurnCompleted(_)
            )
        })
        .await?;
        if seen
            .iter()
            .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
        {
            return Err(
                "native Auto left a person request; the conformance case did not answer it".into(),
            );
        }
        if !matches!(seen.last(), Some(SessionEvent::TurnCompleted(end))
            if end.turn_id == turn_id && end.status == TurnOutcome::Completed)
        {
            return Err("the native Auto turn did not complete".into());
        }
        validate(recipe, decision, turn_id, &seen, &(fixture.requests)())?;
        if marker.exists() != (decision == "allow") {
            return Err(format!("native {decision} had the wrong marker effect"));
        }
        if decision == "allow"
            && std::fs::read_to_string(&marker).ok().as_deref() != Some(MARKER_TEXT)
        {
            return Err("the approved native command wrote different marker content".into());
        }
        adapter
            .close_session(spec.settings.thread_id)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn validate(
    recipe: &NativeNotificationReview,
    decision: &str,
    turn_id: orch_types::Id,
    seen: &[SessionEvent],
    requests: &[ModelRequest],
) -> Result<(), String> {
    let review_model = recipe.model.as_deref().unwrap_or(MODEL);
    let raw: Vec<_> = seen
        .iter()
        .filter_map(|event| match event {
            SessionEvent::RawFrame(frame) if frame.direction == "in" => Some(&frame.payload),
            _ => None,
        })
        .collect();
    let starts: Vec<_> = raw
        .iter()
        .filter(|frame| {
            frame.get("method").and_then(Json::as_str) == Some(recipe.started_method.as_str())
        })
        .collect();
    let ends: Vec<_> = raw
        .iter()
        .filter(|frame| {
            frame.get("method").and_then(Json::as_str) == Some(recipe.completed_method.as_str())
        })
        .collect();
    if starts.len() != 1 || ends.len() != 1 {
        return Err("expected exactly one native reviewer start and completion".into());
    }
    let started = &starts[0]["params"];
    let ended = &ends[0]["params"];
    for key in ["reviewId", "threadId", "turnId", "targetItemId"] {
        if started[key].is_null() || started[key] != ended[key] {
            return Err(format!(
                "native review correlation changed or omitted {key}"
            ));
        }
    }
    let expected = if decision == "allow" {
        "approved"
    } else {
        "denied"
    };
    if ended["review"]["status"] != expected {
        return Err(format!("native {decision} reported another review outcome"));
    }
    if decision == "unavailable"
        && !ended["review"]["rationale"]
            .as_str()
            .is_some_and(|text| text.contains("403"))
    {
        return Err("unavailable reviewer omitted the actual 403 failure rationale".into());
    }
    let reviewed: Vec<_> = requests
        .iter()
        .filter(|request| {
            request.body.pointer(&recipe.schema_pointer) == Some(&recipe.response_schema)
        })
        .collect();
    if reviewed.is_empty() || (decision != "unavailable" && reviewed.len() != 1) {
        return Err("the expected native reviewer requests were not observed".into());
    }
    let status = if decision == "unavailable" { 403 } else { 200 };
    if reviewed.iter().any(|request| {
        request.body["model"] != review_model
            || request.status != Some(status)
            || request.body.pointer(&recipe.schema_pointer) != Some(&recipe.response_schema)
    }) {
        return Err(
            "reviewer HTTP result or requested output schema differs from the pinned observation"
                .into(),
        );
    }
    if requests.iter().any(|request| {
        request.body["model"]
            .as_str()
            .is_some_and(|model| model != MODEL && model != review_model)
    }) {
        return Err("native review requested an additional unallowed model".into());
    }
    let items: Vec<_> = seen
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(item)
                if item.payload.pointer("/tool/name") == Some(&json!(REVIEW_TOOL)) =>
            {
                Some(item)
            }
            _ => None,
        })
        .collect();
    if items.len() != 1 {
        return Err("native review did not become exactly one completed timeline item".into());
    }
    let item = items[0];
    if item.status != orch_agent_contract::events::ItemStatus::Completed {
        return Err("native review did not finish as a factual completed item".into());
    }
    if !seen.iter().any(|event| {
        matches!(event, SessionEvent::ItemStarted(start)
        if start.native_ref == item.native_ref && start.turn_id == turn_id)
    }) {
        return Err("native review timeline item lost its turn correlation".into());
    }
    let result = item
        .payload
        .pointer("/tool/result")
        .ok_or("native review item lost its result")?;
    if result["model"] != review_model
        || result["outcome"] != expected
        || result["reason"] != ended["review"]["rationale"]
        || result["action"] != ended["action"]
    {
        return Err(
            "native review projection changed its model, outcome, rationale or action".into(),
        );
    }
    let native_model = ended["review"]
        .get("model")
        .or_else(|| ended.get("model"))
        .and_then(Json::as_str);
    if result["reported"] != json!(native_model.is_some()) {
        return Err("review projection invented native completion model provenance".into());
    }
    let native_usage = match ended["review"].get("usage").or_else(|| ended.get("usage")) {
        None | Some(Json::Null) => Json::Null,
        Some(value) => {
            let usage: orch_agent_contract::events::TokenUsage =
                serde_json::from_value(value.clone()).map_err(|error| {
                    format!("native reviewer usage is not the declared shape: {error}")
                })?;
            serde_json::to_value(usage).map_err(|error| error.to_string())?
        }
    };
    if result["usage"] != native_usage {
        return Err("review projection changed or invented separate native usage".into());
    }
    Ok(())
}
