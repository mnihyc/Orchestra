//! Durable phases with weights and a percentage, serialization, cancellation, undo and recovery.
//!
//! A job is a row (`orch_db::queries::jobs`) whose `progress` lists its phases in order, each with
//! its weight and state, and whose `plan` holds what it was asked to do and what its runner decided
//! on the way. The runner works through the phases that are not done, writes the row after each step,
//! and so can be taken over at any moment: a process that is started on a database where a job is
//! `running` finds it by the same sweep that finds new ones, takes the lease on it once the lease of
//! the process that died has lapsed, and goes on from the phase the row says. Every phase is
//! restartable: asked again it finds what it did before and does not do it twice, and the commands
//! it sends carry identifiers derived from the job, so that a command already sent is not sent again.
//!
//! **Percentage.** The weights of the phases a job has are scaled to one hundred. The percentage is
//! the weight of the phases that are done plus the weight of the running phase times how far it is,
//! and it never goes down. **One job at a time** runs for an environment: [`make`] refuses a second
//! while one is not over, naming it. **Cancel** is a request written in the job's plan; the runner
//! sees it between its steps and while it waits for a command, cancels that command, and undoes
//! what the job made. A job that has passed its commit cannot be cancelled and is never undone.

use crate::commands::{Outcome as Ended, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hosting::WATCH_EVERY;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use orch_api_contract::workspace::{JobProgress, PhaseState};
use orch_db::queries::environments::hosting as environments;
use orch_db::queries::jobs::{self as rows, JobRow, NewJob, kind, state};
use orch_types::{Id, Json, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// What the whole bar is: the weights of a job's phases are scaled to this.
const WHOLE: u32 = 100;
/// How many times a phase is asked again after a failure of the system under it (the database, a
/// link that broke), before the job fails: such a failure is not the job's.
const PHASE_RETRIES: u32 = 2;
/// How long to wait before asking a phase again.
const PHASE_RETRY_PAUSE: Duration = Duration::from_millis(500);
/// The code of a failure that is the system's own and not the host's or the person's.
pub const INTERNAL: &str = "internal";
/// How many jobs one sweep takes up at most.
const RUN_BATCH: i64 = 50;
/// How many jobs of one batch of forks and switches run at once when the setting is not there.
const BATCH_PARALLEL_DEFAULT: i64 = 4;
/// How many finished jobs of an environment its sync shows beside those that are not over.
pub const RECENT_JOBS: i64 = 5;

// Content/capture work does not change anybody's authority. Lifecycle, retained
// resource and capability jobs still declare the exclusive barrier before a phase.
fn phase_scope(job_kind: &str) -> orch_db::queries::transaction_guard::Scope {
    use orch_db::queries::transaction_guard::Scope;
    if matches!(
        job_kind,
        kind::SNAPSHOT
            | kind::DELIVERY
            | kind::WORKSPACE_PREPARE
            | kind::GIT_COMMIT
            | kind::GIT_PUSH
            | kind::EXPORT
            | kind::IMPORT
            | crate::jobs::background_end::KIND
            | crate::jobs::kept_images::KIND
            | crate::jobs::measurement::KIND
            | crate::jobs::document_notices::KIND
    ) {
        Scope::Shared
    } else {
        Scope::Authority
    }
}

/// A phase of a job and its weight before the weights are scaled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Phase {
    /// Its name.
    pub name: &'static str,
    /// Where the time usually goes: only the proportions count.
    pub weight: u32,
}

/// Why a phase did not end well.
#[derive(Debug, Clone, PartialEq)]
pub enum Stop {
    /// It failed, for a reason that is kept on the job.
    Failed(Failure),
    /// Somebody asked for the job to be cancelled.
    Cancelled,
    /// The runner may no longer run the job (its lease went to another process, or the engine is
    /// stopping): nothing is changed, and whoever runs it next goes on.
    Lost,
}

/// A reason for a job to fail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    /// A stable code.
    pub code: String,
    /// What went wrong, in a sentence.
    pub message: String,
    /// The end of what a phase printed, where one prints.
    pub log: Option<String>,
}

/// Once cleanup is admitted, recovery must finish the same failure rather than resume success.
/// This intent is written under the authoritative claim before any external undo operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "failure", rename_all = "snake_case")]
enum Abort {
    Failed(Failure),
    Cancelled,
}

/// Reads the terminal direction without admitting new work; a damaged intent fails closed.
pub(crate) fn abort_reason(row: &JobRow) -> Result<Option<Stop>, EngineError> {
    row.plan
        .get("abort")
        .cloned()
        .map(|value| {
            serde_json::from_value::<Abort>(value)
                .map(|intent| match intent {
                    Abort::Failed(failure) => Stop::Failed(failure),
                    Abort::Cancelled => Stop::Cancelled,
                })
                .map_err(Into::into)
        })
        .transpose()
}

impl Failure {
    /// A failure with a code and a sentence.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            log: None,
        }
    }

    /// The same, with the end of what was printed.
    #[must_use]
    pub fn with_log(mut self, log: Option<String>) -> Self {
        self.log = log;
        self
    }
}

impl From<Failure> for Stop {
    fn from(failure: Failure) -> Self {
        Self::Failed(failure)
    }
}

impl From<EngineError> for Stop {
    fn from(error: EngineError) -> Self {
        match error {
            EngineError::Db(error) => Self::from(error),
            other => Self::Failed(Failure::new(INTERNAL, other.to_string())),
        }
    }
}

impl From<orch_db::DbError> for Stop {
    fn from(error: orch_db::DbError) -> Self {
        let facts = persistence_facts(&error);
        tracing::error!(
            phase = "job_persistence",
            category = facts.category,
            sqlstate = facts.sqlstate.as_deref().unwrap_or("unknown"),
            constraint = facts.constraint.as_deref().unwrap_or("unknown"),
            column = facts.column.unwrap_or("unknown"),
            "a job could not persist its change"
        );
        Self::Failed(Failure::new(
            INTERNAL,
            "The change could not be saved. Try again.",
        ))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct PersistenceFacts {
    category: &'static str,
    sqlstate: Option<String>,
    constraint: Option<String>,
    column: Option<&'static str>,
}

fn schema_identity(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
    .then(|| value.to_owned())
}

/// Fixed names from the thread and history database projections. Never return a
/// caller's arbitrary column/error text, even if it looks like an identifier.
fn persistence_column(value: &str) -> Option<&'static str> {
    const COLUMNS: &[&str] = &[
        "active_continuation_id",
        "activity_seq",
        "agent_id",
        "archived_at",
        "arrangement",
        "attachment_issues",
        "author_id",
        "before_orch",
        "before_snapshot_id",
        "blocked_detail",
        "blocked_reason",
        "cancelled_by",
        "chat_refs",
        "client_id",
        "command_cleanup",
        "completed_at",
        "content",
        "context",
        "continuation_id",
        "created_at",
        "credential_path",
        "current_segment_id",
        "current_turn_id",
        "deleted_at",
        "delivered_at",
        "delivery",
        "effort",
        "end_manifest_blob",
        "end_reason",
        "ended_at",
        "env_generation",
        "environment_id",
        "error",
        "execution_revision",
        "executor",
        "files_end_manifest",
        "id",
        "import_fidelity",
        "import_report",
        "input_id",
        "interrupted_by",
        "kind",
        "last_activity_at",
        "lease_id",
        "limit_bypass",
        "links",
        "model",
        "model_input_id",
        "name",
        "native_action",
        "native_ref",
        "native_session_ref",
        "native_turn_ref",
        "next_seq",
        "node_id",
        "ordinal",
        "org_id",
        "owner_id",
        "params",
        "parent_id",
        "parent_item_id",
        "payload",
        "pending_notices",
        "phase",
        "phase_at",
        "pinned_at",
        "pinned_by",
        "position",
        "provider_id",
        "queue_held",
        "queue_held_at",
        "queue_held_by",
        "queue_revision",
        "search_text",
        "segment_id",
        "seq_created",
        "seq_state",
        "seq_updated",
        "side_from_turn",
        "side_of",
        "side_reading",
        "source",
        "started_at",
        "started_by",
        "starting_step",
        "state",
        "state_detail",
        "status",
        "stream",
        "thread_id",
        "tool_set",
        "topic_id",
        "turn_id",
        "updated_at",
        "usage",
        "usage_detail",
        "waited_ms",
        "waiting_since",
        "work_observed",
        "workspace_dir",
    ];
    COLUMNS
        .iter()
        .copied()
        .find(|column| value == *column || value == format!("\"{column}\""))
}

/// Typed categories only: SQLx can fail before PostgreSQL emits any ERROR.
/// This boundary deliberately never formats the error or its nested source.
fn persistence_facts(error: &orch_db::DbError) -> PersistenceFacts {
    use orch_db::DbError;
    let mut facts = PersistenceFacts {
        category: "database",
        sqlstate: None,
        constraint: None,
        column: None,
    };
    let (state, constraint) = match error {
        DbError::Unique { constraint } => (Some("23505".to_owned()), Some(constraint.as_str())),
        DbError::Reference { constraint } => (Some("23503".to_owned()), Some(constraint.as_str())),
        DbError::Check { constraint, .. } => (Some("23514".to_owned()), Some(constraint.as_str())),
        // The shared conversion collapses multiple SQLSTATEs into these
        // variants. Do not invent an exact one after that information is lost.
        DbError::Rule { constraint } => {
            facts.category = "database_rule";
            (None, Some(constraint.as_str()))
        }
        DbError::Retry => {
            facts.category = "transaction_retry";
            (None, None)
        }
        DbError::Other(error) => {
            if let Some(database) = error.as_database_error() {
                facts.column = database
                    .try_downcast_ref::<sqlx::postgres::PgDatabaseError>()
                    .and_then(sqlx::postgres::PgDatabaseError::column)
                    .and_then(persistence_column);
                (
                    database
                        .code()
                        .filter(|code| {
                            code.len() == 5
                                && code
                                    .bytes()
                                    .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
                        })
                        .map(|code| code.into_owned()),
                    database.constraint(),
                )
            } else {
                facts.category = match error {
                    sqlx::Error::ColumnNotFound(column) => {
                        facts.column = persistence_column(column);
                        "client_column_not_found"
                    }
                    sqlx::Error::ColumnDecode { index, .. } => {
                        facts.column = persistence_column(index);
                        "client_column_decode"
                    }
                    sqlx::Error::RowNotFound => "client_row_not_found",
                    sqlx::Error::Decode(_) => "client_decode",
                    sqlx::Error::PoolClosed => "pool_closed",
                    sqlx::Error::PoolTimedOut => "pool_timeout",
                    sqlx::Error::Io(_) => "client_io",
                    sqlx::Error::Protocol(_) => "client_protocol",
                    _ => "client_other",
                };
                (None, None)
            }
        }
    };
    facts.sqlstate = state;
    facts.constraint = constraint.and_then(schema_identity);
    facts
}

/// How a phase that did not fail ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Done {
    /// It did its work.
    Finished,
    /// It had nothing to do, or could not be done and does not stop the job; the sentence is shown.
    Skipped(String),
}

/// What became of what a job made, after it failed or was cancelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Undone {
    /// All of it was removed again.
    Everything,
    /// Something is left, said in a sentence.
    Partly(String),
    /// The same owned undo must be retried; keep its durable abort intent and hold visible.
    Deferred(String),
}

/// The phases of one kind of job.
#[async_trait]
pub trait Phased: Send + Sync {
    /// Does a phase. It is restartable: asked again, it finds what it did before.
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop>;

    /// Undoes what the job made, after a failure or a cancel before its commit.
    async fn undo(&self, job: &Job, why: &Stop) -> Undone;

    /// What the job produced, once every phase is done.
    async fn result(&self, _job: &Job) -> Option<Json> {
        None
    }

    /// Factual irreversible outcomes that remain after failure or cancellation; ordinary changes have none.
    async fn partial_result(&self, _job: &Job) -> Option<Json> {
        None
    }
}

// ---------------------------------------------------------------- progress

/// The weights of `weights` scaled so that they add up to one hundred; what rounding leaves over goes
/// to the heaviest phase.
#[must_use]
pub fn scaled(weights: &[u32]) -> Vec<u32> {
    let total: u32 = weights.iter().sum();
    if total == 0 {
        return vec![0; weights.len()];
    }
    let mut shares: Vec<u32> = weights
        .iter()
        .map(|weight| weight * WHOLE / total)
        .collect();
    let left = WHOLE.saturating_sub(shares.iter().sum());
    let heaviest = weights
        .iter()
        .enumerate()
        .max_by_key(|(index, weight)| (**weight, std::cmp::Reverse(*index)))
        .map(|(index, _)| index);
    if let Some(index) = heaviest
        && let Some(share) = shares.get_mut(index)
    {
        *share += left;
    }
    shares
}

/// The progress of a job that has not begun: its phases in order, all pending.
#[must_use]
pub fn initial_progress(phases: &[Phase]) -> JobProgress {
    let shares = scaled(&phases.iter().map(|phase| phase.weight).collect::<Vec<_>>());
    JobProgress {
        activity: None,
        phase: phases
            .first()
            .map(|phase| phase.name.to_owned())
            .unwrap_or_default(),
        phases: phases
            .iter()
            .zip(shares)
            .map(|(phase, weight)| orch_api_contract::workspace::JobPhase {
                name: phase.name.to_owned(),
                weight,
                state: PhaseState::Pending,
            })
            .collect(),
        percent: 0,
        files_done: 0,
        files_total: None,
        bytes_done: 0,
        bytes_total: None,
        rate_bps: None,
        eta_seconds: None,
        message: None,
    }
}

/// The percentage of a job whose running phase is `fraction` of the way: the weights of the phases
/// that are done, and that fraction of the weight of the one that runs; never below what was shown.
#[must_use]
pub fn percent_of(progress: &JobProgress, fraction: f64) -> u32 {
    let fraction = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut sum = 0.0_f64;
    for phase in &progress.phases {
        match phase.state {
            PhaseState::Done => sum += f64::from(phase.weight),
            PhaseState::Running => sum += f64::from(phase.weight) * fraction,
            PhaseState::Pending | PhaseState::Skipped => {}
        }
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the sum is between 0 and 100 by construction: weights add up to 100 and the fraction is clamped"
    )]
    let whole = sum.floor() as u32;
    whole.max(progress.percent).min(WHOLE)
}

/// What the row holds as a job's progress, with the end of what a phase printed beside it.
fn stored(progress: &JobProgress, log: Option<&str>) -> Json {
    let mut value = serde_json::to_value(progress).unwrap_or(Json::Null);
    if let (Some(object), Some(log)) = (value.as_object_mut(), log) {
        object.insert("log".to_owned(), Json::String(log.to_owned()));
    }
    value
}

/// A job's progress as the row holds it, and the end of what a phase printed.
#[must_use]
pub fn read_progress(row: &JobRow) -> (JobProgress, Option<String>) {
    let log = row
        .progress
        .get("log")
        .and_then(Json::as_str)
        .map(str::to_owned);
    let progress =
        serde_json::from_value(row.progress.clone()).unwrap_or_else(|_| initial_progress(&[]));
    (progress, log)
}

// ---------------------------------------------------------------- making a job

/// Why a job could not be made.
#[derive(Debug, thiserror::Error)]
pub enum MakeError {
    /// A job of the kinds that run one at a time is not over for that environment.
    #[error("a job is running for this environment already")]
    Running(Box<JobRow>),
    /// The environment is not there.
    #[error("there is no such environment")]
    NoEnvironment,
    /// The database failed.
    #[error(transparent)]
    Db(#[from] orch_db::DbError),
}

/// A job to make.
#[derive(Debug, Clone)]
pub struct Making<'a> {
    /// What it does.
    pub kind: &'static str,
    /// The environment it works on.
    pub environment: Id,
    /// What it is asked to do.
    pub plan: Json,
    /// Its phases.
    pub phases: &'a [Phase],
    /// Who made it.
    pub created_by: Option<Id>,
}

/// Makes a job in the caller's transaction, queued, unless a job of the kinds that run one at a
/// time is not over for the environment: then it is [`MakeError::Running`], naming that job. The
/// environment's row is locked first, so that two requests cannot both find it free.
pub async fn make(
    conn: &mut orch_db::Conn,
    making: &Making<'_>,
    now: DateTime<Utc>,
) -> Result<JobRow, MakeError> {
    environments::lock_environment(conn, making.environment)
        .await?
        .ok_or(MakeError::NoEnvironment)?;
    if rows::retained::SERIALIZED.contains(&making.kind)
        && let Some(running) = rows::retained::open_of_environment(conn, making.environment)
            .await?
            .into_iter()
            // Removing a retired unit never delays the current unit's start or replacement.
            .find(|job| {
                !(job.kind == kind::ENVIRONMENT_REMOVE
                    && job
                        .plan
                        .get("runtime")
                        .is_some_and(|value| !value.is_null()))
            })
    {
        return Err(MakeError::Running(Box::new(running)));
    }
    let progress = serde_json::to_value(initial_progress(making.phases)).unwrap_or(Json::Null);
    Ok(rows::insert(
        conn,
        &NewJob {
            id: new_id(),
            kind: making.kind,
            node_id: None,
            environment_id: Some(making.environment),
            provider_id: None,
            plan: &making.plan,
            progress: &progress,
            created_by: making.created_by,
        },
        now,
    )
    .await?)
}

/// Applies how far a host says a piece of work has come to the job it was done for: while the job's
/// image phase runs, the host's percentage is how far that phase is, and the end of what it printed is
/// kept on the job. A report for a job that is over, or about another phase, changes nothing.
pub(crate) async fn report_progress(
    conn: &mut orch_db::Conn,
    machine: Id,
    job: Id,
    command_id: Option<Id>,
    reported: &Json,
    log: Option<&str>,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    if !orch_db::queries::machine_reports::owns_host_job(conn, machine, job).await? {
        return Ok(());
    }
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
    let (mut progress, mut kept) = read_progress(&row);
    let before = progress.clone();
    let Ok(reported) = serde_json::from_value::<JobProgress>(reported.clone()) else {
        return Ok(());
    };
    if reported
        .activity
        .as_ref()
        .and_then(|activity| activity.directories_done)
        .is_some_and(|count| count < 0)
        || reported.files_done < 0
        || reported.bytes_done < 0
        || reported.files_total.is_some_and(|total| total < 0)
        || reported.bytes_total.is_some_and(|total| total < 0)
        || reported.rate_bps.is_some_and(|rate| rate < 0)
        || reported.eta_seconds.is_some_and(|eta| eta < 0)
    {
        return Ok(());
    }
    let host_phase = reported.phase == progress.phase
        || matches!(
            row.plan
                .pointer("/active_command/kind")
                .and_then(Json::as_str),
            Some(
                "fs.snapshot"
                    | "fs.materialize"
                    | "fs.data.export"
                    | "fs.data.import"
                    | "fs.restore"
                    | "git.worktree.snapshot"
            )
        );
    let declared = command_id.is_some()
        && host_phase
        && row
            .plan
            .pointer("/active_command/reported")
            .and_then(Json::as_bool)
            == Some(true)
        && row
            .plan
            .pointer("/active_command/phase")
            .and_then(Json::as_str)
            == Some(progress.phase.as_str())
        && row
            .plan
            .pointer("/active_command/machine")
            .and_then(Json::as_str)
            .is_some_and(|id| id == machine.to_string());
    let phase_runs = progress
        .phases
        .iter()
        .any(|phase| phase.name == progress.phase && phase.state == PhaseState::Running)
        && ((matches!(reported.phase.as_str(), "image" | "tools")
            && reported.phase == progress.phase)
            || declared);
    if !phase_runs {
        return Ok(());
    }
    // One tools phase may download several builds. Its producer reports a single download's
    // byte progress; the provisioning loop advances the phase only after each verified build.
    if reported.phase != "tools" {
        let from = row
            .plan
            .pointer("/active_command/span/from")
            .and_then(Json::as_f64)
            .unwrap_or(0.0);
        let to = row
            .plan
            .pointer("/active_command/span/to")
            .and_then(Json::as_f64)
            .unwrap_or(1.0);
        // Copied bytes do not prove that the command's final checks and cleanup have ended.
        // Keep the last point of a declared file command for its controller's checkpoint.
        let reported_limit = if declared { WHOLE - 1 } else { WHOLE };
        progress.percent = percent_of(
            &progress,
            from + (to - from) * f64::from(reported.percent.min(reported_limit)) / f64::from(WHOLE),
        );
    }
    progress.bytes_done = reported.bytes_done;
    progress.bytes_total = reported.bytes_total;
    progress.files_done = reported.files_done;
    progress.files_total = reported.files_total;
    progress.rate_bps = reported.rate_bps;
    progress.eta_seconds = reported.eta_seconds;
    let directories_done = reported
        .activity
        .as_ref()
        .and_then(|activity| activity.directories_done);
    let changed = progress.files_total != before.files_total
        || progress.bytes_total != before.bytes_total
        || progress.files_done != before.files_done
        || progress.bytes_done != before.bytes_done
        || progress.percent != before.percent
        || (reported.message.is_some() && before.message != reported.message)
        || progress
            .activity
            .as_ref()
            .and_then(|activity| activity.directories_done)
            != directories_done;
    if changed || progress.activity.is_none() {
        progress.activity = Some(orch_api_contract::workspace::JobActivity {
            waiting_for: None,
            updated_at: orch_types::Time::new(at),
            stall_after_seconds: 0,
            directories_done,
            stopping: false,
        });
    }
    if reported.message.is_some() {
        progress.message = reported.message;
    }
    if let Some(log) = log {
        kept = Some(log.to_owned());
    }
    rows::save_progress(conn, job, &stored(&progress, kept.as_deref()), at).await?;
    Ok(())
}

// Only the exact data selected by this command supplies its total; workspace totals never stand
// in for a home, private layer or the data displaced on a mount's target.
fn command_totals(
    plan: &Json,
    command: &Outgoing,
) -> Result<(Option<i64>, Option<i64>), EngineError> {
    if command.kind == orch_link_contract::commands::CommandKind::FsWrite
        && let Some(upload) = plan.get("workspace_upload")
        && upload.pointer("/write/blob") == command.body.get("blob")
    {
        return Ok((Some(1), upload["size"].as_i64()));
    }
    if let Some(mount) = command.body["mount_id"]
        .as_str()
        .and_then(|value| value.parse::<Id>().ok())
    {
        let copies = crate::mount_copies::of_plan(plan)?;
        if let Some(copy) = copies.copies.iter().find(|copy| copy.mount_id == mount) {
            let totals = if command.machine == copy.source_machine
                || command.body.get("archive").is_some()
            {
                (Some(copy.file_count), Some(copy.total_bytes))
            } else {
                copy.kept
                    .as_ref()
                    .map_or((None, None), |kept| (kept.file_count, kept.total_bytes))
            };
            return Ok(if totals == (Some(0), Some(0)) {
                (None, None)
            } else {
                totals
            });
        }
    }
    let files = plan
        .get("files")
        .or_else(|| plan.pointer("/forkplan/files"));
    let Some(files) = files else {
        return Ok((None, None));
    };
    let selected = command.body["volumes"]
        .as_array()
        .cloned()
        .or_else(|| {
            command
                .body
                .get("selected")
                .map(|value| vec![value.clone()])
        })
        .unwrap_or_default();
    if selected.is_empty() {
        return Ok((None, None));
    }
    let mut total_files = Some(0i64);
    let mut total_bytes = Some(0i64);
    for selected in selected {
        let selector = selected["selector"].as_str().unwrap_or_default();
        let value = match selector {
            "home" => files.get("home").filter(|value| value["measured"] == true),
            "file_states" => files
                .get("file_states")
                .filter(|value| value["complete"] == true),
            _ => selector.strip_prefix("private:").and_then(|id| {
                files["private_views"].as_array().and_then(|views| {
                    views
                        .iter()
                        .find(|view| view["mount"]["id"] == id && view["measured"] == true)
                })
            }),
        };
        total_files = total_files
            .zip(value.and_then(|value| value["file_count"].as_i64()))
            .map(|(a, b)| a.saturating_add(b));
        total_bytes = total_bytes
            .zip(value.and_then(|value| value["total_bytes"].as_i64()))
            .map(|(a, b)| a.saturating_add(b));
    }
    Ok((total_files, total_bytes))
}

// ---------------------------------------------------------------- the running job

#[derive(Debug, Default)]
struct Flags {
    cancelled: AtomicBool,
    lost: AtomicBool,
}

/// A job as its runner holds it: what a phase works through.
#[derive(Debug, Clone)]
pub struct Job {
    engine: Engine,
    id: Id,
    runner: Id,
    flags: Arc<Flags>,
}

impl Job {
    /// A handle to a claimed runner; every metadata write still checks the stored claim.
    pub(crate) fn claimed(engine: Engine, id: Id, runner: Id) -> Self {
        Self {
            engine,
            id,
            runner,
            flags: Arc::new(Flags::default()),
        }
    }

    /// Locks the authoritative claim on the caller's connection, without acquiring another one.
    /// Cancellation does not revoke ownership: an owned cancellation must still be undone and ended.
    pub(crate) async fn lock_owned(
        &self,
        conn: &mut orch_db::Conn,
    ) -> Result<Option<JobRow>, EngineError> {
        let row = rows::lock(conn, self.id).await?;
        let owned = row.as_ref().is_some_and(|row| {
            let runner = row.plan.get("runner");
            row.state == state::RUNNING
                && runner
                    .and_then(|runner| runner.get("id"))
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse::<Id>().ok())
                    == Some(self.runner)
                && runner
                    .and_then(|runner| runner.get("until"))
                    .and_then(|until| serde_json::from_value::<DateTime<Utc>>(until.clone()).ok())
                    .is_some_and(|until| until > self.now())
        });
        if !owned || self.flags.lost.load(Ordering::SeqCst) || self.engine.is_stopped() {
            self.flags.lost.store(true, Ordering::SeqCst);
            return Ok(None);
        }
        Ok(row)
    }

    /// The job's identifier.
    #[must_use]
    pub const fn id(&self) -> Id {
        self.id
    }

    /// The engine the job runs in.
    #[must_use]
    pub const fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The moment, by the engine's clock.
    #[must_use]
    pub fn now(&self) -> DateTime<Utc> {
        self.engine.inner.clock.now()
    }

    /// Whether the phase may go on: not cancelled, not taken over, and the engine not stopping.
    pub fn check(&self) -> Result<(), Stop> {
        self.check_with(true)
    }

    fn check_with(&self, respect_cancel: bool) -> Result<(), Stop> {
        if self.flags.lost.load(Ordering::SeqCst) || self.engine.is_stopped() {
            return Err(Stop::Lost);
        }
        if respect_cancel && self.flags.cancelled.load(Ordering::SeqCst) {
            return Err(Stop::Cancelled);
        }
        Ok(())
    }

    /// The job's row as it is now.
    pub async fn row(&self) -> Result<JobRow, EngineError> {
        let mut conn = self.engine.inner.db.acquire().await?;
        rows::job(&mut conn, self.id)
            .await?
            .ok_or_else(|| EngineError::unreadable("a job that is being run is gone"))
    }

    /// The job's plan as it is now: what it was asked to do and what its runner decided.
    pub async fn plan(&self) -> Result<Json, EngineError> {
        Ok(self.row().await?.plan)
    }

    /// Writes members into the plan, which later phases and a later runner find.
    pub async fn remember(&self, members: &Json) -> Result<(), EngineError> {
        let mut tx = self.engine.inner.db.begin().await?;
        if self.lock_owned(&mut tx).await?.is_none() {
            return Ok(());
        }
        rows::merge_plan(&mut tx, self.id, members, self.now()).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// Freezes the original failure/cancellation before cleanup can have a destructive effect.
    /// A takeover inherits this intent even when apply was already checkpointed Done.
    pub(crate) async fn begin_undo(
        &self,
        why: &Stop,
    ) -> Result<Option<(JobRow, Stop)>, EngineError> {
        let intent = match why {
            Stop::Failed(failure) => Abort::Failed(failure.clone()),
            Stop::Cancelled => Abort::Cancelled,
            Stop::Lost => return Ok(None),
        };
        let mut tx = self.engine.db().begin().await?;
        let Some(mut row) = self.lock_owned(&mut tx).await? else {
            return Ok(None);
        };
        let reason = if let Some(reason) = abort_reason(&row)? {
            reason
        } else {
            let value = serde_json::to_value(intent)?;
            rows::merge_plan(&mut tx, self.id, &json!({ "abort": value }), self.now()).await?;
            row.plan["abort"] = value;
            why.clone()
        };
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Some((row, reason)))
    }

    /// Passes the point after which the job is not cancelled and not undone. Returns whether it was
    /// passed: a request to cancel that was made before is honoured instead.
    pub async fn pass_commit(&self) -> Result<(), Stop> {
        let mut tx = self.engine.inner.db.begin().await?;
        let row = self.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        if let Some(reason) = abort_reason(&row)? {
            return Err(reason);
        }
        if rows::pass_commit(&mut tx, self.id, self.now()).await? {
            tx.commit().await.map_err(orch_db::DbError::from)?;
            Ok(())
        } else {
            self.flags.cancelled.store(true, Ordering::SeqCst);
            Err(Stop::Cancelled)
        }
    }

    /// Changes the progress of the job in a transaction that holds its row, and writes it with the
    /// next number of its environment's feed.
    pub async fn amend(
        &self,
        change: impl FnOnce(&mut JobProgress, &mut Option<String>) + Send,
    ) -> Result<(), EngineError> {
        let mut tx = self.engine.inner.db.begin().await?;
        let Some(row) = self.lock_owned(&mut tx).await? else {
            return Ok(());
        };
        let (mut progress, mut log) = read_progress(&row);
        let before = progress.clone();
        change(&mut progress, &mut log);
        if progress != before {
            let waiting_for = progress
                .activity
                .as_ref()
                .and_then(|activity| activity.waiting_for.clone());
            let directories_done = progress
                .activity
                .as_ref()
                .and_then(|activity| activity.directories_done);
            progress.activity = Some(orch_api_contract::workspace::JobActivity {
                waiting_for,
                updated_at: orch_types::Time::new(self.now()),
                stall_after_seconds: 0,
                directories_done,
                stopping: row.cancel_requested(),
            });
        }
        rows::save_progress(
            &mut tx,
            self.id,
            &stored(&progress, log.as_deref()),
            self.now(),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// Says how far the running phase is (from nothing to one) and, if there is something to say, what
    /// it is doing. The percentage follows, and never goes down.
    pub async fn progress(&self, fraction: f64, message: Option<&str>) -> Result<(), Stop> {
        self.check()?;
        let message = message.map(str::to_owned);
        self.amend(move |progress, _| {
            progress.percent = percent_of(progress, fraction);
            if message.is_some() {
                progress.message = message;
            }
        })
        .await?;
        Ok(())
    }

    /// Sends a command and waits for its end, cancelling it if the job is cancelled meanwhile. A
    /// failure of the command is the job's failure, with the code the machine gave.
    pub async fn command(&self, mut command: Outgoing) -> Result<Json, Stop> {
        use orch_link_contract::commands::{CommandKind, WorkReport};
        if matches!(
            command.kind,
            CommandKind::FsWrite
                | CommandKind::NodeMountExport
                | CommandKind::NodeMountImport
                | CommandKind::NodeRuntimeVolumesExport
                | CommandKind::NodeRuntimeVolumeImport
                | CommandKind::FsSnapshot
                | CommandKind::FsMaterialize
                | CommandKind::FsDataExport
                | CommandKind::FsDataImport
                | CommandKind::FsRestore
        ) {
            let row = self.row().await?;
            let (progress, _) = read_progress(&row);
            if command.body.get("work").is_none_or(Json::is_null) {
                let timeout_secs = crate::jobs::measurement::setting(
                    &mut *self
                        .engine
                        .db()
                        .acquire()
                        .await
                        .map_err(EngineError::from)?,
                    orch_settings_contract::registry::keys::JOB_IO_TIMEOUT_SECS,
                )
                .await?;
                let timeout_secs = u64::try_from(timeout_secs)
                    .map_err(|_| EngineError::unreadable("the file-work allowance is invalid"))?;
                let (files_total, bytes_total) = command_totals(&row.plan, &command)?;
                command.body["work"] = serde_json::to_value(WorkReport {
                    files_total,
                    bytes_total,
                    job_id: self.id,
                    phase: progress.phase,
                    timeout_secs,
                })
                .map_err(EngineError::from)?;
                if command.deadline.is_none() {
                    let first = self
                        .engine
                        .inner
                        .commands
                        .status(command.id)
                        .await
                        .map_err(commands_stop)?
                        .map_or(self.now(), |info| info.issued_at);
                    command.deadline = Some(orch_types::Time::new(
                        first
                            + chrono::Duration::seconds(
                                i64::try_from(timeout_secs).unwrap_or(i64::MAX),
                            ),
                    ));
                }
            }
        }
        self.run_command(command, true).await
    }

    /// Gives one command its own part of a phase, so a source read cannot fill the target's copy bar.
    pub async fn command_part(
        &self,
        command: Outgoing,
        span: std::ops::Range<f64>,
    ) -> Result<Json, Stop> {
        self.check()?;
        if !(0.0..=1.0).contains(&span.start) || !(span.start..=1.0).contains(&span.end) {
            return Err(Stop::Failed(Failure::new(
                INTERNAL,
                "the command's progress span is invalid",
            )));
        }
        self.remember(
            &json!({"progress_span":{"command":command.id,"from":span.start,"to":span.end}}),
        )
        .await?;
        self.command(command).await
    }

    /// As [`Job::command`], for the work of undoing: a request to cancel the job does not stop it, since
    /// it is what the job is being undone for.
    pub async fn command_to_undo(&self, command: Outgoing) -> Result<Json, Stop> {
        self.run_command(command, false).await
    }

    /// Checks cancellation before an irreversible item, then confirms that item before stopping between items.
    pub async fn command_irreversible(&self, command: Outgoing) -> Result<Json, Stop> {
        self.check()?;
        // The watch may still be delivering a cancellation committed while the previous
        // object was being removed. Admit each new irreversible object from durable state.
        if self.row().await?.cancel_requested() {
            self.flags.cancelled.store(true, Ordering::SeqCst);
            return Err(Stop::Cancelled);
        }
        self.run_command(command, false).await
    }

    async fn run_command(&self, command: Outgoing, respect_cancel: bool) -> Result<Json, Stop> {
        use orch_link_contract::commands::CommandKind;
        let retry_busy = matches!(
            command.kind,
            CommandKind::EnvDetach | CommandKind::EnvDataRemove | CommandKind::FsDataImport
        );
        let commands = Arc::clone(&self.engine.inner.commands);
        let until = if retry_busy {
            let first = commands
                .status(command.id)
                .await
                .map_err(commands_stop)?
                .map_or_else(|| self.now(), |info| info.issued_at);
            Some(command.deadline.unwrap_or_else(|| {
                orch_types::Time::new(
                    first + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
                )
            }))
        } else {
            None
        };
        let due = until.map(|until| {
            tokio::time::Instant::now() + (until.get() - self.now()).to_std().unwrap_or_default()
        });
        let mut attempt = 0u32;
        let mut current_id = command.id;
        let mut delay_new_attempt = false;
        loop {
            self.check_with(respect_cancel)?;
            // A deadline bounds admitting or waiting for work, not replaying its durable answer.
            // Walk retained Busy children without another pause: a later child may already have
            // succeeded before this phase lost its checkpoint, even if recovery is much later.
            let prior = if retry_busy {
                commands.status(current_id).await.map_err(commands_stop)?
            } else {
                None
            };
            let exists = prior.is_some();
            let mut next = command.clone();
            if let Some(until) = until {
                next.deadline = Some(until);
            }
            next.id = current_id;
            let id = next.id;
            let ended = if let Some(ended) = prior.and_then(|info| info.outcome) {
                ended
            } else {
                if due.is_some_and(|due| tokio::time::Instant::now() >= due)
                    || until.is_some_and(|until| until.get() <= self.now())
                {
                    return Err(command_deadline(attempt));
                }
                if delay_new_attempt && !exists {
                    let pause = due.map_or(crate::commands::BUSY_RETRY_PAUSE, |due| {
                        due.saturating_duration_since(tokio::time::Instant::now())
                            .min(crate::commands::BUSY_RETRY_PAUSE)
                    });
                    delay_new_attempt = false;
                    tokio::time::sleep(pause).await;
                    continue;
                }
                if let Some(due) = due {
                    match tokio::time::timeout_at(due, self.run_command_once(next, respect_cancel))
                        .await
                    {
                        Ok(ended) => ended?,
                        Err(_) => {
                            self.check_with(respect_cancel)?;
                            if let Some(ended) = commands
                                .status(id)
                                .await
                                .map_err(commands_stop)?
                                .and_then(|info| info.outcome)
                            {
                                ended
                            } else {
                                let _ = commands.cancel(id).await;
                                return Err(command_deadline(attempt));
                            }
                        }
                    }
                } else {
                    self.run_command_once(next, respect_cancel).await?
                }
            };
            if retry_busy
                && matches!(&ended, Ended::Failed(error) if error.code == "busy" && error.retryable)
            {
                attempt = attempt.checked_add(1).ok_or_else(|| {
                    Stop::Failed(Failure::new(
                        "busy",
                        "The operation is still busy; retry the job.",
                    ))
                })?;
                current_id = crate::commands::busy_retry_id(current_id);
                // Only a genuinely new attempt waits, outside every key and SQL transaction.
                // Existing attempts keep their body, key, machine and the original deadline.
                delay_new_attempt = true;
                continue;
            }
            return match ended {
                Ended::Succeeded(result) => Ok(result),
                Ended::Failed(error) => Err(Stop::Failed(Failure::new(error.code, error.detail))),
                Ended::Expired => Err(Stop::Failed(Failure::new(
                    "expired",
                    "the host did not take the command up in time",
                ))),
                Ended::Cancelled => Err(
                    if self.flags.cancelled.load(Ordering::SeqCst)
                        || self.row().await?.cancel_requested()
                    {
                        Stop::Cancelled
                    } else {
                        Stop::Failed(Failure::new("cancelled", "the command was cancelled"))
                    },
                ),
            };
        }
    }

    async fn run_command_once(
        &self,
        command: Outgoing,
        respect_cancel: bool,
    ) -> Result<Ended, Stop> {
        self.check_with(respect_cancel)?;
        let commands = Arc::clone(&self.engine.inner.commands);
        let id = command.id;
        if respect_cancel {
            let row = self.row().await?;
            if row.cancel_requested() {
                self.flags.cancelled.store(true, Ordering::SeqCst);
                return Err(Stop::Cancelled);
            }
            let span = row.plan.get("progress_span").filter(|span| {
                span["command"]
                    .as_str()
                    .is_some_and(|value| value == id.to_string())
            });
            let reported = command.body.get("work").is_some_and(|work| !work.is_null())
                || matches!(
                    command.kind,
                    orch_link_contract::commands::CommandKind::FsSnapshot
                        | orch_link_contract::commands::CommandKind::FsMaterialize
                        | orch_link_contract::commands::CommandKind::FsDataExport
                        | orch_link_contract::commands::CommandKind::FsDataImport
                        | orch_link_contract::commands::CommandKind::FsRestore
                        | orch_link_contract::commands::CommandKind::GitWorktreeSnapshot
                );
            self.remember(&json!({"active_command":{"id":id,"machine":command.machine,"kind":command.kind.as_str(),"reported":reported,"phase":read_progress(&row).0.phase,"span":span}})).await?;
        }
        self.check_with(respect_cancel)?;
        commands.issue(command).await.map_err(commands_stop)?;
        let waiting = commands.wait(id);
        tokio::pin!(waiting);
        let mut watch = tokio::time::interval(WATCH_EVERY);
        loop {
            tokio::select! {
                ended = &mut waiting => {
                    let ended = ended.map_err(commands_stop)?;
                    if respect_cancel && !matches!(ended, Ended::Cancelled | Ended::Expired) {
                        self.remember(&json!({"active_command":null})).await?;
                    }
                    return Ok(ended);
                }
                _ = watch.tick() => {
                    if let Err(stop) = self.check_with(respect_cancel) {
                        if stop == Stop::Cancelled {
                            let _ = commands.cancel(id).await;
                        }
                        return Err(stop);
                    }
                }
            }
        }
    }

    /// Waits for `how_long`, ending early with the reason when the job is cancelled or lost.
    pub async fn pause(&self, how_long: Duration) -> Result<(), Stop> {
        let until = tokio::time::Instant::now() + how_long;
        loop {
            self.check()?;
            let left = until.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return Ok(());
            }
            tokio::time::sleep(left.min(WATCH_EVERY)).await;
        }
    }

    /// Resolves once the job may no longer be run by this runner.
    async fn gone(&self) {
        loop {
            if self.flags.lost.load(Ordering::SeqCst) || self.engine.is_stopped() {
                return;
            }
            tokio::time::sleep(WATCH_EVERY).await;
        }
    }

    /// Keeps the lease and reads the request to cancel, until it is aborted.
    async fn watch(self) {
        let lease = self.engine.inner.config.hosting.job_lease;
        let renew_every = lease / 3;
        let mut renewed = tokio::time::Instant::now();
        loop {
            tokio::time::sleep(WATCH_EVERY).await;
            if self.engine.is_stopped() {
                return;
            }
            let Ok(mut conn) = self.engine.inner.db.acquire().await else {
                continue;
            };
            if renewed.elapsed() >= renew_every {
                let now = self.now();
                let until = now + chrono::Duration::from_std(lease).unwrap_or_default();
                match rows::claim(&mut conn, self.id, self.runner, until, now).await {
                    Ok(true) => renewed = tokio::time::Instant::now(),
                    Ok(false) => {
                        self.flags.lost.store(true, Ordering::SeqCst);
                        return;
                    }
                    Err(error) => {
                        tracing::warn!(%error, job = %self.id, "the lease on a job could not be renewed");
                    }
                }
            }
            if let Ok(true) = rows::cancel_requested(&mut conn, self.id).await {
                self.flags.cancelled.store(true, Ordering::SeqCst);
            }
        }
    }
}

/// What a link that cannot carry a command means for a phase.
fn command_deadline(attempt: u32) -> Stop {
    Stop::Failed(Failure::new(
        if attempt == 0 { "timeout" } else { "busy" },
        "The machine did not finish the operation before its deadline. Retry the job.",
    ))
}

fn commands_stop(error: crate::commands::CommandsError) -> Stop {
    use crate::commands::CommandsError;
    match error {
        CommandsError::Stopping => Stop::Lost,
        CommandsError::UnknownMachine | CommandsError::Revoked => Stop::Failed(Failure::new(
            "host_gone",
            "the host's machine is not known to the system any more",
        )),
        other => Stop::Failed(Failure::new(INTERNAL, other.to_string())),
    }
}

// ---------------------------------------------------------------- running jobs

impl Engine {
    /// Takes up the jobs that are due: new ones, and ones whose runner's lease has lapsed. Each is run
    /// in a task of its own, by one process at a time.
    pub(crate) async fn run_jobs(&self) -> Result<(), EngineError> {
        self.expire_measurements().await?;
        self.run_document_notice_jobs().await?;
        let now = self.inner.clock.now();
        let lease =
            chrono::Duration::from_std(self.inner.config.hosting.job_lease).unwrap_or_default();
        let due = {
            let mut conn = self.inner.db.acquire().await?;
            rows::lifecycle_runnable(
                &mut conn,
                &[
                    kind::ENVIRONMENT_PROVISION,
                    kind::ENVIRONMENT_RESTART,
                    crate::jobs::mcp_restart::KIND,
                    crate::jobs::capability_change::KIND,
                    crate::jobs::kept_images::KIND,
                    crate::jobs::cleanup::KIND,
                    crate::resource_pause::KIND,
                    kind::ENVIRONMENT_REMOVE,
                    rows::retained::RESTORE,
                    rows::retained::PURGE,
                    kind::PROVIDER_PROVISION,
                    kind::SNAPSHOT,
                    crate::jobs::measurement::KIND,
                    kind::DELIVERY,
                    kind::WORKSPACE_PREPARE,
                    kind::GIT_COMMIT,
                    kind::GIT_PUSH,
                    kind::WORKSPACE_RESTORE,
                    // The export of a place.
                    kind::EXPORT,
                    // What a person brings into a place, and the taking of it back.
                    kind::IMPORT,
                    crate::jobs::background_end::KIND,
                    crate::jobs::continuation::KIND,
                    crate::jobs::mcp_offering::KIND,
                    crate::jobs::host_delete::KIND,
                ],
                now,
                RUN_BATCH,
            )
            .await?
        };
        for id in due {
            let runner = new_id();
            let taken = {
                let mut conn = self.inner.db.acquire().await?;
                rows::claim(&mut conn, id, runner, now + lease, now).await?
            };
            if taken {
                let engine = self.clone();
                tokio::spawn(async move { engine.run_job(id, runner).await });
            }
        }
        // Runtime updates and moves share the fork runner's bounded batch claims.
        self.run_runtime_changes().await?;
        self.run_fork_jobs().await
    }

    /// Notification work has a separate installation-wide four-slot admission, rather than a fresh batch per sweep.
    async fn run_document_notice_jobs(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let lease =
            chrono::Duration::from_std(self.inner.config.hosting.job_lease).unwrap_or_default();
        let mut tx = self.inner.db.begin().await?;
        let taken = rows::claim_document_notices(&mut tx, now, now + lease).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        for (id, runner) in taken {
            let engine = self.clone();
            tokio::spawn(async move {
                engine.run_job(id, runner).await;
            });
        }
        Ok(())
    }

    /// Takes up the forks and switches that are due (the part that forks and switches). A job of a batch
    /// that is queued waits while as many of its batch run as the setting allows: it is locked already.
    async fn run_fork_jobs(&self) -> Result<(), EngineError> {
        use orch_db::queries::jobs::forks as fork_rows;
        let now = self.inner.clock.now();
        let lease =
            chrono::Duration::from_std(self.inner.config.hosting.job_lease).unwrap_or_default();
        let (parallel, due) = {
            let mut conn = self.inner.db.acquire().await?;
            let parallel = orch_settings::get::<i64>(
                &mut conn,
                orch_settings::Place::System,
                orch_settings_contract::registry::keys::SWITCH_BATCH_PARALLEL,
            )
            .await?
            .unwrap_or(BATCH_PARALLEL_DEFAULT);
            let due = fork_rows::runnable(&mut conn, now, parallel, RUN_BATCH).await?;
            (parallel, due)
        };
        for id in due {
            let runner = new_id();
            let taken = {
                let mut tx = self.inner.db.begin().await?;
                let taken =
                    fork_rows::claim_in_batch(&mut tx, id, runner, now + lease, now, parallel)
                        .await?;
                tx.commit().await.map_err(orch_db::DbError::from)?;
                taken
            };
            if taken {
                let engine = self.clone();
                tokio::spawn(async move { engine.run_job(id, runner).await });
            }
        }
        Ok(())
    }

    // Runtime updates and moves: bounded execution, with holds already taken by confirmation.
    async fn run_runtime_changes(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let lease =
            chrono::Duration::from_std(self.inner.config.hosting.job_lease).unwrap_or_default();
        let (parallel, due) = {
            let mut conn = self.inner.db.acquire().await?;
            let parallel = orch_settings::get::<i64>(
                &mut conn,
                orch_settings::Place::System,
                orch_settings_contract::registry::keys::SWITCH_BATCH_PARALLEL,
            )
            .await?
            .unwrap_or(BATCH_PARALLEL_DEFAULT);
            (
                parallel,
                orch_db::queries::jobs::changes::runnable(&mut conn, now, parallel, RUN_BATCH)
                    .await?,
            )
        };
        for id in due {
            let runner = new_id();
            let mut tx = self.inner.db.begin().await?;
            let taken =
                rows::forks::claim_in_batch(&mut tx, id, runner, now + lease, now, parallel)
                    .await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            if taken {
                let engine = self.clone();
                tokio::spawn(async move {
                    engine.run_job(id, runner).await;
                });
            }
        }
        Ok(())
    }

    /// Asks that jobs be looked at at once, instead of at the next sweep.
    pub fn kick(&self) {
        self.inner.signal.notify_one();
    }

    /// Asks that a job be cancelled: it is undone as a failure is, unless it has passed its commit.
    /// Returns whether the request stands.
    pub async fn cancel_job(&self, job: Id, by: Id) -> Result<bool, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let asked = rows::request_cancel(&mut conn, job, by, self.inner.clock.now()).await?;
        drop(conn);
        if asked {
            self.stop_job_command(job).await?;
            self.kick();
        }
        Ok(asked)
    }

    /// Sends the accepted job cancellation to its active machine command without waiting for cleanup.
    pub async fn stop_job_command(&self, job: Id) -> Result<(), EngineError> {
        let row = rows::job(&mut *self.db().acquire().await?, job).await?;
        if let Some(row) = row.filter(|row| row.cancel_requested())
            && let Some(id) = row
                .plan
                .pointer("/active_command/id")
                .and_then(Json::as_str)
                .and_then(|value| value.parse::<Id>().ok())
        {
            self.inner
                .commands
                .cancel(id)
                .await
                .map_err(|error| EngineError::unreadable(error.to_string()))?;
        }
        Ok(())
    }

    async fn run_job(&self, id: Id, runner: Id) {
        let job = Job::claimed(self.clone(), id, runner);
        let watch = tokio::spawn(job.clone().watch());
        let ended = self.drive_job(&job).await;
        watch.abort();
        if let Err(error) = ended {
            tracing::warn!(%error, %id, "a job could not be carried on; it is taken up again");
        }
        if let Ok(mut conn) = self.inner.db.acquire().await {
            let _ = rows::release(&mut conn, id, runner, self.inner.clock.now()).await;
        }
    }

    /// The phased work of a job of this kind.
    async fn phased_for(&self, row: &JobRow) -> Result<Option<Box<dyn Phased>>, EngineError> {
        Ok(match row.kind.as_str() {
            crate::jobs::continuation::KIND => {
                Some(Box::new(crate::jobs::continuation::Continuing::of(row)?))
            }
            crate::jobs::background_end::KIND => Some(Box::new(
                crate::jobs::background_end::Ending::of(self.clone(), row)?,
            )),
            kind::ENVIRONMENT_PROVISION => Some(Box::new(crate::jobs::provision::Provision::of(
                self.clone(),
                row,
            ))),
            crate::jobs::capability_change::KIND => Some(Box::new(
                crate::jobs::capability_change::Applying::of(self.clone(), row)?,
            )),
            crate::jobs::kept_images::KIND => Some(Box::new(
                crate::jobs::kept_images::Sweeping::of(self.clone(), row)?,
            )),
            crate::jobs::cleanup::KIND => Some(Box::new(crate::jobs::cleanup::Cleaning::of(
                self.clone(),
                row,
            )?)),
            crate::resource_pause::KIND => Some(Box::new(crate::resource_pause::Quiescing::of(
                self.clone(),
                row,
            )?)),
            // A managed provider's runtime (the part that keeps providers): its making, its start and the
            // renewal of its sign-in, and the removal of a unit it retired.
            kind::PROVIDER_PROVISION => Some(Box::new(
                crate::jobs::provider_provision::ProviderProvision::of(self.clone(), row),
            )),
            kind::ENVIRONMENT_REMOVE if row.provider_id.is_some() => Some(Box::new(
                crate::jobs::provider_provision::ProviderRemoval::of(self.clone(), row),
            )),
            kind::ENVIRONMENT_REMOVE => Some(Box::new(crate::jobs::remove::Removal::of(
                self.clone(),
                row,
            ))),
            rows::retained::RESTORE | rows::retained::PURGE => Some(Box::new(
                crate::jobs::retained::Kept::of(self.clone(), row)?,
            )),
            kind::SNAPSHOT => Some(Box::new(crate::jobs::snapshot::Snapshotting::of(
                self.clone(),
                row,
            )?)),
            crate::jobs::document_notices::KIND => Some(Box::new(
                crate::jobs::document_notices::Delivering::of(self.clone(), row)?,
            )),
            crate::jobs::measurement::KIND => Some(Box::new(crate::jobs::measurement::Measuring)),
            kind::DELIVERY if row.plan.get("workspace_upload").is_some() => {
                Some(Box::new(crate::jobs::workspace_upload::Upload::of(row)?))
            }
            kind::DELIVERY => Some(Box::new(crate::delivery::Delivery::of(row)?)),
            kind::WORKSPACE_PREPARE | kind::GIT_COMMIT | kind::GIT_PUSH => Some(Box::new(
                crate::jobs::source::Preparing::of(self.clone(), row)?,
            )),
            kind::WORKSPACE_RESTORE => Some(Box::new(crate::jobs::restore::Restoring::of(
                self.clone(),
                row,
            )?)),
            // The part that forks and switches.
            crate::jobs::fork::KIND => Some(Box::new(crate::jobs::fork::Forking::of(row)?)),
            // The export of a place.
            kind::EXPORT => Some(Box::new(crate::jobs::export::Exporting::of(
                self.clone(),
                row,
            )?)),
            // What a person brings into a place, and the taking of it back.
            kind::IMPORT => Some(Box::new(crate::jobs::import::Importing::of(
                self.clone(),
                row,
            )?)),
            // Runtime updates and recreations preserve the old unit until commit.
            kind::ENVIRONMENT_RESTART => Some(Box::new(crate::jobs::restart::Restarting::of(row)?)),
            kind::ENVIRONMENT_RECREATE | kind::PROVIDER_RECREATE => {
                Some(Box::new(crate::jobs::recreate::Recreating::of(row)?))
            }
            kind::ENVIRONMENT_MOVE => {
                if row.plan.get("portable").is_some() {
                    Some(Box::new(crate::jobs::machine_move::Moving::of(row)?))
                } else {
                    Some(Box::new(crate::jobs::environment_move::Moving::of(row)?))
                }
            }
            crate::jobs::host_delete::KIND => {
                Some(Box::new(crate::jobs::host_delete::Deleting::of(row)?))
            }
            crate::jobs::mcp_offering::KIND => {
                Some(Box::new(crate::jobs::mcp_offering::Offering::of(row)?))
            }
            crate::jobs::mcp_restart::KIND => {
                Some(Box::new(crate::jobs::mcp_restart::Restarting::of(row)?))
            }
            _ => None,
        })
    }

    pub(crate) async fn drive_job(&self, job: &Job) -> Result<(), EngineError> {
        let row = job.row().await?;
        if !row.is_open() {
            return Ok(());
        }
        // A request to cancel that was made before the job was taken up is known at once: the watch
        // that reads it from the row looks only after its first pause, and by then a phase would have
        // sent its command.
        if row.cancel_requested() {
            job.flags.cancelled.store(true, Ordering::SeqCst);
        }
        // Runtime changes with an obsolete or damaged plan fail visibly instead of holding work forever.
        let prepared = match self.phased_for(&row).await {
            Ok(prepared) => prepared,
            Err(error)
                if matches!(
                    row.kind.as_str(),
                    kind::ENVIRONMENT_RECREATE
                        | kind::PROVIDER_RECREATE
                        | kind::ENVIRONMENT_MOVE
                        | kind::ENVIRONMENT_RESTART
                ) =>
            {
                return self
                    .end_job(
                        job,
                        None,
                        Stop::Failed(Failure::new("invalid_plan", error.to_string())),
                    )
                    .await;
            }
            Err(error) => return Err(error),
        };
        let Some(phased) = prepared else {
            return self
                .end_job(
                    job,
                    None,
                    Stop::Failed(Failure::new(
                        INTERNAL,
                        format!("this system does not run jobs of the kind {}", row.kind),
                    )),
                )
                .await;
        };
        if let Some(reason) = abort_reason(&row)? {
            return self.end_job(job, Some(phased.as_ref()), reason).await;
        }
        let names: Vec<String> = read_progress(&row)
            .0
            .phases
            .iter()
            .map(|phase| phase.name.clone())
            .collect();
        for name in names {
            let (progress, _) = read_progress(&job.row().await?);
            let state_of = progress
                .phases
                .iter()
                .find(|phase| phase.name == name)
                .map(|phase| phase.state);
            if matches!(state_of, Some(PhaseState::Done | PhaseState::Skipped)) {
                continue;
            }
            if let Err(stop) = job.check() {
                return self.end_job(job, Some(phased.as_ref()), stop).await;
            }
            let running = name.clone();
            job.amend(move |progress, _| {
                progress.phase.clone_from(&running);
                for phase in &mut progress.phases {
                    if phase.name == running {
                        phase.state = PhaseState::Running;
                    }
                }
                progress.message = None;
                progress.files_done = 0;
                progress.files_total = None;
                progress.bytes_done = 0;
                progress.bytes_total = None;
                progress.rate_bps = None;
                progress.eta_seconds = None;
                progress.activity = None;
            })
            .await?;
            let mut attempts = 0;
            let result = loop {
                let step = tokio::select! {
                    done = crate::notifications::operation_scope(row.plan.get("notification_operation").and_then(Json::as_str).and_then(|id|id.parse().ok()),orch_db::queries::transaction_guard::request_scope(phase_scope(&row.kind), phased.run(job, &name))) => done,
                    () = job.gone() => Err(Stop::Lost),
                };
                match step {
                    Err(Stop::Failed(ref failure))
                        if failure.code == INTERNAL && attempts < PHASE_RETRIES =>
                    {
                        attempts += 1;
                        tracing::warn!(job = %job.id, phase = %name, message = %failure.message, "a phase failed on the system's side; it is asked again");
                        tokio::time::sleep(PHASE_RETRY_PAUSE).await;
                    }
                    other => break other,
                }
            };
            match result {
                Ok(done) => {
                    let finished = name.clone();
                    let skipped = match done {
                        Done::Finished => None,
                        Done::Skipped(why) => Some(why),
                    };
                    job.amend(move |progress, _| {
                        for phase in &mut progress.phases {
                            if phase.name == finished {
                                phase.state = if skipped.is_some() {
                                    PhaseState::Skipped
                                } else {
                                    PhaseState::Done
                                };
                            }
                        }
                        if skipped.is_some() {
                            progress.message = skipped;
                        }
                        progress.percent = percent_of(progress, 0.0);
                    })
                    .await?;
                }
                Err(stop) => return self.end_job(job, Some(phased.as_ref()), stop).await,
            }
        }
        let result = phased.result(job).await;
        self.finish_job(job, result.as_ref()).await
    }

    /// Commits a successful terminal result only while this exact runner still owns the job.
    pub(crate) async fn finish_job(
        &self,
        job: &Job,
        result: Option<&Json>,
    ) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let Some(finished_row) = job.lock_owned(&mut tx).await? else {
            return Ok(());
        };
        if abort_reason(&finished_row)?.is_some() {
            return Ok(());
        }
        let (mut progress, log) = read_progress(&finished_row);
        progress.percent = WHOLE;
        if !rows::finish(
            &mut tx,
            job.id,
            state::SUCCEEDED,
            &stored(&progress, log.as_deref()),
            result,
            None,
            self.inner.clock.now(),
        )
        .await?
        {
            return Ok(());
        }
        crate::notifications::job_finished(&mut tx, job.id, self.inner.clock.now()).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        // Completion hooks need the phase writes and terminal state, including committed captures.
        let closed = job.row().await?;
        self.after_job(&closed).await;
        Ok(())
    }

    /// Ends a job that did not go through: undone as far as it can be, and the reason kept.
    pub(crate) async fn end_job(
        &self,
        job: &Job,
        phased: Option<&dyn Phased>,
        stop: Stop,
    ) -> Result<(), EngineError> {
        if stop == Stop::Lost {
            return Ok(());
        }
        // Persist the terminal direction before admitting cleanup, and keep its first reason.
        // The transaction is over before any machine/provider/file/timer wait begins.
        let Some((row, stop)) = job.begin_undo(&stop).await? else {
            return Ok(());
        };
        if let Some(active) = row
            .plan
            .get("active_command")
            .filter(|value| !value.is_null())
        {
            let command = active["id"]
                .as_str()
                .and_then(|value| value.parse::<Id>().ok())
                .ok_or_else(|| EngineError::unreadable("the stopping command has no identity"))?;
            let machine = active["machine"]
                .as_str()
                .and_then(|value| value.parse::<Id>().ok())
                .ok_or_else(|| EngineError::unreadable("the stopping command has no machine"))?;
            let commands = &self.inner.commands;
            let _ = commands.cancel(command).await;
            let seconds = crate::jobs::measurement::setting(
                &mut *self.db().acquire().await?,
                orch_settings_contract::registry::keys::JOB_STOP_WAIT_SECS,
            )
            .await?;
            let until = tokio::time::Instant::now()
                + Duration::from_secs(u64::try_from(seconds).unwrap_or_default());
            while !commands.stopped(command).await.unwrap_or(false) {
                if tokio::time::Instant::now() >= until {
                    let name = orch_db::queries::resources::machine(
                        &mut *self.db().acquire().await?,
                        machine,
                    )
                    .await?
                    .map_or_else(|| machine.to_string(), |machine| machine.name);
                    job.amend(move |progress, _| {
                        progress.message = Some(format!(
                            "Still stopping on {name}; waiting for the machine to confirm."
                        ))
                    })
                    .await?;
                    return Ok(());
                }
                if job.check_with(false).is_err() {
                    return Ok(());
                }
                tokio::time::sleep(WATCH_EVERY).await;
            }
            job.remember(&json!({"active_command":null})).await?;
        }
        let undone = match phased {
            Some(phased) => {
                if row.kind == kind::ENVIRONMENT_PROVISION {
                    crate::jobs::source::undo_provision_sources(job, &stop).await?;
                }
                orch_db::queries::transaction_guard::request_scope(
                    phase_scope(&row.kind),
                    phased.undo(job, &stop),
                )
                .await
            }
            None => Undone::Everything,
        };
        if let Undone::Deferred(message) = undone {
            job.amend(move |progress, _| progress.message = Some(message))
                .await?;
            // The runner releases its lease normally; the next bounded sweep inherits Abort.
            // This is not a successful cleanup or a terminal failure that could release its hold.
            return Ok(());
        }
        let (to, failure) = match &stop {
            Stop::Cancelled => (
                state::CANCELLED,
                Failure::new("cancelled", "the job was cancelled"),
            ),
            Stop::Failed(failure) => (state::FAILED, failure.clone()),
            Stop::Lost => return Ok(()),
        };
        let partial = match phased {
            Some(phased) => phased.partial_result(job).await,
            None => None,
        };
        let mut tx = self.inner.db.begin().await?;
        let Some(row) = job.lock_owned(&mut tx).await? else {
            return Ok(());
        };
        let (progress, mut log) = read_progress(&row);
        if failure.log.is_some() {
            log.clone_from(&failure.log);
        }
        let error = json!({
            "code": failure.code,
            "message": failure.message,
            "phase": progress.phase,
            "undone": undone == Undone::Everything,
        });
        if !rows::finish(
            &mut tx,
            job.id,
            to,
            &stored(&progress, log.as_deref()),
            partial.as_ref(),
            Some(&error),
            self.inner.clock.now(),
        )
        .await?
        {
            return Ok(());
        }
        crate::notifications::job_finished(&mut tx, job.id, self.inner.clock.now()).await?;
        if to == state::FAILED
            && row.kind == kind::PROVIDER_PROVISION
            && row.plan.get("mode").and_then(Json::as_str) == Some("renew")
            && let Some(provider) = row.provider_id
        {
            crate::notifications::provider_renewal_failed(
                &mut tx,
                provider,
                row.id,
                &failure.message,
                self.inner.clock.now(),
            )
            .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        // Undo may also change the durable plan before its final cleanup runs.
        let closed = job.row().await?;
        self.after_job(&closed).await;
        Ok(())
    }

    /// What follows a job that ended: the threads that waited for its environment look again. A job
    /// about the files of a thread changes nothing they wait for.
    fn after_job<'a>(
        &'a self,
        row: &'a JobRow,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        // Success and undo both reach these hooks after their terminal commit.
        // Keep the resource-specific completion states out of each driving branch.
        Box::pin(self.after_job_inner(row))
    }

    async fn after_job_inner(&self, row: &JobRow) {
        self.after_volume_carry(row).await;
        self.after_mount_copies(row).await;
        if row.plan.get("recovery").is_some() {
            self.after_recreate(row).await;
            return;
        }
        // A tool-server job refreshes its own routes; it never wakes or releases another turn.
        if row.kind == crate::jobs::mcp_restart::KIND {
            return;
        }
        if matches!(
            row.kind.as_str(),
            rows::retained::RESTORE | rows::retained::PURGE | kind::ENVIRONMENT_REMOVE
        ) {
            if let Err(error) = self.tell_retained_failure(row).await {
                tracing::warn!(%error,"the current home admins could not be told of unresolved retained files");
            }
            if row.kind == kind::ENVIRONMENT_REMOVE
                && let Err(error) = self.purge_due_kept().await
            {
                tracing::warn!(%error,"the files whose retention has ended could not be scheduled for purge");
            }
            if row.kind != kind::ENVIRONMENT_REMOVE {
                return;
            }
        }
        // Runtime moves release the files pinned for their copy and all affected threads.
        if row.kind == kind::ENVIRONMENT_MOVE {
            self.after_move(row).await;
            return;
        }
        // Runtime updates release every thread they held after the durable job closes.
        if row.kind == kind::ENVIRONMENT_RESTART {
            self.after_restart(row).await;
            return;
        }
        if matches!(
            row.kind.as_str(),
            kind::ENVIRONMENT_RECREATE | kind::PROVIDER_RECREATE
        ) {
            self.after_recreate(row).await;
            return;
        }
        if kind::WORKSPACE.contains(&row.kind.as_str()) {
            if row.kind == kind::WORKSPACE_RESTORE {
                self.after_file_restore(row).await;
            }
            return;
        }
        // The part that forks and switches: the threads it held are let go.
        if row.kind == crate::jobs::fork::KIND {
            self.after_fork_job(row).await;
            return;
        }
        if row.kind == crate::jobs::continuation::KIND {
            if let Some(thread) = row.node_id
                && let Err(error) = self.release_lock(thread).await
            {
                tracing::warn!(%error,"A continuation's released queue awaits ordinary reconciliation.");
            }
            return;
        }
        // The jobs of a provider's runtime (the part that keeps providers): the sign-in that waited for
        // the unit is given to its harness, and the threads that waited for the provider look again.
        if let Some(provider) = row.provider_id {
            self.after_provider_job(row, provider).await;
            return;
        }
        // The export of a place: the person who asked is told, with the link to the archive.
        if row.kind == kind::EXPORT {
            self.after_export_job(row).await;
            return;
        }
        if let Some(environment) = row.environment_id
            && let Err(error) = self.reevaluate_environment(environment).await
        {
            tracing::warn!(%error, "the threads of an environment could not be woken");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_database_diagnostics_distinguish_a_known_missing_column_without_exposing_error_text() {
        let facts = persistence_facts(&orch_db::DbError::Other(sqlx::Error::ColumnNotFound(
            "continuation_id".into(),
        )));
        assert_eq!(
            facts,
            PersistenceFacts {
                category: "client_column_not_found",
                sqlstate: None,
                constraint: None,
                column: Some("continuation_id")
            }
        );
        assert_eq!(persistence_column("\"seq_updated\""), Some("seq_updated"));
        assert_eq!(persistence_column("synthetic_private_identifier"), None);
    }

    #[test]
    fn job_database_diagnostics_keep_values_and_arbitrary_nested_errors_out_of_safe_fields() {
        let error = orch_db::DbError::Check {
            table: "synthetic private row".into(),
            constraint: "untrusted\nsynthetic SQL".into(),
        };
        assert_eq!(
            persistence_facts(&error),
            PersistenceFacts {
                category: "database",
                sqlstate: Some("23514".into()),
                constraint: None,
                column: None
            }
        );
        assert_eq!(
            persistence_facts(&orch_db::DbError::Other(sqlx::Error::Protocol(
                "synthetic private body".into()
            ))),
            PersistenceFacts {
                category: "client_protocol",
                sqlstate: None,
                constraint: None,
                column: None
            }
        );
        assert_eq!(schema_identity(&"x".repeat(64)), None);
    }

    #[test]
    fn a_storage_failure_keeps_its_constraint_out_of_the_persons_job_and_inbox() {
        for stop in [
            Stop::from(orch_db::DbError::Reference {
                constraint: "model_inputs_segment_id_fkey".to_owned(),
            }),
            Stop::from(EngineError::Db(orch_db::DbError::Reference {
                constraint: "model_inputs_segment_id_fkey".to_owned(),
            })),
        ] {
            let Stop::Failed(failure) = stop else {
                panic!("a failed job")
            };
            assert_eq!(failure.code, INTERNAL);
            assert_eq!(failure.message, "The change could not be saved. Try again.");
            assert!(!failure.message.contains("fkey"));
        }
    }

    fn phases() -> Vec<Phase> {
        vec![
            Phase {
                name: "schedule",
                weight: 2,
            },
            Phase {
                name: "image",
                weight: 40,
            },
            Phase {
                name: "create",
                weight: 16,
            },
        ]
    }

    #[test]
    fn the_weights_of_the_phases_a_job_has_add_up_to_one_hundred() {
        for weights in [
            vec![1, 1, 1],
            vec![2, 40, 16],
            vec![7],
            vec![3, 3, 3, 3, 3, 3, 3],
        ] {
            assert_eq!(scaled(&weights).iter().sum::<u32>(), 100, "{weights:?}");
        }
        assert!(scaled(&[]).is_empty());
    }

    #[test]
    fn the_percentage_is_what_is_done_plus_the_running_phase_by_how_far_it_is_and_never_goes_down()
    {
        let mut progress = initial_progress(&phases());
        assert_eq!(percent_of(&progress, 0.0), 0);
        progress.phases[0].state = PhaseState::Done;
        progress.phases[1].state = PhaseState::Running;
        let image = progress.phases[1].weight;
        let before = percent_of(&progress, 0.5);
        assert_eq!(before, progress.phases[0].weight + image / 2);
        progress.percent = before;
        assert_eq!(
            percent_of(&progress, 0.1),
            before,
            "a phase that starts over does not take the bar back"
        );
        progress.phases[1].state = PhaseState::Done;
        progress.phases[2].state = PhaseState::Done;
        assert_eq!(percent_of(&progress, 0.0), 100);
    }
}
