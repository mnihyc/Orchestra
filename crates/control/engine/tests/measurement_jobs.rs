//! A plan answers while its reusable volume count is running and receives its complete count on the next plan.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::World;
use hosting::{ScriptedHost, bind, running, serve, template};
use orch_authz::Caller;
use orch_link_contract::commands::CommandKind;
use serde_json::json;

#[tokio::test]
async fn host_reported_copy_provision_and_count_phases_accept_live_counters_without_reversing_the_bar()
 {
    use orch_db::queries::{jobs, link};
    use orch_link_contract::events::{Event, JobProgress};
    use orch_link_contract::streams::Stream;
    use orch_types::new_id;
    let world = World::new().await;
    let host = ScriptedHost::on(&world, "Progress source", 16_384).await;
    for phase in [
        "copy",
        "provision",
        "measure",
        "fingerprint_source",
        "fingerprint_target",
    ] {
        let id = new_id();
        let command = new_id();
        let mut tx = world.pool.begin().await.unwrap();
        jobs::insert(&mut tx, &jobs::NewJob {
            id, kind: "measurement", node_id: None, environment_id: None, provider_id: None, created_by: None,
            plan: &json!({"active_command":{"id":command,"machine":host.machine,"phase":phase,"reported":true},
                "runner":{"id":new_id(),"until":world.clock.now()+chrono::Duration::minutes(10)}}),
            progress: &json!({"phase":phase,"phases":[{"name":phase,"weight":100,"state":"running"}],"percent":0,
                "files_done":0,"files_total":null,"bytes_done":0,"bytes_total":null,"rate_bps":null,"eta_seconds":null,"message":null}),
        }, world.clock.now()).await.unwrap();
        sqlx::query("UPDATE jobs SET state='running' WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .unwrap();
        link::insert_command(
            &mut tx,
            &link::NewCommand {
                id: command,
                machine_id: host.machine,
                kind: "node.mount.measure",
                serial_key: None,
                body: &json!({"work":{"job_id":id,"phase":phase}}),
                deadline_at: world.clock.now() + chrono::Duration::minutes(10),
            },
            world.clock.now(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE commands SET state='accepted' WHERE id=$1")
            .bind(command)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        for (reported_phase, percent, command) in [
            (phase, 80, command),
            ("history", 99, command),
            (phase, 99, new_id()),
            (phase, 20, command),
            (phase, 40, command),
        ] {
            host.added.machine.publish(&Stream::Node, &Event::JobProgress(JobProgress { command_id:Some(command),
                job_id:id, log:None, progress:serde_json::from_value(json!({"phase":reported_phase,"phases":[],"percent":percent,
                    "files_done":percent,"files_total":100,"bytes_done":percent*100,"bytes_total":10000,
                    "rate_bps":100,"eta_seconds":10,"message":null})).unwrap(),
            })).await;
        }
        common::eventually(
            "all three ordered progress reports to be applied",
            || async { hosting::job(&world, id).await.1["files_done"] == 40 },
        )
        .await;
        let progress = hosting::job(&world, id).await.1;
        assert_eq!(progress["percent"], 80, "the {phase} bar moved backwards");
        assert_eq!(progress["bytes_done"], 4000);
        assert_eq!(progress["rate_bps"], 100);
        assert!(progress["activity"]["updated_at"].is_string());
    }
    world.engine().stop();
}

#[tokio::test]
async fn a_preflight_answers_before_a_count_finishes_and_other_plans_reuse_the_count() {
    let world = World::new().await;
    serve(&world);
    let source = ScriptedHost::on(&world, "Count source", 16_384).await;
    let target = ScriptedHost::on(&world, "Count target", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _) = running(&world, &source, template, "Counting").await;
    let thread = world.thread("Count before changing").await;
    bind(&world, thread, environment).await;
    source
        .added
        .machine
        .handler
        .hold(CommandKind::NodeRuntimeVolumesMeasure);
    let request = serde_json::from_value(json!({"result":"new","history":{"mode":"none"},
        "environment":{"new":{"name":"Counted destination","template_id":template,
            "host_id":target.id,"files":{"mode":"none"},"home":true,"private_views":false}}}))
    .unwrap();
    let caller = Caller::session(world.ada);
    let first = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    let first = first.plan.files.unwrap().home.unwrap();
    assert!(!first.measured);
    let measurement = first.measurement.unwrap();
    assert!(measurement.measured_at.is_none());
    assert_eq!(
        first.file_count, 0,
        "a pending count is unknown, not an invented total"
    );
    let second = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    assert_eq!(
        second
            .plan
            .files
            .unwrap()
            .home
            .unwrap()
            .measurement
            .unwrap()
            .job_id,
        measurement.job_id
    );
    source
        .wait_began(CommandKind::NodeRuntimeVolumesMeasure, 1)
        .await;
    source.added.machine.handler.release();
    let (state, _, error) = hosting::wait_over(&world, measurement.job_id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let complete = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    let home = complete.plan.files.unwrap().home.unwrap();
    assert!(home.measured);
    assert_eq!(home.file_count, 3);
    assert_eq!(home.total_bytes, 8192);
    assert_eq!(home.measurement.unwrap().job_id, measurement.job_id);
    assert!(home.measurement.unwrap().measured_at.is_some());
    assert_eq!(source.began(CommandKind::NodeRuntimeVolumesMeasure), 1);
    world.engine().stop();
}

#[tokio::test]
async fn a_silent_count_keeps_running_while_watched_and_expires_after_its_view_leaves() {
    use orch_db::queries::jobs;
    use orch_types::new_id;
    let world = World::new().await;
    let id = new_id();
    world.live.watching_job(id, true);
    let mut tx = world.pool.begin().await.unwrap();
    jobs::insert(&mut tx, &jobs::NewJob {
        id, kind: "measurement", node_id: None, environment_id: None, provider_id: None, created_by: None,
        plan: &json!({"keep_until":world.clock.now()-chrono::Duration::seconds(1),
            "runner":{"id":new_id(),"until":world.clock.now()+chrono::Duration::minutes(10)}}),
        progress: &json!({"phase":"measure","phases":[],"percent":0,"files_done":0,
            "files_total":null,"bytes_done":0,"bytes_total":null,"rate_bps":null,"eta_seconds":null,"message":null}),
    }, world.clock.now()).await.unwrap();
    sqlx::query("UPDATE jobs SET state='running' WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    common::eventually("an active view to renew a silent count", || async {
        let mut conn = world.pool.acquire().await.unwrap();
        let row = jobs::job(&mut conn, id).await.unwrap().unwrap();
        assert!(!row.cancel_requested());
        row.plan["keep_until"]
            .as_str()
            .unwrap()
            .parse::<orch_types::Time>()
            .unwrap()
            .get()
            > world.clock.now()
    })
    .await;
    world.live.watching_job(id, false);
    sqlx::query(
        "UPDATE jobs SET plan=jsonb_set(plan,'{keep_until}',to_jsonb($2::text)) WHERE id=$1",
    )
    .bind(id)
    .bind((world.clock.now() - chrono::Duration::seconds(1)).to_rfc3339())
    .execute(world.pool.pool())
    .await
    .unwrap();
    world.engine().kick();
    common::eventually(
        "an abandoned count to be cancelled after its grace period",
        || async {
            let mut conn = world.pool.acquire().await.unwrap();
            jobs::job(&mut conn, id)
                .await
                .unwrap()
                .unwrap()
                .cancel_requested()
        },
    )
    .await;
    world.engine().stop();
}

#[tokio::test]
async fn an_incomplete_machine_answer_fails_the_count_and_a_retry_does_not_reuse_it() {
    let world = World::new().await;
    serve(&world);
    let source = ScriptedHost::on(&world, "Incomplete source", 16_384).await;
    let target = ScriptedHost::on(&world, "Retry destination", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _) = running(&world, &source, template, "Invalid count").await;
    let thread = world.thread("Retry a failed count").await;
    bind(&world, thread, environment).await;
    source.answer(CommandKind::NodeRuntimeVolumesMeasure, |_| {
        Ok(
            orch_link_contract::commands::CommandResult::NodeRuntimeVolumesMeasure(
                orch_link_contract::commands::NodeRuntimeVolumesMeasureResult {
                    volumes: Vec::new(),
                },
            ),
        )
    });
    let request = serde_json::from_value(json!({"result":"new","history":{"mode":"none"},
        "environment":{"new":{"name":"Retry count","template_id":template,"host_id":target.id,
            "files":{"mode":"none"},"home":true,"private_views":false}}}))
    .unwrap();
    let caller = Caller::session(world.ada);
    let first = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap()
        .plan
        .files
        .unwrap()
        .home
        .unwrap()
        .measurement
        .unwrap()
        .job_id;
    assert_eq!(hosting::wait_over(&world, first).await.0, "failed");
    let second = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap()
        .plan
        .files
        .unwrap()
        .home
        .unwrap()
        .measurement
        .unwrap()
        .job_id;
    assert_ne!(
        first, second,
        "a failed partial answer became a reusable complete count"
    );
    world.engine().stop();
}

#[tokio::test]
async fn a_directory_count_uses_its_environment_feed_only_for_its_declared_command_and_subject() {
    use orch_db::queries::{jobs, link, machine_reports};
    use orch_link_contract::{
        events::{Event, JobProgress},
        streams::Stream,
    };
    use orch_types::new_id;
    let world = World::new().await;
    let id = new_id();
    let command = new_id();
    let mut tx = world.pool.begin().await.unwrap();
    jobs::insert(&mut tx,&jobs::NewJob{
        id,kind:"measurement",node_id:None,environment_id:None,provider_id:None,created_by:None,
        plan:&json!({"subject":{"environment_id":world.environment},"active_command":{"id":command,"machine":world.machine_id,"phase":"measure","reported":true},"runner":{"id":new_id(),"until":world.clock.now()+chrono::Duration::minutes(10)}}),
        progress:&json!({"phase":"measure","phases":[{"name":"measure","weight":100,"state":"running"}],"percent":0,"files_done":0,"files_total":null,"bytes_done":0,"bytes_total":null,"rate_bps":null,"eta_seconds":null,"message":null}),
    },world.clock.now()).await.unwrap();
    sqlx::query("UPDATE jobs SET state='running' WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let body = json!({"environment_id":world.environment,"work":{"job_id":id,"phase":"measure"}});
    link::insert_command(
        &mut tx,
        &link::NewCommand {
            id: command,
            machine_id: world.machine_id,
            kind: "fs.data.measure",
            serial_key: None,
            body: &body,
            deadline_at: world.clock.now() + chrono::Duration::minutes(10),
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE commands SET state='accepted' WHERE id=$1")
        .bind(command)
        .execute(&mut *tx)
        .await
        .unwrap();
    let stream = Stream::Env {
        environment_id: world.environment,
    };
    let report = json!({"job_id":id});
    assert!(
        machine_reports::accepts(
            &mut tx,
            world.machine_id,
            &stream,
            &[("job.progress", &report)]
        )
        .await
        .unwrap()
    );
    sqlx::query("UPDATE commands SET body=jsonb_set(body,'{environment_id}',to_jsonb($2::text)) WHERE id=$1").bind(command).bind(new_id().to_string()).execute(&mut *tx).await.unwrap();
    assert!(
        !machine_reports::accepts(
            &mut tx,
            world.machine_id,
            &stream,
            &[("job.progress", &report)]
        )
        .await
        .unwrap(),
        "an unrelated environment supplied a count"
    );
    sqlx::query("UPDATE commands SET body=$2 WHERE id=$1")
        .bind(command)
        .bind(&body)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.machine.publish(&stream,&Event::JobProgress(JobProgress{command_id:Some(command),job_id:id,log:None,progress:serde_json::from_value(json!({"phase":"measure","phases":[],"percent":0,"files_done":1204,"files_total":null,"bytes_done":4096,"bytes_total":null,"rate_bps":null,"eta_seconds":null,"message":null})).unwrap()})).await;
    common::eventually(
        "the directory count's environment report to reach its live job",
        || async { hosting::job(&world, id).await.1["files_done"] == 1204 },
    )
    .await;
    world.engine().stop();
}
