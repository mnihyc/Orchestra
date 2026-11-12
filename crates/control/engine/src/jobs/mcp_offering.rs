//! Retained discovery jobs become import candidates without copying native definitions.

use crate::{
    EngineError,
    jobs::runner::{Done, Failure, Job, Phase, Phased, Stop, Undone},
};
use async_trait::async_trait;
use orch_db::queries::resources::mcp as rows;
use orch_types::{Id, Json};
use serde::Deserialize;

/// The retained job kind; discovery no longer creates new jobs of this kind.
pub const KIND: &str = "mcp_offering";
/// Settle its original metadata phase without issuing another native operation.
pub const PHASES: [Phase; 1] = [Phase {
    name: "import",
    weight: 100,
}];

#[derive(Deserialize)]
struct Recipe {
    server: Id,
    machine: Id,
    source: Json,
}

/// Keeps an older discovered candidate visible for an explicit import.
pub struct Offering {
    recipe: Recipe,
}
impl Offering {
    /// Reads the safe discovery metadata retained by the original job.
    pub fn of(row: &orch_db::queries::jobs::JobRow) -> Result<Self, EngineError> {
        Ok(Self {
            recipe: serde_json::from_value(row.plan.clone())?,
        })
    }
}

#[async_trait]
impl Phased for Offering {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        if phase != "import" {
            return Err(Failure::new("invalid_plan", "The offering has no such phase").into());
        }
        let mut tx = job.engine().db().begin().await?;
        rows::lock_definition(&mut tx, self.recipe.server).await?;
        if job.lock_owned(&mut tx).await?.is_none() {
            return Err(Stop::Lost);
        }
        let owns: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mcp_server_offerings WHERE server_id=$1 AND machine_id=$2 AND state='pending' AND job_id=$3 AND source=$4)")
            .bind(self.recipe.server).bind(self.recipe.machine).bind(job.id()).bind(&self.recipe.source)
            .fetch_one(&mut *tx).await.map_err(orch_db::DbError::from)?;
        if owns {
            rows::found(
                &mut tx,
                self.recipe.server,
                self.recipe.machine,
                &self.recipe.source,
                "import_required",
                job.now(),
            )
            .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Done::Skipped(
            "The discovered definition waits for an explicit import".into(),
        ))
    }
    async fn undo(&self, _job: &Job, _reason: &Stop) -> Undone {
        // No new native operation was admitted; any older command's actual
        // outcome remains retained in its original command journal.
        Undone::Everything
    }
    async fn result(&self, _job: &Job) -> Option<Json> {
        Some(
            serde_json::json!({"server_id":self.recipe.server,"machine_id":self.recipe.machine,"requires_import":true}),
        )
    }
}
