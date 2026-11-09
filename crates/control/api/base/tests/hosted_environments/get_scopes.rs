//! All native individual reading shapes and the once-only retained inspection observation.

use super::*;
use orch_api_contract::nodes::DeletedInspectionQuery;

const READ: &str = "EnvironmentGet.read";
const INSPECT: &str = "EnvironmentGet.inspect_deleted";

fn rules(s: &Scene, operation: &str, action: &str) -> Vec<Json> {
    vec![
        rule(
            operation,
            "environment",
            s.first.base.id,
            "self",
            &["environment"],
            &[action],
        ),
        s.sources(operation),
    ]
}

async fn call(s: &Scene, token: &str, inspect: bool) -> RawReply {
    s.world
        .app
        .call(EnvironmentGet, &[&s.first.base.id])
        .token(token)
        .query(&DeletedInspectionQuery {
            inspect_deleted: inspect.then_some(true),
        })
        .send_bytes()
        .await
}

async fn shown(s: &Scene, token: &str, inspect: bool) -> Environment {
    let reply = call(s, token, inspect).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn audits(s: &Scene, actor: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE actor_id=$1 AND object_kind='environment' AND object_id=$2 AND action='deleted.inspected'")
        .bind(actor).bind(s.first.base.id).fetch_one(&s.world.db.pool).await.unwrap()
}

async fn wait_for(
    s: &Scene,
    blocker: i32,
    text: &str,
    request: &tokio::task::JoinHandle<RawReply>,
) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!request.is_finished(), "the native reader must reach the observed SQL wait");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
                .bind(blocker).bind(text).fetch_one(&s.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the exact native SQL and blocking backend identify arrival");
}

#[tokio::test]
async fn environment_get_keeps_native_individual_body_and_exact_variant_action_and_source_scopes() {
    let s = Scene::new().await;
    let native = shown(&s, &s.team.c.admin.token, false).await;
    let reader = key(&s.world, s.team.c.admin.id(), rules(&s, READ, "read")).await;
    assert_eq!(shown(&s, &reader.secret, false).await, native);
    for policy in [
        rules(&s, "EnvironmentList", "read"),
        rules(&s, INSPECT, "manage"),
        rules(&s, READ, "manage"),
        vec![
            rule(
                READ,
                "host",
                s.first.base.id,
                "self",
                &["environment"],
                &["read"],
            ),
            s.sources(READ),
        ],
        vec![rule(
            READ,
            "environment",
            s.first.base.id,
            "self",
            &["environment"],
            &["read"],
        )],
    ] {
        let denied = key(&s.world, s.team.c.admin.id(), policy).await;
        assert_eq!(
            call(&s, &denied.secret, false).await.status,
            StatusCode::FORBIDDEN,
            "another variant, action, kind or missing actual foreign source cannot lend content"
        );
    }
    let denied = key(&s.world, s.team.c.stranger.id(), rules(&s, READ, "read")).await;
    assert_eq!(
        call(&s, &denied.secret, false).await.status,
        StatusCode::FORBIDDEN,
        "exact key scope supplies no ordinary Environment reading door"
    );
    assert_eq!(
        audits(&s, s.team.c.admin.id()).await,
        0,
        "ordinary live reads have no retained audit"
    );
}

#[tokio::test]
async fn environment_get_keeps_the_native_through_thread_cut_without_lending_whole_foreign_sources()
{
    use orch_db::queries::access::{PageKey, RowWrite, write_row};
    let s = Scene::new().await;
    let thread = new_id();
    let mut conn = s.world.conn().await;
    let page = PageKey::new("environment", s.first.base.id, "live");
    for permission in [
        "environment.view",
        "environment.manage",
        "files.view",
        "files.manage",
        "terminal.view",
        "terminal.manage",
        "browser.view",
        "browser.manage",
        "vnc.view",
        "vnc.manage",
        "ports.view",
        "ports.manage",
    ] {
        write_row(
            &mut conn,
            &RowWrite {
                page: &page,
                profile: "any",
                permission,
                inherit: false,
                locked: false,
            },
            Some(s.team.c.admin.id()),
            s.world.now(),
        )
        .await
        .unwrap();
    }
    orch_db::queries::hierarchy::testing::insert_node(
        &mut conn,
        thread,
        "thread",
        Some(s.team.topic),
        "Native visible witness",
        s.team.c.admin.id(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(s.first.base.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let entry = resources::name_person(
        &s.world,
        "node",
        thread,
        "thread.view",
        s.team.c.stranger.id(),
    )
    .await;
    let native = shown(&s, &s.team.c.stranger.token, false).await;
    assert_eq!(
        native.generation, 0,
        "ordinary native through-thread projection is established first"
    );
    assert!(native.host.is_none() && native.template.is_none() && native.mounts.is_empty());
    assert!(native.sandbox_readiness.is_none());
    assert_eq!(native.config, json!({}));
    let reader = key(
        &s.world,
        s.team.c.stranger.id(),
        vec![
            rule(
                READ,
                "environment",
                s.first.base.id,
                "self",
                &["environment"],
                &["read"],
            ),
            rule(
                READ,
                "org",
                s.team.c.acme,
                "subtree",
                &["org", "project", "topic", "thread", "group"],
                &["read"],
            ),
        ],
    )
    .await;
    assert_eq!(
        shown(&s, &reader.secret, false).await,
        native,
        "no Host/Template/Machine/Mount scope or whole Environment View is manufactured for the cut"
    );
    orch_db::queries::access::remove_entry(
        &mut *s.world.conn().await,
        entry,
        s.team.c.admin.id(),
        s.world.now(),
    )
    .await
    .unwrap();
    assert_eq!(
        call(&s, &reader.secret, false).await.status,
        StatusCode::FORBIDDEN,
        "the actual native alternative witness must remain authorized"
    );
}

#[tokio::test]
async fn retained_environment_get_keeps_exact_inspection_and_once_per_day_audit_after_the_native_lock()
 {
    for expire in [false, true] {
        let s = Scene::new().await;
        super::retained::keep(&s).await;
        let reader = key(&s.world, s.team.c.admin.id(), rules(&s, INSPECT, "manage")).await;
        assert_eq!(audits(&s, s.team.c.admin.id()).await, 0);
        let denied = key(&s.world, s.team.c.admin.id(), rules(&s, READ, "read")).await;
        assert_eq!(
            call(&s, &denied.secret, true).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            audits(&s, s.team.c.admin.id()).await,
            0,
            "refused preparation invokes no audit hook"
        );
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
        let id = s.first.base.id;
        let token = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(EnvironmentGet, &[&id]);
            call.token(&token)
                .query(&DeletedInspectionQuery {
                    inspect_deleted: Some(true),
                })
                .send_bytes()
                .await
        });
        wait_for(
            &s,
            pid,
            "%FROM environments WHERE id = $1 FOR UPDATE%",
            &request,
        )
        .await;
        assert_eq!(
            audits(&s, s.team.c.admin.id()).await,
            0,
            "the first audit effect has not happened"
        );
        if expire {
            s.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.status,
            if expire {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::OK
            }
        );
        if expire {
            assert_eq!(audits(&s, s.team.c.admin.id()).await, 0);
        } else {
            let selected: Environment = serde_json::from_slice(&reply.raw).unwrap();
            assert!(selected.inspection.as_ref().unwrap().readonly);
            assert_eq!(audits(&s, s.team.c.admin.id()).await, 1);
            let native = shown(&s, &s.team.c.admin.token, true).await;
            assert_eq!(
                selected, native,
                "the original retained native formatter supplies the body"
            );
            assert_eq!(shown(&s, &reader.secret, true).await, native);
            assert_eq!(
                audits(&s, s.team.c.admin.id()).await,
                1,
                "snapshot preparation, rendering, repeats and validation cannot replay the once-per-day observation"
            );
        }
    }
}

#[tokio::test]
async fn retained_environment_get_publication_keeps_original_native_sources_without_replaying_the_audit()
 {
    for change in [false, true] {
        let s = Scene::new().await;
        super::retained::keep(&s).await;
        let other = a_template(
            &s.world,
            &s.team.c.admin,
            s.team.c.acme,
            "Other retained input",
        )
        .await;
        let reader = key(&s.world, s.team.c.admin.id(), rules(&s, INSPECT, "manage")).await;
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
        let id = s.first.base.id;
        let token = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(EnvironmentGet, &[&id]);
            call.token(&token)
                .query(&DeletedInspectionQuery {
                    inspect_deleted: Some(true),
                })
                .send_bytes()
                .await
        });
        wait_for(&s, pid, "%INSERT INTO api_token_activity%", &request).await;
        assert_eq!(
            audits(&s, s.team.c.admin.id()).await,
            1,
            "the successful snapshot's auxiliary audit is committed"
        );
        if change {
            sqlx::query("UPDATE environments SET template_id=$2 WHERE id=$1")
                .bind(s.first.base.id)
                .bind(other)
                .execute(&s.world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.status,
            if change {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::OK
            }
        );
        assert_eq!(
            audits(&s, s.team.c.admin.id()).await,
            1,
            "final current-source validation neither rolls back the completed observation nor calls it again"
        );
    }
}

#[tokio::test]
async fn retained_environment_get_retries_a_changed_identity_snapshot_before_its_once_only_audit() {
    let s = Scene::new().await;
    super::retained::keep(&s).await;
    let reader = key(&s.world, s.team.c.admin.id(), rules(&s, INSPECT, "manage")).await;
    let mut blocker = s.world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE runtimes IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = s.world.app.clone();
    let id = s.first.base.id;
    let token = reader.secret.clone();
    let request = tokio::spawn(async move {
        let call = app.call(EnvironmentGet, &[&id]);
        call.token(&token)
            .query(&DeletedInspectionQuery {
                inspect_deleted: Some(true),
            })
            .send_bytes()
            .await
    });
    wait_for(&s, pid, "%runtimes%", &request).await;
    assert_eq!(
        audits(&s, s.team.c.admin.id()).await,
        0,
        "a blocked snapshot has not invoked auxiliary work"
    );
    // A genuine non-key update conflicts with re-locking the old identity tuple
    // in the repeatable snapshot; the fresh body establishes that retry occurred.
    sqlx::query("UPDATE users SET display_name='After the retained snapshot conflict' WHERE id=$1")
        .bind(s.team.c.admin.id())
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let view: Environment = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(
        view.base.owner.display_name,
        "After the retained snapshot conflict"
    );
    assert_eq!(
        audits(&s, s.team.c.admin.id()).await,
        1,
        "only the successful fresh snapshot invokes the native inspection observation"
    );
}
