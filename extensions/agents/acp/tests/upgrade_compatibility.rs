//! Published-schema fixtures run through the actual ACP launcher, RPC reader and session actor.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use orch_adapter_acp::AcpAdapter;
use orch_adapter_acp::process::{
    HarnessProcess, LaunchError, LaunchRequest, Launcher, ProcessControl, ProcessExit,
};
use orch_agent_contract::adapter::{AgentAdapter, EventSink};
use orch_agent_contract::session::{ApprovalDecision, SessionMode};
use orch_testkit::harness_corpus::{
    Bindings, Corpus, Frame, Observation, Scenario, Transcript, assert_unrecognized, fixture_root,
    replay_io,
};
use orch_types::{ManualClock, new_id};
use tokio::io::{BufReader, duplex, split};
use tokio::sync::{Notify, watch};

const WAIT: Duration = Duration::from_secs(10);

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
                    detail: "script control closed".into(),
                };
            }
        }
    }
    fn kill(&mut self) {
        self.kill.notify_one();
    }
}
struct Done;
#[async_trait]
impl ProcessControl for Done {
    async fn wait(&mut self) -> ProcessExit {
        ProcessExit {
            code: Some(0),
            detail: String::new(),
        }
    }
    fn kill(&mut self) {}
}

/// Launcher glue only: all matching, aliases, clocks, bytes and transcript validation use the shared replayer.
struct Scripts {
    frames: Mutex<VecDeque<Vec<Frame>>>,
    clock: ManualClock,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    results: Arc<Mutex<Vec<Result<Transcript, String>>>>,
}
impl Scripts {
    fn new(frames: Vec<Vec<Frame>>, clock: ManualClock) -> Arc<Self> {
        Arc::new(Self {
            frames: Mutex::new(frames.into()),
            clock,
            tasks: Mutex::new(vec![]),
            results: Arc::default(),
        })
    }
    async fn finish(&self) {
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
        for task in tasks {
            tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
        }
        assert!(
            self.frames.lock().unwrap().is_empty(),
            "a declared native process was not opened"
        );
        for result in self.results.lock().unwrap().iter() {
            assert!(result.is_ok(), "native replay failed: {result:?}");
        }
    }
}
#[async_trait]
impl Launcher for Scripts {
    async fn launch(&self, request: LaunchRequest) -> Result<HarnessProcess, LaunchError> {
        if request.args == ["--version"] {
            return Ok(HarnessProcess {
                stdin: Box::new(tokio::io::sink()),
                stdout: Box::new(std::io::Cursor::new(b"0.2.1-rc.2\n".to_vec())),
                stderr: None,
                control: Box::new(Done),
            });
        }
        let frames = self
            .frames
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra process");
        let (adapter, harness) = duplex(64 * 1024);
        let (stdout, stdin) = split(adapter);
        let (reader, mut writer) = split(harness);
        let mut reader = BufReader::new(reader);
        let (mut stderr, error_reader) = duplex(64 * 1024);
        let (ended, receiver) = watch::channel(None);
        let kill = Arc::new(Notify::new());
        let signal = kill.clone();
        let clock = self.clock.clone();
        let results = self.results.clone();
        let task = tokio::spawn(async move {
            let mut bindings = Bindings::new();
            let replay = tokio::select! {
                result = replay_io(&frames, &mut reader, &mut writer, &mut stderr, &clock, &mut bindings, WAIT) => Some(result),
                () = signal.notified() => None,
            };
            let how = match replay {
                Some(Ok(transcript)) => {
                    let code = transcript.exit_code;
                    results.lock().unwrap().push(Ok(transcript));
                    if let Some(code) = code {
                        ProcessExit {
                            code,
                            detail: String::new(),
                        }
                    } else {
                        // A successful probe remains alive until the real adapter ends it.
                        signal.notified().await;
                        ProcessExit {
                            code: None,
                            detail: "ended by adapter".into(),
                        }
                    }
                }
                Some(Err(error)) => {
                    let detail = error.to_string();
                    results.lock().unwrap().push(Err(detail.clone()));
                    ProcessExit {
                        code: Some(1),
                        detail,
                    }
                }
                None => {
                    results.lock().unwrap().push(Err(
                        "adapter ended before its declared exchange completed".into(),
                    ));
                    ProcessExit {
                        code: None,
                        detail: "early cancellation".into(),
                    }
                }
            };
            ended.send_replace(Some(how));
        });
        self.tasks.lock().unwrap().push(task);
        Ok(HarnessProcess {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            stderr: Some(Box::new(error_reader)),
            control: Box::new(Control {
                kill,
                ended: receiver,
            }),
        })
    }
}

fn corpus() -> Corpus {
    Corpus::load(&fixture_root().join("dsh/0.2.1-rc.2")).unwrap()
}
fn native_program() -> orch_adapter_acp::program::AcpProgram {
    let mut program = orch_adapter_acp::programs::dsh().unwrap();
    // This corpus qualifies the vendor's native ACP, not Orch's separately versioned companion protocol.
    program.companion = None;
    program
}

async fn replay_case(corpus: &Corpus, name: &str) {
    let only_start = name.starts_with("protocol_") && name != "protocol_newer_minor";
    let no_turn = matches!(
        name,
        "start"
            | "resume"
            | "session_close"
            | "session_unload"
            | "protocol_newer_minor"
            | "automatic_divergence"
            | "idle_unknown_event"
    );
    let clock = orch_testkit::clock::manual();
    let frames = corpus.frames[name].clone();
    let scripts = Scripts::new(
        if only_start {
            vec![frames]
        } else {
            vec![corpus.frames["start"][..2].to_vec(), frames]
        },
        clock.clone(),
    );
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let (provider, profile) = (new_id(), new_id());
    let mut adapter = AcpAdapter::with_launcher(native_program(), scripts.clone(), Arc::new(clock));
    let (sink, mut stream) = EventSink::channel();
    let started = adapter
        .start(common::agent_start(provider, profile, home.path()), sink)
        .await;
    let mut observed = vec![Observation::from_result("start", &started).unwrap()];
    if only_start {
        let message = started.unwrap_err().detail();
        if name == "protocol_newer_major" {
            assert!(
                message.contains("version 2") && message.contains("version 1"),
                "{message}"
            );
        }
    } else {
        started.unwrap();
        let mut spec = common::spec_in(
            work.path(),
            profile,
            provider,
            if name == "resume" {
                SessionMode::Resume
            } else {
                SessionMode::New
            },
        );
        if name == "resume" {
            spec.settings.native_session_ref = Some("fixture-session".into());
        }
        spec.settings.raw_events = false;
        let thread = spec.settings.thread_id;
        let opened = adapter.open_session(spec.clone()).await;
        observed.push(Observation::from_result("open_session", &opened).unwrap());
        opened.unwrap();
        let turn = common::turn_for(&spec, "fixture input");
        let turn_id = turn.turn_id;
        if !no_turn {
            adapter.start_turn(turn).await.unwrap();
        }
        let mut interrupted = false;
        let terminal = if matches!(name, "process_exit" | "broken_stream") {
            "session.lost"
        } else if no_turn {
            if matches!(name, "automatic_divergence" | "idle_unknown_event") {
                "item.completed"
            } else {
                "session.opened"
            }
        } else {
            "turn.completed"
        };
        loop {
            let native = tokio::time::timeout(WAIT, stream.next())
                .await
                .unwrap()
                .expect("adapter event stream closed");
            let event = Observation::from_event(&native).unwrap();
            assert_ne!(event.name, "raw.frame", "raw logging is disabled");
            if name == "interrupt" && event.name == "item.delta" && !interrupted {
                adapter.interrupt(thread, turn_id).await.unwrap();
                interrupted = true;
            }
            if event.name == "approval.requested" {
                let reference = event.body["request_ref"].as_str().unwrap();
                let options = event.body["request"]["options"].as_array().unwrap();
                let deny = options
                    .iter()
                    .find(|option| option["style"] == "deny")
                    .expect("a real refusal/cancellation must be offered");
                adapter
                    .resolve_approval(
                        thread,
                        reference,
                        ApprovalDecision::Option {
                            option_id: deny["id"].as_str().unwrap().into(),
                            note: None,
                        },
                    )
                    .await
                    .unwrap();
            }
            let done = event.name == terminal;
            observed.push(event);
            if done {
                break;
            }
        }
        if name == "tolerance_malformed" {
            assert!(observed.iter().any(|row| {
                row.name == "item.completed"
                    && row.body["payload"]["message"]
                        .as_str()
                        .is_some_and(|text| text.contains("{broken"))
            }));
        }
        match name {
            "tolerance_unknown_event" => {
                assert_unrecognized(&observed, "DeepSeek Harness", "future_update", 2).unwrap()
            }
            "idle_unknown_event" => {
                assert_unrecognized(&observed, "DeepSeek Harness", "future_idle_update", 1).unwrap()
            }
            "tolerance_unknown_method" => {
                assert_unrecognized(&observed, "DeepSeek Harness", "future/notice", 1).unwrap();
                assert_unrecognized(&observed, "DeepSeek Harness", "future/request", 1).unwrap();
            }
            _ => {}
        }
        if no_turn {
            assert!(
                !observed.iter().any(|row| row.name == "turn.started"),
                "an idle fact cannot start a turn"
            );
        }
        let closed = adapter.close_session(thread).await;
        observed.push(Observation::from_result("close_session", &closed).unwrap());
        closed.unwrap();
    }
    adapter.stop().await.unwrap();
    scripts.finish().await;
    corpus.manifest.scenarios[name]
        .assert_observed(&observed, &mut Bindings::new())
        .unwrap_or_else(|error| panic!("{name}: {error}; actual {observed:#?}"));
}

#[tokio::test]
async fn every_supported_native_dsh_scenario_replays_through_the_actual_adapter() {
    let corpus = corpus();
    assert_eq!(corpus.manifest.program, "dsh");
    assert_eq!(corpus.manifest.version, "0.2.1-rc.2");
    for (name, scenario) in &corpus.manifest.scenarios {
        if matches!(scenario, Scenario::Supported { .. }) {
            replay_case(&corpus, name).await;
        }
    }
}

#[tokio::test]
async fn compatible_acp_additions_are_accepted_and_a_new_major_is_refused_with_its_reason() {
    let corpus = corpus();
    for name in [
        "protocol_newer_minor",
        "protocol_newer_major",
        "protocol_missing_version",
        "protocol_invalid_version",
    ] {
        replay_case(&corpus, name).await;
    }
}

#[test]
fn unsupported_native_cases_are_explicit_and_do_not_claim_companion_features_or_recordings() {
    let corpus = corpus();
    assert!(matches!(
        corpus.manifest.scenarios["fork"],
        Scenario::Unsupported { .. }
    ));
    for &name in orch_testkit::harness_corpus::SCENARIOS {
        assert!(corpus.manifest.scenarios.contains_key(name));
    }
    assert!(matches!(
        corpus.manifest.evidence,
        orch_testkit::harness_corpus::Evidence::PublishedProtocol { .. }
    ));
}
