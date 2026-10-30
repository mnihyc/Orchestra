//! Archives past their time, and those whose person is gone, are removed by the clock and their bytes let go for the sweep of the store, an archive whose place was purged stays its person's until its time, the rows of those that were removed or that failed are forgotten after a month and not before, an archive that is being written is never touched, what a killed writer left in the scratch directory goes once it is a day old, and the job is one the worker schedules.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::{DateTime, Duration, Utc};
use orch_config::ServerConfig;
use orch_db::queries::exports::{self as rows, Written};
use orch_db::queries::jobs::{self, NewJob};
use orch_types::{Hash, Id, new_id};
use orch_worker_core::channels::Notifiers;
use orch_worker_core::exports::{self, Purged, ROW_KEPT_DAYS};
use orch_worker_core::purge;
use orch_worker_core::schedule::{self, Job};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use support::world::World;

/// A person and a place for the archives of a test, and a second person with archives of their own.
struct Office {
    world: World,
    org: Id,
    ada: Id,
    bea: Id,
    /// The scratch directory of the store: not made until a test needs it, removed when the test is over.
    scratch: PathBuf,
}

impl Drop for Office {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

async fn office() -> Office {
    let world = World::new(Notifiers::new()).await;
    let org = world.org("Acme").await;
    let ada = world.person("ada").await;
    let bea = world.person("bea").await;
    Office {
        world,
        org,
        ada,
        bea,
        scratch: std::env::temp_dir().join(format!("orch-scratch-{}", new_id().simple())),
    }
}

/// What a run removed when it found nothing in the scratch directory.
fn swept(archives: usize, forgotten: u64) -> Purged {
    Purged {
        removed: archives,
        forgotten,
        scratch: 0,
    }
}

impl Office {
    /// The bytes of an archive in the store of content: its row in `blobs`, pinned `pins` times.
    async fn blob(&self, content: &str, pins: i32) -> Hash {
        let hash = Hash::of(content.as_bytes());
        sqlx::query(
            "INSERT INTO blobs (hash, size, pin_count) VALUES ($1, $2, $3)
             ON CONFLICT (hash) DO UPDATE SET pin_count = $3",
        )
        .bind(hash.as_bytes().to_vec())
        .bind(i64::try_from(content.len()).unwrap())
        .bind(pins)
        .execute(&self.world.test.pool)
        .await
        .unwrap();
        hash
    }

    /// An archive that is ready, made `made_hours_ago` hours before the clock's now and kept for `keep_hours`.
    async fn ready(&self, user: Id, hash: &Hash, made_hours_ago: i64, keep_hours: i64) -> Id {
        let id = new_id();
        let made = self.world.now() - Duration::hours(made_hours_ago);
        let mut conn = self.world.test.pool.acquire().await.unwrap();
        rows::insert(
            &mut conn,
            id,
            user,
            self.org,
            &json!({ "what": "all" }),
            made,
        )
        .await
        .unwrap();
        let counts = json!({});
        let left_out = json!([]);
        let stored = hash.to_string();
        assert!(
            rows::make_ready(
                &mut conn,
                id,
                &Written {
                    blob: &stored,
                    bytes: 10,
                    counts: &counts,
                    left_out: &left_out,
                    at: made,
                    expires_at: made + Duration::hours(keep_hours),
                },
            )
            .await
            .unwrap()
        );
        id
    }

    async fn pending(&self, user: Id) -> Id {
        let id = new_id();
        let mut conn = self.world.test.pool.acquire().await.unwrap();
        rows::insert(&mut conn, id, user, self.org, &json!({}), self.world.now())
            .await
            .unwrap();
        id
    }

    async fn state(&self, id: Id) -> Option<(String, Option<String>)> {
        sqlx::query_as("SELECT state, blob FROM exports WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.world.test.pool)
            .await
            .unwrap()
    }

    async fn pins(&self, hash: &Hash) -> (i32, DateTime<Utc>) {
        sqlx::query_as("SELECT pin_count, last_access_at FROM blobs WHERE hash = $1")
            .bind(hash.as_bytes().to_vec())
            .fetch_one(&self.world.test.pool)
            .await
            .unwrap()
    }

    async fn purge(&self) -> Option<Purged> {
        exports::purge(&self.world.db(), &self.world.clock, &self.scratch)
            .await
            .unwrap()
    }

    /// Whose an archive is, and the place it was made of: none for a person or a place that is gone.
    async fn belongs(&self, id: Id) -> (Option<Id>, Option<Id>) {
        sqlx::query_as("SELECT user_id, node_id FROM exports WHERE id = $1")
            .bind(id)
            .fetch_one(&self.world.test.pool)
            .await
            .unwrap()
    }

    /// How many blobs of the store something still holds.
    async fn pinned(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM blobs WHERE pin_count > 0")
            .fetch_one(&self.world.test.pool)
            .await
            .unwrap()
    }

    /// Removes a person for good: their row goes, and what refers to them without owning them is emptied.
    async fn forget_person(&self, person: Id) {
        let gone = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(person)
            .execute(&self.world.test.pool)
            .await
            .unwrap();
        assert_eq!(gone.rows_affected(), 1);
    }

    /// Purges a place the way the worker does it once the days a deleted place is kept are over.
    async fn forget_place(&self, place: Id) {
        sqlx::query("UPDATE nodes SET deleted_at = $2 WHERE id = $1")
            .bind(place)
            .bind(self.world.now() - Duration::days(60))
            .execute(&self.world.test.pool)
            .await
            .unwrap();
        let purged = purge::soft_deleted(
            &self.world.db(),
            &self.world.clock,
            &ServerConfig::default(),
        )
        .await
        .unwrap()
        .expect("the lock is free");
        assert!(
            purged
                .iter()
                .any(|each| each.table == "nodes" && each.rows == 1),
            "{purged:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_archive_past_its_time_is_removed_and_its_bytes_are_let_go_for_the_sweep_of_the_store() {
    let o = office().await;
    let (kept_hash, gone_hash, other_hash) = (
        o.blob("kept", 1).await,
        o.blob("gone", 1).await,
        o.blob("another person's", 1).await,
    );
    // Made two hours ago and kept for a day, made a day and a half ago and kept for a day (ten hours past),
    // and a second person's, as old.
    let kept = o.ready(o.ada, &kept_hash, 2, 24).await;
    let gone = o.ready(o.ada, &gone_hash, 36, 24).await;
    let theirs = o.ready(o.bea, &other_hash, 36, 24).await;
    let writing = o.pending(o.ada).await;

    assert_eq!(o.purge().await, Some(swept(2, 0)));
    assert_eq!(
        o.state(kept).await.unwrap().0,
        "ready",
        "its time has not come"
    );
    assert_eq!(o.state(gone).await, Some(("expired".to_owned(), None)));
    assert_eq!(o.state(theirs).await, Some(("expired".to_owned(), None)));
    assert_eq!(
        o.state(writing).await.unwrap().0,
        "pending",
        "one being written is not touched"
    );
    let (pins, _) = o.pins(&kept_hash).await;
    assert_eq!(pins, 1);
    for hash in [&gone_hash, &other_hash] {
        let (pins, last) = o.pins(hash).await;
        assert_eq!(pins, 0, "nothing holds the bytes any more");
        assert_eq!(
            last,
            DateTime::UNIX_EPOCH,
            "and the sweep of the store may take them at its next run"
        );
    }

    // Nothing is due until the clock says so; then the one that was kept goes too.
    assert_eq!(o.purge().await, Some(swept(0, 0)));
    o.world.clock.advance(Duration::hours(23));
    assert_eq!(o.purge().await.unwrap().removed, 1);
    assert_eq!(o.state(kept).await, Some(("expired".to_owned(), None)));
}

#[tokio::test(flavor = "multi_thread")]
async fn bytes_that_another_archive_holds_too_stay_pinned() {
    let o = office().await;
    let hash = o.blob("the same bytes", 2).await;
    let first = o.ready(o.ada, &hash, 36, 24).await;
    let second = o.ready(o.bea, &hash, 1, 24).await;
    assert_eq!(o.purge().await.unwrap().removed, 1);
    assert_eq!(o.state(first).await.unwrap().0, "expired");
    assert_eq!(o.state(second).await.unwrap().0, "ready");
    let (pins, last) = o.pins(&hash).await;
    assert_eq!(pins, 1, "one pin is let go and the other stays");
    assert!(
        last > DateTime::UNIX_EPOCH,
        "so the bytes are not for the sweep"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_row_of_an_archive_that_was_removed_or_failed_stays_a_month_and_is_then_forgotten() {
    let o = office().await;
    let hash = o.blob("old", 1).await;
    // One that ended an hour ago, one that failed, and one that is being written for ever.
    let removed = o.ready(o.ada, &hash, 2, 1).await;
    let failed = o.pending(o.ada).await;
    {
        let mut conn = o.world.test.pool.acquire().await.unwrap();
        assert!(
            rows::make_failed(&mut conn, failed, &json!([]))
                .await
                .unwrap()
        );
    }
    let writing = o.pending(o.bea).await;
    assert_eq!(o.purge().await, Some(swept(1, 0)));
    assert_eq!(o.state(removed).await.unwrap().0, "expired");

    // Twenty-nine days later nothing is forgotten; two days after that both are, and what is being written is not.
    o.world.clock.advance(Duration::days(ROW_KEPT_DAYS - 1));
    assert_eq!(o.purge().await.unwrap().forgotten, 0);
    assert!(o.state(removed).await.is_some() && o.state(failed).await.is_some());
    o.world.clock.advance(Duration::days(2));
    assert_eq!(o.purge().await.unwrap().forgotten, 2);
    assert!(o.state(removed).await.is_none() && o.state(failed).await.is_none());
    assert_eq!(o.state(writing).await.unwrap().0, "pending");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_that_finds_another_run_at_the_job_does_nothing_and_the_job_is_one_the_worker_schedules()
 {
    let o = office().await;
    let hash = o.blob("late", 1).await;
    let late = o.ready(o.ada, &hash, 48, 24).await;
    let holding = schedule::take(&o.world.db(), Job::ExportPurge)
        .await
        .unwrap()
        .expect("the lock is free");
    assert_eq!(o.purge().await, None, "another run holds the lock");
    assert_eq!(o.state(late).await.unwrap().0, "ready");
    // Let go of the lock now, not whenever the connection gets round to the rollback a drop leaves behind.
    holding.rollback().await.unwrap();
    assert_eq!(o.purge().await.unwrap().removed, 1);
    assert!(Job::ALL.contains(&Job::ExportPurge));
    assert_eq!(Job::ExportPurge.key(), "export_purge");
    assert_eq!(Job::named("export_purge"), Some(Job::ExportPurge));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_archive_of_a_person_who_was_removed_for_good_is_removed_at_once_and_its_bytes_are_let_go()
 {
    let o = office().await;
    let (hers_hash, his_hash) = (o.blob("hers", 1).await, o.blob("his", 1).await);
    // Both were made an hour ago and are kept for a day: neither one's time has come.
    let hers = o.ready(o.ada, &hers_hash, 1, 24).await;
    let his = o.ready(o.bea, &his_hash, 1, 24).await;
    assert_eq!(o.purge().await, Some(swept(0, 0)));

    // Ada is removed for good. Her archive does not go with her: the row stays, with nobody it is for.
    o.forget_person(o.ada).await;
    assert_eq!(o.belongs(hers).await, (None, Some(o.org)));
    assert_eq!(o.state(hers).await.unwrap().0, "ready");
    assert_eq!(o.pins(&hers_hash).await.0, 1, "the bytes are still held");

    // The next sweep removes it without waiting for its time, and lets its bytes go.
    assert_eq!(o.purge().await, Some(swept(1, 0)));
    assert_eq!(o.state(hers).await, Some(("expired".to_owned(), None)));
    let (pins, last) = o.pins(&hers_hash).await;
    assert_eq!(pins, 0);
    assert_eq!(
        last,
        DateTime::UNIX_EPOCH,
        "the sweep of the store takes the bytes at its next run, grace or none"
    );
    assert_eq!(o.state(his).await.unwrap().0, "ready", "Bea's is hers");
    assert_eq!(o.pinned().await, 1, "only Bea's bytes are held");
    // Nothing is left to do, however often the sweep runs.
    assert_eq!(o.purge().await, Some(swept(0, 0)));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_archive_of_a_place_that_was_purged_stays_its_persons_until_its_time_and_then_goes() {
    let o = office().await;
    let hash = o.blob("of a place", 1).await;
    let id = o.ready(o.ada, &hash, 1, 24).await;
    assert_eq!(o.belongs(id).await, (Some(o.ada), Some(o.org)));

    // The place is purged: the archive keeps its person and loses the place it was made of, and is kept as long as
    // it was to be, for its person may still fetch it.
    o.forget_place(o.org).await;
    assert_eq!(o.belongs(id).await, (Some(o.ada), None));
    assert_eq!(o.purge().await, Some(swept(0, 0)));
    assert_eq!(o.state(id).await.unwrap().0, "ready");
    assert_eq!(o.pins(&hash).await.0, 1);

    // At its time it is removed like any other, and nothing holds its bytes any more.
    o.world.clock.advance(Duration::hours(22));
    assert_eq!(o.purge().await, Some(swept(0, 0)));
    o.world.clock.advance(Duration::hours(1));
    assert_eq!(o.purge().await, Some(swept(1, 0)));
    assert_eq!(o.state(id).await, Some(("expired".to_owned(), None)));
    let (pins, last) = o.pins(&hash).await;
    assert_eq!((pins, last), (0, DateTime::UNIX_EPOCH));
    assert_eq!(o.pinned().await, 0);

    // The row of the archive of a place that is gone stays a month like any other, and is then forgotten.
    o.world.clock.advance(Duration::days(ROW_KEPT_DAYS + 1));
    assert_eq!(o.purge().await.unwrap().forgotten, 1);
    assert!(o.state(id).await.is_none());
}

/// A file in the scratch directory, last changed `age` before the clock's now, with the bytes a writer that was
/// killed would have left of a zip.
fn left(directory: &Path, name: &str, now: DateTime<Utc>, age: Duration) -> PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let path = directory.join(name);
    std::fs::write(
        &path,
        b"PK\x03\x04 the start of a zip that was never finished",
    )
    .unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::from(now - age))
        .unwrap();
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_a_killed_writer_left_is_removed_once_it_is_a_day_old_and_no_job_that_runs_is_its_writer()
 {
    let o = office().await;
    let now = o.world.now();
    let (running, finished, lost, young) = (new_id(), new_id(), new_id(), new_id());
    // Two jobs that write archives: one that runs, one that is over. The others have no job at all.
    {
        let mut conn = o.world.test.pool.acquire().await.unwrap();
        for id in [running, finished] {
            jobs::insert(
                &mut conn,
                &NewJob {
                    id,
                    kind: "export",
                    node_id: None,
                    environment_id: None,
                    provider_id: None,
                    plan: &json!({}),
                    progress: &json!({}),
                    created_by: Some(o.ada),
                },
                now,
            )
            .await
            .unwrap();
        }
        sqlx::query("UPDATE jobs SET state = 'running', started_at = $2 WHERE id = $1")
            .bind(running)
            .bind(now)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE jobs SET state = 'failed', finished_at = $2 WHERE id = $1")
            .bind(finished)
            .bind(now)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let (two_days, a_bit_less) = (Duration::days(2), Duration::hours(23));
    let archive = |job: Id| format!("export-{}.zip", job.simple());
    let kept_alive = left(&o.scratch, &archive(running), now, two_days);
    let ended = left(&o.scratch, &archive(finished), now, two_days);
    let orphan = left(&o.scratch, &archive(lost), now, two_days);
    let recent = left(&o.scratch, &archive(young), now, a_bit_less);
    let chat = left(
        &o.scratch,
        &format!("chat-{}.zip", new_id().simple()),
        now,
        two_days,
    );
    let chat_now = left(
        &o.scratch,
        &format!("chat-{}.zip", new_id().simple()),
        now,
        a_bit_less,
    );
    // Whatever else lies there is not ours to remove, however old.
    let part = left(
        &o.scratch,
        &format!("blob-{}.part", Hash::of(b"upload")),
        now,
        two_days,
    );
    let notes = left(&o.scratch, "notes.txt", now, two_days);
    let folder = o.scratch.join(format!("export-{}.zip", new_id().simple()));
    std::fs::create_dir_all(&folder).unwrap();

    let done = o.purge().await.unwrap();
    assert_eq!(
        done,
        Purged {
            removed: 0,
            forgotten: 0,
            scratch: 3
        },
        "the zip of a job that is over, the zip no job wrote, and a chat's"
    );
    for gone in [&ended, &orphan, &chat] {
        assert!(!gone.exists(), "{}", gone.display());
    }
    for stays in [&kept_alive, &recent, &chat_now, &part, &notes, &folder] {
        assert!(stays.exists(), "{}", stays.display());
    }

    // A day later the young ones are old, and the one of the job that runs is still its writer's.
    o.world.clock.advance(Duration::hours(2));
    let done = o.purge().await.unwrap();
    assert_eq!(done.scratch, 2, "{done:?}");
    assert!(!recent.exists() && !chat_now.exists());
    assert!(kept_alive.exists() && part.exists() && notes.exists());

    // When the job is over, its file goes too, and a run that finds the directory gone does nothing.
    sqlx::query("UPDATE jobs SET state = 'succeeded', finished_at = $2 WHERE id = $1")
        .bind(running)
        .bind(o.world.now())
        .execute(&o.world.test.pool)
        .await
        .unwrap();
    assert_eq!(o.purge().await.unwrap().scratch, 1);
    assert!(!kept_alive.exists());
    std::fs::remove_dir_all(&o.scratch).unwrap();
    assert_eq!(o.purge().await, Some(swept(0, 0)));
}
