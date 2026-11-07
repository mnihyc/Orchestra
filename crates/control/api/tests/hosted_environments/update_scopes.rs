//! The native configuration writer, actual changed home, receipts and current resource readback.

use super::*;
use orch_api_contract::environment::{EnvironmentUpdate, EnvironmentUpdateBody};

const OPERATION: &str = "EnvironmentUpdate";

fn root(s: &Scene, actions: &[&str]) -> Json {
    rule(
        OPERATION,
        "environment",
        s.first.base.id,
        "self",
        &["environment"],
        actions,
    )
}

fn body(revision: i64) -> EnvironmentUpdateBody {
    EnvironmentUpdateBody {
        expected_revision: revision,
        name: None,
        config: None,
        data_sources: None,
        home_node_id: None,
        remote_write: None,
        terminal: None,
        browser: None,
        vnc: None,
    }
}

async fn current(s: &Scene) -> Environment {
    s.world
        .app
        .call(EnvironmentGet, &[&s.first.base.id])
        .token(&s.team.c.admin.token)
        .send()
        .await
        .ok()
}

async fn call(s: &Scene, token: &str, body: &EnvironmentUpdateBody) -> RawReply {
    let call = s.world.app.call(EnvironmentUpdate, &[&s.first.base.id]);
    call.token(token).body(body).send_bytes().await
}

fn receipt(reply: &RawReply, id: Id, revision: i64, changed: bool) {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(
        value,
        json!({"projection":"receipt", "environment_id":id,"revision":revision,"changed":changed})
    );
}

async fn audits(s: &Scene) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_kind='environment' AND object_id=$1 AND action=$2")
        .bind(s.first.base.id).bind(orch_access_contract::audit::AuditAction::EnvironmentUpdated.as_str())
        .fetch_one(&s.world.db.pool).await.unwrap()
}

#[tokio::test]
async fn configuration_manage_keeps_native_mutation_and_honest_unchanged_receipts_without_read_or_home_write()
 {
    let s = Scene::new().await;
    let before = current(&s).await;
    let writer = key(
        &s.world,
        s.team.c.admin.id(),
        vec![
            root(&s, &["manage"]),
            rule(
                "EnvironmentGet.read",
                "environment",
                s.first.base.id,
                "self",
                &["environment"],
                &["read"],
            ),
        ],
    )
    .await;
    let mut change = body(before.revision);
    change.name = Some("write-only native configuration".into());
    receipt(
        &call(&s, &writer.secret, &change).await,
        s.first.base.id,
        before.revision + 1,
        true,
    );
    let changed = current(&s).await;
    assert_eq!(changed.base.name, "write-only native configuration");
    assert_eq!(changed.revision, before.revision + 1);
    let count = audits(&s).await;
    assert_eq!(count, 1);
    for home in [None, Some(changed.home.id)] {
        let mut unchanged = body(changed.revision);
        unchanged.home_node_id = home;
        receipt(
            &call(&s, &writer.secret, &unchanged).await,
            s.first.base.id,
            changed.revision,
            false,
        );
        assert_eq!(
            audits(&s).await,
            count,
            "native no-op has no invented audit or revision"
        );
    }
    for rules in [
        vec![rule(
            "EnvironmentGet.read",
            "environment",
            s.first.base.id,
            "self",
            &["environment"],
            &["read"],
        )],
        vec![root(&s, &["write"])],
        vec![rule(
            OPERATION,
            "topic",
            s.first.base.id,
            "self",
            &["environment"],
            &["manage"],
        )],
    ] {
        let denied = key(&s.world, s.team.c.admin.id(), rules).await;
        assert_eq!(
            call(&s, &denied.secret, &body(changed.revision))
                .await
                .status,
            StatusCode::FORBIDDEN
        );
    }
    let denied = key(
        &s.world,
        s.team.c.stranger.id(),
        vec![root(&s, &["manage"])],
    )
    .await;
    assert_eq!(
        call(&s, &denied.secret, &body(changed.revision))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(current(&s).await.revision, changed.revision);
}

#[tokio::test]
async fn configuration_home_write_uses_the_actual_new_node_namespace_and_preserves_native_interact()
{
    let s = Scene::new().await;
    let project = make_project(
        &s.world,
        &s.team.c.boss,
        s.team.c.acme,
        "New configuration home",
    )
    .await;
    let destination = make_topic(&s.world, &s.team.c.boss, project.base.id, "Chosen home")
        .await
        .base
        .id;
    let before = current(&s).await;
    let mut change = body(before.revision);
    change.home_node_id = Some(destination);
    for destination_rule in [
        None,
        Some(rule(
            OPERATION,
            "topic",
            s.team.topic,
            "self",
            &["environment"],
            &["write"],
        )),
    ] {
        let mut rules = vec![root(&s, &["manage"])];
        rules.extend(destination_rule);
        let denied = key(&s.world, s.team.c.admin.id(), rules).await;
        assert_eq!(
            call(&s, &denied.secret, &change).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(current(&s).await.home.id, before.home.id);
    }
    resources::name_person(
        &s.world,
        "environment",
        s.first.base.id,
        "environment.manage",
        s.team.c.viewer.id(),
    )
    .await;
    let rules = vec![
        root(&s, &["manage"]),
        rule(
            OPERATION,
            "topic",
            destination,
            "self",
            &["environment"],
            &["write"],
        ),
    ];
    let denied = key(&s.world, s.team.c.viewer.id(), rules.clone()).await;
    assert_eq!(
        call(&s, &denied.secret, &change).await.status,
        StatusCode::FORBIDDEN,
        "the ordinary new-home node.interact permission remains independent of Environment management"
    );
    let writer = key(&s.world, s.team.c.admin.id(), rules).await;
    receipt(
        &call(&s, &writer.secret, &change).await,
        s.first.base.id,
        before.revision + 1,
        true,
    );
    assert_eq!(current(&s).await.home.id, destination);
    let no_write = key(&s.world, s.team.c.admin.id(), vec![root(&s, &["manage"])]).await;
    let mut unchanged = body(before.revision + 1);
    unchanged.home_node_id = Some(destination);
    receipt(
        &call(&s, &no_write.secret, &unchanged).await,
        s.first.base.id,
        before.revision + 1,
        false,
    );
}

#[tokio::test]
async fn configuration_full_success_and_stale_current_content_use_only_the_same_operation_read() {
    let s = Scene::new().await;
    let before = current(&s).await;
    let writer = key(&s.world, s.team.c.admin.id(), vec![root(&s, &["manage"])]).await;
    let mut change = body(before.revision);
    change.name = Some("current-private-configuration-marker".into());
    receipt(
        &call(&s, &writer.secret, &change).await,
        s.first.base.id,
        before.revision + 1,
        true,
    );
    let stale = body(before.revision);
    let denied = call(&s, &writer.secret, &stale).await;
    assert_eq!(denied.status, StatusCode::CONFLICT);
    let value: Json = serde_json::from_slice(&denied.raw).unwrap();
    assert!(value["current"].is_null());
    assert!(!String::from_utf8_lossy(&denied.raw).contains("current-private-configuration-marker"));
    let reader = key(
        &s.world,
        s.team.c.admin.id(),
        vec![root(&s, &["manage", "read"]), s.sources(OPERATION)],
    )
    .await;
    let reply = call(&s, &reader.secret, &stale).await;
    assert_eq!(
        reply.status,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Json = serde_json::from_slice(&reply.raw).unwrap();
    let native = current(&s).await;
    assert_eq!(value["current"], json!(native));
    let reply = call(&s, &reader.secret, &body(native.revision)).await;
    assert_eq!(reply.status, StatusCode::OK);
    let shown: Environment = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(
        shown, native,
        "the same unchanged native individual formatter supplies full readback"
    );
    assert_eq!(
        audits(&s).await,
        1,
        "stale and unchanged requests do not replay the writer"
    );
}

#[tokio::test]
async fn configuration_refreshes_the_actual_key_after_its_native_environment_lock_before_mutation()
{
    for expire in [false, true] {
        let s = Scene::new().await;
        let before = current(&s).await;
        let writer = key(&s.world, s.team.c.admin.id(), vec![root(&s, &["manage"])]).await;
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
        let mut change = body(before.revision);
        change.name = Some("after-native-configuration-wait".into());
        let request = tokio::spawn(async move {
            let call = app.call(EnvironmentUpdate, &[&id]);
            call.token(&writer.secret).body(&change).send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the writer must reach its actual native lock");
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM environments WHERE id = $1 FOR UPDATE%')")
                    .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
                if waiting { break; } tokio::task::yield_now().await;
            }
        }).await.expect("the exact configuration preparation Environment FOR UPDATE wait");
        assert_eq!(audits(&s).await, 0);
        if expire {
            s.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            let after = current(&s).await;
            assert_eq!(after.base.name, before.base.name);
            assert_eq!(after.revision, before.revision);
            assert_eq!(audits(&s).await, 0);
        } else {
            receipt(&reply, s.first.base.id, before.revision + 1, true);
            assert_eq!(audits(&s).await, 1);
        }
    }
}

#[tokio::test]
async fn configuration_publication_keeps_the_committed_mutation_and_does_not_readmit_completed_manage()
 {
    for mode in [
        "unchanged",
        "read_only_key",
        "native_manage_lost",
        "expired",
    ] {
        let s = Scene::new().await;
        let entry = resources::name_person(
            &s.world,
            "environment",
            s.first.base.id,
            "environment.manage",
            s.team.c.viewer.id(),
        )
        .await;
        let before = current(&s).await;
        let writer = key(
            &s.world,
            s.team.c.viewer.id(),
            vec![root(&s, &["manage", "read"]), s.sources(OPERATION)],
        )
        .await;
        let control = call(&s, &writer.secret, &body(before.revision)).await;
        assert_eq!(control.status, StatusCode::OK);
        let control: Json = serde_json::from_slice(&control.raw).unwrap();
        assert_eq!(
            control["base"]["id"],
            json!(s.first.base.id),
            "the actual full manager control must succeed first"
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
        let id = s.first.base.id;
        let token = writer.secret.clone();
        let mut change = body(before.revision);
        change.name = Some("committed-configuration-publication-marker".into());
        let request = tokio::spawn(async move {
            let call = app.call(EnvironmentUpdate, &[&id]);
            call.token(&token).body(&change).send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the committed configuration body must reach final publication");
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
                if waiting { break; } tokio::task::yield_now().await;
            }
        }).await.expect("the actual postcommit configuration activity INSERT wait");
        let native = current(&s).await;
        assert_eq!(
            native.base.name,
            "committed-configuration-publication-marker"
        );
        assert_eq!(native.revision, before.revision + 1);
        assert_eq!(audits(&s).await, 1);
        let xmin: String = sqlx::query_scalar("SELECT xmin::text FROM environments WHERE id=$1")
            .bind(id)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        match mode {
            "read_only_key" => {
                let scope: KeyScope = serde_json::from_value(
                    json!({"rules":[root(&s, &["read"]), s.sources(OPERATION)]}),
                )
                .unwrap();
                assert!(scope.validate().is_empty());
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(writer.id)
                    .bind(json!(scope))
                    .execute(&s.world.db.pool)
                    .await
                    .unwrap();
            }
            "native_manage_lost" => {
                orch_db::queries::access::remove_entry(
                    &mut *s.world.conn().await,
                    entry,
                    s.team.c.admin.id(),
                    s.world.now(),
                )
                .await
                .unwrap();
            }
            "expired" => s.world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if matches!(mode, "native_manage_lost" | "expired") {
            receipt(&reply, id, native.revision, true);
        } else {
            assert_eq!(
                reply.status,
                StatusCode::OK,
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            let full: Environment = serde_json::from_slice(&reply.raw).unwrap();
            assert_eq!(full.base.name, native.base.name);
            assert_eq!(full.revision, native.revision);
        }
        let after: String = sqlx::query_scalar("SELECT xmin::text FROM environments WHERE id=$1")
            .bind(id)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        assert_eq!(
            after, xmin,
            "final source checking never replays the committed configuration update"
        );
        assert_eq!(audits(&s).await, 1);
    }
}
