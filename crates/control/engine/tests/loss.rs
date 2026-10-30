//! Permanent loss ends unreachable work atomically without pretending physical files were deleted.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::World;
use orch_api_contract::loss::{LossDeclareBody, LossTarget};
use orch_authz::{Caller, Principal};
use orch_db::queries::{environments::hosting as envs, jobs, loss};
use orch_types::{Time, new_id};
use serde_json::json;

async fn principal(world: &World, conn: &mut orch_db::Conn) -> Principal {
    Principal::load(
        conn,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn a_lost_host_closes_jobs_promises_and_locations_in_the_same_transaction() {
    let world = World::new().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "Gone", 16_384).await;
    let template = hosting::template(&world).await;
    let (environment, runtime, connector) = hosting::running(&world, &host, template, "Main").await;
    let thread = world.thread("Work").await;
    world.engine().stop();
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(environment)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET online=false WHERE id=$1")
        .bind(host.machine)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET removal_phase='kept',retained_bytes=4096,retained_volumes=$2,volumes_purge_at=$3 WHERE id=$1").bind(environment).bind(json!([{"host_id":host.id,"name":"kept"}])).bind(world.clock.now()+chrono::Duration::days(7)).execute(&mut *tx).await.unwrap();
    let job = jobs::insert(
        &mut tx,
        &jobs::NewJob {
            id: new_id(),
            kind: jobs::kind::ENVIRONMENT_REMOVE,
            node_id: None,
            environment_id: Some(environment),
            provider_id: None,
            plan: &json!({}),
            progress: &json!({"phase":"remove"}),
            created_by: Some(world.ada),
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    let count_job = jobs::insert(
        &mut tx,
        &jobs::NewJob {
            id: new_id(),
            kind: orch_engine::jobs::measurement::KIND,
            node_id: None,
            environment_id: None,
            provider_id: None,
            plan: &json!({"machine":host.machine,"subject":{"environment_id":environment}}),
            progress: &json!({"phase":"measure"}),
            created_by: Some(world.ada),
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    let comparison_job = jobs::insert(
        &mut tx,
        &jobs::NewJob {
            id: new_id(),
            kind: orch_engine::jobs::measurement::KIND,
            node_id: None,
            environment_id: None,
            provider_id: None,
            plan: &json!({"machine":new_id(),"body":{"target_machine":host.machine},"subject":{"environment_id":new_id()}}),
            progress: &json!({"phase":"fingerprint_target"}),
            created_by: Some(world.ada),
        },
        world.clock.now(),
    ).await.unwrap();
    let who = principal(&world, &mut tx).await;
    let plan = world
        .engine()
        .plan_loss(&mut tx, &who, LossTarget::Host { id: host.id })
        .await
        .unwrap();
    assert_eq!(plan.environments.len(), 1);
    assert_eq!(plan.environments[0].threads.len(), 1);
    assert_eq!(plan.kept_bytes, Some(4096));
    tx.commit().await.unwrap();
    let calls = host.calls().len();
    let body = LossDeclareBody {
        notify: true,
        preflight_id: plan.preflight_id,
        confirm: "Gone".into(),
    };
    // A caller's failed transaction must leave every promise and job intact.
    let mut tx = world.pool.begin().await.unwrap();
    world
        .engine()
        .declare_loss(&mut tx, &who, &body)
        .await
        .unwrap();
    assert_eq!(
        envs::environment(&mut tx, environment)
            .await
            .unwrap()
            .unwrap()
            .status,
        "lost"
    );
    tx.rollback().await.unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    assert_eq!(
        envs::environment(&mut tx, environment)
            .await
            .unwrap()
            .unwrap()
            .status,
        "running"
    );
    assert_eq!(
        jobs::job(&mut tx, job.id).await.unwrap().unwrap().state,
        "queued"
    );
    assert_eq!(
        jobs::job(&mut tx, count_job.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "queued"
    );
    assert_eq!(
        jobs::job(&mut tx, comparison_job.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "queued"
    );
    world
        .engine()
        .declare_loss(&mut tx, &who, &body)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    assert_eq!(host.calls().len(), calls, "declaration contacts no host");
    assert!(loss::machine_lost(&mut conn, host.machine).await.unwrap());
    assert!(
        !orch_db::queries::resources::machine(&mut conn, connector.id)
            .await
            .unwrap()
            .unwrap()
            .is_active()
    );
    let runtime = envs::runtime(&mut conn, runtime).await.unwrap().unwrap();
    assert_eq!(runtime.state, "retired");
    assert_eq!(runtime.remove_at, None);
    let removal = orch_db::queries::environments::retained::removal(&mut conn, environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(removal.removal_phase.as_deref(), Some("lost"));
    assert_eq!(removal.retained_bytes, Some(4096));
    assert_eq!(removal.purged_at, None, "loss is never claimed as deletion");
    assert_eq!(
        jobs::job(&mut conn, job.id).await.unwrap().unwrap().state,
        "lost"
    );
    assert_eq!(
        jobs::job(&mut conn, count_job.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "lost",
        "a count waiting on a lost source also ends without claiming a completed measurement"
    );
    assert_eq!(
        jobs::job(&mut conn, comparison_job.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "lost",
        "a comparison also depends on its target, even when its source belongs elsewhere"
    );
    assert_eq!(
        world.state(thread).await.1.as_deref(),
        Some("environment_unavailable")
    );
    assert!(!loss::accepts_threads(&mut conn, environment).await.unwrap());
    let (units, mounts) = orch_db::queries::environments::changes::host_dependencies(
        &mut conn,
        host.id,
        host.machine,
    )
    .await
    .unwrap();
    assert!(
        units.is_empty() && mounts.is_empty(),
        "the lost host can now be deleted"
    );
    let notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id=$1 AND data->>'loss'='true'",
    )
    .bind(world.ada)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        notices, 1,
        "one recover item for an owner who is also a manager"
    );
    let notice: serde_json::Value = sqlx::query_scalar(
        "SELECT data FROM notifications WHERE user_id=$1 AND data->>'loss'='true'",
    )
    .bind(world.ada)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(notice["thread_count"], 1);
    assert!(
        notice.get("thread_ids").is_none(),
        "grouped counts reveal no private thread identifiers"
    );
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='host.lost'",
    )
    .bind(host.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(audit, 1);
    drop(conn);
    world.shutdown().await;
}

#[tokio::test]
async fn a_changed_inventory_and_a_live_machine_refuse_loss() {
    let world = World::new().await;
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("UPDATE machines SET owner_id=$2,home_id=$3,org_id=$3 WHERE id=$1")
        .bind(world.machine_id)
        .bind(world.ada)
        .bind(world.root)
        .execute(&mut *tx)
        .await
        .unwrap();
    orch_authz::ownership::start_resource(
        &mut tx,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Machine,
            id: world.machine_id,
        },
        world.ada,
        orch_authz::ownership::NewAccess::OnlyMe,
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let who = principal(&world, &mut tx).await;
    let target = LossTarget::Machine {
        id: world.machine_id,
    };
    assert!(matches!(
        world.engine().plan_loss(&mut tx, &who, target).await,
        Err(orch_engine::loss::LossError::Online)
    ));
    sqlx::query("UPDATE machines SET online=false WHERE id=$1")
        .bind(world.machine_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let plan = world
        .engine()
        .plan_loss(&mut tx, &who, target)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET revision=revision+1 WHERE id=$1")
        .bind(world.environment)
        .execute(&mut *tx)
        .await
        .unwrap();
    let body = LossDeclareBody {
        notify: true,
        preflight_id: plan.preflight_id,
        confirm: plan.name,
    };
    assert!(matches!(
        world.engine().declare_loss(&mut tx, &who, &body).await,
        Err(orch_engine::loss::LossError::Stale)
    ));
    tx.rollback().await.unwrap();
    world.shutdown().await;
}

#[tokio::test]
async fn an_environment_loss_keeps_its_shared_physical_machine_registered() {
    let world = World::new().await;
    let mut tx = world.pool.begin().await.unwrap();
    let who = principal(&world, &mut tx).await;
    sqlx::query("UPDATE machines SET online=false WHERE id=$1")
        .bind(world.machine_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let plan = world
        .engine()
        .plan_loss(
            &mut tx,
            &who,
            LossTarget::Environment {
                id: world.environment,
            },
        )
        .await
        .unwrap();
    world
        .engine()
        .declare_loss(
            &mut tx,
            &who,
            &LossDeclareBody {
                notify: true,
                preflight_id: plan.preflight_id,
                confirm: plan.name,
            },
        )
        .await
        .unwrap();
    assert!(!loss::machine_lost(&mut tx, world.machine_id).await.unwrap());
    assert!(
        !loss::accepts_threads(&mut tx, world.environment)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    world.shutdown().await;
}

#[tokio::test]
async fn an_environment_loss_keeps_reachable_retired_copies_on_other_hosts() {
    let world = World::new().await;
    hosting::serve(&world);
    let source = hosting::ScriptedHost::on(&world, "Current", 16_384).await;
    let previous = hosting::ScriptedHost::on(&world, "Previous", 16_384).await;
    let template = hosting::template(&world).await;
    let (environment, current, connector) =
        hosting::running(&world, &source, template, "Main").await;
    let (_, retired, old_connector) =
        hosting::running(&world, &previous, template, "Earlier").await;
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("UPDATE runtimes SET generation=2,loss_missing=true WHERE id=$1")
        .bind(current)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET generation=2 WHERE id=$1")
        .bind(environment)
        .execute(&mut *tx)
        .await
        .unwrap();
    let deadline = world.clock.now() + chrono::Duration::days(1);
    sqlx::query("UPDATE runtimes SET environment_id=$2,state='retired',remove_at=$3 WHERE id=$1")
        .bind(retired)
        .bind(environment)
        .bind(deadline)
        .execute(&mut *tx)
        .await
        .unwrap();
    let deadline = envs::runtime(&mut tx, retired)
        .await
        .unwrap()
        .unwrap()
        .remove_at
        .unwrap();
    let who = principal(&world, &mut tx).await;
    let plan = world
        .engine()
        .plan_loss(&mut tx, &who, LossTarget::Environment { id: environment })
        .await
        .unwrap();
    world
        .engine()
        .declare_loss(
            &mut tx,
            &who,
            &LossDeclareBody {
                notify: false,
                preflight_id: plan.preflight_id,
                confirm: plan.name,
            },
        )
        .await
        .unwrap();
    assert!(loss::machine_lost(&mut tx, connector.id).await.unwrap());
    assert!(!loss::machine_lost(&mut tx, old_connector.id).await.unwrap());
    assert!(!loss::machine_lost(&mut tx, source.machine).await.unwrap());
    let retained = envs::runtime(&mut tx, retired).await.unwrap().unwrap();
    assert_eq!(retained.state, "retired");
    assert_eq!(retained.remove_at, Some(deadline));
    let retained_is_reachable: bool =
        sqlx::query_scalar("SELECT lost_at IS NULL FROM runtimes WHERE id=$1")
            .bind(retired)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(retained_is_reachable);
    tx.commit().await.unwrap();
    world.shutdown().await;
}

#[tokio::test]
async fn re_creation_uses_the_same_environment_copies_only_held_files_and_writes_the_missing_list()
{
    use orch_engine::manifest::{self, Entry, EntryKind, Header, MANIFEST_VERSION};
    use orch_link_contract::commands::{
        Command, CommandKind, CommandResult, FsMaterializeResult, NodeVolumeInspectResult,
    };
    use orch_types::Hash;
    use std::sync::{Arc, Mutex};
    let world = World::on_a_key("a-test-key").await;
    world
        .set(world.topic, "binding.prefer", json!(["key_separated"]))
        .await;
    hosting::serve(&world);
    let source = hosting::ScriptedHost::on(&world, "Source", 16_384).await;
    let target = hosting::ScriptedHost::on(&world, "Target", 16_384).await;
    let template = hosting::template_of(&world, &["orch-exec"], &["fake"]).await;
    let (environment, old, _) = hosting::running(&world, &source, template, "Main").await;
    let thread = world.thread("Recoverable work").await;
    let store = Arc::new(
        orch_blob_local::LocalBlobStore::open(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../.tmp/loss-blobs")
                .join(new_id().to_string()),
        )
        .await
        .unwrap(),
    );
    world.engine().attach_blobs(store);
    let second_thread = world.thread("Another recoverable conversation").await;
    for id in [thread, second_thread] {
        world
            .engine()
            .submit_input(
                &Caller::session(world.ada),
                id,
                common::queued("Remember this before losing the host"),
            )
            .await
            .unwrap();
        world.run_turn(id, 0, "Recorded reply before loss").await;
    }
    target.answer(CommandKind::NodeVolumeInspect, |_| {
        Ok(CommandResult::NodeVolumeInspect(NodeVolumeInspectResult {
            exists: false,
            bytes: None,
        }))
    });
    let mut conn = world.pool.acquire().await.unwrap();
    let held = world
        .engine()
        .put_manifest(&mut conn, b"kept bytes".to_vec())
        .await
        .unwrap()
        .hash;
    let now = world.clock.now();
    let entry = |path: &str, hash: Hash, size: i64| Entry {
        path: path.into(),
        kind: EntryKind::File,
        size,
        mtime_ns: 0,
        mode: 0o644,
        hash: Some(hash),
        link: None,
    };
    let original = manifest::build(
        &Header {
            v: MANIFEST_VERSION,
            root: "/workspace".into(),
            generated_at: Time::new(now - chrono::Duration::hours(1)),
            generation: 1,
            case_sensitive: true,
        },
        vec![
            entry("present.txt", held, 10),
            entry("missing.txt", Hash::of(b"absent"), 6),
        ],
    )
    .unwrap();
    let original = world
        .engine()
        .put_manifest(&mut conn, original.bytes)
        .await
        .unwrap()
        .hash;
    sqlx::query("UPDATE environments SET manifest_blob=$2,manifest_at=$3 WHERE id=$1")
        .bind(environment)
        .bind(original.as_bytes().as_slice())
        .bind(now)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=ANY($1)")
        .bind(&[thread, second_thread][..])
        .bind(environment)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE segments SET environment_id=$2,env_generation=1 WHERE thread_id=ANY($1)")
        .bind(&[thread, second_thread][..])
        .bind(environment)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET online=false WHERE id=$1")
        .bind(source.machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let mut tx = world.pool.begin().await.unwrap();
    let who = principal(&world, &mut tx).await;
    let plan = world
        .engine()
        .plan_loss(&mut tx, &who, LossTarget::Host { id: source.id })
        .await
        .unwrap();
    world
        .engine()
        .declare_loss(
            &mut tx,
            &who,
            &LossDeclareBody {
                preflight_id: plan.preflight_id,
                confirm: plan.name,
                notify: false,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let before = source.calls().len();
    let mut tx = world.pool.begin().await.unwrap();
    let prepared = world
        .engine()
        .prepare_recovery(&mut tx, &who, environment, Some(target.id))
        .await
        .unwrap();
    assert_eq!(
        target.count(CommandKind::NodeVolumeInspect),
        0,
        "database planning makes no machine call"
    );
    tx.rollback().await.unwrap();
    let draft = world.engine().measure_recovery(prepared).await.unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    assert_eq!(draft.files.file_count, 2);
    assert_eq!(
        draft.files.source_as_of,
        Time::new(now - chrono::Duration::hours(1))
    );
    assert_eq!(draft.files.missing.count, 1);
    assert_eq!(draft.files.missing.sample, ["missing.txt"]);
    let selected = world
        .engine()
        .read_manifest(&draft.recipe.manifest.unwrap())
        .await
        .unwrap();
    assert!(selected.get("present.txt").is_some());
    assert!(selected.get("missing.txt").is_none());
    let job =
        orch_engine::recovery::queue(&world.engine(), &mut tx, &draft.recipe, Some(new_id()), now)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    target.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let runtime = target.started().unwrap();
    let connector = hosting::enroll_holding(&world, runtime, CommandKind::EnvAttach).await;
    let materialized = Arc::new(Mutex::new(Vec::new()));
    let captured = materialized.clone();
    connector
        .added
        .machine
        .handler
        .on(CommandKind::FsMaterialize, move |command| {
            let Command::FsMaterialize(body) = command else {
                unreachable!()
            };
            captured.lock().unwrap().push(body.manifest_blob);
            Ok(CommandResult::FsMaterialize(FsMaterializeResult {
                files: 1,
                bytes: 10,
                skipped: 0,
            }))
        });
    connector.let_go();
    let result = hosting::wait_over(&world, job.id).await;
    assert_eq!(result.0, "succeeded", "{result:?}");
    let mut conn = world.pool.acquire().await.unwrap();
    let recovered = envs::environment(&mut conn, environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.id, environment);
    assert_eq!(recovered.generation, 2);
    assert_eq!(recovered.status, "running");
    assert_eq!(recovered.machine_id, Some(connector.id));
    let completed = orch_db::queries::jobs::job(&mut conn, job.id)
        .await
        .unwrap()
        .unwrap();
    let histories = completed.plan["recovery_histories"].as_object().unwrap();
    assert_eq!(
        histories.len(),
        2,
        "one job keeps each thread's Carry supplier"
    );
    assert_eq!(completed.plan["recovery_history_pins_released"], true);
    for id in [thread, second_thread] {
        let history = &histories[&id.to_string()];
        assert!(
            history["notice"]
                .as_str()
                .unwrap()
                .contains("Recorded reply before loss")
        );
        let hash: Hash = serde_json::from_value(history["blob"].clone()).unwrap();
        assert_eq!(
            orch_db::queries::storage::blob(&mut conn, &hash)
                .await
                .unwrap()
                .unwrap()
                .pin_count,
            0,
            "terminal recovery releases exactly its recorded-content pins"
        );
        let row = orch_db::queries::threads::thread(&mut conn, id)
            .await
            .unwrap()
            .unwrap();
        let segment =
            orch_db::queries::threads::segment(&mut conn, row.current_segment_id.unwrap())
                .await
                .unwrap()
                .unwrap();
        assert!(
            segment.native_session_ref.is_none(),
            "a fresh home continues from the record"
        );
    }
    assert_eq!(
        envs::runtime(&mut conn, old).await.unwrap().unwrap().state,
        "retired"
    );
    assert_eq!(
        source.calls().len(),
        before,
        "no source command is required after permanent loss"
    );
    let hashes = materialized.lock().unwrap().clone();
    let mut missing_list = false;
    for hash in hashes {
        let written = world.engine().read_manifest(&hash).await.unwrap();
        if let Some(entry) = written.get("handoff/missing-files.txt") {
            assert_eq!(entry.hash, Some(Hash::of(b"missing.txt\n")));
            missing_list = true;
        }
    }
    assert!(
        missing_list,
        "the missing file list is materialized in the thread's handoff directory"
    );
    drop(conn);
    world.shutdown().await;
}

#[tokio::test]
async fn an_unconfirmed_input_stays_failed_with_its_text_and_the_lost_turn_is_stamped() {
    let world = World::new().await;
    let thread = world.thread("Interrupted delivery").await;
    let submitted = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            common::queued("Keep these exact words"),
        )
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    assert_eq!(world.input_state(submitted.input.id).await.0, "delivering");
    world.set_machine_online(world.machine_id, false).await;
    let mut tx = world.pool.begin().await.unwrap();
    let who = principal(&world, &mut tx).await;
    let plan = world
        .engine()
        .plan_loss(
            &mut tx,
            &who,
            LossTarget::Environment {
                id: world.environment,
            },
        )
        .await
        .unwrap();
    world
        .engine()
        .declare_loss(
            &mut tx,
            &who,
            &LossDeclareBody {
                preflight_id: plan.preflight_id,
                confirm: plan.name,
                notify: false,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        world.input_state(submitted.input.id).await,
        ("failed".into(), Some("environment_lost_unconfirmed".into()))
    );
    assert_eq!(world.turn_state(start.turn_id).await, "interrupted");
    let mut conn = world.pool.acquire().await.unwrap();
    let input = orch_db::queries::threads::input(&mut conn, submitted.input.id)
        .await
        .unwrap()
        .unwrap();
    assert!(input.content.to_string().contains("Keep these exact words"));
    let seq: i64 = sqlx::query_scalar("SELECT seq_updated FROM turns WHERE id=$1")
        .bind(start.turn_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert!(seq > 0);
    let notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id=$1 AND data->>'operation_kind'='lost'",
    )
    .bind(world.ada)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(notices, 0);
}

#[tokio::test]
async fn a_stopped_connector_does_not_make_a_healthy_answering_hosts_environment_eligible_for_loss()
{
    let world = World::new().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "Healthy", 16_384).await;
    let template = hosting::template(&world).await;
    let (environment, runtime, connector) =
        hosting::running(&world, &host, template, "Stopped").await;
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("UPDATE machines SET online=false WHERE id=$1")
        .bind(connector.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE runtimes SET state='stopped' WHERE id=$1")
        .bind(runtime)
        .execute(&mut *tx)
        .await
        .unwrap();
    let who = principal(&world, &mut tx).await;
    assert!(matches!(
        world
            .engine()
            .plan_loss(&mut tx, &who, LossTarget::Environment { id: environment })
            .await,
        Err(orch_engine::loss::LossError::Online)
    ));
    assert!(
        loss::eligible_environments(&mut tx, &[environment])
            .await
            .unwrap()
            .is_empty()
    );
    // The node's explicit absent-unit report, independently of its connector, makes loss available.
    sqlx::query("UPDATE runtimes SET loss_missing=true WHERE id=$1")
        .bind(runtime)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        loss::eligible_environments(&mut tx, &[environment])
            .await
            .unwrap(),
        vec![environment]
    );
    assert!(
        world
            .engine()
            .plan_loss(&mut tx, &who, LossTarget::Environment { id: environment })
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn owned_restored_volumes_preserve_the_local_native_session_and_wrong_ownership_blocks_the_plan()
 {
    use orch_link_contract::commands::{
        CommandError, CommandKind, CommandResult, ErrorCode, NodeVolumeInspectResult,
    };
    for wrong in [false, true] {
        let world = World::on_a_key("a-test-key").await;
        world
            .set(world.topic, "binding.prefer", json!(["key_separated"]))
            .await;
        hosting::serve(&world);
        let source = hosting::ScriptedHost::on(&world, "Lost source", 16_384).await;
        let target = hosting::ScriptedHost::on(&world, "Restored target", 16_384).await;
        let template = hosting::template_of(&world, &["orch-exec"], &["fake"]).await;
        let (environment, _, _unit) =
            hosting::running(&world, &source, template, "Native home").await;
        let thread = world.thread("Native conversation").await;
        // A completed native session exists before its environment's home is backed up.
        world
            .engine()
            .submit_input(
                &Caller::session(world.ada),
                thread,
                common::queued("Remember the conversation"),
            )
            .await
            .unwrap();
        world.run_turn(thread, 0, "Recorded reply").await;
        let mut tx = world.pool.begin().await.unwrap();
        let current = orch_db::queries::threads::thread(&mut tx, thread)
            .await
            .unwrap()
            .unwrap()
            .current_segment_id
            .unwrap();
        let old = orch_db::queries::threads::segment(&mut tx, current)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(old.arrangement, "key_separated");
        assert!(old.native_session_ref.is_some());
        sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
            .bind(thread)
            .bind(environment)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("UPDATE segments SET environment_id=$2,env_generation=1 WHERE id=$1")
            .bind(current)
            .bind(environment)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("UPDATE machines SET online=false WHERE id=$1")
            .bind(source.machine)
            .execute(&mut *tx)
            .await
            .unwrap();
        let who = principal(&world, &mut tx).await;
        let plan = world
            .engine()
            .plan_loss(&mut tx, &who, LossTarget::Host { id: source.id })
            .await
            .unwrap();
        world
            .engine()
            .declare_loss(
                &mut tx,
                &who,
                &LossDeclareBody {
                    preflight_id: plan.preflight_id,
                    confirm: plan.name,
                    notify: false,
                },
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        target.answer(CommandKind::NodeVolumeInspect, move |_| {
            if wrong {
                Err(CommandError::new(
                    ErrorCode::Conflict,
                    "A different environment owns this restored volume",
                ))
            } else {
                Ok(CommandResult::NodeVolumeInspect(NodeVolumeInspectResult {
                    exists: true,
                    bytes: Some(4096),
                }))
            }
        });
        let mut tx = world.pool.begin().await.unwrap();
        let prepared = world
            .engine()
            .prepare_recovery(&mut tx, &who, environment, Some(target.id))
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        let draft = world.engine().measure_recovery(prepared).await;
        if wrong {
            assert!(
                matches!(draft,Err(orch_engine::updates::UpdateError::Blocked(ref blockers)) if blockers.iter().any(|blocker|blocker.code=="restored_volume_unavailable"))
            );
            assert_eq!(
                target.count(CommandKind::NodeRuntimeCreate),
                0,
                "ownership is refused before a unit or volume is changed"
            );
            continue;
        }
        let draft = draft.unwrap();
        let mut tx = world.pool.begin().await.unwrap();
        assert_eq!(draft.recipe.restored.len(), 2);
        assert_eq!(draft.recipe.restored.get("home"), Some(&Some(4096)));
        assert_eq!(draft.files.bytes_to_download, 0);
        let job = orch_engine::recovery::queue(
            &world.engine(),
            &mut tx,
            &draft.recipe,
            None,
            world.clock.now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        world.engine().kick();
        target.wait_for(CommandKind::NodeRuntimeStart, 1).await;
        let runtime = target.started().unwrap();
        let connector = hosting::enroll(&world, runtime).await;
        let (state, _, error) = hosting::wait_over(&world, job.id).await;
        assert_eq!(state, "succeeded", "{error:?}");
        let mut conn = world.pool.acquire().await.unwrap();
        let row = orch_db::queries::threads::thread(&mut conn, thread)
            .await
            .unwrap()
            .unwrap();
        let segment =
            orch_db::queries::threads::segment(&mut conn, row.current_segment_id.unwrap())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            segment.native_session_ref, old.native_session_ref,
            "the restored home retains the harness's native resume identity"
        );
        assert_ne!(segment.id, old.id);
        assert_eq!(segment.env_generation, 2);
        assert_eq!(
            connector.count(CommandKind::FsMaterialize),
            0,
            "a restored workspace and home are used as they are"
        );
        assert!(
            target
                .calls()
                .iter()
                .filter(|call| call.kind == CommandKind::NodeVolumeRemove)
                .all(
                    |call| call.body["name"] != draft.recipe.volumes["home"]["name"]
                        && call.body["name"] != draft.recipe.volumes["workspace"]["name"]
                )
        );
    }
}

#[tokio::test]
async fn re_creating_a_lost_provider_checks_the_callers_own_host_use_instead_of_borrowing_its_owners()
 {
    use orch_db::queries::access::{PageKey, testing::add_entry};
    let world = World::on_a_key("a-test-key").await;
    hosting::serve(&world);
    let target = hosting::ScriptedHost::on(&world, "Provider target", 16_384).await;
    let template = hosting::template(&world).await;
    let caller = world.person("responder").await;
    let now = world.clock.now();
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("UPDATE env_templates SET kind='provider' WHERE id=$1")
        .bind(template)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE providers SET runtime_unavailable='Its runtime was declared lost',status='offline',runtime_config=runtime_config||jsonb_build_object('runtime.template_id',$2::uuid::text,'runtime.host_id',$3::uuid::text) WHERE id=$1").bind(world.provider).bind(template).bind(target.id).execute(&mut *tx).await.unwrap();
    for (kind, id, permission) in [
        ("provider", world.provider, "provider.manage"),
        ("template", template, "template.use"),
    ] {
        add_entry(
            &mut tx,
            &PageKey::new(kind, id, "live"),
            "any",
            permission,
            ("user", Some(caller), None),
            None,
            now,
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(&mut tx, Caller::session(caller), Time::new(now))
        .await
        .unwrap();
    assert!(
        matches!(world.engine().draft_provider_recovery(&mut tx,&principal,world.provider).await,Err(orch_engine::updates::UpdateError::Blocked(ref blockers)) if blockers.iter().any(|row|row.code=="no_host"))
    );
    tx.rollback().await.unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    add_entry(
        &mut tx,
        &PageKey::new("host", target.id, "live"),
        "any",
        "host.use",
        ("user", Some(caller), None),
        None,
        now,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM providers WHERE id=$1 FOR UPDATE")
        .bind(world.provider)
        .execute(&mut *tx)
        .await
        .unwrap();
    let principal = Principal::load(&mut tx, Caller::session(caller), Time::new(now))
        .await
        .unwrap();
    let draft = world
        .engine()
        .draft_provider_recovery(&mut tx, &principal, world.provider)
        .await
        .unwrap();
    assert_eq!(draft.recipe.host, target.id);
    assert_eq!(draft.recipe.by, caller);
    assert!(draft.grants.is_empty());
    let job = orch_engine::provider_recovery::queue(
        &world.engine(),
        &mut tx,
        &principal,
        &draft.recipe,
        now,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    target
        .wait_for(
            orch_link_contract::commands::CommandKind::NodeRuntimeCreate,
            1,
        )
        .await;
    let created = target
        .calls()
        .into_iter()
        .find(|call| call.kind == orch_link_contract::commands::CommandKind::NodeRuntimeCreate)
        .unwrap();
    assert!(
        created.body["volumes"]["state"]["name"]
            .as_str()
            .unwrap()
            .contains(&job.id.to_string()),
        "a lost connector state is never reused"
    );
    let row = jobs::job(&mut world.pool.acquire().await.unwrap(), job.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.plan["provider_recovery"]["by"], json!(caller));
}

#[tokio::test]
async fn an_environment_recovery_checks_its_managers_own_host_and_template_access() {
    use orch_db::queries::access::{PageKey, testing::add_entry};
    use orch_engine::updates::UpdateError;
    use orch_link_contract::commands::CommandKind;
    let world = World::new().await;
    hosting::serve(&world);
    let source = hosting::ScriptedHost::on(&world, "Source permissions", 16_384).await;
    let target = hosting::ScriptedHost::on(&world, "Destination permissions", 16_384).await;
    let template = hosting::template(&world).await;
    let (environment, _, _) = hosting::running(&world, &source, template, "Shared recovery").await;
    let caller = world.person("environment-responder").await;
    let now = world.clock.now();
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("UPDATE machines SET online=false WHERE id=$1")
        .bind(source.machine)
        .execute(&mut *tx)
        .await
        .unwrap();
    let owner = principal(&world, &mut tx).await;
    let plan = world
        .engine()
        .plan_loss(&mut tx, &owner, LossTarget::Host { id: source.id })
        .await
        .unwrap();
    world
        .engine()
        .declare_loss(
            &mut tx,
            &owner,
            &LossDeclareBody {
                preflight_id: plan.preflight_id,
                confirm: plan.name,
                notify: false,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    let who = Principal::load(&mut tx, Caller::session(caller), Time::new(now))
        .await
        .unwrap();
    assert!(matches!(
        world
            .engine()
            .prepare_recovery(&mut tx, &who, environment, Some(target.id))
            .await,
        Err(UpdateError::Denied)
    ));
    tx.rollback().await.unwrap();
    for (kind, id, permission) in [
        ("environment", environment, "environment.manage"),
        ("host", target.id, "host.use"),
        ("template", template, "template.use"),
    ] {
        let mut tx = world.pool.begin().await.unwrap();
        add_entry(
            &mut tx,
            &PageKey::new(kind, id, "live"),
            "any",
            permission,
            ("user", Some(caller), None),
            None,
            now,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let mut tx = world.pool.begin().await.unwrap();
        let who = Principal::load(&mut tx, Caller::session(caller), Time::new(now))
            .await
            .unwrap();
        let prepared = world
            .engine()
            .prepare_recovery(&mut tx, &who, environment, Some(target.id))
            .await;
        if kind == "template" {
            assert_eq!(prepared.unwrap().host(), target.id);
        } else {
            assert!(
                matches!(prepared,Err(UpdateError::Blocked(ref rows)) if rows.iter().any(|row|row.code=="no_host")),
                "management cannot borrow the environment owner's destination grants"
            );
        }
        tx.rollback().await.unwrap();
    }
    assert_eq!(
        target.count(CommandKind::NodeVolumeInspect),
        0,
        "permission planning sends no native probe"
    );
    world.shutdown().await;
}

#[tokio::test]
async fn several_lost_environments_obey_the_batch_limit_and_one_failure_does_not_fail_the_others() {
    use orch_link_contract::commands::{
        CommandError, CommandKind, CommandResult, ErrorCode, NodeVolumeInspectResult,
    };
    let world = World::new().await;
    hosting::serve(&world);
    sqlx::query("INSERT INTO settings (node_id,key,value) VALUES(NULL,'switch.batch_parallel','1') ON CONFLICT(node_id,key) DO UPDATE SET value='1'")
        .execute(world.pool.pool()).await.unwrap();
    let source = hosting::ScriptedHost::on(&world, "Batch source", 16_384).await;
    let target = hosting::ScriptedHost::on(&world, "Batch target", 16_384).await;
    let template = hosting::template(&world).await;
    let mut environments = Vec::new();
    for name in ["First", "Second", "Third"] {
        environments.push(hosting::running(&world, &source, template, name).await.0);
    }
    world.set_machine_online(source.machine, false).await;
    let mut tx = world.pool.begin().await.unwrap();
    let principal = principal(&world, &mut tx).await;
    let plan = world
        .engine()
        .plan_loss(&mut tx, &principal, LossTarget::Host { id: source.id })
        .await
        .unwrap();
    world
        .engine()
        .declare_loss(
            &mut tx,
            &principal,
            &LossDeclareBody {
                preflight_id: plan.preflight_id,
                confirm: plan.name,
                notify: false,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    target.answer(CommandKind::NodeVolumeInspect, |_| {
        Ok(CommandResult::NodeVolumeInspect(NodeVolumeInspectResult {
            exists: false,
            bytes: None,
        }))
    });
    // Each recovered environment lacks a source listing; its ordinary missing-list artifact has held bytes.
    let store = std::sync::Arc::new(
        orch_blob_local::LocalBlobStore::open(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../.tmp/loss-batch-blobs")
                .join(new_id().to_string()),
        )
        .await
        .unwrap(),
    );
    world.engine().attach_blobs(store);
    target.refuse(
        CommandKind::NodeImageEnsure,
        CommandError::new(ErrorCode::NotFound, "This member's image is unavailable"),
    );
    let batch = new_id();
    let mut queued = Vec::new();
    let mut tx = world.pool.begin().await.unwrap();
    let mut prepared = Vec::new();
    for environment in environments {
        prepared.push(
            world
                .engine()
                .prepare_recovery(&mut tx, &principal, environment, Some(target.id))
                .await
                .unwrap(),
        );
    }
    tx.rollback().await.unwrap();
    let mut drafts = Vec::new();
    for prepared in prepared {
        drafts.push(world.engine().measure_recovery(prepared).await.unwrap());
    }
    let mut tx = world.pool.begin().await.unwrap();
    for draft in drafts {
        queued.push(
            orch_engine::recovery::queue(
                &world.engine(),
                &mut tx,
                &draft.recipe,
                Some(batch),
                world.clock.now(),
            )
            .await
            .unwrap()
            .id,
        );
    }
    tx.commit().await.unwrap();
    world.engine().kick();
    let enrolled = std::sync::Mutex::new(std::collections::BTreeSet::new());
    let connectors = std::sync::Mutex::new(Vec::new());
    let observed = std::sync::Mutex::new(serde_json::Value::Null);
    hosting::await_event(
        "independent recoveries finish",
        world.engine().config().hosting.enroll_timeout
            + world.engine().config().hosting.attach_wait,
        || async {
            let mut conn = world.pool.acquire().await.unwrap();
            let rows: Vec<(String, String, serde_json::Value)> = sqlx::query_as(
                "SELECT id::text,state,plan||jsonb_build_object('_observed_progress',progress,'_observed_error',error) FROM jobs WHERE batch_id=$1 ORDER BY id",
            )
            .bind(batch)
            .fetch_all(&mut *conn)
            .await
            .unwrap();
            let snapshot = json!(rows.iter().map(|(id,state,plan)| json!({
                "id":id,"state":state,"runtime":plan.get("runtime"),
                "phase":plan.pointer("/_observed_progress/phase"),"error":plan.get("_observed_error"),
            })).collect::<Vec<_>>());
            {
                let mut previous = observed.lock().unwrap();
                if *previous != snapshot {
                    eprintln!("Recovery member states: {snapshot}");
                    *previous = snapshot;
                }
            }
            assert!(
                rows.iter()
                    .filter(|(_, state, _)| state == "running")
                    .count()
                    <= 1,
                "the configured batch limit is preserved"
            );
            for (_, state, plan) in &rows {
                if state != "running" {
                    continue;
                }
                let Some(runtime) = plan["runtime"]
                    .as_str()
                    .and_then(|id| id.parse::<orch_types::Id>().ok())
                else {
                    continue;
                };
                if !target.calls().iter().any(|call| {
                    call.kind == CommandKind::NodeRuntimeStart
                        && call.body["runtime_id"] == json!(runtime)
                }) || !enrolled.lock().unwrap().insert(runtime)
                {
                    continue;
                }
                let connector =
                    hosting::enroll_holding(&world, runtime, CommandKind::EnvAttach).await;
                connector
                    .added
                    .machine
                    .handler
                    .on(CommandKind::FsMaterialize, |_| {
                        Ok(CommandResult::FsMaterialize(
                            orch_link_contract::commands::FsMaterializeResult {
                                files: 1,
                                bytes: 1,
                                skipped: 0,
                            },
                        ))
                    });
                connector.let_go();
                connectors.lock().unwrap().push(connector);
            }
            rows.len() == 3
                && rows
                    .iter()
                    .all(|(_, state, _)| matches!(state.as_str(), "succeeded" | "failed"))
        },
    )
    .await;
    let mut states = Vec::new();
    for id in queued {
        states.push(
            jobs::job(&mut world.pool.acquire().await.unwrap(), id)
                .await
                .unwrap()
                .unwrap()
                .state,
        );
    }
    assert_eq!(states.iter().filter(|state| *state == "failed").count(), 1);
    assert_eq!(
        states.iter().filter(|state| *state == "succeeded").count(),
        2
    );
    world.shutdown().await;
}

#[tokio::test]
async fn a_lost_or_still_recovering_default_cannot_receive_a_new_thread() {
    let world = World::new().await;
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("UPDATE environments SET status='lost',lost_at=$2 WHERE id=$1")
        .bind(world.environment)
        .bind(world.clock.now())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for status in ["lost", "provisioning"] {
        sqlx::query("UPDATE environments SET status=$2 WHERE id=$1")
            .bind(world.environment)
            .bind(status)
            .execute(world.pool.pool())
            .await
            .unwrap();
        assert!(
            !loss::accepts_threads(&mut world.pool.acquire().await.unwrap(), world.environment)
                .await
                .unwrap()
        );
        let result = world
            .engine()
            .create_thread(
                &Caller::session(world.ada),
                orch_engine::binding::NewThread {
                    topic: world.topic,
                    name: "New work".into(),
                    agent: world.agent,
                    provider: world.provider,
                    environment: world.environment,
                    directory: "new".into(),
                    arrangement: None,
                    model: Some(common::MODEL.into()),
                    effort: None,
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(orch_engine::binding::CreateThreadError::EnvironmentUnavailable)
        ));
    }
}

#[tokio::test]
async fn default_attention_follows_the_effective_inherited_lock_and_ignores_fresh_topics() {
    let world = World::new().await;
    world
        .set(world.project, "env.default_id", json!(world.environment))
        .await;
    let other = new_id();
    world.set(world.topic, "env.default_id", json!(other)).await;
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        loss::default_topics(&mut conn, &[world.environment])
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("UPDATE settings SET locked=true WHERE node_id=$1 AND key='env.default_id'")
        .bind(world.project)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        loss::default_topics(&mut conn, &[world.environment])
            .await
            .unwrap(),
        vec![(world.environment, world.topic)]
    );
    drop(conn);
    world
        .set(world.topic, "env.thread_default", json!("fresh"))
        .await;
    assert!(
        loss::default_topics(
            &mut world.pool.acquire().await.unwrap(),
            &[world.environment]
        )
        .await
        .unwrap()
        .is_empty()
    );
}

#[tokio::test]
async fn host_deletion_names_its_remaining_environment_wait_and_cancellation_keeps_that_work() {
    use futures::FutureExt;
    let world = World::new().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "Waiting host", 16_384).await;
    let template = hosting::template(&world).await;
    let (environment, _, _unit) =
        hosting::running(&world, &host, template, "Still managed elsewhere").await;
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("UPDATE hosts SET schedulable=false WHERE id=$1")
        .bind(host.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let recipe = orch_engine::jobs::host_delete::Recipe {
        host: host.id,
        machine: host.machine,
        by: world.ada,
        token: None,
        schedulable: true,
        children: vec![],
        delete_volumes: vec![],
        admin_override: false,
        custody: None,
    };
    let parent = jobs::insert(
        &mut tx,
        &jobs::NewJob {
            id: new_id(),
            kind: orch_engine::jobs::host_delete::KIND,
            node_id: None,
            environment_id: None,
            provider_id: None,
            created_by: Some(world.ada),
            plan: &json!({"cascade":recipe}),
            progress: &serde_json::to_value(orch_engine::jobs::runner::initial_progress(
                &orch_engine::jobs::host_delete::PHASES,
            ))
            .unwrap(),
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    let waited = std::panic::AssertUnwindSafe(common::eventually(
        "the host deletion says which managers it waits for",
        || async {
            let row = jobs::job(&mut world.pool.acquire().await.unwrap(), parent.id)
                .await
                .unwrap();
            assert!(
                !row.as_ref().is_some_and(|job| matches!(
                    job.state.as_str(),
                    "failed" | "cancelled" | "succeeded" | "lost"
                )),
                "the wait ended before its remaining environment: {row:?}"
            );
            row.is_some_and(|job| {
                job.progress
                    .pointer("/activity/waiting_for/type")
                    .and_then(serde_json::Value::as_str)
                    == Some("host_environments")
            })
        },
    ))
    .catch_unwind()
    .await;
    if let Err(error) = waited {
        let row = jobs::job(&mut world.pool.acquire().await.unwrap(), parent.id)
            .await
            .unwrap()
            .unwrap();
        eprintln!(
            "host wait ended: state={} progress={} error={:?}",
            row.state, row.progress, row.error
        );
        std::panic::resume_unwind(error);
    }
    let row = jobs::job(&mut world.pool.acquire().await.unwrap(), parent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.progress.pointer("/activity/waiting_for/count"),
        Some(&json!(1))
    );
    assert_eq!(
        host.count(orch_link_contract::commands::CommandKind::NodeRuntimeRemove),
        0,
        "waiting does not remove another environment"
    );
    assert!(
        world
            .engine()
            .cancel_job(parent.id, world.ada)
            .await
            .unwrap()
    );
    common::eventually("cancelling the wait settles the parent", || async {
        jobs::job(&mut world.pool.acquire().await.unwrap(), parent.id)
            .await
            .unwrap()
            .is_some_and(|job| job.state == "cancelled")
    })
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let host_row = orch_db::queries::resources::hosting::host(&mut conn, host.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        host_row.schedulable,
        "a safe cancellation restores the original scheduling choice"
    );
    let state: String = sqlx::query_scalar("SELECT status FROM environments WHERE id=$1")
        .bind(environment)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(state, "running");
    assert_eq!(
        host.count(orch_link_contract::commands::CommandKind::NodeRuntimeRemove),
        0
    );
    world.engine().stop();
}

async fn delete_host_data_case(erase: bool, lost: bool, refuse: bool) {
    use orch_api_contract::loss::{HostDataFate, HostDataRow, LossResource};
    use orch_driver_contract::resources::{Resource, ResourceInventory, ResourceKind, ResourceRef};
    use orch_engine::jobs::host_delete::{Custody, KIND, NativeData, PHASES, Recipe};
    use orch_link_contract::commands::{
        Command, CommandError, CommandKind, CommandResult, Empty, ErrorCode,
    };
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };
    let world = World::new().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "Departing host", 16_384).await;
    let item = |kind, id: &str, busy, depends_on: Vec<ResourceRef>| Resource {
        resource: ResourceRef {
            kind,
            id: id.into(),
        },
        name: id.into(),
        size_bytes: Some(4),
        labels: BTreeMap::new(),
        depends_on,
        aliases: vec![],
        last_used_at: None,
        busy,
    };
    let image = item(ResourceKind::Image, "image", false, vec![]);
    let keep = item(ResourceKind::Volume, "kept-data", false, vec![]);
    let remove_data = item(ResourceKind::Volume, "delete-data", false, vec![]);
    let layer = item(ResourceKind::KeptLayer, "kept-layer", false, vec![]);
    let tool = item(ResourceKind::ToolVersion, "tool", false, vec![]);
    let unit = item(
        ResourceKind::Unit,
        "unit",
        true,
        vec![image.resource.clone(), keep.resource.clone()],
    );
    let observed = ResourceInventory {
        observed_at: Time::new(world.clock.now()),
        resources: vec![
            unit,
            image,
            keep.clone(),
            remove_data.clone(),
            layer.clone(),
            tool,
        ],
        unavailable: vec![],
    };
    let inventory = Arc::new(Mutex::new(observed.clone()));
    let listing = inventory.clone();
    host.answer(CommandKind::NodeResourcesList, move |_| {
        Ok(CommandResult::NodeResourcesList(
            listing.lock().unwrap().clone(),
        ))
    });
    let changing = inventory.clone();
    host.answer(CommandKind::NodeResourcesRemove, move |command| {
        let Command::NodeResourcesRemove(body) = command else {
            panic!("not a resource removal");
        };
        assert!(
            !matches!(body.id.as_str(), "kept-data" | "kept-layer"),
            "erasure never selects Leave data"
        );
        assert_eq!(body.stop_first, body.kind == ResourceKind::Unit);
        if refuse && body.id == "image" {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the native runtime still needs this image",
            ));
        }
        let mut current = changing.lock().unwrap();
        assert!(
            !current
                .resources
                .iter()
                .any(|object| object.depends_on.contains(&body.reference())),
            "native dependencies are removed first"
        );
        current
            .resources
            .retain(|object| object.resource != body.reference());
        Ok(CommandResult::NodeResourcesRemove(Empty {}))
    });
    let data_row = |object: &Resource, fate| HostDataRow {
        id: format!("native:{}", object.resource.id),
        resource: LossResource {
            id: host.id,
            name: Some("Departing host".into()),
        },
        label: Some(object.name.clone()),
        kind: if object.resource.kind == ResourceKind::Volume {
            "volume"
        } else {
            "kept_layer"
        }
        .into(),
        bytes: object.size_bytes,
        until: None,
        selected: fate,
        choices: vec![HostDataFate::Leave, HostDataFate::Delete],
        host: None,
        copy: None,
        blockers: vec![],
    };
    let custody = Custody {
        erase,
        mounts: vec![],
        copies: Default::default(),
        native_data: vec![
            NativeData {
                resource: keep.resource.clone(),
                row: data_row(&keep, HostDataFate::Leave),
            },
            NativeData {
                resource: remove_data.resource.clone(),
                row: data_row(&remove_data, HostDataFate::Delete),
            },
            NativeData {
                resource: layer.resource.clone(),
                row: data_row(&layer, HostDataFate::Leave),
            },
        ],
        uninstall_command: None,
        frozen_inventory: Some(observed),
    };
    let mut tx = world.pool.begin().await.unwrap();
    let now = world.clock.now();
    let session = new_id();
    sqlx::query("INSERT INTO sessions(id,user_id,refresh_hash,expires_at) VALUES($1,$2,$3,$4)")
        .bind(session)
        .bind(world.ada)
        .bind(session.as_bytes().as_slice())
        .bind(now + chrono::Duration::days(1))
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE hosts SET schedulable=false WHERE id=$1")
        .bind(host.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    if lost {
        sqlx::query("UPDATE machines SET status='revoked',online=false,lost_at=$2 WHERE id=$1")
            .bind(host.machine)
            .bind(now)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    let recipe = Recipe {
        host: host.id,
        machine: host.machine,
        by: world.ada,
        token: None,
        schedulable: true,
        children: vec![],
        delete_volumes: vec![],
        admin_override: false,
        custody: Some(custody),
    };
    let job = jobs::insert(
        &mut tx,
        &jobs::NewJob {
            id: new_id(),
            kind: KIND,
            node_id: None,
            environment_id: None,
            provider_id: None,
            created_by: Some(world.ada),
            plan: &json!({"cascade":recipe,"identity":{"session_id":session}}),
            progress: &serde_json::to_value(orch_engine::jobs::runner::initial_progress(&PHASES))
                .unwrap(),
        },
        now,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    let (state, _, error) = hosting::wait_over(&world, job.id).await;
    assert_eq!(
        state,
        if refuse { "failed" } else { "succeeded" },
        "{error:?}"
    );
    let row = jobs::job(&mut world.pool.acquire().await.unwrap(), job.id)
        .await
        .unwrap()
        .unwrap();
    let result = row.result.unwrap();
    let history: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM hosts WHERE id=$1)")
        .bind(host.id)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert!(
        history,
        "runtime history retains its original host identity"
    );
    let remains: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE h.id=$1 AND m.deleted_at IS NULL)")
        .bind(host.id)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(remains, refuse, "a native failure keeps the registration");
    assert!(
        result["uninstall_command"].is_null(),
        "an unreported local installation never invents a runnable command"
    );
    if lost {
        assert_eq!(host.count(CommandKind::NodeResourcesList), 0);
        assert_eq!(host.count(CommandKind::NodeResourcesRemove), 0);
        assert_eq!(result["unreachable"], true);
        assert_eq!(inventory.lock().unwrap().resources.len(), 6);
    } else if refuse {
        assert!(result["deleted_at"].is_null());
        assert_eq!(result["left_confirmed"], false);
        assert!(
            result["left"]
                .as_array()
                .unwrap()
                .iter()
                .any(|object| object["name"] == "image")
        );
    } else {
        let current = inventory.lock().unwrap();
        assert!(
            current
                .resources
                .iter()
                .any(|object| object.resource == keep.resource)
        );
        assert!(
            current
                .resources
                .iter()
                .any(|object| object.resource == layer.resource)
        );
        assert!(
            !current
                .resources
                .iter()
                .any(|object| object.resource == remove_data.resource)
        );
        assert_eq!(current.resources.len(), if erase { 2 } else { 5 });
        assert_eq!(result["left_confirmed"], true);
        assert_eq!(
            result["left"].as_array().unwrap().len(),
            current.resources.len()
        );
        if !erase {
            assert_eq!(host.count(CommandKind::NodeResourcesRemove), 1);
        }
    }
    assert_eq!(
        host.count(CommandKind::NodeVolumeRemove),
        0,
        "host cleanup uses the common node vocabulary"
    );
    world.engine().stop();
}

#[tokio::test]
async fn deleting_a_host_erases_non_data_and_preserves_every_leave_choice() {
    delete_host_data_case(true, false, false).await;
}

#[tokio::test]
async fn deleting_a_host_with_erasure_off_only_removes_explicitly_named_data() {
    delete_host_data_case(false, false, false).await;
}

#[tokio::test]
async fn deleting_a_lost_host_sends_no_native_cleanup_and_keeps_an_unconfirmed_inventory() {
    delete_host_data_case(true, true, false).await;
}

#[tokio::test]
async fn unconfirmed_host_erasure_keeps_the_registration_and_reports_its_last_observation() {
    delete_host_data_case(true, false, true).await;
}

#[tokio::test]
async fn a_host_inventory_review_uses_the_real_read_command_without_file_measurement_fields() {
    use orch_driver_contract::resources::ResourceInventory;
    use orch_link_contract::commands::{Command, CommandKind, CommandResult};
    let world = World::new().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "Inventory review", 16_384).await;
    let inventory = ResourceInventory {
        observed_at: Time::new(world.clock.now()),
        resources: vec![],
        unavailable: vec![],
    };
    let answer = inventory.clone();
    host.answer(CommandKind::NodeResourcesList, move |command| {
        assert!(matches!(command, Command::NodeResourcesList(_)));
        assert_eq!(
            command.body().unwrap(),
            json!({}),
            "inventory has no file work payload"
        );
        Ok(CommandResult::NodeResourcesList(answer.clone()))
    });
    let (pending, reference) = world
        .engine()
        .host_custody_inventory(host.id, host.machine, world.ada)
        .await
        .unwrap();
    assert!(pending.is_none());
    assert!(reference.measured_at.is_none());
    let (state, _, error) = hosting::wait_over(&world, reference.job_id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let (completed, next) = world
        .engine()
        .host_custody_inventory(host.id, host.machine, world.ada)
        .await
        .unwrap();
    assert_eq!(completed, Some(inventory.clone()));
    assert_eq!(next.job_id, reference.job_id);
    assert!(next.measured_at.is_some());
    let cached: serde_json::Value =
        sqlx::query_scalar("SELECT resource_inventory FROM hosts WHERE id=$1")
            .bind(host.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(cached, serde_json::to_value(inventory).unwrap());
    assert_eq!(host.count(CommandKind::NodeResourcesList), 1);
    assert_eq!(host.count(CommandKind::NodeResourcesRemove), 0);
    world.engine().stop();
}

#[tokio::test]
async fn a_host_data_move_keeps_its_childs_copy_and_never_erases_the_source_before_the_new_copy_arrives()
 {
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
    use orch_api_contract::loss::{HostDataFate, HostDataRow, LossResource};
    use orch_authz::ownership::{NewAccess, start_resource};
    use orch_db::queries::resources::mounts::{self, LocationRow, MountRow};
    use orch_driver_contract::resources::{Resource, ResourceInventory, ResourceKind, ResourceRef};
    use orch_engine::{
        jobs::host_delete::{Custody, KIND, MountData, PHASES, Recipe},
        mount_copies::{CopyDecision, DataRef, MountDecisions},
    };
    use orch_link_contract::commands::{Command, CommandKind, CommandResult, Empty, VolumeArchive};
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };
    let world = World::new().await;
    hosting::serve(&world);
    let source = hosting::ScriptedHost::on(&world, "Source data", 16_384).await;
    let target = hosting::ScriptedHost::on(&world, "Destination data", 16_384).await;
    let mount = new_id();
    let child = new_id();
    let parent = new_id();
    let now = world.clock.now();
    let original = format!("orch-mount-{mount}");
    let child_name = format!("orch-mount-{mount}-{}", &child.simple().to_string()[20..]);
    let next_name = format!("orch-mount-{mount}-{}", &parent.simple().to_string()[20..]);
    let location = |machine, source: String| LocationRow {
        mount_id: mount,
        machine_id: machine,
        host_id: None,
        host_name: None,
        kind: "volume".into(),
        source,
        storage: None,
        data_kind: None,
        created_by: world.ada,
        created_at: now,
    };
    let from = location(source.machine, original.clone());
    let native = Resource {
        resource: ResourceRef {
            kind: ResourceKind::Volume,
            id: "source-data".into(),
        },
        name: original.clone(),
        size_bytes: Some(0),
        labels: BTreeMap::from([("orch.mount_id".into(), mount.to_string())]),
        depends_on: vec![],
        aliases: vec![],
        last_used_at: None,
        busy: false,
    };
    let inventory = Arc::new(Mutex::new(ResourceInventory {
        observed_at: Time::new(now),
        resources: vec![native.clone()],
        unavailable: vec![],
    }));
    let listing = inventory.clone();
    source.answer(CommandKind::NodeResourcesList, move |_| {
        Ok(CommandResult::NodeResourcesList(
            listing.lock().unwrap().clone(),
        ))
    });
    let removing = inventory.clone();
    source.answer(CommandKind::NodeResourcesRemove, move |command| {
        let Command::NodeResourcesRemove(body) = command else {
            panic!("not a resource removal");
        };
        assert_eq!(body.id, "source-data");
        removing.lock().unwrap().resources.clear();
        Ok(CommandResult::NodeResourcesRemove(Empty {}))
    });
    source.answer(CommandKind::NodeMountExport, move |_| {
        Ok(CommandResult::NodeMountExport(VolumeArchive {
            parts: vec![],
            size: 0,
        }))
    });
    target.answer(CommandKind::NodeMountImport, move |_| {
        Ok(CommandResult::NodeMountImport(Empty {}))
    });
    target
        .added
        .machine
        .handler
        .hold(CommandKind::NodeMountImport);
    let copy = CopyDecision {
        mount_id: mount,
        source_machine: source.machine,
        source_driver: "docker".into(),
        from: DataRef {
            kind: "volume".into(),
            source: original.clone(),
            storage: None,
        },
        target_machine: target.machine,
        target_driver: "docker".into(),
        to: DataRef {
            kind: "volume".into(),
            source: next_name.clone(),
            storage: None,
        },
        made: true,
        kept: None,
        file_count: 0,
        total_bytes: 0,
        data_kind: None,
    };
    let mut earlier = copy.clone();
    earlier.to.source = child_name.clone();
    earlier.file_count = 1;
    earlier.total_bytes = 4;
    let decisions = MountDecisions {
        detached: vec![],
        copies: vec![copy],
    };
    let row = HostDataRow {
        id: format!("mount:{mount}"),
        resource: LossResource {
            id: mount,
            name: Some("shared-data".into()),
        },
        label: None,
        kind: "mount".into(),
        bytes: Some(0),
        until: None,
        selected: HostDataFate::Move,
        choices: vec![
            HostDataFate::Leave,
            HostDataFate::Move,
            HostDataFate::Delete,
        ],
        host: Some(orch_api_contract::resources::HostRef {
            id: target.id,
            name: "Destination data".into(),
            address: String::new(),
        }),
        copy: None,
        blockers: vec![],
    };
    let custody = Custody {
        erase: false,
        mounts: vec![MountData {
            location: from.clone(),
            kept: None,
            fate: HostDataFate::Move,
            row,
            native: Some(native.resource.clone()),
        }],
        copies: decisions.clone(),
        native_data: vec![],
        uninstall_command: None,
        frozen_inventory: Some(inventory.lock().unwrap().clone()),
    };
    let mut tx = world.pool.begin().await.unwrap();
    mounts::insert(
        &mut tx,
        &MountRow {
            id: mount,
            name: "shared-data".into(),
            description: String::new(),
            org_id: Some(world.root),
            home_id: Some(world.root),
            owner_id: Some(world.ada),
            target: "/data/shared".into(),
            read_only: false,
            enabled: true,
            default_fate: "copy".into(),
            revision: 1,
        },
        now,
    )
    .await
    .unwrap();
    start_resource(
        &mut tx,
        ObjectRef {
            kind: ObjectKind::Mount,
            id: mount,
        },
        world.ada,
        NewAccess::OnlyMe,
        Time::new(now),
    )
    .await
    .unwrap();
    mounts::set_location(&mut tx, &from).await.unwrap();
    mounts::set_location(&mut tx, &location(target.machine, child_name.clone()))
        .await
        .unwrap();
    let child_plan = json!({"committed":true,"mount_decisions":MountDecisions{detached:vec![],copies:vec![earlier]}});
    jobs::insert(
        &mut tx,
        &jobs::NewJob {
            id: child,
            kind: "environment_move",
            node_id: None,
            environment_id: Some(world.environment),
            provider_id: None,
            created_by: Some(world.ada),
            plan: &child_plan,
            progress: &json!({}),
        },
        now,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE jobs SET state='succeeded',finished_at=$2 WHERE id=$1")
        .bind(child)
        .bind(now)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE hosts SET schedulable=false WHERE id=$1")
        .bind(source.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let recipe = Recipe {
        host: source.id,
        machine: source.machine,
        by: world.ada,
        token: None,
        schedulable: true,
        children: vec![child],
        delete_volumes: vec![],
        admin_override: false,
        custody: Some(custody),
    };
    jobs::insert(
        &mut tx,
        &jobs::NewJob {
            id: parent,
            kind: KIND,
            node_id: None,
            environment_id: None,
            provider_id: None,
            created_by: Some(world.ada),
            plan: &json!({"cascade":recipe,"mount_decisions":decisions}),
            progress: &serde_json::to_value(orch_engine::jobs::runner::initial_progress(&PHASES))
                .unwrap(),
        },
        now,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    target.wait_began(CommandKind::NodeMountImport, 1).await;
    assert_eq!(
        source.count(CommandKind::NodeResourcesRemove),
        0,
        "the acknowledged destination copy must precede source erasure"
    );
    assert_eq!(inventory.lock().unwrap().resources.len(), 1);
    assert_eq!(
        source.count(CommandKind::NodeMountExport),
        1,
        "the parent reads a fresh source, rather than reusing an earlier child's snapshot"
    );
    target.added.machine.handler.release();
    let (state, _, error) = hosting::wait_over(&world, parent).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let mut conn = world.pool.acquire().await.unwrap();
    let locations = mounts::locations(&mut conn, &[mount]).await.unwrap();
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].machine_id, target.machine);
    assert_eq!(locations[0].source, next_name);
    let kept = mounts::kept(&mut conn, &[mount]).await.unwrap();
    assert!(
        kept.iter()
            .any(|data| data.machine_id == target.machine && data.source == child_name),
        "a sibling's published data remains recorded for cleanup or inspection"
    );
    assert!(inventory.lock().unwrap().resources.is_empty());
    assert_eq!(source.count(CommandKind::NodeVolumeRemove), 0);
    world.engine().stop();
}
