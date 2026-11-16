//! A completed private upload written as a durable job whose cancellation joins its machine command.

/// Native original-request and physical destination checks for queued upload work.
pub mod authority;

use super::runner::{Done, Failure, INTERNAL, Job, Phase, Phased, Stop, Undone};
use crate::commands::{Outcome, Outgoing};
use crate::error::EngineError;
use async_trait::async_trait;
use orch_access_contract::audit::AuditAction;
use orch_api_contract::workspace::FileStat;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::{environments, jobs, link, storage};
use orch_link_contract::commands::{FsRelayCommit, FsRelayPrepare, FsWrite, SerialKey};
use orch_settings::Place;
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
    if serde_json::from_value::<FileStat>(written.clone()).is_err() {
        return Ok(());
    }
    let Some(command) = link::command(conn, command).await?.filter(|command| {
        command.machine_id == machine
            && matches!(command.kind.as_str(), "fs.write" | "fs.relay.commit")
    }) else {
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

/// The verified private file selected at the native destination, with its immutable receive path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeUpload {
    pub session_id: Id,
    pub receive_path: String,
    pub workspace_root: String,
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
    /// The carrier selected at admission; native content has no blob-store pin.
    pub relay: Option<NativeUpload>,
    /// Whether this write leaves a cached workspace version, captured before dispatch.
    pub relayed: bool,
    /// The exact authenticated request and its accepted minima, retained for each forward step.
    pub request: crate::operation_scope::http_origin::HttpOrigin,
    /// The actual workspace root at admission, including a connected directory's incarnation.
    pub workspace_root: String,
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

    /// Settles the write once: the file's receipt, its cached version in a place that caches, and the
    /// content the upload held. `awaiting_choice` is a write that failed on a clash with the file at the
    /// path: the person may still choose Replace or Keep both from the same upload, so its content is
    /// let go of but not removed at once, and has the grace of ordinary content.
    async fn finish(
        &self,
        job: &Job,
        written: Option<FileStat>,
        awaiting_choice: bool,
    ) -> Result<(), EngineError> {
        let mut tx = job.engine().db().begin().await?;
        let row = job
            .lock_owned(&mut tx)
            .await?
            .ok_or_else(|| EngineError::unreadable("the workspace upload lost its claim"))?;
        if row.plan["upload_settled"] == true {
            return Ok(());
        }
        let admitted = &self.0;
        let place = Place::Environment(admitted.write.environment_id);
        // A place that relays keeps no copy of what it wrote: the file is on the machine, and the
        // upload's copy in the store goes with the write (a connector that cannot relay is served through it).
        let relayed = admitted.relayed;
        if let Some(stat) = &written {
            if !relayed {
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
            }
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
        let released = if admitted.relay.is_some() {
            if written.is_some()
                && let Some(relay) = &admitted.relay
            {
                orch_db::queries::uploads::targets::written(&mut tx, relay.session_id, job.id())
                    .await?;
            }
            None
        } else if awaiting_choice {
            storage::unpin_blobs(&mut tx, &[admitted.write.blob], job.now()).await?;
            None
        } else {
            Some(
                crate::transfers::release_transfer(
                    &mut tx,
                    place,
                    &[admitted.write.blob],
                    job.now(),
                )
                .await?,
            )
        };
        jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({"upload_settled":true,"written":written}),
            job.now(),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        if let Some(released) = released {
            released.settle(job.engine()).await;
        }
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
                && admitted
                    .relay
                    .as_ref()
                    .is_none_or(|relay| environment.workspace_root == relay.workspace_root)
        });
        if environment.is_none() {
            return Err(Stop::Failed(Failure::new(
                "conflict",
                "the environment changed before the upload was written",
            )));
        }
        let written = if let Some(relay) = &admitted.relay {
            let prepare = Outgoing::new(
                crate::ids::derived("workspace.upload.prepare", &[job.id().as_bytes()]),
                admitted.machine,
                SerialKey::None,
                &FsRelayPrepare {
                    environment_id: admitted.write.environment_id,
                    receive_path: relay.receive_path.clone(),
                    path: admitted.write.path.clone(),
                    session_id: relay.session_id,
                    work: None,
                },
            )
            .map_err(|error| Stop::Failed(Failure::new(INTERNAL, error.to_string())))?;
            job.command(prepare).await?;
            let command = Outgoing::new(
                command_id(job.id()),
                admitted.machine,
                SerialKey::Environment(admitted.write.environment_id),
                &FsRelayCommit {
                    environment_id: admitted.write.environment_id,
                    receive_path: relay.receive_path.clone(),
                    path: admitted.write.path.clone(),
                    session_id: relay.session_id,
                    hash: admitted.write.blob,
                    size: admitted.size,
                    mode: admitted.write.mode,
                    expected_hash: admitted.write.expected_hash,
                    work: None,
                },
            )
            .map_err(|error| Stop::Failed(Failure::new(INTERNAL, error.to_string())))?;
            job.command(command).await?
        } else {
            let command = Outgoing::new(
                command_id(job.id()),
                admitted.machine,
                SerialKey::Environment(admitted.write.environment_id),
                &admitted.write,
            )
            .map_err(|error| Stop::Failed(Failure::new(INTERNAL, error.to_string())))?;
            job.command(command).await?
        };
        let stat: FileStat = serde_json::from_value(written).map_err(EngineError::from)?;
        self.finish(job, Some(stat), false).await?;
        Ok(Done::Finished)
    }

    async fn undo(&self, job: &Job, why: &Stop) -> Undone {
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
        let clash = matches!(why, Stop::Failed(failure) if failure.code.starts_with("conflict"));
        match self.finish(job, written, clash).await {
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
