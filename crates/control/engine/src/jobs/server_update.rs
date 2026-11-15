//! The server release uses the ordinary durable job and native node report paths.
use async_trait::async_trait;
use orch_api_contract::admin::{ServerUpdateObservation, ServerUpdateStage};
use orch_db::{
    Conn, DbError,
    queries::jobs::{self as rows, JobRow, NewJob},
};
use orch_link_contract::commands::{NodeServerUpdate, SerialKey};
use orch_types::{Id, Json, Time, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::runner::{self, Done, Failure, Job, Phase, Phased, Stop, Undone};
use crate::{Engine, EngineError, commands::Outgoing, operation_scope::http_origin::HttpOrigin};

pub mod command;
/// The same jobs table and runner used by environment and provider updates.
pub const KIND: &str = "server_update";
const PHASES: [Phase; 2] = [
    Phase {
        name: "commit",
        weight: 0,
    },
    Phase {
        name: "update",
        weight: 1,
    },
];

/// The actual admitted release and bundled physical node, with its original HTTP administration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub host: Id,
    pub machine: Id,
    pub release: String,
    pub prepare_timeout_secs: u64,
    pub origin: HttpOrigin,
}

pub async fn latest(conn: &mut Conn) -> Result<Option<JobRow>, DbError> {
    let id: Option<Id> = sqlx::query_scalar("SELECT j.id FROM jobs j JOIN installation_records r ON j.id=(r.value #>> '{}')::uuid WHERE r.key='server_update_job' AND j.kind='server_update'").fetch_optional(&mut *conn).await?;
    match id {
        Some(id) => rows::job(conn, id).await,
        None => Ok(None),
    }
}
/// Only a canonical bundle can originate these observations, including after a rollback restored the database.
pub async fn report(
    conn: &mut Conn,
    host: Id,
    machine: Id,
    report: &orch_link_contract::events::NodeServerUpdate,
    at: Time,
) -> Result<(), EngineError> {
    let Some(row) = rows::lock(conn, report.job_id).await? else {
        return Ok(());
    };
    if row.kind != KIND {
        return Ok(());
    }
    let request: Request = serde_json::from_value(row.plan.clone())?;
    if request.host != host
        || request.machine != machine
        || report.observation.to_version != request.release
    {
        return Ok(());
    }
    let bundled: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE h.id=$1 AND h.machine_id=$2 AND h.bundled_with_server AND m.deleted_at IS NULL AND m.status='active')").bind(host).bind(machine).fetch_one(&mut *conn).await.map_err(DbError::from)?;
    if !bundled {
        return Ok(());
    }
    if row.plan.get("committed") != Some(&json!(true)) {
        return Ok(());
    }
    let Some(command) =
        orch_db::queries::link::command_with_authority(conn, report.command_id).await?
    else {
        return Ok(());
    };
    let Some(authority) = command.authority else {
        return Ok(());
    };
    let outgoing = Outgoing::new(
        report.command_id,
        machine,
        SerialKey::Machine,
        &NodeServerUpdate {
            job_id: row.id,
            release: request.release.clone(),
            timeout_secs: request.prepare_timeout_secs,
        },
    )
    .map_err(EngineError::from)?
    .by(Time::new(command.command.deadline_at));
    if command.command.machine_id != machine
        || command.command.kind != outgoing.kind.as_str()
        || command.command.body != outgoing.body
        || command.command.serial_key() != outgoing.key.wire().as_deref()
    {
        return Ok(());
    }
    let Ok(authority) = serde_json::from_value::<crate::commands::CommandAuthority>(authority)
    else {
        tracing::warn!(job = %row.id, "an updater report's stored native source does not read");
        return Ok(());
    };
    let source = command::Source::of(row.id, &request, &outgoing)?;
    let expected = request.origin.command_source(
        &outgoing,
        crate::operation_scope::http_command_source::CommandEvidence::ServerUpdate(Box::new(
            source,
        )),
    )?;
    if !expected.same_origin(&authority) {
        return Ok(());
    }
    let previous: Option<ServerUpdateObservation> = row
        .plan
        .get("observation")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?;
    if previous.as_ref().is_some_and(|previous| {
        matches!(
            previous.stage,
            ServerUpdateStage::Updated | ServerUpdateStage::RolledBack
        )
    }) {
        return Ok(());
    }
    rows::merge_plan(
        conn,
        row.id,
        &json!({"observation":report.observation}),
        at.get(),
    )
    .await?;
    let (mut progress, log) = runner::read_progress(&row);
    progress.message = Some(
        serde_json::to_value(report.observation.stage)?
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    );
    // Equal named steps supply a conservative phase estimate; the observation remains the actual status.
    let step = match report.observation.stage {
        ServerUpdateStage::Queued => 0,
        ServerUpdateStage::Fetching => 1,
        ServerUpdateStage::Verifying => 2,
        ServerUpdateStage::Applying => 3,
        ServerUpdateStage::Stopping => 4,
        ServerUpdateStage::Dumping => 5,
        ServerUpdateStage::Activating => 6,
        ServerUpdateStage::Starting => 7,
        ServerUpdateStage::Checking => 8,
        ServerUpdateStage::RollingBack | ServerUpdateStage::Recovering => 8,
        ServerUpdateStage::Updated | ServerUpdateStage::RolledBack | ServerUpdateStage::Failed => 9,
    };
    progress.percent = progress.percent.max(step * 100 / 10);
    if let Some(activity) = &mut progress.activity {
        activity.updated_at = at;
        activity.waiting_for = None;
    }
    let mut value = serde_json::to_value(progress)?;
    if let Some(log) = log {
        value["log"] = json!(log);
    }
    rows::save_progress(conn, row.id, &value, at.get()).await?;
    if report.observation.stage.terminal() {
        let restored = report.observation.stage == ServerUpdateStage::RolledBack;
        let succeeded = report.observation.stage == ServerUpdateStage::Updated;
        let state = if succeeded {
            rows::state::SUCCEEDED
        } else {
            rows::state::FAILED
        };
        let error = (!succeeded).then(|| json!({"code":if restored {"server_update_rolled_back"} else {"server_update_failed"},"message":"The host updater recorded its final outcome.","phase":"update","undone":restored}));
        let result = serde_json::to_value(&report.observation)?;
        value["percent"] = json!(100);
        if let Some(phases) = value.get_mut("phases").and_then(Json::as_array_mut) {
            for phase in phases {
                phase["state"] = json!("done");
            }
        }
        // The exact host's completed effect remains true even when an earlier transport attempt failed.
        // This changes only the accepted job's factual outcome, never permission or native work.
        if row.state == rows::state::FAILED {
            sqlx::query("UPDATE jobs SET state=$2,progress=$3,result=$4,error=$5,finished_at=$6,updated_at=$6 WHERE id=$1 AND kind='server_update' AND state='failed'")
                .bind(row.id).bind(state).bind(&value).bind(&result).bind(&error).bind(at.get()).execute(&mut *conn).await.map_err(DbError::from)?;
        } else {
            rows::finish(
                conn,
                row.id,
                state,
                &value,
                Some(&result),
                error.as_ref(),
                at.get(),
            )
            .await?;
        }
    }
    Ok(())
}
impl Engine {
    /// The API has required ordinary system administration; the native command rechecks that same origin.
    pub async fn queue_server_update(
        &self,
        conn: &mut Conn,
        request: Request,
        by: Id,
    ) -> Result<JobRow, EngineError> {
        if by != request.origin.origin().user()
            || request.origin.origin().operation() != "AdminServerUpdateStart"
            || request.prepare_timeout_secs == 0
            || !orch_build_info::release::is_release_version(&request.release)
        {
            return Err(EngineError::unreadable(
                "the server update has another original operation",
            ));
        }
        let plan = serde_json::to_value(request)?;
        let progress = serde_json::to_value(runner::initial_progress(&PHASES))?;
        let row = rows::insert(
            conn,
            &NewJob {
                id: new_id(),
                kind: KIND,
                node_id: None,
                environment_id: None,
                provider_id: None,
                plan: &plan,
                progress: &progress,
                created_by: Some(by),
            },
            self.inner.clock.now(),
        )
        .await?;
        orch_db::queries::installation::put(
            conn,
            "server_update_job",
            &json!(row.id),
            self.inner.clock.now(),
        )
        .await?;
        Ok(row)
    }
}

pub(crate) struct Updating {
    request: Request,
}
impl Updating {
    pub(crate) fn of(row: &JobRow) -> Result<Self, EngineError> {
        Ok(Self {
            request: serde_json::from_value(row.plan.clone())?,
        })
    }
    async fn update(&self, job: &Job) -> Result<Done, Stop> {
        loop {
            job.check()?;
            let plan = job.plan().await?;
            if let Some(value) = plan.get("observation") {
                let observed: ServerUpdateObservation =
                    serde_json::from_value(value.clone()).map_err(EngineError::from)?;
                match observed.stage {
                    ServerUpdateStage::Updated => return Ok(Done::Finished),
                    ServerUpdateStage::RolledBack => {
                        return Err(Stop::Failed(Failure::new(
                            "server_update_rolled_back",
                            "The new release failed readiness; the previous release is running again.",
                        )));
                    }
                    ServerUpdateStage::Failed => {
                        return Err(Stop::Failed(Failure::new(
                            "server_update_failed",
                            "The server updater could not finish. Its recorded reason and recovery command remain available.",
                        )));
                    }
                    _ => {}
                }
            }
            if plan.get("requested") != Some(&json!(true)) {
                let sequence = plan
                    .get("start_sequence")
                    .and_then(Json::as_u64)
                    .unwrap_or(0);
                let next = sequence.checked_add(1).ok_or_else(|| {
                    EngineError::unreadable("the server update command sequence is exhausted")
                })?;
                job.remember(&json!({"start_sequence":next})).await?;
                let mut command = Outgoing::new(
                    crate::ids::derived(
                        "server.update",
                        &[job.id().as_bytes(), &sequence.to_be_bytes()],
                    ),
                    self.request.machine,
                    SerialKey::Machine,
                    &NodeServerUpdate {
                        job_id: job.id(),
                        release: self.request.release.clone(),
                        timeout_secs: self.request.prepare_timeout_secs,
                    },
                )
                .map_err(EngineError::from)?;
                let seconds = i64::try_from(self.request.prepare_timeout_secs).map_err(|_| {
                    EngineError::unreadable("the updater preparation allowance exceeds its clock")
                })?;
                let interval = chrono::Duration::try_seconds(seconds).ok_or_else(|| {
                    EngineError::unreadable("the updater preparation allowance exceeds its clock")
                })?;
                let deadline = job.now().checked_add_signed(interval).ok_or_else(|| {
                    EngineError::unreadable("the updater preparation deadline exceeds its clock")
                })?;
                command.deadline = Some(Time::new(deadline));
                let source = command::Source::of(job.id(), &self.request, &command)?;
                command.authority = Some(self.request.origin.command_source(
                    &command,
                    crate::operation_scope::http_command_source::CommandEvidence::ServerUpdate(
                        source,
                    ),
                )?);
                match job.command(command).await {
                    Ok(_) => {
                        job.remember(&json!({"requested":true})).await?;
                    }
                    Err(Stop::Failed(failure))
                        if matches!(failure.code.as_str(), "timeout" | "busy") => {}
                    Err(error) => return Err(error),
                }
            }
            tokio::time::sleep(job.engine().inner.config.sweep_every).await;
        }
    }
}
#[async_trait]
impl Phased for Updating {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        match phase {
            "commit" => {
                job.pass_commit().await?;
                Ok(Done::Finished)
            }
            "update" => self.update(job).await,
            _ => Err(Stop::Failed(Failure::new(
                "internal",
                "the server update has no such phase",
            ))),
        }
    }
    async fn undo(&self, job: &Job, _: &Stop) -> Undone {
        let restored = job
            .plan()
            .await
            .ok()
            .and_then(|plan| plan.get("observation").cloned())
            .and_then(|value| serde_json::from_value::<ServerUpdateObservation>(value).ok())
            .is_some_and(|value| value.stage == ServerUpdateStage::RolledBack);
        if restored {
            Undone::Everything
        } else {
            Undone::Partly("The host updater retains its own journal and recovery path.".into())
        }
    }
    async fn result(&self, job: &Job) -> Option<Json> {
        job.plan().await.ok()?.get("observation").cloned()
    }
    async fn partial_result(&self, job: &Job) -> Option<Json> {
        self.result(job).await
    }
}
