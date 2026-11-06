//! Genuine Snapshot deletion keeps custody, exact scope, native pin semantics and IO boundaries.

use super::*;

fn manage(environment: Id) -> Json {
    keys::rule(
        "SnapshotDelete",
        "environment",
        environment,
        "self",
        &["manage"],
    )
}

async fn deletion(site: &Site, id: Id, token: &str) -> RawReply {
    site.app()
        .call(SnapshotDelete, &[&id])
        .token(token)
        .send_bytes()
        .await
}

fn empty(reply: RawReply) {
    assert_eq!(
        reply.status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert!(reply.raw.is_empty());
}

async fn pins(site: &Site, hash: Hash) -> i32 {
    sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(hash.as_bytes().as_slice())
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn delete_uses_the_native_environment_and_manage_without_lending_ordinary_files_authority() {
    let (site, id) = fixture().await;
    for rule in [
        keys::read("SnapshotDelete", site.environment),
        keys::rule(
            "SnapshotUpdate",
            "environment",
            site.environment,
            "self",
            &["manage"],
        ),
        manage(id),
        keys::rule(
            "SnapshotDelete",
            "account",
            site.ada.id(),
            "subtree",
            &["manage"],
        ),
    ] {
        let key = keys::scoped(&site, site.ada.id(), vec![rule]).await;
        assert_eq!(
            deletion(&site, id, &key.secret).await.status,
            StatusCode::FORBIDDEN
        );
        assert!(
            storage::snapshot(&mut *site.world.conn().await, id)
                .await
                .unwrap()
                .is_some()
        );
    }
    let readonly = keys::scoped(&site, site.vic.id(), vec![manage(site.environment)]).await;
    assert_eq!(
        deletion(&site, id, &readonly.secret).await.status,
        StatusCode::FORBIDDEN
    );
    let allowed = keys::scoped(&site, site.ada.id(), vec![manage(site.environment)]).await;
    let busy = new_id();
    storage::begin_snapshot(
        &mut *site.world.conn().await,
        &NewSnapshot {
            id: busy,
            thread_id: Some(site.thread),
            environment_id: site.environment,
            env_generation: 1,
            label: "Still taking native state",
            selection: &json!({"include":["**"],"exclude":[]}),
            created_by: Some(site.ada.id()),
            at: site.world.now(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        deletion(&site, busy, &allowed.secret).await.status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        storage::snapshot(&mut *site.world.conn().await, busy)
            .await
            .unwrap()
            .unwrap()
            .state,
        "running"
    );
    empty(deletion(&site, id, &allowed.secret).await);
    assert!(
        storage::snapshot(&mut *site.world.conn().await, id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(pins(&site, Hash::of(CONTENT)).await, 0);
}

#[tokio::test]
async fn deletion_preserves_shared_native_pins_and_tracked_pins_when_the_manifest_store_is_missing()
{
    let (site, first) = fixture().await;
    let second = copied(
        &site,
        Some(site.thread),
        vec![(FILE.into(), CONTENT.into())],
    )
    .await;
    let manifest = storage::snapshot(&mut *site.world.conn().await, first)
        .await
        .unwrap()
        .unwrap()
        .manifest_blob
        .unwrap();
    assert_eq!(pins(&site, Hash::of(CONTENT)).await, 2);
    let key = keys::scoped(&site, site.ada.id(), vec![manage(site.environment)]).await;
    empty(deletion(&site, first, &key.secret).await);
    assert_eq!(pins(&site, Hash::of(CONTENT)).await, 1);
    assert_eq!(pins(&site, manifest).await, 1);
    assert!(
        storage::snapshot(&mut *site.world.conn().await, second)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        site.app()
            .app
            .blobs
            .store()
            .delete(&manifest)
            .await
            .unwrap()
    );
    empty(deletion(&site, second, &site.ada.token).await);
    assert_eq!(pins(&site, Hash::of(CONTENT)).await, 0);
    assert_eq!(pins(&site, manifest).await, 0);
}

#[tokio::test]
async fn deletion_checks_current_key_after_each_actual_thread_snapshot_pin_and_blob_wait() {
    for stage in ["thread", "snapshot", "pins", "blob"] {
        for expired in [false, true] {
            let (site, id) = fixture().await;
            let key = keys::scoped(&site, site.ada.id(), vec![manage(site.environment)]).await;
            let mut blocker = site.world.db.pool.begin().await.unwrap();
            let pattern = match stage {
                "thread" => {
                    sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
                        .bind(site.thread)
                        .execute(&mut *blocker)
                        .await
                        .unwrap();
                    "%FROM threads t%FOR UPDATE OF t%"
                }
                "snapshot" => {
                    sqlx::query("SELECT id FROM snapshots WHERE id=$1 FOR UPDATE")
                        .bind(id)
                        .execute(&mut *blocker)
                        .await
                        .unwrap();
                    "%SELECT id FROM snapshots WHERE id=$1 FOR UPDATE%"
                }
                "pins" => {
                    sqlx::query(
                        "SELECT blob FROM snapshot_blob_refs WHERE snapshot_id=$1 FOR UPDATE",
                    )
                    .bind(id)
                    .execute(&mut *blocker)
                    .await
                    .unwrap();
                    "%FROM snapshot_blob_refs WHERE snapshot_id=$1 ORDER BY blob FOR UPDATE%"
                }
                _ => {
                    sqlx::query("SELECT hash FROM blobs WHERE hash=$1 FOR UPDATE")
                        .bind(Hash::of(CONTENT).as_bytes().as_slice())
                        .execute(&mut *blocker)
                        .await
                        .unwrap();
                    "%SELECT hash FROM blobs WHERE hash = ANY%ORDER BY hash FOR UPDATE%"
                }
            };
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let app = site.app().clone();
            let request = tokio::spawn(async move {
                let call = app.call(SnapshotDelete, &[&id]).token(&key.secret);
                call.send_bytes().await
            });
            tokio::time::timeout(RENDEZVOUS, async {
                loop {
                    assert!(!request.is_finished(), "the native {stage} wait must be reached");
                    let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
                        .bind(pid).bind(pattern).fetch_one(&site.world.db.pool).await.unwrap();
                    if blocked { break; } tokio::task::yield_now().await;
                }
            }).await.expect("observe the literal native blocker and SQL");
            assert!(
                storage::snapshot(&mut *site.world.conn().await, id)
                    .await
                    .unwrap()
                    .is_some()
            );
            assert_eq!(pins(&site, Hash::of(CONTENT)).await, 1);
            if expired {
                site.world.advance(chrono::Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(RENDEZVOUS, request)
                .await
                .unwrap()
                .unwrap();
            if expired {
                assert_eq!(
                    reply.status,
                    StatusCode::UNAUTHORIZED,
                    "{stage}: {}",
                    String::from_utf8_lossy(&reply.raw)
                );
                assert!(
                    storage::snapshot(&mut *site.world.conn().await, id)
                        .await
                        .unwrap()
                        .is_some()
                );
                assert_eq!(pins(&site, Hash::of(CONTENT)).await, 1);
            } else {
                empty(reply);
                assert!(
                    storage::snapshot(&mut *site.world.conn().await, id)
                        .await
                        .unwrap()
                        .is_none()
                );
                assert_eq!(pins(&site, Hash::of(CONTENT)).await, 0);
            }
        }
    }
}

#[tokio::test]
async fn deletion_reads_the_manifest_once_and_refuses_expiry_after_the_actual_store_wait() {
    for expired in [false, true] {
        let (mut site, id) = fixture().await;
        let manifest = storage::snapshot(&mut *site.world.conn().await, id)
            .await
            .unwrap()
            .unwrap()
            .manifest_blob
            .unwrap();
        let key = keys::scoped(&site, site.ada.id(), vec![manage(site.environment)]).await;
        let gate = gate_blob_open(&mut site, manifest);
        let app = site.app().clone();
        let request = tokio::spawn(async move {
            let call = app.call(SnapshotDelete, &[&id]).token(&key.secret);
            call.send_bytes().await
        });
        tokio::time::timeout(RENDEZVOUS, gate.reached.notified())
            .await
            .unwrap();
        assert_eq!(gate.reads.load(Ordering::SeqCst), 1);
        assert!(
            storage::snapshot(&mut *site.world.conn().await, id)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(pins(&site, Hash::of(CONTENT)).await, 1);
        if expired {
            site.world.advance(chrono::Duration::hours(2));
        }
        gate.release.notify_one();
        let reply = tokio::time::timeout(RENDEZVOUS, request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(gate.reads.load(Ordering::SeqCst), 1);
        if expired {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert!(
                storage::snapshot(&mut *site.world.conn().await, id)
                    .await
                    .unwrap()
                    .is_some()
            );
            assert_eq!(pins(&site, Hash::of(CONTENT)).await, 1);
        } else {
            empty(reply);
        }
    }
}

#[tokio::test]
async fn deletion_never_uses_a_new_manifest_to_authorize_its_earlier_cleanup_operands() {
    let (mut site, id) = fixture().await;
    let original = storage::snapshot(&mut *site.world.conn().await, id)
        .await
        .unwrap()
        .unwrap()
        .manifest_blob
        .unwrap();
    let key = keys::scoped(&site, site.ada.id(), vec![manage(site.environment)]).await;
    let gate = gate_blob_open(&mut site, original);
    let app = site.app().clone();
    let request = tokio::spawn(async move {
        let call = app.call(SnapshotDelete, &[&id]).token(&key.secret);
        call.send_bytes().await
    });
    tokio::time::timeout(RENDEZVOUS, gate.reached.notified())
        .await
        .unwrap();
    let other = copied(
        &site,
        Some(site.thread),
        vec![(FILE.into(), b"new retained contents".to_vec())],
    )
    .await;
    let replacement = storage::snapshot(&mut *site.world.conn().await, other)
        .await
        .unwrap()
        .unwrap()
        .manifest_blob
        .unwrap();
    sqlx::query("UPDATE snapshots SET manifest_blob=$2 WHERE id=$1")
        .bind(id)
        .bind(replacement.as_bytes().as_slice())
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    gate.release.notify_one();
    let reply = tokio::time::timeout(RENDEZVOUS, request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(
        storage::snapshot(&mut *site.world.conn().await, id)
            .await
            .unwrap()
            .unwrap()
            .manifest_blob,
        Some(replacement)
    );
    assert_eq!(pins(&site, Hash::of(CONTENT)).await, 1);
    assert_eq!(pins(&site, original).await, 1);
    assert_eq!(gate.reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn deletion_keeps_recorded_historical_custody_and_private_export_maker_rules() {
    let (site, id, _) = recorded().await;
    let other = elsewhere(&site, &site.ada, site.org).await;
    move_thread(&site, other).await;
    let wrong = keys::scoped(&site, site.ada.id(), vec![manage(other)]).await;
    assert_eq!(
        deletion(&site, id, &wrong.secret).await.status,
        StatusCode::FORBIDDEN
    );
    let correct = keys::scoped(&site, site.ada.id(), vec![manage(site.environment)]).await;
    empty(deletion(&site, id, &correct.secret).await);
    let row = storage::snapshot(&mut *site.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.storage, "environment");
    assert_eq!(row.state, "expired");
    assert_eq!(row.environment_id, site.environment);
    assert!(row.manifest_blob.is_none());
    let private = copied(&site, None, vec![(FILE.into(), CONTENT.into())]).await;
    let other_reader = keys::scoped(&site, site.dev.id(), vec![manage(site.environment)]).await;
    assert_eq!(
        deletion(&site, private, &other_reader.secret).await.status,
        StatusCode::NOT_FOUND
    );
    empty(deletion(&site, private, &correct.secret).await);
}

#[tokio::test]
async fn completed_deletion_keeps_its_empty_ack_after_activity_wait_without_unpinning_twice() {
    let (site, id) = fixture().await;
    let key = keys::scoped(&site, site.ada.id(), vec![manage(site.environment)]).await;
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
    let request = tokio::spawn(async move {
        let call = app.call(SnapshotDelete, &[&id]).token(&key.secret);
        call.send_bytes().await
    });
    tokio::time::timeout(RENDEZVOUS, async {
        loop {
            assert!(!request.is_finished(), "the committed response must wait at activity append");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&site.world.db.pool).await.unwrap();
            if blocked { break; } tokio::task::yield_now().await;
        }
    }).await.expect("observe the real post-commit activity wait");
    assert!(
        storage::snapshot(&mut *site.world.conn().await, id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(pins(&site, Hash::of(CONTENT)).await, 0);
    let version: String = sqlx::query_scalar("SELECT xmin::text FROM blobs WHERE hash=$1")
        .bind(Hash::of(CONTENT).as_bytes().as_slice())
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap();
    site.world.advance(chrono::Duration::hours(2));
    blocker.commit().await.unwrap();
    empty(
        tokio::time::timeout(RENDEZVOUS, request)
            .await
            .unwrap()
            .unwrap(),
    );
    let after: String = sqlx::query_scalar("SELECT xmin::text FROM blobs WHERE hash=$1")
        .bind(Hash::of(CONTENT).as_bytes().as_slice())
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap();
    assert_eq!(version, after);
    assert_eq!(pins(&site, Hash::of(CONTENT)).await, 0);
}
