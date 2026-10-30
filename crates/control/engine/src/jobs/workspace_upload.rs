//! A completed private upload written as a durable job whose cancellation joins its machine command.

use super::runner::{Done, Failure, INTERNAL, Job, Phase, Phased, Stop, Undone};
use crate::commands::{Outcome, Outgoing};
use crate::error::EngineError;
use async_trait::async_trait;
use orch_access_contract::audit::AuditAction;
use orch_api_contract::workspace::FileStat;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::{environments, jobs, link, storage};
use orch_link_contract::commands::{FsWrite, SerialKey};
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// A single file's bytes are all of the work; only the controller marks its completion.
pub const PHASES: [Phase; 1] = [Phase {
    name: "files",
    weight: 100,
}];

fn command_id(job: Id) -> Id {
    crate::ids::derived("workspace.upload.write", &[job.as_bytes()])
}

/// Retains a completed native upload in the transaction that acknowledges its handler has ended.
pub async fn retain_native_result(
    conn: &mut orch_db::Conn,
    machine: Id,
    command: Id,
    written: &Json,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), orch_db::DbError> {
    if written.get("path").is_none() || written.get("hash").is_none() {
        return Ok(());
    }
    let Some(command) = link::command(conn, command)
        .await?
        .filter(|command| command.machine_id == machine && command.kind == "fs.write")
    else {
        return Ok(());
    };
    let Some(id) = command
        .body
        .pointer("/work/job_id")
        .and_then(Json::as_str)
        .and_then(|value| value.parse::<Id>().ok())
    else {
        return Ok(());
    };
    let Some(row) = jobs::lock(conn, id).await? else {
        return Ok(());
    };
    if !row.is_open()
        || row.kind != jobs::kind::DELIVERY
        || row.plan.get("workspace_upload").is_none()
        || row.plan.get("native_written").is_some()
        || row.plan.pointer("/workspace_upload/machine") != Some(&json!(machine))
        || command.id != command_id(id)
        || !row
            .plan
            .pointer("/active_command/id")
            .and_then(Json::as_str)
            .is_some_and(|id| id == command.id.to_string())
    {
        return Ok(());
    }
    // The command's first terminal outcome stays intact. Its real file receipt belongs to
    // the job even when that native answer crosses cancellation or later command retention.
    jobs::merge_plan(conn, id, &json!({"native_written":written}), at).await?;
    Ok(())
}

/// The immutable admission facts of one workspace write, kept in its ordinary delivery job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Admission {
    /// The authenticated machine selected at admission.
    pub machine: Id,
    /// The environment generation selected at admission.
    pub generation: i32,
    /// The write with its final absence or version condition.
    pub write: FsWrite,
    /// The actual stored byte size, verified by the API.
    pub size: i64,
    /// The environment's home for its history line.
    pub home: Id,
    /// The environment's owning root for its history line.
    pub org: Id,
    /// The person who admitted the write.
    pub by: Id,
    /// The request provenance retained for its eventual audit.
    pub origin: Origin,
    /// The admitted actor and path, for older history readers.
    pub summary: String,
    /// The browser batch which groups its actual writes into one history line.
    pub batch: Id,
}

/// One delivery job's workspace write and its retained content ownership.
pub struct Upload(Admission);

impl Upload {
    /// Reads a tagged workspace write, refusing a damaged or foreign environment identity.
    pub fn of(row: &jobs::JobRow) -> Result<Self, EngineError> {
        let admission: Admission = serde_json::from_value(row.plan["workspace_upload"].clone())?;
        if row.environment_id != Some(admission.write.environment_id)
            || row.created_by != Some(admission.by)
            || admission.size < 0
        {
            return Err(EngineError::unreadable(
                "the workspace upload has inconsistent admission facts",
            ));
        }
        Ok(Self(admission))
    }

    async fn finish(&self, job: &Job, written: Option<FileStat>) -> Result<(), EngineError> {
        let mut tx = job.engine().db().begin().await?;
        let row = job
            .lock_owned(&mut tx)
            .await?
            .ok_or_else(|| EngineError::unreadable("the workspace upload lost its claim"))?;
        if row.plan["upload_settled"] == true {
            return Ok(());
        }
        let admitted = &self.0;
        if let Some(stat) = &written {
            storage::record_cached(
                &mut tx,
                admitted.write.environment_id,
                &storage::CachedFile {
                    path: admitted.write.path.clone(),
                    size: stat.entry.size,
                    mtime_ns: stat
                        .entry
                        .mtime
                        .get()
                        .timestamp_millis()
                        .saturating_mul(storage::NANOS_PER_MILLI),
                    hash: stat.hash.unwrap_or(admitted.write.blob),
                    cached_at: job.now(),
                },
            )
            .await?;
            let action = orch_db::audit::action!(AuditAction::WorkspaceFileWritten);
            let mut entry = Entry::new(
                Actor::User(admitted.by),
                action.as_str(),
                "environment",
                Some(admitted.write.environment_id),
                &admitted.summary,
            );
            entry.history = action.is_history_line();
            entry.node_id = Some(admitted.home);
            entry.org_id = Some(admitted.org);
            entry.environment_id = Some(admitted.write.environment_id);
            entry.details = json!({"path":admitted.write.path,"size":stat.entry.size,
                "hash":stat.hash.unwrap_or(admitted.write.blob),"upload_batch":admitted.batch,
                "replaced":admitted.write.expected_hash.is_none()});
            audit::append(&mut tx, &entry, &admitted.origin, job.now()).await?;
            environments::thread_engine::touch_activity(
                &mut tx,
                admitted.write.environment_id,
                job.now(),
            )
            .await?;
        }
        storage::unpin_blobs(&mut tx, &[admitted.write.blob], job.now()).await?;
        jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({"upload_settled":true,"written":written}),
            job.now(),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }
}

#[async_trait]
impl Phased for Upload {
    async fn run(&self, job: &Job, _phase: &str) -> Result<Done, Stop> {
        let admitted = &self.0;
        let environment = environments::workspace::environment(
            &mut *job
                .engine()
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?,
            admitted.write.environment_id,
        )
        .await
        .map_err(EngineError::from)?
        .filter(|environment| {
            environment.machine_id == Some(admitted.machine)
                && environment.generation == admitted.generation
        });
        if environment.is_none() {
            return Err(Stop::Failed(Failure::new(
                "conflict",
                "the environment changed before the upload was written",
            )));
        }
        let command = Outgoing::new(
            command_id(job.id()),
            admitted.machine,
            SerialKey::Environment(admitted.write.environment_id),
            &admitted.write,
        )
        .map_err(|error| Stop::Failed(Failure::new(INTERNAL, error.to_string())))?;
        let written = job.command(command).await?;
        let stat: FileStat = serde_json::from_value(written).map_err(EngineError::from)?;
        self.finish(job, Some(stat)).await?;
        Ok(Done::Finished)
    }

    async fn undo(&self, job: &Job, _why: &Stop) -> Undone {
        // A command that won before cancellation keeps its real completed file and audit.
        // A cancelled command's acknowledgement alone does not invent a completed file receipt.
        let plan = match job.plan().await {
            Ok(plan) => plan,
            Err(error) => return Undone::Deferred(error.to_string()),
        };
        let outcome = match plan.get("native_written") {
            Some(body) => Some(Outcome::Succeeded(body.clone())),
            None => match job
                .engine()
                .inner
                .commands
                .status(command_id(job.id()))
                .await
            {
                Ok(info) => info.and_then(|info| info.outcome),
                Err(error) => return Undone::Deferred(error.to_string()),
            },
        };
        let written = match outcome {
            Some(Outcome::Succeeded(body)) => match serde_json::from_value(body) {
                Ok(stat) => Some(stat),
                Err(error) => return Undone::Deferred(error.to_string()),
            },
            _ => None,
        };
        match self.finish(job, written).await {
            Ok(()) => Undone::Partly("files already written stay in the workspace".into()),
            Err(error) => Undone::Deferred(error.to_string()),
        }
    }

    async fn result(&self, job: &Job) -> Option<Json> {
        job.plan()
            .await
            .ok()
            .and_then(|plan| plan.get("written").cloned())
    }
}
