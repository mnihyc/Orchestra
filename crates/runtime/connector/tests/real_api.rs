//! The forwarder against a real model service, made only when `ORCH_REAL_API_CHECK=1` and the key is in `OPENROUTER_API_KEY` of the one command that runs the test: a few small requests, through TLS to a public host, in both formats; the usage of a real stream read as it passes; the models of a real gateway listed; and, where Docker has the image, one turn of the real Codex whose model call goes through the connector's forwarder to the real service with a key the harness never has. The key is read from the environment of the test process, sealed to the connector as the system would send it, and is never printed or written.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::docker::{self, CodexDocker, Containers};
use common::model::{MESSAGES, Options, ProviderRig, RESPONSES, files_holding};
use orch_adapter_codex::CodexAdapter;
use orch_connector_core::harnesses::{AdapterRegistration, AdapterRegistry, HomeRule};
use orch_link::commands::Progress;
use orch_link_contract::commands::ProviderDiscoverModels;
use serde_json::json;

/// The cheapest current model of each family (the facts file of this machine).
const OPENAI_MODEL: &str = "openai/gpt-5.6-luna";
const ANTHROPIC_MODEL: &str = "anthropic/claude-haiku-5.5";
/// The service's two bases: the Responses format below `/api/v1`, the Messages format below `/api`.
const RESPONSES_BASE: &str = "https://openrouter.ai/api/v1";
const MESSAGES_BASE: &str = "https://openrouter.ai/api";

/// The key, if this check is to be made.
fn real_key(test: &str) -> Option<String> {
    if std::env::var("ORCH_REAL_API_CHECK").as_deref() != Ok("1") {
        eprintln!(
            "SKIPPED {test}: set ORCH_REAL_API_CHECK=1 (and OPENROUTER_API_KEY) to make the real requests"
        );
        return None;
    }
    let key = std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|key| !key.is_empty());
    if key.is_none() {
        eprintln!("SKIPPED {test}: OPENROUTER_API_KEY is not in the environment");
    }
    key
}

fn managed() -> Options {
    Options {
        managed: true,
        patience: Some(Duration::from_secs(240)),
        ..Options::default()
    }
}

#[tokio::test]
async fn both_formats_are_forwarded_to_a_real_service_over_tls_and_what_they_used_is_read_from_the_real_streams()
 {
    let Some(key) = real_key("forwarding to a real service") else {
        return;
    };
    let p = ProviderRig::build(managed(), |_, _| ()).await;
    let progress = p
        .start_provider_with_key(
            &[
                (RESPONSES, RESPONSES_BASE.to_owned(), json!({})),
                (MESSAGES, MESSAGES_BASE.to_owned(), json!({})),
            ],
            Some(&key),
        )
        .await;
    assert!(matches!(progress, Progress::Succeeded(_)), "{progress:?}");
    let lease = p
        .open_lease(p.thread, &[OPENAI_MODEL, ANTHROPIC_MODEL], None, None)
        .await;
    let harness = p.harness(p.thread);

    // The Responses format, as a stream.
    let reply = harness
        .post(
            "/v1/responses",
            &json!({ "model": OPENAI_MODEL, "input": "Reply with the single word: pong",
                     "max_output_tokens": 32, "stream": true }),
        )
        .await;
    assert_eq!(
        reply.status,
        200,
        "{}",
        reply.text().chars().take(300).collect::<String>()
    );
    assert!(
        reply.text().contains("response.completed"),
        "the stream reached its end"
    );

    // The Messages format, as a stream.
    let reply = harness
        .client()
        .send(
            harness
                .request(
                    "POST",
                    "/v1/messages",
                    Some(&json!({ "model": ANTHROPIC_MODEL, "max_tokens": 32, "stream": true,
                                  "messages": [ { "role": "user", "content": "Reply with the single word: pong" } ] })),
                )
                .messages_version(),
        )
        .await
        .unwrap();
    assert_eq!(
        reply.status,
        200,
        "{}",
        reply.text().chars().take(300).collect::<String>()
    );
    assert!(
        reply.text().contains("message_stop"),
        "the stream reached its end"
    );

    // A model the lease does not name never leaves the machine.
    let outside = harness
        .post(
            "/v1/responses",
            &json!({ "model": "openai/gpt-5.6-sol", "input": "hi" }),
        )
        .await;
    assert_eq!(outside.status, 403);

    let usage = p.wait_provider_event("lease.usage", 2).await;
    for event in &usage {
        let tokens = &event.body["usage"];
        assert!(tokens["input_tokens"].as_i64().unwrap() > 0, "{tokens}");
        assert!(tokens["output_tokens"].as_i64().unwrap() > 0, "{tokens}");
        assert_eq!(
            tokens["total_tokens"].as_i64().unwrap(),
            tokens["input_tokens"].as_i64().unwrap() + tokens["output_tokens"].as_i64().unwrap()
        );
        assert!(
            event.body["request_ref"].is_string(),
            "the service's own name for the request"
        );
    }
    let closed = p.close_lease(lease).await;
    assert_eq!(closed["requests"], 2);

    // The models of the real gateway, with the prices it lists.
    let listed = p
        .rig
        .succeed(
            &p.provider_key(),
            &ProviderDiscoverModels {
                provider_id: p.rig.provider,
                models: None,
            },
        )
        .await;
    let suggestions = listed["suggestions"].as_array().unwrap();
    let luna = suggestions
        .iter()
        .find(|suggestion| suggestion["model_name"] == OPENAI_MODEL)
        .expect("the gateway lists the cheap model");
    assert!(
        luna["parameters"]["rate"]["input_per_mtok"].is_string(),
        "{luna}"
    );

    assert!(files_holding(p.rig.data.path(), &key).is_empty());
    let told: String = p
        .provider_events()
        .iter()
        .map(|e| e.body.to_string())
        .collect();
    assert!(!told.contains(&key));
}

#[tokio::test]
async fn one_turn_of_the_real_codex_goes_through_the_forwarder_to_the_real_service_with_a_key_it_never_has()
 {
    let Some(key) = real_key("a real Codex turn to a real service") else {
        return;
    };
    if !docker::available("a real Codex turn to a real service") {
        return;
    }
    let containers = Containers::new("codex-real");
    let for_registry = Arc::clone(&containers);
    let options = Options {
        adapters: Some(Box::new(move |data, work| {
            let mounts = vec![data.to_owned(), work.to_owned()];
            let mut registry = AdapterRegistry::new();
            registry
                .register(AdapterRegistration {
                    kind: "codex",
                    adapter_version: 1,
                    builtin: true,
                    needs_binary: true,
                    home_rule: HomeRule::Harness,
                    factory: Arc::new(move |clock| {
                        Box::new(
                            CodexAdapter::with_launcher(Arc::new(CodexDocker::new(
                                Arc::clone(&for_registry),
                                mounts.clone(),
                            )))
                            .with_clock(clock),
                        )
                    }),
                })
                .unwrap();
            registry
        })),
        ..managed()
    };
    let p = ProviderRig::build(options, |_, _| ()).await;
    let progress = p
        .start_provider_with_key(
            &[(RESPONSES, RESPONSES_BASE.to_owned(), json!({}))],
            Some(&key),
        )
        .await;
    assert!(matches!(progress, Progress::Succeeded(_)), "{progress:?}");
    let home = format!("harness/{}/{}", p.rig.provider, p.rig.agent);
    let start: orch_link_contract::commands::AgentStart = serde_json::from_value(json!({
        "provider_id": p.rig.provider, "agent_id": p.rig.agent, "harness_kind": "codex",
        "profile": { "harness_version": null, "config": {}, "env": {} }, "home": home
    }))
    .unwrap();
    p.rig.succeed(&p.rig.harness_key(), &start).await;
    p.open_forwarded_session_in(RESPONSES, OPENAI_MODEL, "bypass", "open")
        .await;
    // The turn may use what one plain answer costs and not a great deal more.
    let lease = p
        .open_lease(
            p.thread,
            &[OPENAI_MODEL],
            Some(RESPONSES),
            Some(json!({ "tokens": 24000 })),
        )
        .await;
    p.start_turn_in(
        "Reply with the single word: pong",
        OPENAI_MODEL,
        "auto",
        "open",
    )
    .await;
    p.rig
        .wait_for(p.thread, p.segment, "turn.completed", 1)
        .await;
    let said = p.rig.messages(p.thread, p.segment);
    assert!(
        !said.is_empty(),
        "the real model answered: {:?}",
        p.rig.kinds(p.thread, p.segment)
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
    let usage = p.wait_provider_event("lease.usage", 1).await;
    println!(
        "real Codex turn: {} request(s) went upstream, the first used {} tokens",
        usage.len(),
        usage[0].body["usage"]["total_tokens"]
    );
    p.close_lease(lease).await;
    for run in containers.recorded() {
        assert!(!run.argv.join(" ").contains(&key));
        assert!(run.env.values().all(|value| !value.contains(&key)));
    }
    assert!(files_holding(p.rig.data.path(), &key).is_empty());
    assert!(files_holding(p.work.path(), &key).is_empty());
    containers.remove_all();
}
