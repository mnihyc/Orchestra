//! Native copy scope, current physical source, real queue waits and completed result receipts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]
#[path = "workspace_snapshots/scope_helpers.rs"]
mod keys;
mod places;
mod support;
mod workspace;

use http::StatusCode;
use orch_api_contract::{
    environment::ThroughThreadQuery,
    workspace::{EnvironmentFileCopy, FileFromToBody},
};
use orch_api_core::testing::{Machine, RawReply};
use orch_link_contract::commands::{Cmd, CommandError, CommandKind};
use orch_types::{Hash, Id, Json};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::sync::CancellationToken;
use workspace::Site;

const CONTENT: &[u8] = b"actual original copy bytes";
const FROM: &str = "web/source";
const TO: &str = "web/destination";
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
fn rule(site: &Site, actions: &[&str]) -> Json {
    keys::rule(
        "EnvironmentFileCopy",
        "environment",
        site.environment,
        "self",
        actions,
    )
}
async fn fixture() -> Site {
    let mut site = Site::new().await;
    site.disk.put(FROM, CONTENT);
    site.power_on();
    site
}
async fn copy(site: &Site, token: &str, thread: Option<Id>, from: &str, to: &str) -> RawReply {
    let call = site
        .app()
        .call(EnvironmentFileCopy, &[&site.environment])
        .token(token)
        .query(&ThroughThreadQuery { thread })
        .body(&FileFromToBody {
            from: from.into(),
            to: to.into(),
        });
    call.send_bytes().await
}
fn shown(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn receipt(reply: RawReply, site: &Site) {
    assert_eq!(
        shown(reply),
        json!({"projection":"receipt","environment_id":site.environment,"path":TO})
    );
}
async fn audits(site: &Site) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE environment_id=$1 AND action=$2")
        .bind(site.environment)
        .bind(orch_access_contract::audit::AuditAction::WorkspaceFileCopied.as_str())
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap()
}
async fn wait_sql(
    site: &Site,
    pid: i32,
    pattern: &str,
    request: &tokio::task::JoinHandle<RawReply>,
) {
    tokio::time::timeout(WAIT,async{loop{assert!(!request.is_finished(),"the literal native wait must be reached");let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)").bind(pid).bind(pattern).fetch_one(&site.world.db.pool).await.unwrap();if blocked{break;}tokio::task::yield_now().await;}}).await.expect("observe the actual queue/audit/activity wait");
}
struct Held {
    disk: Arc<workspace::Disk>,
    entered: CancellationToken,
    release: CancellationToken,
    calls: AtomicUsize,
}
struct Refused {
    disk: Arc<workspace::Disk>,
    entered: CancellationToken,
    release: CancellationToken,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl Machine for Refused {
    async fn run(&self, command: &Cmd) -> Result<Json, CommandError> {
        if command.kind == "fs.copy" {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.cancel();
            self.release.cancelled().await;
            return Err(CommandError::new(
                orch_link_contract::commands::ErrorCode::Conflict,
                "private native conflict detail",
            ));
        }
        self.disk.run(command).await
    }
}
impl Held {
    fn new(site: &Site) -> Arc<Self> {
        Arc::new(Self {
            disk: site.disk.clone(),
            entered: CancellationToken::new(),
            release: CancellationToken::new(),
            calls: AtomicUsize::new(0),
        })
    }
}
#[async_trait::async_trait]
impl Machine for Held {
    async fn run(&self, command: &Cmd) -> Result<Json, CommandError> {
        if command.kind == "fs.copy" {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.cancel();
            self.release.cancelled().await;
        }
        self.disk.run(command).await
    }
}

#[tokio::test]
async fn copy_needs_its_actual_environment_read_and_write_and_keeps_native_metadata() {
    let site = fixture().await;
    for rules in [
        vec![rule(&site, &["read"])],
        vec![rule(&site, &["write"])],
        vec![rule(&site, &["read", "manage"])],
        vec![
            keys::rule(
                "EnvironmentFileStat",
                "environment",
                site.environment,
                "self",
                &["read"],
            ),
            rule(&site, &["write"]),
        ],
        vec![keys::rule(
            "EnvironmentFileCopy",
            "thread",
            site.environment,
            "self",
            &["read", "write"],
        )],
    ] {
        let key = keys::scoped(&site, site.ada.id(), rules).await;
        assert_eq!(
            copy(&site, &key.secret, Some(site.thread), FROM, TO)
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        assert!(!site.disk.exists(TO));
        assert_eq!(site.sent(CommandKind::FsCopy), 0);
    }
    let key = keys::scoped(&site, site.ada.id(), vec![rule(&site, &["read", "write"])]).await;
    let body = shown(copy(&site, &key.secret, Some(site.thread), FROM, TO).await);
    assert_eq!(body["entry"]["path"], TO);
    assert_eq!(body["entry"]["size"], CONTENT.len());
    assert_eq!(body["hash"], json!(Hash::of(CONTENT)));
    assert_eq!(site.disk.read(TO).unwrap(), CONTENT);
    assert_eq!(site.disk.read(FROM).unwrap(), CONTENT);
    assert_eq!(site.sent(CommandKind::FsCopy), 1);
}

#[tokio::test]
async fn copy_keeps_native_files_management_and_both_remote_write_ceilings() {
    for condition in ["viewer", "environment", "machine"] {
        let site = fixture().await;
        if condition == "environment" {
            sqlx::query("UPDATE environments SET remote_write=false WHERE id=$1")
                .bind(site.environment)
                .execute(&site.world.db.pool)
                .await
                .unwrap();
        }
        if condition == "machine" {
            sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy}',COALESCE(facts->'policy','{}'::jsonb)||'{\"environment.remote_write\":false}'::jsonb) WHERE id=$1").bind(site.machine).execute(&site.world.db.pool).await.unwrap();
        }
        let actor = if condition == "viewer" {
            &site.vic
        } else {
            &site.ada
        };
        let key = keys::scoped(&site, actor.id(), vec![rule(&site, &["read", "write"])]).await;
        let native = copy(&site, &actor.token, Some(site.thread), FROM, TO).await;
        let bounded = copy(&site, &key.secret, Some(site.thread), FROM, TO).await;
        assert_eq!(bounded.status, native.status);
        assert!(!bounded.status.is_success());
        assert!(!site.disk.exists(TO));
        assert_eq!(site.sent(CommandKind::FsCopy), 0);
    }
}

#[tokio::test]
async fn copy_rechecks_current_source_scope_policy_and_expiry_at_the_actual_queue_wait() {
    for change in ["live", "read", "write", "expiry", "generation", "policy"] {
        let site = fixture().await;
        let key = keys::scoped(&site, site.ada.id(), vec![rule(&site, &["read", "write"])]).await;
        let mut blocker = site.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
            .bind(site.environment)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = site.app().clone();
        let environment = site.environment;
        let thread = site.thread;
        let token = key.secret.clone();
        let request = tokio::spawn(async move {
            let call = app
                .call(EnvironmentFileCopy, &[&environment])
                .token(&token)
                .query(&ThroughThreadQuery {
                    thread: Some(thread),
                })
                .body(&FileFromToBody {
                    from: FROM.into(),
                    to: TO.into(),
                });
            call.send_bytes().await
        });
        wait_sql(&site, pid, "%FROM environments%FOR SHARE%", &request).await;
        assert!(!site.disk.exists(TO));
        assert_eq!(site.sent(CommandKind::FsCopy), 0);
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='fs.copy'")
            .fetch_one(&site.world.db.pool)
            .await
            .unwrap();
        assert_eq!(before, 0);
        match change {
            "read" | "write" => {
                let actions = if change == "read" {
                    vec!["write"]
                } else {
                    vec!["read"]
                };
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(key.id)
                    .bind(json!({"rules":[rule(&site,&actions)]}))
                    .execute(&site.world.db.pool)
                    .await
                    .unwrap();
            }
            "expiry" => site.world.advance(chrono::Duration::hours(2)),
            "generation" => {
                sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                    .bind(site.environment)
                    .execute(&mut *blocker)
                    .await
                    .unwrap();
            }
            "policy" => {
                sqlx::query("UPDATE environments SET remote_write=false WHERE id=$1")
                    .bind(site.environment)
                    .execute(&mut *blocker)
                    .await
                    .unwrap();
            }
            _ => {}
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if change == "live" {
            shown(reply);
            assert_eq!(site.disk.read(TO).unwrap(), CONTENT);
        } else {
            assert!(!reply.status.is_success());
            assert!(!site.disk.exists(TO));
            assert_eq!(site.sent(CommandKind::FsCopy), 0);
            let after: i64 =
                sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='fs.copy'")
                    .fetch_one(&site.world.db.pool)
                    .await
                    .unwrap();
            assert_eq!(after, 0);
        }
    }
}

#[tokio::test]
async fn accepted_copy_reconciles_once_and_returns_a_receipt_after_key_loss() {
    for change in ["live", "read", "write", "revoked", "expiry"] {
        let mut site = fixture().await;
        let held = Held::new(&site);
        site.power_on_with(held.clone());
        let key = keys::scoped(&site, site.ada.id(), vec![rule(&site, &["read", "write"])]).await;
        let app = site.app().clone();
        let environment = site.environment;
        let token = key.secret.clone();
        let request = tokio::spawn(async move {
            let call = app
                .call(EnvironmentFileCopy, &[&environment])
                .token(&token)
                .query(&ThroughThreadQuery { thread: None })
                .body(&FileFromToBody {
                    from: FROM.into(),
                    to: TO.into(),
                });
            call.send_bytes().await
        });
        tokio::time::timeout(WAIT, held.entered.cancelled())
            .await
            .unwrap();
        assert!(!site.disk.exists(TO));
        match change {
            "read" | "write" => {
                let actions = if change == "read" {
                    vec!["write"]
                } else {
                    vec!["read"]
                };
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(key.id)
                    .bind(json!({"rules":[rule(&site,&actions)]}))
                    .execute(&site.world.db.pool)
                    .await
                    .unwrap();
            }
            "revoked" => keys::revoke(&site, &key).await,
            "expiry" => site.world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        held.release.cancel();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if matches!(change, "live" | "write") {
            assert_eq!(shown(reply)["hash"], json!(Hash::of(CONTENT)));
        } else {
            receipt(reply, &site);
        }
        assert_eq!(site.disk.read(TO).unwrap(), CONTENT);
        assert_eq!(held.calls.load(Ordering::SeqCst), 1);
        assert_eq!(audits(&site).await, 1);
    }
}

#[tokio::test]
async fn completed_copy_audit_and_activity_waits_never_replay_the_filesystem_write() {
    for table in ["audit_log", "api_token_activity"] {
        let site = fixture().await;
        let key = keys::scoped(&site, site.ada.id(), vec![rule(&site, &["read", "write"])]).await;
        let mut blocker = site.world.db.pool.begin().await.unwrap();
        let sql = if table == "audit_log" {
            "LOCK TABLE audit_log IN SHARE MODE"
        } else {
            "LOCK TABLE api_token_activity IN SHARE MODE"
        };
        sqlx::query(sql).execute(&mut *blocker).await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = site.app().clone();
        let environment = site.environment;
        let request = tokio::spawn(async move {
            let call = app
                .call(EnvironmentFileCopy, &[&environment])
                .token(&key.secret)
                .query(&ThroughThreadQuery { thread: None })
                .body(&FileFromToBody {
                    from: FROM.into(),
                    to: TO.into(),
                });
            call.send_bytes().await
        });
        let pattern = if table == "audit_log" {
            "%INSERT INTO audit_log%"
        } else {
            "%INSERT INTO api_token_activity%"
        };
        wait_sql(&site, pid, pattern, &request).await;
        assert_eq!(site.disk.read(TO).unwrap(), CONTENT);
        assert_eq!(site.sent(CommandKind::FsCopy), 1);
        site.world.advance(chrono::Duration::hours(2));
        blocker.commit().await.unwrap();
        receipt(
            tokio::time::timeout(WAIT, request).await.unwrap().unwrap(),
            &site,
        );
        assert_eq!(site.sent(CommandKind::FsCopy), 1);
        assert_eq!(audits(&site).await, 1);
    }
}

#[tokio::test]
async fn copy_keeps_the_original_result_association_and_native_failure_semantics() {
    let mut site = fixture().await;
    let held = Held::new(&site);
    site.power_on_with(held.clone());
    let key = keys::scoped(&site, site.ada.id(), vec![rule(&site, &["read", "write"])]).await;
    let app = site.app().clone();
    let environment = site.environment;
    let token = key.secret.clone();
    let request = tokio::spawn(async move {
        let call = app
            .call(EnvironmentFileCopy, &[&environment])
            .token(&token)
            .query(&ThroughThreadQuery { thread: None })
            .body(&FileFromToBody {
                from: FROM.into(),
                to: TO.into(),
            });
        call.send_bytes().await
    });
    tokio::time::timeout(WAIT, held.entered.cancelled())
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
        .bind(site.environment)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    held.release.cancel();
    receipt(
        tokio::time::timeout(WAIT, request).await.unwrap().unwrap(),
        &site,
    );
    assert_eq!(site.disk.read(TO).unwrap(), CONTENT);
    assert_eq!(held.calls.load(Ordering::SeqCst), 1);
    let failure = copy(&site, &site.ada.token, None, FROM, TO).await;
    assert!(!failure.status.is_success());
    assert_eq!(site.disk.read(TO).unwrap(), CONTENT);
    assert_eq!(
        audits(&site).await,
        1,
        "a refused existing destination is not another completed copy"
    );
}

#[tokio::test]
async fn directory_copy_and_private_command_evidence_keep_the_native_wire_and_scope_namespace() {
    let site = fixture().await;
    site.disk.put("web/tree/a", b"a");
    site.disk.put("web/tree/nested/b", b"b");
    let key = keys::scoped(&site, site.ada.id(), vec![rule(&site, &["read", "write"])]).await;
    shown(
        copy(
            &site,
            &key.secret,
            Some(site.thread),
            "web/tree",
            "web/tree-copy",
        )
        .await,
    );
    assert_eq!(site.disk.read("web/tree-copy/a").unwrap(), b"a");
    assert_eq!(site.disk.read("web/tree-copy/nested/b").unwrap(), b"b");
    let seen = site.played.as_ref().unwrap().seen_of(CommandKind::FsCopy);
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].body,
        json!({"environment_id":site.environment,"from":"web/tree","to":"web/tree-copy"})
    );
    let metadata =
        orch_db::queries::link::command_authority(&mut *site.world.conn().await, seen[0].id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(metadata["kind"], "file_copy");
    let decoded: orch_engine::commands::CommandAuthority =
        serde_json::from_value(metadata.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), metadata);
}

#[tokio::test]
async fn copy_refusals_keep_their_native_status_and_current_same_operation_read_at_rpc_and_publication()
 {
    for stage in ["rpc", "activity"] {
        for change in ["read", "none", "other"] {
            let mut site = fixture().await;
            let gate = Arc::new(Refused {
                disk: site.disk.clone(),
                entered: CancellationToken::new(),
                release: CancellationToken::new(),
                calls: AtomicUsize::new(0),
            });
            site.power_on_with(gate.clone());
            let key =
                keys::scoped(&site, site.ada.id(), vec![rule(&site, &["read", "write"])]).await;
            let mut blocker = if stage == "activity" {
                let mut tx = site.world.db.pool.begin().await.unwrap();
                sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                Some(tx)
            } else {
                None
            };
            let pid = if let Some(tx) = blocker.as_mut() {
                Some(
                    sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()")
                        .fetch_one(&mut **tx)
                        .await
                        .unwrap(),
                )
            } else {
                None
            };
            let app = site.app().clone();
            let environment = site.environment;
            let token = key.secret.clone();
            let request = tokio::spawn(async move {
                let call = app
                    .call(EnvironmentFileCopy, &[&environment])
                    .token(&token)
                    .query(&ThroughThreadQuery { thread: None })
                    .body(&FileFromToBody {
                        from: FROM.into(),
                        to: TO.into(),
                    });
                call.send_bytes().await
            });
            tokio::time::timeout(WAIT, gate.entered.cancelled())
                .await
                .unwrap();
            if let Some(pid) = pid {
                gate.release.cancel();
                wait_sql(&site, pid, "%INSERT INTO api_token_activity%", &request).await;
            }
            let rules = match change {
                "read" => vec![rule(&site, &["read"])],
                "other" => vec![
                    rule(&site, &["write"]),
                    keys::rule(
                        "EnvironmentFileStat",
                        "environment",
                        site.environment,
                        "self",
                        &["read"],
                    ),
                ],
                _ => vec![rule(&site, &["write"])],
            };
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(key.id)
                .bind(json!({"rules":rules}))
                .execute(&site.world.db.pool)
                .await
                .unwrap();
            gate.release.cancel();
            if let Some(tx) = blocker {
                tx.commit().await.unwrap();
            }
            let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
            assert_eq!(
                reply.status,
                StatusCode::CONFLICT,
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            let problem: Json = serde_json::from_slice(&reply.raw).unwrap();
            assert_eq!(problem["code"], "conflict.exists");
            assert_eq!(
                String::from_utf8_lossy(&reply.raw).contains("private native conflict detail"),
                change == "read"
            );
            assert!(!site.disk.exists(TO));
            assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
            assert_eq!(audits(&site).await, 0);
        }
    }
}
