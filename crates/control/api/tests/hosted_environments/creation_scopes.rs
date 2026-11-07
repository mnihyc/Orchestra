//! Standalone creation uses real native placement sources and its own optional singleton Read.
use super::*;
use orch_api_contract::environment::{EnvironmentCreateReceipt, EnvironmentCreateResponse};
use tokio::task::JoinHandle;

const OP: &str = "EnvironmentCreate";
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
fn write(s: &Scene) -> Vec<Json> {
    vec![
        rule(
            OP,
            "topic",
            s.team.topic,
            "self",
            &["environment"],
            &["manage"],
        ),
        rule(
            OP,
            "template",
            s.team.template,
            "self",
            &["template"],
            &["read"],
        ),
        rule(OP, "host", s.team.host, "self", &["host"], &["write"]),
    ]
}
fn full_scope(s: &Scene) -> Vec<Json> {
    vec![rule(
        OP,
        "org",
        s.team.c.acme,
        "subtree",
        &[
            "environment",
            "template",
            "host",
            "machine",
            "mount",
            "org",
            "project",
            "topic",
            "thread",
            "group",
        ],
        &["manage", "write", "read"],
    )]
}
fn body(s: &Scene, name: &str) -> NewEnvironment {
    let mut body = wanted(&s.team, name);
    body.host_id = Some(Some(s.team.host));
    body
}
async fn call(s: &Scene, token: &str, name: &str) -> RawReply {
    let call = s.world.app.call(EnvironmentCreate, &[]);
    call.token(token).body(&body(s, name)).send_bytes().await
}
fn start(s: &Scene, token: String, name: &str) -> JoinHandle<RawReply> {
    let app = s.world.app.clone();
    let body = body(s, name);
    tokio::spawn(async move {
        let call = app.call(EnvironmentCreate, &[]);
        call.token(&token).body(&body).send_bytes().await
    })
}
fn receipt(reply: &RawReply) -> EnvironmentCreateReceipt {
    assert_eq!(
        reply.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice::<EnvironmentCreateResponse>(&reply.raw)
        .unwrap()
        .into_full()
        .unwrap_err()
}
fn full(reply: &RawReply) -> Environment {
    assert_eq!(
        reply.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice::<EnvironmentCreateResponse>(&reply.raw)
        .unwrap()
        .into_full()
        .unwrap()
}
async fn count(s: &Scene) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM environments WHERE name LIKE 'Creation scope%'")
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap()
}
async fn wait(s: &Scene, pid: i32, pattern: &str, request: &JoinHandle<RawReply>) {
    tokio::time::timeout(WAIT,async {loop {
        assert!(!request.is_finished(),"the actual native creator reaches its SQL boundary");
        let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
            .bind(pid).bind(pattern).fetch_one(&s.world.db.pool).await.unwrap();
        if blocked {break;}tokio::task::yield_now().await;
    }}).await.expect("actual native creation SQL wait");
}

#[tokio::test]
async fn standalone_creation_requires_exact_home_template_and_physical_placement_scopes() {
    let s = Scene::new().await;
    let mut refusals = Vec::new();
    for missing in 0..write(&s).len() {
        let mut rules = write(&s);
        rules.remove(missing);
        refusals.push(rules);
    }
    refusals.push(vec![rule(
        "EnvironmentUpdate",
        "topic",
        s.team.topic,
        "self",
        &["environment"],
        &["manage"],
    )]);
    let mut wrong = write(&s);
    wrong[0] = rule(OP, "topic", new_id(), "self", &["environment"], &["manage"]);
    refusals.push(wrong);
    for rules in refusals {
        let token = key(&s.world, s.team.c.admin.id(), rules).await;
        assert_eq!(
            call(&s, &token.secret, "Creation scope refused")
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(count(&s).await, 0);
    }
    let stranger = key(&s.world, s.team.c.stranger.id(), write(&s)).await;
    assert!(matches!(
        call(&s, &stranger.secret, "Creation scope stranger")
            .await
            .status,
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ));
    assert_eq!(count(&s).await, 0);
    let token = key(&s.world, s.team.c.admin.id(), write(&s)).await;
    let reply = call(&s, &token.secret, "Creation scope accepted").await;
    let made = receipt(&reply);
    assert_eq!(
        serde_json::from_slice::<Json>(&reply.raw).unwrap(),
        json!({"projection":"receipt","environment_id":made.environment_id,"job_id":made.job_id,"revision":made.revision})
    );
    let row: (Id, Id, Json) =
        sqlx::query_as("SELECT environment_id,created_by,plan FROM jobs WHERE id=$1")
            .bind(made.job_id)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
    assert_eq!(row.0, made.environment_id);
    assert_eq!(row.1, s.team.c.admin.id());
    assert_eq!(row.2["creator_provision"]["standalone"], true);
    assert_eq!(row.2["creator_provision"]["origin"]["operation"], OP);
    assert!(
        row.2["creator_provision"].get("thread").is_none(),
        "a standalone creator never invents a Thread"
    );
    assert_eq!(count(&s).await, 1);
}

#[tokio::test]
async fn standalone_creation_full_content_keeps_the_native_ordinary_environment_shape() {
    let s = Scene::new().await;
    let token = key(&s.world, s.team.c.admin.id(), full_scope(&s)).await;
    let created = full(&call(&s, &token.secret, "Creation scope full").await);
    assert_eq!(created.base.name, "Creation scope full");
    assert_eq!(created.home.id, s.team.topic);
    assert_eq!(created.generation, 0);
    let ordinary = full(&call(&s, &s.team.c.admin.token, "Creation scope ordinary").await);
    assert_eq!(ordinary.base.status, created.base.status);
    assert_eq!(ordinary.base.placement, created.base.placement);
    assert_eq!(ordinary.template, created.template);
    assert_eq!(ordinary.home, created.home);
    assert_eq!(ordinary.config, created.config);
    let mut wrong_read = write(&s);
    wrong_read.push(rule(
        "EnvironmentGet",
        "topic",
        s.team.topic,
        "subtree",
        &["environment"],
        &["read"],
    ));
    let token = key(&s.world, s.team.c.admin.id(), wrong_read).await;
    receipt(&call(&s, &token.secret, "Creation scope other read").await);
}

#[tokio::test]
async fn standalone_initial_files_require_the_real_source_thread_and_environment_before_planning() {
    use orch_api_contract::binding::NewEnvironmentFiles;
    let s = Scene::new().await;
    let thread = new_id();
    assert!(
        orch_db::queries::threads::insert_thread(
            &mut *s.world.conn().await,
            thread,
            s.team.topic,
            "Initial source",
            s.team.c.admin.id(),
            s.world.now()
        )
        .await
        .unwrap()
    );
    sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(s.first.base.id)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let mut request = body(&s, "Creation scope missing initial read");
    request.files = Some(NewEnvironmentFiles::FromThread {
        from_thread: thread,
        selection: None,
    });
    for (kind, id) in [("thread", thread), ("environment", s.first.base.id)] {
        let mut rules = write(&s);
        rules.push(rule(OP, kind, id, "self", &[kind], &["read"]));
        let token = key(&s.world, s.team.c.admin.id(), rules).await;
        let reply = s
            .world
            .app
            .call(EnvironmentCreate, &[])
            .token(&token.secret)
            .body(&request)
            .send_bytes()
            .await;
        assert_eq!(
            reply.status,
            StatusCode::FORBIDDEN,
            "each real initial source has independent Read"
        );
        assert_eq!(count(&s).await, 0);
    }
}

#[tokio::test]
async fn standalone_creation_checks_actual_key_time_after_its_first_insert_wait() {
    for expired in [false, true] {
        let s = Scene::new().await;
        let token = key(&s.world, s.team.c.admin.id(), write(&s)).await;
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE environments IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let request = start(&s, token.secret, "Creation scope insert wait");
        wait(&s, pid, "%INSERT INTO environments%", &request).await;
        if expired {
            s.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if expired {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert_eq!(count(&s).await, 0);
        } else {
            receipt(&reply);
            assert_eq!(count(&s).await, 1);
        }
    }
}

#[tokio::test]
async fn standalone_creation_publication_keeps_the_real_effect_after_current_read_or_key_loss() {
    for change in ["unchanged", "read_closed", "expired"] {
        let s = Scene::new().await;
        let token = key(&s.world, s.team.c.admin.id(), full_scope(&s)).await;
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let request = start(&s, token.secret, "Creation scope final publication");
        wait(&s, pid, "%INSERT INTO api_token_activity%", &request).await;
        let saved: (Id, String) = sqlx::query_as(
            "SELECT id,xmin::text FROM environments WHERE name='Creation scope final publication'",
        )
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
        match change {
            "read_closed" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(token.id)
                    .bind(json!({"rules":write(&s)}))
                    .execute(&s.world.db.pool)
                    .await
                    .unwrap();
            }
            "expired" => s.world.advance(chrono::Duration::hours(2)),
            _ => (),
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if change == "unchanged" {
            assert_eq!(full(&reply).base.id, saved.0);
        } else {
            assert_eq!(receipt(&reply).environment_id, saved.0);
        }
        let current: String = sqlx::query_scalar("SELECT xmin::text FROM environments WHERE id=$1")
            .bind(saved.0)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        assert_eq!(current, saved.1);
        assert_eq!(count(&s).await, 1, "publication never repeats the creator");
    }
}
