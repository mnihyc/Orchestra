//! Shared corpus contract, exercised without a vendor binary, network service or database.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]

use orch_agent_contract::adapter::{AdapterError, EventSink};
use orch_agent_contract::events::{SessionEvent, SessionLost, SessionTitle};
use orch_testkit::harness_corpus::{
    Bindings, Corpus, CorpusError, Evidence, ExpectedEvent, Frame, ManagedPin, Manifest,
    Observation, SCENARIOS, Scenario, assert_unrecognized, collect_through, matches, replay_io,
};
use orch_types::{Clock, Id};
use serde_json::json;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

fn observed(name: &str, body: serde_json::Value) -> Observation {
    Observation {
        name: name.into(),
        body,
    }
}
fn expected(name: &str, body: serde_json::Value) -> ExpectedEvent {
    ExpectedEvent {
        name: name.into(),
        body,
        subset: true,
    }
}
fn supported(events: Vec<ExpectedEvent>) -> Scenario {
    Scenario::Supported {
        events,
        quiet: false,
        ignore_events: vec![],
    }
}
fn manifest() -> Manifest {
    Manifest {
        format: 1,
        program: "fourth-program".into(),
        version: "1.0.0".into(),
        evidence: Evidence::Synthetic {
            reason: "Shared loader contract fixture, not a vendor recording".into(),
        },
        managed_pins: vec![],
        scenarios: SCENARIOS
            .iter()
            .map(|name| {
                (
                    (*name).into(),
                    if name.starts_with("tolerance_") {
                        supported(vec![expected("session.title", json!({"title":"fixture"}))])
                    } else {
                        Scenario::Unsupported {
                            reason: "Loader-only synthetic fixture has no native program".into(),
                        }
                    },
                )
            })
            .collect(),
    }
}

#[test]
fn every_build_accounts_for_all_scenarios_and_cannot_skip_tolerance() {
    let valid = manifest();
    valid.validate().unwrap();
    let mut missing = valid.clone();
    missing.scenarios.remove("fork");
    assert!(missing.validate().unwrap_err().to_string().contains("fork"));
    let mut unexplained = valid.clone();
    unexplained
        .scenarios
        .insert("fork".into(), Scenario::Unsupported { reason: " ".into() });
    assert!(unexplained.validate().is_err());
    let mut skipped = valid.clone();
    skipped.scenarios.insert(
        "tolerance_unknown_event".into(),
        Scenario::Unsupported {
            reason: "not checked".into(),
        },
    );
    assert!(skipped.validate().is_err());
    let mut quiet = valid;
    quiet.scenarios.insert(
        "tolerance_malformed".into(),
        Scenario::Supported {
            events: vec![],
            quiet: true,
            ignore_events: vec![],
        },
    );
    assert!(quiet.validate().is_err());
}

#[test]
fn a_managed_pin_needs_an_actual_divergence_error_not_its_name_in_text() {
    let mut corpus = manifest();
    corpus.managed_pins.push(ManagedPin {
        name: "automatic_compaction".into(),
        value: json!(false),
        divergence_scenario: "automatic_divergence".into(),
    });
    corpus.scenarios.insert(
        "automatic_divergence".into(),
        supported(vec![expected(
            "item.completed",
            json!({"payload":{"message":"harness_diverged was never emitted"}}),
        )]),
    );
    assert!(corpus.validate().is_err());
    corpus.scenarios.insert(
        "automatic_divergence".into(),
        supported(vec![expected(
            "raw.frame",
            json!({"payload":{"code":"harness_diverged"}}),
        )]),
    );
    assert!(corpus.validate().is_err());
    corpus.scenarios.insert(
        "automatic_divergence".into(),
        supported(vec![expected(
            "item.completed",
            json!({"payload":{"code":"harness_diverged"}}),
        )]),
    );
    corpus.validate().unwrap();
}

#[test]
fn recorded_and_schema_derived_evidence_remain_distinct_and_exactly_anchored() {
    let mut value = serde_json::to_value(manifest()).unwrap();
    value["evidence"] = json!({"kind":"published_protocol","references":["https://protocol.example/schema"],"package":{"name":"example-sdk","version":"3.4.5","sha256":"1".repeat(64)}});
    let corpus: Manifest = serde_json::from_value(value.clone()).unwrap();
    corpus.validate().unwrap();
    value["evidence"]["package"]["sha256"] = json!("not-a-checksum");
    assert!(
        serde_json::from_value::<Manifest>(value)
            .unwrap()
            .validate()
            .is_err()
    );
    let mut recorded = manifest();
    recorded.evidence = Evidence::Recorded { references: vec![] };
    assert!(recorded.validate().is_err());
}

#[test]
fn captures_keep_native_identity_and_do_not_alias_output_text_or_schema_refs() {
    let mut ids = Bindings::new();
    let home = json!("/fixture/owned-launcher/home");
    matches(
        &json!({"$replay_capture":"launcher_home"}),
        &home,
        false,
        &mut ids,
        "frame.result.codexHome",
    )
    .unwrap();
    matches(
        &json!({"$replay_ref":"launcher_home"}),
        &home,
        false,
        &mut ids,
        "reply.result.codexHome",
    )
    .unwrap();
    assert!(
        matches(
            &json!({"$replay_ref":"launcher_home"}),
            &json!("/fixture/another-launcher/home"),
            false,
            &mut ids,
            "reply.result.codexHome",
        )
        .is_err()
    );
    assert!(
        matches(
            &json!({"$replay_capture":"other_home"}),
            &home,
            false,
            &mut ids,
            "frame.result.codexHomeText",
        )
        .is_err()
    );
    matches(
        &json!({"$replay_capture":"request"}),
        &json!(42),
        false,
        &mut ids,
        "frame.id",
    )
    .unwrap();
    matches(
        &json!({"$replay_ref":"request"}),
        &json!(42),
        false,
        &mut ids,
        "reply.id",
    )
    .unwrap();
    assert!(
        matches(
            &json!({"$replay_capture":"request"}),
            &json!(43),
            false,
            &mut ids,
            "frame.id"
        )
        .is_err()
    );
    assert!(
        matches(
            &json!({"$replay_capture":"text"}),
            &json!("new answer"),
            false,
            &mut ids,
            "frame.params.text"
        )
        .is_err()
    );
    assert!(
        matches(
            &json!({"$replay_ref":"request"}),
            &json!(42),
            false,
            &mut ids,
            "frame.params.model"
        )
        .is_err()
    );
    matches(
        &json!({"$ref":"#/$defs/Question"}),
        &json!({"$ref":"#/$defs/Question"}),
        false,
        &mut ids,
        "frame.params.schema",
    )
    .unwrap();
}

#[test]
fn json_objects_may_reorder_but_missing_members_and_array_order_are_never_erased() {
    let mut ids = Bindings::new();
    matches(
        &json!({"a":1,"b":2}),
        &json!({"b":2,"a":1}),
        false,
        &mut ids,
        "frame",
    )
    .unwrap();
    assert!(matches(&json!({"a":null}), &json!({}), true, &mut ids, "frame").is_err());
    assert!(matches(&json!([1, 2]), &json!([2, 1]), true, &mut ids, "frame").is_err());
    assert!(
        matches(
            &json!({"a":1}),
            &json!({"a":1,"new":true}),
            false,
            &mut ids,
            "frame"
        )
        .is_err()
    );
    matches(
        &json!({"a":1}),
        &json!({"a":1,"new":true}),
        true,
        &mut ids,
        "frame",
    )
    .unwrap();
}

#[test]
fn exact_observations_detect_dropped_extra_and_reordered_markers() {
    let case = supported(vec![
        expected("session.title", json!({"title":"one"})),
        expected("session.title", json!({"title":"two"})),
    ]);
    let rows = vec![
        observed("session.title", json!({"title":"one","native_extra":true})),
        observed("session.title", json!({"title":"two"})),
    ];
    case.assert_observed(&rows, &mut Bindings::new()).unwrap();
    assert!(
        case.assert_observed(&rows[..1], &mut Bindings::new())
            .is_err()
    );
    let mut reversed = rows.clone();
    reversed.reverse();
    assert!(
        case.assert_observed(&reversed, &mut Bindings::new())
            .is_err()
    );
    let mut extra = rows;
    extra.push(observed("raw.frame", json!({})));
    assert!(case.assert_observed(&extra, &mut Bindings::new()).is_err());
}

#[test]
fn the_same_observer_handles_public_mappers_streams_and_actual_call_errors() {
    let event = SessionEvent::SessionTitle(SessionTitle {
        title: "kept title".into(),
    });
    assert_eq!(
        Observation::from_session(&event).unwrap(),
        observed("session.title", json!({"title":"kept title"}))
    );
    let error: Result<(), AdapterError> = Err(AdapterError::HarnessError(
        "malformed frame: {broken".into(),
    ));
    let result = Observation::from_result("agent.start", &error).unwrap();
    assert_eq!(result.name, "adapter.result");
    assert_eq!(result.body["ok"], false);
    assert!(
        result.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("{broken")
    );
}

#[test]
fn unknown_frames_are_counted_even_without_raw_logging_and_without_a_turn() {
    let clock = orch_testkit::clock::manual();
    let mut events = Vec::new();
    for sequence in 1..=2 {
        events.extend(
            orch_agent_contract::observations::unrecognized_event(
                "Fourth program",
                "future/update",
                sequence,
                None,
                clock.time(),
            )
            .unwrap()
            .iter()
            .map(Observation::from_session)
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        );
    }
    assert_unrecognized(&events, "Fourth program", "future/update", 2).unwrap();
    assert!(assert_unrecognized(&events[..1], "Fourth program", "future/update", 2).is_err());
    assert!(assert_unrecognized(&[], "Fourth program", "future/update", 1).is_err());
}

#[tokio::test]
async fn native_message_uuid_echo_and_cancel_share_one_identity_without_aliasing_text_or_settings()
{
    let frames = vec![
        Frame::ToHarness {
            frame: json!({"type":"user","uuid":{"$replay_capture":"message"},"message":{"role":"user","content":[{"type":"text","text":"Literal input"}]}}),
            subset: false,
        },
        Frame::FromHarness {
            frame: json!({"type":"user","uuid":{"$replay_ref":"message"},"message":{"role":"user","content":[{"type":"text","text":"Literal input"}]}}),
        },
        Frame::ToHarness {
            frame: json!({"type":"control_request","request_id":"cancel-1","request":{"subtype":"cancel_async_message","message_uuid":{"$replay_ref":"message"}}}),
            subset: false,
        },
        Frame::Exit { code: Some(0) },
    ];
    let (mut adapter, harness) = tokio::io::duplex(64 * 1024);
    let (input, mut output) = tokio::io::split(harness);
    let mut input = BufReader::new(input);
    let mut stderr = tokio::io::sink();
    let clock = orch_testkit::clock::manual();
    let mut ids = Bindings::new();
    let replay = replay_io(
        &frames,
        &mut input,
        &mut output,
        &mut stderr,
        &clock,
        &mut ids,
        Duration::from_secs(10),
    );
    let drive = async {
        let uuid = "11111111-2222-4333-8444-555555555555";
        adapter.write_all(format!("{}\n",json!({"type":"user","uuid":uuid,"message":{"role":"user","content":[{"type":"text","text":"Literal input"}]}})).as_bytes()).await.unwrap();
        let mut reader = BufReader::new(&mut adapter);
        let mut echo = String::new();
        reader.read_line(&mut echo).await.unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&echo).unwrap(),
            json!({"type":"user","uuid":uuid,"message":{"role":"user","content":[{"type":"text","text":"Literal input"}]}})
        );
        reader.get_mut().write_all(format!("{}\n",json!({"type":"control_request","request_id":"cancel-1","request":{"subtype":"cancel_async_message","message_uuid":uuid}})).as_bytes()).await.unwrap();
    };
    let (transcript, ()) = tokio::join!(replay, drive);
    assert_eq!(transcript.unwrap().exit_code, Some(Some(0)));
    let actual = json!("11111111-2222-4333-8444-555555555555");
    assert_eq!(ids.get("message"), Some(&actual));
    for path in [
        "frame.message.text",
        "frame.params.model",
        "frame.params.settings",
        "frame.result.output",
    ] {
        assert!(
            matches(
                &json!({"$replay_ref":"message"}),
                &actual,
                false,
                &mut ids,
                path
            )
            .is_err(),
            "a native identity cannot hide data at {path}"
        );
    }
    assert!(
        matches(
            &json!({"$replay_ref":"message"}),
            &json!("different"),
            false,
            &mut ids,
            "frame.request.message_uuid"
        )
        .is_err()
    );
    assert!(
        matches(
            &json!({"$replay_capture":"object"}),
            &json!({"opaque":true}),
            false,
            &mut ids,
            "frame.uuid"
        )
        .is_err()
    );
}

#[tokio::test]
async fn duplex_replay_preserves_both_directions_clock_schema_refs_and_partial_exit_text() {
    let frames = vec![
        Frame::ToHarness {
            frame: json!({"id":{"$replay_capture":"q"},"method":"initialize","params":{"schema":{"$ref":"#/$defs/Question"}}}),
            subset: false,
        },
        Frame::Tick { milliseconds: 17 },
        Frame::FromHarness {
            frame: json!({"id":{"$replay_ref":"q"},"result":{"ok":true}}),
        },
        Frame::Stderr {
            line: "synthetic diagnostic".into(),
        },
        Frame::Raw {
            line: "{broken".into(),
            newline: false,
        },
        Frame::Exit { code: Some(7) },
    ];
    // Backpressure capacity, not a frame-size cap: large fixtures stream through the same pipes.
    let (mut adapter, harness) = tokio::io::duplex(64 * 1024);
    let (input, mut output) = tokio::io::split(harness);
    let mut input = BufReader::new(input);
    let (mut stderr, mut read_stderr) = tokio::io::duplex(64 * 1024);
    let clock = orch_testkit::clock::manual();
    let before = clock.now();
    let mut ids = Bindings::new();
    let replay = replay_io(
        &frames,
        &mut input,
        &mut output,
        &mut stderr,
        &clock,
        &mut ids,
        Duration::from_secs(10),
    );
    let drive = async {
        adapter.write_all(b"{\"params\":{\"schema\":{\"$ref\":\"#/$defs/Question\"}},\"method\":\"initialize\",\"id\":91}\n").await.unwrap();
        let mut reader = BufReader::new(&mut adapter);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap(),
            json!({"id":91,"result":{"ok":true}})
        );
        let mut rest = String::new();
        reader.read_to_string(&mut rest).await.unwrap();
        assert_eq!(rest, "{broken");
    };
    let (trace, ()) = tokio::join!(replay, drive);
    let trace = trace.unwrap();
    assert_eq!(trace.frames.len(), frames.len());
    assert_eq!(trace.exit_code, Some(Some(7)));
    assert_eq!(ids.get("q"), Some(&json!(91)));
    assert_eq!(clock.now(), before + chrono::Duration::milliseconds(17));
    drop(stderr);
    let mut note = String::new();
    read_stderr.read_to_string(&mut note).await.unwrap();
    assert_eq!(note, "synthetic diagnostic\n");
}

#[tokio::test]
async fn a_dead_process_and_a_missing_reply_are_failures_not_successful_empty_replays() {
    let frames = [Frame::ToHarness {
        frame: json!({"method":"expected"}),
        subset: false,
    }];
    let mut input = BufReader::new(&b""[..]);
    let mut out = tokio::io::sink();
    let mut err = tokio::io::sink();
    assert!(
        replay_io(
            &frames,
            &mut input,
            &mut out,
            &mut err,
            &orch_testkit::clock::manual(),
            &mut Bindings::new(),
            Duration::from_secs(10)
        )
        .await
        .is_err()
    );
    let (_live, input) = tokio::io::duplex(64 * 1024);
    let mut input = BufReader::new(input);
    assert!(matches!(
        replay_io(
            &frames,
            &mut input,
            &mut out,
            &mut err,
            &orch_testkit::clock::manual(),
            &mut Bindings::new(),
            Duration::from_millis(1)
        )
        .await,
        Err(CorpusError::Timeout(_))
    ));
}

#[tokio::test]
async fn collecting_uses_an_actual_native_barrier_and_keeps_every_intermediate_event() {
    let (sink, mut events) = EventSink::channel();
    let thread = Id::from_u128(1);
    let segment = Id::from_u128(2);
    sink.session(
        thread,
        segment,
        SessionEvent::SessionTitle(SessionTitle {
            title: "first".into(),
        }),
    )
    .await
    .unwrap();
    sink.session(
        thread,
        segment,
        SessionEvent::SessionTitle(SessionTitle {
            title: "second".into(),
        }),
    )
    .await
    .unwrap();
    sink.session(
        thread,
        segment,
        SessionEvent::SessionLost(SessionLost {
            reason: "native barrier".into(),
        }),
    )
    .await
    .unwrap();
    let rows = collect_through(&mut events, "session.lost", Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].body["title"], "first");
    assert_eq!(rows[1].body["title"], "second");
    assert_eq!(rows[0].body["thread_id"], json!(thread));
}

#[test]
fn corpus_loading_requires_listed_supported_bytes_and_never_traverses_a_filename() {
    let root = tempfile::tempdir().unwrap();
    let corpus = manifest();
    std::fs::write(
        root.path().join("expected.json"),
        serde_json::to_vec(&corpus).unwrap(),
    )
    .unwrap();
    for (name, scenario) in &corpus.scenarios {
        if matches!(scenario, Scenario::Supported { .. }) {
            std::fs::write(
                root.path().join(format!("{name}.jsonl")),
                "{\"direction\":\"tick\",\"milliseconds\":0}\n",
            )
            .unwrap();
        }
    }
    let loaded = Corpus::load(root.path()).unwrap();
    assert_eq!(loaded.manifest, corpus);
    std::fs::write(root.path().join("forgotten.jsonl"), "{}\n").unwrap();
    assert!(Corpus::load(root.path()).is_err());
    let mut traversal = corpus;
    traversal.scenarios.insert(
        "../outside".into(),
        supported(vec![expected("session.title", json!({}))]),
    );
    assert!(traversal.validate().is_err());
}
