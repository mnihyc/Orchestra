//! Expired expansion details release space while native choices, replay and health remain intact.
use crate::{Engine, EngineError};
use chrono::{DateTime, Utc};
use orch_db::{Conn, DbError, queries::job_leases};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};

#[derive(sqlx::FromRow)]
struct RetainedChoice {
    target: Json,
    source: Json,
    updated_at: DateTime<Utc>,
}

fn source_id(source: &Json, key: &str) -> Option<Id> {
    source.get(key)?.as_str()?.parse().ok()
}

async fn expire(conn: &mut Conn, id: Id, now: DateTime<Utc>) -> Result<(), EngineError> {
    let Some(row) = sqlx::query_as::<_, RetainedChoice>(
        "SELECT target,source,updated_at FROM pool_choices WHERE id=$1 AND state<>'building' AND details_expired_at IS NULL FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)? else {
        return Ok(());
    };
    let pool = if row.target["type"] == "pool" {
        source_id(&row.target, "id")
    } else {
        None
    };
    let days = match pool {
        Some(pool) => {
            match get::<i64>(conn, Place::Pool(pool), keys::POOLS_CHOICE_DETAIL_DAYS).await {
                Ok(days) => days,
                Err(orch_settings::Error::NotFound) => {
                    get::<i64>(conn, Place::System, keys::POOLS_CHOICE_DETAIL_DAYS).await?
                }
                Err(error) => return Err(error.into()),
            }
        }
        None => get::<i64>(conn, Place::System, keys::POOLS_CHOICE_DETAIL_DAYS).await?,
    }
    .ok_or_else(|| EngineError::unreadable("the pool choice retention default is absent"))?;
    if days <= 0
        || chrono::Duration::try_days(days)
            .and_then(|age| now.checked_sub_signed(age))
            .is_none_or(|cutoff| row.updated_at > cutoff)
    {
        return Ok(());
    }
    // Original headers and paths remain usable even after their detail pages expire. Keep
    // the pages too while an associated native job, turn or health attempt is still open.
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pool_attempt_scopes WHERE choice_id=$1 AND settled_at IS NULL)
         OR EXISTS(SELECT 1 FROM jobs WHERE id=$2 AND state IN ('queued','running'))
         OR EXISTS(SELECT 1 FROM jobs WHERE environment_id=$3 AND state IN ('queued','running'))
         OR EXISTS(SELECT 1 FROM turns WHERE thread_id=$4 AND state IN ('starting','running'))",
    )
    .bind(id)
    .bind(source_id(&row.source, "job"))
    .bind(source_id(&row.source, "environment"))
    .bind(source_id(&row.source, "node"))
    .fetch_one(&mut *conn)
    .await
    .map_err(DbError::from)?;
    if active {
        return Ok(());
    }
    sqlx::query("DELETE FROM pool_choice_skips WHERE choice_id=$1")
        .bind(id)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("DELETE FROM pool_choice_members WHERE choice_id=$1")
        .bind(id)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("DELETE FROM pool_choice_nodes WHERE choice_id=$1")
        .bind(id)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("UPDATE pool_choices SET details_expired_at=$2 WHERE id=$1")
        .bind(id)
        .bind(now)
        .execute(conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

impl Engine {
    /// Page through outstanding detail receipts under a renewable lease and commit each page.
    pub(crate) async fn keep_pool_choice_details(&self) -> Result<(), EngineError> {
        let lease = job_leases::Lease::for_run("pool-choice-details", crate::schedule::RUN_LEASE);
        if !lease.hold(self.db(), self.inner.clock.now()).await? {
            return Ok(());
        }
        let result = self.keep_pool_choice_details_held(&lease).await;
        lease.release(self.db()).await?;
        result
    }

    async fn keep_pool_choice_details_held(
        &self,
        lease: &job_leases::Lease,
    ) -> Result<(), EngineError> {
        let batch = {
            let mut conn = self.db().acquire().await?;
            get::<i64>(&mut conn, Place::System, keys::POOLS_READ_BATCH_SIZE)
                .await?
                .ok_or_else(|| EngineError::unreadable("the pool read batch default is absent"))?
        };
        let mut after = None;
        loop {
            if !lease.hold(self.db(), self.inner.clock.now()).await? {
                break;
            }
            let ids: Vec<Id> = {
                let mut conn = self.db().acquire().await?;
                sqlx::query_scalar("SELECT id FROM pool_choices WHERE state<>'building' AND details_expired_at IS NULL AND ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT $2")
                    .bind(after).bind(batch).fetch_all(&mut *conn).await.map_err(DbError::from)?
            };
            if ids.is_empty() {
                break;
            }
            after = ids.last().copied();
            let mut tx = self.db().begin().await?;
            if lease
                .fence(&mut tx, self.inner.clock.as_ref())
                .await?
                .is_none()
            {
                break;
            }
            for id in ids {
                expire(&mut tx, id, self.inner.clock.now()).await?;
            }
            tx.commit().await.map_err(DbError::from)?;
        }
        Ok(())
    }
}
