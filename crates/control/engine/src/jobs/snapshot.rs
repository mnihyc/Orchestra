//! Selected tree hashing, upload and pinning.
//!
//! A snapshot is a thread's directory kept as of a moment. Taking one is a job with the phases
//! `scan`, `hash`, `upload`, `manifest` and `finalize`: the machine walks the selection, hashes the files
//! whose hash it does not know, asks the store which of the hashes it lacks and uploads those, and
//! uploads the list of what it took (one command, `fs.snapshot`, asked in the first phase, whose
//! progress arrives as `job.progress` events and is written on the job's row); the last phase is the
//! system's own: it pins every file the list names and the list itself, writes the snapshot's row as
//! `complete` or `partial` (partial where some file could not be read, vanished or was too large, and is
//! listed without its content), and ends the job with the snapshot as its result.
//!
//! **The job is run like any other.** The runner of the engine ([`crate::jobs::runner`]) takes it up
//! with a lease, carries it through its phases, reads a request to cancel while the machine works, and
//! goes on with it in another process when the one that ran it is gone: the command is asked again under
//! the same identifier, which a machine that has it already does not run twice. The `finalize` phase is
//! the point after which nothing is cancelled and nothing is undone; a snapshot that is cancelled or fails
//! before it is ended as failed (what was uploaded stays in the store as cache and goes with its grace).
//! A snapshot waits behind the jobs that change a whole file system (making an environment, recreating,
//! moving or removing it).
//!
//! Lists of a tree that machines upload on their own account are looked at here too
//! (`Engine::keep_manifest`, called with the other reports of an environment's stream): the latest of
//! an environment is kept and pinned, and the one it replaces is let go.

use crate::commands::Outgoing;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use crate::jobs::runner::{
    Done, Failure, INTERNAL, Job, Phase, Phased, Stop, Undone, initial_progress, read_progress,
};
use crate::manifest::Manifest;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use orch_api_contract::workspace::{JobPhase, JobProgress};
use orch_blob_contract::range::BlobMeta;
use orch_db::Conn;
use orch_db::queries::environments::{
    thread_engine as environment_rows, workspace as workspace_rows,
};
use orch_db::queries::jobs::{self as rows, JobRow, NewJob, kind};
use orch_db::queries::storage::{self, NewSnapshot, SnapshotEnd, snapshot_state};
use orch_link_contract::commands::{FsSnapshot, FsSnapshotResult, Selection, SerialKey};
use orch_link_contract::events::EnvManifest;
use orch_types::{Hash, Id, Json, new_id};
use serde_json::json;
use std::collections::HashSet;
use std::time::Duration;

/// The first phase: the machine walks the selection and does all it does, in the one command.
const SCAN: Phase = Phase {
    name: "scan",
    weight: 5,
};
/// The second phase, as the machine reports it: files hashed.
const HASH: Phase = Phase {
    name: "hash",
    weight: 30,
};
/// The third phase, as the machine reports it: content uploaded. Hashing and uploading are where the
/// time goes.
const UPLOAD: Phase = Phase {
    name: "upload",
    weight: 55,
};
/// The fourth phase, as the machine reports it: the list of what was taken, uploaded.
const MANIFEST: Phase = Phase {
    name: "manifest",
    weight: 5,
};
/// The last phase, the system's own: everything the list names is pinned and the snapshot is ended.
const FINALIZE: Phase = Phase {
    name: "finalize",
    weight: 5,
};

/// The phases of a snapshot, each with its share of the bar out of one hundred.
pub const PHASES: [Phase; 5] = [SCAN, HASH, UPLOAD, MANIFEST, FINALIZE];

/// How often a snapshot that waits behind a job that changes the whole file system looks again:
/// such a job takes minutes, and a look is one query.
const EXCLUSIVE_LOOK_EVERY: Duration = Duration::from_secs(1);

/// The most a list of a tree may take to read into memory: the bound the API puts on one.
const MANIFEST_READ_LIMIT: i64 = 512 * 1024 * 1024;

/// The code a job fails with when the machine that has to take the snapshot is not there.
pub const ENVIRONMENT_UNAVAILABLE: &str = "environment_unavailable";

/// What is asked for when a snapshot is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    /// The thread it is taken through.
    pub thread: Option<Id>,
    /// The environment it is taken in.
    pub environment: Id,
    /// The generation of the environment's unit.
    pub generation: i32,
    /// What it is called.
    pub label: String,
    /// The paths it takes, each with what is under it.
    pub include: Vec<String>,
    /// The paths under those that it leaves out.
    pub exclude: Vec<String>,
    /// Who asks.
    pub created_by: Option<Id>,
}

/// A snapshot that was asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct Begun {
    /// The job that takes it.
    pub job: JobRow,
    /// The snapshot, which is `running` until the job ends.
    pub snapshot: Id,
}

/// What a snapshot job keeps in its plan.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Plan {
    snapshot: Id,
    environment: Id,
    include: Vec<String>,
    exclude: Vec<String>,
}

impl Plan {
    fn of(job: &JobRow) -> Result<Self, EngineError> {
        let id = |member: &str| {
            job.plan
                .get(member)
                .and_then(Json::as_str)
                .and_then(|text| text.parse::<Id>().ok())
                .ok_or_else(|| {
                    EngineError::unreadable(format!("the plan of a snapshot lacks {member}"))
                })
        };
        let list = |member: &str| -> Vec<String> {
            job.plan
                .pointer(&format!("/selection/{member}"))
                .and_then(Json::as_array)
                .map(|each| {
                    each.iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        Ok(Self {
            snapshot: id("snapshot_id")?,
            environment: id("environment_id")?,
            include: list("include"),
            exclude: list("exclude"),
        })
    }
}

impl Engine {
    /// Asks for a snapshot, in the transaction of the caller: the row of the snapshot (running) and the
    /// job that takes it (queued). The job is taken up by the next sweep.
    pub async fn begin_snapshot(
        &self,
        conn: &mut Conn,
        asked: &Asked,
    ) -> Result<Begun, EngineError> {
        let now = self.inner.clock.now();
        let snapshot = new_id();
        let selection = json!({ "include": asked.include, "exclude": asked.exclude });
        storage::begin_snapshot(
            conn,
            &NewSnapshot {
                id: snapshot,
                thread_id: asked.thread,
                environment_id: asked.environment,
                env_generation: asked.generation,
                label: &asked.label,
                selection: &selection,
                created_by: asked.created_by,
                at: now,
            },
        )
        .await?;
        let plan = json!({
            "snapshot_id": snapshot,
            "environment_id": asked.environment,
            "thread_id": asked.thread,
            "workspace_export": asked.thread.is_none(),
            "selection": selection,
        });
        let progress = serde_json::to_value(initial_progress(&PHASES))?;
        let job = rows::insert(
            conn,
            &NewJob {
                id: new_id(),
                kind: kind::SNAPSHOT,
                node_id: asked.thread,
                environment_id: Some(asked.environment),
                provider_id: None,
                plan: &plan,
                progress: &progress,
                created_by: asked.created_by,
            },
            now,
        )
        .await?;
        // The sweep looks once the caller's transaction has had time to commit: before that it would
        // find nothing and wait out its whole interval.
        let engine = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(engine.inner.config.post_commit_grace).await;
            engine.kick();
        });
        Ok(Begun { job, snapshot })
    }
}

// ---------------------------------------------------------------- the phases

/// The phases of taking a snapshot.
#[derive(Debug, Clone)]
pub struct Snapshotting {
    engine: Engine,
    plan: Plan,
}

impl Snapshotting {
    /// The work of a job row of the kind `snapshot`.
    pub fn of(engine: Engine, row: &JobRow) -> Result<Self, EngineError> {
        Ok(Self {
            engine,
            plan: Plan::of(row)?,
        })
    }

    /// The first phase: the machine is asked, once the whole file system is not being changed, and
    /// its answer is kept in the plan for the last.
    async fn take(&self, job: &Job) -> Result<Done, Stop> {
        let mut queued = false;
        loop {
            let open = {
                let mut conn = self
                    .engine
                    .inner
                    .db
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                rows::open_of_environment(&mut conn, self.plan.environment).await?
            };
            if open.is_empty() {
                break;
            }
            if !queued {
                job.amend(|progress, _| {
                    if let Some(activity) = &mut progress.activity {
                        activity.waiting_for =
                            Some(orch_api_contract::workspace::JobWaiting::EnvironmentChange);
                    }
                })
                .await?;
                queued = true;
            }
            job.pause(EXCLUSIVE_LOOK_EVERY).await?;
        }
        if queued {
            job.amend(|progress, _| {
                if let Some(activity) = &mut progress.activity {
                    activity.waiting_for = None;
                }
            })
            .await?;
        }
        let machine = {
            let mut conn = self
                .engine
                .inner
                .db
                .acquire()
                .await
                .map_err(EngineError::from)?;
            match environment_rows::environment(&mut conn, self.plan.environment).await? {
                Some(environment) if environment.deleted_at.is_none() => environment.machine_id,
                _ => None,
            }
        };
        let Some(machine) = machine else {
            return Err(Stop::Failed(Failure::new(
                ENVIRONMENT_UNAVAILABLE,
                "the environment has no machine to take the snapshot",
            )));
        };
        let command = Outgoing::new(
            ids::derived("fs.snapshot", &[job.id().as_bytes().as_slice()]),
            machine,
            SerialKey::Environment(self.plan.environment),
            &FsSnapshot {
                work: None,
                environment_id: self.plan.environment,
                job_id: job.id(),
                selection: Selection {
                    include: self.plan.include.clone(),
                    exclude: self.plan.exclude.clone(),
                },
            },
        )
        .map_err(|error| Stop::Failed(Failure::new(INTERNAL, error.to_string())))?;
        let answer = job.command(command).await?;
        let taken: FsSnapshotResult = serde_json::from_value(answer)
            .map_err(|error| Stop::Failed(Failure::new(INTERNAL, error.to_string())))?;
        job.remember(&json!({ "taken": taken })).await?;
        Ok(Done::Finished)
    }

    /// The last phase: everything the list names is pinned, and the snapshot is written as it came
    /// out. All in one transaction, so that a snapshot is complete with its pins or not at all, and
    /// asked again after a restart it pins nothing twice.
    async fn finalize(&self, job: &Job) -> Result<Done, Stop> {
        job.pass_commit().await?;
        let taken: FsSnapshotResult = job
            .plan()
            .await?
            .get("taken")
            .cloned()
            .and_then(|taken| serde_json::from_value(taken).ok())
            .ok_or_else(|| {
                Stop::Failed(Failure::new(
                    INTERNAL,
                    "what the machine answered to the snapshot was not kept",
                ))
            })?;
        let Some(store) = self.engine.blobs() else {
            return Err(Stop::Failed(Failure::new(
                INTERNAL,
                "no store of content is attached",
            )));
        };
        let bytes = match store.size(&taken.manifest_blob).await {
            Ok(Some(size)) if size <= MANIFEST_READ_LIMIT => {
                let read = store
                    .get(&taken.manifest_blob, None)
                    .await
                    .map_err(EngineError::unreadable)?;
                orch_blob_contract::store::read_all(read.body)
                    .await
                    .map_err(EngineError::unreadable)?
            }
            _ => {
                return Err(Stop::Failed(Failure::new(
                    "content_missing",
                    "the list of what was taken is not in the store",
                )));
            }
        };
        let manifest = tokio::task::spawn_blocking(move || Manifest::parse(&bytes))
            .await
            .map_err(EngineError::unreadable)?
            .map_err(|error| Stop::Failed(Failure::new("content_unreadable", error.to_string())))?;
        let listed: Vec<Hash> = manifest
            .hashes()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let now = job.now();
        let mut tx = self
            .engine
            .inner
            .db
            .begin()
            .await
            .map_err(EngineError::from)?;
        let running = storage::snapshot(&mut tx, self.plan.snapshot)
            .await?
            .is_some_and(|row| row.state == snapshot_state::RUNNING);
        if running {
            let pinned = storage::pin_snapshot(&mut tx, self.plan.snapshot, &listed).await?;
            storage::pin_snapshot(&mut tx, self.plan.snapshot, &[taken.manifest_blob]).await?;
            // A file the list names whose content the store does not hold is as good as one that
            // could not be taken.
            let lacking = listed
                .len()
                .saturating_sub(usize::try_from(pinned).unwrap_or(usize::MAX));
            let lacking = i64::try_from(lacking).unwrap_or(i64::MAX);
            let missing = taken.missing.saturating_add(lacking);
            rows::merge_plan(
                &mut tx,
                job.id(),
                &json!({"snapshot_missing_count":missing}),
                now,
            )
            .await?;
            storage::end_snapshot(
                &mut tx,
                self.plan.snapshot,
                &SnapshotEnd {
                    state: if missing > 0 {
                        snapshot_state::PARTIAL
                    } else {
                        snapshot_state::COMPLETE
                    },
                    file_count: taken.file_count,
                    total_bytes: taken.total_bytes,
                    missing_count: missing,
                    manifest_blob: Some(taken.manifest_blob),
                    at: now,
                },
            )
            .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        job.amend(move |progress, _| {
            progress.files_done = taken.file_count;
            progress.files_total = Some(taken.file_count);
            progress.bytes_done = taken.total_bytes;
            progress.bytes_total = Some(taken.total_bytes);
        })
        .await?;
        Ok(Done::Finished)
    }
}

#[async_trait]
impl Phased for Snapshotting {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        if phase == SCAN.name {
            self.take(job).await
        } else if phase == FINALIZE.name {
            self.finalize(job).await
        } else {
            // What the machine does between the first and the last is in its one command; the
            // phases show how far it has come, from what it reports.
            Ok(Done::Finished)
        }
    }

    async fn undo(&self, job: &Job, _why: &Stop) -> Undone {
        let ended = async {
            let mut conn = self.engine.inner.db.acquire().await?;
            storage::end_snapshot(&mut conn, self.plan.snapshot, &failed_end(job.now())).await
        }
        .await;
        match ended {
            Ok(_) => Undone::Everything,
            Err(error) => Undone::Partly(format!(
                "the snapshot could not be marked as failed: {error}"
            )),
        }
    }

    async fn result(&self, job: &Job) -> Option<Json> {
        // Keep the durable snapshot identifier even if progress detail cannot be read.
        let missing = job.plan().await.ok().and_then(|plan| {
            plan.get("snapshot_missing_count")
                .or_else(|| plan.pointer("/taken/missing"))
                .and_then(Json::as_i64)
        });
        Some(json!({ "snapshot_id": self.plan.snapshot, "missing_count":missing }))
    }
}

fn failed_end(at: DateTime<Utc>) -> SnapshotEnd {
    SnapshotEnd {
        state: snapshot_state::FAILED,
        file_count: 0,
        total_bytes: 0,
        missing_count: 0,
        manifest_blob: None,
        at,
    }
}

// ---------------------------------------------------------------- what a machine says of an environment

impl Engine {
    /// Writes how far a job of the workspace has come, as its machine reports it. A report never takes
    /// the bar back, and never fills it: the last phase is the system's own.
    pub(crate) async fn apply_workspace_job_progress(
        &self,
        conn: &mut Conn,
        machine: Id,
        job: Id,
        command_id: Option<Id>,
        reported: &Json,
    ) -> Result<(), EngineError> {
        let Some(row) = rows::lock(conn, job).await? else {
            return Ok(());
        };
        if !row.is_open() || row.cancel_requested() {
            return Ok(());
        }
        if let Some(command) = command_id
            && !row
                .plan
                .pointer("/active_command/id")
                .and_then(Json::as_str)
                .is_some_and(|id| id == command.to_string())
        {
            return Ok(());
        }
        if !kind::WORKSPACE.contains(&row.kind.as_str())
            || row.plan.get("workspace_upload").is_some()
        {
            // A single-file upload uses the active command's counters; only its controller
            // finishes the phase, after the native result and filesystem cleanup.
            return crate::jobs::runner::report_progress(
                conn,
                machine,
                job,
                command_id,
                reported,
                None,
                self.inner.clock.now(),
            )
            .await;
        }
        let Ok(mut reported) = serde_json::from_value::<JobProgress>(reported.clone()) else {
            return Ok(());
        };
        let (before, _) = read_progress(&row);
        if reported.files_done != before.files_done
            || reported.bytes_done != before.bytes_done
            || reported.phase != before.phase
            || reported.percent != before.percent
            || reported.message != before.message
        {
            reported.activity = Some(orch_api_contract::workspace::JobActivity {
                waiting_for: None,
                updated_at: orch_types::Time::new(self.inner.clock.now()),
                stall_after_seconds: 0,
                directories_done: reported
                    .activity
                    .as_ref()
                    .and_then(|activity| activity.directories_done),
                stopping: false,
            });
        } else {
            reported.activity = before.activity.clone();
        }
        reported.percent = reported.percent.max(before.percent);
        if row.kind == kind::SNAPSHOT {
            reported.percent = reported.percent.min(bar_before_the_last_phase());
            // The phases are the job's own; the machine's say how far each has come, except the last,
            // which is the system's own and which only the runner moves: a runner passes by a phase that
            // is done, so one that the machine called done would never be carried out.
            reported.phases = before
                .phases
                .iter()
                .map(|own| {
                    reported
                        .phases
                        .iter()
                        .find(|each| each.name == own.name && own.name != FINALIZE.name)
                        .map_or_else(
                            || own.clone(),
                            |each| JobPhase {
                                name: own.name.clone(),
                                weight: own.weight,
                                state: each.state,
                            },
                        )
                })
                .collect();
        }
        rows::save_progress(
            conn,
            job,
            &serde_json::to_value(&reported)?,
            self.inner.clock.now(),
        )
        .await?;
        Ok(())
    }

    /// Keeps the list of the tree that a machine uploaded as its environment's latest, pinned; the
    /// one it replaces is let go. A machine speaks only for the environments that are on it.
    pub(crate) async fn keep_manifest(
        &self,
        conn: &mut Conn,
        machine: Id,
        environment: Id,
        manifest: &EnvManifest,
        taken_at: DateTime<Utc>,
    ) -> Result<(), EngineError> {
        let Some(row) = workspace_rows::environment(conn, environment).await? else {
            return Ok(());
        };
        if row.machine_id.is_some_and(|on| on != machine) {
            tracing::warn!(%machine, %environment, "a machine uploaded the list of a tree that is not on it");
            return Ok(());
        }
        // The list was uploaded before it was told of: the store holds it, and the row says so.
        if storage::blob(conn, &manifest.manifest_blob)
            .await?
            .is_none()
        {
            tracing::warn!(%environment, "a list of a tree was announced and the store has no row of it");
            return Ok(());
        }
        if row.manifest_blob == Some(manifest.manifest_blob) {
            return Ok(());
        }
        let now = self.inner.clock.now();
        storage::pin_blobs(conn, &[manifest.manifest_blob]).await?;
        if let Some(Some(before)) =
            workspace_rows::keep_manifest(conn, environment, &manifest.manifest_blob, taken_at)
                .await?
        {
            storage::unpin_blobs(conn, &[before], now).await?;
        }
        Ok(())
    }

    /// Reads a list of a tree from the store.
    pub async fn read_manifest(&self, hash: &Hash) -> Result<Manifest, EngineError> {
        let store = self
            .blobs()
            .ok_or_else(|| EngineError::unreadable("no store of content is attached"))?;
        let read = store
            .get(hash, None)
            .await
            .map_err(EngineError::unreadable)?;
        let bytes = orch_blob_contract::store::read_all(read.body)
            .await
            .map_err(EngineError::unreadable)?;
        tokio::task::spawn_blocking(move || Manifest::parse(&bytes))
            .await
            .map_err(EngineError::unreadable)?
            .map_err(EngineError::unreadable)
    }

    /// Puts a list of a tree in the store, recorded.
    pub async fn put_manifest(
        &self,
        conn: &mut Conn,
        bytes: Vec<u8>,
    ) -> Result<BlobMeta, EngineError> {
        let store = self
            .blobs()
            .ok_or_else(|| EngineError::unreadable("no store of content is attached"))?;
        let hash = Hash::of(&bytes);
        let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
        store
            .put(
                &hash,
                Some(size),
                orch_blob_contract::store::stream_of(vec![bytes.into()]),
            )
            .await
            .map_err(EngineError::unreadable)?;
        storage::register_blob(conn, &hash, size, self.inner.clock.now()).await?;
        Ok(BlobMeta { hash, size })
    }
}

/// How far the bar goes before the last phase, which is the system's own: the weights of the others.
fn bar_before_the_last_phase() -> u32 {
    PHASES[..PHASES.len() - 1]
        .iter()
        .map(|phase| phase.weight)
        .sum()
}
