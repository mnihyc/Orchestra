//! Fork plans use the same inherited mount resolution as provision, including every mode and an explicit private-copy choice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::World;
use hosting::{ScriptedHost, bind, running, serve, template};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_authz::{
    Caller,
    ownership::{NewAccess, start_resource},
};
use orch_db::queries::resources::mounts::{self as rows, LocationRow, MountRow};
use orch_types::{Id, Time, new_id};
use serde_json::json;

#[tokio::test]
async fn a_fork_to_a_new_environment_lists_readonly_shared_and_private_mounts_from_its_actual_host()
{
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "mounted-fork", 16_384).await;
    let template = template(&world).await;
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(jsonb_set(drivers,'{docker,mounts}','\"placed\"'::jsonb),'{docker,private_views}','true'::jsonb),storages=$2 WHERE id=$1")
        .bind(host.id).bind(json!([{"name":"fast","default":true,"capacity_gb":100,"free_gb":80,"private_views":true}])).execute(world.pool.pool()).await.unwrap();
    // The source's unit stays connected: a thread bound to it is served there while the plan is made.
    let (environment, _, _source_unit) = running(&world, &host, template, "Source").await;
    let mut choices = serde_json::Map::new();
    for (name, readonly, selection) in [
        ("readonly", true, json!(true)),
        ("shared", false, json!(true)),
        ("private", true, json!("private")),
    ] {
        let id = new_id();
        let mut conn = world.pool.acquire().await.unwrap();
        rows::insert(
            &mut conn,
            &MountRow {
                id,
                name: name.into(),
                description: String::new(),
                org_id: Some(world.root),
                home_id: Some(world.root),
                owner_id: Some(world.ada),
                target: format!("/data/{name}"),
                read_only: readonly,
                enabled: true,
                default_fate: "as_is".into(),
                revision: 1,
            },
            world.clock.now(),
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ResourceKind::Mount.into(),
                id,
            },
            world.ada,
            NewAccess::OnlyMe,
            Time::new(world.clock.now()),
        )
        .await
        .unwrap();
        rows::set_location(
            &mut conn,
            &LocationRow {
                mount_id: id,
                machine_id: host.machine,
                host_id: Some(host.id),
                host_name: Some("Mounted".into()),
                kind: "path".into(),
                source: format!("/srv/{name}"),
                storage: None,
                data_kind: None,
                created_by: world.ada,
                created_at: world.clock.now(),
            },
        )
        .await
        .unwrap();
        choices.insert(id.to_string(), selection);
    }
    world.set(world.topic, "env.mounts", json!(choices)).await;
    let thread = world.thread("Mounted fork").await;
    bind(&world, thread, environment).await;
    let request = serde_json::from_value(json!({"result":"new","history":{"mode":"none"},"environment":{"new":{"name":"New mounted","template_id":template,"host_id":host.id,"files":{"mode":"none"},"home":false,"private_views":false}}})).unwrap();
    let draft = world
        .engine()
        .draft_fork(&Caller::session(world.ada), thread, &request)
        .await
        .unwrap();
    let planned = serde_json::to_value(&draft.plan).unwrap();
    let mounts = planned["mounts"].as_array().unwrap();
    assert_eq!(mounts.len(), 3);
    for (name, mode) in [
        ("readonly", "read_only"),
        ("shared", "shared"),
        ("private", "private"),
    ] {
        let mount = mounts
            .iter()
            .find(|mount| mount["mount"]["name"] == name)
            .unwrap();
        assert_eq!(mount["mode"], mode);
        assert_eq!(mount["path"], format!("/data/{name}"));
        assert!(mount["detached_because"].is_null());
    }
    assert!(
        !draft
            .plan
            .blockers
            .iter()
            .any(|blocker| blocker.code == "private_view_transfer_unavailable"),
        "leaving private changes starts a fresh view and promises no copy"
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let resolved = orch_engine::mounts::resolve_for(
        &mut conn,
        &orch_engine::mounts::Target {
            environment: Id::nil(),
            home: world.topic,
            own: &json!({}),
            owner: world.ada,
            machine: Some(host.machine),
            driver: Some("docker"),
            workspace: "/workspace",
            home_path: Some("/home/agent"),
            storage: None,
            planned: &[],
            detached: &[],
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    assert!(
        resolved
            .iter()
            .find(|mount| mount.view.mount.name == "private")
            .unwrap()
            .layer
            .is_some(),
        "provision receives a separate owned private layer"
    );
    drop(conn);
    assert!(draft.plan.blockers.is_empty(), "{:?}", draft.plan.blockers);
    let caller = Caller::session(world.ada);
    let kept = world.engine().keep_fork_plan(&caller, draft).await.unwrap();
    let job = world
        .engine()
        .confirm_fork(
            &caller,
            thread,
            &orch_api_contract::forks::ForkCreateBody {
                notify: None,
                preflight_id: kept.preflight_id,
                options: None,
            },
        )
        .await
        .unwrap();
    host.wait_for(
        orch_link_contract::commands::CommandKind::NodeRuntimeStart,
        1,
    )
    .await;
    let _unit = hosting::enroll(&world, host.started().unwrap()).await;
    let (state, _, error) = hosting::wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let create = host
        .calls()
        .into_iter()
        .find(|call| call.kind == orch_link_contract::commands::CommandKind::NodeRuntimeCreate)
        .unwrap();
    assert_eq!(create.body["mounts"].as_array().unwrap().len(), 3);
    let private = create.body["mounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|mount| mount["name"] == "private")
        .unwrap();
    assert_eq!(private["mode"], "private");
    assert!(
        private["layer"]["name"]
            .as_str()
            .unwrap()
            .starts_with("orch-over-")
    );
    world.engine().stop();
}

/// Lets the scripted host's Docker driver place mounts, as the real driver declares.
async fn places_mounts(world: &World, host: &ScriptedHost) {
    sqlx::query(
        "UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,mounts}','\"placed\"'::jsonb) WHERE id=$1",
    )
    .bind(host.id)
    .execute(world.pool.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn a_synced_target_is_compared_on_request_and_keeps_the_time_without_changing_its_fate() {
    use orch_api_contract::forks::ForkRequest;
    use orch_link_contract::commands::{CommandKind, CommandResult, NodeMountFingerprintResult};
    use orch_types::Hash;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let clock = Arc::new(orch_types::ManualClock::new(orch_types::Clock::now(
        &orch_types::SystemClock,
    )));
    let world = World::with_clock(clock.clone()).await;
    serve(&world);
    let source = ScriptedHost::on(&world, "fingerprint-source", 16_384).await;
    let target = ScriptedHost::on(&world, "fingerprint-target", 16_384).await;
    let template = template(&world).await;
    places_mounts(&world, &source).await;
    places_mounts(&world, &target).await;
    let (environment, _, _source_unit) = running(&world, &source, template, "Source").await;
    let id = mount_on(&world, &source, "data", "as_is").await;
    let mut conn = world.pool.acquire().await.unwrap();
    rows::set_location(
        &mut conn,
        &LocationRow {
            mount_id: id,
            machine_id: target.machine,
            host_id: Some(target.id),
            host_name: None,
            kind: "path".into(),
            source: "/srv/synced".into(),
            storage: None,
            data_kind: Some("synced".into()),
            created_by: world.ada,
            created_at: world.clock.now(),
        },
    )
    .await
    .unwrap();
    drop(conn);
    world
        .set(world.topic, "env.mounts", json!({id.to_string(): true}))
        .await;
    let thread = world.thread("Check synced data").await;
    bind(&world, thread, environment).await;
    let reads = Arc::new(AtomicUsize::new(0));
    let differs = Arc::new(AtomicBool::new(false));
    for (host, on_target) in [(&source, false), (&target, true)] {
        let reads = reads.clone();
        let differs = differs.clone();
        let clock = clock.clone();
        host.added
            .machine
            .handler
            .on(CommandKind::NodeMountFingerprint, move |_| {
                reads.fetch_add(1, Ordering::SeqCst);
                // Each reply advances observation time, independently of the plan's start time.
                clock.advance(chrono::Duration::milliseconds(1));
                let content = if on_target && differs.load(Ordering::SeqCst) {
                    b"diff".as_slice()
                } else {
                    b"same".as_slice()
                };
                Ok(CommandResult::NodeMountFingerprint(
                    NodeMountFingerprintResult {
                        fingerprint: Hash::of(content),
                    },
                ))
            });
    }
    let caller = Caller::session(world.ada);
    let mut request: ForkRequest = serde_json::from_value(json!({"result":"new","history":{"mode":"none"},
        "environment":{"new":{"name":"Target","template_id":template,"host_id":target.id,"files":{"mode":"none"},"home":false,"private_views":false}}})).unwrap();
    let unchecked = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "ordinary plans read no file contents"
    );
    assert!(unchecked.plan.mounts[0].fingerprints_available);
    assert!(unchecked.plan.mounts[0].fingerprints.is_none());
    assert_eq!(
        unchecked.plan.mounts[0].data_kind,
        Some(orch_api_contract::mounts::MountDataKind::Own),
        "the source hint does not decide whether the target can be checked"
    );
    request.check_fingerprints = Some(true);
    request.fingerprint_request_id = Some(new_id());
    let before_check = Time::new(world.clock.now());
    let pending = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    assert!(pending.plan.mounts[0].fingerprints.is_none());
    let reference = pending.plan.mounts[0].fingerprint_measurement.unwrap();
    assert!(reference.measured_at.is_none());
    let (state, _, error) = hosting::wait_over(&world, reference.job_id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let checked = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    assert_eq!(
        checked.plan.mounts[0]
            .fingerprint_measurement
            .unwrap()
            .job_id,
        reference.job_id
    );

    let result = checked.plan.mounts[0].fingerprints.as_ref().unwrap();
    assert!(result.matches);
    assert_eq!(result.checked_at, Time::new(world.clock.now()));
    assert!(
        result.checked_at > before_check,
        "the comparison is dated after both locations answer"
    );
    assert_eq!(
        checked.plan.mounts[0].action,
        unchecked.plan.mounts[0].action
    );
    assert_eq!(checked.plan.mounts[0].fate, unchecked.plan.mounts[0].fate);
    assert!(
        checked.plan.blockers.is_empty(),
        "{:?}",
        checked.plan.blockers
    );
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    let kept = world
        .engine()
        .keep_fork_plan(&caller, checked)
        .await
        .unwrap();
    assert!(kept.mounts[0].fingerprints.as_ref().unwrap().matches);
    differs.store(true, Ordering::SeqCst);
    request.fingerprint_request_id = Some(new_id());
    let pending = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    assert!(pending.plan.mounts[0].fingerprints.is_none());
    let reference = pending.plan.mounts[0].fingerprint_measurement.unwrap();
    assert!(reference.measured_at.is_none());
    let (state, _, error) = hosting::wait_over(&world, reference.job_id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let different = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    assert_eq!(
        different.plan.mounts[0]
            .fingerprint_measurement
            .unwrap()
            .job_id,
        reference.job_id
    );

    assert!(
        !different.plan.mounts[0]
            .fingerprints
            .as_ref()
            .unwrap()
            .matches
    );
    assert!(
        different.plan.blockers.is_empty(),
        "different content is informative"
    );
    assert_eq!(reads.load(Ordering::SeqCst), 4);
    request.check_fingerprints = Some(false);
    let fresh = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    assert!(
        fresh.plan.mounts[0].fingerprints.is_none(),
        "a fresh ordinary plan never presents a stale check"
    );
    assert_eq!(reads.load(Ordering::SeqCst), 4);
    request.check_fingerprints = Some(true);
    request.mounts = Some(vec![orch_api_contract::forks::MountChoice {
        mount_id: id,
        fate: orch_api_contract::mounts::MountFate::Detach,
        into: None,
    }]);
    let detached = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    assert!(!detached.plan.mounts[0].fingerprints_available);
    assert!(detached.plan.mounts[0].fingerprints.is_none());
    assert_eq!(
        reads.load(Ordering::SeqCst),
        4,
        "detached data is not read even when asked"
    );
    request.mounts = None;
    let mut conn = world.pool.acquire().await.unwrap();
    rows::set_data_kind(&mut conn, id, target.machine, Some("own"))
        .await
        .unwrap();
    drop(conn);
    let own = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    assert!(!own.plan.mounts[0].fingerprints_available);
    assert!(own.plan.mounts[0].fingerprints.is_none());
    assert_eq!(
        reads.load(Ordering::SeqCst),
        4,
        "a target not labelled synced is not checked"
    );
    let mut conn = world.pool.acquire().await.unwrap();
    rows::set_data_kind(&mut conn, id, target.machine, Some("synced"))
        .await
        .unwrap();
    sqlx::query("UPDATE mounts SET read_only=false WHERE id=$1")
        .bind(id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let writable = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    assert!(!writable.plan.mounts[0].fingerprints_available);
    assert!(writable.plan.mounts[0].fingerprints.is_none());
    assert_eq!(
        reads.load(Ordering::SeqCst),
        4,
        "a writable target is not a read-only synced copy"
    );
    sqlx::query("UPDATE mounts SET read_only=true WHERE id=$1")
        .bind(id)
        .execute(world.pool.pool())
        .await
        .unwrap();
    source
        .added
        .machine
        .handler
        .on(CommandKind::NodeMountFingerprint, |_| {
            Err(orch_link_contract::commands::CommandError::new(
                orch_link_contract::commands::ErrorCode::Unsupported,
                "the host cannot read this location",
            ))
        });
    request.fingerprint_request_id = Some(new_id());
    let failed = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    let reference = failed.plan.mounts[0].fingerprint_measurement.unwrap();
    let (state, _, error) = hosting::wait_over(&world, reference.job_id).await;
    assert_eq!(state, "failed");
    let error = error.unwrap();
    assert!(
        error
            .to_string()
            .contains("the host cannot read this location"),
        "{error}"
    );
    let retained = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    assert_eq!(
        retained.plan.mounts[0]
            .fingerprint_measurement
            .unwrap()
            .job_id,
        reference.job_id
    );
    assert!(retained.plan.mounts[0].fingerprints.is_none());
    // Even a source that now refuses a check does not refuse the already reviewed move: confirmation never scans.
    let job = world
        .engine()
        .confirm_fork(
            &caller,
            thread,
            &orch_api_contract::forks::ForkCreateBody {
                notify: None,
                preflight_id: kept.preflight_id,
                options: None,
            },
        )
        .await
        .unwrap();
    target.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let _target_unit = hosting::enroll(&world, target.started().unwrap()).await;
    let (state, _, error) = hosting::wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    world.engine().stop();
}

#[tokio::test]
async fn a_live_content_check_can_stop_either_read_and_a_replan_never_restarts_it() {
    use orch_link_contract::commands::{CommandKind, CommandResult, NodeMountFingerprintResult};
    use orch_link_contract::events::{Event, JobProgress};
    use orch_link_contract::streams::Stream;
    use orch_types::Hash;
    for target_pass in [false, true] {
        let world = World::new().await;
        serve(&world);
        let source = ScriptedHost::on(&world, "Check source", 16_384).await;
        let target = ScriptedHost::on(&world, "Check target", 16_384).await;
        let template = template(&world).await;
        places_mounts(&world, &source).await;
        places_mounts(&world, &target).await;
        let (environment, _, _source_unit) =
            running(&world, &source, template, "Checked source").await;
        let mount = mount_on(&world, &source, "checked-data", "as_is").await;
        let mut conn = world.pool.acquire().await.unwrap();
        rows::set_location(
            &mut conn,
            &LocationRow {
                mount_id: mount,
                machine_id: target.machine,
                host_id: Some(target.id),
                host_name: None,
                kind: "path".into(),
                source: "/srv/checked-target".into(),
                storage: None,
                data_kind: Some("synced".into()),
                created_by: world.ada,
                created_at: world.clock.now(),
            },
        )
        .await
        .unwrap();
        drop(conn);
        world
            .set(world.topic, "env.mounts", json!({mount.to_string():true}))
            .await;
        let thread = world.thread("Check with progress").await;
        bind(&world, thread, environment).await;
        for host in [&source, &target] {
            host.added
                .machine
                .handler
                .on(CommandKind::NodeMountFingerprint, |_| {
                    Ok(CommandResult::NodeMountFingerprint(
                        NodeMountFingerprintResult {
                            fingerprint: Hash::of(b"same"),
                        },
                    ))
                });
        }
        let reading = if target_pass { &target } else { &source };
        reading
            .added
            .machine
            .handler
            .hold(CommandKind::NodeMountFingerprint);
        let caller = Caller::session(world.ada);
        let mut request: orch_api_contract::forks::ForkRequest = serde_json::from_value(json!({
            "result":"new","history":{"mode":"none"},"check_fingerprints":true,"fingerprint_request_id":new_id(),
            "environment":{"new":{"name":"Checked destination","template_id":template,"host_id":target.id,"files":{"mode":"none"},"home":false,"private_views":false}}
        })).unwrap();
        let pending = world
            .engine()
            .draft_fork(&caller, thread, &request)
            .await
            .unwrap();
        let reference = pending.plan.mounts[0].fingerprint_measurement.unwrap();
        assert!(pending.plan.mounts[0].fingerprints.is_none());
        reading
            .wait_began(CommandKind::NodeMountFingerprint, 1)
            .await;
        if !target_pass {
            assert_eq!(target.began(CommandKind::NodeMountFingerprint), 0);
        }
        let command = reading
            .added
            .machine
            .handler
            .executions()
            .into_iter()
            .find(|run| run.kind == CommandKind::NodeMountFingerprint)
            .unwrap()
            .id;
        let retained = world
            .commands
            .as_engine()
            .retained_command(command)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.machine, reading.machine);
        assert_eq!(retained.body["work"]["job_id"], json!(reference.job_id));
        let phase = if target_pass {
            "fingerprint_target"
        } else {
            "fingerprint_source"
        };
        // The in-memory transport retains commands outside SQL; admit that same command for the report fence.
        let mut conn = world.pool.acquire().await.unwrap();
        orch_db::queries::link::insert_command(
            &mut conn,
            &orch_db::queries::link::NewCommand {
                id: command,
                machine_id: reading.machine,
                kind: "node.mount.fingerprint",
                serial_key: None,
                body: &retained.body,
                deadline_at: world.clock.now()
                    + chrono::Duration::seconds(
                        retained.body["work"]["timeout_secs"].as_i64().unwrap(),
                    ),
            },
            world.clock.now(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE commands SET state='accepted' WHERE id=$1")
            .bind(command)
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        reading.added.machine.publish(&Stream::Node, &Event::JobProgress(JobProgress {
            command_id: Some(command), job_id: reference.job_id, log: None,
            progress: serde_json::from_value(json!({"phase":phase,"phases":[],"percent":0,"files_done":7,"files_total":null,"bytes_done":4096,"bytes_total":null,"rate_bps":null,"eta_seconds":null,"message":null})).unwrap(),
        })).await;
        common::eventually(
            "fingerprint byte counters to reach its live job",
            || async { hosting::job(&world, reference.job_id).await.1["bytes_done"] == 4096 },
        )
        .await;
        let progress = hosting::job(&world, reference.job_id).await.1;
        assert_eq!(progress["phase"], phase);
        assert_eq!(progress["files_done"], 7);
        assert!(progress["activity"]["updated_at"].is_string());
        let repeated = world
            .engine()
            .draft_fork(&caller, thread, &request)
            .await
            .unwrap();
        assert_eq!(
            repeated.plan.mounts[0]
                .fingerprint_measurement
                .unwrap()
                .job_id,
            reference.job_id
        );
        assert!(
            world
                .engine()
                .cancel_job(reference.job_id, world.ada)
                .await
                .unwrap()
        );
        assert_eq!(
            hosting::wait_over(&world, reference.job_id).await.0,
            "cancelled"
        );
        let stopped = world
            .engine()
            .draft_fork(&caller, thread, &request)
            .await
            .unwrap();
        assert_eq!(
            stopped.plan.mounts[0]
                .fingerprint_measurement
                .unwrap()
                .job_id,
            reference.job_id
        );
        assert!(stopped.plan.mounts[0].fingerprints.is_none());
        assert_eq!(source.began(CommandKind::NodeMountFingerprint), 1);
        assert_eq!(
            target.began(CommandKind::NodeMountFingerprint),
            usize::from(target_pass)
        );
        reading.added.machine.handler.release();
        request.fingerprint_request_id = Some(new_id());
        let retry = world
            .engine()
            .draft_fork(&caller, thread, &request)
            .await
            .unwrap();
        let retry_id = retry.plan.mounts[0].fingerprint_measurement.unwrap().job_id;
        assert_ne!(retry_id, reference.job_id);
        let (state, _, error) = hosting::wait_over(&world, retry_id).await;
        assert_eq!(state, "succeeded", "{error:?}");
        let complete = world
            .engine()
            .draft_fork(&caller, thread, &request)
            .await
            .unwrap();
        assert!(
            complete.plan.mounts[0]
                .fingerprints
                .as_ref()
                .unwrap()
                .matches
        );
        world.engine().stop();
    }
}

/// A read-only mount of the world's maker with a directory location on the host and the default given.
async fn mount_on(world: &World, host: &ScriptedHost, name: &str, default_fate: &str) -> Id {
    let id = new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    rows::insert(
        &mut conn,
        &MountRow {
            id,
            name: name.into(),
            description: String::new(),
            org_id: Some(world.root),
            home_id: Some(world.root),
            owner_id: Some(world.ada),
            target: format!("/data/{name}"),
            read_only: true,
            enabled: true,
            default_fate: default_fate.into(),
            revision: 1,
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Mount.into(),
            id,
        },
        world.ada,
        NewAccess::OnlyMe,
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    rows::set_location(
        &mut conn,
        &LocationRow {
            mount_id: id,
            machine_id: host.machine,
            host_id: Some(host.id),
            host_name: None,
            kind: "path".into(),
            source: format!("/srv/{name}"),
            storage: None,
            data_kind: Some("own".into()),
            created_by: world.ada,
            created_at: world.clock.now(),
        },
    )
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn a_new_environment_takes_each_mounts_default_and_leaves_off_what_its_plan_detaches() {
    use orch_api_contract::forks::{
        CopyUnavailable, ForkCreateBody, ForkPlan, ForkRequest, MountAction, MountDetachedBecause,
        PlanMount,
    };
    use orch_api_contract::mounts::{MountDataKind, MountFate};
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "fated-fork", 16_384).await;
    let template = template(&world).await;
    places_mounts(&world, &host).await;
    // The source's unit stays connected: a thread bound to it is served there while the plan is made.
    let (environment, _, _source_unit) = running(&world, &host, template, "Source").await;
    let kept = mount_on(&world, &host, "kept", "as_is").await;
    let dropped = mount_on(&world, &host, "dropped", "detach").await;
    let copied = mount_on(&world, &host, "copied", "copy").await;
    let enabled: serde_json::Map<String, serde_json::Value> = [kept, dropped, copied]
        .iter()
        .map(|id| (id.to_string(), json!(true)))
        .collect();
    world.set(world.topic, "env.mounts", json!(enabled)).await;
    let thread = world.thread("Fated fork").await;
    bind(&world, thread, environment).await;
    let request = |mounts: serde_json::Value| -> ForkRequest {
        serde_json::from_value(json!({"result":"new","history":{"mode":"none"},
            "environment":{"new":{"name":"Fated","template_id":template,"host_id":host.id,"files":{"mode":"none"},"home":false,"private_views":false}},
            "mounts":mounts}))
        .unwrap()
    };
    let row = |plan: &ForkPlan, id: Id| -> PlanMount {
        plan.mounts
            .iter()
            .find(|mount| mount.mount.id == id)
            .unwrap()
            .clone()
    };
    let caller = Caller::session(world.ada);
    let draft = world
        .engine()
        .draft_fork(&caller, thread, &request(json!([])))
        .await
        .unwrap();
    let as_is = row(&draft.plan, kept);
    assert_eq!(as_is.fate, MountFate::AsIs);
    assert_eq!(as_is.action, MountAction::Keep);
    assert_eq!(as_is.fates, vec![MountFate::Detach, MountFate::AsIs]);
    assert_eq!(
        as_is.copy_unavailable,
        Some(CopyUnavailable::SameMachine),
        "the data is on that machine already"
    );
    assert_eq!(
        as_is.data_kind,
        Some(MountDataKind::Own),
        "the hint is shown"
    );
    let off = row(&draft.plan, dropped);
    assert_eq!(
        (off.fate, off.default_fate, off.action),
        (MountFate::Detach, MountFate::Detach, MountAction::Detach)
    );
    assert_eq!(off.detached_because, Some(MountDetachedBecause::Asked));
    let given_way = row(&draft.plan, copied);
    assert_eq!(given_way.default_fate, MountFate::Copy);
    assert_eq!(
        given_way.fate,
        MountFate::AsIs,
        "a default that cannot be done here gives way to mounting it as it is"
    );
    assert!(draft.plan.blockers.is_empty(), "{:?}", draft.plan.blockers);
    let refused = world
        .engine()
        .draft_fork(
            &caller,
            thread,
            &request(json!([{ "mount_id": copied, "fate": "copy" }])),
        )
        .await
        .unwrap();
    assert!(
        refused
            .plan
            .blockers
            .iter()
            .any(|blocker| blocker.code == "mount_copy_unavailable"),
        "asking for what cannot be done here is refused with why: {:?}",
        refused.plan.blockers
    );
    let back = world
        .engine()
        .draft_fork(
            &caller,
            thread,
            &request(json!([{ "mount_id": dropped, "fate": "as_is" }])),
        )
        .await
        .unwrap();
    assert_eq!(row(&back.plan, dropped).action, MountAction::Keep);
    let kept_plan = world.engine().keep_fork_plan(&caller, draft).await.unwrap();
    let job = world
        .engine()
        .confirm_fork(
            &caller,
            thread,
            &ForkCreateBody {
                notify: None,
                preflight_id: kept_plan.preflight_id,
                options: None,
            },
        )
        .await
        .unwrap();
    host.wait_for(
        orch_link_contract::commands::CommandKind::NodeRuntimeStart,
        1,
    )
    .await;
    let _unit = hosting::enroll(&world, host.started().unwrap()).await;
    let (state, _, error) = hosting::wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let create = host
        .calls()
        .into_iter()
        .find(|call| call.kind == orch_link_contract::commands::CommandKind::NodeRuntimeCreate)
        .unwrap();
    let names: Vec<String> = create.body["mounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|mount| mount["name"].as_str().unwrap().to_owned())
        .collect();
    assert!(names.contains(&"kept".to_owned()), "{names:?}");
    assert!(!names.contains(&"dropped".to_owned()), "{names:?}");
    let made: Id =
        sqlx::query_scalar("SELECT (plan->>'new_environment')::uuid FROM jobs WHERE id=$1")
            .bind(job.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    let config: serde_json::Value =
        sqlx::query_scalar("SELECT config FROM environments WHERE id=$1")
            .bind(made)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        config["env.mounts"][dropped.to_string()],
        json!(false),
        "whatever makes its unit again leaves it detached too"
    );
    world.engine().stop();
}

#[tokio::test]
async fn a_plan_says_which_mcp_servers_a_new_environment_keeps_and_which_stay_behind() {
    use orch_api_contract::forks::{ForkRequest, McpFate};
    use orch_db::queries::resources::mcp::{self as servers, McpRow};
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "mcp-fork", 16_384).await;
    let template = template(&world).await;
    // The source's unit stays connected: a thread bound to it is served there while the plan is made.
    let (environment, _, _source_unit) = running(&world, &host, template, "Source").await;
    let row = |name: &str, own: Option<Id>| McpRow {
        imported: false,
        id: new_id(),
        name: name.into(),
        description: String::new(),
        org_id: Some(world.root),
        home_id: Some(world.root),
        owner_id: Some(world.ada),
        machine_id: None,
        environment_id: own,
        spec: json!({"transport":"stdio","command":["tool"]}),
        enabled: true,
        revision: 1,
    };
    let preset = row("team-tool", None);
    let trial = row("trial-tool", Some(environment));
    let mut conn = world.pool.acquire().await.unwrap();
    servers::insert(&mut conn, &preset, world.clock.now())
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::McpServer.into(),
            id: preset.id,
        },
        world.ada,
        NewAccess::OnlyMe,
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    // An environment's own server has no page of its own: it is added to the environment and switched on there.
    servers::insert(&mut conn, &trial, world.clock.now())
        .await
        .unwrap();
    servers::enable(&mut conn, environment, trial.id, true, world.clock.now())
        .await
        .unwrap();
    drop(conn);
    let enabled: serde_json::Map<String, serde_json::Value> =
        std::iter::once((preset.id.to_string(), json!(true))).collect();
    world.set(world.topic, "mcp.enabled", json!(enabled)).await;
    let thread = world.thread("Servers fork").await;
    bind(&world, thread, environment).await;
    let request: ForkRequest = serde_json::from_value(json!({"result":"new","history":{"mode":"none"},
        "environment":{"new":{"name":"With servers","template_id":template,"host_id":host.id,"files":{"mode":"none"},"home":false,"private_views":false}}}))
    .unwrap();
    let draft = world
        .engine()
        .draft_fork(&Caller::session(world.ada), thread, &request)
        .await
        .unwrap();
    let fate = |id: Id| {
        draft
            .plan
            .mcp
            .iter()
            .find(|server| server.id == id)
            .map(|server| (server.fate, server.own))
    };
    assert_eq!(
        fate(preset.id),
        Some((McpFate::Kept, false)),
        "the place switches it on there too: {:?}",
        draft.plan.mcp
    );
    assert_eq!(
        fate(trial.id),
        Some((McpFate::Lost, true)),
        "an environment's own server does not go with a fork into another"
    );
    world.engine().stop();
}
