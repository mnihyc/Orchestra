//! Archives of what a person may read in a place, removed when their time has run out or their person is gone, what an unfinished archive left in the scratch directory removed once it is a day old, and the rows of those that were removed or that failed forgotten after a month.
//!
//! An archive is kept for as many hours as the setting `export.keep_hours` said when it was made (a day as
//! shipped), counted from the moment it was ready. Every ten minutes the archives that are ready and past their
//! time, and those whose person is gone (their row has no person any more), are made `expired`: their row lets go
//! of the blob, and the pin that held the bytes in the store is taken off with its grace at an end, so that the
//! sweep of the API takes them out at its next run. An archive whose place was purged and whose person is not gone
//! stays its person's until its time. The row of an archive that was removed, or that failed, stays for
//! [`ROW_KEPT_DAYS`] days so that its person, and whoever audits, can see that it was made; after them it is
//! forgotten. A person who removes an archive by hand is served by the API at once; this is the clock's.
//!
//! The same run looks in the scratch directory of the store (`tmp/` below the storage root) for what a writer that
//! was killed left: the zip of an archive (`export-<job>.zip`) whose job does not run any more, and the zip of a
//! chat with its files (`chat-<id>.zip`), once the file is a day old. Anything else in that directory is another
//! kind of scratch file and is not touched.
//!
//! The job runs under the lock of its own, and asks the clock, never the system's, so that what is due is what is
//! due at that moment and nothing else.

use crate::schedule::{self, Job};
use chrono::{DateTime, Duration, Utc};
use orch_db::queries::exports as rows;
use orch_db::queries::jobs;
use orch_db::{Db, DbError};
use orch_types::{Clock, Id};
use std::path::Path;

pub use rows::ROW_KEPT_DAYS;

/// How many archives one run removes; the rest are removed by the next, ten minutes later.
const BATCH: i64 = 200;

/// How old the file an unfinished archive left must be before it is removed: a day.
const SCRATCH_KEPT: Duration = Duration::days(1);

/// What a run removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Purged {
    /// Archives whose time had run out or whose person was gone.
    pub removed: usize,
    /// Rows of archives that were removed or that failed, forgotten after the days they stay.
    pub forgotten: u64,
    /// Files that unfinished archives left in the scratch directory.
    pub scratch: usize,
}

/// Removes the archives that are to go and forgets the rows that stayed long enough, under the lock of the job,
/// and removes what unfinished archives left in `scratch`. None where another run holds the lock.
pub async fn purge(db: &Db, clock: &dyn Clock, scratch: &Path) -> Result<Option<Purged>, DbError> {
    let Some(mut tx) = schedule::take(db, Job::ExportPurge).await? else {
        return Ok(None);
    };
    let now = clock.now();
    let removed = rows::expire_due(&mut tx, now, BATCH).await?.len();
    let forgotten = rows::forget_before(&mut tx, now - Duration::days(ROW_KEPT_DAYS)).await?;
    tx.commit().await.map_err(DbError::from)?;
    let scratch = sweep_scratch(db, scratch, now).await?;
    Ok(Some(Purged {
        removed,
        forgotten,
        scratch,
    }))
}

/// What kind of file of the scratch directory a name is, and the job it belongs to where it belongs to one.
enum Left {
    /// The zip an archive is written in: `export-<job>.zip`.
    Archive(Id),
    /// The zip of a chat with its files, made for one answer: `chat-<id>.zip`.
    Chat,
}

/// Reads a name of the scratch directory: an archive's zip or a chat's, and nothing else.
fn left_by(name: &str) -> Option<Left> {
    let id = |rest: &str| {
        let hex = rest.strip_suffix(".zip")?;
        (hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hex.parse::<Id>().ok())
            .flatten()
    };
    if let Some(rest) = name.strip_prefix("export-") {
        return id(rest).map(Left::Archive);
    }
    name.strip_prefix("chat-").and_then(id).map(|_| Left::Chat)
}

/// Removes the zips that writers that were killed left in `dir`, once each is a day old and no job that runs is the
/// one that writes it. Says how many went. A directory that is not there holds nothing.
async fn sweep_scratch(db: &Db, dir: &Path, now: DateTime<Utc>) -> Result<usize, DbError> {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return Ok(0);
    };
    let mut removed = 0;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Some(left) = entry.file_name().to_str().and_then(left_by) else {
            continue;
        };
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if !meta.is_file() || now - DateTime::<Utc>::from(modified) < SCRATCH_KEPT {
            continue;
        }
        if let Left::Archive(job) = left {
            let mut conn = db.acquire().await?;
            let running = jobs::job(&mut conn, job)
                .await?
                .is_some_and(|row| row.is_open());
            if running {
                continue;
            }
        }
        match tokio::fs::remove_file(entry.path()).await {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(%error, file = %entry.path().display(), "a file an unfinished archive left could not be removed");
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_zips_of_an_archive_and_of_a_chat_are_ours_in_the_scratch_directory() {
        let id = "0123456789abcdef0123456789abcdef";
        assert!(matches!(
            left_by(&format!("export-{id}.zip")),
            Some(Left::Archive(_))
        ));
        assert!(matches!(
            left_by(&format!("chat-{id}.zip")),
            Some(Left::Chat)
        ));
        for other in [
            format!("blob-{id}.part"),
            format!("export-{id}.zip.part"),
            "export-short.zip".to_owned(),
            format!("export-{id}"),
            format!("import-{id}.zip"),
            format!("export-{}.zip", "g".repeat(32)),
            "notes.txt".to_owned(),
        ] {
            assert!(left_by(&other).is_none(), "{other}");
        }
    }
}
