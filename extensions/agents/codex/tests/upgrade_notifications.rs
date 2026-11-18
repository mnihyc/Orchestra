//! Registered notification cases use the shared corpus, byte replayer and production RPC reader/mapper.
//! Native controls and approvals remain with actual adapter actor tests.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]

#[path = "common/upgrade_corpus.rs"]
mod corpus;

use corpus::{NOTIFICATIONS as CASES, VERSIONS, expected, frames};
use orch_adapter_codex::{
    events::{Context, Mapper, MapperConfig, OutputStore},
    protocol::{self, Incoming},
};
use orch_testkit::harness_corpus::{Bindings, Frame, Observation, assert_unrecognized, replay_io};
use orch_types::{Id, Json};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{BufReader, duplex, split};
use tokio::sync::Notify;

const WAIT: Duration = Duration::from_secs(10);
async fn replay(version: &str, name: &str) {
    let frames = frames(version, name);
    let count = frames
        .iter()
        .map(|frame| match frame {
            Frame::FromHarness { frame } => {
                assert!(frame.get("method").and_then(Json::as_str).is_some());
                assert!(
                    frame.get("id").is_none(),
                    "native requests require the real approval actor"
                );
                1
            }
            Frame::Raw { line, newline } => {
                assert!(
                    *newline && !line.contains(['\n', '\r']),
                    "one malformed native line is one compatibility observation"
                );
                1
            }
            Frame::Exit { .. } => 1,
            _ => panic!("this suite exercises notifications; controls belong to the adapter actor"),
        })
        .sum::<usize>();
    let clock = orch_testkit::clock::manual();
    let replay_clock = clock.clone();
    // This is stream backpressure, not a frame limit; the real RPC reader retains its production bound.
    let (adapter, harness) = duplex(64 * 1024);
    let (stdout, stdin) = split(adapter);
    let (_rpc, mut incoming) = protocol::connect(stdout, stdin, None);
    let done = Arc::new(Notify::new());
    let end = done.clone();
    let script = tokio::spawn(async move {
        let (reader, mut writer) = split(harness);
        let mut reader = BufReader::new(reader);
        let mut stderr = tokio::io::sink();
        let mut bindings = Bindings::new();
        let result = replay_io(
            &frames,
            &mut reader,
            &mut writer,
            &mut stderr,
            &replay_clock,
            &mut bindings,
            WAIT,
        )
        .await;
        // Keep successful native streams owned through the deciding observation barrier. Only Exit is process loss.
        end.notified().await;
        drop((reader, writer, stderr));
        result
    });
    let owned = tempfile::tempdir().unwrap();
    let outputs = OutputStore::new(owned.path().join("outputs"));
    let context = Context {
        clock: &clock,
        outputs: &outputs,
        windows: None,
    };
    let mut mapper = Mapper::new(MapperConfig {
        output_capture_bytes: 67_108_864,
        thread_id: Id::from_u128(1),
        segment_id: Id::from_u128(2),
        native_thread: "fixture-thread".into(),
        directory: "/fixture/work".into(),
    });
    if name != "compaction_idle" {
        mapper.expect_turn(Id::from_u128(3));
    }
    let mut observed = Vec::new();
    for _ in 0..count {
        let native = tokio::time::timeout(WAIT, incoming.recv())
            .await
            .unwrap()
            .expect("a declared native frame");
        let events = match native {
            Incoming::Notification { method, params } => mapper.handle(&method, &params, &context),
            Incoming::Malformed { text } => vec![mapper.malformed_frame(Some(&text))],
            Incoming::Closed { reason } => mapper.lost(&reason),
            Incoming::Request { .. } => {
                panic!("a native request must exercise its real control actor")
            }
        };
        observed.extend(
            events
                .iter()
                .map(|event| Observation::from_session(event).unwrap()),
        );
    }
    expected(version, name)
        .assert_observed(&observed, &mut Bindings::new())
        .unwrap_or_else(|error| panic!("{version}/{name}: {error}; actual {observed:#?}"));
    corpus::assert_nonfailure_has_no_error(&observed);
    match name {
        "tolerance_unknown_event" => {
            assert_unrecognized(&observed, "Codex", "future/update", 2).unwrap()
        }
        "tolerance_unknown_method" => {
            assert_unrecognized(&observed, "Codex", "future/method", 1).unwrap()
        }
        "compaction_idle" => assert!(observed.iter().all(|event| event.name != "turn.started"
            && event.body["payload"]["data"]["turn_id"].is_null())),
        "tool_web_find" => {
            let raw = observed
                .iter()
                .find_map(|row| {
                    row.body
                        .pointer("/payload/raw/input/inline")
                        .and_then(Json::as_str)
                })
                .unwrap();
            let input: Json = serde_json::from_str(raw).unwrap();
            assert_eq!(input["action"]["pattern"], "classes");
        }
        "tolerance_malformed" => assert!(observed.iter().any(|row| {
            row.body["payload"]["message"]
                .as_str()
                .is_some_and(|text| text.contains("{broken fixture"))
        })),
        _ => {}
    }
    let encoded = serde_json::to_string(&observed).unwrap();
    assert!(
        !encoded.contains("unretained-future-payload") && !encoded.contains("private-hook-prompt")
    );
    assert!(!observed.iter().any(|row| row.name == "raw.frame"));
    done.notify_one();
    let transcript = tokio::time::timeout(WAIT, script)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        transcript.exit_code,
        if name == "transport_exit" {
            Some(Some(7))
        } else {
            None
        }
    );
}

#[tokio::test]
async fn both_published_notification_dialects_replay_without_a_vendor_binary_or_version_gate_override()
 {
    for &version in VERSIONS {
        for &name in CASES {
            replay(version, name).await;
        }
    }
}
