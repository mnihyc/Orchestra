//! Where a hosted harness's program comes from: in a runtime the system made, the tools directory the node gave it; a program the profile names; and the path where there is none. The adapter is given the place, and a harness is never started from a place the system's word alone chose on a machine whose owner did not list it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::model::{Options, ProviderRig};
use serde_json::json;

#[cfg(unix)]
fn program(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

async fn started_with(p: &ProviderRig, profile: serde_json::Value) -> String {
    let body: orch_link_contract::commands::AgentStart = serde_json::from_value(json!({
        "provider_id": p.rig.provider, "agent_id": p.rig.agent, "harness_kind": "fake",
        "profile": profile, "home": null
    }))
    .unwrap();
    p.rig.succeed(&p.rig.harness_key(), &body).await;
    let given = p.given.lock().unwrap().clone();
    let start: serde_json::Value = serde_json::from_str(&given[0]).unwrap();
    start["binary"].as_str().unwrap_or("none").to_owned()
}

#[cfg(unix)]
#[tokio::test]
async fn a_runtime_the_system_made_runs_the_harness_from_its_tools_directory() {
    let tools = tempfile::tempdir().unwrap();
    program(&tools.path().join("fake/fake"));
    let options = Options {
        managed: true,
        tools_dir: Some(tools.path().to_owned()),
        fake_needs_binary: true,
        ..Options::default()
    };
    let p = ProviderRig::build(options, |_, _| ()).await;
    let place = started_with(
        &p,
        json!({ "harness_version": null, "config": {}, "env": {} }),
    )
    .await;
    assert_eq!(place, tools.path().join("fake/fake").display().to_string());
}

#[cfg(unix)]
#[tokio::test]
async fn a_program_the_profile_names_is_the_one_run_and_a_tool_that_is_not_there_leaves_it_to_the_path()
 {
    let tools = tempfile::tempdir().unwrap();
    program(&tools.path().join("fake/fake"));
    let options = Options {
        managed: true,
        tools_dir: Some(tools.path().to_owned()),
        fake_needs_binary: true,
        ..Options::default()
    };
    let p = ProviderRig::build(options, |_, _| ()).await;
    let named = started_with(
        &p,
        json!({ "attach": { "mode": "spawn", "binary": "/opt/elsewhere/fake", "home": null },
                "config": {}, "env": {} }),
    )
    .await;
    assert_eq!(named, "/opt/elsewhere/fake");

    let empty = tempfile::tempdir().unwrap();
    let options = Options {
        managed: true,
        tools_dir: Some(empty.path().to_owned()),
        fake_needs_binary: true,
        ..Options::default()
    };
    let p = ProviderRig::build(options, |_, _| ()).await;
    assert_eq!(
        started_with(&p, json!({ "config": {}, "env": {} })).await,
        "none",
        "no program in the tools directory: the adapter looks on the path"
    );
}

#[tokio::test]
async fn a_machine_a_person_connected_runs_only_the_program_its_owner_listed() {
    let options = Options {
        fake_needs_binary: true,
        ..Options::default()
    };
    let p = ProviderRig::build(options, |_, _| ()).await;
    let body: orch_link_contract::commands::AgentStart = serde_json::from_value(json!({
        "provider_id": p.rig.provider, "agent_id": p.rig.agent, "harness_kind": "fake",
        "profile": { "attach": { "mode": "spawn", "binary": "/tmp/anything", "home": null }, "config": {}, "env": {} },
        "home": null
    }))
    .unwrap();
    assert_eq!(
        p.rig.fail(&p.rig.harness_key(), &body).await,
        "harness_unavailable",
        "the owner of this machine listed no program for it"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn removing_a_confirmed_program_refuses_new_turns_and_reuse_of_the_old_harness() {
    use orch_link_contract::commands::SerialKey;
    use orch_link_contract::policy::PolicyKey;
    let directory = tempfile::tempdir().unwrap();
    let local_program = directory.path().join("fake");
    program(&local_program);
    let configured = local_program.display().to_string();
    let options = Options {
        fake_needs_binary: true,
        ..Options::default()
    };
    let p = ProviderRig::build(options, move |rig, _| {
        rig.config.agent.harnesses = vec![orch_config::connector::AgentHarness {
            kind: "fake".into(),
            path: configured,
        }];
    })
    .await;
    started_with(&p, json!({"config":{},"env":{}})).await;
    p.open_forwarded_session("openai-responses", "mock-large")
        .await;
    p.rig
        .connector
        .as_ref()
        .unwrap()
        .services()
        .policy
        .set_here(PolicyKey::AgentHarnesses, &json!([]))
        .unwrap();

    let start: orch_link_contract::commands::AgentStart = serde_json::from_value(json!({
        "provider_id":p.rig.provider,"agent_id":p.rig.agent,"harness_kind":"fake",
        "profile":{"config":{},"env":{}},"home":null
    }))
    .unwrap();
    assert_eq!(
        p.rig.fail(&p.rig.harness_key(), &start).await,
        "harness_unavailable"
    );
    let turn: orch_link_contract::commands::TurnStart = serde_json::from_value(json!({
        "thread_id":p.thread,"turn_id":orch_types::new_id(),"text":"work","images":[],
        "model":"mock-large","effort":null,"params":{},"approval_mode":"on_request","sandbox_mode":"workspace"
    })).unwrap();
    assert_eq!(
        p.rig.fail(&SerialKey::Thread(p.thread), &turn).await,
        "harness_unavailable"
    );
    let steer = orch_link_contract::commands::TurnSteer {
        thread_id: p.thread,
        turn_id: turn.turn_id,
        text: "more work".into(),
        images: Vec::new(),
        native_action: None,
    };
    assert_eq!(
        p.rig.fail(&SerialKey::Thread(p.thread), &steer).await,
        "harness_unavailable"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_local_program_is_redetected_on_reconnect_and_on_a_machine_probe() {
    use orch_link_contract::commands::{MachineProbe, SerialKey};
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("fake");
    program(&executable);
    let write_version = |version: &str| {
        std::fs::write(&executable, format!("#!/bin/sh\nprintf '{version}\\n'\n")).unwrap();
    };
    write_version("1.0.0");
    let configured = executable.display().to_string();
    let options = Options {
        fake_needs_binary: true,
        ..Options::default()
    };
    let p = ProviderRig::build(options, move |rig, _| {
        rig.config.agent.harnesses = vec![orch_config::connector::AgentHarness {
            kind: "fake".into(),
            path: configured,
        }];
    })
    .await;
    let reports_version = |controller: &orch_testkit::doubles::controller::ScriptedController,
                           version: &str| {
        controller.applied("machine").iter().any(|event| {
            event.kind == "machine.facts"
                && event.body["harnesses"].as_array().is_some_and(|rows| {
                    rows.iter()
                        .any(|row| row["kind"] == "fake" && row["version"] == version)
                })
        })
    };
    let wait = common::PATIENCE;
    assert!(
        p.rig
            .controller
            .wait_until(wait, |controller| reports_version(controller, "1.0.0"))
            .await
    );
    write_version("1.1.0");
    p.rig.hub.faults().sever_now();
    assert!(
        p.rig
            .controller
            .wait_until(wait, |controller| reports_version(controller, "1.1.0"))
            .await
    );
    write_version("1.2.0");
    let facts = p.rig.succeed(&SerialKey::Machine, &MachineProbe {}).await;
    assert!(
        facts["harnesses"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["kind"] == "fake" && row["version"] == "1.2.0")
    );
}

#[tokio::test]
async fn a_profile_cannot_redirect_a_bare_policy_program_with_another_path() {
    let options = Options {
        fake_needs_binary: true,
        ..Options::default()
    };
    let p = ProviderRig::build(options, |rig, _| {
        rig.config.agent.harnesses = vec![orch_config::connector::AgentHarness {
            kind: "fake".into(),
            path: "fake".into(),
        }];
    })
    .await;
    let request: orch_link_contract::commands::AgentStart = serde_json::from_value(json!({
        "provider_id":p.rig.provider,"agent_id":p.rig.agent,"harness_kind":"fake",
        "profile":{"config":{},"env":{"PATH":"/not-the-machines-path"}},"home":null
    }))
    .unwrap();
    assert_eq!(
        p.rig.fail(&p.rig.harness_key(), &request).await,
        "harness_unavailable"
    );
    assert!(
        p.rig
            .connector
            .as_ref()
            .unwrap()
            .services()
            .agents
            .get()
            .unwrap()
            .process(p.rig.provider, p.rig.agent)
            .is_err()
    );
}
