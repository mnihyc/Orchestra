//! Measured previews retain the actual source before queueing, after native waits and at publication.
use super::*;
use crate::snapshot_scope_helpers as keys;
use orch_api_contract::workspace::{ThreadSnapshotPreflight, ThreadSnapshotPreflightBody};
use orch_api_core::testing::{Machine, RawReply};
use orch_link_contract::commands::{Cmd, CommandError};
use orch_types::Json;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

const WAIT: std::time::Duration = std::time::Duration::from_secs(30);

fn rule(environment: Id, actions: &[&str]) -> Json {
    keys::rule(
        "ThreadSnapshotPreflight",
        "environment",
        environment,
        "self",
        actions,
    )
}
async fn fixture() -> Site {
    let mut site = Site::new().await;
    site.disk.put("web/private-marker/a", b"read once");
    site.disk.put("elsewhere/unselected", b"other files");
    site.power_on();
    site
}
async fn request(site: &Site, token: &str, body: &ThreadSnapshotPreflightBody) -> RawReply {
    let call = site
        .app()
        .call(ThreadSnapshotPreflight, &[&site.thread])
        .token(token)
        .body(body);
    call.send_bytes().await
}
fn json_reply(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn snapshots(site: &Site) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM snapshots WHERE thread_id=$1), (SELECT count(*) FROM jobs WHERE kind='snapshot' AND node_id=$1)")
        .bind(site.thread).fetch_one(&site.world.db.pool).await.unwrap()
}

struct Held {
    disk: Arc<workspace::Disk>,
    entered: CancellationToken,
    release: CancellationToken,
    measurements: AtomicUsize,
}
impl Held {
    fn new(site: &Site) -> Arc<Self> {
        Arc::new(Self {
            disk: site.disk.clone(),
            entered: CancellationToken::new(),
            release: CancellationToken::new(),
            measurements: AtomicUsize::new(0),
        })
    }
}
#[async_trait::async_trait]
impl Machine for Held {
    async fn run(&self, command: &Cmd) -> Result<Json, CommandError> {
        if command.kind == "fs.measure" {
            self.measurements.fetch_add(1, Ordering::SeqCst);
            self.entered.cancel();
            self.release.cancelled().await;
        }
        self.disk.run(command).await
    }
}
async fn wait_sql(
    site: &Site,
    pid: i32,
    pattern: &str,
    request: &tokio::task::JoinHandle<RawReply>,
) {
    tokio::time::timeout(WAIT,async {
        loop {
            assert!(!request.is_finished(),"the literal native wait must be reached");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
                .bind(pid).bind(pattern).fetch_one(&site.world.db.pool).await.unwrap();
            if blocked {break;} tokio::task::yield_now().await;
        }
    }).await.expect("observe the real native SQL wait");
}

#[tokio::test]
async fn preflight_needs_both_actual_environment_facets_and_keeps_native_measurement_without_job_effects()
 {
    let site = fixture().await;
    let body = ThreadSnapshotPreflightBody::default();
    let before = snapshots(&site).await;
    let native = json_reply(request(&site, &site.ada.token, &body).await);
    assert_eq!(native["file_count"], 1);
    let seen = site.sent(CommandKind::FsMeasure);
    for rules in [
        vec![rule(site.environment, &["read"])],
        vec![rule(site.environment, &["manage"])],
        vec![rule(site.thread, &["read", "manage"])],
        vec![keys::rule(
            "ThreadSnapshotCreate",
            "environment",
            site.environment,
            "self",
            &["read", "manage"],
        )],
        vec![keys::rule(
            "ThreadSnapshotPreflight",
            "thread",
            site.thread,
            "self",
            &["read", "manage"],
        )],
    ] {
        let key = keys::scoped(&site, site.ada.id(), rules).await;
        assert_eq!(
            request(&site, &key.secret, &body).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            site.sent(CommandKind::FsMeasure),
            seen,
            "scope denial must precede connector work"
        );
    }
    let viewer = keys::scoped(
        &site,
        site.vic.id(),
        vec![rule(site.environment, &["read", "manage"])],
    )
    .await;
    assert_eq!(
        request(&site, &viewer.secret, &body).await.status,
        StatusCode::FORBIDDEN
    );
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![rule(site.environment, &["read", "manage"])],
    )
    .await;
    assert_eq!(json_reply(request(&site, &key.secret, &body).await), native);
    assert_eq!(snapshots(&site).await, before);
}

#[tokio::test]
async fn preflight_checks_expiry_and_scope_after_actual_queue_source_wait_before_any_command_insert()
 {
    for changed in ["live", "expiry", "refused", "generation", "thread"] {
        let site = fixture().await;
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![rule(site.environment, &["read", "manage"])],
        )
        .await;
        let mut blocker = site.world.db.pool.begin().await.unwrap();
        let (sql, target, pattern) = if changed == "thread" {
            (
                "SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE",
                site.thread,
                "%FROM threads%FOR SHARE%",
            )
        } else {
            (
                "SELECT id FROM environments WHERE id=$1 FOR UPDATE",
                site.environment,
                "%FROM environments%FOR SHARE%",
            )
        };
        sqlx::query(sql)
            .bind(target)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let before: i64 =
            sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='fs.measure'")
                .fetch_one(&site.world.db.pool)
                .await
                .unwrap();
        let app = site.app().clone();
        let thread = site.thread;
        let token = key.secret.clone();
        let request = tokio::spawn(async move {
            let call = app
                .call(ThreadSnapshotPreflight, &[&thread])
                .token(&token)
                .body(&ThreadSnapshotPreflightBody::default());
            call.send_bytes().await
        });
        wait_sql(&site, pid, pattern, &request).await;
        assert_eq!(site.sent(CommandKind::FsMeasure), 0);
        let current: i64 =
            sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='fs.measure'")
                .fetch_one(&site.world.db.pool)
                .await
                .unwrap();
        assert_eq!(current, before);
        if changed == "expiry" {
            site.world.advance(Duration::hours(2));
        }
        if changed == "refused" {
            // A change of the key's own rules is ordered after the request that holds the key's row, so it cannot land
            // while the request waits at the source; the org refusing the key can.
            site.world
                .keep_key_out(key.id, site.org, site.ada.id())
                .await;
        }
        if changed == "generation" {
            sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                .bind(site.environment)
                .execute(&mut *blocker)
                .await
                .unwrap();
        }
        if changed == "thread" {
            sqlx::query("UPDATE threads SET environment_id=NULL WHERE node_id=$1")
                .bind(site.thread)
                .execute(&mut *blocker)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if changed == "live" {
            assert_eq!(json_reply(reply)["file_count"], 1);
            assert_eq!(site.sent(CommandKind::FsMeasure), 1);
        } else {
            assert!(!reply.status.is_success());
            assert_eq!(site.sent(CommandKind::FsMeasure), 0);
            let after: i64 =
                sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='fs.measure'")
                    .fetch_one(&site.world.db.pool)
                    .await
                    .unwrap();
            assert_eq!(after, before);
        }
        assert_eq!(snapshots(&site).await, (0, 0));
    }
}

#[tokio::test]
async fn preflight_never_discloses_an_accepted_measurement_after_source_scope_or_key_loss() {
    for changed in [
        "live",
        "generation",
        "directory",
        "scope",
        "revoked",
        "expiry",
    ] {
        let mut site = fixture().await;
        let held = Held::new(&site);
        site.power_on_with(held.clone());
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![rule(site.environment, &["read", "manage"])],
        )
        .await;
        let app = site.app().clone();
        let thread = site.thread;
        let token = key.secret.clone();
        let request = tokio::spawn(async move {
            let call = app
                .call(ThreadSnapshotPreflight, &[&thread])
                .token(&token)
                .body(&ThreadSnapshotPreflightBody::default());
            call.send_bytes().await
        });
        tokio::time::timeout(WAIT, held.entered.cancelled())
            .await
            .unwrap();
        assert_eq!(held.measurements.load(Ordering::SeqCst), 1);
        match changed {
            "generation" => {
                sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                    .bind(site.environment)
                    .execute(&site.world.db.pool)
                    .await
                    .unwrap();
            }
            "directory" => {
                sqlx::query("UPDATE threads SET workspace_dir='elsewhere' WHERE node_id=$1")
                    .bind(site.thread)
                    .execute(&site.world.db.pool)
                    .await
                    .unwrap();
            }
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(key.id)
                    .bind(json!({"rules":[rule(site.environment,&["manage"])]}))
                    .execute(&site.world.db.pool)
                    .await
                    .unwrap();
            }
            "revoked" => keys::revoke(&site, &key).await,
            "expiry" => site.world.advance(Duration::hours(2)),
            _ => {}
        }
        held.release.cancel();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if changed == "live" {
            assert_eq!(json_reply(reply)["file_count"], 1);
        } else {
            assert!(
                !reply.status.is_success(),
                "{changed}: {}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("private-marker"));
        }
        assert_eq!(
            held.measurements.load(Ordering::SeqCst),
            1,
            "current source checks never repeat connector IO"
        );
        assert_eq!(snapshots(&site).await, (0, 0));
    }
}

#[tokio::test]
async fn preflight_explicit_paths_keep_their_native_environment_scope_when_the_default_directory_moves()
 {
    let mut site = fixture().await;
    let held = Held::new(&site);
    site.power_on_with(held.clone());
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![rule(site.environment, &["read", "manage"])],
    )
    .await;
    let app = site.app().clone();
    let thread = site.thread;
    let request = tokio::spawn(async move {
        let call = app
            .call(ThreadSnapshotPreflight, &[&thread])
            .token(&key.secret)
            .body(&ThreadSnapshotPreflightBody {
                selection: Some(ThreadSnapshotCreateSelection {
                    include: vec!["web".into()],
                    exclude: None,
                }),
            });
        call.send_bytes().await
    });
    tokio::time::timeout(WAIT, held.entered.cancelled())
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET workspace_dir='elsewhere' WHERE node_id=$1")
        .bind(site.thread)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    held.release.cancel();
    assert_eq!(
        json_reply(tokio::time::timeout(WAIT, request).await.unwrap().unwrap())["file_count"],
        1
    );
    assert_eq!(held.measurements.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn preflight_final_publication_preserves_current_required_manage_and_its_original_measured_source()
 {
    for changed in ["manage", "read", "generation", "expiry"] {
        let site = fixture().await;
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![rule(site.environment, &["read", "manage"])],
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
        let thread = site.thread;
        let token = key.secret.clone();
        let request = tokio::spawn(async move {
            let call = app
                .call(ThreadSnapshotPreflight, &[&thread])
                .token(&token)
                .body(&ThreadSnapshotPreflightBody::default());
            call.send_bytes().await
        });
        wait_sql(&site, pid, "%INSERT INTO api_token_activity%", &request).await;
        assert_eq!(site.sent(CommandKind::FsMeasure), 1);
        match changed {
            "generation" => {
                sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                    .bind(site.environment)
                    .execute(&site.world.db.pool)
                    .await
                    .unwrap();
            }
            "expiry" => site.world.advance(Duration::hours(2)),
            _ => {
                let actions = if changed == "manage" {
                    vec!["read"]
                } else {
                    vec!["manage"]
                };
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(key.id)
                    .bind(json!({"rules":[rule(site.environment,&actions)]}))
                    .execute(&site.world.db.pool)
                    .await
                    .unwrap();
            }
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        assert!(!reply.status.is_success());
        assert!(!String::from_utf8_lossy(&reply.raw).contains("private-marker"));
        assert_eq!(site.sent(CommandKind::FsMeasure), 1);
        assert_eq!(snapshots(&site).await, (0, 0));
    }
}

#[tokio::test]
async fn preflight_commands_keep_private_exact_origin_evidence_and_unchanged_native_wire_bytes() {
    let site = fixture().await;
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![rule(site.environment, &["read", "manage"])],
    )
    .await;
    json_reply(request(&site, &key.secret, &ThreadSnapshotPreflightBody::default()).await);
    let seen = site
        .played
        .as_ref()
        .unwrap()
        .seen_of(CommandKind::FsMeasure);
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].body,
        json!({"environment_id":site.environment,"selection":{"include":["web"],"exclude":[]}})
    );
    let authority =
        orch_db::queries::link::command_authority(&mut *site.world.conn().await, seen[0].id)
            .await
            .unwrap()
            .unwrap();
    let decoded: orch_engine::commands::CommandAuthority =
        serde_json::from_value(authority.clone()).unwrap();
    assert_eq!(serde_json::to_value(&decoded).unwrap(), authority);
    assert_eq!(authority["kind"], "workspace");
    let native = orch_db::queries::link::command(&mut *site.world.conn().await, seen[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(native.body, seen[0].body);
    assert!(!native.body.to_string().contains(&key.secret));
    let mut unknown = authority;
    unknown["version"] = json!(2);
    assert!(serde_json::from_value::<orch_engine::commands::CommandAuthority>(unknown).is_err());
}
