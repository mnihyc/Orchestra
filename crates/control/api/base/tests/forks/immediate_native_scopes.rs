//! The native immediate fork keeps its one source, exact deferred origin and accepted result projection.
use super::*;
use orch_api_contract::forks::{ForkNowBody, ThreadForkNow};
use orch_api_core::testing::RawReply;
use thread_history_scope::{Proof, key, narrow, rule};
const OP: &str = "ThreadForkNow";
fn manages(thread: Id) -> Vec<Json> {
    vec![rule(OP, "thread", thread, "self", "manage")]
}
async fn start(w: &Forks, thread: Id, secret: &str) -> RawReply {
    w.s.rig
        .world
        .app
        .call(ThreadForkNow, &[&thread])
        .token(secret)
        .body(&ForkNowBody {
            name: Some("Accepted immediate fork".into()),
            ..Default::default()
        })
        .send_bytes()
        .await
}
fn value(reply: &RawReply) -> Proof<Json> {
    assert_eq!(
        reply.status.as_u16(),
        202,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    Ok(serde_json::from_slice(&reply.raw)?)
}
fn receipt(reply: &RawReply) -> Proof<(Id, Id)> {
    let body = value(reply)?;
    assert_eq!(body["projection"], "receipt");
    assert_eq!(
        body.as_object().ok_or("immediate receipt missing")?.len(),
        3
    );
    assert!(body.get("job").is_none() && body.get("history").is_none());
    Ok((
        serde_json::from_value(body["job_id"].clone())?,
        serde_json::from_value(body["thread_id"].clone())?,
    ))
}
async fn wait(
    w: &Forks,
    pid: i32,
    pending: &tokio::task::JoinHandle<RawReply>,
    activity: bool,
) -> Proof {
    tokio::time::timeout(rig::PATIENCE,async {loop {
        assert!(!pending.is_finished(),"the native boundary must be reached");
        let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND (NOT $2 OR query ILIKE '%INSERT INTO api_token_activity%'))")
            .bind(pid).bind(activity).fetch_one(w.s.rig.db.pool()).await.unwrap();
        if blocked {break;}tokio::task::yield_now().await;
    }}).await?;
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn immediate_fork_binds_its_actual_source_without_carried_reads_or_a_neighboring_namespace_and_keeps_native_creation_prerequisites()
-> Proof {
    let w = Forks::new().await;
    let thread = w.thread_with_history("Managed immediate source").await;
    w.s.rig.world.app.app.engine.stop();
    let other =
        w.s.thread_by(&w.s.ada, "Other immediate source")
            .await
            .base
            .id;
    for grants in [
        vec![rule(OP, "thread", thread, "self", "read")],
        vec![rule(OP, "thread", thread, "self", "write")],
        manages(other),
        vec![rule(
            "ThreadForkCreate.fork",
            "thread",
            thread,
            "self",
            "manage",
        )],
    ] {
        let actor = key(&w.s.rig.world, w.s.ada.id(), grants).await?;
        assert_eq!(start(&w, thread, &actor.secret).await.status.as_u16(), 403);
    }
    assert_eq!(
        w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
            .await,
        0
    );
    let reader = key(&w.s.rig.world, w.s.cai.id(), manages(thread)).await?;
    assert!(!start(&w, thread, &reader.secret).await.status.is_success());
    let actor = key(&w.s.rig.world, w.s.ada.id(), manages(thread)).await?;
    let (job, child) = receipt(&start(&w, thread, &actor.secret).await)?;
    let row = orch_db::queries::jobs::job(&mut *w.s.rig.world.conn().await, job)
        .await?
        .ok_or("accepted native job missing")?;
    assert_eq!(row.node_id, Some(thread));
    assert_eq!(row.plan["new_thread"], json!(child));
    assert_eq!(
        row.plan["confirmed_history_source"]["operation"]["origin"]["operation"],
        OP
    );
    assert_eq!(
        row.plan["confirmed_history_source"]["operation"]["credential"]["kind"],
        "token"
    );
    assert_eq!(row.plan["from_environment"], json!(w.s.on.environment));
    assert_eq!(row.plan["to_environment"], json!(w.s.on.environment));
    assert_eq!(
        w.count("SELECT count(*) FROM preflights").await,
        0,
        "the private prefix is consumed atomically"
    );
    assert_eq!(
        w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
            .await,
        1
    );
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn immediate_fork_rechecks_after_the_real_source_wait_before_its_first_private_plan_or_job_write()
-> Proof {
    for mode in ["live", "expired", "source"] {
        let w = Forks::new().await;
        let thread = w.thread_with_history("Waiting immediate source").await;
        w.s.rig.world.app.app.engine.stop();
        let other =
            another_environment(&w.s, "Changed immediate source", Some(w.s.rig.machine_id)).await;
        let actor = key(&w.s.rig.world, w.s.ada.id(), manages(thread)).await?;
        let mut locked = w.s.rig.db.begin().await?;
        orch_db::queries::threads::lock(&mut locked, thread)
            .await?
            .ok_or("native source missing")?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *locked)
            .await?;
        let app = w.s.rig.world.app.clone();
        let secret = actor.secret.clone();
        let pending = tokio::spawn(async move {
            let call = app
                .call(ThreadForkNow, &[&thread])
                .token(&secret)
                .body(&ForkNowBody::default());
            call.send_bytes().await
        });
        wait(&w, pid, &pending, false).await?;
        if mode == "expired" {
            w.s.rig.world.advance(chrono::Duration::hours(2));
        }
        if mode == "source" {
            sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
                .bind(thread)
                .bind(other)
                .execute(&mut *locked)
                .await?;
        }
        locked.commit().await?;
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        if mode == "live" {
            receipt(&reply)?;
        } else {
            assert!(
                matches!(reply.status.as_u16(), 401 | 403),
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(
                w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
                    .await,
                0
            );
        }
        assert_eq!(w.count("SELECT count(*) FROM preflights").await, 0);
    }
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn immediate_completed_result_keeps_its_own_read_after_manage_loss_and_receipt_after_read_loss_or_expiry()
-> Proof {
    for mode in ["live", "namespace", "manage", "read", "expired"] {
        let w = Forks::new().await;
        let thread = w.thread_with_history("Published immediate source").await;
        w.s.rig.world.app.app.engine.stop();
        let namespace = if mode == "namespace" { "JobGet" } else { OP };
        let reads = vec![
            rule(namespace, "thread", thread, "self", "read"),
            rule(namespace, "environment", w.s.on.environment, "self", "read"),
        ];
        let mut grants = manages(thread);
        grants.extend(reads.clone());
        let actor = key(&w.s.rig.world, w.s.ada.id(), grants).await?;
        let mut locked = w.s.rig.db.begin().await?;
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *locked)
            .await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *locked)
            .await?;
        let app = w.s.rig.world.app.clone();
        let secret = actor.secret.clone();
        let pending = tokio::spawn(async move {
            let call = app
                .call(ThreadForkNow, &[&thread])
                .token(&secret)
                .body(&ForkNowBody::default());
            call.send_bytes().await
        });
        wait(&w, pid, &pending, true).await?;
        let stored = w
            .count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
            .await;
        if stored != 1 && std::env::var_os("ORCH_TEST_NATIVE_OPERATIONS").is_some() {
            locked.commit().await?;
            let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
            eprintln!(
                "Native immediate publication stored_before={stored} status={} body={} stored_after={}",
                reply.status,
                String::from_utf8_lossy(&reply.raw),
                w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
                    .await
            );
            panic!("the native fork job was not stored before its publication wait");
        }
        assert_eq!(stored, 1);
        match mode {
            "manage" => narrow(&w.s.rig.world, &actor, reads).await?,
            "read" => narrow(&w.s.rig.world, &actor, manages(thread)).await?,
            "expired" => w.s.rig.world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        locked.commit().await?;
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        let body = value(&reply)?;
        assert_eq!(
            body.get("projection").is_none(),
            matches!(mode, "live" | "manage")
        );
        if matches!(mode, "live" | "manage") {
            let full: orch_api_contract::forks::ForkNowReply = serde_json::from_value(body)?;
            assert_eq!(full.job.node_id, Some(thread));
        } else {
            receipt(&reply)?;
        }
        assert_eq!(
            w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
                .await,
            1
        );
    }
    Ok(())
}
pub(super) async fn new_native(
    w: &Forks,
    kind: CommandKind,
    before: &std::collections::BTreeSet<Id>,
) -> Proof<Id> {
    Ok(tokio::time::timeout(rig::PATIENCE, async {
        loop {
            if let Some(id) =
                w.s.rig
                    .machine
                    .handler
                    .executions()
                    .iter()
                    .find(|run| run.kind == kind && !before.contains(&run.id))
                    .map(|run| run.id)
            {
                break id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?)
}
async fn evidence(w: &Forks, id: Id) -> Proof<orch_engine::commands::CommandAuthority> {
    let authority =
        w.s.rig
            .commands
            .issued_authority(id)
            .ok_or("native fork command lost its original source")?;
    let private = serde_json::to_value(&authority)?;
    assert_eq!(private["kind"], "fork_native");
    assert_eq!(
        private["authority"]["source"]["origin"]["origin"]["operation"],
        OP
    );
    let mut tx = w.s.rig.db.begin().await?;
    let row = orch_db::queries::link::command(&mut tx, id)
        .await?
        .ok_or("native journal missing")?;
    let clock = w.s.rig.world.app.app.clock.clone();
    let access = orch_api_core::thread_access::AuthzAccess::new(clock.clone());
    assert!(row.body.get("authority").is_none() && row.body.get("origin").is_none());
    assert!(
        authority
            .current_callers(&mut tx, &row, clock.as_ref(), &access)
            .await?
            .is_some()
    );
    let mut changed = row.clone();
    changed.machine_id = orch_types::new_id();
    assert!(
        authority
            .current_callers(&mut tx, &changed, clock.as_ref(), &access)
            .await?
            .is_none()
    );
    let mut changed = row.clone();
    changed.deadline_at += chrono::Duration::seconds(1);
    assert!(
        authority
            .current_callers(&mut tx, &changed, clock.as_ref(), &access)
            .await?
            .is_none()
    );
    tx.rollback().await?;
    Ok(authority)
}
#[tokio::test(flavor = "multi_thread")]
async fn immediate_fork_native_startup_retains_the_actual_source_through_live_and_revoked_controls()
-> Proof {
    for revoked in [false, true] {
        let w = Forks::new().await;
        let thread = w.thread_with_history("Immediate startup source").await;
        sqlx::query("UPDATE harnesses SET status='error',start_command_id=NULL WHERE provider_id=$1 AND agent_id=$2")
            .bind(w.s.on.provider).bind(w.s.on.agent).execute(w.s.rig.db.pool()).await?;
        let before =
            w.s.rig
                .machine
                .handler
                .executions()
                .iter()
                .map(|run| run.id)
                .collect();
        w.s.rig.machine.handler.hold(CommandKind::AgentStart);
        let actor = key(&w.s.rig.world, w.s.ada.id(), manages(thread)).await?;
        let (job, child) = receipt(&start(&w, thread, &actor.secret).await)?;
        let started = new_native(&w, CommandKind::AgentStart, &before).await;
        if started.is_err() && std::env::var_os("ORCH_TEST_NATIVE_OPERATIONS").is_some() {
            let row = orch_db::queries::jobs::job(&mut *w.s.rig.world.conn().await, job).await?;
            if let Some(row) = row {
                eprintln!(
                    "Native startup wait ended: job={job}, state={}, progress={}, error={:?}",
                    row.state, row.progress, row.error
                );
            } else {
                eprintln!("Native startup wait ended: the accepted job is absent");
            }
        }
        let id = started?;
        evidence(&w, id).await?;
        if revoked {
            narrow(
                &w.s.rig.world,
                &actor,
                vec![rule(OP, "thread", thread, "self", "read")],
            )
            .await?;
        }
        w.s.rig.machine.handler.release();
        let done = w.ended(&w.s.ada, job).await;
        if !revoked {
            assert_eq!(done.state, JobState::Succeeded, "{done:?}");
            assert!(
                orch_db::queries::threads::thread(&mut *w.s.rig.world.conn().await, child)
                    .await?
                    .is_some()
            );
        } else {
            assert_ne!(done.state, JobState::Succeeded);
        }
        assert_eq!(
            w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
                .await,
            1
        );
    }
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn immediate_fork_native_history_keeps_exact_private_source_and_stops_new_forward_work_after_scope_loss()
-> Proof {
    for revoked in [false, true] {
        let w = Forks::new().await;
        let thread = w.thread_with_history("Immediate native history").await;
        let before =
            w.s.rig
                .machine
                .handler
                .executions()
                .iter()
                .map(|run| run.id)
                .collect();
        w.s.rig.machine.handler.hold(CommandKind::SessionOpenFenced);
        let actor = key(&w.s.rig.world, w.s.ada.id(), manages(thread)).await?;
        let (job, _child) = receipt(&start(&w, thread, &actor.secret).await)?;
        let id = new_native(&w, CommandKind::SessionOpenFenced, &before).await?;
        evidence(&w, id).await?;
        if revoked {
            narrow(
                &w.s.rig.world,
                &actor,
                vec![rule(OP, "thread", thread, "self", "read")],
            )
            .await?;
        }
        w.s.rig.machine.handler.release();
        let done = w.ended(&w.s.ada, job).await;
        if revoked {
            assert_ne!(done.state, JobState::Succeeded);
        } else {
            assert_eq!(done.state, JobState::Succeeded, "{done:?}");
        }
        assert_eq!(
            w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
                .await,
            1
        );
    }
    Ok(())
}
