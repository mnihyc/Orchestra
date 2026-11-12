//! Independent SDK-derived corpora qualify the current reader; they do not claim either binary was run.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
mod common;

use common::corpus::{Actor, SESSION, WAIT, turn};
use orch_adapter_claude_code::{
    conversation::{self, Command},
    events::{EventMapper, MapperSettings},
    sessions::Origin,
    stream::Frame as NativeFrame,
};
use orch_agent_contract::session::{ApprovalDecision, ElicitationAction, TurnSteer};
use orch_testkit::harness_corpus::{
    Bindings, Corpus, Evidence, Frame, Observation, SCENARIOS, Scenario, assert_unrecognized,
    collect_through, fixture_root,
};
use orch_types::Json;
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

const VERSIONS: [&str; 2] = ["2.1.312", "2.1.319"];

fn uses_actor(name: &str) -> bool {
    matches!(
        name,
        "start"
            | "resume"
            | "fork"
            | "session_close"
            | "session_unload"
            | "interrupt"
            | "steer"
            | "steer_late"
            | "broken_stream"
            | "process_exit"
            | "background_end"
            | "mcp_elicitation"
            | "settings_program"
            | "settings_server"
            | "automatic_divergence"
    ) || name.starts_with("approval_")
        || name.starts_with("tolerance_")
        || name == "user_input"
}
fn idle(name: &str) -> bool {
    matches!(
        name,
        "start"
            | "resume"
            | "fork"
            | "session_close"
            | "session_unload"
            | "settings_program"
            | "settings_server"
            | "automatic_divergence"
    )
}

/// Ordinary native messages use the public mapper; control requests are never sent through this path.
fn replay_mapper(corpus: &Corpus, name: &str) -> Vec<Observation> {
    let clock = orch_testkit::clock::manual();
    let mut mapper = EventMapper::new(
        MapperSettings {
            directory: "/work".into(),
            cwd: "/work".into(),
            raw_events: false,
        },
        Arc::new(clock.clone()),
    );
    mapper.process_started(true);
    let mut rows = if name == "compaction_idle" {
        Vec::new()
    } else {
        mapper
            .begin_turn_with_model(turn(), "mock-large", None)
            .iter()
            .map(|event| Observation::from_session(event).unwrap())
            .collect()
    };
    for frame in &corpus.frames[name] {
        match frame {
            Frame::FromHarness { frame } => {
                let frame = NativeFrame::parse(&frame.to_string()).unwrap();
                assert!(!matches!(
                    frame.kind(),
                    orch_adapter_claude_code::stream::FrameKind::ControlRequest
                        | orch_adapter_claude_code::stream::FrameKind::ControlResponse
                ));
                rows.extend(
                    mapper
                        .on_frame(&frame)
                        .iter()
                        .map(|event| Observation::from_session(event).unwrap()),
                );
            }
            Frame::Tick { milliseconds } => {
                clock.advance(chrono::Duration::milliseconds(i64::from(*milliseconds)))
            }
            _ => panic!("{name} requires a real native actor, not an ordinary mapper"),
        }
    }
    if name == "subagents" {
        assert!(
            rows.iter().any(|row| row.name == "item.started"
                && row.body["stream"] == "sub:agent"
                && row.body["parent_native_ref"] == "agent"
                && row.body["turn_id"] == json!(turn())),
            "child output must retain the actual parent and originating turn"
        );
    }
    if name == "compaction_idle" {
        assert!(
            !rows
                .iter()
                .any(|row| row.name == "item.started" || row.name == "turn.started"),
            "an idle compaction fact cannot invent an input or turn"
        );
    }
    rows
}

async fn resolve(actor: &Actor, name: &str) -> Observation {
    let decision = match name {
        "mcp_elicitation" => ApprovalDecision::Elicitation {
            action: ElicitationAction::Cancel,
            content: None,
        },
        "user_input" => ApprovalDecision::Answers {
            answers: BTreeMap::from([("q1".into(), vec!["Yes".into()])]),
            note: None,
        },
        _ => ApprovalDecision::Option {
            option_id: if name == "approval_plan" {
                "keep_planning"
            } else {
                "deny"
            }
            .into(),
            note: Some("Corpus refusal".into()),
        },
    };
    let answer = actor
        .handle
        .ask(|reply| Command::Resolve {
            request_ref: "permission".into(),
            decision,
            reply,
        })
        .await
        .unwrap();
    let row = Observation::from_result("resolve", &answer).unwrap();
    answer.unwrap();
    row
}

async fn replay_actor(corpus: &Corpus, name: &str) -> Vec<Observation> {
    let origin = match name {
        "resume" => Origin::Resume,
        "fork" => Origin::Fork {
            from: "00000000-0000-0000-0000-000000000005".into(),
        },
        _ => Origin::New,
    };
    let managed = matches!(
        name,
        "settings_program" | "settings_server" | "automatic_divergence"
    );
    let mut actor =
        Actor::start_with_home(corpus.frames[name].clone(), origin, idle(name), managed).await;
    let mut rows = vec![
        Observation::from_result(
            if idle(name) { "warm" } else { "start_turn" },
            &actor.opened,
        )
        .unwrap(),
    ];
    actor.opened.as_ref().unwrap();

    if name.starts_with("approval_") || matches!(name, "user_input" | "mcp_elicitation") {
        rows.extend(
            collect_through(&mut actor.events, "approval.requested", WAIT)
                .await
                .unwrap(),
        );
        rows.push(resolve(&actor, name).await);
    } else if matches!(name, "interrupt" | "steer" | "steer_late") {
        rows.extend(
            collect_through(&mut actor.events, "turn.started", WAIT)
                .await
                .unwrap(),
        );
        let answer = if name == "interrupt" {
            actor
                .handle
                .ask(|reply| Command::Interrupt {
                    turn_id: turn(),
                    reply,
                })
                .await
                .unwrap()
        } else {
            actor
                .handle
                .ask(|reply| Command::Steer {
                    steer: TurnSteer {
                        thread_id: common::corpus::thread(),
                        turn_id: turn(),
                        text: "Corpus steer".into(),
                        images: vec![],
                    },
                    blocks: conversation::message_blocks("Corpus steer", vec![]),
                    reply,
                })
                .await
                .unwrap()
        };
        rows.push(
            Observation::from_result(
                if name == "interrupt" {
                    "interrupt"
                } else {
                    "steer"
                },
                &answer,
            )
            .unwrap(),
        );
        if name == "steer_late" {
            assert_eq!(answer.unwrap_err().code(), "not_found");
        } else {
            answer.unwrap();
        }
    } else if name == "background_end" {
        rows.extend(
            collect_through(&mut actor.events, "session.background_tasks", WAIT)
                .await
                .unwrap(),
        );
        let roster = actor
            .handle
            .ask(|reply| Command::Background { reply })
            .await;
        rows.push(Observation::from_result("background", &roster).unwrap());
        let id = roster
            .unwrap()
            .items
            .first()
            .expect("the actual observed native run is offered")
            .id
            .clone();
        let ended = actor
            .handle
            .ask(|reply| Command::EndBackground { id, reply })
            .await;
        rows.push(Observation::from_result("end_background", &ended).unwrap());
        ended.unwrap();
    } else if matches!(name, "broken_stream" | "process_exit") {
        rows.extend(
            collect_through(&mut actor.events, "session.lost", WAIT)
                .await
                .unwrap(),
        );
    }
    rows.extend(actor.collect().await);
    // These source controls do not pretend a native version probe ran; they check the actual launch origin.
    let launched = actor
        .scripts
        .launched
        .lock()
        .unwrap()
        .first()
        .unwrap()
        .clone();
    let argument = |name: &str| {
        launched
            .args
            .windows(2)
            .find(|pair| pair[0] == name)
            .map(|pair| pair[1].as_str())
    };
    match name {
        "start" => assert_eq!(argument("--session-id"), Some(SESSION)),
        "resume" => assert_eq!(argument("--resume"), Some(SESSION)),
        "fork" => {
            assert_eq!(
                argument("--resume"),
                Some("00000000-0000-0000-0000-000000000005")
            );
            assert_eq!(argument("--session-id"), Some(SESSION));
            assert!(launched.args.iter().any(|arg| arg == "--fork-session"));
        }
        _ => {}
    }
    if name == "automatic_divergence" {
        let installed: Json = serde_json::from_str(argument("--settings").unwrap()).unwrap();
        for pin in &corpus.manifest.managed_pins {
            let actual = pin
                .name
                .split('.')
                .try_fold(&installed, |value, key| value.get(key));
            assert_eq!(
                actual,
                Some(&pin.value),
                "the real managed launch must install {}",
                pin.name
            );
            assert!(
                rows.iter().any(|row| row.name == "item.completed"
                    && row.body["payload"]["code"] == "harness_diverged"
                    && row.body["payload"]["details"]["setting"] == pin.name.as_str()),
                "{} needs its own actual divergence error",
                pin.name
            );
        }
    }
    match name {
        "tolerance_unknown_event" => {
            assert_unrecognized(&rows, "Claude Code", "future_update", 2).unwrap()
        }
        "tolerance_unknown_method" => {
            assert_unrecognized(&rows, "Claude Code", "control_request/future_method", 1).unwrap()
        }
        "tolerance_optional_fields" => {
            assert_unrecognized(&rows, "Claude Code", "future_update", 1).unwrap()
        }
        _ => {}
    }
    assert_eq!(
        rows.iter().filter(|row| row.name == "turn.started").count(),
        usize::from(!idle(name)),
        "only the explicitly issued turn may start"
    );
    for row in &rows {
        if row.name != "adapter.result" {
            assert_eq!(row.body["thread_id"], json!(common::corpus::thread()));
            assert_eq!(row.body["segment_id"], json!(common::corpus::segment()));
        }
    }
    assert!(!rows.iter().any(|row| row.name == "raw.frame"));
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("DO NOT PUBLISH")
    );
    let closed = actor.handle.ask(|reply| Command::Close { reply }).await;
    rows.push(Observation::from_result("close", &closed).unwrap());
    closed.unwrap();
    actor.scripts.finish().await;
    if name == "steer_late" {
        let transcript = actor.scripts.transcripts();
        let sent: Vec<_> = transcript[0]
            .frames
            .iter()
            .filter_map(|step| match step {
                Frame::ToHarness { frame, .. } => Some(frame),
                _ => None,
            })
            .collect();
        let message = sent
            .iter()
            .find(|frame| frame.pointer("/message/content/0/text") == Some(&json!("Corpus steer")))
            .unwrap();
        let cancel = sent
            .iter()
            .find(|frame| frame.pointer("/request/subtype") == Some(&json!("cancel_async_message")))
            .unwrap();
        assert_eq!(
            cancel["request"]["message_uuid"], message["uuid"],
            "the exact unconsumed steer is cancelled"
        );
    }
    if name == "steer" {
        let transcripts = actor.scripts.transcripts();
        let sent: Vec<_> = transcripts[0]
            .frames
            .iter()
            .filter_map(|step| match step {
                Frame::ToHarness { frame, .. } => Some(frame),
                _ => None,
            })
            .collect();
        assert_eq!(
            sent.iter().filter(|frame| frame["type"] == "user").count(),
            2,
            "only the original input and actual acknowledged steer are sent"
        );
        assert!(
            !sent
                .iter()
                .any(|frame| frame.pointer("/request/subtype")
                    == Some(&json!("cancel_async_message"))),
            "an acknowledged steer must not be cancelled"
        );
    }
    rows
}

#[tokio::test]
async fn both_published_sdk_corpora_replay_every_supported_scenario_through_its_real_consumer() {
    for version in VERSIONS {
        let corpus = Corpus::load(&fixture_root().join("claude-code").join(version)).unwrap();
        assert_eq!(corpus.manifest.version, version);
        for (name, scenario) in &corpus.manifest.scenarios {
            if matches!(scenario, Scenario::Unsupported { .. }) {
                continue;
            }
            let observed = if uses_actor(name) {
                replay_actor(&corpus, name).await
            } else {
                replay_mapper(&corpus, name)
            };
            scenario
                .assert_observed(&observed, &mut Bindings::new())
                .unwrap_or_else(|error| panic!("{version}/{name}: {error}; actual {observed:#?}"));
        }
    }
}

#[test]
fn every_scenario_and_pin_has_explicit_versioned_evidence_without_expanding_native_support() {
    for version in VERSIONS {
        let corpus = Corpus::load(&fixture_root().join("claude-code").join(version)).unwrap();
        assert_eq!(corpus.manifest.scenarios.len(), SCENARIOS.len() + 1);
        assert!(matches!(
            corpus.manifest.scenarios.get("steer_late"),
            Some(Scenario::Supported { .. })
        ));
        let Evidence::PublishedProtocol { package, .. } = &corpus.manifest.evidence else {
            panic!("these are written SDK fixtures, not native recordings");
        };
        assert_eq!(package.name, "@anthropic-ai/claude-agent-sdk");
        assert_eq!(
            package.version,
            format!("0.3.{}", version.rsplit('.').next().unwrap())
        );
        assert_eq!(
            corpus.manifest.managed_pins.len(),
            if version == "2.1.319" { 55 } else { 54 }
        );
        assert_eq!(
            corpus
                .manifest
                .managed_pins
                .iter()
                .any(|pin| pin.name == "env.CLAUDE_CODE_DISABLE_FEATURE_FLAGS"),
            version == "2.1.319",
            "the older unsupported runtime dialect cannot claim the newly qualified feature-flags pin"
        );
        for &name in SCENARIOS {
            assert!(corpus.manifest.scenarios.contains_key(name));
        }
        for (name, scenario) in &corpus.manifest.scenarios {
            if matches!(scenario, Scenario::Unsupported { .. }) {
                assert!(matches!(
                    name.as_str(),
                    "approval_checkpoint" | "approval_prompt_user_rule" | "approval_network"
                ));
            }
        }
    }
    assert_eq!(
        orch_adapter_claude_code::config::SUPPORTED_RANGE,
        "=2.1.319",
        "protocol replay does not qualify an installed older binary"
    );
}
