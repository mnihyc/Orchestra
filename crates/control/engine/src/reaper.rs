//! The schedule that finds what is due for removal (environments, those unused for as long as their setting says among them, retired runtimes, a deleted provider's runtime, what a purged org has on hosts), gives notice, and retries with growing pauses.
//!
//! Every `[engine] reaper_secs` the engine looks at the environments and their units: it starts a
//! removal job for each environment whose removal is due and for each retired runtime of an environment
//! whose time has come (`jobs::remove`); it finishes a stop that a process that died began; it stops a
//! running environment that has had no activity for as long as its `env.idle_stop_minutes` allows and
//! has no turn running; and it schedules the removal of an environment that has had no thread for as
//! long as its `env.unused_remove_days` allows, a day ahead (`[engine] removal_notice_hours`), so that
//! its owner and whoever manages it can keep it. A removal that fails is dated again by the job, with
//! a pause that grows, and is shown on the environment and on its host.
//!
//! Jobs that are due are taken up at every sweep, not only at the reaper's pace: an environment being
//! made does not wait for the schedule, and neither does a directory on a person's machine whose
//! machine came or went.

use crate::engine::{Engine, lock};
use crate::environments::StopReason;
use crate::error::EngineError;
use crate::jobs::runner::MakeError;
use crate::jobs::{provision, remove};
use chrono::{DateTime, Utc};
use orch_db::DbError;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::environments::hosting::{self as rows, EnvironmentState};
use orch_settings::{Place as SettingsPlace, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::Id;
use serde_json::json;
use tokio::time::Instant;

/// How many environments or runtimes one look at the schedule takes up for each kind of work.
const REAP_BATCH: i64 = 100;
/// The audit action of a removal that was scheduled.
const REMOVAL_SCHEDULED: &str = "environment.removal_scheduled";
/// Minutes in a day, for the days an unused environment is allowed.
const MINUTES_PER_DAY: i64 = 1_440;

impl Engine {
    /// Takes up the jobs of environments that are due, brings the directories on people's machines in
    /// step with their machines, and on the reaper's pace looks for the rest.
    pub(crate) async fn keep_environments(&self) {
        if let Err(error) = self.deliver_lifecycle_controls().await {
            tracing::warn!(%error, "committed lifecycle controls will be delivered on the next sweep");
        }
        if let Err(error) = self.run_jobs().await {
            tracing::warn!(%error, "the jobs of environments could not be taken up; the next sweep looks again");
        }
        // Accepted/future removal deadlines are independent of idle/temporary policy sweeps.
        if let Err(error) = self.start_due_removals().await {
            tracing::warn!(%error, "due environment removals will be queued on the next sweep");
        }
        self.keep_connected().await;
        self.keep_retention().await;
        // The copies of a credential that are about to lapse are replaced as the sweep goes by.
        if let Err(error) = self.renew_shared_copies().await {
            tracing::warn!(%error, "the copies of credentials that are about to lapse could not be looked at");
        }
        let due = {
            let mut last = lock(&self.inner.reaped);
            let every = self.inner.config.hosting.reaper_every;
            if last.is_some_and(|at: Instant| at.elapsed() < every) {
                false
            } else {
                *last = Some(Instant::now());
                true
            }
        };
        if due && let Err(error) = self.reap().await {
            tracing::warn!(%error, "the schedule of environments could not be carried out; it is looked at again");
        }
    }

    /// One look at everything of environments that may be due.
    async fn reap(&self) -> Result<(), EngineError> {
        self.schedule_agent_history(false);
        self.reap_temporary_environments().await?;
        self.purge_due_kept().await?;
        self.finish_stale_stops().await?;
        self.stop_idle().await?;
        self.schedule_unused().await?;
        // The runtimes of managed providers keep to the same pace.
        self.reap_provider_runtimes().await?;
        // So do the credentials that no environment needs any longer.
        self.return_unneeded_handovers().await?;
        Ok(())
    }

    /// Starts the removal of what is due: an environment whose time has come, and a retired unit of an
    /// environment. One that already has a job under way is left to it.
    async fn start_due_removals(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let (environments, retired) = {
            let mut conn = self.inner.db.acquire().await?;
            (
                rows::environments_due(&mut conn, now, REAP_BATCH).await?,
                rows::runtimes_due(&mut conn, now, REAP_BATCH).await?,
            )
        };
        let mut queued = false;
        for environment in environments {
            queued |= self.queue_removal(now, environment, None).await?;
        }
        for runtime in retired {
            if let Some(environment) = runtime.environment_id {
                queued |= self
                    .queue_removal(now, environment, Some(runtime.id))
                    .await?;
            }
        }
        if queued {
            self.kick();
        }
        Ok(())
    }

    async fn queue_removal(
        &self,
        now: DateTime<Utc>,
        environment: Id,
        runtime: Option<Id>,
    ) -> Result<bool, EngineError> {
        let mut tx = self.inner.db.begin().await?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        if runtime.is_none() {
            self.schedule_environment_removal(&mut tx, environment, None, now, now)
                .await?;
        }
        match remove::queue(&mut tx, now, environment, runtime).await {
            Ok(_) => {
                tx.commit().await.map_err(DbError::from)?;
                Ok(true)
            }
            Err(MakeError::Running(_) | MakeError::NoEnvironment) => Ok(false),
            Err(MakeError::Db(error)) => Err(error.into()),
        }
    }

    /// Finishes the stops that were begun and not carried through, by a process that is gone.
    async fn finish_stale_stops(&self) -> Result<(), EngineError> {
        let wait =
            chrono::Duration::from_std(self.inner.config.hosting.stop_wait).unwrap_or_default();
        let before = self.inner.clock.now() - wait - wait;
        let stale = {
            let mut conn = self.inner.db.acquire().await?;
            rows::stale_stops(&mut conn, before, REAP_BATCH).await?
        };
        for runtime in stale {
            if let Some(environment) = runtime.environment_id {
                let engine = self.clone();
                tokio::spawn(async move {
                    if let Err(error) = engine.finish_stop(environment, runtime.id).await {
                        tracing::warn!(%error, %environment, "a stop that was begun could not be finished");
                    }
                });
            }
        }
        Ok(())
    }

    /// Stops the environments that have had no activity for as long as their own setting allows.
    async fn stop_idle(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let running = {
            let mut conn = self.inner.db.acquire().await?;
            rows::running_environments(&mut conn, REAP_BATCH).await?
        };
        for environment in running {
            if !self.is_idle(&environment, now).await? {
                // A warning that could not be written now is tried at the next look; it holds no stop up.
                if let Err(error) = self.warn_idle(&environment, now).await {
                    tracing::debug!(%error, environment = %environment.id, "an idle warning was not sent");
                }
                continue;
            }
            // A stop of this one that an earlier look began is not over: the host is slow, and asking it
            // again before it has answered, or before the wait for it has run out, is a second command to
            // stop the same unit.
            if !lock(&self.inner.stopping).insert(environment.id) {
                continue;
            }
            let engine = self.clone();
            tokio::spawn(async move {
                let stopped = engine
                    .stop_environment(environment.id, false, StopReason::Idle)
                    .await;
                lock(&engine.inner.stopping).remove(&environment.id);
                if let Err(error) = stopped {
                    // A turn that runs, or a credential that cannot be returned, keeps it up; so does a
                    // host that is slow. It is looked at again.
                    tracing::debug!(%error, environment = %environment.id, "an idle environment was not stopped");
                }
            });
        }
        Ok(())
    }

    /// Warns the people of an environment whose idle stop is near, `env.idle_warning_minutes` before it (0
    /// sends none): once for each stretch without activity, through the notifications of the idle stop.
    async fn warn_idle(
        &self,
        environment: &EnvironmentState,
        now: DateTime<Utc>,
    ) -> Result<(), EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let Some(stops_at) = crate::environments::idle_stop_at(&mut conn, environment).await?
        else {
            return Ok(());
        };
        let minutes: i64 = setting(
            &mut conn,
            SettingsPlace::Environment(environment.id),
            keys::ENV_IDLE_WARNING_MINUTES,
        )
        .await?
        .unwrap_or(0);
        if !warning_due(now, stops_at, minutes) {
            return Ok(());
        }
        crate::notifications::environment_idle_warning(&mut conn, environment, stops_at, now)
            .await?;
        Ok(())
    }

    async fn is_idle(
        &self,
        environment: &EnvironmentState,
        now: DateTime<Utc>,
    ) -> Result<bool, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let minutes: i64 = setting(
            &mut conn,
            SettingsPlace::Environment(environment.id),
            keys::ENV_IDLE_STOP_MINUTES,
        )
        .await?
        .unwrap_or(0);
        if minutes <= 0 {
            return Ok(false);
        }
        let last = environment
            .last_activity_at
            .unwrap_or(environment.created_at);
        Ok(now - last >= chrono::Duration::minutes(minutes))
    }

    /// Schedules the removal of the environments that have had no thread for as long as their setting
    /// allows, ahead by the notice, with a line in their history.
    async fn schedule_unused(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let notice = chrono::Duration::from_std(self.inner.config.hosting.removal_notice)
            .unwrap_or_default();
        let candidates = {
            let mut conn = self.inner.db.acquire().await?;
            rows::refresh_unused(&mut conn, now).await?;
            rows::unused_candidates(&mut conn, REAP_BATCH).await?
        };
        for environment in candidates {
            let Some(since) = environment.unused_since else {
                continue;
            };
            let days: i64 = {
                let mut conn = self.inner.db.acquire().await?;
                setting(
                    &mut conn,
                    SettingsPlace::Environment(environment.id),
                    keys::ENV_UNUSED_REMOVE_DAYS,
                )
                .await?
                .unwrap_or(0)
            };
            if days <= 0 || now - since < chrono::Duration::minutes(days * MINUTES_PER_DAY) {
                continue;
            }
            let removal = now + notice;
            // The schedule and the line in the history that says it are one act: nobody sees the one
            // without the other, and a failure to write the line schedules nothing.
            let mut tx = self.inner.db.begin().await?;
            orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
            if !self
                .schedule_environment_removal(&mut tx, environment.id, None, removal, now)
                .await?
            {
                continue;
            }
            orch_authz::epoch::bump(&mut tx).await?;
            let told = rows::managers_of(&mut tx, environment.id).await?;
            let summary = format!(
                "The environment {} has had no thread for {days} days; its removal is scheduled for {}",
                environment.name,
                removal.format("%Y-%m-%dT%H:%M:%SZ"),
            );
            let mut entry = Entry::new(
                Actor::System,
                REMOVAL_SCHEDULED,
                "environment",
                Some(environment.id),
                &summary,
            );
            entry.history = true;
            entry.node_id = Some(environment.home_node_id);
            entry.org_id = Some(environment.org_id);
            let promise =
                orch_db::queries::environments::retained::removal(&mut tx, environment.id).await?;
            entry.details = json!({
                "environment": environment.id,
                "reason": "unused",
                "at": removal,
                "told": told,
                "removal_revision":promise.as_ref().map(|p|p.removal_revision),
                "keep_days":promise.as_ref().and_then(|p|p.removed_keep_days),
            });
            audit::append(&mut tx, &entry, &Origin::default(), now).await?;
            tx.commit().await.map_err(DbError::from)?;
            tracing::info!(environment = %environment.id, "the removal of an unused environment is scheduled; its owner can keep it");
        }
        Ok(())
    }

    /// Starts, for the environments the schedule or a request brings up, the job the status asks for.
    /// A thin wrapper so that the modules of other parts need not know the job's name.
    pub async fn queue_start(&self, environment: &EnvironmentState) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let mode = if environment.status == "stopped" {
            provision::Mode::Start
        } else {
            provision::Mode::Make
        };
        match provision::queue(&mut tx, self.inner.clock.now(), environment, mode, None).await {
            Ok(_) | Err(MakeError::Running(_)) => {}
            Err(MakeError::NoEnvironment) => return Ok(()),
            Err(MakeError::Db(error)) => return Err(error.into()),
        }
        tx.commit().await.map_err(DbError::from)?;
        self.kick();
        Ok(())
    }
}

/// Whether the idle warning is due at `now` for a stop at `stops_at`: within the `minutes` before it, and not
/// once the stop is due (the stop's own notice says that). Zero minutes, or fewer, send none.
fn warning_due(now: DateTime<Utc>, stops_at: DateTime<Utc>, minutes: i64) -> bool {
    minutes > 0
        && now < stops_at
        && chrono::Duration::try_minutes(minutes)
            .and_then(|ahead| stops_at.checked_sub_signed(ahead))
            .is_none_or(|from| now >= from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn the_idle_warning_is_due_within_its_minutes_before_the_stop_and_never_when_set_to_none() {
        let stops_at = Utc.with_ymd_and_hms(2026, 10, 15, 14, 5, 0).unwrap();
        let before = |minutes| stops_at - chrono::Duration::minutes(minutes);
        assert!(!warning_due(before(61), stops_at, 60), "too early");
        assert!(
            warning_due(before(60), stops_at, 60),
            "exactly the setting's minutes ahead"
        );
        assert!(warning_due(before(1), stops_at, 60));
        assert!(
            !warning_due(stops_at, stops_at, 60),
            "at the stop, the stop's notice tells it"
        );
        assert!(!warning_due(before(1), stops_at, 0), "0 sends none");
        assert!(
            warning_due(before(1), stops_at, i64::MAX),
            "a warning longer than time can say is due at once"
        );
    }
}
