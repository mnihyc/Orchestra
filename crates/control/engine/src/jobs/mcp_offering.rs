//! A rediscovered native MCP definition is copied on its own machine before its retained resource is offered again.

use crate::{
    EngineError,
    commands::Outgoing,
    ids,
    jobs::runner::{Done, Failure, Job, Phase, Phased, Stop, Undone},
};
use async_trait::async_trait;
use orch_db::queries::{
    environments::hosting as envs,
    resources::{self, mcp as rows},
};
use orch_link_contract::commands::{McpImport, McpImportResult, SerialKey};
use orch_types::{Id, Json};
use serde::Deserialize;

/// The durable job kind created by a native discovery.
pub const KIND: &str = "mcp_offering";
/// Importing one protected copy is the whole operation, with its native command deadline.
pub const PHASES: [Phase; 1] = [Phase {
    name: "import",
    weight: 100,
}];

#[derive(Deserialize)]
struct Recipe {
    server: Id,
    machine: Id,
    environment: Id,
    source: Json,
}

/// One native definition copy, with no model call and no modification of its resource's page.
pub struct Offering {
    recipe: Recipe,
}
impl Offering {
    /// Reads the safe discovery metadata captured by the environment report.
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
        let mut conn = job.engine().db().begin().await?;
        rows::lock_definition(&mut conn, self.recipe.server).await?;
        let owns:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mcp_server_offerings WHERE server_id=$1 AND machine_id=$2 AND state='pending' AND job_id=$3)").bind(self.recipe.server).bind(self.recipe.machine).bind(job.id()).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
        if !owns {
            return Ok(Done::Skipped(
                "The offering was already reviewed by another act".into(),
            ));
        }

        let server = rows::servers(&mut conn, &[self.recipe.server])
            .await?
            .pop()
            .filter(|row| row.imported)
            .ok_or_else(|| Failure::new("not_found", "The imported resource is gone"))?;
        if server
            .spec
            .pointer("/imported/definition_fingerprint")
            .and_then(Json::as_str)
            != self.recipe.source["definition_fingerprint"].as_str()
            || server.spec["transport"] != self.recipe.source["transport"]
        {
            rows::found(
                &mut conn,
                server.id,
                self.recipe.machine,
                &self.recipe.source,
                "different_definition",
                job.now(),
            )
            .await?;
            conn.commit().await.map_err(orch_db::DbError::from)?;
            return Ok(Done::Skipped(
                "The canonical definition changed before this offering".into(),
            ));
        }
        let machine = resources::machine(&mut conn, self.recipe.machine)
            .await?
            .filter(|row| row.is_active())
            .ok_or_else(|| Failure::new("source_offline", "The offering machine is unavailable"))?;
        let environment = envs::environment(&mut conn, self.recipe.environment)
            .await?
            .filter(|row| {
                row.machine_id == Some(machine.id)
                    && row.status != "lost"
                    && row.deleted_at.is_none()
            })
            .ok_or_else(|| {
                Failure::new(
                    "source_offline",
                    "The discovered environment is unavailable",
                )
            })?;
        if server.home_id != Some(environment.org_id) {
            return Err(Failure::new(
                "access.denied",
                "The server and the new offering no longer have the same home",
            )
            .into());
        }
        let request = McpImport {
            expected_fingerprint: self
                .recipe
                .source
                .get("definition_fingerprint")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(EngineError::from)?,
            environment_id: environment.id,
            server_id: server.id,
            source: self.recipe.source["source"]
                .as_str()
                .unwrap_or_default()
                .into(),
            scope: self.recipe.source["scope"]
                .as_str()
                .unwrap_or_default()
                .into(),
            name: self.recipe.source["name"]
                .as_str()
                .unwrap_or_default()
                .into(),
            directory: self.recipe.source["directory"].as_str().map(str::to_owned),
        };
        let result = job
            .command(
                Outgoing::new(
                    ids::derived("mcp.offer", &[job.id().as_bytes()]),
                    machine.id,
                    SerialKey::Environment(environment.id),
                    &request,
                )
                .map_err(EngineError::from)?,
            )
            .await?;
        let result: McpImportResult = serde_json::from_value(result).map_err(EngineError::from)?;
        let mut source = self.recipe.source.clone();
        source["digest"] = serde_json::json!(result.digest);
        source["tools"] = serde_json::json!(result.tools);
        let mut tx = conn;
        if job.lock_owned(&mut tx).await?.is_none() {
            return Err(Stop::Lost);
        }
        let current = resources::machine(&mut tx, machine.id)
            .await?
            .filter(|row| row.is_active())
            .ok_or_else(|| Failure::new("source_offline", "The offering machine was revoked"))?;
        source["definition_fingerprint"] =
            serde_json::to_value(result.definition_fingerprint).map_err(EngineError::from)?;
        source["transport"] = serde_json::json!(result.transport);
        let current_server = rows::lock(&mut tx, server.id)
            .await?
            .ok_or_else(|| Failure::new("not_found", "The imported resource is gone"))?;
        let expected = current_server
            .spec
            .pointer("/imported/definition_fingerprint")
            .and_then(Json::as_str);
        let same = expected.is_some()
            && source["definition_fingerprint"].as_str() == expected
            && current_server.spec["transport"] == source["transport"]
            && current_server.home_id == Some(environment.org_id);
        if same {
            rows::offer(
                &mut tx,
                server.id,
                current.id,
                Some(environment.id),
                &source,
                job.now(),
            )
            .await?;
        } else {
            rows::found(
                &mut tx,
                server.id,
                current.id,
                &source,
                "different_definition",
                job.now(),
            )
            .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Done::Finished)
    }
    async fn undo(&self, job: &Job, reason: &Stop) -> Undone {
        let why = match reason {
            Stop::Failed(failure) => failure.message.as_str(),
            _ => "The native copy was not made",
        };
        if let Ok(mut conn) = job.engine().db().acquire().await {
            let _ = rows::offer_failed(
                &mut conn,
                self.recipe.server,
                self.recipe.machine,
                job.id(),
                why,
                job.now(),
            )
            .await;
        }
        Undone::Everything
    }
    async fn result(&self, _job: &Job) -> Option<Json> {
        Some(serde_json::json!({"server_id":self.recipe.server,"machine_id":self.recipe.machine}))
    }
}
