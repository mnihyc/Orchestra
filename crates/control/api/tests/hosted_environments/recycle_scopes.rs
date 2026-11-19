//! Actual per-candidate recycling, native process/stop commands and current completed Read.

use super::*;
use orch_api_contract::environment::{NodeEnvironmentRecycle, RecycleBody, RecycleReply};
use std::time::Duration;

const OP: &str = "NodeEnvironmentRecycle";
const WAIT: Duration = Duration::from_secs(30);
fn wanted(dry_run: bool, processes: bool) -> RecycleBody {
    RecycleBody {
        idle_minutes: 0,
        dry_run,
        include_with_processes: Some(processes),
    }
}
fn grant(s: &Scene, id: Id, actions: &[&str]) -> Json {
    rule(OP, "environment", id, "self", &["environment"], actions)
}
async fn call(s: &Scene, token: &str, body: &RecycleBody) -> RawReply {
    let call = s.world.app.call(NodeEnvironmentRecycle, &[&s.team.topic]);
    call.token(token).body(body).send_bytes().await
}
fn success(reply: &RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn status(s: &Scene, id: Id) -> String {
    sqlx::query_scalar("SELECT status FROM environments WHERE id=$1")
        .bind(id)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap()
}
async fn stops(s: &Scene) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='environment.stopped'")
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap()
}
async fn commands(s: &Scene) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE kind IN ('proc.list','node.runtime.stop')",
    )
    .fetch_one(&s.world.db.pool)
    .await
    .unwrap()
}
async fn runtime(s: &Scene, id: Id) -> Id {
    sqlx::query_scalar(
        "SELECT id FROM runtimes WHERE environment_id=$1 AND state NOT IN ('retired','removed')",
    )
    .bind(id)
    .fetch_one(&s.world.db.pool)
    .await
    .unwrap()
}
async fn sql_wait(s: &Scene, pid: i32, pattern: &str, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(WAIT,async {
        loop {
            assert!(!request.is_finished(),"the native request must reach the real SQL wait");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
                .bind(pid).bind(pattern).fetch_one(&s.world.db.pool).await.unwrap();
            if blocked {break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("the actual native recycle SQL wait");
}
/// The same native protocol peer role as resources::answer, with typed process/stop results.
fn peer(s: &Scene, expected: usize) -> tokio::task::JoinHandle<Vec<(String, Json)>> {
    let pool = s.world.db.pool.clone();
    tokio::spawn(async move {
        tokio::time::timeout(WAIT,async {
            let mut answered=Vec::new();
            while answered.len()<expected {
                let rows:Vec<(Id,String,Json)>=sqlx::query_as("SELECT id,kind,body FROM commands WHERE kind IN ('proc.list','node.runtime.stop') AND state IN ('queued','sent','accepted') ORDER BY created_at,id")
                    .fetch_all(&pool).await.unwrap();
                for (id,kind,body) in rows {
                    let result=if kind=="proc.list" {json!({"processes":[]})} else {json!({})};
                    let done=sqlx::query("UPDATE commands SET state='succeeded',result=$2 WHERE id=$1 AND state IN ('queued','sent','accepted')")
                        .bind(id).bind(result).execute(&pool).await.unwrap().rows_affected();
                    if done>0 {answered.push((kind,body));}
                }
                tokio::task::yield_now().await;
            }
            answered
        }).await.expect("the actual native peer commands must drain")
    })
}

#[tokio::test]
async fn recycle_scope_selects_actual_manageable_environments_and_keeps_native_dry_run_without_read()
 {
    let s = Scene::new().await;
    let native = call(&s, &s.team.c.admin.token, &wanted(true, true)).await;
    let native: RecycleReply = serde_json::from_value(success(&native)).unwrap();
    assert_eq!(
        native.candidates.len(),
        2,
        "the actual native eligibility premise"
    );
    let actor = key(
        &s.world,
        s.team.c.admin.id(),
        vec![
            grant(&s, s.first.base.id, &["manage"]),
            rule(
                "NodeEnvironmentList",
                "environment",
                s.first.base.id,
                "self",
                &["environment"],
                &["read"],
            ),
        ],
    )
    .await;
    let value = success(&call(&s, &actor.secret, &wanted(true, true)).await);
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["candidates"], 1);
    assert_eq!(value["stopped"], 0);
    assert_eq!(value["accepted"], 0);
    assert_eq!(value["skipped_no_permission"], 1);
    assert!(value.get("environment").is_none());
    assert_eq!(status(&s, s.first.base.id).await, "running");
    assert_eq!(status(&s, s.second.base.id).await, "running");
    assert_eq!((stops(&s).await, commands(&s).await), (0, 0));
    let reader = key(
        &s.world,
        s.team.c.admin.id(),
        vec![
            grant(&s, s.first.base.id, &["manage", "read"]),
            s.sources(OP),
        ],
    )
    .await;
    let full: RecycleReply = serde_json::from_value(success(
        &call(&s, &reader.secret, &wanted(true, true)).await,
    ))
    .unwrap();
    assert_eq!(full.candidates.len(), 1);
    assert_eq!(full.candidates[0].environment.id, s.first.base.id);
    assert_eq!(
        full.candidates[0].environment,
        native
            .candidates
            .iter()
            .find(|row| row.environment.id == s.first.base.id)
            .unwrap()
            .environment
    );
    let viewer = key(
        &s.world,
        s.team.c.viewer.id(),
        vec![
            grant(&s, s.first.base.id, &["manage", "read"]),
            s.sources(OP),
        ],
    )
    .await;
    let full: RecycleReply = serde_json::from_value(success(
        &call(&s, &viewer.secret, &wanted(true, true)).await,
    ))
    .unwrap();
    assert!(full.candidates.is_empty());
    assert_eq!(full.skipped_no_permission, 2);
    assert_eq!((stops(&s).await, commands(&s).await), (0, 0));
}

#[tokio::test]
async fn recycle_manage_only_stops_its_actual_unit_once_and_preserves_the_other_candidates() {
    let s = Scene::new().await;
    let actor = key(
        &s.world,
        s.team.c.admin.id(),
        vec![grant(&s, s.first.base.id, &["manage"])],
    )
    .await;
    let unit = runtime(&s, s.first.base.id).await;
    let answering = peer(&s, 1);
    let value = success(&call(&s, &actor.secret, &wanted(false, true)).await);
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["stopped"], 1);
    assert_eq!(value["accepted"], 1);
    assert_eq!(value["remaining"], 0);
    assert_eq!(status(&s, s.first.base.id).await, "stopped");
    assert_eq!(status(&s, s.second.base.id).await, "running");
    let answered = answering.await.unwrap();
    assert_eq!(answered.len(), 1);
    assert_eq!(answered[0].0, "node.runtime.stop");
    assert_eq!(answered[0].1["runtime_id"], json!(unit));
    assert_eq!(stops(&s).await, 1);
}

#[tokio::test]
async fn recycle_checks_the_original_raw_key_after_the_real_environment_row_wait() {
    for expired in [false, true] {
        let s = Scene::new().await;
        let actor = key(
            &s.world,
            s.team.c.admin.id(),
            vec![grant(&s, s.first.base.id, &["manage"])],
        )
        .await;
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
            .bind(s.first.base.id)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.world.app.clone();
        let node = s.team.topic;
        let secret = actor.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(NodeEnvironmentRecycle, &[&node]);
            call.token(&secret)
                .body(&wanted(false, true))
                .send_bytes()
                .await
        });
        sql_wait(&s, pid, "%FROM environments%FOR UPDATE%", &request).await;
        assert_eq!((stops(&s).await, commands(&s).await), (0, 0));
        if expired {
            s.world.advance(chrono::Duration::hours(2));
        }
        let answering = if expired { None } else { Some(peer(&s, 1)) };
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if expired {
            assert!(matches!(
                reply.status,
                StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED
            ));
            assert_eq!((stops(&s).await, commands(&s).await), (0, 0));
        } else {
            assert_eq!(success(&reply)["stopped"], 1);
            answering.unwrap().await.unwrap();
        }
    }
}

#[tokio::test]
async fn completed_recycle_read_loss_after_the_real_activity_wait_keeps_its_stopped_receipt() {
    for closed in [false, true] {
        let s = Scene::new().await;
        let actor = key(
            &s.world,
            s.team.c.admin.id(),
            vec![
                grant(&s, s.first.base.id, &["manage", "read"]),
                s.sources(OP),
            ],
        )
        .await;
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let answering = peer(&s, 1);
        let app = s.world.app.clone();
        let node = s.team.topic;
        let secret = actor.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(NodeEnvironmentRecycle, &[&node]);
            call.token(&secret)
                .body(&wanted(false, true))
                .send_bytes()
                .await
        });
        sql_wait(&s, pid, "%INSERT INTO api_token_activity%", &request).await;
        assert_eq!(status(&s, s.first.base.id).await, "stopped");
        assert_eq!(stops(&s).await, 1);
        if closed {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(actor.id)
                .bind(json!({"rules":[grant(&s,s.first.base.id,&["manage"])]}))
                .execute(&s.world.db.pool)
                .await
                .unwrap();
        }
        let before:(String,String)=sqlx::query_as("SELECT e.xmin::text,r.xmin::text FROM environments e JOIN runtimes r ON r.environment_id=e.id WHERE e.id=$1")
            .bind(s.first.base.id).fetch_one(&s.world.db.pool).await.unwrap();
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        let value = success(&reply);
        assert_eq!(value["stopped"], 1);
        if closed {
            assert_eq!(value["projection"], "receipt");
            assert_eq!(value["accepted"], 1);
        } else {
            assert_eq!(
                value["candidates"][0]["environment"]["id"],
                json!(s.first.base.id)
            );
            assert!(value.get("projection").is_none());
        }
        let after:(String,String)=sqlx::query_as("SELECT e.xmin::text,r.xmin::text FROM environments e JOIN runtimes r ON r.environment_id=e.id WHERE e.id=$1")
            .bind(s.first.base.id).fetch_one(&s.world.db.pool).await.unwrap();
        assert_eq!(before, after);
        assert_eq!(stops(&s).await, 1);
        assert_eq!(commands(&s).await, 1);
        answering.await.unwrap();
    }
}

#[tokio::test]
async fn recycle_process_probe_carries_its_real_origin_and_rechecks_before_any_stop() {
    for close in [false, true] {
        let s = Scene::new().await;
        let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
            .bind(s.team.host)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE environments SET machine_id=$2 WHERE id=$1")
            .bind(s.first.base.id)
            .bind(machine)
            .execute(&s.world.db.pool)
            .await
            .unwrap();
        let actor = key(
            &s.world,
            s.team.c.admin.id(),
            vec![grant(&s, s.first.base.id, &["manage"])],
        )
        .await;
        let app = s.world.app.clone();
        let node = s.team.topic;
        let secret = actor.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(NodeEnvironmentRecycle, &[&node]);
            call.token(&secret)
                .body(&wanted(false, false))
                .send_bytes()
                .await
        });
        let command=tokio::time::timeout(WAIT,async {
            loop {
                assert!(!request.is_finished(),"the actual native process query must be queued");
                let command:Option<Id>=sqlx::query_scalar("SELECT id FROM commands WHERE kind='proc.list' AND state IN ('queued','sent','accepted') ORDER BY created_at,id LIMIT 1").fetch_optional(&s.world.db.pool).await.unwrap();
                if let Some(command)=command {break command;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the real process command journal");
        let evidence =
            orch_db::queries::link::command_authority(&mut *s.world.conn().await, command)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(evidence["kind"], "recycle_probe");
        assert_eq!(evidence["authority"]["input"]["origin"]["operation"], OP);
        assert_eq!(
            evidence["authority"]["input"]["association"]["environment"],
            json!(s.first.base.id)
        );
        assert_eq!(
            evidence["authority"]["body"],
            json!({"environment_id":s.first.base.id})
        );
        assert_eq!(
            (stops(&s).await, status(&s, s.first.base.id).await),
            (0, "running".into())
        );
        if close {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(actor.id)
                .bind(json!({"rules":[]}))
                .execute(&s.world.db.pool)
                .await
                .unwrap();
        }
        sqlx::query("UPDATE commands SET state='succeeded',result=$2 WHERE id=$1")
            .bind(command)
            .bind(json!({"processes":[]}))
            .execute(&s.world.db.pool)
            .await
            .unwrap();
        let answering = if close { None } else { Some(peer(&s, 1)) };
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if close {
            assert!(matches!(
                reply.status,
                StatusCode::OK | StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED
            ));
            assert_eq!(stops(&s).await, 0);
            assert_eq!(commands(&s).await, 1);
            assert_eq!(status(&s, s.first.base.id).await, "running");
        } else {
            assert_eq!(success(&reply)["stopped"], 1);
            assert_eq!(stops(&s).await, 1);
            assert_eq!(commands(&s).await, 2);
            answering.unwrap().await.unwrap();
        }
    }
}
