//! Replays of the observed native prefix; no terminal result or missing transition is synthesized.
use super::notices;
use base64::Engine as _;
use orch_adapter_claude_code::{
    events::{EventMapper, MapperSettings},
    stream::Frame,
};
use orch_agent_contract::{
    events::{DeltaField, ItemContent, ItemStatus, MarkerPayload, SessionEvent, ToolPayload},
    session::TurnId,
};
use orch_types::{Hash, Json, ManualClock, new_id};
use serde_json::json;
use std::sync::Arc;

struct Capture {
    case: &'static str,
    flags_off_requested: bool,
    frames: usize,
    answer: &'static str,
    source: &'static str,
}

const CAPTURES: [Capture; 8] = [
    Capture {
        case: "startup",
        flags_off_requested: false,
        frames: 20,
        answer: "native conformance reached the model",
        source: include_str!("../fixtures/native/2.1.319/default/startup.json"),
    },
    Capture {
        case: "tools.result_form",
        flags_off_requested: false,
        frames: 36,
        answer: "result forms received",
        source: include_str!("../fixtures/native/2.1.319/default/tools.result_form.json"),
    },
    Capture {
        case: "parallel_calls",
        flags_off_requested: false,
        frames: 43,
        answer: "read both",
        source: include_str!("../fixtures/native/2.1.319/default/parallel_calls.json"),
    },
    Capture {
        case: "image_result",
        flags_off_requested: false,
        frames: 32,
        answer: "image seen",
        source: include_str!("../fixtures/native/2.1.319/default/image_result.json"),
    },
    Capture {
        case: "startup",
        flags_off_requested: true,
        frames: 20,
        answer: "native conformance reached the model",
        source: include_str!("../fixtures/native/2.1.319/disabled/startup.json"),
    },
    Capture {
        case: "tools.result_form",
        flags_off_requested: true,
        frames: 36,
        answer: "result forms received",
        source: include_str!("../fixtures/native/2.1.319/disabled/tools.result_form.json"),
    },
    Capture {
        case: "parallel_calls",
        flags_off_requested: true,
        frames: 43,
        answer: "read both",
        source: include_str!("../fixtures/native/2.1.319/disabled/parallel_calls.json"),
    },
    Capture {
        case: "image_result",
        flags_off_requested: true,
        frames: 32,
        answer: "image seen",
        source: include_str!("../fixtures/native/2.1.319/disabled/image_result.json"),
    },
];

fn document(capture: &Capture) -> Json {
    let document: Json = serde_json::from_str(capture.source).unwrap();
    assert_eq!(document["harness"], "claude-code");
    assert_eq!(document["version"], "2.1.319");
    assert_eq!(document["recorded"], "2026-11-08");
    assert_eq!(document["case"], capture.case);
    assert_eq!(
        document["feature_flags_off_requested"],
        capture.flags_off_requested
    );
    assert_eq!(
        document["raw_frame_count"].as_u64().unwrap(),
        u64::try_from(capture.frames).unwrap()
    );
    assert_eq!(document["frames"].as_array().unwrap().len(), capture.frames);
    let digest = document["original_sha256"].as_str().unwrap();
    assert_eq!(digest.len(), 64);
    assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    for row in document["frames"].as_array().unwrap() {
        assert!(matches!(
            row["direction"].as_str(),
            Some("from_harness" | "to_harness")
        ));
        assert!(row["payload"].is_object());
    }
    document
}

fn incoming(document: &Json) -> impl Iterator<Item = Frame> + '_ {
    document["frames"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["direction"] == "from_harness")
        .map(|row| Frame::parse(&row["payload"].to_string()).unwrap())
}

fn mapper(raw_events: bool) -> EventMapper {
    EventMapper::new(
        MapperSettings {
            directory: "/workspace/case".into(),
            cwd: "/workspace/case".into(),
            raw_events,
        },
        Arc::new(ManualClock::new("2026-11-08T00:00:00Z".parse().unwrap())),
    )
}

struct Replay {
    mapper: EventMapper,
    turn: TurnId,
    events: Vec<SessionEvent>,
    outputs: tempfile::TempDir,
}

fn replay(document: &Json, raw: bool) -> Replay {
    let mut mapper = mapper(raw);
    let outputs = tempfile::tempdir().unwrap();
    mapper.set_image_outputs(outputs.path().to_path_buf());
    mapper.process_started(true);
    let turn = new_id();
    let mut events = mapper.begin_turn_with_model(turn, "mock-large", None);
    for frame in incoming(document) {
        events.extend(mapper.on_frame(&frame));
    }
    Replay {
        mapper,
        turn,
        events,
        outputs,
    }
}

#[test]
fn native_prefixes_retain_wire_direction_and_never_invent_a_terminal_result() {
    for capture in &CAPTURES {
        let document = document(capture);
        let incoming: Vec<_> = incoming(&document).collect();
        let sent = document["frames"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["direction"] == "to_harness")
            .count();
        assert_eq!(
            sent,
            if capture.case == "tools.result_form" {
                5
            } else {
                3
            }
        );
        assert_eq!(incoming.len() + sent, capture.frames);
        let init = incoming
            .iter()
            .find(|frame| frame.subtype() == Some("init"))
            .unwrap();
        assert_eq!(init.text("claude_code_version"), Some("2.1.319"));
        assert_eq!(init.text("model"), Some("mock-large"));
        assert_eq!(init.text("permissionMode"), Some("default"));
        assert_eq!(
            incoming
                .iter()
                .filter(|frame| frame.text("type") == Some("command_lifecycle"))
                .count(),
            2
        );
        assert!(
            incoming
                .iter()
                .all(|frame| frame.text("type") != Some("result"))
        );
        for raw in [false, true] {
            let replay = replay(&document, raw);
            assert!(
                replay.mapper.turn_active(),
                "the capture ends before native result consumption"
            );
            assert!(
                replay
                    .events
                    .iter()
                    .all(|event| !matches!(event, SessionEvent::TurnCompleted(_)))
            );
            assert_eq!(notices(&replay.events, "model_rerouted"), 0);
            let raw_count = replay
                .events
                .iter()
                .filter(|event| matches!(event, SessionEvent::RawFrame(_)))
                .count();
            assert_eq!(raw_count, if raw { incoming.len() } else { 0 });
        }
    }
}

#[test]
fn recorded_init_reports_the_actual_model_and_mode_before_or_after_turn_admission() {
    for capture in &CAPTURES {
        let document = document(capture);
        let init = incoming(&document)
            .find(|frame| frame.subtype() == Some("init"))
            .unwrap();
        for init_first in [true, false] {
            let mut mapper = mapper(false);
            mapper.process_started(true);
            let mut events = Vec::new();
            if init_first {
                events.extend(mapper.on_frame(&init));
            }
            let turn = new_id();
            events.extend(mapper.begin_turn_with_model(turn, "a-different-requested-model", None));
            if !init_first {
                events.extend(mapper.on_frame(&init));
            }
            assert_eq!(notices(&events, "model_rerouted"), 1);
            let notice = events.iter().find_map(|event| match event {
                SessionEvent::ItemStarted(item) if matches!(&item.content, ItemContent::Error(error) if error.code == "model_rerouted") => Some(item),
                _ => None,
            }).unwrap();
            assert_eq!(notice.turn_id, turn);
            let ItemContent::Error(error) = &notice.content else {
                panic!("a model comparison notice")
            };
            assert!(error.message.contains("a-different-requested-model"));
            assert!(error.message.contains("mock-large"));
            assert!(!error.retryable);
            assert!(events.iter().any(|event| matches!(event, SessionEvent::ItemCompleted(item) if item.native_ref == notice.native_ref && item.status == ItemStatus::Completed)));
            let modes: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::ItemCompleted(item)
                        if item.payload["marker"] == "harness_changed"
                            && item.payload["data"]["change"] == "approval_mode" =>
                    {
                        Some(item)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(modes.len(), 1);
            assert_eq!(
                modes[0].payload["data"]["value"],
                json!({"native_mode":"default"})
            );
            assert_eq!(
                modes[0].payload["data"]["turn_id"],
                if init_first { Json::Null } else { json!(turn) }
            );
            assert!(
                mapper.on_frame(&init).is_empty(),
                "the same native init is not published again"
            );
        }
    }
}

#[test]
fn recorded_status_and_command_lifecycle_are_safe_owned_deduplicated_observations() {
    for capture in &CAPTURES {
        let document = document(capture);
        for frame in incoming(&document).filter(|frame| {
            frame.subtype() == Some("status") || frame.text("type") == Some("command_lifecycle")
        }) {
            let mut mapper = mapper(false);
            let mut references = Vec::new();
            for turn in [new_id(), new_id()] {
                mapper.process_started(true);
                mapper.begin_turn(turn);
                let events = mapper.on_frame(&frame);
                let started: Vec<_> = events
                    .iter()
                    .filter_map(|event| match event {
                        SessionEvent::ItemStarted(item)
                            if matches!(
                                &item.content,
                                ItemContent::Marker(MarkerPayload::HarnessChanged(_))
                            ) =>
                        {
                            Some(item)
                        }
                        _ => None,
                    })
                    .collect();
                assert_eq!(started.len(), 1);
                assert_eq!(started[0].turn_id, turn);
                let ItemContent::Marker(MarkerPayload::HarnessChanged(change)) =
                    &started[0].content
                else {
                    panic!("a native observation")
                };
                if frame.subtype() == Some("status") {
                    assert_eq!(change.change, "session_status");
                    assert_eq!(change.value, json!({"type":"working","active_flags":[]}));
                } else {
                    assert_eq!(change.change, "unrecognized_event");
                    assert_eq!(change.subject.as_deref(), Some("command_lifecycle"));
                    assert_eq!(change.value, json!({"count":1}));
                }
                assert!(!change.diverged);
                assert_eq!(change.turn_id, Some(turn));
                assert!(events.iter().any(|event| matches!(event, SessionEvent::ItemCompleted(item) if item.native_ref == started[0].native_ref && item.status == ItemStatus::Completed)));
                assert!(
                    events
                        .iter()
                        .all(|event| !matches!(event, SessionEvent::TurnCompleted(_)))
                );
                references.push(started[0].native_ref.clone());
                assert!(mapper.on_frame(&frame).is_empty());
            }
            assert_ne!(
                references[0], references[1],
                "a new process generation owns its observations"
            );
        }
    }
}

#[test]
fn streamed_and_whole_native_assistant_blocks_make_one_answer_and_one_input_echo() {
    for capture in &CAPTURES {
        let document = document(capture);
        for raw in [false, true] {
            let mut replay = replay(&document, raw);
            let starts: Vec<_> = replay
                .events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::ItemStarted(item)
                        if matches!(&item.content, ItemContent::AgentMessage(_)) =>
                    {
                        Some(item)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(starts.len(), 1);
            let answer = starts[0];
            assert_eq!(answer.turn_id, replay.turn);
            let whole: Vec<_> = replay
                .events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::ItemUpdated(item)
                        if item.native_ref == answer.native_ref
                            && item.payload.get("text").is_some() =>
                    {
                        Some(item)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(whole.len(), 1);
            assert_eq!(whole[0].payload["text"], capture.answer);
            assert!(replay.events.iter().all(|event| !matches!(event, SessionEvent::ItemCompleted(item) if item.native_ref == answer.native_ref)), "the final answer is visible but awaits the missing native result");
            let mut text = String::new();
            for delta in replay.events.iter().filter_map(|event| match event {
                SessionEvent::ItemDelta(delta)
                    if delta.native_ref == answer.native_ref && delta.field == DeltaField::Text =>
                {
                    Some(delta)
                }
                _ => None,
            }) {
                assert_eq!(delta.offset, i64::try_from(text.chars().count()).unwrap());
                text.push_str(&delta.data);
            }
            assert_eq!(text, capture.answer);
            let sent = document["frames"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["direction"] == "to_harness" && row["payload"]["type"] == "user")
                .unwrap();
            assert_eq!(
                replay.mapper.take_echoed(),
                vec![sent["payload"]["uuid"].as_str().unwrap().to_owned()]
            );
            assert!(replay.mapper.take_echoed().is_empty());
        }
    }
}

#[test]
fn parallel_native_reads_keep_their_actual_inputs_outputs_and_separate_step_identities() {
    for capture in CAPTURES
        .iter()
        .filter(|capture| capture.case == "parallel_calls")
    {
        let document = document(capture);
        let mut replay = replay(&document, false);
        for (id, path, text) in [
            ("toolu_mock_000001_0", "first.txt", "the first result"),
            ("toolu_mock_000001_1", "second.txt", "the second result"),
        ] {
            let starts: Vec<_> = replay
                .events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::ItemStarted(item) if item.native_ref == id => Some(item),
                    _ => None,
                })
                .collect();
            assert_eq!(starts.len(), 1);
            assert_eq!(starts[0].turn_id, replay.turn);
            assert!(
                matches!(&starts[0].content, ItemContent::ToolCall(call) if matches!(&call.tool, ToolPayload::FileRead { .. }))
            );
            let ends: Vec<_> = replay
                .events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::ItemCompleted(item) if item.native_ref == id => Some(item),
                    _ => None,
                })
                .collect();
            assert_eq!(ends.len(), 1);
            let end = ends[0];
            assert_eq!(end.status, ItemStatus::Completed);
            assert_eq!(end.payload["tool"]["kind"], "file_read");
            assert_eq!(end.payload["tool"]["path"], path);
            assert_eq!(end.payload["tool"]["content"]["inline"], text);
            assert_eq!(end.payload["tool"]["start_line"], 1);
            assert_eq!(end.payload["tool"]["total_lines"], 1);
            assert_eq!(end.payload["raw"]["name"], "Read");
            let input: Json =
                serde_json::from_str(end.payload["raw"]["input"]["inline"].as_str().unwrap())
                    .unwrap();
            assert_eq!(
                input,
                json!({"file_path":format!("/workspace/case/{path}")})
            );
            assert_eq!(end.payload["raw"]["output"]["inline"], format!("1\t{text}"));
        }
        for frame in incoming(&document).filter(|frame| frame.get("tool_use_result").is_some()) {
            assert!(
                replay.mapper.on_frame(&frame).is_empty(),
                "repeated native tool results do not finish either step twice"
            );
        }
    }
}

#[test]
fn native_mcp_text_and_structured_result_forms_are_not_concatenated_or_repeated() {
    for capture in CAPTURES
        .iter()
        .filter(|capture| capture.case == "tools.result_form")
    {
        let document = document(capture);
        let replay = replay(&document, false);
        for (id, tool, result, wire) in [
            (
                "toolu_mock_000001_0",
                "read_shared",
                json!("Orch conformance text"),
                "Orch conformance text",
            ),
            (
                "toolu_mock_000001_1",
                "announce",
                json!({"conformance_accepted":true}),
                "{\"conformance_accepted\":true}",
            ),
        ] {
            let ends: Vec<_> = replay
                .events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::ItemCompleted(item) if item.native_ref == id => Some(item),
                    _ => None,
                })
                .collect();
            assert_eq!(ends.len(), 1);
            assert_eq!(ends[0].status, ItemStatus::Completed);
            assert_eq!(ends[0].payload["tool"]["kind"], "mcp");
            assert_eq!(ends[0].payload["tool"]["server"], "questions");
            assert_eq!(ends[0].payload["tool"]["tool"], tool);
            assert_eq!(ends[0].payload["tool"]["arguments"], json!({}));
            assert_eq!(ends[0].payload["tool"]["result"], result);
            assert_eq!(ends[0].payload["raw"]["output"]["inline"], wire);
            assert!(ends[0].payload["tool"]["error"].is_null());
        }
        assert!(
            replay
                .events
                .iter()
                .all(|event| !matches!(event, SessionEvent::ApprovalRequested(_))),
            "control replies alone are not a captured elicitation request"
        );
    }
}

#[test]
fn native_read_image_retains_the_recorded_bytes_once_under_the_original_tool() {
    for capture in CAPTURES
        .iter()
        .filter(|capture| capture.case == "image_result")
    {
        let document = document(capture);
        let result = incoming(&document)
            .find(|frame| frame.get("tool_use_result").is_some())
            .unwrap();
        let blocks = result.blocks();
        let image = &blocks[0]["content"][0]["source"];
        let encoded = image["data"].as_str().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let hash = Hash::of(&bytes);
        let mut replay = replay(&document, false);
        let images: Vec<_> = replay
            .events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::ItemCompleted(item) if item.payload["tool"]["kind"] == "image" => {
                    Some(item)
                }
                _ => None,
            })
            .collect();
        assert_eq!(images.len(), 1);
        let image = images[0];
        assert_eq!(image.status, ItemStatus::Completed);
        assert_eq!(image.output_blob.as_ref().unwrap().hash, hash);
        assert_eq!(image.payload["tool"]["media_type"], "image/png");
        assert_eq!(image.payload["tool"]["blob"], hash.to_string());
        assert!(!image.payload.to_string().contains(encoded));
        let started = replay
            .events
            .iter()
            .find_map(|event| match event {
                SessionEvent::ItemStarted(item) if item.native_ref == image.native_ref => {
                    Some(item)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(started.turn_id, replay.turn);
        assert_eq!(
            started.parent_native_ref.as_deref(),
            Some("toolu_mock_000001_0")
        );
        assert_eq!(
            std::fs::read(replay.outputs.path().join(hash.to_string())).unwrap(),
            bytes
        );
        assert!(replay.mapper.on_frame(&result).is_empty());
        assert_eq!(std::fs::read_dir(replay.outputs.path()).unwrap().count(), 1);
    }
}
