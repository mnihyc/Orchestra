//! One environment MCP server restarted with its own budget cleared, then offered tools refreshed without touching a turn.

use async_trait::async_trait;
use orch_access_contract::audit::AuditAction;
use orch_db::{
    Conn,
    queries::{environments::hosting as environments, jobs},
};
use orch_link_contract::commands::{McpRestart, McpRestartResult, SerialKey};
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::commands::Outgoing;
use crate::jobs::runner::{self, Done, Failure, Job, Phase, Phased, Stop, Undone};
use crate::{Engine, EngineError};

/// This job never takes the environment-wide or thread holds used by recreation.
pub const KIND: &str = "mcp_restart";
const PHASES: [Phase; 3] = [
    Phase {
        name: "restart",
        weight: 70,
    },
    Phase {
        name: "agents",
        weight: 20,
    },
    Phase {
        name: "record",
        weight: 10,
    },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Recipe {
    environment: Id,
    generation: i32,
    machine: Id,
    server: Id,
    name: String,
    by: Id,
}

fn fail(message: impl Into<String>) -> Stop {
    Stop::Failed(Failure::new("mcp_restart_unavailable", message))
}

impl Engine {
    /// Queues the named enabled server only. HTTP has already required the environment's Manage door.
    pub async fn queue_mcp_restart(
        &self,
        conn: &mut Conn,
        environment: Id,
        name: &str,
        by: Id,
    ) -> Result<jobs::JobRow, EngineError> {
        let env = environments::lock_environment(conn, environment)
            .await?
            .filter(|env| env.deleted_at.is_none())
            .ok_or_else(|| EngineError::unreadable("the environment is gone"))?;
        if env.status != "running" {
            return Err(EngineError::unreadable(
                "the environment must be running to restart its server",
            ));
        }
        let server = crate::mcp::resolve(conn, environment, Time::new(self.inner.clock.now()))
            .await?
            .into_iter()
            .find(|server| server.row.name == name)
            .ok_or_else(|| EngineError::unreadable("there is no enabled server of that name"))?;
        if let Some(reason) = server.reason {
            return Err(EngineError::unreadable(reason));
        }
        let recipe = Recipe {
            environment,
            generation: env.generation,
            machine: env
                .machine_id
                .ok_or_else(|| EngineError::unreadable("the environment has no connector"))?,
            server: server.row.id,
            name: name.into(),
            by,
        };
        runner::make(
            conn,
            &runner::Making {
                kind: KIND,
                environment,
                plan: serde_json::to_value(recipe)?,
                phases: &PHASES,
                created_by: Some(by),
            },
            self.inner.clock.now(),
        )
        .await
        .map_err(EngineError::unreadable)
    }
}

pub(crate) struct Restarting {
    recipe: Recipe,
}

impl Restarting {
    pub(crate) fn of(row: &jobs::JobRow) -> Result<Self, EngineError> {
        Ok(Self {
            recipe: serde_json::from_value(row.plan.clone())?,
        })
    }

    async fn restart(&self, job: &Job) -> Result<Done, Stop> {
        let mut conn = job
            .engine()
            .db()
            .acquire()
            .await
            .map_err(EngineError::from)?;
        let env = environments::environment(&mut conn, self.recipe.environment)
            .await?
            .ok_or_else(|| fail("the environment is gone"))?;
        if env.machine_id != Some(self.recipe.machine)
            || env.generation != self.recipe.generation
            || env.status != "running"
        {
            return Err(fail(
                "the environment changed; restart the server on its current machine",
            ));
        }
        let server = crate::mcp::resolve(&mut conn, self.recipe.environment, Time::new(job.now()))
            .await?
            .into_iter()
            .find(|server| {
                server.row.id == self.recipe.server && server.row.name == self.recipe.name
            })
            .ok_or_else(|| fail("the named server is no longer enabled"))?;
        if let Some(reason) = &server.reason {
            return Err(fail(reason.clone()));
        }
        let plan = job.plan().await?;
        let body = match plan.get("restart_body") {
            Some(body) => serde_json::from_value(body.clone()).map_err(EngineError::from)?,
            None => {
                let body = McpRestart(
                    crate::mcp::restart_request(&mut conn, self.recipe.environment, &server)
                        .await?,
                );
                job.remember(&json!({"restart_body":body})).await?;
                body
            }
        };
        drop(conn);
        // Stopping and starting this one process is one operation; cancellation may still cancel a queued job.
        job.pass_commit().await?;
        let value = job
            .command(
                Outgoing::new(
                    crate::ids::derived("mcp.restart", &[job.id().as_bytes()]),
                    self.recipe.machine,
                    SerialKey::Environment(self.recipe.environment),
                    &body,
                )
                .map_err(EngineError::from)?,
            )
            .await?;
        let result: McpRestartResult = serde_json::from_value(value).map_err(EngineError::from)?;
        if !result
            .servers
            .iter()
            .any(|server| server.name == self.recipe.name && server.state == "running")
        {
            return Err(fail("the named server did not come back running"));
        }
        let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
        let changed = sqlx::query("UPDATE environments SET facts=jsonb_set(facts,'{mcp_servers}',$4,true) WHERE id=$1 AND machine_id=$2 AND generation=$3")
            .bind(self.recipe.environment).bind(self.recipe.machine).bind(self.recipe.generation).bind(serde_json::to_value(result.servers).map_err(EngineError::from)?).execute(&mut *tx).await.map_err(orch_db::DbError::from)?.rows_affected();
        if changed != 1 {
            return Err(fail("the environment moved before the restart finished"));
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Done::Finished)
    }

    async fn record(&self, job: &Job) -> Result<Done, Stop> {
        let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
        let row = jobs::lock(&mut tx, job.id())
            .await?
            .ok_or_else(|| fail("the restart job is gone"))?;
        if row.plan.get("recorded") != Some(&json!(true)) {
            let env = environments::environment(&mut tx, self.recipe.environment)
                .await?
                .ok_or_else(|| fail("the environment is gone"))?;
            let action = orch_db::audit::action!(AuditAction::McpRestarted);
            let summary = format!("Restarted MCP server {}", self.recipe.name);
            let mut entry = orch_db::audit::Entry::new(
                orch_db::audit::Actor::User(self.recipe.by),
                action.as_str(),
                "environment",
                Some(self.recipe.environment),
                &summary,
            );
            entry.environment_id = Some(self.recipe.environment);
            entry.org_id = Some(env.org_id);
            entry.node_id = Some(env.home_node_id);
            entry.history = true;
            entry.details =
                json!({"server":self.recipe.server,"name":self.recipe.name,"job":job.id()});
            orch_db::audit::append(
                &mut tx,
                &entry,
                &orch_db::audit::Origin::default(),
                job.now(),
            )
            .await?;
            jobs::merge_plan(&mut tx, job.id(), &json!({"recorded":true}), job.now()).await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Done::Finished)
    }
}

#[async_trait]
impl Phased for Restarting {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        match phase {
            "restart" => self.restart(job).await,
            "agents" => {
                job.engine()
                    .reload_mcp(self.recipe.environment, job.id())
                    .await?;
                Ok(Done::Finished)
            }
            "record" => self.record(job).await,
            _ => Err(fail("the restart job has no such phase")),
        }
    }
    async fn undo(&self, _: &Job, _: &Stop) -> Undone {
        Undone::Everything
    }
    async fn result(&self, _: &Job) -> Option<Json> {
        Some(json!({"server":self.recipe.server,"name":self.recipe.name}))
    }
}
