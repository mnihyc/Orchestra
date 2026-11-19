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
    p.rig
        .wait_for(p.thread, p.segment, "turn.completed", 1)
        .await;

    // Events arrived in order, and the controller saw no gap or repeat.
    let kinds = p.rig.kinds(p.thread, p.segment);
    assert_eq!(
        kinds.first().map(String::as_str),
        Some("session.opened"),
        "{kinds:?}"
    );
    assert_eq!(
        kinds.last().map(String::as_str),
        Some("turn.completed"),
        "{kinds:?}"
    );
    let first_item = kinds
        .iter()
        .position(|kind| kind.starts_with("item."))
        .unwrap();
    let started_at = kinds
        .iter()
        .position(|kind| kind == "turn.started")
        .unwrap();
    assert!(
        started_at < first_item,
        "the turn starts before its items: {kinds:?}"
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
    let usage = p.wait_provider_event("lease.usage", 1).await;
    assert_eq!(usage.len(), requests.len(), "one report for each request");
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
