//! A program that is nothing but a manifest runs a thread: the connector hosts it through the scripted adapter, which becomes the program its manifest names, and what the system is told of it (its kind, its name, its descriptor) is that program's.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::sync::Arc;

use common::Rig;
use orch_adapter_fake::FakeAdapter;
use orch_connector_core::harnesses::{AdapterRegistration, AdapterRegistry, HomeRule};
use orch_testkit::programs::{THIRD_KIND, third_manifest};
use orch_types::new_id;
use serde_json::json;

#[tokio::test]
async fn a_program_with_nothing_but_a_manifest_is_hosted_by_the_scripted_adapter_and_runs_a_thread()
{
    let manifest = third_manifest();
    let program = orch_adapter_fake::descriptor::Program::of(&manifest);
    let mut registry = AdapterRegistry::new();
    registry
        .register(AdapterRegistration {
            kind: orch_agent_programs::intern(&manifest.kind),
            adapter_version: 1,
            builtin: false,
            needs_binary: manifest.placement.needs_binary,
            home_rule: HomeRule::Connector,
            factory: Arc::new(move |clock| {
                Box::new(FakeAdapter::with_program(clock, program.clone()))
            }),
        })
        .unwrap();
    registry.set_manifest(
        orch_agent_programs::intern(&manifest.kind),
        manifest.clone(),
    );
    let mut rig = Rig::new();
    rig.adapters = registry;
    rig.start().await;

    let start: orch_link_contract::commands::AgentStart = serde_json::from_value(json!({
        "provider_id": rig.provider, "agent_id": rig.agent, "harness_kind": THIRD_KIND,
        "profile": { "harness_version": null, "config": {}, "env": {} }, "home": null
    }))
    .unwrap();
    let started = rig.succeed(&rig.harness_key(), &start).await;
    assert_eq!(started["descriptor"]["kind"], THIRD_KIND);
    assert_eq!(
        started["descriptor"]["harness"]["name"], "Third program",
        "the program is called what its manifest calls it"
    );
    assert_eq!(
        started["descriptor"]["text"]["zh-CN"]["harness.name"], "第三个程序",
        "and in the other language"
    );

    let (thread, segment) = (new_id(), new_id());
    rig.open_session(rig.open_body(thread, segment, "/tmp/work"))
        .await;
    rig.start_turn(thread, "hello").await;
    rig.wait_for(thread, segment, "turn.completed", 1).await;
    assert_eq!(rig.messages(thread, segment), ["You said: hello"]);

    // What the machine tells the system of its adapters carries the program's manifest.
    let facts = rig
        .adapters
        .facts()
        .into_iter()
        .find(|adapter| adapter.kind == THIRD_KIND)
        .unwrap();
    assert_eq!(facts.manifest.as_ref(), Some(&manifest));
    assert!(!facts.builtin);
    rig.connector.unwrap().stop().await;
}
