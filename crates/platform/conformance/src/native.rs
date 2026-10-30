//! Native harness cases shared by adapter tests and the node's isolated build runner.
//!
//! The cases are the same for every program. Which of them a program's build has to pass is its manifest's list
//! (`checks.conformance`), and what differs between programs (the approval mode a check's session is opened in, how
//! the program names an MCP tool, what a quiet start leaves behind) is what the adapter says through its hooks
//! (`native_check`, `mcp_tool_name`, `verify_quiet_start`). Nothing here names a program.

pub mod elicitation;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::Engine as _;
use futures::{Stream, StreamExt};
use orch_agent_contract::adapter::{
    AdapterEvent, AgentAdapter, AgentStart, AgentStarted, EventSink,
};
use orch_agent_contract::conformance::{ModelPaths, NativeCheck, ObservedRequest, QuietEvidence};
use orch_agent_contract::descriptor::{ApprovalMode, ArrangementType, SandboxMode};
use orch_agent_contract::events::{
    HarnessRequestDetail, HarnessRequestKind, SessionEvent, TurnOutcome,
};
use orch_agent_contract::history::ExportKind;
use orch_agent_contract::session::{
    ApprovalDecision, ExecAccess, ProviderAccess, SessionMode, SessionSettings, SessionSpec,
    TurnStart,
};
use orch_api_contract::admin::HarnessConformanceCase;
use orch_types::{Json, new_id};
use serde_json::json;

/// Model routing name understood by the scripted model on both native protocols.
pub const MODEL: &str = "mock-large";
/// A valid one-pixel PNG proves that a native image result reaches the model.
const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

/// One scripted service request, without credential headers.
#[derive(Debug, Clone)]
pub struct ModelRequest {
    /// Request path seen by the scripted model.
    pub path: String,
    /// Parsed request body.
    pub body: Json,
}

/// The paths of the scripted model that a quiet start of a program speaking `protocol` reaches and may reach,
/// where its adapter says nothing of its own.
fn protocol_paths(protocol: &str) -> (Vec<String>, Vec<String>) {
    let (reached, more): (&[&str], &[&str]) = match protocol {
        "openai-responses" => (&["/responses"], &["/models"]),
        "openai-chat" => (&["/chat/completions"], &["/models"]),
        "anthropic-messages" => (&["/v1/messages"], &["/v1/messages/count_tokens"]),
        _ => (&[], &[]),
    };
    let own = |paths: &[&str]| {
        paths
            .iter()
            .map(|path| (*path).to_owned())
            .collect::<Vec<_>>()
    };
    let mut allowed = own(reached);
    allowed.extend(own(more));
    (own(reached), allowed)
}

fn path_in(path: &str, paths: &ModelPaths, allowed: &[String]) -> bool {
    allowed.iter().any(|each| path.ends_with(each.as_str()))
        || paths.exact.iter().any(|each| path == each)
}

/// The line of a script that has the model call a tool by its role: the program's name for the tool, and the arguments
/// of the script with the names the program gives them. A program whose manifest names no tool for the role cannot be
/// asked for the case that needs it.
fn tool_line(fixture: &Fixture<'_>, role: &str, arguments: &Json) -> Result<String, String> {
    let tool =
        fixture.checks.tool_names.get(role).ok_or_else(|| {
            format!("the manifest of the program names no tool for the role {role}")
        })?;
    let (name, arguments) = tool.call(arguments);
    Ok(format!("tool {name} {arguments}"))
}

/// Disposable native context supplied by the node or the adapter's test launcher.
pub struct Fixture<'a> {
    /// Exact startup inputs; the binary is the build being checked.
    pub start: AgentStart,
    /// Exact expected build version; another executable cannot certify it.
    pub version: String,
    /// Fresh working directory belonging only to this case.
    pub workspace: PathBuf,
    /// Case deadline resolved by the caller.
    pub within: Duration,
    /// Requests observed at the same scripted model as orch-mock-llm.
    pub requests: &'a (dyn Fn() -> Vec<ModelRequest> + Send + Sync),
    /// What the manifest of the program under test lists: the cases a build has to pass, and the probes.
    pub checks: &'a orch_agent_programs::manifest::Checks,
    /// Launch environments observed by the caller, for telemetry checks.
    pub launches: &'a (dyn Fn() -> Vec<BTreeMap<String, String>> + Send + Sync),
    /// MCP replies observed by the controlled elicitation service, when checking it.
    pub elicitation: &'a (dyn Fn() -> Vec<Json> + Send + Sync),
    /// Optional controlled-frame evidence for an explicit native proof; ordinary checks keep it off.
    pub observe: Option<&'a (dyn Fn(&SessionEvent) + Send + Sync)>,
}

/// The cases of a program's check, with a descriptor feature only when failure is local to that feature.
#[must_use]
pub fn cases(checks: &orch_agent_programs::manifest::Checks) -> Vec<(&str, Option<&str>)> {
    checks.cases().collect()
}

/// Observations of native surfaces that Orch does not use; these never certify or disable a feature.
#[must_use]
pub fn probes(checks: &orch_agent_programs::manifest::Checks) -> &[String] {
    &checks.probes
}

/// Runs one shared native case, retaining absence of native evidence as a failure.
pub async fn run_case(
    adapter: &mut dyn AgentAdapter,
    fixture: &Fixture<'_>,
    name: &str,
) -> HarnessConformanceCase {
    observe(adapter, fixture, name, false).await
}

/// Runs an informational native probe with the same assertions, outside the certification catalogue.
pub async fn run_probe(
    adapter: &mut dyn AgentAdapter,
    fixture: &Fixture<'_>,
    name: &str,
) -> HarnessConformanceCase {
    observe(adapter, fixture, name, true).await
}

async fn observe(
    adapter: &mut dyn AgentAdapter,
    fixture: &Fixture<'_>,
    name: &str,
    informational: bool,
) -> HarnessConformanceCase {
    let began = Instant::now();
    let feature = cases(fixture.checks)
        .iter()
        .find(|(case, _)| *case == name)
        .and_then(|(_, feature)| *feature)
        .map(str::to_owned);
    let required = cases(fixture.checks).iter().any(|(case, _)| *case == name);
    let allowed = if informational {
        probes(fixture.checks).iter().any(|probe| probe == name)
    } else {
        required
    };
    let result = tokio::time::timeout(fixture.within, async {
        if !allowed {
            return Err("unknown native case or probe".to_owned());
        }
        exercise(adapter, fixture, name).await
    })
    .await
    .unwrap_or_else(|_| {
        Err(format!(
            "no native answer within {} s",
            fixture.within.as_secs()
        ))
    });
    // Cleanup shares the case's deadline; it cannot double the configured time after a failed observation.
    let remaining = fixture.within.saturating_sub(began.elapsed());
    let stopped = tokio::time::timeout(remaining, adapter.stop()).await;
    let (unsupported, error) = match result {
        Ok(unsupported) => (unsupported, None),
        Err(error) => (None, Some(error)),
    };
    let error = error.or_else(|| match stopped {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(format!("the harness could not stop: {error}")),
        Err(_) => Some("the harness did not stop before the case deadline".to_owned()),
    });
    HarnessConformanceCase {
        name: name.to_owned(),
        passed: error.is_none(),
        feature,
        unsupported: if error.is_none() { unsupported } else { None },
        error,
        elapsed_ms: i64::try_from(began.elapsed().as_millis()).unwrap_or(i64::MAX),
    }
}

fn session(fixture: &Fixture<'_>, check: &NativeCheck, protocol: &str) -> SessionSpec {
    let directory = fixture.workspace.display().to_string();
    SessionSpec {
        settings: SessionSettings {
            agent_id: fixture.start.agent_id,
            thread_id: new_id(),
            segment_id: new_id(),
            mode: SessionMode::New,
            native_session_ref: None,
            upto_turn_ordinal: None,
            workspace_root: directory.clone(),
            instruction_root_marker: false,
            directory: directory.clone(),
            cwd: directory,
            mounts: Vec::new(),
            model: MODEL.to_owned(),
            effort: None,
            params: json!({}),
            approval_mode: check.approval_mode,
            sandbox_mode: SandboxMode::Open,
            instructions: "You are working inside Orch.".to_owned(),
            available_models: vec![MODEL.to_owned()],
            subagent_limits: Default::default(),
            memory: None,
            arrangement: ArrangementType::KeySeparated,
            orch_tools: Vec::new(),
            orch_tool_schemas: Default::default(),
            raw_events: fixture.observe.is_some(),
            side_reading: None,
            workspace_configuration: None,
        },
        session_key: new_id().simple().to_string(),
        archive: None,
        history: None,
        provider: ProviderAccess::Forwarded {
            provider_id: fixture.start.provider_id,
            protocol: protocol.to_owned(),
        },
        exec: ExecAccess {
            executor: None,
            tool_set: None,
            environment_id: new_id(),
            generation: 1,
        },
        mcp: Vec::new(),
        skills: None,
    }
}

fn turn(spec: &SessionSpec, text: &str) -> TurnStart {
    TurnStart {
        plan: false,
        thread_id: spec.settings.thread_id,
        turn_id: new_id(),
        text: text.to_owned(),
        images: Vec::new(),
        model: MODEL.to_owned(),
        effort: None,
        params: json!({}),
        approval_mode: spec.settings.approval_mode,
        sandbox_mode: spec.settings.sandbox_mode,
    }
}

async fn until(
    events: &mut (impl Stream<Item = AdapterEvent> + Unpin),
    done: impl Fn(&SessionEvent) -> bool,
) -> Result<Vec<SessionEvent>, String> {
    let mut seen = Vec::new();
    while let Some(event) = events.next().await {
        match event {
            AdapterEvent::Session { event, .. } => {
                let end = done(&event);
                seen.push(event);
                if end {
                    return Ok(seen);
                }
            }
            AdapterEvent::Status(status)
                if status.status == orch_agent_contract::facts::HarnessState::Error =>
            {
                return Err(status
                    .detail
                    .unwrap_or_else(|| "the native harness failed".to_owned()));
            }
            _ => {}
        }
    }
    Err("the native event stream ended".to_owned())
}

async fn completed(
    adapter: &mut dyn AgentAdapter,
    events: &mut (impl Stream<Item = AdapterEvent> + Unpin),
    spec: &SessionSpec,
    script: &str,
) -> Result<Vec<SessionEvent>, String> {
    adapter
        .start_turn(turn(spec, script))
        .await
        .map_err(|error| error.to_string())?;
    let mut seen = Vec::new();
    loop {
        let next = until(events, |event| {
            matches!(
                event,
                SessionEvent::ApprovalRequested(_) | SessionEvent::TurnCompleted(_)
            )
        })
        .await?;
        let last = next.last().cloned();
        seen.extend(next);
        match last {
            Some(SessionEvent::ApprovalRequested(asked)) => {
                if spec.settings.approval_mode == ApprovalMode::Bypass {
                    return Err("a native bypass turn unexpectedly asked for permission".to_owned());
                }
                let option = asked
                    .request
                    .options
                    .iter()
                    .find(|option| {
                        option.id == "allow_once"
                            && option.style == orch_agent_contract::events::OptionStyle::Allow
                    })
                    .ok_or_else(|| {
                        "no one-use allow option in the native permission request".to_owned()
                    })?;
                adapter
                    .resolve_approval(
                        spec.settings.thread_id,
                        &asked.request_ref,
                        ApprovalDecision::Option {
                            option_id: option.id.clone(),
                            note: None,
                        },
                    )
                    .await
                    .map_err(|error| error.to_string())?;
            }
            Some(SessionEvent::TurnCompleted(end)) if end.status == TurnOutcome::Completed => {
                return Ok(seen);
            }
            _ => return Err("the native turn did not complete successfully".to_owned()),
        }
    }
}

/// A native archive must carry both turns to another home, without borrowing the source's store.
async fn native_import_observation(
    adapter: &mut dyn AgentAdapter,
    fixture: &Fixture<'_>,
    events: &mut (impl Stream<Item = AdapterEvent> + Unpin),
    source: &SessionSpec,
    check: &NativeCheck,
    protocol: &str,
) -> Result<(), String> {
    for words in ["native-import-source-one", "native-import-source-two"] {
        completed(adapter, events, source, &format!("#!mock\nsay {words}\n")).await?;
    }
    let archive = adapter
        .export_session(source.settings.thread_id, ExportKind::Native, None)
        .await
        .map_err(|error| format!("native export: {error}"))?;
    adapter
        .close_session(source.settings.thread_id)
        .await
        .map_err(|error| error.to_string())?;
    adapter.stop().await.map_err(|error| error.to_string())?;
    let home = fixture.workspace.join("native-import-home");
    std::fs::create_dir_all(&home).map_err(|error| error.to_string())?;
    let mut start = fixture.start.clone();
    start.home = Some(home.display().to_string());
    let (sink, imported_events) = EventSink::channel();
    let mut imported_events = imported_events.inspect(|event| {
        if let Some(observe) = fixture.observe
            && let AdapterEvent::Session { event, .. } = event
        {
            observe(event);
        }
    });
    let started = adapter
        .start(start, sink)
        .await
        .map_err(|error| error.to_string())?;
    if started.harness_version != fixture.version {
        return Err(format!(
            "expected {}, observed {}",
            fixture.version, started.harness_version
        ));
    }
    let mut imported = session(fixture, check, protocol);
    imported.settings.mode = SessionMode::ImportNative;
    imported.archive = Some(archive);
    adapter
        .open_session(imported.clone())
        .await
        .map_err(|error| format!("native import {}: {error}", error.code()))?;
    completed(
        adapter,
        &mut imported_events,
        &imported,
        "#!mock\nsay native-import-followup\n",
    )
    .await?;
    let body = (fixture.requests)()
        .last()
        .map(|request| request.body.to_string())
        .ok_or_else(|| "the imported session made no model request".to_owned())?;
    for words in [
        "native-import-source-one",
        "native-import-source-two",
        "native-import-followup",
    ] {
        if !body.contains(words) {
            return Err(format!("the native import lost {words}"));
        }
    }
    Ok(())
}

async fn exercise(
    adapter: &mut dyn AgentAdapter,
    fixture: &Fixture<'_>,
    name: &str,
) -> Result<Option<String>, String> {
    let (sink, events) = EventSink::channel();
    let mut events = events.inspect(|event| {
        if let Some(observe) = fixture.observe
            && let AdapterEvent::Session { event, .. } = event
        {
            observe(event);
        }
    });
    let started = adapter
        .start(fixture.start.clone(), sink)
        .await
        .map_err(|error| error.to_string())?;
    if started.harness_version != fixture.version {
        return Err(format!(
            "expected {}, observed {}",
            fixture.version, started.harness_version
        ));
    }
    started
        .descriptor
        .check()
        .map_err(|error| format!("invalid descriptor: {error:?}"))?;
    let check = adapter.native_check();
    let protocol = started
        .descriptor
        .forwarded_protocols
        .first()
        .cloned()
        .unwrap_or_default();
    let mut spec = session(fixture, &check, &protocol);
    if name == "sandbox" {
        spec.settings.sandbox_mode = SandboxMode::ReadOnly;
        if started
            .descriptor
            .sandbox_modes
            .contains(&SandboxMode::ReadOnly)
        {
            return Err("an unproved read-only boundary was offered".to_owned());
        }
        let error = adapter.open_session(spec).await.err().ok_or_else(|| {
            "a session requiring the unproved read-only boundary was accepted".to_owned()
        })?;
        if error.code() != "unsupported" || !error.to_string().to_lowercase().contains("read-only")
        {
            return Err(format!("wrong read-only refusal: {error}"));
        }
        if fixture.workspace.join("forbidden-marker").exists() {
            return Err("a refused session wrote its marker".to_owned());
        }
        return Ok(None);
    }
    if name == "command_approval" {
        spec.settings.approval_mode = ApprovalMode::Ask;
    }
    if matches!(name, "elicitation" | "tools.result_form") {
        spec.mcp = vec!["questions".to_owned()];
    }
    let opened = adapter
        .open_session(spec.clone())
        .await
        .map_err(|error| error.to_string())?;
    // A program that exports a file of its history is checked by exporting and importing it; one that keeps a
    // conversation where it is made is checked by taking it over in the home it is in, and in a copy of the home.
    let exports = started.descriptor.capabilities.export_native;
    match name {
        "native_import" if !exports => {
            native_takeover(
                adapter,
                fixture,
                &mut events,
                &spec,
                &opened.native_session_ref,
                &check,
                &protocol,
                false,
            )
            .await?
        }
        "history.native_import" if !exports => {
            native_takeover(
                adapter,
                fixture,
                &mut events,
                &spec,
                &opened.native_session_ref,
                &check,
                &protocol,
                true,
            )
            .await?;
            return Ok(None);
        }
        "native_import" => {
            native_import_observation(adapter, fixture, &mut events, &spec, &check, &protocol)
                .await?
        }
        "startup" => {
            completed(
                adapter,
                &mut events,
                &spec,
                "#!mock\nsay native conformance reached the model\n",
            )
            .await?;
            startup(&*adapter, fixture, &started, &check)?;
        }
        "history.native_import" => {
            return native_history_import(adapter, fixture, &mut events, &spec, &check, &protocol)
                .await;
        }
        "command_approval" => {
            let marker = fixture.workspace.join("must-not-exist");
            let script = format!(
                "#!mock\n{}\nthen\nsay denied\n",
                tool_line(
                    fixture,
                    "command",
                    &json!({"command":format!("touch {}", marker.display())})
                )?
            );
            adapter
                .start_turn(turn(&spec, &script))
                .await
                .map_err(|error| error.to_string())?;
            let seen = until(&mut events, |event| {
                matches!(
                    event,
                    SessionEvent::ApprovalRequested(_) | SessionEvent::TurnCompleted(_)
                )
            })
            .await?;
            let Some(SessionEvent::ApprovalRequested(asked)) = seen.last() else {
                return Err("the native command did not ask for approval".to_owned());
            };
            if asked.request.request_kind != HarnessRequestKind::Command || marker.exists() {
                return Err("a pending command had its effect or lost its approval kind".to_owned());
            }
            adapter
                .resolve_approval(
                    spec.settings.thread_id,
                    &asked.request_ref,
                    ApprovalDecision::Option {
                        option_id: "deny".to_owned(),
                        note: None,
                    },
                )
                .await
                .map_err(|error| error.to_string())?;
            let ended = until(&mut events, |event| {
                matches!(event, SessionEvent::TurnCompleted(_))
            })
            .await?;
            if marker.exists()
                || !matches!(ended.last(), Some(SessionEvent::TurnCompleted(end)) if end.status == TurnOutcome::Completed)
            {
                return Err("a declined command wrote its marker or did not complete".to_owned());
            }
        }
        "user_input" => {
            let script = format!(
                "#!mock\n{}\nthen\nsay question finished\n",
                tool_line(
                    fixture,
                    "ask_user",
                    &json!({"questions":[{
                        "id": "choice", "header": "Choice", "question": "Which value?",
                        "options": [
                            { "label": "First", "description": "First value" },
                            { "label": "Second", "description": "Second value" }
                        ]
                    }]})
                )?
            );
            adapter
                .start_turn(turn(&spec, &script))
                .await
                .map_err(|error| error.to_string())?;
            let seen = until(&mut events, |event| {
                matches!(
                    event,
                    SessionEvent::ApprovalRequested(_) | SessionEvent::TurnCompleted(_)
                )
            })
            .await?;
            let Some(SessionEvent::ApprovalRequested(asked)) = seen.last() else {
                return Err("the native default mode did not ask the person".to_owned());
            };
            let HarnessRequestDetail::UserInput { questions, .. } = &asked.request.detail else {
                return Err("the native user input lost its questions".to_owned());
            };
            if questions
                .first()
                .is_none_or(|question| question.id != "choice")
            {
                return Err("the native question lost its id".to_owned());
            }
            adapter
                .resolve_approval(
                    spec.settings.thread_id,
                    &asked.request_ref,
                    ApprovalDecision::Answers {
                        answers: BTreeMap::from([("choice".to_owned(), vec!["First".to_owned()])]),
                        note: None,
                    },
                )
                .await
                .map_err(|error| error.to_string())?;
            let ended = until(&mut events, |event| {
                matches!(event, SessionEvent::TurnCompleted(_))
            })
            .await?;
            if !matches!(ended.last(), Some(SessionEvent::TurnCompleted(end)) if end.status == TurnOutcome::Completed)
            {
                return Err(
                    "the native question's correlated answer did not complete its turn".to_owned(),
                );
            }
        }
        "dynamic_tools" => {
            adapter.start_turn(turn(&spec, "#!mock\ntool announce {\"text\":\"Conformance announcement\",\"kind\":\"progress\"}\nthen\nsay dynamic result {{result}}\n")).await.map_err(|error| error.to_string())?;
            let seen = until(&mut events, |event| {
                matches!(
                    event,
                    SessionEvent::OrchToolCall(_) | SessionEvent::TurnCompleted(_)
                )
            })
            .await?;
            let Some(SessionEvent::OrchToolCall(call)) = seen.last() else {
                return Err(
                    "the loaded native surface did not expose a correlated Orch tool call"
                        .to_owned(),
                );
            };
            if call.tool != "announce" {
                return Err("the native call lost its tool name".to_owned());
            }
            adapter
                .tool_result(
                    spec.settings.thread_id,
                    &call.call_ref,
                    Ok(json!({"accepted":true})),
                )
                .await
                .map_err(|error| error.to_string())?;
            let ended = until(&mut events, |event| {
                matches!(event, SessionEvent::TurnCompleted(_))
            })
            .await?;
            if !matches!(ended.last(), Some(SessionEvent::TurnCompleted(end)) if end.status == TurnOutcome::Completed)
            {
                return Err("the correlated tool result did not complete its turn".to_owned());
            }
        }
        "elicitation" => {
            let script = format!(
                "#!mock\ntool {} {{}}\nthen\nsay form result {{{{result}}}}\n",
                adapter.mcp_tool_name("questions", "ask")
            );
            adapter
                .start_turn(turn(&spec, &script))
                .await
                .map_err(|error| error.to_string())?;
            let seen = until(&mut events, |event| {
                matches!(
                    event,
                    SessionEvent::ApprovalRequested(_) | SessionEvent::TurnCompleted(_)
                )
            })
            .await?;
            let Some(SessionEvent::ApprovalRequested(asked)) = seen.last() else {
                return Err("the loaded native MCP client did not expose the form".to_owned());
            };
            let HarnessRequestDetail::McpElicitation { schema, .. } = &asked.request.detail else {
                return Err("the native form lost its schema".to_owned());
            };
            if asked.request.request_kind != HarnessRequestKind::McpElicitation
                || schema["mode"] != "form"
                || schema["requestedSchema"]["required"] != json!(["choice"])
            {
                return Err("the native form lost its required field or kind".to_owned());
            }
            adapter
                .resolve_approval(
                    spec.settings.thread_id,
                    &asked.request_ref,
                    ApprovalDecision::Elicitation {
                        action: orch_agent_contract::session::ElicitationAction::Accept,
                        content: Some(json!({"choice":"chosen"})),
                    },
                )
                .await
                .map_err(|error| error.to_string())?;
            let ended = until(&mut events, |event| {
                matches!(event, SessionEvent::TurnCompleted(_))
            })
            .await?;
            if !(fixture.elicitation)()
                .iter()
                .any(|reply| reply["result"]["content"]["choice"] == "chosen")
                || !matches!(ended.last(), Some(SessionEvent::TurnCompleted(end)) if end.status == TurnOutcome::Completed)
            {
                return Err(
                    "the native MCP client did not return the correlated form answer".to_owned(),
                );
            }
        }
        "nested_subagent" => {
            let script = format!(
                "#!mock\n{}\nthen\nsay parent completed\n",
                tool_line(
                    fixture,
                    "spawn_agent",
                    &json!({"message":"#!mock\nsay native child marker"})
                )?
            );
            let input = turn(&spec, &script);
            let root = input.turn_id;
            adapter
                .start_turn(input)
                .await
                .map_err(|error| error.to_string())?;
            let mut seen = Vec::new();
            let mut root_done = false;
            // The native spawn starts before its child id exists. Its final payload supplies the link;
            // a child's first event may precede that update or follow the parent's completion.
            loop {
                let next = until(&mut events, |_| true).await?;
                for event in &next {
                    if let SessionEvent::TurnCompleted(end) = event
                        && end.turn_id == root
                    {
                        if end.status != TurnOutcome::Completed {
                            return Err(
                                "the native parent turn did not complete successfully".to_owned()
                            );
                        }
                        root_done = true;
                    }
                    if matches!(event, SessionEvent::ApprovalRequested(_)) {
                        return Err(
                            "a native bypass turn unexpectedly asked for permission".to_owned()
                        );
                    }
                }
                seen.extend(next);
                if root_done && correlated_child(&seen) {
                    break;
                }
            }
        }
        "parallel_calls" => {
            for (name, text) in [
                ("first.txt", "the first result"),
                ("second.txt", "the second result"),
            ] {
                std::fs::write(fixture.workspace.join(name), text)
                    .map_err(|error| error.to_string())?;
            }
            let script = format!(
                "#!mock\n{}\n{}\nthen\nsay read both\n",
                tool_line(
                    fixture,
                    "read_file",
                    &json!({"path":fixture.workspace.join("first.txt")})
                )?,
                tool_line(
                    fixture,
                    "read_file",
                    &json!({"path":fixture.workspace.join("second.txt")})
                )?
            );
            let seen = completed(adapter, &mut events, &spec, &script).await?;
            let calls: Vec<_> = seen
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::ItemCompleted(done) if done.payload.get("tool").is_some() => {
                        Some(done)
                    }
                    _ => None,
                })
                .collect();
            if calls.len() != 2 || calls[0].native_ref == calls[1].native_ref {
                return Err("parallel native calls did not retain separate timeline ids".to_owned());
            }
            for (name, result) in [
                ("first.txt", "the first result"),
                ("second.txt", "the second result"),
            ] {
                if !calls.iter().any(|call| {
                    let body = call.payload.to_string();
                    body.contains(name) && body.contains(result)
                }) {
                    return Err(format!("{name} lost its own native result"));
                }
            }
        }
        "image_result" => {
            let path = fixture.workspace.join("pixel.png");
            std::fs::write(
                &path,
                base64::engine::general_purpose::STANDARD
                    .decode(PIXEL)
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            completed(
                adapter,
                &mut events,
                &spec,
                &format!(
                    "#!mock\n{}\nthen\nsay image seen\n",
                    tool_line(fixture, "read_file", &json!({"path":path}))?
                ),
            )
            .await?;
            if !(fixture.requests)().iter().any(|request| {
                let body = request.body.to_string();
                body.contains("\"type\":\"image\"") && body.contains("image/png")
            }) {
                return Err(
                    "the native image tool result did not reach the model as an image".to_owned(),
                );
            }
        }
        "tools.result_form" => {
            let script = format!(
                "#!mock\ntool {} {{}}\ntool {} {{}}\nthen\nsay result forms received\n",
                adapter.mcp_tool_name("questions", "read_shared"),
                adapter.mcp_tool_name("questions", "announce")
            );
            completed(adapter, &mut events, &spec, &script).await?;
            let requests = (fixture.requests)();
            let body = requests
                .last()
                .ok_or_else(|| "no model request followed the tool results".to_owned())?
                .body
                .to_string();
            let text_forms = body.matches("Orch conformance text").count();
            let structured_forms = body.matches("conformance_accepted").count();
            if text_forms != 1 || structured_forms != 1 {
                return Err(format!(
                    "native tools did not deliver one text or structured result each, without a duplicate (text: {text_forms}, structured: {structured_forms})"
                ));
            }
        }
        _ => return Err("unknown native case".to_owned()),
    }
    Ok(None)
}

fn correlated_child(events: &[SessionEvent]) -> bool {
    use orch_agent_contract::events::{ItemContent, ToolPayload};
    events.iter().any(|event| {
        let SessionEvent::ItemStarted(child) = event else {
            return false;
        };
        let Some(parent_ref) = &child.parent_native_ref else {
            return false;
        };
        if !child.stream.starts_with("sub:") {
            return false;
        }
        events.iter().any(|event| {
            let SessionEvent::ItemStarted(parent) = event else {
                return false;
            };
            if &parent.native_ref != parent_ref {
                return false;
            }
            let ItemContent::ToolCall(call) = &parent.content else {
                return false;
            };
            let ToolPayload::Subagent { child_stream, .. } = &call.tool else {
                return false;
            };
            child_stream == &child.stream
                || events.iter().any(|event| {
                    let payload = match event {
                        SessionEvent::ItemUpdated(update)
                            if update.native_ref == parent.native_ref =>
                        {
                            &update.payload
                        }
                        SessionEvent::ItemCompleted(done)
                            if done.native_ref == parent.native_ref =>
                        {
                            &done.payload
                        }
                        _ => return false,
                    };
                    payload["tool"]["kind"] == "subagent"
                        && payload["tool"]["child_stream"].as_str() == Some(child.stream.as_str())
                })
        })
    })
}

/// Copies a program home, file by file, for the case that moves a home with its environment. Sockets and other
/// special files are not copied: a home at rest holds none that matters.
fn copy_home(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_home(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// The words of the last request to the scripted model.
fn last_request(fixture: &Fixture<'_>) -> Result<String, String> {
    (fixture.requests)()
        .last()
        .map(|request| request.body.to_string())
        .ok_or_else(|| "the session made no model request".to_owned())
}

/// A conversation is taken over natively where the program keeps it: forked at a turn in the home it is made in
/// (the next request carries the turns up to there and no later one), and resumed in a copy of the home, as it is
/// when an environment moves with its volumes. With `recorded`, the fork is also read back from the program as
/// exactly the two turns it was forked through.
#[allow(
    clippy::too_many_arguments,
    reason = "one case reads everything the shared fixture holds"
)]
async fn native_takeover(
    adapter: &mut dyn AgentAdapter,
    fixture: &Fixture<'_>,
    events: &mut (impl Stream<Item = AdapterEvent> + Unpin),
    source: &SessionSpec,
    source_ref: &str,
    check: &NativeCheck,
    protocol: &str,
    recorded: bool,
) -> Result<(), String> {
    use orch_agent_contract::discovery::RecordedEntry;
    use orch_agent_contract::events::ItemContent;

    // Three distinct completed turns: a fork through the second must leave the third behind.
    let answers = [
        "native takeover first answer",
        "native takeover second answer",
        "native takeover third answer",
    ];
    for answer in answers {
        completed(adapter, events, source, &format!("#!mock\nsay {answer}\n")).await?;
    }
    adapter
        .close_session(source.settings.thread_id)
        .await
        .map_err(|error| error.to_string())?;

    // The same home: a fork through the second turn.
    let asked = (fixture.requests)().len();
    let mut fork = session(fixture, check, protocol);
    fork.settings.mode = SessionMode::Fork;
    fork.settings.native_session_ref = Some(source_ref.to_owned());
    fork.settings.upto_turn_ordinal = Some(2);
    let forked = adapter
        .open_session(fork.clone())
        .await
        .map_err(|error| format!("native fork {}: {error}", error.code()))?;
    if forked.native_session_ref.is_empty() || forked.native_session_ref == source_ref {
        return Err("the fork did not become a session of its own".to_owned());
    }
    if (fixture.requests)().len() != asked {
        return Err("opening the fork asked the model".to_owned());
    }
    if recorded {
        let record = adapter
            .read_session(&forked.native_session_ref, true)
            .await
            .map_err(|error| format!("the fork could not be read back: {error}"))?;
        if record.turns != 2 {
            return Err(format!(
                "the fork kept {} of the 2 turns asked for",
                record.turns
            ));
        }
        let file = record
            .file
            .ok_or_else(|| "the fork has no recorded entries".to_owned())?;
        let text = std::fs::read_to_string(file.path).map_err(|error| error.to_string())?;
        let mut found = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let entry: RecordedEntry =
                serde_json::from_str(line).map_err(|error| error.to_string())?;
            if let RecordedEntry::Item(item) = entry
                && item.stream == "main"
                && let ItemContent::AgentMessage(message) = item.content
            {
                found.push(message.text);
            }
        }
        if found != answers[..2] {
            return Err(format!("the fork changed the recorded answers: {found:?}"));
        }
    }
    completed(
        adapter,
        events,
        &fork,
        "#!mock\nsay native takeover fork followup\n",
    )
    .await?;
    let body = last_request(fixture)?;
    for answer in &answers[..2] {
        if !body.contains(answer) {
            return Err(format!("the fork lost {answer}"));
        }
    }
    if body.contains(answers[2]) || !body.contains("native takeover fork followup") {
        return Err("the fork carried a turn after the one it was forked through".to_owned());
    }
    adapter
        .close_session(fork.settings.thread_id)
        .await
        .map_err(|error| error.to_string())?;
    adapter.stop().await.map_err(|error| error.to_string())?;

    // A home that comes along: the same build reopens the conversation from a copy of the home.
    let source_home = fixture
        .start
        .home
        .as_deref()
        .map(std::path::Path::new)
        .ok_or_else(|| "native takeover needs its disposable source home".to_owned())?;
    if fixture.workspace.starts_with(source_home) {
        return Err("the native-takeover workspace must be outside its source home".to_owned());
    }
    let copy = fixture.workspace.join("native-takeover-home");
    copy_home(source_home, &copy).map_err(|error| error.to_string())?;
    let mut start = fixture.start.clone();
    start.home = Some(copy.to_string_lossy().into_owned());
    let (sink, moved_events) = EventSink::channel();
    let mut moved_events = moved_events.inspect(|event| {
        if let Some(observe) = fixture.observe
            && let AdapterEvent::Session { event, .. } = event
        {
            observe(event);
        }
    });
    let started = adapter
        .start(start, sink)
        .await
        .map_err(|error| error.to_string())?;
    if started.harness_version != fixture.version {
        return Err("the copied home started a different build".to_owned());
    }
    let mut moved = session(fixture, check, protocol);
    moved.settings.mode = SessionMode::Resume;
    moved.settings.native_session_ref = Some(source_ref.to_owned());
    adapter
        .open_session(moved.clone())
        .await
        .map_err(|error| format!("native resume in a copied home {}: {error}", error.code()))?;
    completed(
        adapter,
        &mut moved_events,
        &moved,
        "#!mock\nsay native takeover moved followup\n",
    )
    .await?;
    let body = last_request(fixture)?;
    for answer in answers {
        if !body.contains(answer) {
            return Err(format!("the copied home lost {answer}"));
        }
    }
    Ok(())
}

async fn native_history_import(
    adapter: &mut dyn AgentAdapter,
    fixture: &Fixture<'_>,
    events: &mut (impl Stream<Item = AdapterEvent> + Unpin),
    source: &SessionSpec,
    check: &NativeCheck,
    protocol: &str,
) -> Result<Option<String>, String> {
    use orch_agent_contract::discovery::RecordedEntry;
    use orch_agent_contract::events::ItemContent;
    use orch_agent_contract::history::ExportKind;

    // Two distinct completed turns prove that an imported file contains a conversation,
    // rather than merely an accepted header or a pointer back into the source's store.
    let answers = ["native import first answer", "native import second answer"];
    for answer in answers {
        completed(adapter, events, source, &format!("#!mock\nsay {answer}\n")).await?;
    }
    let archive = adapter
        .export_session(source.settings.thread_id, ExportKind::Native, None)
        .await
        .map_err(|error| format!("native export failed: {error}"))?;
    adapter.stop().await.map_err(|error| error.to_string())?;

    // Keep only the exported file at its new path. The old home must not remain
    // at the paths an archive could reference, even when the launcher's mounts persist.
    // Both homes belong to this disposable native case, never a person's configuration.
    let source_home = fixture
        .start
        .home
        .as_deref()
        .map(std::path::Path::new)
        .ok_or_else(|| "native import needs its disposable source home".to_owned())?;
    if fixture.workspace.starts_with(source_home) {
        return Err("the native-import workspace must be outside its source home".to_owned());
    }
    let mut archive = archive;
    let exported = fixture.workspace.join("native-import.archive");
    std::fs::copy(&archive.path, &exported).map_err(|error| error.to_string())?;
    archive.path = exported.to_string_lossy().into_owned();
    let retired = fixture.workspace.join("retired-native-import-source");
    std::fs::rename(source_home, retired).map_err(|error| error.to_string())?;
    // An empty placeholder keeps an existing bind mount valid without its source store.
    std::fs::create_dir(source_home).map_err(|error| error.to_string())?;
    let home = fixture.workspace.join("native-import-home");
    std::fs::create_dir(&home).map_err(|error| error.to_string())?;
    let mut start = fixture.start.clone();
    start.home = Some(home.to_string_lossy().into_owned());
    let (sink, _events) = EventSink::channel();
    let started = adapter
        .start(start, sink)
        .await
        .map_err(|error| error.to_string())?;
    if started.harness_version != fixture.version {
        return Err("native import started a different build".to_owned());
    }
    if !adapter
        .discover_sessions()
        .await
        .map_err(|error| format!("fresh-home discovery failed: {error}"))?
        .is_empty()
    {
        return Err("the native-import target home already contains a session".to_owned());
    }
    let requests = (fixture.requests)().len();
    let mut target = session(fixture, check, protocol);
    target.settings.mode = SessionMode::ImportNative;
    target.archive = Some(archive);
    let opened = match adapter.open_session(target).await {
        Ok(opened) => opened,
        Err(error) => {
            if error.code() == "unsupported" {
                let sessions = home.join("sessions");
                if sessions.exists()
                    && std::fs::read_dir(sessions)
                        .map_err(|error| error.to_string())?
                        .next()
                        .is_some()
                {
                    return Err("an unsupported native import wrote a session".to_owned());
                }
                if !adapter
                    .discover_sessions()
                    .await
                    .map_err(|error| format!("post-refusal discovery failed: {error}"))?
                    .is_empty()
                {
                    return Err("an unsupported native import registered a session".to_owned());
                }
                if (fixture.requests)().len() != requests {
                    return Err("an unsupported native import submitted a model request".to_owned());
                }
                return Ok(Some(format!("native import unsupported: {error}")));
            }
            return Err(format!("native import {}: {error}", error.code()));
        }
    };
    let record = adapter
        .read_session(&opened.native_session_ref, true)
        .await
        .map_err(|error| format!("the imported conversation could not be read: {error}"))?;
    if record.turns != answers.len() as i64 {
        return Err(format!(
            "native import kept {} of {} turns",
            record.turns,
            answers.len()
        ));
    }
    let file = record
        .file
        .ok_or_else(|| "the imported conversation has no recorded entries".to_owned())?;
    let text = std::fs::read_to_string(file.path).map_err(|error| error.to_string())?;
    let mut found = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let entry: RecordedEntry = serde_json::from_str(line).map_err(|error| error.to_string())?;
        if let RecordedEntry::Item(item) = entry
            && item.stream == "main"
            && let ItemContent::AgentMessage(message) = item.content
        {
            found.push(message.text);
        }
    }
    if found != answers {
        return Err(format!(
            "native import changed the recorded answers: {found:?}"
        ));
    }
    if (fixture.requests)().len() != requests {
        return Err("native import submitted an unexpected model request".to_owned());
    }
    Ok(None)
}

fn startup(
    adapter: &dyn AgentAdapter,
    fixture: &Fixture<'_>,
    started: &AgentStarted,
    check: &NativeCheck,
) -> Result<(), String> {
    let requests = (fixture.requests)();
    let (mut reached, mut allowed) = (
        check.model_paths.reached.clone(),
        check.model_paths.allowed.clone(),
    );
    if reached.is_empty()
        && let Some(protocol) = started.descriptor.forwarded_protocols.first()
    {
        (reached, allowed) = protocol_paths(protocol);
    }
    if allowed.is_empty() {
        allowed.clone_from(&reached);
    }
    if !reached.is_empty()
        && !requests.iter().any(|request| {
            reached
                .iter()
                .any(|path| request.path.ends_with(path.as_str()))
        })
    {
        return Err("the native binary did not reach the controlled model".to_owned());
    }
    if !allowed.is_empty()
        && requests
            .iter()
            .any(|request| !path_in(&request.path, &check.model_paths, &allowed))
    {
        return Err("startup contacted an unexpected controlled-service path".to_owned());
    }
    let seen: Vec<ObservedRequest> = requests
        .iter()
        .map(|request| ObservedRequest {
            path: request.path.clone(),
            body: request.body.clone(),
        })
        .collect();
    let launches = (fixture.launches)();
    let home = fixture.start.home.as_deref().map(PathBuf::from);
    adapter.verify_quiet_start(&QuietEvidence {
        requests: &seen,
        home: home.as_deref(),
        launches: &launches,
    })
}

#[cfg(test)]
mod correlation_tests {
    use super::*;
    use orch_agent_contract::events::{
        AgentMessagePayload, ItemCompleted, ItemContent, ItemStarted, ItemStatus, RawCall,
        SubagentState, ToolCallPayload, ToolPayload,
    };

    #[test]
    fn a_child_must_match_the_exact_parent_and_its_eventual_stream_in_either_order() {
        let call = ToolCallPayload {
            tool: ToolPayload::Subagent {
                child_stream: String::new(),
                name: None,
                role: None,
                prompt: None,
                state: SubagentState::Running,
                model: None,
            },
            title: "Spawn".to_owned(),
            started_at: orch_types::Clock::time(&orch_types::SystemClock),
            duration_ms: None,
            raw: RawCall {
                name: "spawn_agent".to_owned(),
                input: orch_agent_contract::events::OutputRef::of("{}"),
                output: None,
                is_error: false,
            },
        };
        let parent = SessionEvent::ItemStarted(ItemStarted {
            turn_id: new_id(),
            native_ref: "spawn".to_owned(),
            stream: "main".to_owned(),
            parent_native_ref: None,
            content: ItemContent::ToolCall(call),
        });
        let mut child = ItemStarted {
            turn_id: new_id(),
            native_ref: "child-message".to_owned(),
            stream: "sub:child".to_owned(),
            parent_native_ref: Some("spawn".to_owned()),
            content: ItemContent::AgentMessage(AgentMessagePayload {
                text: "marker".to_owned(),
                text_len: 6,
                phase: None,
            }),
        };
        let done = SessionEvent::ItemCompleted(ItemCompleted {
            native_ref: "spawn".to_owned(),
            status: ItemStatus::Completed,
            payload: json!({"tool":{"kind":"subagent","child_stream":"sub:child"}}),
            output_blob: None,
        });
        assert!(!correlated_child(&[
            parent.clone(),
            SessionEvent::ItemStarted(child.clone())
        ]));
        assert!(correlated_child(&[
            parent.clone(),
            SessionEvent::ItemStarted(child.clone()),
            done.clone()
        ]));
        assert!(correlated_child(&[
            parent.clone(),
            done.clone(),
            SessionEvent::ItemStarted(child.clone())
        ]));
        child.parent_native_ref = Some("another-spawn".to_owned());
        assert!(!correlated_child(&[
            parent.clone(),
            done.clone(),
            SessionEvent::ItemStarted(child.clone())
        ]));
        child.parent_native_ref = Some("spawn".to_owned());
        child.stream = "sub:another-child".to_owned();
        assert!(!correlated_child(&[
            parent,
            done,
            SessionEvent::ItemStarted(child)
        ]));
    }
}
