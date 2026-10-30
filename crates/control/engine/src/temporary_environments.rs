//! Temporary environment warning, Keep and cleanup through the ordinary leased removal jobs.

use crate::{Engine, error::EngineError};
use chrono::{DateTime, Utc};
use orch_db::{
    Conn, DbError,
    queries::environments::{hosting, temporary as rows},
};
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Time};

/// The owner always has this much time after a warning to keep or export the files.
const NOTICE_HOURS: i64 = 1;
/// The same bounded sweep size as ordinary environment maintenance.
const BATCH: i64 = 100;

fn after_hours(at: DateTime<Utc>, hours: i64) -> Result<DateTime<Utc>, EngineError> {
    chrono::Duration::try_hours(hours)
        .and_then(|duration| at.checked_add_signed(duration))
        .ok_or_else(|| {
            EngineError::unreadable("the temporary idle time is outside the supported date range")
        })
}

async fn idle_hours(conn: &mut Conn, id: Id) -> Result<i64, EngineError> {
    setting::<i64>(conn, Place::Environment(id), keys::ENV_TEMPORARY_IDLE_HOURS)
        .await?
        .filter(|hours| *hours > 0)
        .ok_or_else(|| EngineError::unreadable("temporary idle time must be at least one hour"))
}

/// Creation and ordinary setting edits use the same durable lifetime record.
pub async fn ensure(
    conn: &mut Conn,
    id: Id,
    now: DateTime<Utc>,
) -> Result<Option<rows::Lifetime>, EngineError> {
    let Some(environment) = hosting::environment(conn, id).await? else {
        return Ok(None);
    };
    if rows::mode(&environment.config).is_none() || environment.deleted_at.is_some() {
        return Ok(None);
    }
    if let Some(row) = rows::read(conn, id).await? {
        return Ok(Some(row));
    }
    let hours = idle_hours(conn, id).await?;
    let activity = environment
        .last_activity_at
        .unwrap_or(environment.created_at);
    let row = rows::Lifetime {
        environment_id: id,
        thread_id: None,
        idle_hours: hours,
        activity_at: activity,
        archive_at: None,
        expires_at: after_hours(activity, hours)?,
        warned_at: None,
        warned_owner_id: None,
        active: false,
    };
    rows::save(conn, &row, now, false).await?;
    Ok(Some(row))
}

/// Thread creation associates its environment before committing, without a second request.
pub async fn bind_thread(
    conn: &mut Conn,
    environment: Id,
    thread: Id,
    owner: Id,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    if ensure(conn, environment, now).await?.is_some() {
        rows::associate(conn, environment, thread, owner).await?;
    }
    Ok(())
}

/// Export is owner activity and leaves a fresh notice opportunity after its capture completes.
pub async fn export_activity(
    conn: &mut Conn,
    id: Id,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    sqlx::query("UPDATE environments SET last_activity_at=$2,updated_at=$2,revision=revision+1 WHERE id=$1 AND deleted_at IS NULL")
        .bind(id).bind(now).execute(&mut *conn).await.map_err(DbError::from)?;
    let Some(mut row) = ensure(conn, id, now).await? else {
        return Ok(());
    };
    let environment = hosting::environment(conn, id)
        .await?
        .ok_or_else(|| EngineError::unreadable("the environment is gone"))?;
    row.idle_hours = idle_hours(conn, id).await?;
    row.activity_at = now;
    row.archive_at = if rows::mode(&environment.config) == Some(true) {
        match row.thread_id {
            Some(thread) => rows::archived_at(conn, thread).await?,
            None => None,
        }
    } else {
        None
    };
    let idle = after_hours(now, row.idle_hours)?;
    row.expires_at = row
        .archive_at
        .map_or(idle, |archive| archive.min(idle))
        .max(after_hours(now, NOTICE_HOURS)?);
    row.warned_at = None;
    row.warned_owner_id = None;
    rows::save(conn, &row, now, false).await?;
    Ok(())
}

struct Timing {
    now: DateTime<Utc>,
    activity: DateTime<Utc>,
    hours: i64,
    archived: Option<DateTime<Utc>>,
    owner: Id,
    active: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Wait,
    Warn,
    Remove,
}

fn advance(row: &mut rows::Lifetime, timing: &Timing) -> Result<Action, EngineError> {
    let idle = after_hours(timing.activity, timing.hours)?;
    if row.activity_at != timing.activity
        || row.idle_hours != timing.hours
        || row.archive_at != timing.archived
    {
        row.activity_at = timing.activity;
        row.idle_hours = timing.hours;
        row.archive_at = timing.archived;
        row.expires_at = timing.archived.map_or(idle, |archive| archive.min(idle));
        row.warned_at = None;
        row.warned_owner_id = None;
    }
    row.active = timing.active;
    if timing.active {
        row.warned_at = None;
        row.warned_owner_id = None;
        return Ok(Action::Wait);
    }
    let notice = after_hours(timing.now, NOTICE_HOURS)?;
    if row.expires_at <= notice
        && (row.warned_at.is_none() || row.warned_owner_id != Some(timing.owner))
    {
        // Late sweeps and a newly archived thread still give a full warning/export opportunity.
        row.expires_at = row.expires_at.max(notice);
        row.warned_at = Some(timing.now);
        row.warned_owner_id = Some(timing.owner);
        return Ok(Action::Warn);
    }
    if row.expires_at <= timing.now
        && row
            .warned_at
            .is_some_and(|at| after_hours(at, NOTICE_HOURS).is_ok_and(|due| due <= timing.now))
    {
        return Ok(Action::Remove);
    }
    Ok(Action::Wait)
}

impl Engine {
    /// A bounded, nonblocking sweep queues cleanup; machine and file work belongs to job leases.
    pub(crate) async fn reap_temporary_environments(&self) -> Result<(), EngineError> {
        let ids = {
            let mut conn = self.db().acquire().await?;
            let mut ids = rows::untracked(&mut conn, BATCH).await?;
            ids.extend(rows::candidates(&mut conn, BATCH).await?);
            ids.sort_unstable();
            ids.dedup();
            ids
        };
        let mut queued = false;
        for id in ids {
            let mut tx = self.db().begin().await?;
            orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
            if !rows::try_lock(&mut tx, id).await? {
                continue;
            }
            let Some(environment) = hosting::environment(&mut tx, id).await? else {
                continue;
            };
            let Some(with_thread) = rows::mode(&environment.config) else {
                rows::forget(&mut tx, id).await?;
                tx.commit().await.map_err(DbError::from)?;
                continue;
            };
            let Some(mut row) = ensure(&mut tx, id, self.inner.clock.now()).await? else {
                continue;
            };
            let before = row.clone();
            let archived = if with_thread {
                match row.thread_id {
                    Some(thread) => rows::archived_at(&mut tx, thread).await?,
                    None => None,
                }
            } else {
                None
            };
            let active = row.active || hosting::active_turns(&mut tx, id).await? > 0;
            let hours = idle_hours(&mut tx, id).await?;
            // Reads and lock waits may span an archive or a later clock tick. Every environment
            // gets its full notice from the decision time, never the beginning of the batch.
            let now = self.inner.clock.now();
            let activity = if active {
                now
            } else {
                environment
                    .last_activity_at
                    .unwrap_or(environment.created_at)
                    .max(row.activity_at)
            };
            let timing = Timing {
                now,
                activity,
                hours,
                archived,
                owner: environment.owner_id,
                active,
            };
            let action = advance(&mut row, &timing)?;
            let visible_change = row.idle_hours != before.idle_hours
                || row.archive_at != before.archive_at
                || row.warned_at != before.warned_at
                || (!active && row.expires_at != before.expires_at);
            rows::save(&mut tx, &row, now, visible_change).await?;
            match action {
                Action::Warn => {
                    crate::notifications::temporary_environment_warning(
                        &mut tx,
                        &environment,
                        row.expires_at,
                        now,
                    )
                    .await?;
                }
                Action::Remove => {
                    // Exports and all other work must finish before its files can disappear.
                    let open = orch_db::queries::jobs::open_of_environment(&mut tx, id).await?;
                    if !open.is_empty() {
                        tx.commit().await.map_err(DbError::from)?;
                        continue;
                    }
                    self.remove_bindings_in(&mut tx, id, environment.owner_id, Time::new(now))
                        .await?;
                    orch_authz::epoch::bump(&mut tx).await?;
                    self.schedule_environment_removal_with_days(
                        &mut tx,
                        id,
                        None,
                        (now, now),
                        Some(0),
                    )
                    .await?;
                    match crate::jobs::remove::queue(&mut tx, now, id, None).await {
                        Ok(_) => queued = true,
                        Err(
                            crate::jobs::runner::MakeError::Running(_)
                            | crate::jobs::runner::MakeError::NoEnvironment,
                        ) => continue,
                        Err(crate::jobs::runner::MakeError::Db(error)) => return Err(error.into()),
                    }
                }
                Action::Wait => {}
            }
            tx.commit().await.map_err(DbError::from)?;
        }
        if queued {
            self.kick();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn start() -> (rows::Lifetime, Timing) {
        let at = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let owner = orch_types::new_id();
        (
            rows::Lifetime {
                environment_id: orch_types::new_id(),
                thread_id: None,
                idle_hours: 24,
                activity_at: at,
                archive_at: None,
                expires_at: at + Duration::hours(24),
                warned_at: None,
                warned_owner_id: None,
                active: false,
            },
            Timing {
                now: at,
                activity: at,
                hours: 24,
                archived: None,
                owner,
                active: false,
            },
        )
    }

    #[test]
    fn the_owner_is_warned_once_an_hour_before_idle_removal() {
        let (mut row, mut time) = start();
        time.now += Duration::hours(22);
        assert_eq!(advance(&mut row, &time).unwrap(), Action::Wait);
        time.now += Duration::hours(1);
        assert_eq!(advance(&mut row, &time).unwrap(), Action::Warn);
        let deadline = row.expires_at;
        assert_eq!(deadline, time.now + Duration::hours(1));
        assert_eq!(advance(&mut row, &time).unwrap(), Action::Wait);
        time.now += Duration::minutes(59);
        assert_eq!(advance(&mut row, &time).unwrap(), Action::Wait);
        time.now += Duration::minutes(1);
        assert_eq!(advance(&mut row, &time).unwrap(), Action::Remove);
        assert_eq!(row.expires_at, deadline);
    }

    #[test]
    fn archiving_the_creating_thread_and_a_late_sweep_both_leave_a_full_export_hour() {
        for late in [false, true] {
            let (mut row, mut time) = start();
            time.now += Duration::hours(if late { 40 } else { 2 });
            if !late {
                time.archived = Some(time.now);
            }
            assert_eq!(advance(&mut row, &time).unwrap(), Action::Warn);
            assert_eq!(row.expires_at, time.now + Duration::hours(1));
            time.now += Duration::minutes(59);
            assert_eq!(advance(&mut row, &time).unwrap(), Action::Wait);
            time.now += Duration::minutes(1);
            assert_eq!(advance(&mut row, &time).unwrap(), Action::Remove);
        }
    }

    #[test]
    fn new_activity_restarts_idle_time_and_a_new_owner_gets_their_own_warning() {
        let (mut row, mut time) = start();
        time.now += Duration::hours(23);
        assert_eq!(advance(&mut row, &time).unwrap(), Action::Warn);
        time.activity = time.now;
        assert_eq!(advance(&mut row, &time).unwrap(), Action::Wait);
        assert_eq!(row.expires_at, time.now + Duration::hours(24));
        assert!(row.warned_at.is_none());
        time.now += Duration::hours(24);
        assert_eq!(advance(&mut row, &time).unwrap(), Action::Warn);
        time.owner = orch_types::new_id();
        time.now += Duration::minutes(30);
        assert_eq!(advance(&mut row, &time).unwrap(), Action::Warn);
        assert_eq!(row.expires_at, time.now + Duration::hours(1));
        assert_eq!(row.warned_owner_id, Some(time.owner));
    }

    #[test]
    fn a_running_turn_or_export_defers_cleanup_even_after_archive_and_gets_a_new_warning_when_done()
    {
        let (mut row, mut time) = start();
        time.now += Duration::hours(2);
        time.archived = Some(time.now);
        time.active = true;
        assert_eq!(advance(&mut row, &time).unwrap(), Action::Wait);
        time.now += Duration::hours(2);
        assert_eq!(advance(&mut row, &time).unwrap(), Action::Wait);
        assert!(row.warned_at.is_none());
        time.active = false;
        assert_eq!(advance(&mut row, &time).unwrap(), Action::Warn);
        assert_eq!(row.expires_at, time.now + Duration::hours(1));
    }

    #[test]
    fn changing_the_places_idle_time_changes_the_deadline_without_an_overflow_panic() {
        let (mut row, mut time) = start();
        time.hours = 48;
        assert_eq!(advance(&mut row, &time).unwrap(), Action::Wait);
        assert_eq!(row.expires_at, time.now + Duration::hours(48));
        time.hours = i64::MAX;
        assert!(advance(&mut row, &time).is_err());
    }
}
