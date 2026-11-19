//! Relabeling keeps native custody, safe receipts and its post-lock current key boundary.

use super::*;
use crate::snapshot_scope_helpers as keys;
use orch_api_core::testing::RawReply;
use orch_types::Json;
use std::time::Duration as Wait;

// The existing action-guard fixture budget is a watchdog, never an authority lifetime.
const LOCK_WAIT: Wait = Wait::from_secs(30);

pub(super) async fn activity_wait(
    site: &Site,
    pid: i32,
    request: &tokio::task::JoinHandle<RawReply>,
) {
    tokio::time::timeout(LOCK_WAIT, async {
        loop {
            assert!(
                !request.is_finished(),
                "the response did not reach its activity append"
            );
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                 AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid))
                 AND query LIKE '%INSERT INTO api_token_activity%')",
            )
            .bind(pid)
            .fetch_one(&site.world.db.pool)
            .await
            .unwrap();
            if blocked {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the committed Snapshot response must reach its actual activity append");
}

async fn stored(site: &Site) -> Id {
    super::snapshot_scopes::copied(site, Some(site.thread), Vec::new()).await
}

async fn rename(site: &Site, id: Id, token: &str, label: &str) -> RawReply {
    site.app()
        .call(SnapshotUpdate, &[&id])
        .token(token)
        .body(&SnapshotUpdateBody {
            label: label.into(),
        })
        .send_bytes()
        .await
}

fn accepted(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn label(site: &Site, id: Id) -> String {
    storage::snapshot(&mut *site.world.conn().await, id)
        .await
        .unwrap()
        .unwrap()
        .label
}

#[tokio::test]
async fn completed_snapshot_label_publication_keeps_its_receipt_after_real_activity_wait() {
    for change in ["unchanged", "expired", "source_removed"] {
        let site = Site::new().await;
        let id = stored(&site).await;
        let writer = keys::scoped(
            &site,
            site.ada.id(),
            vec![keys::rule(
                "SnapshotUpdate",
                "environment",
                site.environment,
                "self",
                &["manage", "read"],
            )],
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
        let app = site.world.app.clone();
        let request = tokio::spawn(async move {
            let request =
                app.call(SnapshotUpdate, &[&id])
                    .token(&writer.secret)
                    .body(&SnapshotUpdateBody {
                        label: "Committed held Snapshot label".into(),
                    });
            request.send_bytes().await
        });
        activity_wait(&site, pid, &request).await;
        let before: (String, String) =
            sqlx::query_as("SELECT label,xmin::text FROM snapshots WHERE id=$1")
                .bind(id)
                .fetch_one(&site.world.db.pool)
                .await
                .unwrap();
        assert_eq!(
            before.0, "Committed held Snapshot label",
            "the effect must have committed before the wait"
        );
        if change == "expired" {
            site.world.advance(chrono::Duration::hours(2));
        }
        if change == "source_removed" {
            assert!(
                storage::delete_snapshot(&mut *site.world.conn().await, id)
                    .await
                    .unwrap()
                    .is_some()
            );
        }
        blocker.commit().await.unwrap();
        let reply = accepted(
            tokio::time::timeout(LOCK_WAIT, request)
                .await
                .unwrap()
                .unwrap(),
        );
        if change == "unchanged" {
            assert_eq!(reply["id"], json!(id));
            assert_eq!(reply["label"], "Committed held Snapshot label");
        } else {
            assert_eq!(reply, json!({"projection":"receipt","snapshot_id":id}));
        }
        let after: Option<(String, String)> =
            sqlx::query_as("SELECT label,xmin::text FROM snapshots WHERE id=$1")
                .bind(id)
                .fetch_optional(&site.world.db.pool)
                .await
                .unwrap();
        if change == "source_removed" {
            assert!(after.is_none());
        } else {
            assert_eq!(
                after.as_ref(),
                Some(&before),
                "final publication cannot repeat the label writer"
            );
        }
    }
}

#[tokio::test]
async fn completed_snapshot_label_readback_does_not_reapply_manage_after_its_effect() {
    use crate::support::FullPermissionPageReply;
    use orch_api_contract::access::PermissionPageUpdate;
    let site = Site::new().await;
    let id = stored(&site).await;
    let writer = keys::scoped(
        &site,
        site.dev.id(),
        vec![keys::rule(
            "SnapshotUpdate",
            "environment",
            site.environment,
            "self",
            &["manage", "read"],
        )],
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
    let app = site.world.app.clone();
    let request = tokio::spawn(async move {
        let request =
            app.call(SnapshotUpdate, &[&id])
                .token(&writer.secret)
                .body(&SnapshotUpdateBody {
                    label: "Committed while management was held".into(),
                });
        request.send_bytes().await
    });
    activity_wait(&site, pid, &request).await;
    let before: (String, String) =
        sqlx::query_as("SELECT label,xmin::text FROM snapshots WHERE id=$1")
            .bind(id)
            .fetch_one(&site.world.db.pool)
            .await
            .unwrap();
    assert_eq!(before.0, "Committed while management was held");
    site.app()
        .call(PermissionPageUpdate, &[&"environments", &site.environment])
        .token(&site.ada.token)
        .raw_body(&json!({"whole":"private","rows":[{
            "permission":"files.view","add":[{"holder":{"kind":"user","id":site.dev.id()}}]
        }]}))
        .send()
        .await
        .full_permission_page()
        .ok();
    let permitted = site
        .app()
        .call(SnapshotGet, &[&id])
        .token(&site.dev.token)
        .send()
        .await
        .ok();
    assert_eq!(
        permitted.label, before.0,
        "ordinary FilesView remains after the real permission change"
    );
    let denied = rename(&site, id, &site.dev.token, "Must remain refused").await;
    assert_eq!(
        denied.status,
        StatusCode::FORBIDDEN,
        "ordinary FilesManage must actually be gone"
    );
    blocker.commit().await.unwrap();
    let reply = accepted(
        tokio::time::timeout(LOCK_WAIT, request)
            .await
            .unwrap()
            .unwrap(),
    );
    assert_eq!(reply["id"], json!(id));
    assert_eq!(
        reply["label"], before.0,
        "completed content uses current same-operation Read and native FilesView"
    );
    let after: (String, String) =
        sqlx::query_as("SELECT label,xmin::text FROM snapshots WHERE id=$1")
            .bind(id)
            .fetch_one(&site.world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        after, before,
        "the completed writer is never retried at readback/publication"
    );
}

#[tokio::test]
async fn snapshot_manage_keeps_relabeling_and_only_own_operation_read_returns_metadata() {
    let site = Site::new().await;
    let id = stored(&site).await;
    let manage = keys::rule(
        "SnapshotUpdate",
        "environment",
        site.environment,
        "self",
        &["manage"],
    );
    let writer = keys::scoped(&site, site.ada.id(), vec![manage.clone()]).await;
    assert_eq!(
        accepted(rename(&site, id, &writer.secret, "Writer label").await),
        json!({"projection":"receipt","snapshot_id":id})
    );
    assert_eq!(label(&site, id).await, "Writer label");
    let unrelated_read = keys::scoped(
        &site,
        site.ada.id(),
        vec![manage.clone(), keys::read("SnapshotGet", site.environment)],
    )
    .await;
    assert_eq!(
        accepted(rename(&site, id, &unrelated_read.secret, "Other endpoint read").await),
        json!({"projection":"receipt","snapshot_id":id})
    );
    let full = keys::scoped(
        &site,
        site.ada.id(),
        vec![manage, keys::read("SnapshotUpdate", site.environment)],
    )
    .await;
    let reply = accepted(rename(&site, id, &full.secret, "Full label").await);
    assert_eq!(reply["id"], json!(id));
    assert_eq!(reply["label"], "Full label");
    assert_eq!(reply["environment"]["id"], json!(site.environment));
    assert!(reply.get("projection").is_none());
    let ordinary = accepted(rename(&site, id, &site.ada.token, "Browser label").await);
    assert_eq!(ordinary["label"], "Browser label");
}

#[tokio::test]
async fn snapshot_relabel_binds_the_native_environment_not_the_path_id_or_account() {
    let site = Site::new().await;
    let id = stored(&site).await;
    let before = label(&site, id).await;
    for (user, rules) in [
        (
            site.ada.id(),
            vec![keys::rule(
                "SnapshotUpdate",
                "environment",
                id,
                "self",
                &["manage"],
            )],
        ),
        (
            site.ada.id(),
            vec![keys::rule(
                "SnapshotUpdate",
                "document",
                site.environment,
                "self",
                &["manage"],
            )],
        ),
        (
            site.ada.id(),
            vec![keys::read("SnapshotGet", site.environment)],
        ),
        (
            site.eve.id(),
            vec![keys::rule(
                "SnapshotUpdate",
                "environment",
                site.environment,
                "self",
                &["manage"],
            )],
        ),
    ] {
        let key = keys::scoped(&site, user, rules).await;
        let reply = rename(&site, id, &key.secret, "Refused label").await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert!(!String::from_utf8_lossy(&reply.raw).contains(&before));
        assert_eq!(label(&site, id).await, before);
    }
}

#[tokio::test]
async fn snapshot_legacy_relabel_keeps_its_current_native_full_response() {
    let site = Site::new().await;
    let id = stored(&site).await;
    let writer = keys::legacy_writer(&site, site.ada.id(), None).await;
    let reply = accepted(rename(&site, id, &writer.secret, "Legacy label").await);
    assert_eq!(reply["id"], json!(id));
    assert_eq!(reply["label"], "Legacy label");
    assert!(reply.get("projection").is_none());
}

#[tokio::test]
async fn snapshot_ordinary_refusal_stays_before_native_target_contention() {
    let site = Site::new().await;
    let id = stored(&site).await;
    let mut blocker = site.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
        .bind(site.thread)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let refused = tokio::time::timeout(
        LOCK_WAIT,
        rename(&site, id, &site.eve.token, "Must not wait"),
    )
    .await
    .expect("ordinary denial must precede native target contention");
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    blocker.rollback().await.unwrap();
}

#[tokio::test]
async fn snapshot_full_relabel_reads_its_origin_separately_from_its_current_files_governor() {
    let site = Site::new().await;
    let id = stored(&site).await;
    let destination =
        super::snapshot_scopes::elsewhere(&site, &site.eve, site.eve.person.personal_org).await;
    super::snapshot_scopes::move_thread(&site, destination).await;
    let manage = keys::rule(
        "SnapshotUpdate",
        "environment",
        destination,
        "self",
        &["manage"],
    );
    let without_origin = keys::scoped(
        &site,
        site.eve.id(),
        vec![manage.clone(), keys::read("SnapshotUpdate", destination)],
    )
    .await;
    assert_eq!(
        accepted(rename(&site, id, &without_origin.secret, "Kept current custody").await),
        json!({"projection":"receipt","snapshot_id":id})
    );
    let full = keys::scoped(
        &site,
        site.eve.id(),
        vec![
            manage,
            keys::read("SnapshotUpdate", destination),
            keys::read("SnapshotUpdate", site.environment),
        ],
    )
    .await;
    let reply = accepted(rename(&site, id, &full.secret, "Origin admitted").await);
    assert_eq!(reply["label"], "Origin admitted");
    assert_eq!(reply["environment"]["id"], json!(site.environment));
}

#[tokio::test]
async fn snapshot_relabel_rechecks_the_real_key_after_its_actual_thread_row_wait() {
    for expire in [false, true] {
        let site = Site::new().await;
        let id = stored(&site).await;
        let before = label(&site, id).await;
        let writer = keys::scoped(
            &site,
            site.ada.id(),
            vec![keys::rule(
                "SnapshotUpdate",
                "environment",
                site.environment,
                "self",
                &["manage"],
            )],
        )
        .await;
        let mut blocker = site.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
            .bind(site.thread)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = site.app().clone();
        let secret = writer.secret.clone();
        let request = tokio::spawn(async move {
            let request =
                app.call(SnapshotUpdate, &[&id])
                    .token(&secret)
                    .body(&SnapshotUpdateBody {
                        label: "After target wait".into(),
                    });
            request.send_bytes().await
        });
        tokio::time::timeout(LOCK_WAIT, async {
            loop {
                assert!(
                    !request.is_finished(),
                    "the request did not reach its native Thread row wait"
                );
                let waited: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                     AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid))
                     AND query LIKE '%FROM threads%FOR SHARE%')",
                )
                .bind(pid)
                .fetch_one(&site.world.db.pool)
                .await
                .unwrap();
                if waited {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the actual Snapshot relabel did not wait on its native Thread footprint");
        if expire {
            site.world.advance(Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(LOCK_WAIT, request)
            .await
            .unwrap()
            .unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert_eq!(label(&site, id).await, before);
        } else {
            assert_eq!(
                accepted(reply),
                json!({"projection":"receipt","snapshot_id":id})
            );
            assert_eq!(label(&site, id).await, "After target wait");
        }
    }
}
