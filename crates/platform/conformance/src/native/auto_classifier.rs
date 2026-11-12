//! Staged-classifier evidence uses the existing native fixture and adapter-owned wire validation.
use super::*;
use orch_agent_contract::conformance::{
    NativeClassifierCase, NativeClassifierEvidence, NativeClassifierOutcome,
    NativeClassifierReview, NativeRequestPattern, NativeReviewRequest,
};

pub(super) async fn exercise(
    adapter: &mut dyn AgentAdapter,
    fixture: &Fixture<'_>,
    started: &AgentStarted,
    check: &NativeCheck,
    protocol: &str,
    recipe: &NativeClassifierReview,
) -> Result<(), String> {
    if !started
        .descriptor
        .approval_modes
        .contains(&ApprovalMode::Auto)
    {
        return Err("Auto is unavailable; classifier observations require explicit candidate admission until real proof".into());
    }
    let first = recipe
        .cases
        .first()
        .ok_or("classifier recipe has no observed cases")?;
    let mut excluded = session(fixture, check, protocol);
    excluded.settings.model = first.session_model.clone();
    excluded.settings.approval_mode = ApprovalMode::Auto;
    excluded.settings.available_models.clear();
    let before = (fixture.requests)().len();
    let error = adapter
        .open_session(excluded)
        .await
        .err()
        .ok_or("excluded classifier session model was admitted")?;
    if error.code() != "unsupported"
        || !error
            .to_string()
            .contains(&recipe.excluded_refusal_fragment)
        || (fixture.requests)().len() != before
    {
        return Err("excluded classifier model did not fail before model traffic".into());
    }
    for case in &recipe.cases {
        // The same opaque command must actually ask under On request before an effect can prove
        // the classifier route. Each phase has a fresh session, process and event stream.
        phase(adapter, fixture, check, protocol, recipe, case, true).await?;
        phase(adapter, fixture, check, protocol, recipe, case, false).await?;
    }
    Ok(())
}

async fn phase(
    adapter: &mut dyn AgentAdapter,
    fixture: &Fixture<'_>,
    check: &NativeCheck,
    protocol: &str,
    recipe: &NativeClassifierReview,
    case: &NativeClassifierCase,
    control: bool,
) -> Result<(), String> {
    let model_control = fixture
        .control_model
        .ok_or("classifier fixture has no model queue control")?;
    adapter.stop().await.map_err(|error| error.to_string())?;
    model_control(ModelControl::Reset)?;
    let (sink, events) = EventSink::channel();
    let mut events = events.inspect(|event| {
        if let Some(observe) = fixture.observe
            && let AdapterEvent::Session { event, .. } = event
        {
            observe(event);
        }
    });
    let root = fixture.workspace.join(format!(
        "classifier-{}-{}",
        case.name,
        if control { "control" } else { "auto" }
    ));
    let work = root.join("work");
    std::fs::create_dir(&root).map_err(|error| error.to_string())?;
    std::fs::create_dir(&work).map_err(|error| error.to_string())?;
    let home = root.join("home");
    std::fs::create_dir(&home).map_err(|error| error.to_string())?;
    let mut start = fixture.start.clone();
    start.home = Some(home.display().to_string());
    start.profile.env.extend(case.profile_env.clone());
    let observed = adapter
        .start(start, sink)
        .await
        .map_err(|error| error.to_string())?;
    if observed.harness_version != fixture.version {
        return Err("classifier phase changed the pinned executable".into());
    }
    let marker = root.join("marker");
    if recipe.command.matches("__ORCH_MARKER__").count() != 1 {
        return Err("classifier command must name exactly one owned marker".into());
    }
    let quoted_marker = format!("'{}'", marker.display().to_string().replace('\'', "'\\''"));
    let command = recipe.command.replace("__ORCH_MARKER__", &quoted_marker);
    let mut spec = session(fixture, check, protocol);
    spec.settings.model = case.session_model.clone();
    spec.settings.available_models = case.available_models.clone();
    spec.settings.approval_mode = if control {
        ApprovalMode::OnRequest
    } else {
        ApprovalMode::Auto
    };
    spec.settings.sandbox_mode = recipe.sandbox_mode;
    spec.settings.directory = work.display().to_string();
    spec.settings.cwd = work.display().to_string();
    spec.settings.workspace_root = work.display().to_string();
    spec.settings.raw_events = true;
    let opened = adapter
        .open_session(spec.clone())
        .await
        .map_err(|error| error.to_string())?;
    if !control
        && !opened
            .approval_reviewer
            .as_ref()
            .is_some_and(|reviewer| reviewer.model == case.declared_reviewer)
    {
        return Err(
            "native classifier session declaration differs from observed process routing".into(),
        );
    }
    let mut arguments = recipe
        .command_arguments
        .as_object()
        .cloned()
        .ok_or("classifier command arguments are not an object")?;
    arguments.insert("command".into(), json!(command));
    model_control(ModelControl::QueueMatching {
        model: Some(case.session_model.clone()),
        script: tool_line(fixture, "command", &Json::Object(arguments))?,
        pattern: case.initial_request.clone(),
        times: 1,
    })?;
    if !control {
        for stage in &case.stages {
            model_control(ModelControl::QueueMatching {
                model: Some(stage.model.clone()),
                script: stage.script.clone(),
                pattern: stage.request.clone(),
                times: 1,
            })?;
        }
        // End the main turn cleanly after its tool result. This response can never answer a classifier
        // request: the two roles have different structural matchers even when their model is identical.
        model_control(ModelControl::QueueMatching {
            model: Some(case.session_model.clone()),
            script: "say native classifier case finished".into(),
            pattern: case.continuation_request.clone(),
            times: 1,
        })?;
    }
    // Unknown protocols get an explicit error rather than guessed XML. Persistence is bounded by
    // this existing case deadline; post-run validation rejects unexpected or repeated traffic.
    model_control(ModelControl::QueueMatching {
        model: None,
        script: "error permission status=403 unsupported-native-classifier-request".into(),
        pattern: NativeRequestPattern::default(),
        times: u64::MAX,
    })?;
    let mut input = turn(
        &spec,
        "Use the requested opaque command once to create only the disposable sibling marker. Request native permission if needed; do not change it or retry.",
    );
    input.model = case.session_model.clone();
    let turn_id = input.turn_id;
    adapter
        .start_turn(input)
        .await
        .map_err(|error| error.to_string())?;
    let mut seen = until(&mut events, |event| {
        matches!(
            event,
            SessionEvent::ApprovalRequested(_) | SessionEvent::TurnCompleted(_)
        )
    })
    .await?;
    if control {
        if !matches!(seen.last(),Some(SessionEvent::ApprovalRequested(asked)) if asked.turn_id==turn_id)
            || marker.exists()
        {
            return Err(
                "opaque command did not remain pending without effect in the On request control"
                    .into(),
            );
        }
        // Closing an unanswered permission request can otherwise let its final continuation
        // race the next phase's model reset. Finish this native turn before changing phases.
        adapter
            .interrupt(spec.settings.thread_id, turn_id)
            .await
            .map_err(|error| error.to_string())?;
        let ended = until(&mut events, |event| {
            matches!(event, SessionEvent::TurnCompleted(_))
        })
        .await?;
        if !matches!(ended.last(), Some(SessionEvent::TurnCompleted(end))
            if end.turn_id == turn_id && end.status == TurnOutcome::Interrupted)
            || marker.exists()
        {
            return Err("the unanswered control did not stop without its marker effect".into());
        }
        seen.extend(ended);
    } else if !matches!(seen.last(),Some(SessionEvent::TurnCompleted(end)) if end.turn_id==turn_id && end.status==TurnOutcome::Completed)
    {
        return Err("classifier case did not complete; no person request was answered".into());
    }
    let requests = (fixture.requests)();
    let views: Vec<_> = requests
        .iter()
        .map(|request| NativeReviewRequest {
            path: &request.path,
            body: &request.body,
            status: request.status,
            queued: request.queued,
        })
        .collect();
    adapter.verify_native_classifier(&NativeClassifierEvidence {
        case,
        command: &command,
        control,
        turn_id,
        native_session_ref: &opened.native_session_ref,
        reviewer: opened.approval_reviewer.as_ref(),
        events: &seen,
        requests: &views,
    })?;
    if !control {
        let allowed = case.outcome == NativeClassifierOutcome::Allow;
        if marker.exists() != allowed
            || (allowed
                && std::fs::read_to_string(&marker).ok().as_deref()
                    != Some(recipe.marker_contents.as_str()))
        {
            return Err("classifier decision and exact disposable marker effect disagree".into());
        }
    }
    adapter
        .close_session(spec.settings.thread_id)
        .await
        .map_err(|error| error.to_string())?;
    adapter.stop().await.map_err(|error| error.to_string())?;
    Ok(())
}
