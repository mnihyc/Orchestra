//! Stored label effects preserve native custody and separate optional metadata from management.

use super::*;
use orch_types::Json;

fn scope(environment: Id, actions: &[&str]) -> Json {
    keys::rule(
        "SnapshotUpdate",
        "environment",
        environment,
        "self",
        actions,
    )
}

async fn rename(site: &Site, id: Id, token: &str, label: &str) -> RawReply {
    let call = site
        .app()
        .call(SnapshotUpdate, &[&id])
        .token(token)
        .body(&SnapshotUpdateBody {
            label: label.to_owned(),
        });
    call.send_bytes().await
}

async fn label(site: &Site, id: Id) -> String {
    storage::snapshot(&mut *site.world.conn().await, id)
        .await
        .unwrap()
        .unwrap()
        .label
}

fn body(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn wait_sql(
    site: &Site,
    pid: i32,
    pattern: &str,
    request: &tokio::task::JoinHandle<RawReply>,
) {
    tokio::time::timeout(RENDEZVOUS, async {
        loop {
            assert!(!request.is_finished(), "the native boundary must actually wait");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
                .bind(pid).bind(pattern).fetch_one(&site.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("observe the real Snapshot or activity wait");
}

#[tokio::test]
async fn label_change_needs_its_actual_governor_manage_and_returns_only_its_completed_receipt_without_read()
 {
    let (site, id) = fixture().await;
    let before = label(&site, id).await;
    for rule in [
        scope(site.environment, &["read"]),
        scope(id, &["manage"]),
        keys::rule(
            "SnapshotDelete",
            "environment",
            site.environment,
            "self",
            &["manage"],
        ),
        keys::rule("SnapshotUpdate", "thread", site.thread, "self", &["manage"]),
    ] {
        let key = keys::scoped(&site, site.ada.id(), vec![rule]).await;
        assert_eq!(
            rename(&site, id, &key.secret, "Forbidden label")
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(label(&site, id).await, before);
    }
    let denied = keys::scoped(
        &site,
        site.vic.id(),
        vec![scope(site.environment, &["manage"])],
    )
    .await;
    assert_eq!(
        rename(&site, id, &denied.secret, "No native permission")
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(label(&site, id).await, before);
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            scope(site.environment, &["manage"]),
            keys::read("SnapshotGet", site.environment),
        ],
    )
    .await;
    assert_eq!(
        body(rename(&site, id, &key.secret, "  Own label  ").await),
        json!({"projection":"receipt","snapshot_id":id})
    );
    assert_eq!(label(&site, id).await, "Own label");
}

#[tokio::test]
async fn same_operation_read_keeps_the_complete_ordinary_metadata_and_label_validation() {
    let (site, id) = fixture().await;
    let ordinary = body(rename(&site, id, &site.ada.token, "Reviewed").await);
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![scope(site.environment, &["manage", "read"])],
    )
    .await;
    assert_eq!(
        body(rename(&site, id, &key.secret, "Reviewed").await),
        ordinary
    );
    assert_eq!(
        rename(&site, id, &key.secret, "  ").await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(label(&site, id).await, "Reviewed");
    assert_eq!(site.sent(CommandKind::FsRecordRead), 0);
    assert_eq!(site.sent(CommandKind::FsSnapshot), 0);
}

#[tokio::test]
async fn copied_metadata_reads_both_the_current_governor_and_its_historical_origin() {
    let (site, id) = fixture().await;
    let current = elsewhere(&site, &site.ada, site.org).await;
    move_thread(&site, current).await;
    let old = keys::scoped(
        &site,
        site.ada.id(),
        vec![scope(site.environment, &["manage", "read"])],
    )
    .await;
    assert_eq!(
        rename(&site, id, &old.secret, "Wrong governor")
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let current_only = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            scope(current, &["manage", "read"]),
            keys::read("SnapshotGet", site.environment),
        ],
    )
    .await;
    assert_eq!(
        body(rename(&site, id, &current_only.secret, "No origin content").await),
        json!({"projection":"receipt","snapshot_id":id})
    );
    let both = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            scope(current, &["manage", "read"]),
            scope(site.environment, &["read"]),
        ],
    )
    .await;
    let shown = body(rename(&site, id, &both.secret, "Both sources").await);
    assert_eq!(shown["label"], "Both sources");
    assert_eq!(shown["environment"]["id"], json!(site.environment));
    assert_eq!(shown["thread_id"], json!(site.thread));
}

#[tokio::test]
async fn recorded_and_private_snapshots_keep_their_native_origin_and_maker_custody() {
    let (site, id, _) = recorded().await;
    let other = elsewhere(&site, &site.ada, site.org).await;
    move_thread(&site, other).await;
    let wrong = keys::scoped(
        &site,
        site.ada.id(),
        vec![scope(other, &["manage", "read"])],
    )
    .await;
    assert_eq!(
        rename(&site, id, &wrong.secret, "Other workspace")
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![scope(site.environment, &["manage"])],
    )
    .await;
    assert_eq!(
        body(rename(&site, id, &key.secret, "Recorded origin").await),
        json!({"projection":"receipt","snapshot_id":id})
    );
    let private = copied(&site, None, vec![(FILE.into(), CONTENT.into())]).await;
    let other_maker = keys::scoped(
        &site,
        site.dev.id(),
        vec![scope(site.environment, &["manage", "read"])],
    )
    .await;
    assert_eq!(
        rename(&site, private, &other_maker.secret, "Not my export")
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        body(rename(&site, private, &key.secret, "Own export").await),
        json!({"projection":"receipt","snapshot_id":private})
    );
}

#[tokio::test]
async fn label_authority_expires_after_the_actual_snapshot_lock_before_the_first_update() {
    for expired in [false, true] {
        let (site, id) = fixture().await;
        let original = label(&site, id).await;
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![scope(site.environment, &["manage"])],
        )
        .await;
        let mut blocker = site.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM snapshots WHERE id=$1 FOR UPDATE")
            .bind(id)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = site.app().clone();
        let request = tokio::spawn(async move {
            let call =
                app.call(SnapshotUpdate, &[&id])
                    .token(&key.secret)
                    .body(&SnapshotUpdateBody {
                        label: "After the lock".into(),
                    });
            call.send_bytes().await
        });
        wait_sql(
            &site,
            pid,
            "%SELECT id FROM snapshots%FOR UPDATE%",
            &request,
        )
        .await;
        assert_eq!(label(&site, id).await, original);
        if expired {
            site.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(RENDEZVOUS, request)
            .await
            .unwrap()
            .unwrap();
        if expired {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert_eq!(label(&site, id).await, original);
        } else {
            assert_eq!(
                body(reply),
                json!({"projection":"receipt","snapshot_id":id})
            );
            assert_eq!(label(&site, id).await, "After the lock");
        }
    }
}

#[tokio::test]
async fn label_change_does_not_adopt_replaced_snapshot_custody_after_its_native_lock_wait() {
    let (site, id) = fixture().await;
    let original = label(&site, id).await;
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![scope(site.environment, &["manage"])],
    )
    .await;
    let mut blocker = site.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM snapshots WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = site.app().clone();
    let request = tokio::spawn(async move {
        let call = app
            .call(SnapshotUpdate, &[&id])
            .token(&key.secret)
            .body(&SnapshotUpdateBody {
                label: "A different association".into(),
            });
        call.send_bytes().await
    });
    wait_sql(
        &site,
        pid,
        "%SELECT id FROM snapshots%FOR UPDATE%",
        &request,
    )
    .await;
    sqlx::query("UPDATE snapshots SET thread_id=NULL WHERE id=$1")
        .bind(id)
        .execute(&mut *blocker)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(RENDEZVOUS, request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(label(&site, id).await, original);
}

#[tokio::test]
async fn completed_label_publication_keeps_its_receipt_without_replaying_after_activity_wait() {
    for change in ["manage", "read", "expiry", "association"] {
        let (site, id) = fixture().await;
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![scope(site.environment, &["manage", "read"])],
        )
        .await;
        let mut blocker = site.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = site.app().clone();
        let token = key.secret.clone();
        let request = tokio::spawn(async move {
            let call = app
                .call(SnapshotUpdate, &[&id])
                .token(&token)
                .body(&SnapshotUpdateBody {
                    label: "Committed".into(),
                });
            call.send_bytes().await
        });
        wait_sql(&site, pid, "%INSERT INTO api_token_activity%", &request).await;
        assert_eq!(label(&site, id).await, "Committed");
        if change == "expiry" {
            site.world.advance(chrono::Duration::hours(2));
        } else if change == "association" {
            let other = elsewhere(&site, &site.ada, site.org).await;
            move_thread(&site, other).await;
        } else {
            let actions = if change == "manage" {
                vec!["read"]
            } else {
                vec!["manage"]
            };
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(key.id)
                .bind(json!({"rules":[scope(site.environment,&actions)]}))
                .execute(&site.world.db.pool)
                .await
                .unwrap();
        }
        let before: String = sqlx::query_scalar("SELECT xmin::text FROM snapshots WHERE id=$1")
            .bind(id)
            .fetch_one(&site.world.db.pool)
            .await
            .unwrap();
        blocker.commit().await.unwrap();
        let shown = body(
            tokio::time::timeout(RENDEZVOUS, request)
                .await
                .unwrap()
                .unwrap(),
        );
        if change == "manage" {
            assert_eq!(shown["label"], "Committed");
            assert_eq!(shown["id"], json!(id));
        } else {
            assert_eq!(shown, json!({"projection":"receipt","snapshot_id":id}));
        }
        let after: String = sqlx::query_scalar("SELECT xmin::text FROM snapshots WHERE id=$1")
            .bind(id)
            .fetch_one(&site.world.db.pool)
            .await
            .unwrap();
        assert_eq!(before, after, "publication never repeats the label writer");
    }
}
