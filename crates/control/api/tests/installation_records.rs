//! Management's bounded read-only release check and protected manifest-led backup facts, with a held clock and a real database.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod admin_support;
mod places;
mod support;

use axum::{Router, body::Body, response::Response, routing::get};
use chrono::Duration;
use orch_api_contract::admin::{
    AdminBackupRestoreList, AdminInstallationRecordsGet, AdminReleaseCheck, BackupCause,
};
use orch_api_contract::common::ErrorCode;
use orch_config::ServerConfig;
use orch_types::Time;
use orch_worker_core::{backups, releases};
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use support::World;

fn manifest(version: &str) -> String {
    json!({"schema_version":1,"project":"orch","tag":format!("v{version}"),"version":version,
        "commit":"a".repeat(40),"link_protocol":{"current":1,"minimum":1},"connector_minimum":"0.1.0",
        "assets":[{"name":"orch-server-linux-x86_64.zip","download_url":"orch-server-linux-x86_64.zip",
            "size":1,"blake3":"b".repeat(64)}],"images":[],"harness_builds":[]}).to_string()
}

struct Site {
    config: orch_config::server::Release,
    reply: Arc<Mutex<(u16, String, u64)>>,
    calls: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Site {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Site {
    async fn new() -> Self {
        let reply = Arc::new(Mutex::new((200, manifest("1.2.3"), 0)));
        let calls = Arc::new(AtomicUsize::new(0));
        let (answers, requests) = (reply.clone(), calls.clone());
        let router = Router::new().route(
            "/latest/download/version.json",
            get(move |request: axum::extract::Request| {
                let (answers, requests) = (answers.clone(), requests.clone());
                async move {
                    requests.fetch_add(1, Ordering::SeqCst);
                    for name in [
                        "authorization",
                        "cookie",
                        "x-orch-machine",
                        "x-orch-client-certificate",
                    ] {
                        assert!(
                            !request.headers().contains_key(name),
                            "a read-only release check sends no identity"
                        );
                    }
                    let (status, body, delay) = answers.lock().unwrap().clone();
                    if delay > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    }
                    Response::builder()
                        .status(status)
                        .header("location", "http://127.0.0.1:1/unconfigured")
                        .header("last-modified", "Fri, 01 Jan 2100 00:00:00 GMT")
                        .body(Body::from(body))
                        .unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            config: orch_config::server::Release {
                url,
                request_timeout_secs: 1,
                manifest_max_bytes: 1_048_576,
            },
            reply,
            calls,
            task,
        }
    }
}

#[tokio::test]
async fn checks_ship_off_manual_checks_name_the_offer_once_and_enabled_checks_wait_until_due() {
    let site = Site::new().await;
    let mut config = ServerConfig::default();
    config.release = site.config.clone();
    let world = World::with(config).await;
    let people = admin_support::cast(&world).await;
    let app = &world.app.app;
    assert!(
        releases::check(
            &app.db,
            &*app.clock,
            &site.config,
            app.build.version,
            releases::Wanted::WhenDue
        )
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(site.calls.load(Ordering::SeqCst), 0);
    for _ in 0..2 {
        let check = world
            .app
            .call(AdminReleaseCheck, &[])
            .token(&people.root.token)
            .send()
            .await
            .ok();
        assert_eq!(check.offered_version.as_deref(), Some("1.2.3"));
        assert_eq!(check.command.as_deref(), Some("./update.sh v1.2.3"));
        assert_eq!(check.newer, Some(true));
        assert_eq!(
            check.notes_url.as_deref(),
            Some(format!("{}/tag/v1.2.3", site.config.url).as_str())
        );
    }
    let notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE data->>'reason'='release_available'",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        notices, 1,
        "a repeated successful lookup must not announce that version twice"
    );
    world.set("release.check_days", json!(1)).await;
    world.advance(Duration::days(1) - Duration::seconds(1));
    assert!(
        releases::check(
            &app.db,
            &*app.clock,
            &site.config,
            app.build.version,
            releases::Wanted::WhenDue
        )
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(
        site.calls.load(Ordering::SeqCst),
        1,
        "manual requests share the hourly cache"
    );
    world.advance(Duration::seconds(1));
    assert!(
        releases::check(
            &app.db,
            &*app.clock,
            &site.config,
            app.build.version,
            releases::Wanted::WhenDue
        )
        .await
        .unwrap()
        .is_some()
    );
    assert_eq!(site.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn installation_reads_finish_with_one_pool_slot_without_holding_a_guard_over_external_work() {
    let site = Site::new().await;
    let records = tempfile::tempdir().unwrap();
    let mut config = ServerConfig::default();
    config.release = site.config.clone();
    config.installation.records_dir = records.path().to_owned();
    let world = World::with(config).await;
    let people = admin_support::cast(&world).await;
    // Reserve bare connections, never transactions. Each request must reuse the sole free slot
    // in successive short phases; a nested pooled acquire would exhaust this pool.
    let mut reserved = Vec::new();
    for _ in 1..world.db.pool.options().get_max_connections() {
        reserved.push(world.db.pool.acquire().await.unwrap());
    }
    // This is a deadlock regression bound, not a performance bar: it covers the source's
    // one-second request limit and database phases while catching the old ten-second pool wait.
    let bound = std::time::Duration::from_secs(5);
    let checked = tokio::time::timeout(
        bound,
        world
            .app
            .call(AdminReleaseCheck, &[])
            .token(&people.root.token)
            .send(),
    )
    .await
    .expect("the release lookup held its request transaction while acquiring another connection");
    assert_eq!(checked.status, 200);
    let shown = tokio::time::timeout(
        bound,
        world
            .app
            .call(AdminInstallationRecordsGet, &[])
            .token(&people.root.token)
            .send(),
    )
    .await
    .expect("the records read held a connection or transaction over its external reads");
    assert_eq!(shown.status, 200);
    let inventory = tokio::time::timeout(
        bound,
        world
            .app
            .call(AdminBackupRestoreList, &[])
            .token(&people.root.token)
            .send(),
    )
    .await
    .expect("the manifest read held its request transaction over file work");
    assert_eq!(inventory.status, 200);
    drop(reserved);
}

#[tokio::test]
async fn system_page_and_retry_share_an_hourly_cache_even_when_the_release_source_fails() {
    let site = Site::new().await;
    *site.reply.lock().unwrap() = (503, String::new(), 0);
    let mut config = ServerConfig::default();
    config.release = site.config.clone();
    let world = World::with(config).await;
    let people = admin_support::cast(&world).await;
    let first = world
        .app
        .call(AdminInstallationRecordsGet, &[])
        .token(&people.root.token)
        .send()
        .await
        .ok()
        .release
        .unwrap();
    assert!(first.error.is_some());
    assert_eq!(site.calls.load(Ordering::SeqCst), 1);
    world.advance(Duration::seconds(3599));
    let cached = world
        .app
        .call(AdminReleaseCheck, &[])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    assert_eq!(cached, first);
    assert_eq!(site.calls.load(Ordering::SeqCst), 1);
    world.advance(Duration::seconds(1));
    world
        .app
        .call(AdminReleaseCheck, &[])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    assert_eq!(site.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn invalid_oversized_redirected_older_and_timed_out_answers_keep_the_dated_success() {
    let site = Site::new().await;
    let mut config = ServerConfig::default();
    config.release = site.config.clone();
    let world = World::with(config).await;
    let people = admin_support::cast(&world).await;
    let first = world
        .app
        .call(AdminReleaseCheck, &[])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    for reply in [
        (200, "not JSON".to_owned(), 0),
        (200, "x".repeat(1_048_577), 0),
        (302, String::new(), 0),
        (200, manifest("0.1.0"), 0),
        (200, manifest("2.0.0"), 2000),
    ] {
        *site.reply.lock().unwrap() = reply;
        world.advance(Duration::hours(1));
        let failed = world
            .app
            .call(AdminReleaseCheck, &[])
            .token(&people.root.token)
            .send()
            .await
            .ok();
        assert!(failed.error.is_some());
        assert_eq!(failed.checked_at, first.checked_at);
        assert_eq!(failed.offered_version, first.offered_version);
        let shown = world
            .app
            .call(AdminInstallationRecordsGet, &[])
            .token(&people.root.token)
            .send()
            .await
            .ok();
        assert_eq!(shown.release.unwrap(), failed);
        assert!(
            shown.backup.is_none(),
            "an HTTP date cannot claim that a local backup ran"
        );
    }
}

#[tokio::test]
async fn only_browser_session_system_admins_read_or_request_installation_facts() {
    let site = Site::new().await;
    *site.reply.lock().unwrap() = (503, String::new(), 0);
    let mut config = ServerConfig::default();
    config.release = site.config.clone();
    let world = World::with(config).await;
    let people = admin_support::cast(&world).await;
    for who in [&people.boss, &people.ada] {
        admin_support::assert_no_system_admin(
            &admin_support::refused_to(&world, AdminReleaseCheck, &[], who, None, "").await,
            "check",
        );
        admin_support::assert_no_system_admin(
            &admin_support::refused_to(&world, AdminInstallationRecordsGet, &[], who, None, "")
                .await,
            "records",
        );
        admin_support::assert_no_system_admin(
            &admin_support::refused_to(&world, AdminBackupRestoreList, &[], who, None, "").await,
            "restore list",
        );
    }
    let token = admin_support::api_token(&world, &people.root).await;
    assert_eq!(
        world
            .app
            .call(AdminReleaseCheck, &[])
            .token(&token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthSessionRequired
    );
    assert_eq!(
        world
            .app
            .call(AdminBackupRestoreList, &[])
            .token(&token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthSessionRequired
    );
    let check = world
        .app
        .call(AdminReleaseCheck, &[])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    assert!(check.error.unwrap().contains("release source"));
}

fn pair(
    dir: &std::path::Path,
    system: &str,
    at: Time,
    cause: BackupCause,
) -> (std::path::PathBuf, std::path::PathBuf) {
    // Metadata fixture only: the released backup smoke, not this test, proves pg_dump's custom format.
    let dump = dir.join("database.dump");
    let file = dir.join("database.manifest.json");
    std::fs::write(&dump, b"a completed dump fixture").unwrap();
    let (bytes, hash) = backups::hash_file(&dump).unwrap();
    let manifest = backups::Manifest {
        format_version: 1,
        system_id: system.to_owned(),
        release: "v1.0.0".into(),
        schema: "1".into(),
        database_at: at,
        completed_at: at,
        cause,
        dump: backups::Dump {
            name: "database.dump".into(),
            bytes,
            hash,
        },
        included: json!({"database":{"count":1}}),
        excluded: json!({"blobs":{"included":false}}),
    };
    std::fs::write(&file, serde_json::to_vec(&manifest).unwrap()).unwrap();
    (dump, file)
}

#[tokio::test]
async fn the_restore_list_comes_from_the_verified_manifest_with_unknowns_and_without_inventing_host_copies()
 {
    let site = Site::new().await;
    let records = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let mut config = ServerConfig::default();
    config.installation.records_dir = records.path().to_owned();
    config.release = site.config.clone();
    let world = World::with(config).await;
    let people = admin_support::cast(&world).await;
    assert!(
        world
            .app
            .call(AdminBackupRestoreList, &[])
            .token(&people.root.token)
            .send()
            .await
            .ok()
            .is_none()
    );
    let system = world
        .app
        .app
        .link
        .certificates()
        .authority_certificate()
        .fingerprint();
    let at = Time::new(world.now());
    let (dump, file) = pair(files.path(), &system, at, BackupCause::Manual);
    let mut manifest: backups::Manifest =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let environment = orch_types::new_id();
    let host = orch_types::new_id();
    manifest.included = json!({"database":{"included":true,"count":1,"reason":"one exported snapshot",
        "location":"database.dump","location_status":"known"}});
    manifest.excluded = json!({
        "workspaces":{"included":false,"count":1,"reason":"host file backup required","entries":[{
            "id":environment,"host_id":host,"driver":"docker","storage":"team","location":"workspace-one",
            "location_status":"known","availability":"available","secret":"never-return-this-extra-property"}]},
        "browser_profiles":{"included":false,"count":null,"environments_requiring_inspection":4,
            "reason":"inspect homes and workspaces","location_status":"unknown"},
        "provider_states":{"included":false,"count":0,"reason":"no provider runtimes","entries":[]}
    });
    std::fs::write(&file, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let published = backups::publish_pair(records.path(), &dump, &file, &system).unwrap();
    let list = world
        .app
        .call(AdminBackupRestoreList, &[])
        .token(&people.root.token)
        .send()
        .await
        .ok()
        .unwrap();
    assert_eq!(list.backup.completed_at, at);
    assert_eq!(list.backup.dump_name, "database.dump");
    assert_eq!(list.manifest_name, published.manifest_file);
    assert_eq!(list.manifest_hash, published.manifest_hash);
    assert_eq!(list.included[0].category, "database");
    assert_eq!(list.included[0].count, Some(1));
    assert_eq!(
        list.included[0].locations[0].location.as_deref(),
        Some("database.dump")
    );
    let workspace = list
        .excluded
        .iter()
        .find(|group| group.category == "workspaces")
        .unwrap();
    assert_eq!(workspace.count, Some(1));
    assert_eq!(workspace.locations[0].object_id, Some(environment));
    assert_eq!(workspace.locations[0].host_id, Some(host));
    assert!(workspace.locations[0].location_known);
    assert_eq!(
        workspace.locations[0].copy_available, None,
        "a known path cannot claim a separate file backup exists"
    );
    let profiles = list
        .excluded
        .iter()
        .find(|group| group.category == "browser_profiles")
        .unwrap();
    assert_eq!(profiles.count, None);
    assert_eq!(profiles.environments_requiring_inspection, Some(4));
    assert!(
        profiles.locations.is_empty(),
        "an unknown inventory is not fabricated from current state"
    );
    assert_eq!(
        list.excluded
            .iter()
            .find(|group| group.category == "provider_states")
            .unwrap()
            .count,
        Some(0)
    );
    assert!(
        !serde_json::to_string(&list)
            .unwrap()
            .contains("never-return-this-extra-property")
    );
    let mut wrong = list.backup.clone();
    wrong.dump_name = "another.dump".to_owned();
    assert!(backups::inventory::read(records.path(), &system, Some(&wrong)).is_err());
    world.advance(Duration::days(1));
    backups::failed(
        records.path(),
        world.app.app.clock.time(),
        "a later backup failed",
    )
    .unwrap();
    let later = world
        .app
        .call(AdminBackupRestoreList, &[])
        .token(&people.root.token)
        .send()
        .await
        .ok()
        .unwrap();
    assert_eq!(later.backup.completed_at, at);
    assert!(later.backup.last_failure.is_some());
    assert_eq!(later.excluded, list.excluded);
    std::fs::write(records.path().join(&published.manifest_file), b"changed").unwrap();
    let refused = world
        .app
        .call(AdminBackupRestoreList, &[])
        .token(&people.root.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::SourceOffline);
    // A problem's detail is its code's catalogue sentence; what was wrong stays in the server's log.
    assert_eq!(
        refused.detail,
        orch_api_contract::messages::for_code(ErrorCode::SourceOffline)
    );
    let held = world
        .app
        .call(AdminInstallationRecordsGet, &[])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    assert_eq!(
        held.backup.unwrap().completed_at,
        at,
        "a failed inventory read cannot erase the completed backup"
    );
}

#[tokio::test]
async fn local_failures_and_bad_hashes_keep_the_last_success_and_its_warning_age_and_repair_takes_no_dump()
 {
    let site = Site::new().await;
    let records = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let mut config = ServerConfig::default();
    config.installation.records_dir = records.path().to_owned();
    config.release = site.config.clone();
    let world = World::with(config).await;
    let people = admin_support::cast(&world).await;
    let system = world
        .app
        .app
        .link
        .certificates()
        .authority_certificate()
        .fingerprint();
    let at = Time::new(world.now());
    let (dump, manifest) = pair(files.path(), &system, at, BackupCause::PreUpdate);
    let published = backups::publish_pair(records.path(), &dump, &manifest, &system).unwrap();
    let first = world
        .app
        .call(AdminInstallationRecordsGet, &[])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    assert!(!first.backup_warning);
    assert_eq!(first.backup.unwrap().cause, BackupCause::PreUpdate);
    world.advance(Duration::days(2));
    assert!(
        !world
            .app
            .call(AdminInstallationRecordsGet, &[])
            .token(&people.root.token)
            .send()
            .await
            .ok()
            .backup_warning
    );
    world.advance(Duration::seconds(1));
    backups::failed(
        records.path(),
        world.app.app.clock.time(),
        "the latest dump failed",
    )
    .unwrap();
    let failed = world
        .app
        .call(AdminInstallationRecordsGet, &[])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    assert!(failed.backup_warning);
    assert_eq!(failed.backup.as_ref().unwrap().completed_at, at);
    assert!(failed.backup.unwrap().last_failure.is_some());
    let before = std::fs::read(&dump).unwrap();
    backups::publish_pair(records.path(), &dump, &manifest, &system).unwrap();
    assert_eq!(
        std::fs::read(&dump).unwrap(),
        before,
        "metadata repair must not take or change a dump"
    );
    let repaired = world
        .app
        .call(AdminInstallationRecordsGet, &[])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    assert!(repaired.backup.unwrap().last_failure.is_none());
    std::fs::write(records.path().join(published.manifest_file), b"changed").unwrap();
    let bad = world
        .app
        .call(AdminInstallationRecordsGet, &[])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    assert!(bad.backup_error.unwrap().contains("hash"));
    assert_eq!(bad.backup.unwrap().completed_at, at);
}

#[tokio::test]
async fn another_installation_or_older_pair_cannot_advance_the_imported_success() {
    let records = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let world = World::new().await;
    let system = world
        .app
        .app
        .link
        .certificates()
        .authority_certificate()
        .fingerprint();
    let at = Time::new(world.now());
    let (dump, manifest) = pair(files.path(), &system, at, BackupCause::Manual);
    backups::publish_pair(records.path(), &dump, &manifest, &system).unwrap();
    backups::reconcile(&world.app.app.db, records.path(), &system, at)
        .await
        .unwrap();
    let (held, error) = backups::reconcile(
        &world.app.app.db,
        records.path(),
        "another-installation",
        at,
    )
    .await
    .unwrap();
    assert!(error.unwrap().contains("another installation"));
    assert_eq!(held.unwrap().completed_at, at);
    let (dump, manifest) = pair(
        files.path(),
        &system,
        Time::new(world.now() - Duration::days(1)),
        BackupCause::Manual,
    );
    assert!(backups::publish_pair(records.path(), &dump, &manifest, &system).is_err());
}
