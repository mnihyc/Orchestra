//! Registered protocol controls use the actual adapter and the one shared byte replayer.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
mod common;
#[path = "common/upgrade_corpus.rs"]
mod corpus;

use async_trait::async_trait;
use futures::StreamExt;
use orch_adapter_codex::{
    CodexAdapter,
    process::{HarnessProcess, LaunchError, LaunchRequest, Launcher, ProcessControl, ProcessExit},
};
use orch_agent_contract::adapter::{
    AdapterEvent, AgentAdapter, AgentStart, EventSink, EventStream,
};
use orch_agent_contract::descriptor::{ApprovalMode, SandboxMode};
use orch_agent_contract::facts::{AgentProfile, HarnessState};
use orch_agent_contract::loopback::Loopback;
use orch_agent_contract::observations::CANCEL_UNRECOGNIZED_APPROVAL;
use orch_agent_contract::session::{
    ApprovalDecision, ElicitationAction, SessionMode, TurnStart, TurnSteer,
};
use orch_plugin_contract::Secret;
use orch_testkit::harness_corpus::{
    Bindings, Evidence, Frame, Observation, Scenario, Transcript, replay_io,
};
use orch_types::{Id, Json, ManualClock, new_id};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader, duplex, split};
use tokio::sync::{Notify, watch};

const WAIT: Duration = Duration::from_secs(10);
fn directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upgrades/controls")
}
fn expected(version: &str, name: &str) -> Scenario {
    if name != "background_control_declared_but_missing" {
        return corpus::expected(version, name);
    }
    serde_json::from_slice(
        &std::fs::read(
            directory()
                .join(version)
                .join(format!("{name}.expected.json")),
        )
        .unwrap(),
    )
    .unwrap()
}
fn frames(version: &str, name: &str) -> Vec<Frame> {
    if name != "background_control_declared_but_missing" {
        return corpus::frames(version, name);
    }
    std::fs::read_to_string(directory().join(version).join(format!("{name}.jsonl")))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

struct Control {
    kill: Arc<Notify>,
    ended: watch::Receiver<Option<ProcessExit>>,
}
#[async_trait]
impl ProcessControl for Control {
    async fn wait(&mut self) -> ProcessExit {
        loop {
            let known = self.ended.borrow_and_update().clone();
            if let Some(known) = known {
                return known;
            }
            if self.ended.changed().await.is_err() {
                return ProcessExit {
                    code: None,
                    detail: "fixture control ended".into(),
                };
            }
        }
    }
    fn kill(&mut self) {
        self.kill.notify_one();
    }
}
struct Script {
    frames: Mutex<Option<Vec<Frame>>>,
    clock: ManualClock,
    task: Mutex<Option<tokio::task::JoinHandle<Result<Transcript, String>>>>,
}
impl Script {
    fn new(frames: Vec<Frame>, clock: ManualClock) -> Arc<Self> {
        Arc::new(Self {
            frames: Mutex::new(Some(frames)),
            clock,
            task: Mutex::new(None),
        })
    }
    async fn finish(&self, expected_exit: Option<Option<i32>>) {
        let task = self.task.lock().unwrap().take().unwrap();
        let transcript = tokio::time::timeout(WAIT, task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(transcript.exit_code, expected_exit);
        assert!(
            self.frames.lock().unwrap().is_none(),
            "the real adapter must launch exactly one process"
        );
    }
}
#[async_trait]
impl Launcher for Script {
    async fn launch(&self, request: LaunchRequest) -> Result<HarnessProcess, LaunchError> {
        let home = request
            .env
            .get("CODEX_HOME")
            .expect("the test supplied an owned home")
            .clone();
        let frames = self
            .frames
            .lock()
            .unwrap()
            .take()
            .expect("only one native process");
        // Pipe capacity is backpressure, not a protocol frame limit.
        let (adapter, harness) = duplex(64 * 1024);
        let (stdout, stdin) = split(adapter);
        let (error_writer, stderr) = duplex(64 * 1024);
        let (ended, receiver) = watch::channel(None);
        let kill = Arc::new(Notify::new());
        let signal = kill.clone();
        let clock = self.clock.clone();
        let task = tokio::spawn(async move {
            let (reader, mut writer) = split(harness);
            let mut reader = BufReader::new(reader);
            let mut error_writer = error_writer;
            let mut bindings = Bindings::from([("codex-home".into(), Json::String(home))]);
            let mut result = replay_io(
                &frames,
                &mut reader,
                &mut writer,
                &mut error_writer,
                &clock,
                &mut bindings,
                WAIT,
            )
            .await
            .map_err(|error| error.to_string());
            let exit = if let Some(code) = result
                .as_ref()
                .ok()
                .and_then(|transcript| transcript.exit_code)
            {
                ProcessExit {
                    code,
                    detail: "Fixture native exit".into(),
                }
            } else if let Some(error) = result.as_ref().err().cloned() {
                ProcessExit {
                    code: Some(1),
                    detail: error,
                }
            } else {
                tokio::select! {
                    eof = reader.fill_buf() => {
                        if eof.is_ok_and(|bytes| bytes.is_empty()) {
                            ProcessExit { code: Some(0), detail: String::new() }
                        } else {
                            result = Err("adapter emitted unlisted bytes after its declared exchange".into());
                            ProcessExit { code: Some(1), detail: "unlisted adapter frame".into() }
                        }
                    }
                    () = signal.notified() => ProcessExit { code: None, detail: "ended by adapter".into() },
                }
            };
            let _ = ended.send_replace(Some(exit));
            drop((reader, writer, error_writer));
            result
        });
        *self.task.lock().unwrap() = Some(task);
        Ok(HarnessProcess {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            stderr: Some(Box::new(stderr)),
            control: Box::new(Control {
                kill,
                ended: receiver,
            }),
        })
    }
}

fn bypass(name: &str) -> bool {
    matches!(
        name,
        "approval_unknown"
            | "approval_unknown_method"
            | "approval_checkpoint"
            | "approval_prompt_user_rule"
            | "approval_network"
    )
}
async fn next(stream: &mut EventStream) -> AdapterEvent {
    tokio::time::timeout(WAIT, stream.next())
        .await
        .unwrap()
        .expect("native adapter event")
}
async fn observe_until(
    adapter: &mut CodexAdapter,
    stream: &mut EventStream,
    thread: Id,
    name: &str,
    observed: &mut Vec<Observation>,
    terminal: &str,
) {
    loop {
        let native = next(stream).await;
        let event = Observation::from_event(&native).unwrap();
        assert_ne!(event.name, "raw.frame", "raw logging is disabled");
        let done = match terminal {
            "pin-error" => {
                event.name == "item.completed"
                    && event.body["payload"]["code"] == "harness_diverged"
            }
            "background-completed" => {
                event.name == "session.background_tasks"
                    && event.body["tasks"]
                        .as_array()
                        .is_some_and(|tasks| tasks.iter().any(|task| task["state"] == "completed"))
            }
            "native-closed" => {
                event.name == "item.completed"
                    && event.body["payload"]["data"]["change"] == "session_lifecycle"
            }
            other => event.name == other,
        };
        if event.name == "approval.requested" {
            let reference = event.body["request_ref"].as_str().unwrap();
            let request = &event.body["request"];
            let decision = match request["request_kind"].as_str().unwrap() {
                "user_input" => ApprovalDecision::Answers {
                    answers: BTreeMap::from([("fixture-question".into(), vec!["Keep".into()])]),
                    note: None,
                },
                "mcp_elicitation" => ApprovalDecision::Elicitation {
                    action: ElicitationAction::Cancel,
                    content: None,
                },
                _ => {
                    let options = request["options"].as_array().unwrap();
                    if matches!(name, "approval_unknown" | "approval_unknown_method") {
                        assert_eq!(options.len(), 1);
                        assert_eq!(options[0]["id"], CANCEL_UNRECOGNIZED_APPROVAL);
                        assert_eq!(options[0]["style"], "deny");
                    }
                    if matches!(name, "approval_checkpoint" | "approval_prompt_user_rule") {
                        assert_eq!(
                            options
                                .iter()
                                .map(|option| option["id"].as_str().unwrap())
                                .collect::<Vec<_>>(),
                            ["once", "deny", "abort"]
                        );
                    }
                    let deny = options
                        .iter()
                        .find(|option| option["style"] == "deny")
                        .expect("a real refusal must be offered");
                    ApprovalDecision::Option {
                        option_id: deny["id"].as_str().unwrap().into(),
                        note: None,
                    }
                }
            };
            let result = adapter.resolve_approval(thread, reference, decision).await;
            observed.push(event);
            observed.push(Observation::from_result("resolve_approval", &result).unwrap());
            result.unwrap();
        } else {
            observed.push(event);
        }
        if done {
            return;
        }
    }
}
async fn stop(
    adapter: &mut CodexAdapter,
    stream: &mut EventStream,
    observed: &mut Vec<Observation>,
) {
    adapter.stop().await.unwrap();
    loop {
        let native = next(stream).await;
        let ended = matches!(&native, AdapterEvent::Status(status) if status.status == HarnessState::Offline);
        observed.push(Observation::from_event(&native).unwrap());
        if ended {
            return;
        }
    }
}
async fn replay(version: &str, name: &str) {
    let expected = expected(version, name);
    if let Scenario::Unsupported { reason } = &expected {
        assert!(!reason.is_empty());
        if !matches!(name, "approval_plan" | "background_end") {
            assert_eq!(version, "0.165.0");
            assert!(matches!(
                name,
                "approval_checkpoint" | "approval_prompt_user_rule"
            ));
        }
        return;
    }
    let clock = orch_testkit::clock::manual();
    let script = Script::new(frames(version, name), clock.clone());
    let home = tempfile::tempdir().unwrap();
    let (agent, provider, thread) = (new_id(), new_id(), new_id());
    let mut adapter = CodexAdapter::with_launcher(script.clone()).with_clock(Arc::new(clock));
    if name == "background_control_declared_but_missing" {
        let mut schema: Json =
            serde_json::from_str(include_str!("../schema/0.167.0/app-server.schema.json")).unwrap();
        schema["definitions"]["ClientRequest"]["oneOf"].as_array_mut().unwrap().push(json!({"properties":{"method":{"enum":["thread/backgroundTerminals/list","thread/backgroundTerminals/terminate"]}}}));
        adapter = adapter
            .with_native_schema("0.170.0".into(), schema)
            .unwrap();
    }
    let (sink, mut stream) = EventSink::channel();
    let started = adapter
        .start(
            AgentStart {
                provider_kind: None,
                provider_id: provider,
                agent_id: agent,
                harness_kind: "codex".into(),
                profile: AgentProfile::default(),
                home: Some(home.path().to_string_lossy().into_owned()),
                provider_config: json!({}),
                loopback: Loopback::new(41999, Secret::new("fixture-loopback")),
                binary: None,
            },
            sink,
        )
        .await;
    let mut observed = vec![Observation::from_result("start", &started).unwrap()];
    if name == "start" {
        if version == "0.165.0" {
            assert!(started.unwrap_err().detail().contains("older"));
        } else {
            started.unwrap();
            stop(&mut adapter, &mut stream, &mut observed).await;
        }
    } else {
        started.unwrap();
        let mode = match name {
            "resume" => SessionMode::Resume,
            "fork" => SessionMode::Fork,
            _ => SessionMode::New,
        };
        let native = match mode {
            SessionMode::Resume => Some("fixture-thread"),
            SessionMode::Fork => Some("source-thread"),
            _ => None,
        };
        let mut spec = common::plain_spec(
            "/fixture/work",
            agent,
            provider,
            new_id(),
            thread,
            mode,
            native,
        );
        spec.settings.approval_mode = if bypass(name) {
            ApprovalMode::Bypass
        } else {
            ApprovalMode::Ask
        };
        let approval = spec.settings.approval_mode;
        assert!(!spec.settings.raw_events);
        let opened = adapter.open_session(spec).await;
        observed.push(Observation::from_result("open_session", &opened).unwrap());
        opened.unwrap();
        let idle = name.starts_with("pin_")
            || matches!(
                name,
                "resume"
                    | "fork"
                    | "session_close"
                    | "session_unload"
                    | "settings_server"
                    | "automatic_divergence"
                    | "background_end_unavailable"
                    | "background_control_declared_but_missing"
            );
        if idle {
            if matches!(name, "session_unload" | "settings_server") {
                assert!(adapter.discover_sessions().await.unwrap().is_empty());
            }
            let terminal = if name.starts_with("pin_") {
                "pin-error"
            } else {
                match name {
                    "automatic_divergence" | "settings_server" => "pin-error",
                    "session_unload" => "native-closed",
                    _ => "session.opened",
                }
            };
            observe_until(
                &mut adapter,
                &mut stream,
                thread,
                name,
                &mut observed,
                terminal,
            )
            .await;
        } else {
            let turn = new_id();
            adapter
                .start_turn(TurnStart {
                    native_model: None,
                    native_wake: None,
                    plan: false,
                    thread_id: thread,
                    turn_id: turn,
                    text: "Fixture input".into(),
                    images: vec![],
                    model: "mock-large".into(),
                    effort: None,
                    params: json!({}),
                    approval_mode: approval,
                    sandbox_mode: SandboxMode::Open,
                })
                .await
                .unwrap();
            if matches!(name, "interrupt" | "steer") {
                observe_until(
                    &mut adapter,
                    &mut stream,
                    thread,
                    name,
                    &mut observed,
                    "turn.started",
                )
                .await;
                let result = if name == "interrupt" {
                    adapter.interrupt(thread, turn).await
                } else {
                    adapter
                        .steer(TurnSteer {
                            thread_id: thread,
                            turn_id: turn,
                            text: "Keep the original scope.".into(),
                            images: vec![],
                        })
                        .await
                };
                observed.push(Observation::from_result(name, &result).unwrap());
                result.unwrap();
            }
            let terminal = if name == "interrupt" {
                "turn.command_cleanup"
            } else if matches!(name, "process_exit" | "broken_stream") {
                "session.lost"
            } else if name == "background_wake" {
                "background-completed"
            } else {
                "turn.completed"
            };
            observe_until(
                &mut adapter,
                &mut stream,
                thread,
                name,
                &mut observed,
                terminal,
            )
            .await;
        }
        if name == "background_wake" {
            let starts: Vec<_> = observed
                .iter()
                .filter(|row| row.name == "turn.started")
                .collect();
            assert_eq!(
                starts.len(),
                1,
                "native task completion cannot admit its own turn"
            );
            let origin = starts[0].body["turn_id"].clone();
            drop(starts);
            for task in observed
                .iter()
                .filter(|row| row.name == "session.background_tasks")
                .flat_map(|row| row.body["tasks"].as_array().unwrap())
            {
                assert_eq!(task["origin_turn"], origin);
            }
            // This fixture supplies the controller's admitted input; Engine tests own its original-author/current-cap admission.
            adapter
                .start_turn(TurnStart {
                    native_wake: None,
                    plan: false,
                    thread_id: thread,
                    turn_id: new_id(),
                    native_model: None,
                    text: "Continue after the fixture command completed.".into(),
                    images: vec![],
                    model: "mock-large".into(),
                    effort: None,
                    params: json!({}),
                    approval_mode: approval,
                    sandbox_mode: SandboxMode::Open,
                })
                .await
                .unwrap();
            observe_until(
                &mut adapter,
                &mut stream,
                thread,
                name,
                &mut observed,
                "turn.completed",
            )
            .await;
            let turns: Vec<_> = observed
                .iter()
                .filter(|row| row.name == "turn.started")
                .collect();
            assert_eq!(turns.len(), 2);
            assert_ne!(turns[0].body["turn_id"], turns[1].body["turn_id"]);
        }
        if matches!(
            name,
            "background_end_unavailable" | "background_control_declared_but_missing"
        ) {
            let background = adapter.background_commands(thread).await;
            observed.push(Observation::from_result("background_commands", &background).unwrap());
            assert!(!background.unwrap().supported);
            if name == "background_end_unavailable" {
                let ended = adapter
                    .end_background_command(thread, "unoffered-command")
                    .await;
                observed.push(Observation::from_result("end_background_command", &ended).unwrap());
                assert_eq!(
                    ended.unwrap().reason.as_deref(),
                    Some("This Codex version cannot end background terminals.")
                );
            }
        }
        if matches!(name, "process_exit" | "broken_stream") {
            let loss = observed
                .iter()
                .find(|row| row.name == "session.lost")
                .unwrap();
            let reason = loss.body["reason"].as_str().unwrap();
            assert!(
                reason.contains('7') && reason.contains("Fixture native exit diagnostic"),
                "{reason}"
            );
            adapter.stop().await.unwrap();
            while let Ok(event) = stream.try_recv() {
                observed.push(Observation::from_event(&event).unwrap());
            }
        } else {
            let closed = adapter.close_session(thread).await;
            observed.push(Observation::from_result("close_session", &closed).unwrap());
            closed.unwrap();
            stop(&mut adapter, &mut stream, &mut observed).await;
        }
    }
    script
        .finish(if matches!(name, "process_exit" | "broken_stream") {
            Some(Some(7))
        } else {
            None
        })
        .await;
    expected
        .assert_observed(&observed, &mut Bindings::new())
        .unwrap_or_else(|error| panic!("{version}/{name}: {error}; actual {observed:#?}"));
    if matches!(name, "approval_unknown" | "approval_unknown_method") {
        assert!(
            !serde_json::to_string(&observed)
                .unwrap()
                .contains("private-control-input")
        );
    }
}

#[test]
fn both_registered_corpora_cover_every_shared_scenario_and_every_exact_managed_pin() {
    for &version in corpus::VERSIONS {
        let registered = corpus::build(version);
        let manifest = &registered.manifest;
        assert_eq!(manifest.program, "codex");
        assert_eq!(manifest.version, version);
        let actual: BTreeSet<_> = manifest.scenarios.keys().cloned().collect();
        let wanted: BTreeSet<_> = corpus::controls()
            .into_iter()
            .chain(corpus::NOTIFICATIONS.iter().map(|name| (*name).to_owned()))
            .collect();
        assert_eq!(
            actual, wanted,
            "every listed case must reach its real replay seam"
        );
        let metadata: Json = serde_json::from_slice(
            &std::fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("schema")
                    .join(version)
                    .join("sources.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let Evidence::PublishedProtocol {
            package,
            references,
        } = &manifest.evidence
        else {
            panic!("schema-derived fixtures must never claim a native recording");
        };
        assert_eq!(package.version, version);
        assert_eq!(
            package.sha256,
            metadata["files"][0]["sha256"].as_str().unwrap()
        );
        assert_eq!(
            references,
            &metadata["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|file| format!(
                    "{}#sha256={}",
                    file["source"].as_str().unwrap(),
                    file["sha256"].as_str().unwrap()
                ))
                .collect::<Vec<_>>()
        );
        let expected_pins = orch_adapter_codex::process::automation_pins();
        let pins: BTreeMap<_, _> = manifest
            .managed_pins
            .iter()
            .map(|pin| (pin.name.clone(), pin.value.clone()))
            .collect();
        assert_eq!(pins, expected_pins);
        assert_eq!(manifest.managed_pins.len(), expected_pins.len());
        for pin in &manifest.managed_pins {
            assert_eq!(pin.divergence_scenario, corpus::pin_case(&pin.name));
            let Scenario::Supported {
                events,
                ignore_events,
                quiet,
            } = &manifest.scenarios[&pin.divergence_scenario]
            else {
                panic!("each pin must have its own actual actor evidence");
            };
            assert!(!quiet && !ignore_events.contains(&"item.completed".into()));
            assert!(
                events
                    .iter()
                    .any(|row| row.body["payload"]["data"]["value"]["pins"] == json!([pin.name]))
            );
            assert!(
                events
                    .iter()
                    .any(|row| row.body["payload"]["code"] == "harness_diverged")
            );
        }
        for (name, scenario) in &manifest.scenarios {
            match scenario {
                Scenario::Supported {
                    events,
                    ignore_events,
                    quiet,
                } => {
                    assert!(!quiet && !events.is_empty());
                    assert!(!ignore_events.contains(&"item.completed".into()));
                    assert!(!registered.frames[name].is_empty());
                }
                Scenario::Unsupported { reason } => {
                    assert!(!reason.is_empty());
                    if matches!(name.as_str(), "approval_plan" | "background_end") {
                        let schema: Json = serde_json::from_slice(
                            &std::fs::read(
                                Path::new(env!("CARGO_MANIFEST_DIR"))
                                    .join("schema")
                                    .join(version)
                                    .join("app-server.schema.json"),
                            )
                            .unwrap(),
                        )
                        .unwrap();
                        let (definition, methods): (_, &[&str]) = if name == "background_end" {
                            (
                                "ClientRequest",
                                &[
                                    "thread/backgroundTerminals/list",
                                    "thread/backgroundTerminals/terminate",
                                ],
                            )
                        } else {
                            ("ServerRequest", &["item/plan/requestApproval"])
                        };
                        assert!(
                            !schema["definitions"][definition]["oneOf"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|variant| variant["properties"]["method"]["enum"]
                                    .as_array()
                                    .is_some_and(|names| names.iter().any(|name| methods
                                        .iter()
                                        .any(|method| name.as_str() == Some(*method)))))
                        );
                    } else {
                        assert_eq!(version, "0.165.0");
                        assert!(matches!(
                            name.as_str(),
                            "approval_checkpoint" | "approval_prompt_user_rule"
                        ));
                    }
                }
            }
        }
    }
}
#[tokio::test]
async fn registered_native_controls_and_each_pin_replay_through_the_real_actor_without_a_version_override()
 {
    for &version in corpus::VERSIONS {
        for name in corpus::controls() {
            replay(version, &name).await;
        }
        // This explicitly synthetic future build tests a declared method that the server then refuses.
        replay(version, "background_control_declared_but_missing").await;
    }
}
