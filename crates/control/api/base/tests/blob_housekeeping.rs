//! What the store of content lets go of, and when: a blob nobody pins is kept for its grace and then goes, content a cached version of a file names stays, a cache over its quota is trimmed from the content asked for least lately, and an upload that was given up goes with the pieces it left.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;
mod workspace;

use chrono::Duration;
use orch_api_contract::threads::{UploadSessionCreate, UploadSessionCreateBody};
use orch_api_core::cache::{collect_garbage, evict_caches};
use orch_db::queries::storage::{self, CachedFile};
use orch_types::{Hash, Id};
use serde_json::json;
use workspace::{Depot, Site};

/// The hours a blob that nothing wants is kept: the shipped grace.
const GRACE_HOURS: i64 = 24;

async fn collect(site_app: &orch_api_core::state::App) -> orch_api_core::cache::Collected {
    collect_garbage(
        &site_app.db,
        &site_app.blobs,
        site_app.clock.as_ref(),
        &site_app.config,
    )
    .await
    .unwrap()
}

async fn held(app: &orch_api_core::state::App, hash: &Hash) -> (bool, bool) {
    let mut conn = app.db.acquire().await.unwrap();
    let row = storage::blob(&mut conn, hash).await.unwrap().is_some();
    let file = app.blobs.store().size(hash).await.unwrap().is_some();
    (row, file)
}

/// Content cached for a version of a file of the environment, as a fetch would have left it.
async fn cached(site: &Site, path: &str, bytes: &[u8], mtime_ns: i64) -> Hash {
    let app = &site.app().app;
    let meta = app
        .blobs
        .put_bytes(bytes::Bytes::copy_from_slice(bytes))
        .await
        .unwrap();
    let mut conn = app.db.acquire().await.unwrap();
    app.blobs
        .register(&mut conn, meta, app.clock.now())
        .await
        .unwrap();
    storage::record_cached(
        &mut conn,
        site.environment,
        &CachedFile {
            path: path.to_owned(),
            size: meta.size,
            mtime_ns,
            hash: meta.hash,
            cached_at: app.clock.now(),
        },
    )
    .await
    .unwrap();
    meta.hash
}

fn kilobyte(fill: u8) -> Vec<u8> {
    vec![fill; 1_000]
}

#[tokio::test]
async fn a_blob_nobody_pins_is_kept_for_its_grace_and_then_goes() {
    let depot = Depot::new().await;
    let app = depot.app();
    let hash = depot.store(b"content that nothing points at").await;

    assert_eq!(collect(app).await.blobs, 0, "it was only just stored");
    assert_eq!(held(app, &hash).await, (true, true));

    depot.world.advance(Duration::hours(GRACE_HOURS - 1));
    assert_eq!(collect(app).await.blobs, 0, "an hour of its grace is left");
    assert_eq!(held(app, &hash).await, (true, true));

    depot.world.advance(Duration::hours(2));
    let gone = collect(app).await;
    assert_eq!(gone.blobs, 1);
    assert_eq!(gone.bytes, 30);
    assert_eq!(
        held(app, &hash).await,
        (false, false),
        "both the row and the content go"
    );
}

#[tokio::test]
async fn a_blob_that_is_pinned_stays_however_old_and_is_cache_again_when_the_last_pin_goes() {
    let depot = Depot::new().await;
    let app = depot.app();
    let hash = depot.store(b"a snapshot's file").await;
    let mut conn = depot.world.conn().await;
    assert_eq!(app.blobs.pin(&mut conn, &[hash]).await.unwrap(), 0);
    assert_eq!(app.blobs.pin(&mut conn, &[hash]).await.unwrap(), 0);

    depot.world.advance(Duration::hours(GRACE_HOURS * 10));
    assert_eq!(collect(app).await.blobs, 0, "two things hold it");

    app.blobs
        .unpin(&mut conn, &[hash], depot.world.now())
        .await
        .unwrap();
    depot.world.advance(Duration::hours(GRACE_HOURS * 10));
    assert_eq!(collect(app).await.blobs, 0, "one thing still holds it");

    app.blobs
        .unpin(&mut conn, &[hash], depot.world.now())
        .await
        .unwrap();
    depot.world.advance(Duration::hours(GRACE_HOURS - 1));
    assert_eq!(
        collect(app).await.blobs,
        0,
        "its grace begins when the last pin goes, not when it was stored"
    );
    depot.world.advance(Duration::hours(2));
    assert_eq!(collect(app).await.blobs, 1);
}

#[tokio::test]
async fn pinning_something_the_store_never_had_is_told_to_the_one_who_pins() {
    let depot = Depot::new().await;
    let mut conn = depot.world.conn().await;
    let unknown = Hash::of(b"never stored");
    assert_eq!(
        depot.app().blobs.pin(&mut conn, &[unknown]).await.unwrap(),
        1,
        "one hash had no row to pin"
    );
}

#[tokio::test]
async fn content_that_was_asked_for_lately_is_kept() {
    let depot = Depot::new().await;
    let app = depot.app();
    let hash = depot.store(b"read now and then").await;
    depot.world.advance(Duration::hours(GRACE_HOURS - 4));
    let mut conn = depot.world.conn().await;
    storage::touch_blob(&mut conn, &hash, depot.world.now())
        .await
        .unwrap();
    depot.world.advance(Duration::hours(GRACE_HOURS - 4));
    assert_eq!(
        collect(app).await.blobs,
        0,
        "more than a day since it was stored, but less since it was read"
    );
    depot.world.advance(Duration::hours(5));
    assert_eq!(collect(app).await.blobs, 1);
}

#[tokio::test]
async fn content_that_a_cached_version_of_a_file_names_stays_though_nothing_pins_it() {
    let site = Site::new().await;
    let app = &site.app().app;
    let hash = cached(&site, "web/a.txt", b"the version a person read", 1_000_000).await;
    site.world.advance(Duration::hours(GRACE_HOURS * 5));
    assert_eq!(collect(app).await.blobs, 0);
    assert_eq!(held(app, &hash).await, (true, true));

    // Once the version is forgotten, its content has the grace like any other.
    let mut conn = site.world.conn().await;
    storage::forget_cached_under(&mut conn, site.environment, "web/a.txt")
        .await
        .unwrap();
    drop(conn);
    assert_eq!(collect(app).await.blobs, 1);
}

#[tokio::test]
async fn a_cache_over_its_quota_is_trimmed_from_the_content_asked_for_least_lately() {
    let site = Site::new().await;
    site.world
        .set("storage.env_cache_quota_bytes", json!(1_500))
        .await;
    let app = &site.app().app;
    let oldest = cached(&site, "web/old.txt", &kilobyte(1), 1_000_000).await;
    site.world.advance(Duration::hours(1));
    let middle = cached(&site, "web/middle.txt", &kilobyte(2), 2_000_000).await;
    site.world.advance(Duration::hours(1));
    let newest = cached(&site, "web/new.txt", &kilobyte(3), 3_000_000).await;

    let done = evict_caches(&app.db, app.clock.as_ref()).await.unwrap();
    assert_eq!((done.environments, done.versions), (1, 2));

    let mut conn = site.world.conn().await;
    let left = storage::cached_below(&mut conn, site.environment, "")
        .await
        .unwrap();
    assert_eq!(
        left.iter().map(|file| file.hash).collect::<Vec<_>>(),
        [newest],
        "the version asked for last is the one that is kept"
    );
    // The content of what was forgotten has its grace like any other.
    drop(conn);
    assert_eq!(held(app, &oldest).await, (true, true));
    site.world.advance(Duration::hours(GRACE_HOURS + 1));
    assert_eq!(collect(app).await.blobs, 2);
    assert_eq!(held(app, &oldest).await, (false, false));
    assert_eq!(held(app, &middle).await, (false, false));
    assert_eq!(held(app, &newest).await, (true, true));
}

#[tokio::test]
async fn a_cache_within_its_quota_is_left_alone() {
    let site = Site::new().await;
    site.world
        .set("storage.env_cache_quota_bytes", json!(5_000))
        .await;
    let app = &site.app().app;
    cached(&site, "web/a.txt", &kilobyte(1), 1).await;
    cached(&site, "web/b.txt", &kilobyte(2), 2).await;
    let done = evict_caches(&app.db, app.clock.as_ref()).await.unwrap();
    assert_eq!((done.environments, done.versions), (0, 0));
}

#[tokio::test]
async fn content_that_something_pins_is_never_evicted_from_the_cache() {
    let site = Site::new().await;
    site.world
        .set("storage.env_cache_quota_bytes", json!(1_500))
        .await;
    let app = &site.app().app;
    let pinned = cached(&site, "web/pinned.txt", &kilobyte(1), 1_000_000).await;
    site.world.advance(Duration::hours(1));
    let other = cached(&site, "web/other.txt", &kilobyte(2), 2_000_000).await;
    let mut conn = site.world.conn().await;
    app.blobs.pin(&mut conn, &[pinned]).await.unwrap();
    drop(conn);

    let done = evict_caches(&app.db, app.clock.as_ref()).await.unwrap();
    assert_eq!(done.versions, 1);
    let mut conn = site.world.conn().await;
    let left = storage::cached_below(&mut conn, site.environment, "")
        .await
        .unwrap();
    assert_eq!(
        left.iter().map(|file| file.hash).collect::<Vec<_>>(),
        [pinned],
        "the content that is pinned stays though it was asked for first"
    );
    assert!(held(app, &other).await.0);
}

#[tokio::test]
async fn an_upload_that_no_piece_came_to_for_a_day_goes_with_the_pieces_it_left() {
    let depot = Depot::new().await;
    let app = depot.app();
    let command = depot.uploading().await;
    let bytes = vec![5_u8; 3_000];
    let hash = Hash::of(&bytes);
    depot
        .content
        .put(depot.machine, command, &hash, 0, 3_000, &bytes[..1_000])
        .await;
    let part = app.blobs.part_of(&hash);
    assert!(
        tokio::fs::try_exists(&part).await.unwrap(),
        "the pieces wait in a file"
    );

    depot.world.advance(Duration::hours(23));
    assert_eq!(collect(app).await.uploads, 0);
    depot.world.advance(Duration::hours(2));
    assert_eq!(collect(app).await.uploads, 1);
    assert!(
        !tokio::fs::try_exists(&part).await.unwrap(),
        "the file is removed"
    );
    let mut conn = depot.world.conn().await;
    assert!(storage::upload(&mut conn, &hash).await.unwrap().is_none());
}

#[tokio::test]
async fn a_browser_upload_that_ran_out_goes_with_its_pieces() {
    let site = Site::new().await;
    let app = &site.app().app;
    let begun = site
        .app()
        .call(UploadSessionCreate, &[])
        .token(&site.ada.token)
        .body(&UploadSessionCreateBody {
            size: 2_000,
            name: "report.pdf".to_owned(),
            media_type: None,
        })
        .send()
        .await
        .ok();
    let session: Id = begun.upload_id;
    let part = app.blobs.part_of_session(session);
    tokio::fs::create_dir_all(part.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&part, vec![1_u8; 500]).await.unwrap();

    site.world.advance(Duration::hours(23));
    assert_eq!(
        collect(app).await.uploads,
        0,
        "the session is good for a day"
    );
    site.world.advance(Duration::hours(2));
    assert_eq!(collect(app).await.uploads, 1);
    assert!(!tokio::fs::try_exists(&part).await.unwrap());
    let mut conn = site.world.conn().await;
    assert!(
        storage::lock_session(&mut conn, session)
            .await
            .unwrap()
            .is_none()
    );
}
