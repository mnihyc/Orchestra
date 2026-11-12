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
    assert_eq!(body["path"], TO);
    assert_eq!(body["size"], CONTENT.len());
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
            let host = orch_types::new_id();
            let mut conn = site.world.conn().await;
            orch_db::queries::resources::insert_host(
                &mut conn,
                &orch_db::queries::resources::NewHost {
                    id: host,
                    machine_id: site.machine,
                    owner_id: Some(site.ada.id()),
                    home_id: Some(site.org),
                    name: "The actual workspace host",
                },
                site.world.now(),
            )
            .await
            .unwrap();
            sqlx::query("UPDATE environments SET config=config||jsonb_build_object('env.host_id',$2::text) WHERE id=$1")
                .bind(site.environment)
                .bind(host.to_string())
                .execute(&mut *conn)
                .await
                .unwrap();
            sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy}',COALESCE(facts->'policy','{}'::jsonb)||'{\"environment.remote_write\":false}'::jsonb) WHERE id=$1").bind(site.machine).execute(&site.world.db.pool).await.unwrap();
            let physical = orch_db::queries::environment_facts::of(&mut conn, site.environment)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(physical.machine_id, Some(site.machine));
            assert!(
                orch_db::queries::environment_facts::refuses(
                    &mut conn,
                    site.environment,
                    "environment.remote_write",
                )
                .await
                .unwrap()
            );
        }
        let actor = if condition == "viewer" {
            &site.vic
        } else {
            &site.ada
        };
        let key = keys::scoped(&site, actor.id(), vec![rule(&site, &["read", "write"])]).await;
        let native = copy(&site, &actor.token, Some(site.thread), FROM, TO).await;
        let bounded = copy(&site, &key.secret, Some(site.thread), FROM, TO).await;
        assert_eq!(
            bounded.status,
            native.status,
            "native case: {condition}; native: {}; bounded: {}",
            String::from_utf8_lossy(&native.raw),
            String::from_utf8_lossy(&bounded.raw)
        );
        assert!(!bounded.status.is_success());
        assert!(!site.disk.exists(TO));
        assert_eq!(site.sent(CommandKind::FsCopy), 0);
    }
}

const AFTER: &str = "web/destination-after";
async fn copy_commands(site: &Site) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='fs.copy'")
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap()
}
async fn scope_of(conn: &mut sqlx::PgConnection, key: Id) -> Json {
    sqlx::query_scalar("SELECT operation_scope FROM api_tokens WHERE id=$1")
        .bind(key)
        .fetch_one(conn)
        .await
        .unwrap()
}
/// What a refusal says, without the request it answers.
fn refusal(reply: &RawReply) -> orch_api_contract::common::Problem {
    let mut problem = reply.problem();
    problem.request_id.clear();
    problem
}
/// Takes a fresh look at the backends on the case's own connection. `pg_stat_activity` is read once per transaction and
/// the same picture is shown until the transaction ends, and the case's connection is inside one: a question asked
/// without clearing the picture first is answered from the moment of its first question, before the change of the key
/// had reached the database, and stays blind to it however long it is asked.
async fn fresh_look(connection: &mut sqlx::PgConnection) {
    sqlx::query("SELECT pg_stat_clear_snapshot()")
        .execute(connection)
        .await
        .unwrap();
}
/// Waits until the update of the key's rules waits, through any chain of waiting backends, for the case's own
/// transaction by way of at least one other: the request holding its key, blocked at its source. The question is asked
/// on the case's own connection, so that it takes nothing from the pool the request and the update work in, and it is
/// asked of a fresh look at the backends each time. It ends by that wait, not by a count of tries; the time it is given
/// bounds a failure and nothing else.
async fn waits_behind_the_request(
    blocker: &mut sqlx::PgConnection,
    pid: i32,
    update: &tokio::task::JoinHandle<Result<sqlx::postgres::PgQueryResult, sqlx::Error>>,
    pool: &sqlx::PgPool,
) {
    let waited = tokio::time::timeout(WAIT, async {
        loop {
            assert!(
                !update.is_finished(),
                "the change of the key landed while the request held the key"
            );
            fresh_look(&mut *blocker).await;
            let behind: bool = sqlx::query_scalar(
                "WITH RECURSIVE chain(pid, depth) AS (
                     SELECT n.pid, 0 FROM pg_stat_activity n
                      WHERE n.datname = current_database() AND n.wait_event_type = 'Lock'
                        AND n.query LIKE 'UPDATE api_tokens SET operation_scope%'
                   UNION
                     SELECT holder.pid, chain.depth + 1
                       FROM chain CROSS JOIN LATERAL unnest(pg_blocking_pids(chain.pid)) AS holder(pid)
                      WHERE chain.depth < 8)
                 SELECT EXISTS(SELECT 1 FROM chain WHERE pid = $1 AND depth >= 2)",
            )
            .bind(pid)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
            if behind {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    if waited.is_err() {
        // What every backend of the case's database was doing, and what is left of the pool, so that a
        // failure says whether the change never reached the database or was not recognised.
        fresh_look(&mut *blocker).await;
        let report: String = sqlx::query_scalar(
            "SELECT COALESCE(string_agg(format('pid %s state %s wait %s/%s xid %s blocked by %s: %s',
                        pid, state, wait_event_type, wait_event, backend_xid, pg_blocking_pids(pid),
                        left(regexp_replace(COALESCE(query, ''), '[[:space:]]+', ' ', 'g'), 150)),
                     E'\n' ORDER BY pid), '')
               FROM pg_stat_activity WHERE datname = current_database()",
        )
        .fetch_one(&mut *blocker)
        .await
        .unwrap_or_else(|error| format!("(the activity could not be read: {error})"));
        panic!(
            "the change of the key must wait for the request that holds it (the case's backend {pid}; \
             the change finished: {}; the pool holds {} connections, {} idle, of {})\n{report}",
            update.is_finished(),
            pool.size(),
            pool.num_idle(),
            pool.options().get_max_connections(),
        );
    }
}
/// A request holds the row of its key from its credential lock to the end of its transaction, so a change of the key's
/// rules made while it waits at its queue source cannot land: it waits for the request, which waits for the case. It is
/// ordered after the copy that was admitted under the rules it replaces, and the next copy sees it.
async fn key_change_is_ordered_after_the_admitted_copy(lost: &str) {
    let mut site = fixture().await;
    let held = Held::new(&site);
    site.power_on_with(held.clone());
    let key = keys::scoped(&site, site.ada.id(), vec![rule(&site, &["read", "write"])]).await;
    let kept: &[&str] = if lost == "read" {
        &["write"]
    } else {
        &["read"]
    };
    let narrowed = json!({"rules":[rule(&site, kept)]});
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
    assert_eq!(site.sent(CommandKind::FsCopy), 0);
    assert_eq!(copy_commands(&site).await, 0);
    let stored = scope_of(&mut blocker, key.id).await;
    let pool = site.world.db.pool.clone();
    let id = key.id;
    let update = tokio::spawn(async move {
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
            .bind(id)
            .bind(narrowed)
            .execute(&pool)
            .await
    });
    waits_behind_the_request(&mut blocker, pid, &update, &site.world.db.pool).await;
    assert_eq!(
        scope_of(&mut blocker, key.id).await,
        stored,
        "{lost}: the change of the key has not committed while the request holds it"
    );
    assert!(!request.is_finished());
    blocker.commit().await.unwrap();
    // The change ends when the request's transaction does: by what it returns, the copy it admitted is already queued.
    let landed = tokio::time::timeout(WAIT, update)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(landed.rows_affected(), 1);
    assert_eq!(
        copy_commands(&site).await,
        1,
        "{lost}: the admitted copy was queued before the change of the key could commit"
    );
    assert!(
        !request.is_finished(),
        "{lost}: the machine still holds the copy"
    );
    tokio::time::timeout(WAIT, held.entered.cancelled())
        .await
        .unwrap();
    held.release.cancel();
    let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
    if lost == "read" {
        receipt(reply, &site);
    } else {
        assert_eq!(shown(reply)["hash"], json!(Hash::of(CONTENT)));
    }
    assert_eq!(site.disk.read(TO).unwrap(), CONTENT);
    assert_eq!(held.calls.load(Ordering::SeqCst), 1);
    assert_eq!(copy_commands(&site).await, 1);
    // The next copy with the same key is refused as a key that never held the right is, and queues nothing.
    let next = copy(&site, &key.secret, Some(site.thread), FROM, AFTER).await;
    assert_eq!(next.status, StatusCode::FORBIDDEN, "{lost}");
    let never = keys::scoped(&site, site.ada.id(), vec![rule(&site, kept)]).await;
    let reference = copy(&site, &never.secret, Some(site.thread), FROM, AFTER).await;
    assert_eq!(reference.status, StatusCode::FORBIDDEN, "{lost}");
    assert_eq!(refusal(&next), refusal(&reference), "{lost}");
    assert!(!site.disk.exists(AFTER));
    assert_eq!(site.sent(CommandKind::FsCopy), 1);
    assert_eq!(copy_commands(&site).await, 1);
}

#[tokio::test]
async fn copy_rechecks_current_source_scope_policy_and_expiry_at_the_actual_queue_wait() {
    for change in ["live", "read", "write", "expiry", "generation", "policy"] {
        if matches!(change, "read" | "write") {
            key_change_is_ordered_after_the_admitted_copy(change).await;
            continue;
        }
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

#[tokio::test]
async fn a_completed_copy_header_reads_its_native_result_under_a_new_read_only_key_without_its_spent_write()
 {
    let site = fixture().await;
    let key = keys::scoped(&site, site.ada.id(), vec![rule(&site, &["read", "write"])]).await;
    let act = orch_types::new_id();
    let body = FileFromToBody {
        from: FROM.into(),
        to: TO.into(),
    };
    let call = site
        .app()
        .call(EnvironmentFileCopy, &[&site.environment])
        .token(&key.secret)
        .header("Idempotency-Key", &act.to_string())
        .query(&ThroughThreadQuery {
            thread: Some(site.thread),
        })
        .body(&body);
    let first = shown(call.send_bytes().await);
    assert_eq!(first["path"], TO);
    assert_eq!(first["hash"], json!(Hash::of(CONTENT)));
    assert_eq!(site.disk.read(TO).unwrap(), CONTENT);
    assert_eq!(site.sent(CommandKind::FsCopy), 1);
    let saved: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(site.ada.id())
    .bind(act.to_string())
    .fetch_one(&site.world.db.pool)
    .await
    .unwrap();
    let command: Id = serde_json::from_value(saved.0["outcome"]["id"].clone()).unwrap();
    assert_eq!(
        saved.0["outcome"]["data"]["native_command"]["id"],
        json!(command)
    );
    assert!(saved.0.to_string().find("entry").is_none());
    assert!(saved.0["outcome"]["data"].get("hash").is_none());
    let old =
        orch_db::queries::link::command_with_authority(&mut *site.world.conn().await, command)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(old.command.result.as_ref().unwrap(), &first);
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(key.id)
        .bind(site.world.now())
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET remote_write=false WHERE id=$1")
        .bind(site.environment)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    site.disk.make_unreadable(FROM);
    let authority: orch_engine::commands::CommandAuthority =
        serde_json::from_value(old.authority.clone().unwrap()).unwrap();
    let factual = authority.file_copy_receipt(&old.command).unwrap().unwrap();
    assert_eq!(factual.environment(), site.environment);
    assert_eq!(factual.destination(), TO);
    for field in ["id", "machine", "body", "serial"] {
        let mut other = old.command.clone();
        match field {
            "id" => other.id = orch_types::new_id(),
            "machine" => other.machine_id = orch_types::new_id(),
            "body" => other.body["to"] = json!("web/substitute"),
            _ => other.serial_key = Some("different".into()),
        }
        assert!(
            authority.file_copy_receipt(&other).unwrap().is_none(),
            "{field}"
        );
    }
    let reader = keys::scoped(&site, site.ada.id(), vec![rule(&site, &["read"])]).await;
    let call = site
        .app()
        .call(EnvironmentFileCopy, &[&site.environment])
        .token(&reader.secret)
        .header("Idempotency-Key", &act.to_string())
        .query(&ThroughThreadQuery {
            thread: Some(site.thread),
        })
        .body(&body);
    assert_eq!(shown(call.send_bytes().await), first);
    let unrelated = keys::scoped(
        &site,
        site.ada.id(),
        vec![keys::rule(
            "EnvironmentFileStat",
            "environment",
            site.environment,
            "self",
            &["read"],
        )],
    )
    .await;
    let call = site
        .app()
        .call(EnvironmentFileCopy, &[&site.environment])
        .token(&unrelated.secret)
        .header("Idempotency-Key", &act.to_string())
        .query(&ThroughThreadQuery {
            thread: Some(site.thread),
        })
        .body(&body);
    receipt(call.send_bytes().await, &site);
    let after =
        orch_db::queries::link::command_with_authority(&mut *site.world.conn().await, command)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(after.command, old.command);
    assert_eq!(after.authority, old.authority);
    let same: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(site.ada.id())
    .bind(act.to_string())
    .fetch_one(&site.world.db.pool)
    .await
    .unwrap();
    assert_eq!(same, saved);
    assert_eq!(site.sent(CommandKind::FsCopy), 1);
    assert_eq!(audits(&site).await, 1);
}

#[tokio::test]
async fn a_copy_header_retains_actual_acceptance_after_its_waiter_is_cancelled() {
    let mut site = fixture().await;
    let gate = Held::new(&site);
    site.power_on_with(gate.clone());
    let key = keys::scoped(&site, site.ada.id(), vec![rule(&site, &["read", "write"])]).await;
    let act = orch_types::new_id();
    let app = site.app().clone();
    let environment = site.environment;
    let token = key.secret.clone();
    let request = tokio::spawn(async move {
        let call = app
            .call(EnvironmentFileCopy, &[&environment])
            .token(&token)
            .header("Idempotency-Key", &act.to_string())
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
    let saved: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(site.ada.id())
    .bind(act.to_string())
    .fetch_one(&site.world.db.pool)
    .await
    .unwrap();
    let command: Id = serde_json::from_value(saved.0["outcome"]["id"].clone()).unwrap();
    request.abort();
    assert!(matches!(request.await, Err(error) if error.is_cancelled()));
    let reader = keys::scoped(&site, site.ada.id(), vec![rule(&site, &["read"])]).await;
    let call = site
        .app()
        .call(EnvironmentFileCopy, &[&environment])
        .token(&reader.secret)
        .header("Idempotency-Key", &act.to_string())
        .query(&ThroughThreadQuery { thread: None })
        .body(&FileFromToBody {
            from: FROM.into(),
            to: TO.into(),
        });
    let replay = tokio::time::timeout(WAIT, call.send_bytes()).await.unwrap();
    receipt(replay, &site);
    assert!(!site.disk.exists(TO));
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
    assert_eq!(site.sent(CommandKind::FsCopy), 1);
    let same: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(site.ada.id())
    .bind(act.to_string())
    .fetch_one(&site.world.db.pool)
    .await
    .unwrap();
    assert_eq!(same, saved);
    gate.release.cancel();
    tokio::time::timeout(WAIT, async {
        loop {
            let row = orch_db::queries::link::command(&mut *site.world.conn().await, command)
                .await
                .unwrap()
                .unwrap();
            if row.state == "succeeded" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(site.disk.read(TO).unwrap(), CONTENT);
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
}
