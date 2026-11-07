//! Native creation/export retain their actual source through queued work and completed receipts.
use super::*;
use crate::snapshot_scope_helpers as keys;
use orch_api_core::testing::{Machine, RawReply};
use orch_db::queries::jobs;
use orch_link_contract::commands::{Cmd, CommandError};
use orch_types::Json;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
fn scope(site: &Site, export: bool, actions: &[&str]) -> Json {
    keys::rule(
        if export {
            "EnvironmentFilesExport"
        } else {
            "ThreadSnapshotCreate"
        },
        "environment",
        site.environment,
        "self",
        actions,
    )
}
async fn fixture() -> Site {
    let mut site = Site::new().await;
    site.disk
        .put("web/original-source/kept", b"accepted content");
    site.power_on();
    site
}
async fn start(site: &Site, token: &str, export: bool) -> RawReply {
    if export {
        let call = site
            .app()
            .call(EnvironmentFilesExport, &[&site.environment])
            .token(token);
        call.send_bytes().await
    } else {
        let call = site
            .app()
            .call(ThreadSnapshotCreate, &[&site.thread])
            .token(token)
            .body(&ThreadSnapshotCreateBody {
                label: Some("A supplied snapshot label".into()),
                selection: None,
            });
        call.send_bytes().await
    }
}
fn accepted(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn job_id(body: &Json) -> Id {
    body["id"]
        .as_str()
        .or_else(|| body["job_id"].as_str())
        .unwrap()
        .parse()
        .unwrap()
}
async fn snapshot_id(site: &Site, job: Id) -> Id {
    jobs::job(&mut *site.world.conn().await, job)
        .await
        .unwrap()
        .unwrap()
        .plan["snapshot_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}
async fn counts(site: &Site) -> (i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM snapshots),(SELECT count(*) FROM jobs WHERE kind='snapshot')",
    )
    .fetch_one(&site.world.db.pool)
    .await
    .unwrap()
}
async fn ended(site: &Site, id: Id) -> jobs::JobRow {
    eventually("the exact original snapshot job to end", || async {
        jobs::job(&mut *site.world.conn().await, id)
            .await
            .unwrap()
            .is_some_and(|row| !row.is_open())
    })
    .await;
    jobs::job(&mut *site.world.conn().await, id)
        .await
        .unwrap()
        .unwrap()
}
async fn wait_sql(
    site: &Site,
    pid: i32,
    pattern: &str,
    request: &tokio::task::JoinHandle<RawReply>,
) {
    tokio::time::timeout(WAIT,async{loop{assert!(!request.is_finished(),"the actual native wait must be reached");let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)").bind(pid).bind(pattern).fetch_one(&site.world.db.pool).await.unwrap();if blocked{break;}tokio::task::yield_now().await;}}).await.expect("observe the real source/audit/publication wait");
}
struct Held {
    disk: Arc<workspace::Disk>,
    kind: &'static str,
    entered: CancellationToken,
    release: CancellationToken,
    calls: AtomicUsize,
}
impl Held {
    fn new(site: &Site, kind: &'static str) -> Arc<Self> {
        Arc::new(Self {
            disk: site.disk.clone(),
            kind,
            entered: CancellationToken::new(),
            release: CancellationToken::new(),
            calls: AtomicUsize::new(0),
        })
    }
}
#[async_trait::async_trait]
impl Machine for Held {
    async fn run(&self, command: &Cmd) -> Result<Json, CommandError> {
        if command.kind == self.kind {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.cancel();
            self.release.cancelled().await;
        }
        self.disk.run(command).await
    }
}

#[tokio::test]
async fn snapshot_creation_and_export_bind_the_actual_inherent_read_and_manage_without_lending_native_authority()
 {
    let site = fixture().await;
    for export in [false, true] {
        let before = counts(&site).await;
        for rules in [
            vec![scope(&site, export, &["read"])],
            vec![scope(&site, export, &["manage"])],
            vec![keys::rule(
                if export {
                    "EnvironmentFilesExport"
                } else {
                    "ThreadSnapshotCreate"
                },
                "environment",
                site.thread,
                "self",
                &["read", "manage"],
            )],
            vec![keys::rule(
                "ThreadSnapshotPreflight",
                "environment",
                site.environment,
                "self",
                &["read", "manage"],
            )],
        ] {
            let key = keys::scoped(&site, site.ada.id(), rules).await;
            assert_eq!(
                start(&site, &key.secret, export).await.status,
                StatusCode::FORBIDDEN
            );
            assert_eq!(counts(&site).await, before);
        }
        let viewer = keys::scoped(
            &site,
            site.vic.id(),
            vec![scope(&site, export, &["read", "manage"])],
        )
        .await;
        assert_eq!(
            start(&site, &viewer.secret, export).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(counts(&site).await, before);
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![scope(&site, export, &["read", "manage"])],
        )
        .await;
        let shown = accepted(start(&site, &key.secret, export).await);
        let id = job_id(&shown);
        let raw = jobs::job(&mut *site.world.conn().await, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(shown["state"], "queued");
        assert!(shown["result"].is_null());
        assert_eq!(
            raw.plan["snapshot_origin"]["origin"]["operation"],
            if export {
                "EnvironmentFilesExport"
            } else {
                "ThreadSnapshotCreate"
            }
        );
        assert_eq!(raw.plan["snapshot_origin"]["job"], json!(id));
        assert_eq!(
            raw.plan["snapshot_origin"]["snapshot"],
            raw.plan["snapshot_id"]
        );
        assert_eq!(raw.node_id, (!export).then_some(site.thread));
        assert_eq!(raw.environment_id, Some(site.environment));
        assert_eq!(raw.created_by, Some(site.ada.id()));
        assert_eq!(counts(&site).await, (before.0 + 1, before.1 + 1));
    }
}

#[tokio::test]
async fn snapshot_files_management_remains_distinct_from_whole_environment_export_management() {
    let site = fixture().await;
    let key = keys::scoped(
        &site,
        site.dev.id(),
        vec![
            scope(&site, false, &["read", "manage"]),
            scope(&site, true, &["read", "manage"]),
        ],
    )
    .await;
    accepted(start(&site, &key.secret, false).await);
    let before = counts(&site).await;
    assert_eq!(
        start(&site, &key.secret, true).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(counts(&site).await, before);
}

#[tokio::test]
async fn whole_workspace_export_preserves_offline_queuing_no_hidden_thread_and_native_busy_refusal()
{
    let site = fixture().await;
    site.power_off().await;
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![scope(&site, true, &["read", "manage"])],
    )
    .await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM threads")
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap();
    let shown = accepted(start(&site, &key.secret, true).await);
    let id = job_id(&shown);
    let snapshot = snapshot_id(&site, id).await;
    assert_eq!(
        storage::snapshot(&mut *site.world.conn().await, snapshot)
            .await
            .unwrap()
            .unwrap()
            .thread_id,
        None
    );
    assert_eq!(site.sent(CommandKind::FsMeasure), 0);
    assert_eq!(site.sent(CommandKind::FsSnapshot), 0);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM threads")
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    let mut blocker = site.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
        .bind(site.environment)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let prior = counts(&site).await;
    assert_eq!(
        start(&site, &key.secret, true).await.status,
        StatusCode::CONFLICT
    );
    assert_eq!(counts(&site).await, prior);
    blocker.rollback().await.unwrap();
}

#[tokio::test]
async fn snapshot_creation_rechecks_after_its_real_post_measurement_thread_lock_before_any_insert()
{
    for changed in ["live", "expiry", "directory"] {
        let mut site = fixture().await;
        let held = Held::new(&site, "fs.measure");
        site.power_on_with(held.clone());
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![scope(&site, false, &["read", "manage"])],
        )
        .await;
        let app = site.app().clone();
        let thread = site.thread;
        let request = tokio::spawn(async move {
            let call = app
                .call(ThreadSnapshotCreate, &[&thread])
                .token(&key.secret)
                .body(&ThreadSnapshotCreateBody::default());
            call.send_bytes().await
        });
        tokio::time::timeout(WAIT, held.entered.cancelled())
            .await
            .unwrap();
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
        held.release.cancel();
        wait_sql(&site, pid, "%FROM threads%FOR UPDATE%", &request).await;
        assert_eq!(counts(&site).await, (0, 0));
        if changed == "expiry" {
            site.world.advance(Duration::hours(2));
        }
        if changed == "directory" {
            sqlx::query("UPDATE threads SET workspace_dir='changed' WHERE node_id=$1")
                .bind(site.thread)
                .execute(&mut *blocker)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if changed == "live" {
            accepted(reply);
            assert_eq!(counts(&site).await, (1, 1));
        } else {
            assert!(!reply.status.is_success());
            assert_eq!(counts(&site).await, (0, 0));
        }
        assert_eq!(held.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn snapshot_creator_rolls_back_sql_after_its_actual_audit_wait_when_the_origin_ends() {
    for export in [false, true] {
        let site = fixture().await;
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![scope(&site, export, &["read", "manage"])],
        )
        .await;
        let mut blocker = site.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE audit_log IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = site.app().clone();
        let target = if export {
            site.environment
        } else {
            site.thread
        };
        let request = tokio::spawn(async move {
            if export {
                let call = app
                    .call(EnvironmentFilesExport, &[&target])
                    .token(&key.secret);
                call.send_bytes().await
            } else {
                let call = app
                    .call(ThreadSnapshotCreate, &[&target])
                    .token(&key.secret)
                    .body(&ThreadSnapshotCreateBody::default());
                call.send_bytes().await
            }
        });
        wait_sql(&site, pid, "%INSERT INTO audit_log%", &request).await;
        assert_eq!(counts(&site).await, (0, 0));
        site.world.advance(Duration::hours(2));
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        assert!(!reply.status.is_success());
        assert_eq!(counts(&site).await, (0, 0));
        assert_eq!(site.sent(CommandKind::FsSnapshot), 0);
    }
}

#[tokio::test]
async fn queued_snapshot_work_keeps_its_original_operation_and_current_key_before_new_copy_io() {
    for export in [false, true] {
        for change in ["live", "read", "manage", "revoked", "expiry"] {
            let site = fixture().await;
            let key = keys::scoped(
                &site,
                site.ada.id(),
                vec![scope(&site, export, &["read", "manage"])],
            )
            .await;
            let shown = accepted(start(&site, &key.secret, export).await);
            let id = job_id(&shown);
            let snapshot = snapshot_id(&site, id).await;
            match change {
                "read" | "manage" => {
                    let actions = if change == "read" {
                        vec!["manage"]
                    } else {
                        vec!["read"]
                    };
                    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                        .bind(key.id)
                        .bind(json!({"rules":[scope(&site,export,&actions)]}))
                        .execute(&site.world.db.pool)
                        .await
                        .unwrap();
                }
                "revoked" => keys::revoke(&site, &key).await,
                "expiry" => site.world.advance(Duration::hours(2)),
                _ => {}
            }
            site.app().app.engine.start();
            let job = ended(&site, id).await;
            let row = storage::snapshot(&mut *site.world.conn().await, snapshot)
                .await
                .unwrap()
                .unwrap();
            if change == "live" {
                assert_eq!(job.state, "succeeded");
                assert_eq!(row.state, "complete");
                assert_eq!(site.sent(CommandKind::FsSnapshot), 1);
            } else {
                assert_eq!(job.state, "failed");
                assert_eq!(row.state, "failed");
                assert_eq!(site.sent(CommandKind::FsSnapshot), 0);
            }
            site.app().app.engine.stop();
        }
    }
}

#[tokio::test]
async fn accepted_snapshot_copy_finishes_its_original_custody_once_after_key_expiry() {
    let mut site = fixture().await;
    let held = Held::new(&site, "fs.snapshot");
    site.power_on_with(held.clone());
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![scope(&site, false, &["read", "manage"])],
    )
    .await;
    let shown = accepted(start(&site, &key.secret, false).await);
    let id = job_id(&shown);
    let snapshot = snapshot_id(&site, id).await;
    site.app().app.engine.start();
    tokio::time::timeout(WAIT, held.entered.cancelled())
        .await
        .unwrap();
    site.world.advance(Duration::hours(2));
    held.release.cancel();
    let job = ended(&site, id).await;
    assert_eq!(job.state, "succeeded");
    let row = storage::snapshot(&mut *site.world.conn().await, snapshot)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "complete");
    assert_eq!(row.file_count, 1);
    assert_eq!(held.calls.load(Ordering::SeqCst), 1);
    let pins: i64 =
        sqlx::query_scalar("SELECT count(*) FROM snapshot_blob_refs WHERE snapshot_id=$1")
            .bind(snapshot)
            .fetch_one(&site.world.db.pool)
            .await
            .unwrap();
    assert_eq!(pins, 2);
    assert_eq!(counts(&site).await, (1, 1));
    site.app().app.engine.stop();
}

#[tokio::test]
async fn a_changed_runtime_snapshot_pointer_never_redirects_the_original_job_or_its_undo() {
    let site = fixture().await;
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![scope(&site, false, &["read", "manage"])],
    )
    .await;
    let first = job_id(&accepted(start(&site, &key.secret, false).await));
    let first_snapshot = snapshot_id(&site, first).await;
    let second = job_id(&accepted(start(&site, &key.secret, false).await));
    let second_snapshot = snapshot_id(&site, second).await;
    sqlx::query(
        "UPDATE jobs SET plan=jsonb_set(plan,'{snapshot_id}',to_jsonb($2::uuid)) WHERE id=$1",
    )
    .bind(first)
    .bind(second_snapshot)
    .execute(&site.world.db.pool)
    .await
    .unwrap();
    site.app().app.engine.start();
    assert_eq!(ended(&site, first).await.state, "failed");
    assert_eq!(ended(&site, second).await.state, "succeeded");
    assert_eq!(
        storage::snapshot(&mut *site.world.conn().await, first_snapshot)
            .await
            .unwrap()
            .unwrap()
            .state,
        "failed"
    );
    assert_eq!(
        storage::snapshot(&mut *site.world.conn().await, second_snapshot)
            .await
            .unwrap()
            .unwrap()
            .state,
        "complete"
    );
    assert_eq!(site.sent(CommandKind::FsSnapshot), 1);
    site.app().app.engine.stop();
}

#[tokio::test]
async fn completed_snapshot_start_keeps_its_safe_ack_without_replaying_creation_after_activity_wait()
 {
    for export in [false, true] {
        for change in ["manage", "read", "expiry"] {
            let site = fixture().await;
            let key = keys::scoped(
                &site,
                site.ada.id(),
                vec![scope(&site, export, &["read", "manage"])],
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
            let target = if export {
                site.environment
            } else {
                site.thread
            };
            let token = key.secret.clone();
            let request = tokio::spawn(async move {
                if export {
                    let call = app.call(EnvironmentFilesExport, &[&target]).token(&token);
                    call.send_bytes().await
                } else {
                    let call = app
                        .call(ThreadSnapshotCreate, &[&target])
                        .token(&token)
                        .body(&ThreadSnapshotCreateBody::default());
                    call.send_bytes().await
                }
            });
            wait_sql(&site, pid, "%INSERT INTO api_token_activity%", &request).await;
            assert_eq!(counts(&site).await, (1, 1));
            let id: Id = sqlx::query_scalar("SELECT id FROM jobs WHERE kind='snapshot'")
                .fetch_one(&site.world.db.pool)
                .await
                .unwrap();
            let snapshot = snapshot_id(&site, id).await;
            if change == "expiry" {
                site.world.advance(Duration::hours(2));
            } else {
                let actions = if change == "manage" {
                    vec!["read"]
                } else {
                    vec!["manage"]
                };
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(key.id)
                    .bind(json!({"rules":[scope(&site,export,&actions)]}))
                    .execute(&site.world.db.pool)
                    .await
                    .unwrap();
            }
            blocker.commit().await.unwrap();
            let body = accepted(tokio::time::timeout(WAIT, request).await.unwrap().unwrap());
            if change == "manage" {
                assert_eq!(body["id"], json!(id));
                assert_eq!(body["state"], "queued");
            } else {
                assert_eq!(
                    body,
                    json!({"projection":"receipt","job_id":id,"snapshot_id":snapshot})
                );
            }
            assert_eq!(counts(&site).await, (1, 1));
            assert_eq!(
                site.sent(CommandKind::FsMeasure),
                if export { 0 } else { 1 }
            );
            assert_eq!(site.sent(CommandKind::FsSnapshot), 0);
        }
    }
}
