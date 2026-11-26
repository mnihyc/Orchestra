//! Current environment preparation comes only from an accepted job or an issued native attach.
use crate::{engine::Engine, error::EngineError, unit::Unit};
use orch_api_contract::threads::EnvironmentPreparationKind;
use orch_db::{Conn, queries::threads};
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};

/// Internal receipt; directory and resource names are projected by the existing current reader.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    /// The operation actually accepted by its producer.
    pub kind: EnvironmentPreparationKind,
    /// The real provision job, where there is one.
    pub actual_job: Option<Id>,
    /// The issued attach command, never a synthetic job.
    pub actual_command: Option<Id>,
    /// When the native work was accepted or issued.
    pub observed_at: Time,
    generation: i32,
}

/// Reads the original selected environment association, never a later replacement's facts.
pub async fn of_thread(
    conn: &mut Conn,
    thread: Id,
    environment: Id,
) -> Result<Option<Observation>, EngineError> {
    let selected: Option<(Option<Json>, i32)> = sqlx::query_as(
        "SELECT e.environment_preparation,e.generation FROM threads t JOIN environments e ON e.id=t.environment_id
        WHERE t.node_id=$1 AND t.environment_id=$2 AND t.state='starting' AND t.starting_step='environment'
        AND e.deleted_at IS NULL AND e.remove_at IS NULL AND e.status NOT IN ('failed','disabled','removed','lost')")
        .bind(thread).bind(environment).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let Some((Some(stored), generation)) = selected else {
        return Ok(None);
    };
    let Ok(observation) = serde_json::from_value::<Observation>(stored) else {
        return Ok(None);
    };
    if let Some(job) = observation.actual_job {
        let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE id=$1 AND environment_id=$2 AND kind='environment_provision' AND state IN ('queued','running'))")
            .bind(job).bind(environment).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
        if !current {
            return Ok(None);
        }
    } else if observation.actual_command.is_none() || observation.generation != generation {
        return Ok(None);
    }
    Ok(Some(observation))
}

/// This receipt commits in the same transaction as the job which accepts this native operation.
pub(crate) async fn accepted_job(
    conn: &mut Conn,
    environment: Id,
    generation: i32,
    kind: EnvironmentPreparationKind,
    job: Id,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), EngineError> {
    let value = Observation {
        kind,
        generation,
        actual_job: Some(job),
        actual_command: None,
        observed_at: Time::new(at),
    };
    let value = serde_json::to_value(value)?;
    sqlx::query("UPDATE environments SET environment_preparation=$2 WHERE id=$1")
        .bind(environment)
        .bind(value)
        .execute(conn)
        .await
        .map_err(orch_db::DbError::from)?;
    Ok(())
}

impl Engine {
    /// The command has been issued; generic file commands never enter this producer.
    pub(crate) async fn record_environment_attach(
        &self,
        command: &crate::commands::Outgoing,
    ) -> Result<(), EngineError> {
        let body: orch_link_contract::commands::EnvAttach =
            serde_json::from_value(command.body.clone())?;
        let mut tx = self.db().begin().await?;
        let at = self.inner.clock.now();
        let changed = sqlx::query("UPDATE environments SET environment_preparation=jsonb_build_object(
                'kind','attach','actual_job',NULL,'actual_command',$2::uuid,'observed_at',$3::timestamptz,'generation',generation)
            WHERE id=$1 AND placement='connected' AND machine_id=$4 AND deleted_at IS NULL AND remove_at IS NULL
                AND ($5::text IS NULL OR workspace_root=$5) AND status NOT IN ('disabled','removed','lost')")
            .bind(body.environment_id).bind(command.id).bind(at).bind(command.machine).bind(body.workspace_root)
            .execute(&mut *tx).await.map_err(orch_db::DbError::from)?.rows_affected();
        if changed > 0 {
            orch_db::queries::jobs::stamp_environment(&mut tx, body.environment_id, at).await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        if changed == 0 {
            return Ok(());
        }
        // Release the environment lock before publishing thread metadata, preserving thread lock order.
        let mut tx = self.db().begin().await?;
        let ids: Vec<Id> = sqlx::query_scalar("SELECT node_id FROM threads WHERE environment_id=$1 AND state='starting' AND starting_step='environment' ORDER BY node_id")
            .bind(body.environment_id).fetch_all(&mut *tx).await.map_err(orch_db::DbError::from)?;
        let mut unit = Unit::new(&mut tx, at);
        for id in ids {
            if let Some(row) = threads::lock(unit.conn, id).await?
                && row.environment_id == Some(body.environment_id)
            {
                let seq = unit.stamp(id).await?;
                threads::set_seq_state(unit.conn, id, seq).await?;
            }
        }
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(())
    }
}
