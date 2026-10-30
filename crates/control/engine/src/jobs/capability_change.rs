//! Durable live capability decisions, applied on the prepared unit before they are announced as available.

use super::runner::{Done, Failure, Job, MakeError, Making, Phase, Phased, Stop, Undone, make};
use crate::{Engine, EngineError, commands::Outgoing, ids, unit::Unit};
use async_trait::async_trait;
use orch_db::queries::{
    environments::{capabilities, hosting as environments},
    jobs,
    resources::hosting as hosts,
};
use orch_driver_contract::capabilities::CapabilityState;
use orch_link_contract::commands::{NodeCapabilitiesApply, NodeCapabilitiesApplyResult, SerialKey};
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const KIND: &str = "capability_change";
const PHASES: [Phase; 2] = [
    Phase {
        name: "apply",
        weight: 90,
    },
    Phase {
        name: "commit",
        weight: 10,
    },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Plan {
    pub runtime: Id,
    pub capability: String,
    pub grant: bool,
    pub before: CapabilityState,
    pub after: CapabilityState,
    pub requester: Option<Id>,
    pub decided_by: Option<Id>,
}

pub(crate) async fn queue(
    conn: &mut orch_db::Conn,
    environment: Id,
    plan: &Plan,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<jobs::JobRow, MakeError> {
    make(
        conn,
        &Making {
            kind: KIND,
            environment,
            plan: json!(plan),
            phases: &PHASES,
            created_by: plan.decided_by,
        },
        now,
    )
    .await
}

pub struct Applying {
    engine: Engine,
    environment: Id,
    plan: Plan,
}
impl Applying {
    pub fn of(engine: Engine, row: &jobs::JobRow) -> Result<Self, EngineError> {
        Ok(Self {
            engine,
            environment: row
                .environment_id
                .ok_or_else(|| EngineError::unreadable("the capability job has no environment"))?,
            plan: serde_json::from_value(row.plan.clone())?,
        })
    }

    async fn command(&self, job: &Job, undo: bool) -> Result<(), Stop> {
        let mut conn = self
            .engine
            .db()
            .acquire()
            .await
            .map_err(EngineError::from)?;
        let current = environments::live_runtime(&mut conn, self.environment)
            .await?
            .ok_or_else(|| {
                Failure::new("gone", "the environment no longer has its prepared unit")
            })?;
        if current.id != self.plan.runtime {
            return Err(Failure::new(
                "changed",
                "the environment's unit changed before the capability was applied",
            )
            .into());
        }
        let host = hosts::host(&mut conn, current.host_id)
            .await?
            .ok_or_else(|| Failure::new("no_host", "the prepared unit's host is gone"))?;
        if !undo && self.plan.grant {
            let facts = capabilities::environment_facts(&mut conn, self.environment)
                .await?
                .ok_or_else(|| Failure::new("gone", "the environment is gone"))?;
            if facts.deleted_at.is_some() {
                return Err(Failure::new("gone", "the environment is removed").into());
            }
            let definition = capabilities::definition(&mut conn, &self.plan.capability)
                .await?
                .ok_or_else(|| Failure::new("gone", "the capability definition is gone"))?;
            if self
                .engine
                .capability_policy(&mut conn, facts.home_node_id, &definition)
                .await?
                .decision
                == "deny"
            {
                return Err(Failure::new(
                    "denied",
                    "the current capability policy denies this grant",
                )
                .into());
            }
        }
        drop(conn);
        let body = NodeCapabilitiesApply {
            runtime_id: self.plan.runtime,
            capabilities: json!(if undo && self.plan.grant {
                &self.plan.before
            } else {
                &self.plan.after
            }),
        };
        let command = Outgoing::new(
            ids::derived(
                if undo {
                    "capability.live.undo"
                } else {
                    "capability.live.apply"
                },
                &[job.id().as_bytes()],
            ),
            host.machine_id,
            SerialKey::Runtime(self.plan.runtime),
            &body,
        )
        .map_err(EngineError::from)?;
        let reply = if undo {
            job.command_to_undo(command).await?
        } else {
            job.command(command).await?
        };
        let reply: NodeCapabilitiesApplyResult =
            serde_json::from_value(reply).map_err(EngineError::from)?;
        if !reply.applied {
            let reason = reply.reason.unwrap_or_else(|| {
                "the node could not apply this capability; recreate is needed".into()
            });
            return Err(Failure::new(
                "live_unavailable",
                if reply.restarted {
                    format!(
                        "{reason}. The environment restarted to restore its previous permissions."
                    )
                } else {
                    reason
                },
            )
            .into());
        }
        Ok(())
    }
}

/// Keep the application result with the terminal job transaction, including after a runner crashes.
/// Approval records consent; this separate result says whether the capability actually became available.
pub(crate) async fn finished(
    conn: &mut orch_db::Conn,
    row: &jobs::JobRow,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), EngineError> {
    if row.kind != KIND {
        return Ok(());
    }
    let plan: Plan = serde_json::from_value(row.plan.clone())?;
    let failed = matches!(row.state.as_str(), "failed" | "cancelled" | "lost");
    let reason = row
        .error
        .as_ref()
        .and_then(|error| error.get("message"))
        .and_then(Json::as_str)
        .unwrap_or("the capability change did not finish");
    let mut unit = Unit::new(conn, now);
    if let Some(approval) = row
        .plan
        .get("approval")
        .and_then(Json::as_str)
        .and_then(|id| id.parse::<Id>().ok())
    {
        sqlx::query("UPDATE approvals SET decision=COALESCE(decision,'{}'::jsonb) || jsonb_build_object('application',$2::jsonb) WHERE id=$1 AND kind='capability'")
            .bind(approval)
            .bind(json!({"job_id":row.id,"state":if failed {"failed"} else {"applied"},"reason":if failed {Some(reason)} else {None}}))
            .execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        if let Some(person) = plan.decided_by {
            unit.approvals_changed(&[person]);
        }
    }
    if failed && plan.grant {
        let label = capabilities::definition(&mut *unit.conn, &plan.capability)
            .await?
            .map_or(plan.capability, |definition| definition.label);
        Engine::system_input(&mut unit, plan.requester, &format!(
            "The capability \"{label}\" was not granted: {reason}. Request it again after the host's manager has resolved this failure."
        )).await?;
    }
    // finish queues PostgreSQL notifications on this transaction; no thread is woken for a failed grant.
    let _ = unit.finish().await?;
    Ok(())
}

#[async_trait]
impl Phased for Applying {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        match phase {
            "apply" => {
                self.command(job, false).await?;
                job.remember(&json!({"applied":true})).await?;
            }
            "commit" => {
                if job.plan().await?.get("committed").and_then(Json::as_bool) == Some(true) {
                    return Ok(Done::Finished);
                }
                let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
                environments::lock_environment(&mut tx, self.environment).await?;
                let row = jobs::lock(&mut tx, job.id())
                    .await?
                    .ok_or_else(|| Failure::new("gone", "the capability job is gone"))?;
                if row.cancel_requested() {
                    return Err(Stop::Cancelled);
                }
                if row.plan.get("committed").and_then(Json::as_bool) == Some(true) {
                    return Ok(Done::Finished);
                }
                if environments::live_runtime(&mut tx, self.environment)
                    .await?
                    .is_none_or(|runtime| runtime.id != self.plan.runtime)
                {
                    return Err(Failure::new(
                        "changed",
                        "the environment's unit changed before the grant was recorded",
                    )
                    .into());
                }
                let mut unit = Unit::new(&mut tx, job.now());
                if self.plan.grant {
                    let facts = capabilities::environment_facts(&mut *unit.conn, self.environment)
                        .await?
                        .ok_or_else(|| Failure::new("gone", "the environment is gone"))?;
                    let definition =
                        capabilities::definition(&mut *unit.conn, &self.plan.capability)
                            .await?
                            .ok_or_else(|| {
                                Failure::new("gone", "the capability definition is gone")
                            })?;
                    self.engine
                        .record_capability_grant(
                            &mut unit,
                            &facts,
                            &definition,
                            self.plan.requester,
                            self.plan.decided_by,
                        )
                        .await?;
                    // The next input's ordinary T3 line carries the new held set. No separate
                    // successful-grant input, wake, recreate_before or recreate_after is made.
                }
                let finished = unit.finish().await?;
                // The held set, its audit and the point of no undo share one transaction.
                // A crash cannot leave a committed job whose grant was never recorded.
                if !jobs::pass_commit(&mut tx, job.id(), job.now()).await? {
                    return Err(Stop::Cancelled);
                }
                self.engine.commit(tx, finished).await?;
            }
            _ => return Err(Failure::new("phase", "the live capability phase is unknown").into()),
        }
        Ok(Done::Finished)
    }

    async fn undo(&self, job: &Job, _why: &Stop) -> Undone {
        if job
            .plan()
            .await
            .ok()
            .is_some_and(|plan| plan.get("committed").and_then(Json::as_bool) == Some(true))
        {
            return Undone::Everything;
        }
        // Even a lost acknowledgment may have given the node. The node's serialized inverse
        // removes it and restarts this same container, keeping every installed file.
        match self.command(job, true).await {
            Ok(()) => Undone::Everything,
            Err(error) => Undone::Partly(format!(
                "The previous capability set could not be restored: {error:?}"
            )),
        }
    }
}
