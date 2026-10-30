//! Environment removal acceptance freezes every owned volume and its locked retention policy.

use crate::{Engine, error::EngineError};
use chrono::{DateTime, Utc};
use orch_db::{
    Conn,
    queries::environments::{hosting, retained},
};
use orch_settings::{Place, resolve};
use orch_settings_contract::{provenance::SourceScope, registry::keys};
use orch_types::Id;

/// Seconds in one UTC retention day.
const SECONDS_PER_DAY: u64 = 86_400;

impl Engine {
    /// Removes a lost registration without reopening a closed physical cleanup promise or contacting its source.
    pub async fn remove_lost_environment_in(
        &self,
        conn: &mut Conn,
        environment: Id,
        by: Id,
        now: DateTime<Utc>,
    ) -> Result<bool, EngineError> {
        let row = hosting::lock_environment(conn, environment)
            .await?
            .ok_or_else(|| EngineError::unreadable("the environment is gone"))?;
        if row.status != "lost" || row.deleted_at.is_some() {
            return Ok(false);
        }
        self.remove_bindings_in(conn, environment, by, orch_types::Time::new(now))
            .await?;
        sqlx::query("UPDATE environments SET status='removed',deleted_at=$2,remove_at=$2,remove_reason='request',removal_phase='lost',volumes_purge_at=NULL,revision=revision+1,env_seq=env_seq+1,updated_at=$2 WHERE id=$1")
            .bind(environment).bind(now).execute(conn).await.map_err(orch_db::DbError::from)?;
        Ok(true)
    }

    /// Schedules removal and freezes its policy and exact owned references in the same transaction.
    pub async fn schedule_environment_removal(
        &self,
        conn: &mut Conn,
        environment: Id,
        by: Option<Id>,
        at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<bool, EngineError> {
        self.schedule_environment_removal_with_days(conn, environment, by, (at, now), None)
            .await
    }

    /// Shares the retained-file acceptance path with an org's complete recovery window.
    pub(crate) async fn schedule_environment_removal_with_days(
        &self,
        conn: &mut Conn,
        environment: Id,
        by: Option<Id>,
        (at, now): (DateTime<Utc>, DateTime<Utc>),
        recovery_days: Option<i64>,
    ) -> Result<bool, EngineError> {
        let row = hosting::lock_environment(conn, environment)
            .await?
            .ok_or_else(|| EngineError::unreadable("the environment is gone"))?;
        if row.deleted_at.is_some() {
            return Ok(false);
        }
        let existing = retained::removal(conn, environment).await?;
        if existing.as_ref().is_some_and(|r| r.removal_phase.is_some()) {
            return Ok(false);
        }
        let days = if retained::is_scratch(conn, environment).await?
            || orch_db::queries::environments::temporary::mode(&row.config).is_some()
        {
            0
        } else {
            let policy = resolve(
                conn,
                Place::Environment(environment),
                keys::ENV_REMOVED_KEEP_DAYS,
            )
            .await?;
            let maximum = i64::try_from(self.inner.config.soft_delete.as_secs() / SECONDS_PER_DAY)
                .unwrap_or(i64::MAX);
            let requested = match recovery_days {
                Some(days) => days,
                None => policy
                    .value()
                    .and_then(orch_types::Json::as_i64)
                    .ok_or_else(|| {
                        EngineError::unreadable("removed-file retention is not a number")
                    })?,
            };
            let days = if policy.resolved.source.scope == SourceScope::Default {
                requested.min(maximum)
            } else {
                requested
            };
            if days < 0 || days > maximum {
                return Err(EngineError::unreadable(
                    "removed-file retention exceeds the system retention window",
                ));
            }
            i32::try_from(days).map_err(EngineError::unreadable)?
        };
        let refs = retained::references(conn, environment).await?;
        if days > 0 {
            for reference in &refs {
                let host = orch_db::queries::resources::hosting::host(conn, reference.host_id)
                    .await?
                    .ok_or_else(|| {
                        EngineError::unreadable("the host keeping the files is unavailable")
                    })?;
                if !host
                    .drivers
                    .get(&reference.driver)
                    .and_then(|d| d.get("volumes"))
                    .and_then(orch_types::Json::as_bool)
                    .unwrap_or(false)
                {
                    return Err(EngineError::unreadable(
                        "this driver cannot remove a unit while keeping its owned files",
                    ));
                }
            }
        }
        if !hosting::schedule_removal(conn, environment, at, now).await? && row.remove_at.is_none()
        {
            return Ok(false);
        }
        Ok(retained::freeze(conn, environment, by, days, &refs, now).await?)
    }
}

impl Engine {
    /// Claims eligible kept files; a restore reservation and a changing job exclude this sweep.
    pub(crate) async fn purge_due_kept(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let ids = {
            let mut conn = self.db().acquire().await?;
            retained::due(&mut conn, now, self.inner.config.sweep_batch).await?
        };
        for id in ids {
            let mut tx = self.db().begin().await?;
            orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
            hosting::lock_environment(&mut tx, id).await?;
            let Some(promise) = retained::removal(&mut tx, id).await? else {
                continue;
            };
            let retry = promise.removal_phase.as_deref() == Some("purging");
            if retry {
                // Candidates were selected before this lock: a failed attempt may have scheduled its retry
                // meanwhile. Keep that backoff rather than immediately starting another purge.
                let due: bool = sqlx::query_scalar(
                    "SELECT COALESCE(remove_at <= $2,false) FROM environments WHERE id=$1",
                )
                .bind(id)
                .bind(now)
                .fetch_one(&mut *tx)
                .await
                .map_err(orch_db::DbError::from)?;
                if !due {
                    continue;
                }
            }
            if !retry
                && (promise.removal_phase.as_deref() != Some("kept")
                    || promise.volumes_purge_at.is_none_or(|at| at > now))
            {
                continue;
            }
            match crate::jobs::retained::queue(
                &mut tx,
                id,
                promise.removal_revision,
                true,
                None,
                orch_types::Json::Null,
                now,
            )
            .await
            {
                Ok(_) => {
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                    self.kick();
                }
                Err(
                    crate::jobs::runner::MakeError::Running(_)
                    | crate::jobs::runner::MakeError::NoEnvironment,
                ) => {}
                Err(crate::jobs::runner::MakeError::Db(error)) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

impl Engine {
    /// Uses the existing attention notice for unresolved retained data, with current recipients.
    pub(crate) async fn tell_retained_failure(
        &self,
        original: &orch_db::queries::jobs::JobRow,
    ) -> Result<(), EngineError> {
        if original.plan.get("notification_operation").is_some() {
            return Ok(());
        }

        use orch_db::queries::{access, hierarchy, jobs};
        let mut tx = self.db().begin().await?;
        let Some(row) = jobs::job(&mut tx, original.id).await? else {
            return Ok(());
        };
        if row.state != "failed"
            || row.plan.get("scratch").and_then(orch_types::Json::as_bool) == Some(true)
        {
            return Ok(());
        }
        let Some(id) = row.environment_id else {
            return Ok(());
        };
        let Some(promise) = retained::removal(&mut tx, id)
            .await?
            .filter(|promise| promise.removal_phase.is_some())
        else {
            return Ok(());
        };
        let Some(environment) = hosting::environment(&mut tx, id).await? else {
            return Ok(());
        };
        let home = hierarchy::with_ancestors(&mut tx, &[environment.home_node_id]).await?;
        let present = home
            .iter()
            .find(|n| n.id == environment.home_node_id)
            .filter(|n| n.deleted_at.is_none());
        let candidates = if let Some(home) = present {
            access::admin_people(&mut tx, &home.path, self.inner.clock.now(), true).await?
        } else {
            access::system_admin_people(&mut tx, 1_000).await?
        };
        let mut recipients = Vec::new();
        for person in candidates {
            // Being told is no act: the home's admins are those who administer it when they act as
            // admins, which a person does only in the session's As admin mode.
            let principal = match orch_authz::Principal::load(
                &mut tx,
                orch_authz::Caller::session(person).with_admin_mode(true),
                orch_types::Time::new(self.inner.clock.now()),
            )
            .await
            {
                Ok(p) => p,
                Err(_) => continue,
            };
            if present.is_none() {
                if principal.is_system_admin() {
                    recipients.push(person);
                }
            } else if crate::jobs::retained::authority(&mut tx, &principal, id)
                .await?
                .1
            {
                recipients.push(person);
            }
        }
        let event=crate::notifications::Event {
            kind:crate::notifications::Kind::EnvironmentAttention,
            node:present.map(|n|n.id),thread:None,approval:None,
            title:format!("{}: its kept files need attention",environment.name),
            body:"The host did not complete the operation. The kept files and their deadline remain recorded; physical removal has not been confirmed.".to_owned(),
            data:serde_json::json!({"environment":id,"job":row.id,"phase":row.error.as_ref().and_then(|e|e.get("phase")),"path":format!("/environments/{id}")}),
            source:format!("retained:{}:{}:{}",id,promise.removal_revision,row.kind),actor:None,at:self.inner.clock.now(),
        };
        crate::notifications::tell(
            &mut tx,
            &event,
            &recipients,
            present.map(|_| environment.org_id),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }
}
