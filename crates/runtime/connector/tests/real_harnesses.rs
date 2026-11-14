//! The two real harnesses, hosted by the connector: one turn of Codex and one of Claude Code, each started by the connector from the adapter the program carries, run in a container of the development image, its model call going through the forwarder to the scripted model server with a key that only the connector holds, its events arriving at the scripted controller in order. The tests say so and pass where Docker or the image is not on the machine.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::docker::{self, ClaudeDocker, CodexDocker, Containers};
use common::model::{MESSAGES, MODEL, Options, ProviderRig, RESPONSES, files_holding};
use orch_adapter_claude_code::ClaudeCodeAdapter;
use orch_adapter_codex::CodexAdapter;
use orch_connector_core::harnesses::{AdapterRegistration, AdapterRegistry, HomeRule};
use orch_testkit::doubles::model::{Presentation, Verdict};
use serde_json::json;

#[derive(Clone, Copy)]
enum Harness {
    Codex,
    Claude,
}

impl Harness {
    fn label(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    fn kind(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude-code",
        }
    }

    fn protocol(self) -> &'static str {
        match self {
            Self::Codex => RESPONSES,
            Self::Claude => MESSAGES,
        }
    }

    /// The variable the harness is given the token of the connector's listener in.
    fn token_variable(self) -> &'static str {
        match self {
            Self::Codex => "ORCH_LLM_TOKEN",
            Self::Claude => "ANTHROPIC_AUTH_TOKEN",
        }
    }
}

/// Session observations have no owned turn; a tool or turn-scoped marker is never an idle observation.
fn idle_observation(
    kind: &str,
    body: &serde_json::Value,
    markers: &mut std::collections::BTreeSet<String>,
) -> bool {
    use orch_agent_contract::events::{ErrorPayload, MarkerPayload};
    if kind != "item.completed" || body["status"] != "completed" {
        return false;
    }
    let Some(reference) = body["native_ref"]
        .as_str()
        .filter(|reference| !reference.is_empty())
    else {
        return false;
    };
    if let Ok(MarkerPayload::HarnessChanged(data)) =
        serde_json::from_value::<MarkerPayload>(body["payload"].clone())
    {
        if data.turn_id.is_none() {
            markers.insert(reference.into());
            return true;
        }
        return false;
    }
    let Ok(error) = serde_json::from_value::<ErrorPayload>(body["payload"].clone()) else {
        return false;
    };
    matches!(
        error.code.as_str(),
        "harness_diverged" | "mcp_server_failed"
    ) && error
        .details
        .as_ref()
        .and_then(|details| details.get("turn_id"))
        .is_none_or(serde_json::Value::is_null)
        && markers.iter().any(|marker| {
            reference == format!("{marker}-notice") || reference == format!("{marker}:notice")
        })
}

/// A completed turn closes its owned items; usage/context refinements and idle observations may follow.
fn turn_order<'a>(
    events: impl IntoIterator<Item = (&'a str, &'a serde_json::Value)>,
) -> Result<(), String> {
    let events: Vec<_> = events.into_iter().collect();
    turn_events_ordered(&events)
        .map_err(|error| format!("{error}; after turn.completed: {}", after_turn(&events)))
}

/// Item completions and deltas inherit their owner from their actual preceding start.
fn after_turn(events: &[(&str, &serde_json::Value)]) -> String {
    let Some(end) = events
        .iter()
        .position(|(kind, _)| *kind == "turn.completed")
    else {
        return "none (turn.completed was not observed)".into();
    };
    let mut owners = std::collections::BTreeMap::new();
    let mut suffix = Vec::new();
    for (at, (kind, body)) in events.iter().enumerate() {
        let reference = body["native_ref"]
            .as_str()
            .filter(|value| !value.is_empty());
        if *kind == "item.started"
            && let (Some(reference), Some(turn)) = (reference, body["turn_id"].as_str())
        {
            owners.insert(reference, turn);
        }
        if at <= end {
            continue;
        }
        let turn = body["turn_id"]
            .as_str()
            .or_else(|| reference.and_then(|reference| owners.get(reference).copied()))
            .or_else(|| {
                let payload = &body["payload"];
                if payload["marker"] == "harness_changed" {
                    payload
                        .pointer("/data/turn_id")
                        .and_then(serde_json::Value::as_str)
                } else if payload["code"].is_string() {
                    payload
                        .pointer("/details/turn_id")
                        .and_then(serde_json::Value::as_str)
                } else {
                    None
                }
            })
            .unwrap_or("none");
        suffix.push(format!(
            "{kind} turn_id={turn} native_ref={}",
            reference.unwrap_or("none")
        ));
    }
    if suffix.is_empty() {
        "none".into()
    } else {
        suffix.join("; ")
    }
}

fn turn_events_ordered(events: &[(&str, &serde_json::Value)]) -> Result<(), String> {
    let positions = |kind| {
        events
            .iter()
            .enumerate()
            .filter_map(|(at, (found, _))| (*found == kind).then_some(at))
            .collect::<Vec<_>>()
    };
    let starts = positions("turn.started");
    let ends = positions("turn.completed");
    if starts.len() != 1 || ends.len() != 1 {
        return Err(format!(
            "expected one turn start and completion: {starts:?} / {ends:?}"
        ));
    }
    let (start, end) = (starts[0], ends[0]);
    let turn = &events[start].1["turn_id"];
    if start >= end || !turn.is_string() || events[end].1["turn_id"] != *turn {
        return Err("the actual turn must start before its matching completion".into());
    }
    let mut owned = std::collections::BTreeSet::new();
    let mut open = std::collections::BTreeSet::new();
    let mut idle_markers = std::collections::BTreeSet::new();
    for (at, (kind, body)) in events.iter().enumerate() {
        // Claude asks for context after publishing completion; that reply refines this turn's usage.
        if at > end && *kind == "usage.updated" && body["turn_id"] == *turn {
            continue;
        }
        if let Some(owner) = body.get("turn_id").filter(|owner| !owner.is_null())
            && (owner != turn || at < start || at > end)
        {
            return Err(format!("turn-owned event outside its turn: {kind} {body}"));
        }
        let reference = body["native_ref"].as_str().unwrap_or_default();
        if *kind == "item.started" {
            if body["turn_id"] != *turn || at <= start || at >= end || reference.is_empty() {
                return Err(format!("item started outside its actual turn: {body}"));
            }
            // ItemStarted is idempotent for an existing native reference.
            if owned.insert(reference.to_owned()) {
                open.insert(reference.to_owned());
            }
        } else if kind.starts_with("item.") && owned.contains(reference) {
            if at >= end {
                return Err(format!(
                    "owned item followed turn completion: {kind} {body}"
                ));
            }
            if *kind == "item.completed" {
                open.remove(reference);
            }
        } else {
            let idle = idle_observation(kind, body, &mut idle_markers);
            if ((at < start && kind.starts_with("item.")) || at > end) && !idle {
                return Err(format!(
                    "only idle session observations may surround the turn: {kind} {body}"
                ));
            }
        }
        if at == end && !open.is_empty() {
            return Err(format!(
                "turn completed with owned items still open: {open:?}"
            ));
        }
    }
    if owned.is_empty() {
        return Err("the native turn produced no owned item".into());
    }
    Ok(())
}

/// The adapter of the harness, started through a launcher that runs it in a container.
fn registry(
    harness: Harness,
    containers: &Arc<Containers>,
    mounts: Vec<std::path::PathBuf>,
) -> AdapterRegistry {
    let mut registry = AdapterRegistry::new();
    let containers = Arc::clone(containers);
    let registration = match harness {
        Harness::Codex => AdapterRegistration {
            kind: orch_adapter_codex::descriptor::KIND,
            adapter_version: orch_adapter_codex::descriptor::ADAPTER_VERSION,
            builtin: true,
            needs_binary: true,
            home_rule: HomeRule::Harness,
            factory: Arc::new(move |clock| {
                Box::new(
                    CodexAdapter::with_launcher(Arc::new(CodexDocker::new(
                        Arc::clone(&containers),
                        mounts.clone(),
                    )))
                    .with_clock(clock),
                )
            }),
        },
        Harness::Claude => AdapterRegistration {
            kind: orch_adapter_claude_code::config::KIND,
            adapter_version: orch_adapter_claude_code::config::ADAPTER_VERSION,
            builtin: true,
            needs_binary: true,
            home_rule: HomeRule::Harness,
            factory: Arc::new(move |clock| {
                Box::new(ClaudeCodeAdapter::with_launcher(
                    clock,
                    Arc::new(ClaudeDocker::new(Arc::clone(&containers), mounts.clone())),
                ))
            }),
        },
    };
    registry.register(registration).unwrap();
    registry
}

/// One turn of the harness, hosted by a connector the system made (it has no policy of its own, and names the
/// harness's home), against the scripted model server that wants a key the harness never has.
async fn hosted_turn(harness: Harness) {
    let test = format!("{}'s turn through the forwarder", harness.label());
    if !docker::available(&test) {
        return;
    }
    let containers = Containers::new(harness.label());
    let for_registry = Arc::clone(&containers);
    let options = Options {
        managed: true,
        patience: Some(Duration::from_secs(240)),
        adapters: Some(Box::new(move |data, work| {
            registry(
                harness,
                &for_registry,
                vec![data.to_owned(), work.to_owned()],
            )
        })),
        ..Options::default()
    };
    let p = ProviderRig::build(options, |_, _| ()).await;
    p.start_both_providers().await;

    // The harness process, with the home the system names for it under the data directory.
    let home = format!("harness/{}/{}", p.rig.provider, p.rig.agent);
    let start: orch_link_contract::commands::AgentStart = serde_json::from_value(json!({
        "provider_id": p.rig.provider, "agent_id": p.rig.agent, "harness_kind": harness.kind(),
        "provider_kind": "api",
        "profile": { "harness_version": null, "config": {}, "env": {} }, "home": home
    }))
    .unwrap();
    let started = p.rig.succeed(&p.rig.harness_key(), &start).await;
    assert_eq!(started["descriptor"]["kind"], harness.kind());
    println!(
        "{} {} started",
        harness.kind(),
        started["harness_version"].as_str().unwrap_or("?")
    );
    assert!(
        p.rig.data.path().join(&home).is_dir(),
        "the home of the harness is under the data directory"
    );

    // The session on the forwarded path, the lease of the turn, and the turn.
    p.open_forwarded_session_in(harness.protocol(), MODEL, "bypass", "open")
        .await;
    let lease = p
        .open_lease(p.thread, &[MODEL], Some(harness.protocol()), None)
        .await;
    p.start_turn_in("hello", MODEL, "bypass", "open").await;

    // While it runs, what the container was given is looked at: the token of the listener is there, the
    // key is not.
    let mut inspected = String::new();
    for _ in 0..40 {
        inspected = containers
            .names()
            .iter()
            .filter_map(|name| containers.inspect(name))
            .collect::<Vec<_>>()
            .join("\n");
        if inspected.contains(harness.token_variable()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        inspected.contains(harness.token_variable()),
        "the harness is given the token of the listener in its environment: {inspected}"
    );
    p.wait_for_turn_event("turn.completed", 1).await;

    // Events arrived in order, and the controller saw no gap or repeat.
    let events = p.rig.applied(p.thread, p.segment);
    let kinds: Vec<_> = events.iter().map(|event| event.kind.clone()).collect();
    assert_eq!(
        kinds.first().map(String::as_str),
        Some("session.opened"),
        "{kinds:?}; after turn.completed: {}",
        after_turn(
            &events
                .iter()
                .map(|event| (event.kind.as_str(), &event.body))
                .collect::<Vec<_>>()
        )
    );
    let order = turn_order(
        events
            .iter()
            .map(|event| (event.kind.as_str(), &event.body)),
    );
    assert!(order.is_ok(), "{order:?}; events: {kinds:?}");
    let started_at = events
        .iter()
        .position(|event| event.kind == "turn.started")
        .unwrap();
    let mut idle_markers = std::collections::BTreeSet::new();
    for event in events[..started_at]
        .iter()
        .filter(|event| event.kind.starts_with("item."))
    {
        assert!(
            idle_observation(&event.kind, &event.body, &mut idle_markers),
            "only idle session observations may precede the turn: {} {:?}",
            event.kind,
            event.body
        );
    }
    let turn = &events[started_at].body["turn_id"];
    assert!(turn.is_string(), "the start identifies its actual turn");
    let first_item = events
        .iter()
        .position(|event| event.kind == "item.started" && event.body["turn_id"] == *turn)
        .unwrap_or_else(|| {
            panic!(
                "the native turn produced no owned item: {}",
                p.turn_diagnostics()
            )
        });
    assert!(
        started_at < first_item,
        "the turn starts before its owned items: {kinds:?}"
    );
    assert!(
        p.rig.controller.violations().is_empty(),
        "{:?}",
        p.rig.controller.violations()
    );
    let said = p.rig.messages(p.thread, p.segment);
    assert!(
        said.iter()
            .any(|text| text.contains("You said:") && text.contains("hello")),
        "the model's answer is the turn's: {said:?}"
    );
    let completed = p
        .rig
        .applied(p.thread, p.segment)
        .into_iter()
        .rev()
        .find(|event| event.kind == "turn.completed")
        .unwrap();
    assert_eq!(
        completed.body["status"], "completed",
        "{:?}",
        completed.body
    );

    // The model was asked through the forwarder, with the key, and with nothing else.
    let requests = p.model_requests();
    assert!(!requests.is_empty(), "the harness made a model call");
    for request in &requests {
        assert_eq!(request.credential.verdict, Verdict::Accepted, "{request:?}");
        assert_eq!(request.credential.presented_by, [Presentation::Bearer]);
    }
    // Each request publishes usage independently after its response ends. The
    // first report is not a barrier for the other requests of a native turn.
    let usage = p.wait_provider_event("lease.usage", requests.len()).await;
    assert_eq!(usage.len(), requests.len(), "one report for each request");
    let references: std::collections::BTreeSet<_> = usage
        .iter()
        .map(|event| event.body["request_ref"].as_str().unwrap())
        .collect();
    assert_eq!(
        references.len(),
        requests.len(),
        "each report names a distinct request"
    );
    let closed = p.close_lease(lease).await;
    assert_eq!(
        closed["requests"].as_u64().unwrap() as usize,
        requests.len()
    );

    // The harness never had the key: not in what any container was started with, nor in the environment
    // or the command line the container ran under, nor on any disk the harness could read.
    for run in containers.recorded() {
        assert!(
            !run.argv.join(" ").contains(&p.key),
            "the key is on a command line"
        );
        assert!(
            run.env.values().all(|value| !value.contains(&p.key)),
            "the key is in an environment"
        );
        assert!(
            run.env.contains_key(harness.token_variable()) || !run.env.is_empty(),
            "the harness was given its environment: {:?}",
            run.env.keys().collect::<Vec<_>>()
        );
    }
    assert!(!inspected.contains(&p.key));
    assert!(files_holding(p.rig.data.path(), &p.key).is_empty());
    assert!(files_holding(p.work.path(), &p.key).is_empty());

    // Include observations delivered while provider accounting and credential checks completed.
    let events = p.rig.applied(p.thread, p.segment);
    let order = turn_order(
        events
            .iter()
            .map(|event| (event.kind.as_str(), &event.body)),
    );
    assert!(order.is_ok(), "{order:?}");
    containers.remove_all();
}

#[tokio::test]
async fn one_turn_of_codex_hosted_by_the_connector_goes_through_the_forwarder_with_a_key_the_harness_never_sees()
 {
    hosted_turn(Harness::Codex).await;
}

#[tokio::test]
async fn one_turn_of_claude_code_hosted_by_the_connector_goes_through_the_forwarder_with_a_key_the_harness_never_sees()
 {
    hosted_turn(Harness::Claude).await;
}

fn observation(reference: &str, turn: Option<orch_types::Id>) -> serde_json::Value {
    use orch_agent_contract::events::{MarkerHarnessChanged, MarkerPayload};
    use orch_api_contract::usage::UsageMeasurement;
    let data = MarkerHarnessChanged {
        program: "Fixture".into(),
        change: "session_status".into(),
        subject: None,
        previous: UsageMeasurement::Unsupported,
        value: json!("opened"),
        expected: UsageMeasurement::Unsupported,
        changed: false,
        diverged: false,
        next_turn: false,
        turn_id: turn,
        reported_at: serde_json::from_value(json!("2026-01-01T00:00:00Z")).unwrap(),
    };
    let payload = serde_json::to_value(MarkerPayload::HarnessChanged(Box::new(data))).unwrap();
    json!({"native_ref":reference,"status":"completed","payload":payload})
}

#[test]
fn opening_observations_allow_no_tool_or_turn_owned_item_before_the_turn() {
    let marker = observation("opening", None);
    let mut known = std::collections::BTreeSet::new();
    assert!(idle_observation("item.completed", &marker, &mut known));
    let notice = json!({"native_ref":"opening-notice","status":"completed","payload":{
        "code":"harness_diverged","message":"Fixture observation","retryable":false}});
    assert!(idle_observation("item.completed", &notice, &mut known));
    let owned = observation("turn-owned", Some(orch_types::new_id()));
    assert!(!idle_observation("item.completed", &owned, &mut known));
    assert!(!idle_observation("item.started", &marker, &mut known));
    assert!(!idle_observation(
        "item.completed",
        &json!({"native_ref":"tool","status":"completed","payload":{"tool":{"kind":"command"}}}),
        &mut known
    ));
    let mut turn_notice = notice.clone();
    turn_notice["payload"]["details"] = json!({"turn_id":orch_types::new_id()});
    assert!(!idle_observation(
        "item.completed",
        &turn_notice,
        &mut known
    ));
    let mut unlinked = notice;
    unlinked["native_ref"] = json!("unlinked-notice");
    assert!(!idle_observation("item.completed", &unlinked, &mut known));
}

#[test]
fn completed_turn_order_allows_idle_observations_and_refuses_late_or_unclosed_owned_work() {
    let turn = orch_types::new_id();
    let events = vec![
        ("session.opened", json!({"native_session_ref":"session"})),
        ("item.completed", observation("opening", None)),
        (
            "turn.started",
            json!({"turn_id":turn,"native_turn_ref":"native-turn"}),
        ),
        (
            "item.started",
            json!({"turn_id":turn,"native_ref":"answer","stream":"main","kind":"agent_message","text":""}),
        ),
        (
            "item.delta",
            json!({"native_ref":"answer","field":"text","offset":0,"len":4,"data":"done"}),
        ),
        (
            "item.completed",
            json!({"native_ref":"answer","status":"completed","payload":{"text":"done","text_len":4}}),
        ),
        (
            "turn.completed",
            json!({"turn_id":turn,"status":"completed"}),
        ),
        ("item.completed", observation("idle-after", None)),
    ];
    let check = |events: &[(&str, serde_json::Value)]| {
        turn_order(events.iter().map(|(kind, body)| (*kind, body)))
    };
    assert!(check(&events).is_ok());
    let mut idle_during = events.clone();
    idle_during.insert(5, ("item.completed", observation("idle-during", None)));
    assert!(check(&idle_during).is_ok());
    let mut replay = events.clone();
    replay.insert(6, events[3].clone());
    replay.insert(7, events[5].clone());
    assert!(
        check(&replay).is_ok(),
        "existing item references remain idempotent"
    );
    for at in [7, 8] {
        let mut late_usage = events.clone();
        late_usage.insert(at, ("usage.updated", json!({"turn_id":turn})));
        assert!(
            check(&late_usage).is_ok(),
            "the completed turn may refine its usage on either side of an idle observation"
        );
    }
    for owner in [json!(orch_types::new_id()), serde_json::Value::Null] {
        let mut wrong_usage = events.clone();
        wrong_usage.push(("usage.updated", json!({"turn_id":owner})));
        assert!(
            check(&wrong_usage).is_err(),
            "late usage must name only the completed turn"
        );
    }
    let mut early_usage = events.clone();
    early_usage.insert(2, ("usage.updated", json!({"turn_id":turn})));
    assert!(
        check(&early_usage).is_err(),
        "usage cannot precede its turn"
    );

    for late in [
        (
            "item.started",
            json!({"turn_id":turn,"native_ref":"late","stream":"main","kind":"agent_message","text":""}),
        ),
        (
            "item.completed",
            json!({"native_ref":"answer","status":"completed","payload":{"text":"late","text_len":4}}),
        ),
        (
            "item.delta",
            json!({"native_ref":"answer","field":"text","offset":4,"len":1,"data":"!"}),
        ),
        (
            "item.updated",
            json!({"native_ref":"answer","payload":{"text":"late"}}),
        ),
        (
            "item.completed",
            json!({"native_ref":"unstarted-tool","status":"completed","payload":{"tool":{"kind":"command"}}}),
        ),
        ("item.completed", observation("owned-marker", Some(turn))),
        ("item.completed", observation("answer", None)),
        ("model.request", json!({"turn_id":turn})),
    ] {
        let mut changed = events.clone();
        changed.push(late.clone());
        assert!(check(&changed).is_err(), "late work was accepted: {late:?}");
    }
    let mut unclosed = events.clone();
    let completed = unclosed.remove(5);
    assert!(
        check(&unclosed).is_err(),
        "an open item cannot survive its turn"
    );
    unclosed.push(completed);
    assert!(
        check(&unclosed).is_err(),
        "closing the item after its turn is still too late"
    );
    let mut diagnostic = events;
    diagnostic.push((
        "item.completed",
        json!({"native_ref":"answer","status":"completed","payload":{"text":"late"}}),
    ));
    diagnostic.push(("usage.updated", json!({"turn_id":turn})));
    diagnostic.push(("item.completed", observation("last-idle", None)));
    let error = check(&diagnostic).unwrap_err();
    for expected in [
        "item.completed turn_id=none native_ref=idle-after".to_owned(),
        format!("item.completed turn_id={turn} native_ref=answer"),
        format!("usage.updated turn_id={turn} native_ref=none"),
        "item.completed turn_id=none native_ref=last-idle".to_owned(),
    ] {
        assert!(error.contains(&expected), "missing {expected:?}: {error}");
    }
}
