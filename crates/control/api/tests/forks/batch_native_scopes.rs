//! Real native batches use each selected job's governor before the aggregate.

use super::*;
use orch_api_core::testing::RawReply;
use thread_history_scope::{Proof, key, narrow, rule};

const OP: &str = "ForkBatchGet";

async fn ready() -> (Forks, [Id; 2], ForkBatch) {
    let w = Forks::new().await;
    let a =
        w.s.thread_by(&w.s.ada, "Selected batch member")
            .await
            .base
            .id;
    let b =
        w.s.thread_by(&w.s.ada, "Unselected batch member")
            .await
            .base
            .id;
    let plan = w.batch_plan(&w.s.ada, &w.batch_of(vec![a, b])).await.ok();
    assert_eq!(plan.executable, 2);
    let batch = w.batch_create(&w.s.ada, &plan).await.ok();
    for job in &batch.jobs {
        w.ended(&w.s.ada, job.job.id).await;
    }
    (w, [a, b], batch)
}

async fn get(w: &Forks, batch: Id, secret: &str) -> RawReply {
    let request =
        w.s.rig
            .world
            .app
            .call(ForkBatchGet, &[&batch])
            .token(secret);
    request.send_bytes().await
}

fn shown(reply: &RawReply) -> Proof<ForkBatch> {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    Ok(serde_json::from_slice(&reply.raw)?)
}

fn total(batch: &ForkBatch) -> i64 {
    let n = &batch.counts;
    n.queued + n.running + n.succeeded + n.failed + n.cancelled
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_scope_precedes_counts_and_hidden_and_requires_its_own_read_and_native_gate() -> Proof
{
    let (w, [a, b], batch) = ready().await;
    let world = &w.s.rig.world;
    let selected = key(
        world,
        w.s.ada.id(),
        vec![rule(OP, "thread", a, "self", "read")],
    )
    .await?;
    let actual = shown(&get(&w, batch.batch_id, &selected.secret).await)?;
    assert_eq!(actual.jobs.len(), 1);
    assert_eq!(actual.jobs[0].job.node_id, Some(a));
    assert_eq!(
        actual.jobs[0].thread.name.as_deref(),
        Some("Selected batch member")
    );
    assert_eq!((total(&actual), actual.hidden), (1, 0));
    let original = w.batch_get(&w.s.ada, batch.batch_id).await.ok();
    assert_eq!(
        (original.jobs.len(), total(&original), original.hidden),
        (2, 2, 0)
    );
    for grants in [
        vec![rule("JobGet", "thread", a, "self", "read")],
        vec![rule(OP, "thread", a, "self", "manage")],
    ] {
        let wrong = key(world, w.s.ada.id(), grants).await?;
        assert!(
            !get(&w, batch.batch_id, &wrong.secret)
                .await
                .status
                .is_success()
        );
    }
    // A real permitted operation/target outside this batch yields a genuine empty selection.
    let other = w.s.thread_by(&w.s.ada, "Another batch scope").await.base.id;
    let empty = key(
        world,
        w.s.ada.id(),
        vec![rule(OP, "thread", other, "self", "read")],
    )
    .await?;
    let empty = shown(&get(&w, batch.batch_id, &empty.secret).await)?;
    assert_eq!(
        (empty.jobs.len(), total(&empty), empty.hidden, empty.percent),
        (0, 0, 0, 0)
    );
    let no_native = key(
        world,
        w.s.cai.id(),
        vec![rule(OP, "thread", a, "self", "read")],
    )
    .await?;
    assert!(
        !get(&w, batch.batch_id, &no_native.secret)
            .await
            .status
            .is_success()
    );
    narrow(
        world,
        &selected,
        vec![rule(OP, "thread", b, "self", "read")],
    )
    .await?;
    let actual = shown(&get(&w, batch.batch_id, &selected.secret).await)?;
    assert_eq!(actual.jobs[0].job.node_id, Some(b));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn environment_only_jobs_have_their_actual_governor_and_waiting_machine_names_need_own_read()
-> Proof {
    let (w, [a, b], batch) = ready().await;
    let world = &w.s.rig.world;
    let first = batch
        .jobs
        .iter()
        .find(|job| job.job.node_id == Some(a))
        .ok_or("first native job missing")?
        .job
        .id;
    let second = batch
        .jobs
        .iter()
        .find(|job| job.job.node_id == Some(b))
        .ok_or("second native job missing")?
        .job
        .id;
    let environment: Id = sqlx::query_scalar("SELECT environment_id FROM threads WHERE node_id=$1")
        .bind(a)
        .fetch_one(&world.db.pool)
        .await?;
    // Native maker rows contribute to the aggregate without a thread entry. Arrange
    // that existing stored shape only after the real batch jobs have ended.
    sqlx::query("UPDATE jobs SET node_id=NULL,environment_id=$2 WHERE id=$1")
        .bind(first)
        .bind(environment)
        .execute(&world.db.pool)
        .await?;
    let env = key(
        world,
        w.s.ada.id(),
        vec![rule(OP, "environment", environment, "self", "read")],
    )
    .await?;
    let actual = shown(&get(&w, batch.batch_id, &env.secret).await)?;
    assert_eq!(
        (actual.jobs.len(), total(&actual), actual.hidden),
        (0, 1, 0)
    );
    let machine = w.s.rig.machine_id;
    sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,'{active_command}',$2) WHERE id=$1")
        .bind(second)
        .bind(json!({"id":orch_types::new_id(),"machine":machine}))
        .execute(&world.db.pool)
        .await?;
    let ordinary = w.batch_get(&w.s.ada, batch.batch_id).await.ok();
    assert_eq!(total(&ordinary), 2);
    let named = ordinary
        .jobs
        .iter()
        .find(|job| job.job.id == second)
        .ok_or("native visible job missing")?
        .job
        .progress
        .activity
        .as_ref()
        .and_then(|activity| activity.waiting_for.as_ref())
        .cloned();
    assert!(named.is_some());
    for name_operation in [None, Some("JobGet"), Some(OP)] {
        let mut grants = vec![rule(OP, "thread", b, "self", "read")];
        if let Some(op) = name_operation {
            grants.push(rule(op, "machine", machine, "self", "read"));
        }
        let selected = key(world, w.s.ada.id(), grants).await?;
        let actual = shown(&get(&w, batch.batch_id, &selected.secret).await)?;
        assert_eq!(
            (actual.jobs.len(), total(&actual), actual.hidden),
            (1, 1, 0)
        );
        assert_eq!(
            actual.jobs[0]
                .job
                .progress
                .activity
                .as_ref()
                .and_then(|activity| activity.waiting_for.as_ref()),
            if name_operation == Some(OP) {
                named.as_ref()
            } else {
                None
            }
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_publication_keeps_each_original_job_association_after_the_actual_activity_wait()
-> Proof {
    for mode in ["unchanged", "scope", "expired", "association"] {
        let (w, [a, b], batch) = ready().await;
        let world = &w.s.rig.world;
        let rules = vec![
            rule(OP, "thread", a, "self", "read"),
            rule(OP, "thread", b, "self", "read"),
        ];
        let selected = key(world, w.s.ada.id(), rules).await?;
        assert_eq!(
            shown(&get(&w, batch.batch_id, &selected.secret).await)?
                .jobs
                .len(),
            2
        );
        let mut blocker = world.db.pool.begin().await?;
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = world.app.clone();
        let batch_id = batch.batch_id;
        let secret = selected.secret.clone();
        let pending = tokio::spawn(async move {
            let request = app.call(ForkBatchGet, &[&batch_id]).token(&secret);
            request.send_bytes().await
        });
        tokio::time::timeout(rig::PATIENCE, async {
            loop {
                assert!(!pending.is_finished(), "the actual final activity wait must be reached");
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if waiting { break; }
                tokio::task::yield_now().await;
            }
        }).await?;
        match mode {
            "scope" => {
                narrow(
                    world,
                    &selected,
                    vec![rule(OP, "thread", b, "self", "read")],
                )
                .await?
            }
            "expired" => world.advance(chrono::Duration::hours(2)),
            "association" => {
                let first = batch
                    .jobs
                    .iter()
                    .find(|job| job.job.node_id == Some(a))
                    .ok_or("first native job missing")?
                    .job
                    .id;
                let second = batch
                    .jobs
                    .iter()
                    .find(|job| job.job.node_id == Some(b))
                    .ok_or("second native job missing")?
                    .job
                    .id;
                // Both current governors remain readable; neither can lend its authority
                // to an old job that now belongs to the other thread.
                sqlx::query("UPDATE jobs SET node_id=CASE WHEN id=$1 THEN $3 ELSE $4 END WHERE id=$1 OR id=$2")
                    .bind(first).bind(second).bind(b).bind(a).execute(&world.db.pool).await?;
            }
            _ => {}
        }
        blocker.commit().await?;
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        if mode == "unchanged" {
            assert_eq!(shown(&reply)?.jobs.len(), 2);
        } else {
            assert!(
                !reply.status.is_success(),
                "{mode} cannot publish the earlier job set"
            );
        }
    }
    Ok(())
}
