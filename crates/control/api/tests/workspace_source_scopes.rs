//! Native source preparation retains its actual operation, frozen recipe and accepted outcomes.
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
use orch_api_contract::source::{
    EnvironmentSourcePrepare, EnvironmentSourcePrepareBody, RepositorySource, SourceInfo,
    SourceKind, SourceState,
};
use orch_api_core::testing::{Machine, RawReply};
use orch_db::queries::{jobs, threads};
use orch_link_contract::commands::{Cmd, CommandError, CommandKind};
use orch_types::{Id, Json};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::sync::CancellationToken;
use workspace::Site;

const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
fn env(site: &Site, mode: &str, actions: &[&str]) -> Json {
    keys::rule(
        &format!("EnvironmentSourcePrepare.{mode}"),
        "environment",
        site.environment,
        "self",
        actions,
    )
}
fn thread(site: &Site, mode: &str) -> Json {
    keys::rule(
        &format!("EnvironmentSourcePrepare.{mode}"),
        "thread",
        site.thread,
        "self",
        &["read"],
    )
}
async fn fixture(repository: bool) -> Site {
    let mut site = Site::new().await;
    if repository {
        sqlx::query("UPDATE threads SET workspace_dir='checkout',source=$2 WHERE node_id=$1").bind(site.thread).bind(json!({"source":RepositorySource{repository_url:Some("https://code.example.test/frozen/source.git".into()),branch:Some("frozen-main".into()),secret_name:None}})).execute(&site.world.db.pool).await.unwrap();
    } else {
        site.disk.put("web/existing", b"existing source content");
    }
    site.power_on();
    site
}
async fn prepare(site: &Site, token: &str) -> RawReply {
    let call = site
        .app()
        .call(EnvironmentSourcePrepare, &[&site.environment])
        .token(token)
        .body(&EnvironmentSourcePrepareBody {
            thread_id: site.thread,
            source: None,
        });
    call.send_bytes().await
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
async fn ended(site: &Site, id: Id) -> jobs::JobRow {
    workspace::eventually("the actual source job to end", || async {
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
struct Native {
    disk: Arc<workspace::Disk>,
    kind: &'static str,
    entered: CancellationToken,
    release: CancellationToken,
    calls: AtomicUsize,
}
impl Native {
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
impl Machine for Native {
    async fn run(&self, cmd: &Cmd) -> Result<Json, CommandError> {
        if cmd.kind == self.kind {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.cancel();
            self.release.cancelled().await;
        }
        if cmd.kind == "git.prepare" {
            let body: orch_link_contract::source::GitPrepare =
                serde_json::from_value(cmd.body.clone()).unwrap();
            return Ok(json!(SourceInfo {
                kind: SourceKind::Repository,
                directory: body.directory,
                repository_url: Some(body.repository_url),
                branch: body.branch,
                head: Some("1111111111111111111111111111111111111111".into()),
                state: SourceState::Ready,
                job: None,
                reason: None
            }));
        }
        if cmd.kind == "git.worktree" {
            let body: orch_link_contract::source::GitWorktree =
                serde_json::from_value(cmd.body.clone()).unwrap();
            return Ok(json!(SourceInfo {
                kind: SourceKind::Worktree,
                directory: body.choice.directory,
                repository_url: None,
                branch: Some(body.choice.branch),
                head: Some("1111111111111111111111111111111111111111".into()),
                state: SourceState::Ready,
                job: None,
                reason: None
            }));
        }
        self.disk.run(cmd).await
    }
}
async fn wait_sql(
    site: &Site,
    pid: i32,
    pattern: &str,
    request: &tokio::task::JoinHandle<RawReply>,
) {
    tokio::time::timeout(WAIT,async{loop{assert!(!request.is_finished(),"the actual source wait must be reached");let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)").bind(pid).bind(pattern).fetch_one(&site.world.db.pool).await.unwrap();if blocked{break;}tokio::task::yield_now().await;}}).await.expect("observe the literal native source lock/publication wait");
}

#[tokio::test]
async fn preparation_needs_actual_environment_management_and_its_configuration_thread_read() {
    let site = fixture(false).await;
    for rules in [
        vec![env(&site, "prepare", &["manage"])],
        vec![thread(&site, "prepare")],
        vec![
            env(&site, "prepare", &["manage"]),
            keys::rule("NodeGet.read", "thread", site.thread, "self", &["read"]),
        ],
        vec![
            env(&site, "worktree", &["read", "manage"]),
            thread(&site, "worktree"),
        ],
    ] {
        let key = keys::scoped(&site, site.ada.id(), rules).await;
        assert_eq!(
            prepare(&site, &key.secret).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let viewer = keys::scoped(
        &site,
        site.vic.id(),
        vec![env(&site, "prepare", &["manage"]), thread(&site, "prepare")],
    )
    .await;
    assert_eq!(
        prepare(&site, &viewer.secret).await.status,
        StatusCode::FORBIDDEN
    );
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![env(&site, "prepare", &["manage"]), thread(&site, "prepare")],
    )
    .await;
    let body = accepted(prepare(&site, &key.secret).await);
    assert_eq!(body["state"], "queued");
    assert!(body["result"].is_null());
    assert!(body["log"].is_null());
    let id: Id = body["id"].as_str().unwrap().parse().unwrap();
    let row = jobs::job(&mut *site.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.plan["source_operation"]["origin"]["operation"],
        "EnvironmentSourcePrepare.prepare"
    );
    assert_eq!(row.node_id, Some(site.thread));
    assert_eq!(row.environment_id, Some(site.environment));
}

#[tokio::test]
async fn worktree_intent_is_server_selected_and_additionally_reads_the_existing_workspace() {
    let mut site = fixture(false).await;
    let native = Native::new(&site, "git.worktree");
    native.release.cancel();
    site.power_on_with(native.clone());
    sqlx::query("UPDATE threads SET source=$2 WHERE node_id=$1").bind(site.thread).bind(json!({"source":RepositorySource::default(),"worktree":{"repository_dir":"repo","directory":"web","branch":"new-own-branch","from":{"commit":"1111111111111111111111111111111111111111"}}})).execute(&site.world.db.pool).await.unwrap();
    for rules in [
        vec![env(&site, "prepare", &["manage"]), thread(&site, "prepare")],
        vec![
            env(&site, "worktree", &["manage"]),
            thread(&site, "worktree"),
        ],
    ] {
        let key = keys::scoped(&site, site.ada.id(), rules).await;
        assert_eq!(
            prepare(&site, &key.secret).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            env(&site, "worktree", &["read", "manage"]),
            thread(&site, "worktree"),
        ],
    )
    .await;
    let body = accepted(prepare(&site, &key.secret).await);
    let id: Id = body["id"].as_str().unwrap().parse().unwrap();
    site.app().app.engine.start();
    assert_eq!(ended(&site, id).await.state, "succeeded");
    assert_eq!(native.calls.load(Ordering::SeqCst), 1);
    site.app().app.engine.stop();
}

#[tokio::test]
async fn preparation_refreshes_the_current_key_after_the_actual_environment_lock_before_inserting()
{
    for expired in [false, true] {
        let site = fixture(false).await;
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![env(&site, "prepare", &["manage"]), thread(&site, "prepare")],
        )
        .await;
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
        let request = tokio::spawn(async move {
            let call = app
                .call(EnvironmentSourcePrepare, &[&environment])
                .token(&key.secret)
                .body(&EnvironmentSourcePrepareBody {
                    thread_id: thread,
                    source: None,
                });
            call.send_bytes().await
        });
        wait_sql(&site, pid, "%FROM environments%FOR UPDATE%", &request).await;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='workspace_prepare'")
                .fetch_one(&site.world.db.pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
        if expired {
            site.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if expired {
            assert!(!reply.status.is_success());
        } else {
            accepted(reply);
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='workspace_prepare'")
                .fetch_one(&site.world.db.pool)
                .await
                .unwrap();
        assert_eq!(count, if expired { 0 } else { 1 });
    }
}

#[tokio::test]
async fn queued_preparation_retains_its_original_scope_and_native_thread_authority() {
    for change in ["live", "thread", "environment", "expiry", "revoked"] {
        let site = fixture(false).await;
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![env(&site, "prepare", &["manage"]), thread(&site, "prepare")],
        )
        .await;
        let body = accepted(prepare(&site, &key.secret).await);
        let id: Id = body["id"].as_str().unwrap().parse().unwrap();
        match change {
            "thread" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(key.id)
                    .bind(json!({"rules":[env(&site,"prepare",&["manage"])]}))
                    .execute(&site.world.db.pool)
                    .await
                    .unwrap();
            }
            "environment" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(key.id)
                    .bind(json!({"rules":[thread(&site,"prepare")]}))
                    .execute(&site.world.db.pool)
                    .await
                    .unwrap();
            }
            "expiry" => site.world.advance(chrono::Duration::hours(2)),
            "revoked" => keys::revoke(&site, &key).await,
            _ => {}
        }
        site.app().app.engine.start();
        let row = ended(&site, id).await;
        if change == "live" {
            assert_eq!(row.state, "succeeded");
            assert_eq!(site.sent(CommandKind::FsStat), 1);
        } else {
            assert_eq!(row.state, "failed");
            assert_eq!(site.sent(CommandKind::FsStat), 0);
        }
        site.app().app.engine.stop();
    }
}

#[tokio::test]
async fn accepted_repository_preparation_reconciles_its_exact_frozen_result_after_expiry() {
    let mut site = fixture(true).await;
    let native = Native::new(&site, "git.prepare");
    site.power_on_with(native.clone());
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![env(&site, "prepare", &["manage"]), thread(&site, "prepare")],
    )
    .await;
    let body = accepted(prepare(&site, &key.secret).await);
    let id: Id = body["id"].as_str().unwrap().parse().unwrap();
    site.app().app.engine.start();
    tokio::time::timeout(WAIT, native.entered.cancelled())
        .await
        .unwrap();
    site.world.advance(chrono::Duration::hours(2));
    native.release.cancel();
    assert_eq!(ended(&site, id).await.state, "succeeded");
    let current = threads::thread(&mut *site.world.conn().await, site.thread)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.source.as_ref().unwrap()["info"]["state"], "ready");
    assert_eq!(
        current.source.as_ref().unwrap()["info"]["repository_url"],
        "https://code.example.test/frozen/source.git"
    );
    assert_eq!(native.calls.load(Ordering::SeqCst), 1);
    site.app().app.engine.stop();
}

#[tokio::test]
async fn a_changed_native_recipe_cannot_borrow_the_original_request_or_publish_another_source() {
    let site = fixture(false).await;
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![env(&site, "prepare", &["manage"]), thread(&site, "prepare")],
    )
    .await;
    let body = accepted(prepare(&site, &key.secret).await);
    let id: Id = body["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,'{action,directory}','\"somewhere-else\"'::jsonb) WHERE id=$1").bind(id).execute(&site.world.db.pool).await.unwrap();
    site.app().app.engine.start();
    assert_eq!(ended(&site, id).await.state, "failed");
    assert_eq!(site.sent(CommandKind::FsStat), 0);
    assert!(!site.disk.exists("somewhere-else"));
    site.app().app.engine.stop();
}

#[tokio::test]
async fn generated_preparation_ack_survives_activity_time_expiry_without_returning_old_job_content()
{
    let site = fixture(false).await;
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![env(&site, "prepare", &["manage"]), thread(&site, "prepare")],
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
    let environment = site.environment;
    let thread = site.thread;
    let request = tokio::spawn(async move {
        let call = app
            .call(EnvironmentSourcePrepare, &[&environment])
            .token(&key.secret)
            .body(&EnvironmentSourcePrepareBody {
                thread_id: thread,
                source: None,
            });
        call.send_bytes().await
    });
    wait_sql(&site, pid, "%INSERT INTO api_token_activity%", &request).await;
    site.world.advance(chrono::Duration::hours(2));
    blocker.commit().await.unwrap();
    let body = accepted(tokio::time::timeout(WAIT, request).await.unwrap().unwrap());
    assert_eq!(body["node_id"], json!(site.thread));
    assert_eq!(body["environment_id"], json!(site.environment));
    assert!(body["result"].is_null());
    assert!(body["log"].is_null());
    assert!(body["error"].is_null());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='workspace_prepare'")
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(site.sent(CommandKind::FsStat), 0);
}
