//! Cancellation keeps native per-member commits and current original key scope.

use super::*;
use orch_api_core::testing::RawReply;
use thread_history_scope::{Proof, key, narrow, rule};

const OP: &str = "ForkBatchCancel";

async fn running() -> (Forks, [Id; 2], ForkBatch) {
    let w = Forks::new().await;
    let a = w.thread_with_history("First cancellation member").await;
    let b = w.thread_with_history("Second cancellation member").await;
    w.s.rig.machine.handler.hold(CommandKind::SessionExport);
    let plan = w.batch_plan(&w.s.ada, &w.batch_of(vec![a, b])).await.ok();
    assert_eq!(plan.executable, 2);
    let batch = w.batch_create(&w.s.ada, &plan).await.ok();
    w.export_begun().await;
    (w, [a, b], batch)
}

fn grants(threads: [Id; 2], read: bool) -> Vec<Json> {
    threads
        .into_iter()
        .flat_map(|thread| {
            let mut rules = vec![rule(OP, "thread", thread, "self", "manage")];
            if read {
                rules.push(rule(OP, "thread", thread, "self", "read"));
            }
            rules
        })
        .collect()
}

async fn cancel(w: &Forks, batch: Id, secret: &str) -> RawReply {
    let request =
        w.s.rig
            .world
            .app
            .call(ForkBatchCancel, &[&batch])
            .token(secret);
    request.send_bytes().await
}

fn body(reply: &RawReply) -> Proof<Json> {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    Ok(serde_json::from_slice(&reply.raw)?)
}

async fn finish(w: &Forks, batch: &ForkBatch) {
    w.s.rig.machine.handler.release();
    eventually("native batch members ended", || async {
        w.s.rig
            .engine
            .batch_jobs(batch.batch_id)
            .await
            .unwrap()
            .iter()
            .all(|row| !row.is_open())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_requires_exact_manage_on_every_remaining_subject_and_its_native_creator_and_returns_receipts()
-> Proof {
    let (w, threads, batch) = running().await;
    let world = &w.s.rig.world;
    for rules in [
        vec![rule(OP, "thread", threads[0], "self", "manage")],
        threads
            .into_iter()
            .map(|id| rule(OP, "thread", id, "self", "read"))
            .collect(),
        threads
            .into_iter()
            .map(|id| rule("ThreadInputCreate", "thread", id, "self", "write"))
            .collect(),
        vec![rule(OP, "thread", orch_types::new_id(), "self", "manage")],
    ] {
        let selected = key(world, w.s.ada.id(), rules).await?;
        assert!(
            !cancel(&w, batch.batch_id, &selected.secret)
                .await
                .status
                .is_success()
        );
    }
    let not_creator = key(world, w.s.bea.id(), grants(threads, false)).await?;
    assert!(
        !cancel(&w, batch.batch_id, &not_creator.secret)
            .await
            .status
            .is_success()
    );
    let changed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE batch_id=$1 AND (state='cancelled' OR plan ? 'cancel')",
    )
    .bind(batch.batch_id)
    .fetch_one(&world.db.pool)
    .await?;
    assert_eq!(changed, 0, "a refused act cancels no native member");
    let selected = key(world, w.s.ada.id(), grants(threads, false)).await?;
    let reply = body(&cancel(&w, batch.batch_id, &selected.secret).await)?;
    assert_eq!(reply["projection"], "receipt");
    assert_eq!(reply["batch_id"], json!(batch.batch_id));
    assert_eq!(reply["complete"], true);
    assert!(
        reply["cancelled"].as_array().map_or(0, Vec::len)
            + reply["requested"].as_array().map_or(0, Vec::len)
            > 0
    );
    for old in ["jobs", "counts", "hidden", "percent"] {
        assert!(reply.get(old).is_none());
    }
    finish(&w, &batch).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_full_readback_needs_its_own_read_and_preserves_the_native_result() -> Proof {
    for own_read in [false, true] {
        let (w, threads, batch) = running().await;
        let mut rules = grants(threads, own_read);
        if !own_read {
            rules.extend(
                threads
                    .into_iter()
                    .map(|id| rule("ForkBatchGet", "thread", id, "self", "read")),
            );
        }
        let selected = key(&w.s.rig.world, w.s.ada.id(), rules).await?;
        let reply = body(&cancel(&w, batch.batch_id, &selected.secret).await)?;
        assert_eq!(reply.get("projection").is_none(), own_read);
        if own_read {
            let shown: ForkBatch = serde_json::from_value(reply)?;
            assert_eq!((shown.jobs.len(), shown.hidden), (2, 0));
            for job in &shown.jobs {
                assert!(threads.contains(&job.job.node_id.ok_or("native job lost its thread")?));
            }
        } else {
            assert_eq!(reply["projection"], "receipt");
        }
        finish(&w, &batch).await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_keeps_the_first_committed_member_and_refuses_new_work_after_the_actual_next_row_wait()
-> Proof {
    for mode in ["unchanged", "expired"] {
        let (w, threads, batch) = running().await;
        let world = &w.s.rig.world;
        let selected = key(world, w.s.ada.id(), grants(threads, false)).await?;
        let rows =
            orch_db::queries::jobs::forks::of_batch(&mut *world.conn().await, batch.batch_id)
                .await?;
        assert_eq!(rows.len(), 2);
        let first_id = rows[0].id;
        let second_id = rows[1].id;
        let mut blocker = world.db.pool.begin().await?;
        orch_db::queries::jobs::lock(&mut blocker, second_id)
            .await?
            .ok_or("native second member missing")?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = world.app.clone();
        let secret = selected.secret.clone();
        let batch_id = batch.batch_id;
        let pending = tokio::spawn(async move {
            let request = app.call(ForkBatchCancel, &[&batch_id]).token(&secret);
            request.send_bytes().await
        });
        tokio::time::timeout(rig::PATIENCE, async {
            loop {
                assert!(!pending.is_finished(), "the actual next-member wait must be reached");
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%FROM jobs%FOR UPDATE%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                let first_done: bool = sqlx::query_scalar("SELECT state='cancelled' OR plan ? 'cancel' FROM jobs WHERE id=$1")
                    .bind(first_id).fetch_one(&world.db.pool).await.unwrap();
                if waiting && first_done { break; }
                tokio::task::yield_now().await;
            }
        }).await?;
        match mode {
            "expired" => world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        blocker.commit().await?;
        let reply = body(&tokio::time::timeout(rig::PATIENCE, pending).await??)?;
        assert_eq!(reply["projection"], "receipt");
        assert_eq!(reply["complete"], mode == "unchanged");
        let affected = reply["cancelled"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(reply["requested"].as_array().into_iter().flatten())
            .collect::<Vec<_>>();
        assert!(affected.iter().any(|id| **id == json!(first_id)));
        if mode != "unchanged" {
            assert!(!affected.iter().any(|id| **id == json!(second_id)));
            let second_done: bool = sqlx::query_scalar(
                "SELECT state='cancelled' OR plan ? 'cancel' FROM jobs WHERE id=$1",
            )
            .bind(second_id)
            .fetch_one(&world.db.pool)
            .await?;
            assert!(
                !second_done,
                "unaccepted next work cannot borrow the first member's completion"
            );
        }
        finish(&w, &batch).await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn completed_cancellation_keeps_its_receipt_and_does_not_re_admit_manage_after_the_publication_wait()
-> Proof {
    for mode in ["unchanged", "read", "manage", "association"] {
        let (w, threads, batch) = running().await;
        let world = &w.s.rig.world;
        let selected = key(world, w.s.ada.id(), grants(threads, true)).await?;
        let mut blocker = world.db.pool.begin().await?;
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = world.app.clone();
        let secret = selected.secret.clone();
        let batch_id = batch.batch_id;
        let pending = tokio::spawn(async move {
            let request = app.call(ForkBatchCancel, &[&batch_id]).token(&secret);
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
            "read" => narrow(world, &selected, grants(threads, false)).await?,
            "manage" => {
                narrow(
                    world,
                    &selected,
                    threads
                        .into_iter()
                        .map(|id| rule(OP, "thread", id, "self", "read"))
                        .collect(),
                )
                .await?
            }
            "association" => {
                let first = batch
                    .jobs
                    .iter()
                    .find(|job| job.job.node_id == Some(threads[0]))
                    .ok_or("first native job missing")?
                    .job
                    .id;
                let second = batch
                    .jobs
                    .iter()
                    .find(|job| job.job.node_id == Some(threads[1]))
                    .ok_or("second native job missing")?
                    .job
                    .id;
                sqlx::query("UPDATE jobs SET node_id=CASE WHEN id=$1 THEN $3 ELSE $4 END WHERE id=$1 OR id=$2")
                    .bind(first).bind(second).bind(threads[1]).bind(threads[0]).execute(&world.db.pool).await?;
            }
            _ => {}
        }
        blocker.commit().await?;
        let reply = body(&tokio::time::timeout(rig::PATIENCE, pending).await??)?;
        assert_eq!(
            reply.get("projection").is_none(),
            matches!(mode, "unchanged" | "manage")
        );
        if matches!(mode, "read" | "association") {
            assert_eq!(reply["projection"], "receipt");
            assert_eq!(reply["complete"], true);
            assert!(reply.get("jobs").is_none());
        }
        finish(&w, &batch).await;
    }
    Ok(())
}
