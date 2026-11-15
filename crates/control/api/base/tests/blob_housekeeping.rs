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
use orch_api_core::cache::{collect_garbage, evict_caches, purge_relayed_caches};
use orch_db::queries::storage::{self, CachedFile};
use orch_link_contract::commands::CommandKind;
use orch_settings::Target;
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

async fn purge(app: &orch_api_core::state::App) -> orch_api_core::cache::Purged {
    purge_relayed_caches(&app.db, &app.blobs, app.clock.as_ref())
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
        .register(&mut conn, &meta, app.clock.now())
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
            to: None,
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

// ---------------------------------------------------------------- content a transfer let go of

/// Bytes brought solely for transit, before any independent owner adopts them.
async fn transit(depot: &Depot, bytes: &[u8]) -> Hash {
    let publication = depot
        .app()
        .blobs
        .put_bytes(bytes::Bytes::copy_from_slice(bytes))
        .await
        .unwrap();
    let mut conn = depot.world.conn().await;
    storage::register_blob(
        &mut conn,
        &publication.hash,
        publication.size,
        depot.world.now(),
    )
    .await
    .unwrap();
    publication.hash
}

/// Content a transfer between machines brought into the store, as its job holds it while it runs.
async fn brought(depot: &Depot, bytes: &[u8]) -> Hash {
    let hash = transit(depot, bytes).await;
    let mut conn = depot.world.conn().await;
    assert_eq!(
        storage::pin_transfer_blobs(&mut conn, &[hash])
            .await
            .unwrap(),
        [hash]
    );
    hash
}

#[tokio::test]
async fn content_a_transfer_let_go_of_goes_at_once_and_not_after_the_grace() {
    let depot = Depot::new().await;
    let app = depot.app();
    let hash = brought(&depot, b"a file passing from one machine to another").await;
    depot.world.advance(Duration::hours(GRACE_HOURS * 10));
    assert_eq!(collect(app).await.blobs, 0, "the transfer holds it");

    let at = depot.world.now();
    let mut conn = depot.world.conn().await;
    let freed = storage::release_transfer_blobs(&mut conn, &[hash], at)
        .await
        .unwrap();
    assert_eq!(freed, [hash], "nothing else held it");
    drop(conn);
    assert_eq!(
        held(app, &hash).await,
        (true, true),
        "letting go only frees it"
    );
    app.blobs.forget_after_transfer(&app.db, &freed, at).await;
    assert_eq!(
        held(app, &hash).await,
        (false, false),
        "the row and the file go with the transfer, not a day after it"
    );
}

#[tokio::test]
async fn content_a_transfer_let_go_of_stays_while_anything_else_holds_it() {
    let site = Site::new().await;
    let app = &site.app().app;
    let at = site.world.now();

    // Another pin: a snapshot, an attachment, a second transfer.
    let pinned = cached(&site, "web/a.txt", b"a file a snapshot also holds", 1).await;
    let mut conn = site.world.conn().await;
    storage::forget_cached_under(&mut conn, site.environment, "web/a.txt")
        .await
        .unwrap();
    storage::pin_transfer_blobs(&mut conn, &[pinned])
        .await
        .unwrap();
    storage::pin_transfer_blobs(&mut conn, &[pinned])
        .await
        .unwrap();
    let freed = storage::release_transfer_blobs(&mut conn, &[pinned], at)
        .await
        .unwrap();
    assert!(freed.is_empty(), "one pin is left, so nothing is freed");
    app.blobs
        .forget_after_transfer(&app.db, &[pinned], at)
        .await;
    assert_eq!(held(app, &pinned).await, (true, true));

    // A cached version of a file names it.
    let named = cached(&site, "web/b.txt", b"a file a person read before", 2).await;
    storage::pin_transfer_blobs(&mut conn, &[named])
        .await
        .unwrap();
    let freed = storage::release_transfer_blobs(&mut conn, &[named], at)
        .await
        .unwrap();
    assert_eq!(freed, [named], "no pin is left");
    drop(conn);
    app.blobs.forget_after_transfer(&app.db, &freed, at).await;
    assert_eq!(
        held(app, &named).await,
        (true, true),
        "a cached version keeps the content as it does against the collection"
    );
}

#[tokio::test]
async fn content_asked_for_after_a_transfer_let_go_of_it_is_not_taken_from_under_the_asker() {
    let depot = Depot::new().await;
    let app = depot.app();
    let hash = brought(&depot, b"a file another transfer also brought").await;
    let at = depot.world.now();
    let mut conn = depot.world.conn().await;
    let freed = storage::release_transfer_blobs(&mut conn, &[hash], at)
        .await
        .unwrap();
    assert_eq!(freed, [hash]);
    // Another transfer registers or reads the same content before the first one settles.
    depot.world.advance(Duration::seconds(1));
    storage::touch_blob(&mut conn, &hash, depot.world.now())
        .await
        .unwrap();
    drop(conn);
    app.blobs.forget_after_transfer(&app.db, &freed, at).await;
    assert_eq!(held(app, &hash).await, (true, true));
}

#[tokio::test]
async fn a_pin_is_recorded_only_for_content_that_had_a_row_to_pin() {
    let depot = Depot::new().await;
    let known = depot.store(b"stored").await;
    let unknown = Hash::of(b"never stored");
    let mut conn = depot.world.conn().await;
    assert_eq!(
        storage::pin_transfer_blobs(&mut conn, &[known, unknown])
            .await
            .unwrap(),
        [known],
        "a transfer lets go only of what it pinned"
    );
}

#[tokio::test]
async fn content_that_a_transfer_ended_with_is_removed_unless_something_holds_it() {
    let depot = Depot::new().await;
    let app = depot.app();
    let read = transit(&depot, b"an archive a machine has read").await;
    let held_on = transit(&depot, b"an archive a snapshot also holds").await;
    let mut conn = depot.world.conn().await;
    app.blobs.pin(&mut conn, &[held_on]).await.unwrap();
    drop(conn);
    app.blobs
        .forget_after_transfer(&app.db, &[read, held_on], depot.world.now())
        .await;
    assert_eq!(held(app, &read).await, (false, false));
    assert_eq!(held(app, &held_on).await, (true, true));
}

// ---------------------------------------------------------------- caches of places that relay

#[tokio::test]
async fn a_place_that_relays_forgets_its_cached_versions_and_their_unowned_content() {
    let site = Site::new().await;
    let app = &site.app().app;
    let first = cached(&site, "web/a.txt", b"the version a person read", 1).await;
    let second = cached(&site, "web/b.txt", b"another version", 2).await;

    let done = purge(app).await;
    assert_eq!((done.environments, done.versions), (1, 2));
    let mut conn = site.world.conn().await;
    assert!(
        storage::cached_below(&mut conn, site.environment, "")
            .await
            .unwrap()
            .is_empty(),
        "nothing is cached any more"
    );
    drop(conn);
    assert_eq!(
        held(app, &first).await,
        (false, false),
        "the content goes with its cached version, not a day after it"
    );
    assert_eq!(held(app, &second).await, (false, false));
    let again = purge(app).await;
    assert_eq!(
        (again.environments, again.versions),
        (0, 0),
        "and it is done"
    );
}

#[tokio::test]
async fn content_something_else_holds_stays_though_its_cached_version_is_forgotten() {
    let site = Site::new().await;
    let app = &site.app().app;
    let held_on = cached(&site, "web/a.txt", b"a file a snapshot also holds", 1).await;
    let mut conn = site.world.conn().await;
    app.blobs.pin(&mut conn, &[held_on]).await.unwrap();
    drop(conn);

    assert_eq!(purge(app).await.versions, 1);
    assert_eq!(
        held(app, &held_on).await,
        (true, true),
        "it is no longer a cached version, and is still a snapshot's file"
    );
}

#[tokio::test]
async fn a_place_that_chose_to_cache_keeps_its_cache() {
    let site = Site::new().await;
    let app = &site.app().app;
    let kept = cached(&site, "web/a.txt", b"the version a person read", 1).await;
    site.world
        .set_at(
            Target::Node(site.org),
            "storage.relay_transfers",
            json!(false),
        )
        .await;
    site.world.advance(Duration::hours(GRACE_HOURS * 3));

    let done = purge(app).await;
    assert_eq!((done.environments, done.versions), (0, 0));
    assert_eq!(held(app, &kept).await, (true, true));

    // The system switches to caching, too, and nothing changes; once the org relays again it is purged.
    site.world
        .set("storage.relay_transfers", json!(false))
        .await;
    assert_eq!(purge(app).await.versions, 0);
    site.world
        .set_at(
            Target::Node(site.org),
            "storage.relay_transfers",
            json!(true),
        )
        .await;
    assert_eq!(purge(app).await.versions, 1);
    assert_eq!(held(app, &kept).await, (false, false));
}

// ---------------------------------------------------------------- two transfers that carry the same content

/// A command of a job, as the job issues it to the machine: its body names the job.
async fn job_command(depot: &Depot, job: Id) -> Id {
    let id = depot
        .command(
            depot.machine,
            CommandKind::FsSnapshot,
            &json!({ "environment_id": orch_types::new_id(), "job_id": job,
                     "selection": { "include": [], "exclude": [] } }),
        )
        .await;
    sqlx::query("UPDATE commands SET content_transit=true WHERE id=$1")
        .bind(id)
        .execute(&mut *depot.world.conn().await)
        .await
        .unwrap();
    id
}

/// The first transfer lets go of what it held and removes what nothing else holds, as its end does.
async fn first_transfer_ends(depot: &Depot, shared: &Hash) {
    let app = depot.app();
    let at = depot.world.now();
    let mut conn = depot.world.conn().await;
    let freed = storage::release_transfer_blobs(&mut conn, &[*shared], at)
        .await
        .unwrap();
    drop(conn);
    app.blobs.forget_after_transfer(&app.db, &freed, at).await;
}

#[tokio::test]
async fn content_the_machine_of_a_second_transfer_found_held_stays_when_the_first_transfer_ends() {
    let depot = Depot::new().await;
    let app = depot.app();
    // The first transfer brought the file and holds it.
    let shared = brought(&depot, b"a file that two transfers carry").await;
    // The machine of a second transfer asks which of its files the system lacks; this one it does not.
    let command = job_command(&depot, orch_types::new_id()).await;
    let answer = depot
        .content
        .missing(depot.machine, command, &[shared])
        .await;
    assert_eq!(answer.json()["missing"], json!([]), "{answer:?}");

    // The first transfer ends before the second has taken a pin of its own.
    first_transfer_ends(&depot, &shared).await;
    assert_eq!(
        held(app, &shared).await,
        (true, true),
        "what the second transfer was told the system holds is not taken away"
    );
}

#[tokio::test]
async fn content_the_machine_of_a_second_transfer_uploaded_stays_when_the_first_transfer_ends() {
    let depot = Depot::new().await;
    let app = depot.app();
    let bytes = b"a file that two transfers carry, uploaded by the second as well";
    let shared = brought(&depot, bytes).await;
    // The second machine did not hear that the system holds it, and sends it: the system needs no more.
    let command = job_command(&depot, orch_types::new_id()).await;
    assert_eq!(
        depot
            .content
            .upload(depot.machine, command, bytes, 20)
            .await,
        shared
    );

    first_transfer_ends(&depot, &shared).await;
    assert_eq!(held(app, &shared).await, (true, true));
}

#[tokio::test]
async fn the_end_of_a_job_lets_go_of_what_its_commands_held_and_removes_what_nothing_else_holds() {
    let depot = Depot::new().await;
    let app = depot.app();
    let job = orch_types::new_id();
    let command = job_command(&depot, job).await;
    let hash = depot
        .content
        .upload(depot.machine, command, b"content a transfer uploaded", 10)
        .await;
    let mut conn = depot.world.conn().await;
    assert_eq!(
        storage::blob(&mut conn, &hash)
            .await
            .unwrap()
            .map(|row| row.pin_count),
        Some(1),
        "held for the job from the upload"
    );

    let at = depot.world.now();
    let (released, freed) = storage::release_job_command_pins(&mut conn, job, at)
        .await
        .unwrap();
    assert_eq!((released, freed.clone()), (1, vec![hash]));
    drop(conn);
    app.blobs.forget_after_transfer(&app.db, &freed, at).await;
    assert_eq!(held(app, &hash).await, (false, false));
    let mut conn = depot.world.conn().await;
    assert_eq!(
        storage::release_job_command_pins(&mut conn, job, at)
            .await
            .unwrap()
            .0,
        0,
        "the second release finds nothing, so a pin is never let go of twice"
    );
}

#[tokio::test]
async fn what_a_job_held_and_its_end_did_not_let_go_of_is_let_go_by_the_eviction_job_and_keeps_its_grace()
 {
    let depot = Depot::new().await;
    let app = depot.app();
    // A job that is not there, as one the retention removed after its process ended before the release.
    let command = job_command(&depot, orch_types::new_id()).await;
    let hash = depot
        .content
        .upload(depot.machine, command, b"content whose job is gone", 10)
        .await;
    purge(app).await;
    let mut conn = depot.world.conn().await;
    assert_eq!(
        storage::blob(&mut conn, &hash)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        0
    );
    drop(conn);
    assert_eq!(
        held(app, &hash).await,
        (true, true),
        "it has the grace any content has, and is not removed by this"
    );
}

#[tokio::test]
async fn an_owned_output_keeps_shared_bytes_before_its_reference_event_has_arrived() {
    let depot = Depot::new().await;
    let bytes = b"the same file is also an output being recorded";
    let hash = brought(&depot, bytes).await;
    let mut conn = depot.world.conn().await;
    storage::register_owned_blob(
        &mut conn,
        &hash,
        i64::try_from(bytes.len()).unwrap(),
        depot.world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    first_transfer_ends(&depot, &hash).await;
    assert_eq!(
        held(depot.app(), &hash).await,
        (true, true),
        "a recorded output owns its upload before its later event takes the ordinary pin"
    );
}

#[tokio::test]
async fn a_browser_read_without_a_job_keeps_its_transfer_content_until_its_actual_body_is_dropped()
{
    let depot = Depot::new().await;
    let app = depot.app();
    let shared = brought(&depot, b"recorded bytes still being read by a browser").await;
    let command = orch_types::new_id();
    let reading = app
        .blobs
        .command_transfer(
            app.db.clone(),
            std::sync::Arc::clone(&app.clock),
            app.link.dispatcher(),
            command,
            orch_types::Time::new(depot.world.now() + Duration::hours(6)),
        )
        .await
        .unwrap();
    let mut tx = app.db.begin().await.unwrap();
    storage::pin_for_command(&mut tx, command, None, &[shared], depot.world.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    first_transfer_ends(&depot, &shared).await;
    assert!(
        app.blobs.store().size(&shared).await.unwrap().is_some(),
        "another transfer cannot collect this reader's bytes"
    );
    let body = reading
        .response(axum::response::Response::new(axum::body::Body::from(
            "one streamed body",
        )))
        .into_body();
    let bytes = axum::body::to_bytes(body, 1024).await.unwrap();
    assert_eq!(bytes.as_ref(), b"one streamed body");
    workspace::eventually(
        "the dropped browser read releases its own content",
        || async { app.blobs.store().size(&shared).await.unwrap().is_none() },
    )
    .await;
}

#[tokio::test]
async fn an_accepted_memory_call_keeps_its_fence_in_its_transaction_and_an_old_process_cannot_reopen_it()
 {
    let depot = Depot::new().await;
    let app = depot.app();
    let id = orch_types::new_id();
    let until = orch_types::Time::new(depot.world.now() + Duration::hours(6));
    let held = app.blobs.claim_command_transfer(
        app.db.clone(),
        std::sync::Arc::clone(&app.clock),
        app.link.dispatcher(),
        id,
        until,
    );
    let mut tx = app.db.begin().await.unwrap();
    held.begin_in(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    app.blobs
        .reap_command_transfers(&app.db, app.clock.as_ref(), &app.link.dispatcher())
        .await
        .unwrap();
    {
        let mut tx = app.db.begin().await.unwrap();
        assert!(storage::command_transfer_open(&mut tx, id).await.unwrap());
        tx.commit().await.unwrap();
    }
    let next_process = orch_api_core::blobs::Blobs::in_memory();
    next_process
        .reap_command_transfers(&app.db, app.clock.as_ref(), &app.link.dispatcher())
        .await
        .unwrap();
    let mut tx = app.db.begin().await.unwrap();
    assert!(!storage::command_transfer_open(&mut tx, id).await.unwrap());
    held.begin_in(&mut tx).await.unwrap();
    assert!(
        !storage::command_transfer_open(&mut tx, id).await.unwrap(),
        "late old work cannot reopen the durable release fence"
    );
    tx.commit().await.unwrap();
}
