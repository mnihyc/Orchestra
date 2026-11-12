//! Actual HTTP, retained native command and final-publication cuts of recorded restores.

use super::*;
use http::StatusCode;
use orch_api_contract::{
    forks::{ActiveTurn, ChangeOptions},
    source::{
        EnvironmentFilesRestore, EnvironmentFilesRestoreBody, EnvironmentRestorePreflight,
        EnvironmentRestorePreflightBody, RestorePlan,
    },
};
use orch_api_core::testing::{Machine, RawReply};
use orch_link_contract::{
    commands::{Cmd, CommandError},
    source::{FsRecord, FsRestore},
};
use orch_types::{Id, Json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio_util::sync::CancellationToken;

const WAIT: std::time::Duration = std::time::Duration::from_secs(30);

struct Peer {
    inner: Arc<RestoreReader>,
    calls: Mutex<Vec<Cmd>>,
    hold: Mutex<Option<&'static str>>,
    entered: CancellationToken,
    release: CancellationToken,
    applied: AtomicBool,
    now: Time,
}
#[async_trait::async_trait]
impl Machine for Peer {
    async fn run(&self, command: &Cmd) -> Result<Json, CommandError> {
        self.calls.lock().unwrap().push(command.clone());
        let held = *self.hold.lock().unwrap() == Some(command.kind.as_str());
        if held {
            self.entered.cancel();
            self.release.cancelled().await;
        }
        match command.kind.as_str() {
            "fs.restore.plan" => self.inner.run(command).await,
            "proc.suspend.owned" | "proc.resume.owned" => Ok(json!({"processes":1})),
            "proc.list" => Ok(json!({"as_of":self.now,"processes":[]})),
            "fs.restore" => {
                let body: FsRestore = serde_json::from_value(command.body.clone()).unwrap();
                self.applied.store(true, Ordering::SeqCst);
                self.inner.disk.put("web/code", b"before");
                Ok(json!({"files":1,"undo_snapshot_id":body.undo_snapshot_id,"undone":false}))
            }
            "fs.record" => {
                let body: FsRecord = serde_json::from_value(command.body.clone()).unwrap();
                assert!(self.applied.load(Ordering::SeqCst));
                assert_eq!(body.kind, orch_link_contract::source::RecordKind::Restore);
                let built = orch_engine::manifest::build(
                    &orch_engine::manifest::Header {
                        v: orch_engine::manifest::MANIFEST_VERSION,
                        root: "/workspace/web".into(),
                        generated_at: self.now,
                        generation: 1,
                        case_sensitive: true,
                    },
                    vec![orch_engine::manifest::Entry {
                        path: "code".into(),
                        kind: orch_engine::manifest::EntryKind::File,
                        size: 5,
                        mtime_ns: 0,
                        mode: 0o644,
                        hash: Some(Hash::of(b"later")),
                        link: None,
                    }],
                )
                .unwrap();
                let uploaded = self
                    .inner
                    .content
                    .upload(
                        self.inner.disk.machine,
                        command.id,
                        &built.bytes,
                        workspace::PIECE,
                    )
                    .await;
                Ok(
                    json!({"state":"complete","tree_ref":"owned-original-undo","repository_head":null,"manifest_blob":uploaded,"files":1,"bytes":5,"expires_at":Time::new(self.now.get()+chrono::Duration::days(7)),"reason":null}),
                )
            }
            _ => self.inner.disk.run(command).await,
        }
    }
}
impl Peer {
    fn count(&self, kind: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|command| command.kind == kind)
            .count()
    }
}

async fn fixture() -> (Site, Id, Arc<Peer>) {
    let (mut site, snapshot, inner) = restore_fixture().await;
    let peer = Arc::new(Peer {
        inner,
        calls: Mutex::new(Vec::new()),
        hold: Mutex::new(None),
        entered: CancellationToken::new(),
        release: CancellationToken::new(),
        applied: AtomicBool::new(false),
        now: Time::new(site.world.now()),
    });
    site.power_on_with(peer.clone());
    (site, snapshot, peer)
}
fn preview(environment: Id, actions: &[&str]) -> Json {
    keys::rule(
        "EnvironmentRestorePreflight",
        "environment",
        environment,
        "self",
        actions,
    )
}
fn confirmation(environment: Id, mode: &str, actions: &[&str]) -> Json {
    keys::rule(
        &format!("EnvironmentFilesRestore.{mode}"),
        "environment",
        environment,
        "self",
        actions,
    )
}
fn names(thread: Id) -> Json {
    keys::rule(
        "EnvironmentRestorePreflight",
        "thread",
        thread,
        "self",
        &["read"],
    )
}
async fn review(
    site: &Site,
    snapshot: Id,
    token: &str,
    options: Option<ChangeOptions>,
) -> RawReply {
    let call = site
        .app()
        .call(EnvironmentRestorePreflight, &[&site.environment])
        .token(token)
        .body(&EnvironmentRestorePreflightBody {
            overwrite: Vec::new(),
            snapshot_id: snapshot,
            options,
        });
    call.send_bytes().await
}
async fn confirm(site: &Site, id: Id, token: &str) -> RawReply {
    let call = site
        .app()
        .call(EnvironmentFilesRestore, &[&site.environment])
        .token(token)
        .body(&EnvironmentFilesRestoreBody { preflight_id: id });
    call.send_bytes().await
}
fn full(reply: RawReply) -> RestorePlan {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
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
async fn counts(site: &Site) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM preflights WHERE kind='workspace_restore'),(SELECT count(*) FROM jobs WHERE kind='workspace_restore')")
        .fetch_one(&site.world.db.pool).await.unwrap()
}
async fn narrow(site: &Site, key: Id, rules: Vec<Json>) {
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(key)
        .bind(json!({"rules":rules}))
        .execute(&site.world.db.pool)
        .await
        .unwrap();
}
async fn wait_sql(
    site: &Site,
    pid: i32,
    pattern: &str,
    request: &tokio::task::JoinHandle<RawReply>,
) {
    tokio::time::timeout(WAIT,async {loop {
        assert!(!request.is_finished(),"the actual SQL wait must be reached");
        let held:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
            .bind(pid).bind(pattern).fetch_one(&site.world.db.pool).await.unwrap();
        if held {break;}tokio::task::yield_now().await;
    }}).await.expect("observe the real plan or publication wait");
}
async fn finished(site: &Site, id: Id) -> orch_db::queries::jobs::JobRow {
    tokio::time::timeout(WAIT, async {
        loop {
            let row = jobs::job(&mut *site.world.conn().await, id)
                .await
                .unwrap()
                .unwrap();
            if !row.is_open() {
                break row;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the actual native worker reaches an ending")
}

#[tokio::test]
async fn restore_review_needs_its_own_file_read_and_management_and_names_have_their_own_read() {
    let (site, snapshot, peer) = fixture().await;
    for rules in [
        vec![preview(site.environment, &["manage"])],
        vec![preview(site.environment, &["read"])],
        vec![keys::read("SnapshotGet", site.environment)],
        vec![preview(snapshot, &["read", "manage"])],
    ] {
        let key = keys::scoped(&site, site.ada.id(), rules).await;
        assert_eq!(
            review(&site, snapshot, &key.secret, None).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(counts(&site).await, (0, 0));
    }
    let denied = keys::scoped(
        &site,
        site.vic.id(),
        vec![preview(site.environment, &["read", "manage"])],
    )
    .await;
    assert_eq!(
        review(&site, snapshot, &denied.secret, None).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(peer.count("fs.restore.plan"), 0);
    let plain = full(review(&site, snapshot, &site.ada.token, None).await);
    assert!(plain.writers.iter().any(|writer| {
        writer
            .thread
            .as_ref()
            .is_some_and(|thread| thread.id == site.thread)
    }));
    let root = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            preview(site.environment, &["read", "manage"]),
            keys::rule("NodeGet.read", "thread", site.thread, "self", &["read"]),
        ],
    )
    .await;
    let hidden = full(review(&site, snapshot, &root.secret, None).await);
    assert_eq!(hidden.snapshot, plain.snapshot);
    assert_eq!(hidden.paths, plain.paths);
    assert!(hidden.writers.iter().all(|writer| writer.thread.is_none()));
    let named = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            preview(site.environment, &["read", "manage"]),
            names(site.thread),
        ],
    )
    .await;
    let shown = full(review(&site, snapshot, &named.secret, None).await);
    assert_eq!(shown.writers, plain.writers);
    assert_eq!(counts(&site).await, (3, 0));
}

#[tokio::test]
async fn one_accepted_native_measurement_cannot_save_a_plan_after_key_or_physical_source_loss() {
    for change in ["live", "scope", "source"] {
        let (site, snapshot, peer) = fixture().await;
        *peer.hold.lock().unwrap() = Some("fs.restore.plan");
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![preview(site.environment, &["read", "manage"])],
        )
        .await;
        let app = site.app().clone();
        let env = site.environment;
        let token = key.secret.clone();
        let request = tokio::spawn(async move {
            let call = app
                .call(EnvironmentRestorePreflight, &[&env])
                .token(&token)
                .body(&EnvironmentRestorePreflightBody {
                    overwrite: Vec::new(),
                    snapshot_id: snapshot,
                    options: None,
                });
            call.send_bytes().await
        });
        tokio::time::timeout(WAIT, peer.entered.cancelled())
            .await
            .unwrap();
        if change == "scope" {
            narrow(&site, key.id, vec![preview(env, &["read"])]).await;
        }
        if change == "source" {
            sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                .bind(env)
                .execute(&site.world.db.pool)
                .await
                .unwrap();
        }
        peer.release.cancel();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        assert_eq!(peer.count("fs.restore.plan"), 1);
        if change == "live" {
            full(reply);
            assert_eq!(counts(&site).await, (1, 0));
        } else {
            assert!(!reply.status.is_success());
            assert_eq!(counts(&site).await, (0, 0));
        }
    }
}

#[tokio::test]
async fn new_confirmation_keeps_preview_and_acting_operations_separate_and_never_loans_the_other_keys_manage()
 {
    for change in ["live", "preview", "acting_read"] {
        let (site, snapshot, _) = fixture().await;
        let preview_key = keys::scoped(
            &site,
            site.ada.id(),
            vec![preview(site.environment, &["read", "manage"])],
        )
        .await;
        let plan = full(review(&site, snapshot, &preview_key.secret, None).await);
        let actions = if change == "acting_read" {
            vec!["manage"]
        } else {
            vec!["read", "manage"]
        };
        let acting = keys::scoped(
            &site,
            site.ada.id(),
            vec![confirmation(site.environment, "environment", &actions)],
        )
        .await;
        if change == "preview" {
            narrow(
                &site,
                preview_key.id,
                vec![preview(site.environment, &["read"])],
            )
            .await;
        }
        let reply = confirm(&site, plan.preflight_id, &acting.secret).await;
        if change == "live" {
            let response = accepted(reply);
            let id: Id = serde_json::from_value(response["id"].clone()).unwrap();
            let row = jobs::job(&mut *site.world.conn().await, id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                row.plan["restore_operation"]["review"]["origin"]["operation"],
                "EnvironmentRestorePreflight"
            );
            assert_eq!(
                row.plan["restore_operation"]["origin"]["operation"],
                "EnvironmentFilesRestore.environment"
            );
            assert_eq!(row.plan["action"]["body"]["job_id"], json!(id));
            assert_eq!(counts(&site).await, (1, 1));
        } else {
            assert!(!reply.status.is_success());
            assert_eq!(counts(&site).await, (1, 0));
        }
    }
}

#[tokio::test]
async fn thread_control_alternative_requires_the_actual_writer_write_and_native_interact() {
    let (site, snapshot, _) = fixture().await;
    for (object, id, permission) in [
        ("environment", site.environment, "files.manage"),
        ("node", site.thread, "thread.interact"),
    ] {
        orch_db::queries::access::testing::add_entry(
            &mut *site.world.conn().await,
            &orch_db::queries::access::PageKey::new(object, id, "live"),
            "any",
            permission,
            ("user", Some(site.vic.id()), None),
            None,
            site.world.now(),
        )
        .await
        .unwrap();
    }
    let preview_key = keys::scoped(
        &site,
        site.vic.id(),
        vec![preview(site.environment, &["read", "manage"])],
    )
    .await;
    let plan = full(
        review(
            &site,
            snapshot,
            &preview_key.secret,
            Some(ChangeOptions {
                active_turn: Some(ActiveTurn::Interrupt),
                ..Default::default()
            }),
        )
        .await,
    );
    assert!(plan.blockers.is_empty());
    let missing = keys::scoped(
        &site,
        site.vic.id(),
        vec![confirmation(
            site.environment,
            "threads",
            &["read", "manage"],
        )],
    )
    .await;
    assert_eq!(
        confirm(&site, plan.preflight_id, &missing.secret)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let writer = keys::rule(
        "EnvironmentFilesRestore.threads",
        "thread",
        site.thread,
        "self",
        &["write"],
    );
    let complete = keys::scoped(
        &site,
        site.vic.id(),
        vec![
            confirmation(site.environment, "threads", &["read", "manage"]),
            writer,
        ],
    )
    .await;
    let response = accepted(confirm(&site, plan.preflight_id, &complete.secret).await);
    let id: Id = serde_json::from_value(response["id"].clone()).unwrap();
    let row = jobs::job(&mut *site.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.plan["restore_operation"]["origin"]["operation"],
        "EnvironmentFilesRestore.threads"
    );
}

#[tokio::test]
async fn confirmation_replay_has_its_own_job_read_and_does_not_reauthorize_completed_file_work() {
    let (site, snapshot, peer) = fixture().await;
    let first = keys::scoped(
        &site,
        site.ada.id(),
        vec![preview(site.environment, &["read", "manage"])],
    )
    .await;
    let plan = full(review(&site, snapshot, &first.secret, None).await);
    let starter = keys::scoped(
        &site,
        site.ada.id(),
        vec![confirmation(
            site.environment,
            "environment",
            &["read", "manage"],
        )],
    )
    .await;
    let response = accepted(confirm(&site, plan.preflight_id, &starter.secret).await);
    let id: Id = serde_json::from_value(response["id"].clone()).unwrap();
    keys::revoke(&site, &first).await;
    let other_read = keys::rule("JobGet.ordinary", "job", id, "self", &["read"]);
    let replay = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            confirmation(site.environment, "existing", &["manage"]),
            other_read,
        ],
    )
    .await;
    assert_eq!(
        accepted(confirm(&site, plan.preflight_id, &replay.secret).await),
        json!({"projection":"receipt","job_id":id,"preflight_id":plan.preflight_id})
    );
    let own_read = keys::rule(
        "EnvironmentFilesRestore.existing",
        "job",
        id,
        "self",
        &["read"],
    );
    let full = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            confirmation(site.environment, "existing", &["manage"]),
            own_read,
        ],
    )
    .await;
    assert_eq!(
        accepted(confirm(&site, plan.preflight_id, &full.secret).await)["id"],
        json!(id)
    );
    assert_eq!(counts(&site).await, (1, 1));
    assert_eq!(peer.count("fs.restore.plan"), 1);
    assert_eq!(peer.count("fs.restore"), 0);
}

#[tokio::test]
async fn confirmation_rechecks_the_original_preview_after_the_real_saved_plan_lock() {
    for closed in [false, true] {
        let (site, snapshot, _) = fixture().await;
        let original = keys::scoped(
            &site,
            site.ada.id(),
            vec![preview(site.environment, &["read", "manage"])],
        )
        .await;
        let plan = full(review(&site, snapshot, &original.secret, None).await);
        let acting = keys::scoped(
            &site,
            site.ada.id(),
            vec![confirmation(
                site.environment,
                "environment",
                &["read", "manage"],
            )],
        )
        .await;
        let mut blocker = site.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM preflights WHERE id=$1 FOR UPDATE")
            .bind(plan.preflight_id)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = site.app().clone();
        let env = site.environment;
        let id = plan.preflight_id;
        let request = tokio::spawn(async move {
            let call = app
                .call(EnvironmentFilesRestore, &[&env])
                .token(&acting.secret)
                .body(&EnvironmentFilesRestoreBody { preflight_id: id });
            call.send_bytes().await
        });
        wait_sql(&site, pid, "%FROM preflights%FOR UPDATE%", &request).await;
        if closed {
            keys::revoke(&site, &original).await;
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if closed {
            assert!(!reply.status.is_success());
            assert_eq!(counts(&site).await, (1, 0));
        } else {
            accepted(reply);
            assert_eq!(counts(&site).await, (1, 1));
        }
    }
}

#[tokio::test]
async fn a_queued_restore_does_not_start_writer_or_filesystem_work_after_its_preview_scope_closes()
{
    let (site, snapshot, peer) = fixture().await;
    let original = keys::scoped(
        &site,
        site.ada.id(),
        vec![preview(site.environment, &["read", "manage"])],
    )
    .await;
    let plan = full(review(&site, snapshot, &original.secret, None).await);
    let acting = keys::scoped(
        &site,
        site.ada.id(),
        vec![confirmation(
            site.environment,
            "environment",
            &["read", "manage"],
        )],
    )
    .await;
    let response = accepted(confirm(&site, plan.preflight_id, &acting.secret).await);
    let id: Id = serde_json::from_value(response["id"].clone()).unwrap();
    let measured = peer
        .calls
        .lock()
        .unwrap()
        .iter()
        .find(|command| command.kind == "fs.restore.plan")
        .expect("the admitted planning command was observed")
        .body
        .clone();
    let queued = jobs::job(&mut *site.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    for member in ["record_days", "max_bytes"] {
        assert!(
            measured[member].as_i64().is_some(),
            "the native plan freezes {member}"
        );
        assert_eq!(
            queued.plan["action"][member], measured[member],
            "the queued restore retains the exact measured recording policy"
        );
    }
    keys::revoke(&site, &original).await;
    site.app().app.engine.start();
    let row = finished(&site, id).await;
    assert_eq!(row.state, "failed");
    assert_eq!(peer.count("proc.suspend.owned"), 0);
    assert_eq!(peer.count("fs.restore"), 0);
    site.app().app.engine.stop();
}

#[tokio::test]
async fn accepted_restore_finishes_only_its_owned_undo_and_resume_after_both_original_keys_end() {
    let (site, snapshot, peer) = fixture().await;
    *peer.hold.lock().unwrap() = Some("fs.restore");
    let original = keys::scoped(
        &site,
        site.ada.id(),
        vec![preview(site.environment, &["read", "manage"])],
    )
    .await;
    let plan = full(review(&site, snapshot, &original.secret, None).await);
    let acting = keys::scoped(
        &site,
        site.ada.id(),
        vec![confirmation(
            site.environment,
            "environment",
            &["read", "manage"],
        )],
    )
    .await;
    let response = accepted(confirm(&site, plan.preflight_id, &acting.secret).await);
    let id: Id = serde_json::from_value(response["id"].clone()).unwrap();
    site.app().app.engine.start();
    tokio::time::timeout(WAIT, peer.entered.cancelled())
        .await
        .unwrap();
    keys::revoke(&site, &original).await;
    keys::revoke(&site, &acting).await;
    peer.release.cancel();
    let row = finished(&site, id).await;
    assert_eq!(row.state, "succeeded", "{:?}", row.error);
    assert_eq!(peer.count("fs.restore"), 1);
    assert_eq!(peer.count("fs.record"), 1);
    assert_eq!(peer.count("proc.resume.owned"), 1);
    for command in peer.calls.lock().unwrap().iter().filter(|command| {
        matches!(
            command.kind.as_str(),
            "proc.suspend.owned" | "proc.resume.owned"
        )
    }) {
        assert_eq!(command.body["owner_id"], json!(id));
    }
    assert_eq!(
        site.disk.read("web/code").as_deref(),
        Some(b"before".as_slice())
    );
    site.app().app.engine.stop();
}

#[tokio::test]
async fn saved_review_publication_uses_a_receipt_after_content_or_writer_read_loss_without_remeasuring()
 {
    for change in ["read", "names", "manage"] {
        let (site, snapshot, peer) = fixture().await;
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![
                preview(site.environment, &["read", "manage"]),
                names(site.thread),
            ],
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
        let env = site.environment;
        let token = key.secret.clone();
        let request = tokio::spawn(async move {
            let call = app
                .call(EnvironmentRestorePreflight, &[&env])
                .token(&token)
                .body(&EnvironmentRestorePreflightBody {
                    overwrite: Vec::new(),
                    snapshot_id: snapshot,
                    options: None,
                });
            call.send_bytes().await
        });
        wait_sql(&site, pid, "%INSERT INTO api_token_activity%", &request).await;
        assert_eq!(counts(&site).await, (1, 0));
        let id: Id = sqlx::query_scalar("SELECT id FROM preflights WHERE kind='workspace_restore'")
            .fetch_one(&site.world.db.pool)
            .await
            .unwrap();
        let rules = match change {
            "read" => vec![preview(env, &["manage"]), names(site.thread)],
            "names" => vec![preview(env, &["read", "manage"])],
            _ => vec![preview(env, &["read"]), names(site.thread)],
        };
        narrow(&site, key.id, rules).await;
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        assert_eq!(reply.status, StatusCode::OK);
        let value: Json = serde_json::from_slice(&reply.raw).unwrap();
        if change == "manage" {
            assert_eq!(value["snapshot"]["id"], json!(snapshot));
        } else {
            assert_eq!(value, json!({"projection":"receipt","preflight_id":id}));
        }
        assert_eq!(peer.count("fs.restore.plan"), 1);
        assert_eq!(counts(&site).await, (1, 0));
    }
}
