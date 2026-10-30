//! Exact native user-input prefixes exercised against the pinned app server and model wire.

mod common;
#[path = "input_prefix/confirmed_fork.rs"]
mod confirmed_fork;

use async_trait::async_trait;
use common::{Rig, WAIT};
use orch_agent_contract::adapter::{AdapterEvent, AgentAdapter};
use orch_agent_contract::discovery::RecordedEntry;
use orch_agent_contract::events::{SessionEvent, TurnOutcome};
use orch_agent_contract::history::NativePrefixSource;
use orch_agent_contract::session::{ApprovalDecision, SessionMode, TurnSteer};
use orch_testkit::prefix_history::{codex_model_input, marker_tape};
use orch_types::{Id, new_id};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;
const A: &str = "PREFIX-USER-A";
const B: &str = "PREFIX-USER-B";
const C: &str = "PREFIX-USER-C";
const REVISED: &str = "PREFIX-REVISED-INPUT";
const BEFORE: &str = "PREFIX-BEFORE-STEER-ASSISTANT";
const AFTER: &str = "PREFIX-AFTER-STEER-ASSISTANT";
const HELD: &str = "PREFIX-HELD-TOOL";
const RESULT: &str = "PREFIX-TOOL-RESULT";
const STEER: &str = "PREFIX-STEER-USER";
const CONTINUE: &str = "PREFIX-CONTINUE-ORIGINAL";

struct WireGate {
    delegate: Arc<common::DockerLauncher>,
    program: PathBuf,
    sent: PathBuf,
}
#[async_trait]
impl orch_adapter_codex::process::Launcher for WireGate {
    async fn launch(
        &self,
        mut request: orch_adapter_codex::process::LaunchRequest,
    ) -> Result<orch_adapter_codex::process::HarnessProcess, orch_adapter_codex::process::LaunchError>
    {
        let native = std::mem::replace(&mut request.program, "node".into());
        request.args.insert(0, native);
        request
            .args
            .insert(0, self.program.to_string_lossy().into_owned());
        request.env.insert(
            "ORCH_PREFIX_WIRE_SENT".into(),
            self.sent.to_string_lossy().into_owned(),
        );
        orch_adapter_codex::process::Launcher::launch(self.delegate.as_ref(), request).await
    }
}

async fn native_input(
    rig: &mut Rig,
    native: &str,
    marker: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // These identities come from the real native history API's recorded user
    // items, not from global Orch ordinals or guessed turn numbering.
    let record = rig.adapter.read_session(native, true).await?;
    let file = record
        .file
        .ok_or("the native history API produced no record")?;
    rig.prefix_evidence_file(std::path::Path::new(&file.path));
    let mut found = Vec::new();
    for line in std::fs::read_to_string(file.path)?.lines() {
        if let RecordedEntry::Said(words) = serde_json::from_str::<RecordedEntry>(line)?
            && words.text.contains(marker)
        {
            found.push(
                words
                    .native_ref
                    .strip_prefix("said:")
                    .ok_or("the recorded user item has no native namespace")?
                    .to_owned(),
            );
        }
    }
    assert_eq!(
        found.len(),
        1,
        "the model proof must use exactly one actual stored native user-input identity"
    );
    Ok(found.remove(0))
}

/// The file in which the harness keeps the session `native`, found in the home by the name it gives it.
fn source_file(
    rig: &Rig,
    native: &str,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let wanted = format!("-{native}.jsonl");
    let mut pending = vec![rig.home_path().join("sessions")];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(&wanted))
            {
                return Ok(path);
            }
        }
    }
    Err(format!("the file of the native session {native} is not in the home").into())
}

async fn fresh_native_prefix_case(first: bool) -> Proof {
    let mut rig = Rig::start("input-prefix")
        .await
        .ok_or("the pinned Docker harness is required for prefix acceptance")?;
    assert_eq!(rig.boot().await.harness_version, "0.165.0");
    let thread = new_id();
    let initial = rig.spec(thread, SessionMode::New, None);
    let segment = initial.settings.segment_id;
    let original = rig.adapter.open_session(initial).await?;
    let selected = if first { A } else { B };
    let mut selected_model = None;
    for text in [A, B, C] {
        let events = rig.run_turn(thread, text).await;
        assert!(events.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
        if text == selected {
            selected_model = Some(
                rig.model_requests()
                    .last()
                    .ok_or("the selected source model input is absent")?
                    .body
                    .clone(),
            );
        }
    }
    let source = NativePrefixSource {
        thread_id: thread,
        segment_id: segment,
        native_session_ref: original.native_session_ref.clone(),
    };
    let id = native_input(
        &mut rig,
        &original.native_session_ref,
        if first { A } else { B },
    )
    .await?;
    std::fs::write(
        rig.home_path().join("prefix-proof.json"),
        serde_json::to_vec(&json!({
            "case":if first {"first"} else {"middle"},"source":source.clone(),"requested_api_input":id,
        }))?,
    )?;
    let token = rig
        .adapter
        .inspect_input_prefix(source.clone(), &id)
        .await?;
    assert_eq!(token.empty, first);
    std::fs::write(
        rig.home_path().join("prefix-proof.json"),
        serde_json::to_vec(
            &json!({"case":if first {"first"} else {"middle"},"prefix":token.clone()}),
        )?,
    )?;
    let path = source_file(&rig, &original.native_session_ref)?;
    let count = rig.model_requests().len();
    let occupied = rig.spec(
        thread,
        SessionMode::Fork,
        Some(&original.native_session_ref),
    );
    assert_eq!(count, 3, "only the three authored source inputs ran");
    assert!(
        rig.adapter
            .open_input_prefix(occupied, token.clone())
            .await
            .is_err()
    );
    let mut wrong = source;
    wrong.segment_id = new_id();
    assert!(rig.adapter.inspect_input_prefix(wrong, &id).await.is_err());
    assert!(
        rig.adapter
            .inspect_input_prefix(token.source.clone(), "missing-native-user")
            .await
            .is_err()
    );
    assert_eq!(
        rig.model_requests().len(),
        count,
        "refusal must not create hidden model input"
    );
    rig.adapter.close_session(thread).await?;
    let old = std::fs::read(&path)?;
    std::fs::write(rig.home_path().join("prefix-source-before.jsonl"), &old)?;
    let sentinel = rig.workspace.path().join("prefix-current-file");
    std::fs::write(&sentinel, b"current workspace\0\xff")?;
    let mut stale = token.clone();
    stale.fingerprint = orch_types::Hash::of(b"different inspected prefix");
    let target = rig.spec(
        thread,
        SessionMode::Fork,
        Some(&original.native_session_ref),
    );
    assert!(rig.adapter.open_input_prefix(target, stale).await.is_err());
    let target = rig.spec(
        thread,
        SessionMode::Fork,
        Some(&original.native_session_ref),
    );
    let opened = rig.adapter.open_input_prefix(target, token.clone()).await?;
    assert_ne!(
        opened.session.native_session_ref,
        original.native_session_ref
    );
    assert_eq!(opened.prefix, token);
    assert_eq!(
        rig.model_requests().len(),
        count,
        "prefix preparation must defer any goal and author no input"
    );
    assert!(
        std::fs::read(&path)? == old,
        "the original native source bytes changed; see retained prefix failure evidence"
    );
    assert_eq!(std::fs::read(&sentinel)?, b"current workspace\0\xff");
    let turn = rig.turn(thread, REVISED);
    let turn_id = turn.turn_id;
    rig.adapter.start_turn(turn).await?;
    let events = rig.until_turn_ends().await;
    assert!(events.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.turn_id==turn_id && end.status==TurnOutcome::Completed)));
    let request = rig
        .model_requests()
        .pop()
        .ok_or("the revised model input is absent")?;
    let expected: Vec<(String, String)> = if first {
        vec![("user".into(), REVISED.into())]
    } else {
        vec![
            ("user".into(), A.into()),
            ("assistant".into(), A.into()),
            ("user".into(), REVISED.into()),
        ]
    };
    assert_eq!(
        marker_tape(&request.body, &[A, B, C, REVISED], None),
        expected
    );
    assert_eq!(std::fs::read(&sentinel)?, b"current workspace\0\xff");
    rig.adapter.close_session(thread).await?;
    let resume = rig.spec(
        thread,
        SessionMode::Resume,
        Some(&original.native_session_ref),
    );
    rig.adapter.open_session(resume).await?;
    let resumed = rig.run_turn(thread, CONTINUE).await;
    assert!(resumed.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
    let parent = rig
        .model_requests()
        .pop()
        .ok_or("the retained source did not resume")?;
    assert_eq!(
        marker_tape(&parent.body, &[A, B, C, REVISED, CONTINUE], None),
        vec![
            ("user".into(), A.into()),
            ("assistant".into(), A.into()),
            ("user".into(), B.into()),
            ("assistant".into(), B.into()),
            ("user".into(), C.into()),
            ("assistant".into(), C.into()),
            ("user".into(), CONTINUE.into()),
        ],
        "resuming the original retains every complete original exchange without the revised branch"
    );
    // Only a proved empty native prefix regenerates initial context IDs.
    // Retained conversation/tool IDs remain exact in every other case.
    let expected_full = codex_model_input(
        &selected_model.ok_or("the selected source model input was not recorded")?,
        selected,
        first,
    )?;
    let actual_full = codex_model_input(&request.body, REVISED, first)?;
    assert!(
        actual_full == expected_full,
        "full native model input differs: extra interruption guidance or unknown content is not an exact prefix; see retained model-body evidence"
    );
    rig.adapter.stop().await?;
    let _ = rig.keep_prefix_evidence()?;
    rig.prefix_proof_succeeded();
    Ok(())
}

#[tokio::test]
async fn a_fresh_native_prefix_excludes_the_identified_input_and_preserves_its_original_session()
-> Proof {
    fresh_native_prefix_case(false).await
}

#[tokio::test]
async fn an_empty_native_prefix_preserves_the_complete_first_model_input() -> Proof {
    fresh_native_prefix_case(true).await
}

async fn entered(rig: &mut Rig, thread: Id) -> Proof {
    let marker = rig.workspace.path().join("prefix-tool-entered");
    tokio::time::timeout(WAIT, async {
        while !marker.exists() {
            match tokio::time::timeout(Duration::from_millis(20), rig.next_event()).await {
                Ok(AdapterEvent::Session {
                    event: SessionEvent::ApprovalRequested(ask),
                    ..
                }) => {
                    rig.adapter
                        .resolve_approval(
                            thread,
                            &ask.request_ref,
                            ApprovalDecision::Option {
                                option_id: "once".into(),
                                note: None,
                            },
                        )
                        .await?;
                }
                Ok(AdapterEvent::Session {
                    event: SessionEvent::TurnCompleted(_),
                    ..
                }) => {
                    return Err("the native turn ended before its deterministic tool latch".into());
                }
                _ => {}
            }
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn an_actual_native_steer_keeps_its_history_and_refuses_a_coarser_checkpoint() -> Proof {
    let mut rig = Rig::start("input-prefix-steer")
        .await
        .ok_or("the pinned Docker harness is required for prefix acceptance")?;
    let gate = rig.home.path().join("prefix-wire-gate.cjs");
    std::fs::write(&gate, orch_testkit::prefix_history::STEER_WIRE_GATE)?;
    let sent = rig.workspace.path().join("prefix-steer-sent");
    rig.adapter = orch_adapter_codex::CodexAdapter::with_launcher(Arc::new(WireGate {
        delegate: rig.launcher.clone(),
        program: gate,
        sent: sent.clone(),
    }));
    assert_eq!(rig.boot().await.harness_version, "0.165.0");
    let thread = new_id();
    let initial = rig.spec(thread, SessionMode::New, None);
    let segment = initial.settings.segment_id;
    let original = rig.adapter.open_session(initial).await?;
    let base = rig.run_turn(thread, A).await;
    assert!(base.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
    let first = native_input(&mut rig, &original.native_session_ref, A).await?;
    let source = NativePrefixSource {
        thread_id: thread,
        segment_id: segment,
        native_session_ref: original.native_session_ref.clone(),
    };
    std::fs::write(
        rig.home_path().join("prefix-proof.json"),
        serde_json::to_vec(&json!({"case":"steer","source":source.clone()}))?,
    )?;
    std::fs::write(rig.workspace.path().join("prefix-tool-result"), RESULT)?;
    let command = "# PREFIX-HELD-TOOL\nprintf x >> prefix-tool-runs; printf x > prefix-tool-entered; while [ ! -f prefix-tool-release ]; do sleep 0.02; done; cat prefix-tool-result";
    let prompt = format!(
        "#!mock\nsay {BEFORE}\ntool exec_command {}\nthen\nsay {AFTER}\n",
        json!({"cmd":command,"yield_time_ms":30000})
    );
    let turn = rig.turn(thread, &prompt);
    let turn_id = turn.turn_id;
    rig.adapter.start_turn(turn).await?;
    entered(&mut rig, thread).await?;
    assert!(
        rig.adapter
            .inspect_input_prefix(source.clone(), &first)
            .await
            .is_err(),
        "the current exact source lane must be quiescent"
    );
    let release = rig.workspace.path().join("prefix-tool-release");
    let steering = rig.adapter.steer(TurnSteer {
        thread_id: thread,
        turn_id,
        text: STEER.into(),
        images: Vec::new(),
    });
    let releasing = async {
        tokio::time::timeout(WAIT, async {
            while !sent.exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        std::fs::write(release, b"release")?;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    };
    let (steered, released) = tokio::join!(steering, releasing);
    steered?;
    released?;
    let events = rig.until_turn_ends().await;
    assert!(events.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.turn_id==turn_id && end.status==TurnOutcome::Completed)));
    let markers = [A, BEFORE, HELD, AFTER, RESULT, STEER, REVISED, CONTINUE];
    let requests = rig.model_requests();
    let delivered = requests
        .iter()
        .find(|request| {
            marker_tape(&request.body, &[STEER], None).contains(&("user".into(), STEER.into()))
        })
        .ok_or("the native steer never reached model input")?;
    let before = marker_tape(&delivered.body, &markers, Some(STEER));
    assert!(before.contains(&("assistant".into(), BEFORE.into())));
    assert!(before.contains(&("tool_call".into(), HELD.into())));
    // This is the accepted original input, not a successful edited branch.
    // Retain its whole wire body rather than claiming a marker-only cut.
    let full_steer = delivered.body.clone();
    let final_text = events
        .iter()
        .rev()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(done) => done.payload["text"].as_str(),
            _ => None,
        })
        .ok_or("the original native final response is absent")?;
    let mut original_tape = marker_tape(
        &requests
            .last()
            .ok_or("the original native model input is absent")?
            .body,
        &markers,
        None,
    );
    original_tape.extend(marker_tape(
        &json!({"messages":[{"role":"assistant","content":final_text}]}),
        &markers,
        None,
    ));
    original_tape.push(("user".into(), CONTINUE.into()));
    let native = native_input(&mut rig, &original.native_session_ref, STEER).await?;
    assert!(
        matches!(
            rig.adapter
                .inspect_input_prefix(source.clone(), &native)
                .await,
            Err(orch_agent_contract::adapter::AdapterError::Unsupported(_))
        ),
        "an in-turn input must not be rounded to before its whole turn"
    );
    let mut fabricated = rig.adapter.inspect_input_prefix(source, &first).await?;
    fabricated.before_input_ref = native.clone();
    std::fs::write(
        rig.home_path().join("prefix-proof.json"),
        serde_json::to_vec(&json!({
            "case":"accepted-native-steer-with-explicit-checkpoint-refusal","input":native,"full_model_input":full_steer,
        }))?,
    )?;
    let path = source_file(&rig, &original.native_session_ref)?;
    rig.adapter.close_session(thread).await?;
    let old = std::fs::read(&path)?;
    let count = rig.model_requests().len();
    std::fs::write(rig.home_path().join("prefix-source-before.jsonl"), &old)?;
    let target = rig.spec(
        thread,
        SessionMode::Fork,
        Some(&original.native_session_ref),
    );
    assert!(
        matches!(
            rig.adapter.open_input_prefix(target, fabricated).await,
            Err(orch_agent_contract::adapter::AdapterError::Unsupported(_))
        ),
        "a fabricated token cannot bypass the per-input support boundary"
    );
    assert_eq!(
        rig.model_requests().len(),
        count,
        "unsupported preparation authors no model input"
    );
    assert!(
        std::fs::read(&path)? == old,
        "the original native source bytes changed; see retained prefix failure evidence"
    );
    assert_eq!(
        std::fs::read(rig.workspace.path().join("prefix-tool-runs"))?,
        b"x"
    );
    assert!(!rig.home.path().join(".orch-input-prefixes").exists());
    let source = rig.spec(
        thread,
        SessionMode::Resume,
        Some(&original.native_session_ref),
    );
    rig.adapter.open_session(source).await?;
    let resumed = rig.run_turn(thread, CONTINUE).await;
    assert!(resumed.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
    let continued = rig
        .model_requests()
        .pop()
        .ok_or("the original steered native session did not resume")?;
    assert_eq!(marker_tape(&continued.body, &markers, None), original_tape);
    assert_eq!(
        std::fs::read(rig.workspace.path().join("prefix-tool-runs"))?,
        b"x",
        "the retained source continuation must not replay its old tool"
    );
    rig.adapter.stop().await?;
    let _ = rig.keep_prefix_evidence()?;
    rig.prefix_proof_succeeded();
    Ok(())
}

#[tokio::test]
async fn repeated_edits_keep_native_inherited_ranges_and_refuse_changed_missing_or_cyclic_ancestors()
-> Proof {
    const LATER: &str = "PREFIX-CHILD-LATER";
    const AGAIN: &str = "PREFIX-CHILD-REVISED";
    let mut rig = Rig::start("input-prefix-inherited")
        .await
        .ok_or("the pinned Docker harness is required")?;
    assert_eq!(rig.boot().await.harness_version, "0.165.0");
    let thread = new_id();
    let initial = rig.spec(thread, SessionMode::New, None);
    let segment = initial.settings.segment_id;
    let original = rig.adapter.open_session(initial).await?;
    let source = NativePrefixSource {
        thread_id: thread,
        segment_id: segment,
        native_session_ref: original.native_session_ref.clone(),
    };
    let mut selected_model = None;
    for text in [A, B, C] {
        let events = rig.run_turn(thread, text).await;
        assert!(events.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
        if text == B {
            selected_model = Some(
                rig.model_requests()
                    .last()
                    .ok_or("missing source model input")?
                    .body
                    .clone(),
            );
        }
    }
    let original_path = source_file(&rig, &original.native_session_ref)?;
    let original_header: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(&original_path)?
            .lines()
            .next()
            .ok_or("missing source header")?,
    )?;
    assert_eq!(
        original_header["payload"]["history_mode"], "paginated",
        "the real default is not forced to legacy"
    );
    let id = native_input(&mut rig, &original.native_session_ref, B).await?;
    let prefix = rig.adapter.inspect_input_prefix(source, &id).await?;
    rig.adapter.close_session(thread).await?;
    let original_bytes = std::fs::read(&original_path)?;
    let sentinel = rig.workspace.path().join("prefix-current-file");
    std::fs::write(&sentinel, b"current\0\xff")?;
    let first_spec = rig.spec(
        thread,
        SessionMode::Fork,
        Some(&original.native_session_ref),
    );
    let first_segment = first_spec.settings.segment_id;
    let count = rig.model_requests().len();
    let first = rig.adapter.open_input_prefix(first_spec, prefix).await?;
    assert_ne!(
        first.session.native_session_ref,
        original.native_session_ref
    );
    assert_eq!(rig.model_requests().len(), count);
    let events = rig.run_turn(thread, REVISED).await;
    assert!(events.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
    let first_model = rig
        .model_requests()
        .last()
        .ok_or("missing first revised model input")?
        .body
        .clone();
    assert!(
        codex_model_input(&first_model, REVISED, false)?
            == codex_model_input(
                &selected_model.ok_or("missing selected model input")?,
                B,
                false
            )?
    );
    let events = rig.run_turn(thread, LATER).await;
    assert!(events.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
    let later_model = rig
        .model_requests()
        .last()
        .ok_or("missing inherited source model input")?
        .body
        .clone();
    let child_path = source_file(&rig, &first.session.native_session_ref)?;
    let child_header: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(&child_path)?
            .lines()
            .next()
            .ok_or("missing child header")?,
    )?;
    assert_eq!(child_header["payload"]["history_mode"], "paginated");
    assert!(
        child_header["payload"]["history_base"].is_object(),
        "a repeated edit must exercise actual reference-backed history"
    );
    let id = native_input(&mut rig, &first.session.native_session_ref, LATER).await?;
    let source = NativePrefixSource {
        thread_id: thread,
        segment_id: first_segment,
        native_session_ref: first.session.native_session_ref.clone(),
    };
    let prefix = rig.adapter.inspect_input_prefix(source, &id).await?;
    assert!(!prefix.empty);
    rig.adapter.close_session(thread).await?;
    let child_bytes = std::fs::read(&child_path)?;
    std::fs::write(
        rig.home_path().join("prefix-source-before.jsonl"),
        &child_bytes,
    )?;
    std::fs::write(
        rig.home_path().join("prefix-proof.json"),
        serde_json::to_vec(
            &json!({"case":"reference-backed-second-edit","prefix":prefix.clone()}),
        )?,
    )?;
    let count = rig.model_requests().len();
    let original_text = std::str::from_utf8(&original_bytes)?;
    assert!(original_text.contains(A));
    std::fs::write(
        &original_path,
        original_text.replacen(A, "PREFIX-USER-X", 1),
    )?;
    let target = rig.spec(
        thread,
        SessionMode::Fork,
        Some(&first.session.native_session_ref),
    );
    let changed = rig.adapter.open_input_prefix(target, prefix.clone()).await;
    std::fs::write(&original_path, &original_bytes)?;
    assert!(changed.is_err(), "a changed inherited prefix must refuse");
    let missing = original_path.with_extension("prefix-proof-missing");
    std::fs::rename(&original_path, &missing)?;
    let target = rig.spec(
        thread,
        SessionMode::Fork,
        Some(&first.session.native_session_ref),
    );
    let absent = rig.adapter.open_input_prefix(target, prefix.clone()).await;
    std::fs::rename(&missing, &original_path)?;
    assert!(
        absent.is_err(),
        "missing native ancestors cannot be skipped"
    );
    let text = std::str::from_utf8(&child_bytes)?;
    let (header, tail) = text.split_once('\n').ok_or("missing child newline")?;
    let mut cycle: serde_json::Value = serde_json::from_str(header)?;
    cycle["payload"]["history_base"]["thread_id"] = json!(first.session.native_session_ref);
    std::fs::write(&child_path, format!("{cycle}\n{tail}"))?;
    let target = rig.spec(
        thread,
        SessionMode::Fork,
        Some(&first.session.native_session_ref),
    );
    let cyclic = rig.adapter.open_input_prefix(target, prefix.clone()).await;
    std::fs::write(&child_path, &child_bytes)?;
    assert!(cyclic.is_err(), "cyclic native ancestry must refuse");
    assert_eq!(
        rig.model_requests().len(),
        count,
        "all refused preparations create no model turn"
    );
    let target = rig.spec(
        thread,
        SessionMode::Fork,
        Some(&first.session.native_session_ref),
    );
    let second = rig.adapter.open_input_prefix(target, prefix).await?;
    assert_ne!(
        second.session.native_session_ref,
        first.session.native_session_ref
    );
    assert_eq!(rig.model_requests().len(), count);
    let events = rig.run_turn(thread, AGAIN).await;
    assert!(events.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
    let revised = rig
        .model_requests()
        .last()
        .ok_or("missing second revised model input")?
        .body
        .clone();
    assert!(
        codex_model_input(&revised, AGAIN, false)?
            == codex_model_input(&later_model, LATER, false)?,
        "every inherited and local input/tool block must survive the second native edit"
    );
    assert!(std::fs::read(&original_path)? == original_bytes);
    assert!(std::fs::read(&child_path)? == child_bytes);
    assert_eq!(std::fs::read(&sentinel)?, b"current\0\xff");
    assert!(!rig.home_path().join(".orch-input-prefixes").exists());
    rig.adapter.close_session(thread).await?;
    let resume = rig.spec(
        thread,
        SessionMode::Resume,
        Some(&first.session.native_session_ref),
    );
    rig.adapter.open_session(resume).await?;
    let events = rig.run_turn(thread, CONTINUE).await;
    assert!(events.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
    let resumed = rig
        .model_requests()
        .last()
        .ok_or("missing original child continuation")?
        .body
        .clone();
    assert_eq!(
        marker_tape(&resumed, &[A, REVISED, LATER, AGAIN, CONTINUE], None),
        vec![
            ("user".into(), A.into()),
            ("assistant".into(), A.into()),
            ("user".into(), REVISED.into()),
            ("assistant".into(), REVISED.into()),
            ("user".into(), LATER.into()),
            ("assistant".into(), LATER.into()),
            ("user".into(), CONTINUE.into()),
        ]
    );
    rig.adapter.stop().await?;
    let _ = rig.keep_prefix_evidence()?;
    rig.prefix_proof_succeeded();
    Ok(())
}
