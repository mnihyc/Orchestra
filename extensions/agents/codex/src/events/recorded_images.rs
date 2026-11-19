//! Captured native turns are replayed without a binary, account home, network or model call.
use super::*;
use base64::Engine as _;
use sha2::{Digest as _, Sha256};

const ALIAS: &str = "@fixture/generated.png";
#[derive(serde::Deserialize)]
struct Row {
    direction: String,
    #[serde(default)]
    frame: Option<Json>,
    #[serde(default)]
    redacted: Option<String>,
}
struct Captured {
    rows: Vec<Row>,
    png: Vec<u8>,
}
fn captured(file: &str) -> Result<Captured, Box<dyn std::error::Error>> {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/image-generation/0.167.0");
    let sums = std::fs::read_to_string(root.join("SHA256SUMS"))?;
    for name in [
        "success.frames.jsonl",
        "signin-refusal.frames.jsonl",
        "generated.png",
    ] {
        let expected = sums
            .lines()
            .find_map(|line| {
                let (hash, file) = line.split_once("  ")?;
                (file == name).then_some(hash)
            })
            .ok_or_else(|| format!("the captured manifest does not name {name}"))?;
        let data = std::fs::read(root.join(name))?;
        assert_eq!(
            Sha256::digest(&data)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            expected,
            "the recorded artifact changed: {name}"
        );
    }
    let rows = std::fs::read_to_string(root.join(file))?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<Row>, _>>()?;
    Ok(Captured {
        rows,
        png: std::fs::read(root.join("generated.png"))?,
    })
}
fn materialize(value: &mut Json, image: &Path, png: &[u8]) {
    if let Some(object) = value.as_object_mut() {
        if object.get("type").and_then(Json::as_str) == Some("imageGeneration") {
            if object.get("savedPath").and_then(Json::as_str) == Some(ALIAS) {
                object.insert("savedPath".into(), json!(image.to_string_lossy()));
            }
            if object.get("result").and_then(Json::as_str) == Some(ALIAS) {
                object.insert(
                    "result".into(),
                    json!(base64::engine::general_purpose::STANDARD.encode(png)),
                );
            }
        }
        for value in object.values_mut() {
            materialize(value, image, png);
        }
    } else if let Some(array) = value.as_array_mut() {
        for value in array {
            materialize(value, image, png);
        }
    }
}
struct Replayed {
    events: Vec<SessionEvent>,
    history: Vec<Json>,
    config: MapperConfig,
    turn: String,
    caption: Option<String>,
}
fn replay(
    captured: &Captured,
    outputs: &OutputStore,
    clock: &dyn Clock,
    image: &Path,
) -> Result<Replayed, Box<dyn std::error::Error>> {
    let mut requests = BTreeMap::<String, (String, Json)>::new();
    let mut thread = None;
    let mut requested = None;
    let mut reported = None;
    let mut version = None;
    for row in &captured.rows {
        if let Some(frame) = &row.frame {
            if row.direction == "to_harness" {
                if let (Some(id), Some(method)) =
                    (frame.get("id"), frame.get("method").and_then(Json::as_str))
                {
                    requests.insert(
                        id.to_string(),
                        (
                            method.into(),
                            frame.get("params").cloned().unwrap_or(Json::Null),
                        ),
                    );
                }
            } else if let Some((method, params)) =
                frame.get("id").and_then(|id| requests.get(&id.to_string()))
            {
                if method == "initialize" {
                    version = frame
                        .pointer("/result/userAgent")
                        .and_then(Json::as_str)
                        .and_then(crate::descriptor::version_from_user_agent);
                }
                if method == "thread/start" {
                    thread = frame
                        .pointer("/result/thread/id")
                        .and_then(Json::as_str)
                        .map(str::to_owned);
                    requested = params
                        .get("model")
                        .and_then(Json::as_str)
                        .map(str::to_owned);
                    reported = frame
                        .pointer("/result/model")
                        .and_then(Json::as_str)
                        .map(str::to_owned);
                }
            }
        } else {
            assert_eq!(
                row.redacted.as_deref(),
                Some("account_usage"),
                "an omitted row must say what was redacted"
            );
        }
    }
    assert_eq!(
        version.as_deref(),
        Some("0.167.0"),
        "captured support belongs to its exact native build"
    );
    let config = MapperConfig {
        output_capture_bytes: 67_108_864,
        thread_id: Id::from_u128(1),
        segment_id: Id::from_u128(2),
        native_thread: thread.ok_or("the native thread opening is missing")?,
        directory: "/work".into(),
    };
    let requested = requested.or_else(|| reported.clone()).unwrap_or_default();
    let mut mapper = Mapper::new(config.clone())
        .with_native_web_results(true)
        .with_opened_model(&requested, reported.as_deref());
    let mut events = Vec::new();
    let mut history = Vec::new();
    let mut turn = String::new();
    let mut caption = None;
    let ctx = Context {
        clock,
        outputs,
        windows: None,
    };
    for row in &captured.rows {
        let Some(frame) = &row.frame else { continue };
        if row.direction != "from_harness" {
            continue;
        }
        let Some(method) = frame.get("method").and_then(Json::as_str) else {
            continue;
        };
        let mut params = frame.get("params").cloned().unwrap_or(Json::Null);
        materialize(&mut params, image, &captured.png);
        if method == "turn/started"
            && params.get("threadId").and_then(Json::as_str) == Some(config.native_thread.as_str())
        {
            mapper.expect_turn_model(Id::from_u128(9), &requested);
            turn = params
                .pointer("/turn/id")
                .and_then(Json::as_str)
                .ok_or("the native turn identity is absent")?
                .into();
        }
        if method == "item/completed"
            && params.get("threadId").and_then(Json::as_str) == Some(config.native_thread.as_str())
            && let Some(item) = params.get("item")
        {
            if item.get("type").and_then(Json::as_str) == Some("imageGeneration") {
                caption = item
                    .get("revisedPrompt")
                    .and_then(Json::as_str)
                    .map(str::to_owned);
            }
            history.push(item.clone());
        }
        events.extend(mapper.handle(method, &params, &ctx));
    }
    Ok(Replayed {
        events,
        history,
        config,
        turn,
        caption,
    })
}
fn images(events: &[SessionEvent]) -> Vec<&ItemCompleted> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(item)
                if item
                    .payload
                    .pointer("/tool/capability")
                    .and_then(Json::as_str)
                    == Some("image_generation") =>
            {
                Some(item)
            }
            _ => None,
        })
        .collect()
}
#[test]
fn recorded_codex_image_turn_retains_exact_pixels_caption_and_one_proven_generation()
-> Result<(), Box<dyn std::error::Error>> {
    let captured = captured("success.frames.jsonl")?;
    let owned = tempfile::tempdir()?;
    let image = owned.path().join("generated.png");
    std::fs::write(&image, &captured.png)?;
    let outputs = OutputStore::new(owned.path().join("outputs"));
    let clock = orch_types::ManualClock::new(orch_testkit::clock::start());
    let replayed = replay(&captured, &outputs, &clock, &image)?;
    let generated = images(&replayed.events);
    assert_eq!(
        generated.len(),
        1,
        "the actual native completion must become one generated image"
    );
    assert_eq!(generated[0].status, ItemStatus::Completed);
    assert_eq!(
        generated[0].payload["tool"]["blob"],
        json!(Hash::of(&captured.png))
    );
    assert_eq!(
        generated[0].payload["tool"]["caption"],
        json!(replayed.caption)
    );
    assert!(replayed.events.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
    let count = replayed
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageUpdated(usage) => usage.report.as_ref(),
            _ => None,
        })
        .flat_map(|report| &report.models)
        .filter_map(|model| model.counts.get("image_generation"))
        .copied()
        .max();
    assert_eq!(count, Some(1));
    let mut history = Replay::new(replayed.config).with_native_web_results();
    let ctx = Context {
        clock: &clock,
        outputs: &outputs,
        windows: None,
    };
    let read = history.page(
        &[json!({"id":replayed.turn,"status":"completed","items":replayed.history})],
        &ctx,
        |_| Id::from_u128(9),
    );
    assert_eq!(images(&read).len(), 1);
    assert!(!read.iter().any(|event|matches!(event,SessionEvent::UsageUpdated(usage) if usage.report.as_ref().is_some_and(|report|report.models.iter().any(|model|model.counts.contains_key("image_generation"))))),"history reads never charge another native generation");
    assert_eq!(std::fs::read(&image)?, captured.png);
    Ok(())
}
#[test]
fn recorded_empty_sign_in_refuses_generation_and_keeps_the_real_unauthorized_cause()
-> Result<(), Box<dyn std::error::Error>> {
    let captured = captured("signin-refusal.frames.jsonl")?;
    let owned = tempfile::tempdir()?;
    let image = owned.path().join("unused.png");
    let outputs = OutputStore::new(owned.path().join("outputs"));
    let clock = orch_types::ManualClock::new(orch_testkit::clock::start());
    let replayed = replay(&captured, &outputs, &clock, &image)?;
    assert!(images(&replayed.events).is_empty());
    assert!(replayed.events.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Failed && end.error.as_ref().is_some_and(|error|error.code==turn_error::PROVIDER_UNAUTHORIZED))),"the recorded 401 is an authentication refusal, not a successful image or a guessed plan restriction");
    assert!(!image.exists());
    Ok(())
}
