//! Actual stored recovery-batch members, native job cuts and frozen job associations.

use super::*;
use orch_api_contract::environment::EnvironmentBatch;
use orch_api_contract::loss::EnvironmentRecoverGet;
use orch_db::queries::jobs as job_rows;

const OPERATION: &str = "EnvironmentRecoverGet";

struct Batch {
    id: Id,
    first: Id,
}

async fn batch(s: &Scene) -> Batch {
    let id = new_id();
    let mut rows = Vec::new();
    for (environment, marker) in [
        (s.first.base.id, "first-batch-payload-marker"),
        (s.second.base.id, "second-batch-payload-marker"),
    ] {
        let job: Id = sqlx::query_scalar(
            "SELECT id FROM jobs WHERE environment_id=$1 ORDER BY created_at,id LIMIT 1",
        )
        .bind(environment)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
        sqlx::query("UPDATE jobs SET batch_id=$2,result=$3 WHERE id=$1")
            .bind(job)
            .bind(id)
            .bind(json!({"native_result":marker}))
            .execute(&s.world.db.pool)
            .await
            .unwrap();
        rows.push(job);
    }
    Batch { id, first: rows[0] }
}

fn rules(ids: &[Id]) -> Vec<Json> {
    ids.iter()
        .map(|id| {
            rule(
                OPERATION,
                "environment",
                *id,
                "self",
                &["environment"],
                &["read"],
            )
        })
        .collect()
}

async fn call(s: &Scene, token: &str, batch: Id) -> RawReply {
    s.world
        .app
        .call(EnvironmentRecoverGet, &[&batch])
        .token(token)
        .send_bytes()
        .await
}

async fn shown(s: &Scene, token: &str, batch: Id) -> EnvironmentBatch {
    let reply = call(s, token, batch).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn activity_wait(s: &Scene, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!request.is_finished(),"the stored native batch body must reach activity append");
            let blocked:bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the actual activity INSERT after native snapshot completion");
}

#[tokio::test]
async fn recovery_batch_uses_actual_member_environments_exact_scope_and_the_native_job_body() {
    let s = Scene::new().await;
    let b = batch(&s).await;
    let native = shown(&s, &s.team.c.admin.token, b.id).await;
    assert_eq!(native.jobs.len(), 2);
    let all = key(
        &s.world,
        s.team.c.admin.id(),
        rules(&[s.first.base.id, s.second.base.id]),
    )
    .await;
    assert_eq!(shown(&s, &all.secret, b.id).await, native);
    let first = key(&s.world, s.team.c.admin.id(), rules(&[s.first.base.id])).await;
    let selected = shown(&s, &first.secret, b.id).await;
    assert_eq!(selected.jobs.len(), 1);
    assert_eq!(selected.jobs[0].job.id, b.first);
    assert_eq!(
        selected.jobs[0],
        *native
            .jobs
            .iter()
            .find(|member| member.job.id == b.first)
            .unwrap()
    );
    assert!(
        !serde_json::to_string(&selected)
            .unwrap()
            .contains("second-batch-payload-marker")
    );
    for policy in [
        vec![rule(
            "JobGet",
            "environment",
            s.first.base.id,
            "self",
            &["environment"],
            &["read"],
        )],
        vec![rule(
            OPERATION,
            "environment",
            s.first.base.id,
            "self",
            &["environment"],
            &["manage"],
        )],
    ] {
        let denied = key(&s.world, s.team.c.admin.id(), policy).await;
        let reply = call(&s, &denied.secret, b.id).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert!(!String::from_utf8_lossy(&reply.raw).contains("batch-payload-marker"));
    }
    let opaque = key(&s.world, s.team.c.admin.id(), rules(&[b.id])).await;
    let reply = call(&s, &opaque.secret, b.id).await;
    assert!(
        matches!(reply.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "a stored batch UUID is not an Environment identity"
    );
    assert!(!String::from_utf8_lossy(&reply.raw).contains("batch-payload-marker"));
    let stranger = key(&s.world, s.team.c.stranger.id(), rules(&[s.first.base.id])).await;
    let reply = call(&s, &stranger.secret, b.id).await;
    assert!(
        matches!(reply.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "scope cannot lend native job visibility"
    );
    assert!(!String::from_utf8_lossy(&reply.raw).contains("batch-payload-marker"));
}

#[tokio::test]
async fn recovery_batch_machine_names_follow_actual_native_sources_and_the_host_facade() {
    let s = Scene::new().await;
    let b = batch(&s).await;
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(s.team.host)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,'{active_command}',$2,true) WHERE id=$1")
        .bind(b.first)
        .bind(json!({"machine":machine}))
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let native = shown(&s, &s.team.c.admin.token, b.id).await;
    let expected = native
        .jobs
        .iter()
        .find(|member| member.job.id == b.first)
        .unwrap();
    assert!(
        matches!(
            expected
                .job
                .progress
                .activity
                .as_ref()
                .and_then(|a| a.waiting_for.as_ref()),
            Some(orch_api_contract::workspace::JobWaiting::Machine { .. })
        ),
        "the ordinary source really names a machine"
    );
    let mut wrong = rules(&[s.first.base.id]);
    wrong.push(rule(
        OPERATION,
        "machine",
        machine,
        "self",
        &["machine"],
        &["read"],
    ));
    let hidden = key(&s.world, s.team.c.admin.id(), wrong).await;
    let selected = shown(&s, &hidden.secret, b.id).await;
    assert!(
        selected.jobs[0]
            .job
            .progress
            .activity
            .as_ref()
            .unwrap()
            .waiting_for
            .is_none(),
        "the physical UUID is not a managed Machine's actual Host governor"
    );
    let mut policy = rules(&[s.first.base.id]);
    policy.push(rule(
        OPERATION,
        "host",
        s.team.host,
        "self",
        &["machine"],
        &["read"],
    ));
    let full = key(&s.world, s.team.c.admin.id(), policy).await;
    let selected = shown(&s, &full.secret, b.id).await;
    assert_eq!(
        selected.jobs[0], *expected,
        "exact same-operation source Read retains every native job field"
    );
}

#[tokio::test]
async fn recovery_batch_publication_cannot_replace_a_job_at_the_same_environment_governor() {
    for replace in [false, true] {
        let s = Scene::new().await;
        let b = batch(&s).await;
        let reader = key(&s.world, s.team.c.admin.id(), rules(&[s.first.base.id])).await;
        assert_eq!(
            shown(&s, &reader.secret, b.id).await.jobs[0].job.id,
            b.first
        );
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.world.app.clone();
        let id = b.id;
        let secret = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(EnvironmentRecoverGet, &[&id]);
            call.token(&secret).send_bytes().await
        });
        activity_wait(&s, pid, &request).await;
        if replace {
            let original = job_rows::job(&mut *s.world.conn().await, b.first)
                .await
                .unwrap()
                .unwrap();
            let new = new_id();
            job_rows::insert(
                &mut *s.world.conn().await,
                &job_rows::NewJob {
                    id: new,
                    kind: &original.kind,
                    node_id: original.node_id,
                    environment_id: original.environment_id,
                    provider_id: original.provider_id,
                    plan: &original.plan,
                    progress: &original.progress,
                    created_by: original.created_by,
                },
                s.world.now(),
            )
            .await
            .unwrap();
            sqlx::query("UPDATE jobs SET state='cancelled',batch_id=$2,result=$3 WHERE id=$1")
                .bind(new)
                .bind(b.id)
                .bind(json!({"native_result":"replacement-batch-payload-marker"}))
                .execute(&s.world.db.pool)
                .await
                .unwrap();
            sqlx::query("UPDATE jobs SET batch_id=NULL WHERE id=$1")
                .bind(b.first)
                .execute(&s.world.db.pool)
                .await
                .unwrap();
            let native = shown(&s, &s.team.c.admin.token, b.id).await;
            assert!(native.jobs.iter().any(|member| member.job.id == new));
            assert!(!native.jobs.iter().any(|member| member.job.id == b.first));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if replace {
            assert_eq!(
                reply.status,
                StatusCode::FORBIDDEN,
                "a new job at the same Env cannot validate an earlier member"
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("first-batch-payload-marker"));
        } else {
            assert_eq!(reply.status, StatusCode::OK);
        }
    }
}

#[tokio::test]
async fn recovery_batch_publication_preserves_the_original_full_versus_progress_only_native_cut() {
    for withdraw in [false, true] {
        let s = Scene::new().await;
        let b = batch(&s).await;
        sqlx::query("UPDATE jobs SET kind=$2 WHERE id=$1")
            .bind(b.first)
            .bind(job_rows::kind::WORKSPACE_PREPARE)
            .execute(&s.world.db.pool)
            .await
            .unwrap();
        resources::name_person(
            &s.world,
            "environment",
            s.first.base.id,
            "environment.use",
            s.team.c.stranger.id(),
        )
        .await;
        let files = resources::name_person(
            &s.world,
            "environment",
            s.first.base.id,
            "files.view",
            s.team.c.stranger.id(),
        )
        .await;
        let reader = key(&s.world, s.team.c.stranger.id(), rules(&[s.first.base.id])).await;
        let original = shown(&s, &reader.secret, b.id).await;
        assert_eq!(original.jobs.len(), 1);
        assert_eq!(
            original.jobs[0].job.result.as_ref().unwrap()["native_result"],
            "first-batch-payload-marker",
            "the actual native Files View gate currently permits the full stored job result"
        );
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.world.app.clone();
        let id = b.id;
        let secret = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(EnvironmentRecoverGet, &[&id]);
            call.token(&secret).send_bytes().await
        });
        activity_wait(&s, pid, &request).await;
        if withdraw {
            assert!(
                orch_db::queries::access::remove_entry(
                    &mut *s.world.conn().await,
                    files,
                    s.team.c.admin.id(),
                    s.world.now()
                )
                .await
                .unwrap()
            );
            let current = shown(&s, &s.team.c.stranger.token, b.id).await;
            assert_eq!(current.jobs.len(), 1);
            assert_eq!(current.jobs[0].job.id, b.first);
            assert!(
                current.jobs[0].job.result.is_none(),
                "native Use still permits the safe progress cut after Files View is gone"
            );
            assert!(current.jobs[0].job.log.is_none());
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if withdraw {
            assert_eq!(
                reply.status,
                StatusCode::FORBIDDEN,
                "a current progress-only read cannot validate earlier full stored content"
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("first-batch-payload-marker"));
        } else {
            assert_eq!(reply.status, StatusCode::OK);
        }
    }
}
