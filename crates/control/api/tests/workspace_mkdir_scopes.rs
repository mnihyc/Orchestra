//! Real directory commands retain current source admission and one accepted creation identity.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#[path = "workspace_snapshots/scope_helpers.rs"]
mod keys;
mod places;
mod support;
mod workspace;

use http::StatusCode;
use orch_api_contract::{
    environment::ThroughThreadQuery,
    workspace::{EnvironmentFileMkdir, FilePathBody},
};
use orch_api_core::testing::{Machine, RawReply};
use orch_link_contract::commands::{Cmd, CommandError, CommandKind};
use orch_types::{Json, new_id};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use workspace::Site;

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const PATH: &str = "web/one-native-directory";
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
const OP: &str = "EnvironmentFileMkdir";
async fn fixture() -> Site {
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([42; 32]),
    );
    let mut site = Site::new().await;
    site.power_on();
    site
}
fn rule(site: &Site, operation: &str, actions: &[&str]) -> Json {
    keys::rule(operation, "environment", site.environment, "self", actions)
}
async fn mkdir(site: &Site, token: &str, header: &str, path: &str) -> RawReply {
    let request = site
        .app()
        .call(EnvironmentFileMkdir, &[&site.environment])
        .token(token)
        .header("Idempotency-Key", header)
        .query(&ThroughThreadQuery {
            thread: Some(site.thread),
        })
        .body(&FilePathBody { path: path.into() });
    request.send_bytes().await
}
fn begin(site: &Site, token: &str, header: &str) -> JoinHandle<RawReply> {
    let app = site.app().clone();
    let environment = site.environment;
    let thread = site.thread;
    let token = token.to_owned();
    let header = header.to_owned();
    tokio::spawn(async move {
        let request = app
            .call(EnvironmentFileMkdir, &[&environment])
            .token(&token)
            .header("Idempotency-Key", &header)
            .query(&ThroughThreadQuery {
                thread: Some(thread),
            })
            .body(&FilePathBody { path: PATH.into() });
        request.send_bytes().await
    })
}
fn shown(reply: RawReply) -> Proof<Json> {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    Ok(serde_json::from_slice(&reply.raw)?)
}
fn sparse(site: &Site, reply: RawReply) -> Proof {
    assert_eq!(
        shown(reply)?,
        json!({"projection":"receipt","environment_id":site.environment,"path":PATH})
    );
    Ok(())
}
async fn saved(site: &Site, key: &str) -> Proof<(Json, String)> {
    Ok(sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(site.ada.id())
    .bind(key)
    .fetch_one(&site.world.db.pool)
    .await?)
}
async fn commands(site: &Site) -> Proof<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='fs.mkdir'")
            .fetch_one(&site.world.db.pool)
            .await?,
    )
}
struct Held {
    disk: Arc<workspace::Disk>,
    entered: CancellationToken,
    release: CancellationToken,
    calls: AtomicUsize,
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
        if command.kind == "fs.mkdir" {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.cancel();
            self.release.cancelled().await;
        }
        self.disk.run(command).await
    }
}

#[tokio::test]
async fn a_directory_header_reads_one_confirmed_result_under_only_a_new_current_read() -> Proof {
    let site = fixture().await;
    let writer = keys::scoped(&site, site.ada.id(), vec![rule(&site, OP, &["write"])]).await;
    let header = new_id().to_string();
    sparse(&site, mkdir(&site, &writer.secret, &header, PATH).await)?;
    assert!(site.disk.exists(PATH));
    assert_eq!(commands(&site).await?, 1);
    let accepted = saved(&site, &header).await?;
    let record = orch_db::idempotency::creation::Record::read(accepted.0.clone())?;
    let outcome = record
        .outcome
        .ok_or("accepted native directory command missing")?;
    assert_eq!(outcome.data["environment_id"], json!(site.environment));
    assert_eq!(outcome.data["path"], PATH);
    assert!(outcome.data.get("native_command").is_some());
    assert!(outcome.data.get("result").is_none() && outcome.data.get("stat").is_none());
    keys::revoke(&site, &writer).await;
    sqlx::query("UPDATE environments SET remote_write=false WHERE id=$1")
        .bind(site.environment)
        .execute(&site.world.db.pool)
        .await?;
    let reader = keys::scoped(&site, site.ada.id(), vec![rule(&site, OP, &["read"])]).await;
    let native = shown(mkdir(&site, &reader.secret, &header, PATH).await)?;
    assert_eq!(native["path"], PATH);
    assert_eq!(native["type"], "dir");
    assert!(native.get("projection").is_none());
    let fresh = mkdir(&site, &reader.secret, &new_id().to_string(), PATH).await;
    assert_eq!(fresh.status, StatusCode::FORBIDDEN);
    let fresh: Json = serde_json::from_slice(&fresh.raw)?;
    assert_eq!(fresh["code"], "authz.denied");
    let neighbour = keys::scoped(
        &site,
        site.ada.id(),
        vec![rule(&site, "EnvironmentFileStat", &["read"])],
    )
    .await;
    sparse(&site, mkdir(&site, &neighbour.secret, &header, PATH).await)?;
    let mismatch = mkdir(&site, &reader.secret, &header, "web/a-different-directory").await;
    assert_eq!(mismatch.status, StatusCode::CONFLICT);
    let mismatch: Json = serde_json::from_slice(&mismatch.raw)?;
    assert_eq!(mismatch["code"], "conflict.idempotency_mismatch");
    assert!(!site.disk.exists("web/a-different-directory"));
    assert_eq!(saved(&site, &header).await?, accepted);
    assert_eq!(commands(&site).await?, 1);
    assert_eq!(site.sent(CommandKind::FsMkdir), 1);
    Ok(())
}

#[tokio::test]
async fn a_cancelled_directory_waiter_keeps_one_pending_identity_and_native_completion() -> Proof {
    let mut site = fixture().await;
    let held = Held::new(&site);
    site.power_on_with(held.clone());
    let writer = keys::scoped(&site, site.ada.id(), vec![rule(&site, OP, &["write"])]).await;
    let header = new_id().to_string();
    let request = begin(&site, &writer.secret, &header);
    tokio::time::timeout(WAIT, held.entered.cancelled()).await?;
    let accepted = saved(&site, &header).await?;
    request.abort();
    let _ = request.await;
    keys::revoke(&site, &writer).await;
    let reader = keys::scoped(&site, site.ada.id(), vec![rule(&site, OP, &["read"])]).await;
    sparse(
        &site,
        tokio::time::timeout(WAIT, mkdir(&site, &reader.secret, &header, PATH)).await?,
    )?;
    assert!(!site.disk.exists(PATH));
    assert_eq!(commands(&site).await?, 1);
    held.release.cancel();
    workspace::eventually("the actual accepted mkdir command completes", || async {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM commands WHERE kind='fs.mkdir' AND state='succeeded')",
        )
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap_or(false)
    })
    .await;
    let read = shown(mkdir(&site, &reader.secret, &header, PATH).await)?;
    assert_eq!(read["path"], PATH);
    assert_eq!(read["type"], "dir");
    assert!(site.disk.exists(PATH));
    assert_eq!(held.calls.load(Ordering::SeqCst), 1);
    assert_eq!(commands(&site).await?, 1);
    assert_eq!(saved(&site, &header).await?, accepted);
    Ok(())
}

#[tokio::test]
async fn directory_scope_and_lifetime_recheck_after_the_actual_native_environment_wait() -> Proof {
    for change in ["live", "write", "expiry", "generation", "policy"] {
        let site = fixture().await;
        let writer = keys::scoped(&site, site.ada.id(), vec![rule(&site, OP, &["write"])]).await;
        let header = new_id().to_string();
        let mut blocker = site.world.db.pool.begin().await?;
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
            .bind(site.environment)
            .execute(&mut *blocker)
            .await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let request = begin(&site, &writer.secret, &header);
        tokio::time::timeout(WAIT, async {
            loop {
                assert!(!request.is_finished(), "the request reaches its actual native directory source wait");
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM environments%FOR SHARE%')")
                    .bind(pid).fetch_one(&site.world.db.pool).await?;
                if waiting { break; }
                tokio::task::yield_now().await;
            }
            Ok::<_,sqlx::Error>(())
        }).await??;
        assert_eq!(commands(&site).await?, 0);
        assert!(!site.disk.exists(PATH));
        match change {
            "write" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(writer.id)
                    .bind(json!({"rules":[]}))
                    .execute(&site.world.db.pool)
                    .await?;
            }
            "expiry" => site.world.advance(chrono::Duration::hours(2)),
            "generation" => {
                sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                    .bind(site.environment)
                    .execute(&mut *blocker)
                    .await?;
            }
            "policy" => {
                sqlx::query("UPDATE environments SET remote_write=false WHERE id=$1")
                    .bind(site.environment)
                    .execute(&mut *blocker)
                    .await?;
            }
            _ => {}
        }
        blocker.commit().await?;
        let response = tokio::time::timeout(WAIT, request).await??;
        if change == "live" {
            sparse(&site, response)?;
            assert!(site.disk.exists(PATH));
            assert_eq!(commands(&site).await?, 1);
        } else {
            assert!(
                !response.status.is_success(),
                "{change}: {}",
                String::from_utf8_lossy(&response.raw)
            );
            assert!(!site.disk.exists(PATH));
            assert_eq!(commands(&site).await?, 0);
            let record =
                orch_db::idempotency::creation::Record::read(saved(&site, &header).await?.0)?;
            assert!(record.outcome.is_none());
        }
    }
    Ok(())
}

#[tokio::test]
async fn completed_directory_creation_keeps_its_receipt_after_its_original_read_closes() -> Proof {
    let mut site = fixture().await;
    let held = Held::new(&site);
    site.power_on_with(held.clone());
    let writer = keys::scoped(
        &site,
        site.ada.id(),
        vec![rule(&site, OP, &["read", "write"])],
    )
    .await;
    let header = new_id().to_string();
    let request = begin(&site, &writer.secret, &header);
    tokio::time::timeout(WAIT, held.entered.cancelled()).await?;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(writer.id)
        .bind(json!({"rules":[rule(&site, OP, &["write"])]}))
        .execute(&site.world.db.pool)
        .await?;
    held.release.cancel();
    sparse(&site, tokio::time::timeout(WAIT, request).await??)?;
    assert!(site.disk.exists(PATH));
    assert_eq!(held.calls.load(Ordering::SeqCst), 1);
    assert_eq!(commands(&site).await?, 1);
    Ok(())
}
