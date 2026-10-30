//! A durable answer to the node's startup/hourly kept-image inventory, retaining current and archived ownership.

use super::runner::{Done, Failure, Job, Phased, Stop, Undone};
use crate::{Engine, EngineError, ids};
use async_trait::async_trait;
use orch_types::{Id, Json};

pub const KIND: &str = "kept_images_sweep";
pub struct Sweeping {
    engine: Engine,
    host: Id,
    epoch: u64,
}
impl Sweeping {
    pub fn of(engine: Engine, row: &orch_db::queries::jobs::JobRow) -> Result<Self, EngineError> {
        Ok(Self {
            engine,
            host: row
                .plan
                .get("host")
                .and_then(Json::as_str)
                .and_then(|id| id.parse().ok())
                .ok_or_else(|| EngineError::unreadable("the sweep has no host"))?,
            epoch: row
                .plan
                .get("epoch")
                .and_then(Json::as_u64)
                .ok_or_else(|| EngineError::unreadable("the sweep has no inventory epoch"))?,
        })
    }
}
#[async_trait]
impl Phased for Sweeping {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        if phase != "sweep" {
            return Err(Failure::new("phase", "the kept-image sweep phase is unknown").into());
        }
        let command = self
            .engine
            .kept_sweep_command(
                self.host,
                self.epoch,
                ids::derived("node.kept.sweep", &[job.id().as_bytes()]),
            )
            .await?;
        job.command(command).await?;
        self.engine.cleanup_unused_caches(job, self.host).await?;
        Ok(Done::Finished)
    }
    async fn undo(&self, job: &Job, _why: &Stop) -> Undone {
        if job
            .plan()
            .await
            .ok()
            .and_then(|plan| plan["cleanup_results"].as_array().cloned())
            .is_some_and(|items| items.iter().any(|item| item["outcome"] == "removed"))
        {
            Undone::Partly("Unused caches already removed stay removed".into())
        } else {
            Undone::Everything
        }
    }
    async fn result(&self, job: &Job) -> Option<Json> {
        job.plan()
            .await
            .ok()
            .map(|plan| serde_json::json!({"cleanup":plan["cleanup_results"]}))
    }
    async fn partial_result(&self, job: &Job) -> Option<Json> {
        let plan = job.plan().await.ok()?;
        let results = plan.get("cleanup_results")?.as_array()?;
        Some(serde_json::json!({"cleanup": results}))
    }
}
