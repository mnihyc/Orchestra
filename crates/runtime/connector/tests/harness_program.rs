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

#[cfg(unix)]
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
async fn a_connected_machine_fetches_a_verified_build_only_with_permission_and_can_reuse_it_after_downloads_are_disabled()
 {
    use orch_link::commands::Progress;
    use orch_link_contract::commands::{AgentStart, AgentStop};
    use orch_link_contract::policy::PolicyKey;
    use orch_node_core::{settings::NodeSettings, tools::ToolCache};
    use sha2::{Digest, Sha256};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let bytes = b"#!/bin/sh\nexit 0\n";
    let hash = hex::encode(Sha256::digest(bytes));
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let server = axum::Router::new().route(
        "/build",
        axum::routing::get(move || {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                bytes.to_vec()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/build", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, server).await.unwrap();
    });
    for (programs, fetch, blocked) in [
        (false, true, Some("agent.programs")),
        (true, false, Some("tools.fetch")),
        (true, true, None),
    ] {
        let mut p = ProviderRig::build(
            Options {
                fake_needs_binary: true,
                ..Options::default()
            },
            |rig, _| {
                rig.config.agent.programs = programs;
                rig.config.tools.fetch = fetch;
                let cache =
                    Arc::new(ToolCache::open(&NodeSettings::in_dir(rig.data.path())).unwrap());
                let existing = rig.plug.take().unwrap();
                rig.plug = Some(Arc::new(move |builder, roles, data, key, clock| {
                    existing(builder.build_cache(cache.clone()), roles, data, key, clock)
                }));
            },
        )
        .await;
        let mut body: AgentStart = serde_json::from_value(json!({
            "provider_id":p.rig.provider,"agent_id":p.rig.agent,"harness_kind":"fake",
            "profile":{"harness_version":"1.0.0","config":{},"env":{}},"home":null,
            "runtime":{"variables":{},"build":{"job_id":null,"name":"fake","version":"1.0.0","url":url,"sha256":hash}}
        })).unwrap();
        if let Some(policy) = blocked {
            let Progress::Failed(error) = p.rig.send(&p.rig.harness_key(), &body).await else {
                panic!("the local policy was ignored");
            };
            assert!(error.detail.contains(policy), "{error:?}");
            assert_eq!(error.details.as_ref().unwrap()["policy_key"], policy);
            assert_eq!(hits.load(Ordering::SeqCst), 0);
            assert!(p.given.lock().unwrap().is_empty());
        } else {
            p.rig.succeed(&p.rig.harness_key(), &body).await;
            let executable = p.rig.data.path().join("tools/fake/1.0.0/fake");
            assert_eq!(std::fs::read(&executable).unwrap(), bytes);
            let starts: serde_json::Value =
                serde_json::from_str(&p.given.lock().unwrap()[0]).unwrap();
            assert_eq!(starts["binary"], executable.display().to_string());
            assert_eq!(hits.load(Ordering::SeqCst), 1);
            let mut facts = orch_connector_core::facts::FactsDraft::default();
            p.rig
                .connector
                .as_ref()
                .unwrap()
                .hosting()
                .unwrap()
                .contribute(&mut facts);
            assert!(
                facts
                    .harnesses
                    .iter()
                    .any(|program| program.kind == "fake" && program.version == "1.0.0")
            );
            p.rig
                .succeed(
                    &p.rig.harness_key(),
                    &AgentStop {
                        provider_id: p.rig.provider,
                        agent_id: p.rig.agent,
                        logout: None,
                    },
                )
                .await;
            let policy = &p.rig.connector.as_ref().unwrap().services().policy;
            policy
                .set_here(PolicyKey::ToolsFetch, &json!(false))
                .unwrap();
            p.rig.succeed(&p.rig.harness_key(), &body).await;
            assert_eq!(
                hits.load(Ordering::SeqCst),
                1,
                "the verified cached version is reused without a download"
            );
            p.rig
                .succeed(
                    &p.rig.harness_key(),
                    &AgentStop {
                        provider_id: p.rig.provider,
                        agent_id: p.rig.agent,
                        logout: None,
                    },
                )
                .await;
            policy
                .set_here(PolicyKey::ToolsFetch, &json!(true))
                .unwrap();
            body.profile.harness_version = Some("2.0.0".into());
            let build = body.runtime.as_mut().unwrap().build.as_mut().unwrap();
            build.version = "2.0.0".into();
            build.sha256 = "0".repeat(64);
            let Progress::Failed(error) = p.rig.send(&p.rig.harness_key(), &body).await else {
                panic!("an unverified artifact was executed");
            };
            assert_eq!(error.details.unwrap()["checksum"], "mismatch");
            assert!(!p.rig.data.path().join("tools/fake/2.0.0").exists());
            assert_eq!(p.given.lock().unwrap().len(), 2);
            p.rig.kill().await;
            p.rig.start().await;
            body.profile.harness_version = Some("1.0.0".into());
            let build = body.runtime.as_mut().unwrap().build.as_mut().unwrap();
            build.version = "1.0.0".into();
            build.sha256 = hash.clone();
            p.rig.succeed(&p.rig.harness_key(), &body).await;
            let facts = p.rig.connector.as_ref().unwrap().facts();
            assert_eq!(
                facts
                    .harnesses
                    .iter()
                    .filter(|program| program.kind == "fake" && program.version == "1.0.0")
                    .count(),
                1,
                "a cached program is reported once after reconnect and startup"
            );
            assert_eq!(
                hits.load(Ordering::SeqCst),
                2,
                "reconnecting reused the verified cache"
            );
        }
        if let Some(connector) = p.rig.connector.take() {
            connector.stop().await;
        }
    }
    task.abort();
}
