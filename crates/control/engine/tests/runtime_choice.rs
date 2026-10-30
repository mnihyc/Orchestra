//! The runtime an environment is made under is chosen when it is made, among those its host reports, within what its template permits and its place's policy allows; and what its host declares for that runtime is what holds for it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod common;
mod hosting;
use common::World;
use hosting::ScriptedHost;
use orch_db::queries::environment_facts;
use orch_engine::environments::CreateError;
use orch_types::Id;
use serde_json::json;

/// The classes a host with runc and Kata declares: Kata has a kernel of its own, offers Docker inside and none of the host's devices, and cannot change mounts live.
fn classes() -> serde_json::Value {
    json!([
        {
            "name": "runc", "title": "runc", "boundary": "shared_kernel",
            "capabilities": { "ptrace": {}, "kvm": {}, "privileged": {} },
            "live_changes": { "mounts": true, "capabilities": ["kvm"], "gateway_given": [] },
            "network_egress": ["open", "restricted"], "recreate_preserves_files": true, "keeps_software_on_stop": true
        },
        {
            "name": "kata", "title": "Kata", "boundary": "own_kernel",
            "capabilities": { "ptrace": {}, "docker_inside": {} },
            "unavailable": { "kvm": "a virtual machine has no hardware virtualisation to give" },
            "live_changes": { "mounts": false, "capabilities": [], "gateway_given": [] },
            "network_egress": ["open"], "recreate_preserves_files": true, "keeps_software_on_stop": true,
            "memory_overhead_mb": 320, "enrol_timeout_secs": 300,
            "limits": [
                { "key": "live_changes", "touches": [], "todo": "a mount or device change makes the environment again", "why": "this runtime cannot change them while it runs" },
                { "key": "kvm", "touches": ["kvm"], "todo": "run virtual machines by software emulation", "why": "no hardware virtualisation" }
            ]
        }
    ])
}

/// Makes the host report both runtimes.
async fn host_with_both(world: &World) -> ScriptedHost {
    let host = ScriptedHost::on(world, "Both runtimes", 16_384).await;
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query(
        "UPDATE hosts SET drivers = jsonb_set(jsonb_set(jsonb_set(drivers,'{docker,runtime_classes}',$2),'{docker,runtimes}',$3),'{docker,default_runtime}','\"runc\"') WHERE id = $1",
    )
    .bind(host.id)
    .bind(json!(["runc", "kata"]))
    .bind(classes())
    .execute(&mut *conn)
    .await
    .unwrap();
    host
}

#[tokio::test]
async fn an_environment_is_made_under_the_runtime_its_maker_chose_and_what_its_host_declares_for_it_holds()
 {
    let world = World::new().await;
    let host = host_with_both(&world).await;
    let template = hosting::template(&world).await;
    let made = hosting::make_with(
        &world,
        template,
        "k",
        Some(host.id),
        &json!({"env.runtime": "kata"}),
    )
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let machine = environment_facts::of(&mut conn, made.id)
        .await
        .unwrap()
        .unwrap();
    let described = machine.descriptor.unwrap();
    assert_eq!(described["effective_runtime"], "kata");
    assert!(
        described["capabilities"].get("docker_inside").is_some(),
        "Docker inside is offered under it"
    );
    assert!(
        described["capabilities"].get("kvm").is_none(),
        "the host's devices are not"
    );
    assert_eq!(
        described["live_changes"]["mounts"], false,
        "a mount change waits for a new unit"
    );
    assert_eq!(described["network_egress"], json!(["open"]));
    // The template's own runtime is what an environment takes where its maker chose none.
    let plain = hosting::make_with(&world, template, "r", Some(host.id), &json!({})).await;
    let described = environment_facts::of(&mut conn, plain.id)
        .await
        .unwrap()
        .unwrap()
        .descriptor
        .unwrap();
    assert_eq!(described["effective_runtime"], "runc");
    assert_eq!(described["live_changes"]["mounts"], true);
}

#[tokio::test]
async fn a_host_that_does_not_report_the_runtime_chosen_is_no_place_for_it() {
    let world = World::new().await;
    let host = ScriptedHost::on(&world, "Plain", 8192).await;
    let template = hosting::template(&world).await;
    let refused = hosting::try_make(
        &world,
        template,
        "g",
        Some(host.id),
        &json!({"env.runtime": "gvisor"}),
    )
    .await;
    assert!(
        matches!(refused, Err(CreateError::NoPlace(_))),
        "{refused:?}"
    );
}

async fn set_policy(world: &World, rules: serde_json::Value) {
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("INSERT INTO settings (node_id, key, value) VALUES (NULL, 'environments.runtimes', $1) ON CONFLICT (node_id, key) DO UPDATE SET value = $1")
        .bind(rules)
        .execute(&mut *conn)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_place_that_forbids_or_requires_runtimes_refuses_an_environment_made_against_it() {
    let world = World::new().await;
    let host = host_with_both(&world).await;
    let template = hosting::template(&world).await;
    set_policy(&world, json!({"kata": "forbid"})).await;
    let refused = hosting::try_make(
        &world,
        template,
        "k",
        Some(host.id),
        &json!({"env.runtime": "kata"}),
    )
    .await;
    assert!(
        matches!(&refused, Err(CreateError::Runtime(why)) if why.contains("kata")),
        "{refused:?}"
    );
    hosting::make_with(&world, template, "r", Some(host.id), &json!({})).await;
    set_policy(&world, json!({"kata": "require"})).await;
    let other = hosting::try_make(&world, template, "again", Some(host.id), &json!({})).await;
    assert!(
        matches!(&other, Err(CreateError::Runtime(why)) if why.contains("kata")),
        "the template's own runc is no longer allowed: {other:?}"
    );
    hosting::make_with(
        &world,
        template,
        "ok",
        Some(host.id),
        &json!({"env.runtime": "kata"}),
    )
    .await;
}

#[tokio::test]
async fn a_hosts_default_or_an_alias_does_not_bypass_the_places_runtime_policy() {
    let world = World::new().await;
    let host = host_with_both(&world).await;
    let template = hosting::template(&world).await;
    sqlx::query("UPDATE env_templates SET spec=spec - 'runtime' WHERE id=$1")
        .bind(template)
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(jsonb_set(drivers,'{docker,runtimes,0,aliases}','[\"container-shim\"]'),'{docker,runtime_classes}','[\"runc\",\"kata\",\"container-shim\"]') WHERE id=$1")
        .bind(host.id).execute(world.pool.pool()).await.unwrap();
    set_policy(&world, json!({"runc":"forbid"})).await;
    for (name, config) in [
        ("default", json!({})),
        ("alias", json!({"env.runtime":"container-shim"})),
    ] {
        let refused = hosting::try_make(&world, template, name, Some(host.id), &config).await;
        assert!(
            matches!(&refused,Err(CreateError::Runtime(why)) if why.contains("runc")),
            "{refused:?}"
        );
    }
    set_policy(&world, json!({"kata":"require"})).await;
    let refused = hosting::try_make(&world, template, "required", Some(host.id), &json!({})).await;
    assert!(
        matches!(&refused,Err(CreateError::Runtime(why)) if why.contains("kata")),
        "{refused:?}"
    );
    set_policy(&world, json!({"container-shim":"require"})).await;
    hosting::make_with(
        &world,
        template,
        "canonical",
        Some(host.id),
        &json!({"env.runtime":"runc"}),
    )
    .await;
}

#[tokio::test]
async fn a_runtime_policy_changed_during_an_image_job_is_checked_before_the_unit_is_created() {
    use orch_link_contract::commands::CommandKind;
    let world = World::new().await;
    hosting::serve(&world);
    let host = host_with_both(&world).await;
    let template = hosting::template(&world).await;
    host.added
        .machine
        .handler
        .hold(CommandKind::NodeImageEnsure);
    let made = hosting::make_with(
        &world,
        template,
        "policy-changed",
        Some(host.id),
        &json!({"env.runtime":"kata"}),
    )
    .await;
    host.wait_began(CommandKind::NodeImageEnsure, 1).await;
    set_policy(&world, json!({"kata":"forbid"})).await;
    host.added.machine.handler.release();
    let (state, _, error) = hosting::wait_over(&world, made.job.id).await;
    assert_eq!(state, "failed", "{error:?}");
    let error = error.unwrap();
    assert_eq!(error["code"], "runtime_policy", "{error}");
    assert_eq!(error["phase"], "create", "{error}");
    assert_eq!(
        host.count(CommandKind::NodeRuntimeCreate),
        0,
        "a policy refusal must not create a unit"
    );
}

#[tokio::test]
async fn omitting_the_runtime_does_not_escape_a_templates_allowed_runtimes() {
    let world = World::new().await;
    let host = host_with_both(&world).await;
    let template = hosting::template(&world).await;
    sqlx::query("UPDATE env_templates SET spec=(spec - 'runtime') || '{\"allowed_runtimes\":[\"kata\"]}'::jsonb WHERE id=$1")
        .bind(template).execute(world.pool.pool()).await.unwrap();
    let refused =
        hosting::try_make(&world, template, "host-default", Some(host.id), &json!({})).await;
    assert!(
        matches!(&refused,Err(CreateError::Runtime(why)) if why.contains("runc")),
        "{refused:?}"
    );
    hosting::make_with(
        &world,
        template,
        "allowed",
        Some(host.id),
        &json!({"env.runtime":"kata"}),
    )
    .await;
}

#[tokio::test]
async fn a_template_that_limits_the_choice_refuses_another_runtime() {
    let world = World::new().await;
    let host = host_with_both(&world).await;
    let template: Id = hosting::template(&world).await;
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE env_templates SET spec = spec || '{\"allowed_runtimes\": [\"runc\"]}'::jsonb WHERE id = $1")
        .bind(template)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let refused = hosting::try_make(
        &world,
        template,
        "k",
        Some(host.id),
        &json!({"env.runtime": "kata"}),
    )
    .await;
    assert!(
        matches!(refused, Err(CreateError::Runtime(_))),
        "{refused:?}"
    );
}

#[tokio::test]
async fn the_unit_records_image_observations_instead_of_its_templates_claims() {
    use orch_link_contract::commands::{CommandKind, CommandResult, NodeImageEnsureResult};
    let world = World::new().await;
    hosting::serve(&world);
    let host = host_with_both(&world).await;
    let template = hosting::template(&world).await;
    sqlx::query("UPDATE env_templates SET spec=spec || '{\"image_facts\":{\"image\":\"busybox:latest\",\"docker\":false,\"qemu\":true}}'::jsonb WHERE id=$1")
        .bind(template).execute(world.pool.pool()).await.unwrap();
    for (number, observed) in [Some(true), Some(false), None].into_iter().enumerate() {
        host.answer(CommandKind::NodeImageEnsure, move |_| {
            Ok(CommandResult::NodeImageEnsure(NodeImageEnsureResult {
                digest: Some("sha256:abc".to_owned()),
                built: None,
                programs: observed
                    .map(|present| ("docker".to_owned(), present))
                    .into_iter()
                    .collect(),
            }))
        });
        let made = hosting::make_with(
            &world,
            template,
            &format!("programs-{number}"),
            Some(host.id),
            &json!({"env.runtime":"kata"}),
        )
        .await;
        let runtime = hosting::runtime_started_for_job(&world, &host, made.job.id).await;
        let _unit = hosting::enroll(&world, runtime).await;
        let (state, _, error) = hosting::wait_over(&world, made.job.id).await;
        assert_eq!(state, "succeeded", "{error:?}");
        let definition = orch_db::queries::jobs::changes::definition(
            &mut world.pool.acquire().await.unwrap(),
            runtime,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            definition["spec"]["image_facts"]["docker"].as_bool(),
            observed
        );
        assert!(
            definition["spec"]["image_facts"].get("qemu").is_none(),
            "unobserved stays unknown"
        );
        assert_eq!(
            orch_engine::isolation::image_lacks(Some(&definition), "docker"),
            observed == Some(false)
        );
    }
}

#[test]
fn the_memory_a_guest_costs_beyond_its_limit_is_counted_when_a_host_is_judged_full() {
    use orch_db::queries::resources::hosting::LoadRow;
    let unit = |runtime: &str, state: &str| LoadRow {
        host_id: Id::nil(),
        runtime_id: orch_types::new_id(),
        kind: "environment".into(),
        state: state.into(),
        environment_id: Some(orch_types::new_id()),
        spec: json!({"runtime": runtime, "needs": {"memory_mb": 2048}}),
        config: None,
        last_activity_at: None,
    };
    let drivers = json!({"docker": {"runtimes": classes()}});
    let loads = [
        unit("kata", "running"),
        unit("runc", "running"),
        unit("kata", "stopped"),
    ];
    let plain = orch_engine::places::declared_of(&loads);
    let counted = orch_engine::places::declared_with_overhead(&loads, &drivers);
    assert_eq!(
        counted.memory_mb - plain.memory_mb,
        320,
        "Kata's declared overhead is added once, for the one unit of it that holds memory"
    );
    let chosen = LoadRow {
        config: Some(json!({"env.runtime": "kata"})),
        ..unit("runc", "running")
    };
    assert_eq!(
        orch_engine::places::declared_with_overhead(&[chosen.clone()], &drivers).memory_mb
            - orch_engine::places::declared_of(&[chosen]).memory_mb,
        320,
        "an environment's own choice counts over its template's"
    );
}
