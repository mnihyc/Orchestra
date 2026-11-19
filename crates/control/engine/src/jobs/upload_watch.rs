//! How far the content a machine puts up for a job's command has come, shown while the job waits.
//!
//! A command that makes a file on a machine and puts it up answers when the whole is there. The export
//! of a session as its harness's own archive is one: the archive is the size of the session's history,
//! and for as long as it takes to cross the line the job waits with nothing to show. The system holds
//! what has arrived of every upload of a machine, so that an upload cut short goes on from there
//! ([`orch_db::queries::storage::upload`]); this reads that record, and the job's progress says how
//! many bytes have arrived, at what rate, and when the rest should be there.
//!
//! The upload is found without its hash, which only the machine knows until it answers. The content
//! endpoint records the authenticated command with each accepted piece. Only uploads of that exact
//! command can supply progress; when there are two nothing is shown. The one found is kept, and its
//! command is checked again on every read. What was seen is also kept in the job's
//! plan, so that a runner that takes the job over goes on showing from where it was.
//!
//! **Resuming.** The record this reads is the system's own and lasts a day after the last piece
//! (`STALE_UPLOAD_AGE` in the API). When a command is given up or fails while the machine is
//! putting up, the part that arrived stays, and a new command that puts up the same bytes goes on
//! from there: the connector asks the system where it stands before it sends a piece. So the carry
//! of a session that stopped half way, begun again while the session has not changed, sends only the
//! rest. A failure says so ([`Reached::after_a_stop`]), to whoever reads it.

use std::convert::Infallible;
use std::future::Future;
use std::time::{Duration, Instant};

use orch_db::queries::storage;
use orch_types::{Hash, Id};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::EngineError;
use crate::jobs::runner::{Job, percent_of};

/// How often the system's record of the upload is read: once a second, as often as the node feed
/// reports file work, so that watching costs one short read of a small table in that time.
const LOOK_EVERY: Duration = Duration::from_secs(1);
/// Into how many steps the size of the file is divided for what the plan keeps: twenty, so that the
/// plan is written at most that many times for an upload, and a runner that takes over loses at most
/// a twentieth of what the person was shown.
const CHECKPOINT_STEPS: i64 = 20;
/// The member of the job's plan that keeps what was seen.
pub(crate) const CHECKPOINT: &str = "native_upload";
/// What the job's progress says while the bytes cross the line.
const MESSAGE: &str = "Uploading the session's files";

/// How far an upload had come when it was last seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Reached {
    /// The command whose upload it is.
    pub command: Id,
    /// The hash the content is to have.
    pub hash: Hash,
    /// How large the whole is.
    pub size: i64,
    /// How many bytes had arrived.
    pub received: i64,
}

impl Reached {
    /// The words a failure of the command gets when part of the content had arrived: that the part
    /// is kept, and that begun again it is not sent twice. Nothing when no part had arrived, or the
    /// whole had.
    pub(crate) fn after_a_stop(&self) -> Option<String> {
        (self.received > 0 && self.received < self.size).then(|| {
            format!(
                "{} of the {} of the session's files had reached the system when it stopped. The system keeps them for a day, and carrying the session again goes on from there while the session has not changed.",
                iec(self.received),
                iec(self.size),
            )
        })
    }
}

/// What a wait for a command came to, and how far the command's upload had come.
pub(crate) struct Watched<T> {
    /// What the wait came to.
    pub result: T,
    /// The last that was seen of the upload, if one was seen.
    pub reached: Option<Reached>,
}

/// Waits for `wait`, the job's wait for `command` of `machine`, and while it lasts shows how far the
/// content the machine puts up for it has come. `share` is the part of the running phase that the upload
/// is, from nothing to one: the bar of the phase moves that far while the bytes cross. `succeeded` says
/// whether what the wait came to is a success: the bytes are then no longer the job's to show, and are
/// taken from its progress (after a failure they stay, as the last that was known).
///
/// Showing is for the person, and never fails the command: a read that does not go through is
/// tried again a second later.
pub(crate) async fn watching<T>(
    job: &Job,
    machine: Id,
    command: Id,
    share: f64,
    wait: impl Future<Output = T>,
    succeeded: impl FnOnce(&T) -> bool,
) -> Watched<T> {
    let mut watch = Watch::begin(job, machine, command, share).await;
    let result = {
        tokio::pin!(wait);
        let watcher = watch.run(job);
        tokio::pin!(watcher);
        tokio::select! {
            result = &mut wait => result,
            never = &mut watcher => match never {},
        }
    };
    let succeeded = succeeded(&result);
    if !succeeded {
        watch.reached = match watch.retained(job).await {
            Ok(confirmed) => confirmed,
            Err(error) => {
                tracing::debug!(%error, job = %job.id(), "the remaining partial upload could not be confirmed");
                None
            }
        };
    }
    watch.clear(job, succeeded).await;
    Watched {
        result,
        reached: watch.reached,
    }
}

struct Watch {
    machine: Id,
    command: Id,
    share: f64,
    /// The upload that is looked at, once it is found.
    locked: Option<Hash>,
    reached: Option<Reached>,
    /// How many bytes had arrived when the plan was last written.
    kept: i64,
    /// When the bytes began to be counted for the rate, and how many had arrived then.
    first: Option<(Instant, i64)>,
    /// What the progress says now.
    shown: Option<i64>,
}

impl Watch {
    async fn begin(job: &Job, machine: Id, command: Id, share: f64) -> Self {
        let mut watch = Self {
            machine,
            command,
            share,
            locked: None,
            reached: None,
            kept: 0,
            first: None,
            shown: None,
        };
        // A runner that took the job over finds where its predecessor was.
        if let Ok(plan) = job.plan().await
            && let Some(kept) = plan
                .get(CHECKPOINT)
                .and_then(|kept| serde_json::from_value::<Reached>(kept.clone()).ok())
            && kept.command == command
            && let Ok(mut conn) = job.engine().db().acquire().await
            && let Ok(Some(row)) =
                storage::upload_of_command(&mut conn, machine, command, &kept.hash).await
        {
            // Old checkpoints may predate command attribution. They are hints, never proof of whose bytes these are.
            watch.locked = Some(row.hash);
        }
        watch
    }

    async fn run(&mut self, job: &Job) -> Infallible {
        let mut every = tokio::time::interval(LOOK_EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            every.tick().await;
            if let Err(error) = self.look(job).await {
                tracing::debug!(%error, job = %job.id(), "the bytes of an upload could not be read; the next look tries again");
            }
        }
    }

    /// A failure may promise resumption only for bytes the system still keeps for this exact command. A rejected hash
    /// or expired upload has no retained partial file, even though earlier observations counted bytes that arrived.
    async fn retained(&self, job: &Job) -> Result<Option<Reached>, EngineError> {
        let Some(seen) = self.reached else {
            return Ok(None);
        };
        let mut conn = job.engine().db().acquire().await?;
        Ok(
            storage::upload_of_command(&mut conn, self.machine, self.command, &seen.hash)
                .await?
                .filter(|row| row.size == seen.size)
                .map(|row| Reached {
                    received: row.received.clamp(0, row.size.max(0)),
                    ..seen
                }),
        )
    }

    async fn look(&mut self, job: &Job) -> Result<(), EngineError> {
        let (row, complete) = {
            let mut conn = job.engine().db().acquire().await?;
            let row = match self.locked {
                Some(hash) => {
                    storage::upload_of_command(&mut conn, self.machine, self.command, &hash).await?
                }
                None => {
                    let mut found =
                        storage::uploads_of_command(&mut conn, self.machine, self.command, 2)
                            .await?;
                    if found.len() == 1 { found.pop() } else { None }
                }
            };
            let complete = if row.is_none()
                && let Some(seen) = self.reached
            {
                storage::blob(&mut conn, &seen.hash)
                    .await?
                    .is_some_and(|blob| blob.size == seen.size)
            } else {
                false
            };
            (row, complete)
        };
        let Some(row) = row else {
            // Expiry, hash rejection or another command continuing the file also remove our match. Only a verified
            // whole blob establishes completion; an absent partial record does not.
            if complete
                && let Some(seen) = self.reached
                && seen.received < seen.size
            {
                self.settle(job, seen.size, seen.size).await?;
            }
            return Ok(());
        };
        if self.locked != Some(row.hash) {
            self.locked = Some(row.hash);
            self.first = None;
        }
        self.settle(job, row.received, row.size).await
    }

    /// Takes from the job's progress what this put there: the line that says what is under way, and,
    /// when the command succeeded, the bytes of an upload that is over.
    async fn clear(&self, job: &Job, bytes_too: bool) {
        if self.shown.is_none() {
            return;
        }
        let cleared = job
            .amend(move |progress, _| {
                if progress.message.as_deref() == Some(MESSAGE) {
                    progress.message = None;
                }
                if bytes_too {
                    progress.bytes_done = 0;
                    progress.bytes_total = None;
                    progress.rate_bps = None;
                    progress.eta_seconds = None;
                }
            })
            .await;
        if let Err(error) = cleared {
            tracing::debug!(%error, job = %job.id(), "the bytes of an upload that is over could not be taken from the progress");
        }
    }

    /// Takes in that `received` of `size` bytes have arrived: shows it, and keeps it in the plan when
    /// it has gone a step further than what the plan holds.
    async fn settle(&mut self, job: &Job, received: i64, size: i64) -> Result<(), EngineError> {
        let Some(hash) = self.locked else {
            return Ok(());
        };
        let reached = Reached {
            command: self.command,
            hash,
            size,
            received: received.clamp(0, size.max(0)),
        };
        let first = *self.first.get_or_insert((Instant::now(), reached.received));
        self.reached = Some(reached);
        if self.shown != Some(reached.received) {
            self.shown = Some(reached.received);
            let counted = first.1.min(reached.received);
            let seconds = first.0.elapsed().as_secs_f64();
            let rate = (seconds >= 1.0 && reached.received > counted)
                .then(|| ((reached.received - counted) as f64 / seconds) as i64)
                .filter(|rate| *rate > 0);
            let left = size - reached.received;
            let eta = rate.map(|rate| left.max(0) / rate);
            let fraction = if size > 0 {
                reached.received as f64 / size as f64
            } else {
                1.0
            };
            let share = self.share;
            job.amend(move |progress, _| {
                progress.bytes_done = reached.received;
                progress.bytes_total = Some(reached.size);
                progress.rate_bps = rate;
                progress.eta_seconds = eta;
                progress.message = Some(MESSAGE.to_owned());
                progress.percent = percent_of(progress, share * fraction);
            })
            .await?;
        }
        let step = (size / CHECKPOINT_STEPS).max(1);
        if self.kept == 0 || reached.received - self.kept >= step || reached.received >= size {
            self.kept = reached.received.max(1);
            job.remember(&json!({ CHECKPOINT: reached })).await?;
        }
        Ok(())
    }
}

/// A size as a person reads it: the largest unit it fills, to a tenth.
pub(crate) fn iec(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes.max(0) as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_is_told_in_the_largest_unit_it_fills() {
        assert_eq!(iec(0), "0 bytes");
        assert_eq!(iec(1023), "1023 bytes");
        assert_eq!(iec(1024), "1.0 KiB");
        assert_eq!(iec(3 * 1024 * 1024 + 512 * 1024), "3.5 MiB");
        assert_eq!(iec(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }

    #[test]
    fn a_failure_tells_of_a_part_that_arrived_and_of_no_other_case() {
        let reached = |received| Reached {
            command: Id::nil(),
            hash: Hash::of(b"x"),
            size: 4096,
            received,
        };
        let told = reached(1024).after_a_stop().unwrap();
        assert!(
            told.contains("1.0 KiB of the 4.0 KiB") && told.contains("goes on from there"),
            "{told}"
        );
        assert_eq!(reached(0).after_a_stop(), None, "nothing arrived");
        assert_eq!(reached(4096).after_a_stop(), None, "all of it arrived");
    }
}
