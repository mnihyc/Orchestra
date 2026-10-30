//! Jobs with their phases and locks, and confirmed preflight plans.
#![allow(
    clippy::items_after_test_module,
    reason = "Query modules are grouped separately from local unit tests"
)]
//!
//! A job's row is the truth about how far it is: the phases with their weights and states, the
//! percentage, what it made and what became of it. Whoever runs a job holds a lease on it, a
//! member of its `plan` that names the runner and the moment the lease lapses, so that a process
//! that died in the middle is taken over by another once the lease has run out, and two processes
//! never run one job. A request to cancel is a member of the plan as well, written by whoever
//! makes it and read by the runner between its steps and while it waits.
//!
//! Every change of a job that is about an environment is stamped on the feed of that environment
//! (`environments.env_seq`) and announced when the transaction commits; one that is about a thread
//! (a snapshot of its directory, files on their way into it) is stamped on the feed of the thread's
//! topic as well. Its thread's own row is stamped and announced too: the job's hold and progress
//! are part of that view even when no message, turn or binding changed in this transaction.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json, Seq};

/// The words of `jobs.kind` that the environments part makes and runs, and those of the workspace.
pub mod kind {
    /// A thread's directory being taken.
    pub const SNAPSHOT: &str = "snapshot";
    /// Files on their way into an agent's workspace.
    pub const DELIVERY: &str = "delivery";
    /// A frozen source checkout or an own worktree.
    pub const WORKSPACE_PREPARE: &str = "workspace_prepare";
    /// Selected paths committed under one reviewed status.
    pub const GIT_COMMIT: &str = "git_commit";
    /// An explicit fast-forward push.
    pub const GIT_PUSH: &str = "git_push";
    /// Whole recorded-file restoration with complete Undo.
    pub const WORKSPACE_RESTORE: &str = "workspace_restore";
    /// File plans/results require the owning environment's Files View.
    pub const SOURCE: [&str; 4] = [WORKSPACE_PREPARE, GIT_COMMIT, GIT_PUSH, WORKSPACE_RESTORE];
    /// An archive of what a person may read in a place being written (the export of a place).
    pub const EXPORT: &str = "export";
    /// What a person brings into a place from a package or from plain files being made, or being taken back.
    pub const IMPORT: &str = "import";
    /// The kinds of the workspace: jobs about a thread's files, read and stopped by those who may
    /// read and write them.
    pub const WORKSPACE: [&str; 6] = [
        SNAPSHOT,
        DELIVERY,
        WORKSPACE_PREPARE,
        GIT_COMMIT,
        GIT_PUSH,
        WORKSPACE_RESTORE,
    ];
    /// An environment being made, or started.
    pub const ENVIRONMENT_PROVISION: &str = "environment_provision";
    /// An environment or a retired unit being removed.
    pub const ENVIRONMENT_REMOVE: &str = "environment_remove";
    /// A provider's runtime being made, started, or started to renew the sign-in it holds.
    pub const PROVIDER_PROVISION: &str = "provider_provision";
    /// A provider's runtime given a new unit, or updated.
    pub const PROVIDER_RECREATE: &str = "provider_recreate";
    /// A new unit for an environment, or an update.
    pub const ENVIRONMENT_RECREATE: &str = "environment_recreate";
    /// Stops and starts the same managed unit, preserving its files and queued inputs.
    pub const ENVIRONMENT_RESTART: &str = "environment_restart";
    /// An environment taken to another host.
    pub const ENVIRONMENT_MOVE: &str = "environment_move";
    /// The kinds of which at most one runs for an environment at a time.
    pub const ONE_AT_A_TIME: [&str; 10] = [
        "capability_change",
        ENVIRONMENT_PROVISION,
        ENVIRONMENT_RECREATE,
        ENVIRONMENT_RESTART,
        ENVIRONMENT_MOVE,
        ENVIRONMENT_REMOVE,
        WORKSPACE_PREPARE,
        GIT_COMMIT,
        GIT_PUSH,
        WORKSPACE_RESTORE,
    ];
}

/// The words of `jobs.state`.
pub mod state {
    /// Made and not begun.
    pub const QUEUED: &str = "queued";
    /// Under way.
    pub const RUNNING: &str = "running";
    /// Done.
    pub const SUCCEEDED: &str = "succeeded";
    /// Failed, with what it undid.
    pub const FAILED: &str = "failed";
    /// Cancelled, with what it undid.
    pub const CANCELLED: &str = "cancelled";
    /// Its required machine was permanently lost.
    pub const LOST: &str = "lost";
    /// The states of a job that is not over.
    pub const OPEN: [&str; 2] = [QUEUED, RUNNING];
}

/// The channel on which a change of an environment's feed is announced; its payload is
/// `<environment id>:<sequence>`.
pub const ENVIRONMENT_CHANNEL: &str = "orch_env";

/// A job, as stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct JobRow {
    /// Its identifier.
    pub id: Id,
    /// What it does.
    pub kind: String,
    /// The thread it is about.
    pub node_id: Option<Id>,
    /// The environment it is about.
    pub environment_id: Option<Id>,
    /// The provider whose runtime it is about.
    pub provider_id: Option<Id>,
    /// The batch it belongs to.
    pub batch_id: Option<Id>,
    /// Where it stands.
    pub state: String,
    /// What it was asked to do, and what its runner keeps in it: the lease and a request to cancel.
    pub plan: Json,
    /// How far it has come.
    pub progress: Json,
    /// What it produced.
    pub result: Option<Json>,
    /// Why it failed.
    pub error: Option<Json>,
    /// Who made it.
    pub created_by: Option<Id>,
    /// Its number on the feed of its thread's topic.
    pub topic_seq: Option<Seq>,
    /// Its number on the feed of its environment.
    pub env_seq: Option<Seq>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it began.
    pub started_at: Option<DateTime<Utc>>,
    /// When it ended.
    pub finished_at: Option<DateTime<Utc>>,
    /// When it last changed.
    pub updated_at: DateTime<Utc>,
}

impl JobRow {
    /// Whether it is not over.
    #[must_use]
    pub fn is_open(&self) -> bool {
        state::OPEN.contains(&self.state.as_str())
    }

    /// Whether a request to cancel it was made.
    #[must_use]
    pub fn cancel_requested(&self) -> bool {
        self.plan.get("cancel").is_some()
    }
}

const COLUMNS: &str = "id, kind, node_id, environment_id, provider_id, batch_id, state, plan, progress, result, error,
    created_by, topic_seq, env_seq, created_at, started_at, finished_at, updated_at";

/// A job to make.
#[derive(Debug, Clone, PartialEq)]
pub struct NewJob<'a> {
    /// Its identifier.
    pub id: Id,
    /// What it does.
    pub kind: &'a str,
    /// The thread it is about.
    pub node_id: Option<Id>,
    /// The environment it works on.
    pub environment_id: Option<Id>,
    /// The provider whose runtime it works on.
    pub provider_id: Option<Id>,
    /// What it is asked to do.
    pub plan: &'a Json,
    /// How far it is before it begins: its phases, all pending.
    pub progress: &'a Json,
    /// Who made it.
    pub created_by: Option<Id>,
}

/// Stamps the feed of an environment with its next number and announces it when the transaction
/// commits. None where there is no such environment. The environment's row is locked from now
/// until the transaction ends, which is what keeps the numbers in the order of the commits.
pub async fn stamp_environment(
    conn: &mut Conn,
    environment: Id,
    at: DateTime<Utc>,
) -> Result<Option<Seq>, DbError> {
    let seq: Option<Seq> = sqlx::query_scalar(
        "UPDATE environments SET env_seq = env_seq + 1, updated_at = GREATEST(updated_at, $2)
          WHERE id = $1 RETURNING env_seq",
    )
    .bind(environment)
    .bind(at)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(seq) = seq {
        sqlx::query("SELECT pg_notify($1, $2)")
            .bind(ENVIRONMENT_CHANNEL)
            .bind(format!("{environment}:{seq}"))
            .execute(&mut *conn)
            .await?;
    }
    Ok(seq)
}

/// Publishes the thread's changed job view and stamps its topic, in that lock order.
async fn stamp_topic_of(conn: &mut Conn, thread: Id) -> Result<Option<Seq>, DbError> {
    if let Some(seq) = crate::sequences::next(&mut *conn, thread).await? {
        crate::queries::threads::set_seq_state(&mut *conn, thread, seq).await?;
        crate::sequences::notify(&mut *conn, thread, seq).await?;
    }
    let topic: Option<Id> = sqlx::query_scalar("SELECT topic_id FROM threads WHERE node_id = $1")
        .bind(thread)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(topic) = topic else {
        return Ok(None);
    };
    crate::queries::hierarchy::stamp(conn, topic).await?;
    Ok(
        sqlx::query_scalar("SELECT topic_seq FROM topics WHERE node_id = $1")
            .bind(topic)
            .fetch_optional(conn)
            .await?,
    )
}

/// Makes a job, queued, published on its thread, topic and environment feeds.
pub async fn insert(
    conn: &mut Conn,
    job: &NewJob<'_>,
    at: DateTime<Utc>,
) -> Result<JobRow, DbError> {
    let topic_seq = match job.node_id {
        Some(thread) => stamp_topic_of(conn, thread).await?,
        None => None,
    };
    let env_seq = match job.environment_id {
        Some(environment) => stamp_environment(conn, environment, at).await?,
        None => None,
    };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO jobs (id, kind, node_id, environment_id, provider_id, state, plan, progress, created_by,
                           topic_seq, env_seq, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, 'queued', $6, $7, $8, $9, $10, $11, $11)
         RETURNING {COLUMNS}"
    )))
    .bind(job.id)
    .bind(job.kind)
    .bind(job.node_id)
    .bind(job.environment_id)
    .bind(job.provider_id)
    .bind(job.plan)
    .bind(job.progress)
    .bind(job.created_by)
    .bind(topic_seq)
    .bind(env_seq)
    .bind(at)
    .fetch_one(conn)
    .await?)
}

/// The job `id`.
pub async fn job(conn: &mut Conn, id: Id) -> Result<Option<JobRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM jobs WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The job `id`, locked until the transaction ends.
pub async fn lock(conn: &mut Conn, id: Id) -> Result<Option<JobRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM jobs WHERE id = $1 FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The jobs of the kinds that run one at a time that are not over, for an environment: at most one
/// is, and a request for another is refused naming it.
pub async fn open_of_environment(conn: &mut Conn, environment: Id) -> Result<Vec<JobRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM jobs
          WHERE environment_id = $1 AND kind = ANY($2) AND state IN ('queued', 'running')
          ORDER BY created_at, id"
    )))
    .bind(environment)
    .bind(
        kind::ONE_AT_A_TIME
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>(),
    )
    .fetch_all(conn)
    .await?)
}

/// The jobs about an environment: those that are not over, and the last `recent` that are, newest
/// first.
pub async fn of_environment(
    conn: &mut Conn,
    environment: Id,
    recent: i64,
) -> Result<Vec<JobRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "(SELECT {COLUMNS} FROM jobs WHERE environment_id = $1 AND state IN ('queued', 'running'))
         UNION ALL
         (SELECT {COLUMNS} FROM jobs WHERE environment_id = $1 AND state NOT IN ('queued', 'running')
           ORDER BY created_at DESC, id DESC LIMIT $2)
         ORDER BY created_at DESC, id DESC"
    )))
    .bind(environment)
    .bind(recent)
    .fetch_all(conn)
    .await?)
}

/// The latest job of the given kinds about an environment, over or not.
pub async fn latest_of_environment(
    conn: &mut Conn,
    environment: Id,
    kinds: &[&str],
) -> Result<Option<JobRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM jobs WHERE environment_id = $1 AND kind = ANY($2)
          ORDER BY created_at DESC, id DESC LIMIT 1"
    )))
    .bind(environment)
    .bind(
        kinds
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>(),
    )
    .fetch_optional(conn)
    .await?)
}

/// The identifiers of jobs of the given kinds that are not over and whose lease has run out or was
/// never taken: what a process looks for to run, or to take over.
pub async fn runnable(
    conn: &mut Conn,
    kinds: &[&str],
    now: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM jobs
          WHERE kind = ANY($1) AND state IN ('queued', 'running')
            AND NOT (kind='environment_provision' AND plan ? 'recovery')
            AND (plan -> 'runner' IS NULL OR (plan -> 'runner' ->> 'until')::timestamptz <= $2)
          ORDER BY created_at, id LIMIT $3",
    )
    .bind(
        kinds
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>(),
    )
    .bind(now)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Takes the lease on a job for `runner` until `until`, or renews one that `runner` holds. Returns
/// whether it is held now: a job that is over, or whose lease is another's and has not run out, is
/// not. A job that was queued begins to run.
pub async fn claim(
    conn: &mut Conn,
    id: Id,
    runner: Id,
    until: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    let taken = sqlx::query(
        "UPDATE jobs
            SET plan = jsonb_set(plan, '{runner}',
                                 jsonb_build_object('id', $2::text, 'until', to_jsonb($3::timestamptz))),
                state = 'running', started_at = COALESCE(started_at, $4), updated_at = $4,
                error = NULL, finished_at = NULL
          WHERE id = $1 AND (state IN ('queued', 'running') OR
                (kind = 'workspace_restore' AND state = 'failed' AND plan ->> 'restore_recovery_required' = 'true'))
            AND (plan -> 'runner' IS NULL
                 OR plan -> 'runner' ->> 'id' = $2::text
                 OR (plan -> 'runner' ->> 'until')::timestamptz <= $4)",
    )
    .bind(id)
    .bind(runner)
    .bind(until)
    .bind(now)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(taken > 0)
}

/// Gives up the lease that `runner` holds on a job, so that another may take it at once.
pub async fn release(
    conn: &mut Conn,
    id: Id,
    runner: Id,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE jobs SET plan = jsonb_set(plan, '{runner,until}', to_jsonb($3::timestamptz))
          WHERE id = $1 AND plan -> 'runner' ->> 'id' = $2::text",
    )
    .bind(id)
    .bind(runner)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}

/// Adds members to the plan of a job: what its runner decided and must not decide again.
pub async fn merge_plan(
    conn: &mut Conn,
    id: Id,
    members: &Json,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE jobs SET plan = plan || $2, updated_at = $3 WHERE id = $1")
        .bind(id)
        .bind(members)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// The accepted initial-file request of a first provisioning that never published an environment.
/// A retry carries the source choice, not old native receipts, completed flags or temporary pins.
pub async fn initial_files_request(
    conn: &mut Conn,
    environment: Id,
) -> Result<Option<Json>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT plan->'initial_files' FROM jobs WHERE environment_id=$1
        AND kind='environment_provision' AND plan ? 'initial_files'
        ORDER BY created_at DESC,id DESC LIMIT 1",
    )
    .bind(environment)
    .fetch_optional(conn)
    .await?)
}

/// Writes how far a job has come, published on its thread, topic and environment feeds. A job that is over is
/// left as it is.
pub async fn save_progress(
    conn: &mut Conn,
    id: Id,
    progress: &Json,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let about: Option<(Option<Id>, Option<Id>)> = sqlx::query_as(
        "UPDATE jobs SET progress = $2, updated_at = $3
          WHERE id = $1 AND state IN ('queued', 'running') RETURNING environment_id, node_id",
    )
    .bind(id)
    .bind(progress)
    .bind(at)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((environment, thread)) = about else {
        return Ok(false);
    };
    stamp_job(conn, id, environment, thread, at).await?;
    Ok(true)
}

/// Publishes a job's thread view and stamps its topic and environment feeds.
async fn stamp_job(
    conn: &mut Conn,
    id: Id,
    environment: Option<Id>,
    thread: Option<Id>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let topic_seq = match thread {
        Some(thread) => stamp_topic_of(conn, thread).await?,
        None => None,
    };
    // Match insertion's thread/topic/environment order rather than taking a thread after its environment.
    let env_seq = match environment {
        Some(environment) => stamp_environment(conn, environment, at).await?,
        None => None,
    };
    if env_seq.is_some() || topic_seq.is_some() {
        sqlx::query(
            "UPDATE jobs SET env_seq = COALESCE($2, env_seq), topic_seq = COALESCE($3, topic_seq) WHERE id = $1",
        )
        .bind(id)
        .bind(env_seq)
        .bind(topic_seq)
        .execute(conn)
        .await?;
    }
    Ok(())
}

/// Ends a job: its state, what it produced or why it failed. Returns whether it ended now; a job
/// that is over is not ended again.
pub async fn finish(
    conn: &mut Conn,
    id: Id,
    to: &str,
    progress: &Json,
    result: Option<&Json>,
    error: Option<&Json>,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let ended: Option<(Option<Id>, Option<Id>)> = sqlx::query_as(
        "UPDATE jobs SET state = $2, progress = $3, result = $4, error = $5, finished_at = $6, updated_at = $6,
                         started_at = COALESCE(started_at, $6)
          WHERE id = $1 AND state IN ('queued', 'running') RETURNING environment_id, node_id",
    )
    .bind(id)
    .bind(to)
    .bind(progress)
    .bind(result)
    .bind(error)
    .bind(at)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((environment, thread)) = ended else {
        return Ok(false);
    };
    stamp_job(conn, id, environment, thread, at).await?;
    Ok(true)
}

/// Writes down that `by` asked for the job to be cancelled. Returns whether the request stands now:
/// the job is not over, has not passed the point after which nothing is undone, and was not asked
/// already (a second request changes nothing and is still a yes).
pub async fn request_cancel(
    conn: &mut Conn,
    id: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    request_cancel_by(conn, id, Some(by), at).await
}

/// Requests the same durable cancellation on behalf of a person or a configured system schedule.
pub async fn request_cancel_by(
    conn: &mut Conn,
    id: Id,
    by: Option<Id>,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    // One statement also works for callers without an outer transaction: admission locks the job
    // first, and a cancellation cannot leave a newly admitted command eligible for delivery.
    Ok(sqlx::query_scalar(
        "WITH asked AS (UPDATE jobs
            SET plan = CASE WHEN plan ? 'cancel' THEN plan
                            ELSE jsonb_set(plan, '{cancel}', jsonb_build_object('by', $2::text, 'at', to_jsonb($3::timestamptz))) END,
                progress = jsonb_set(progress, '{activity}', COALESCE(progress->'activity','{}'::jsonb) ||
                    jsonb_build_object('updated_at',to_jsonb($3::timestamptz),'stopping',true,
                        'stall_after_seconds',0,'directories_done',progress->'activity'->'directories_done')),
                updated_at = $3
          WHERE id = $1 AND state IN ('queued', 'running') AND NOT (plan ? 'committed') RETURNING id,plan),
          stopped AS (UPDATE commands c SET state='cancelled',result=NULL,error=NULL,updated_at=$3
            WHERE c.state IN ('queued','sent','accepted') AND EXISTS(SELECT 1 FROM asked j WHERE
                c.body->'work'->>'job_id'=j.id::text OR j.plan->'active_command'->>'id'=c.id::text)
            RETURNING c.id)
          SELECT EXISTS(SELECT 1 FROM asked)",
    )
    .bind(id)
    .bind(by)
    .bind(at)
    .fetch_one(conn)
    .await?)
}

/// Passes the point after which a job is not cancelled and not undone. Returns whether it was
/// passed: a request to cancel that was made before stands, and the job is cancelled instead.
pub async fn pass_commit(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
    let passed = sqlx::query(
        "UPDATE jobs SET plan = plan || '{\"committed\": true}'::jsonb, updated_at = $2
          WHERE id = $1 AND state IN ('queued', 'running') AND NOT (plan ? 'cancel')",
    )
    .bind(id)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(passed > 0)
}

/// Whether a request to cancel the job stands.
pub async fn cancel_requested(conn: &mut Conn, id: Id) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT COALESCE(plan ? 'cancel', false) FROM jobs WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?
            .unwrap_or(false),
    )
}

/// The jobs of the given kinds that are not over, for the environments `environments`.
pub async fn open_for(
    conn: &mut Conn,
    environments: &[Id],
    kinds: &[&str],
) -> Result<Vec<JobRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM jobs
          WHERE environment_id = ANY($1) AND kind = ANY($2) AND state IN ('queued', 'running')
          ORDER BY created_at, id"
    )))
    .bind(environments)
    .bind(
        kinds
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>(),
    )
    .fetch_all(conn)
    .await?)
}

// ---- Management over HTTP: what runs now ----

/// How many jobs are running.
pub async fn running_jobs(conn: &mut Conn) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE state = 'running'")
            .fetch_one(conn)
            .await?,
    )
}

// ---- Fork and switch: the jobs that make one thread of another, and the plans that confirm them ----
//
// Everything below belongs to the part that forks and switches threads. A fork or a switch is a job of the
// kind `thread_fork`; the thread it holds still is whatever an open job of that kind names, so that taking
// the lock is making the job, in one transaction, and releasing it is the job ending.

/// The jobs of forks and switches, the plans that confirm them, and the locks they hold.
pub mod forks {
    use super::{COLUMNS, JobRow, stamp_job, stamp_topic_of};
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    /// The word of `jobs.kind` for a fork or a switch.
    pub const THREAD_FORK: &str = "thread_fork";
    /// The word of `preflights.kind` for the plan of one thread.
    pub const PLAN_KIND: &str = "thread_fork";
    /// The word of `preflights.kind` for the plan of several threads.
    pub const BATCH_PLAN_KIND: &str = "fork_batch";

    /// A fork or a switch to make.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewForkJob<'a> {
        /// Its identifier.
        pub id: Id,
        /// The thread it is asked of: the one that is switched, or the one a fork is taken from.
        pub thread: Id,
        /// The environment it works on: the one the result lands in, when that exists already.
        pub environment: Option<Id>,
        /// The batch it belongs to.
        pub batch: Option<Id>,
        /// What it is asked to do.
        pub plan: &'a Json,
        /// Its phases, all pending.
        pub progress: &'a Json,
        /// Who confirmed it.
        pub created_by: Id,
    }

    /// Makes the job of a fork or a switch, queued, stamped on the feeds it belongs to.
    pub async fn insert(
        conn: &mut Conn,
        job: &NewForkJob<'_>,
        at: DateTime<Utc>,
    ) -> Result<JobRow, DbError> {
        let topic_seq = stamp_topic_of(conn, job.thread).await?;
        let env_seq = match job.environment {
            Some(environment) => super::stamp_environment(conn, environment, at).await?,
            None => None,
        };
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "INSERT INTO jobs (id, kind, node_id, environment_id, batch_id, state, plan, progress, created_by,
                               topic_seq, env_seq, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, 'queued', $6, $7, $8, $9, $10, $11, $11)
             RETURNING {COLUMNS}"
        )))
        .bind(job.id)
        .bind(THREAD_FORK)
        .bind(job.thread)
        .bind(job.environment)
        .bind(job.batch)
        .bind(job.plan)
        .bind(job.progress)
        .bind(job.created_by)
        .bind(topic_seq)
        .bind(env_seq)
        .bind(at)
        .fetch_one(conn)
        .await?)
    }

    /// The open job holding a thread while its binding or runtime changes, or while it is made.
    pub async fn lock_of(conn: &mut Conn, thread: Id) -> Result<Option<JobRow>, DbError> {
        super::changes::thread_hold(conn, thread).await
    }

    /// The open jobs of forks and switches that touch a thread in any way: it is switched, forked, or the
    /// result of a fork. A fork of a thread touches it without holding it.
    pub async fn open_touching_thread(conn: &mut Conn, thread: Id) -> Result<Vec<JobRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM jobs
              WHERE kind = $2 AND state IN ('queued', 'running')
                AND (node_id = $1 OR plan ->> 'new_thread' = $1::text)
              ORDER BY created_at, id"
        )))
        .bind(thread)
        .bind(THREAD_FORK)
        .fetch_all(conn)
        .await?)
    }

    /// The open jobs of forks and switches that work on an environment: it is the one they land in, or the
    /// one a thread leaves.
    pub async fn open_touching_environment(
        conn: &mut Conn,
        environment: Id,
    ) -> Result<Vec<JobRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM jobs
              WHERE kind = $2 AND state IN ('queued', 'running')
                AND (environment_id = $1
                  OR plan ->> 'from_environment' = $1::text
                  OR plan ->> 'to_environment' = $1::text
                  OR plan ->> 'new_environment' = $1::text)
              ORDER BY created_at, id"
        )))
        .bind(environment)
        .bind(THREAD_FORK)
        .fetch_all(conn)
        .await?)
    }

    /// The jobs of a batch, oldest first.
    pub async fn of_batch(conn: &mut Conn, batch: Id) -> Result<Vec<JobRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM jobs WHERE batch_id = $1 ORDER BY created_at, id"
        )))
        .bind(batch)
        .fetch_all(conn)
        .await?)
    }

    /// The jobs of forks and switches that a process may take up now: queued or running, with no runner or
    /// one whose lease has lapsed. A job of a batch that is queued waits while `parallel` jobs of its batch
    /// are running; one that was running already is always taken over.
    pub async fn runnable(
        conn: &mut Conn,
        now: DateTime<Utc>,
        parallel: i64,
        limit: i64,
    ) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT j.id FROM jobs j
              WHERE j.kind = $1 AND j.state IN ('queued', 'running')
                AND (j.plan -> 'runner' IS NULL OR (j.plan -> 'runner' ->> 'until')::timestamptz <= $2)
                AND (j.state = 'running' OR j.batch_id IS NULL
                     OR (SELECT count(*) FROM jobs b WHERE b.batch_id = j.batch_id AND b.state = 'running') < $3)
              ORDER BY j.created_at, j.id LIMIT $4",
        )
        .bind(THREAD_FORK)
        .bind(now)
        .bind(parallel)
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// Takes the lease on a job of a batch for `runner`, as [`super::claim`] does, but only if fewer than
    /// `parallel` jobs of its batch are running (a job already running is always claimed again). Two
    /// processes that look at once are made to take turns by a lock held for the length of the transaction.
    pub async fn claim_in_batch(
        conn: &mut Conn,
        id: Id,
        runner: Id,
        until: DateTime<Utc>,
        now: DateTime<Utc>,
        parallel: i64,
    ) -> Result<bool, DbError> {
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended(coalesce(batch_id::text, id::text), 0))
               FROM jobs WHERE id = $1",
        )
        .bind(id)
        .execute(&mut *conn)
        .await?;
        let taken = sqlx::query(
            "UPDATE jobs
                SET plan = jsonb_set(plan, '{runner}',
                                     jsonb_build_object('id', $2::text, 'until', to_jsonb($3::timestamptz))),
                    state = 'running', started_at = COALESCE(started_at, $4), updated_at = $4
              WHERE id = $1 AND state IN ('queued', 'running')
                AND (plan -> 'runner' IS NULL
                     OR plan -> 'runner' ->> 'id' = $2::text
                     OR (plan -> 'runner' ->> 'until')::timestamptz <= $4)
                AND (state = 'running' OR batch_id IS NULL
                     OR (SELECT count(*) FROM jobs b WHERE b.batch_id = jobs.batch_id AND b.state = 'running') < $5)
                AND (state='running' OR plan->>'batch_environment_job' IS NULL OR plan->>'batch_environment_job'=id::text
                     OR EXISTS (SELECT 1 FROM jobs maker LEFT JOIN environments e ON e.id=(maker.plan->>'new_environment')::uuid
                                WHERE maker.id=(jobs.plan->>'batch_environment_job')::uuid
                                  AND (e.status='running' OR maker.state NOT IN ('queued','running'))))",
        )
        .bind(id)
        .bind(runner)
        .bind(until)
        .bind(now)
        .bind(parallel)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(taken > 0)
    }

    /// Stamps the feeds of a job that changed without a progress report (a lock taken or released).
    pub async fn stamp(
        conn: &mut Conn,
        job: Id,
        environment: Option<Id>,
        thread: Option<Id>,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        stamp_job(conn, job, environment, thread, at).await
    }

    /// A plan that was answered and waits to be confirmed.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct PreflightRow {
        /// Its identifier: what a request to carry it out names.
        pub id: Id,
        /// What it plans: [`PLAN_KIND`] or [`BATCH_PLAN_KIND`].
        pub kind: String,
        /// The thread, for the plan of one.
        pub node_id: Option<Id>,
        /// The environment, for a plan about one alone.
        pub environment_id: Option<Id>,
        /// Who asked for it.
        pub created_by: Id,
        /// The request, the plan as it was answered, and what carrying it out needs.
        pub plan: Json,
        /// When it goes out of date.
        pub expires_at: DateTime<Utc>,
        /// When it was made.
        pub created_at: DateTime<Utc>,
    }

    /// Keeps a plan for the minutes it is good for.
    pub async fn save_preflight(conn: &mut Conn, row: &PreflightRow) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO preflights (id, kind, node_id, environment_id, created_by, plan, expires_at, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(row.id)
        .bind(&row.kind)
        .bind(row.node_id)
        .bind(row.environment_id)
        .bind(row.created_by)
        .bind(&row.plan)
        .bind(row.expires_at)
        .bind(row.created_at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// A plan, if it is there.
    pub async fn preflight(conn: &mut Conn, id: Id) -> Result<Option<PreflightRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, kind, node_id, environment_id, created_by, plan, expires_at, created_at
               FROM preflights WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(conn)
        .await?)
    }

    /// Forgets a plan that was carried out: it is good for one confirmation.
    pub async fn forget_preflight(conn: &mut Conn, id: Id) -> Result<(), DbError> {
        sqlx::query("DELETE FROM preflights WHERE id = $1")
            .bind(id)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Forgets the plans that went out of date before `before`; returns how many.
    pub async fn forget_old_preflights(
        conn: &mut Conn,
        before: DateTime<Utc>,
    ) -> Result<u64, DbError> {
        Ok(sqlx::query("DELETE FROM preflights WHERE expires_at < $1")
            .bind(before)
            .execute(conn)
            .await?
            .rows_affected())
    }

    /// A whole-environment change; compatible fork batches check their thread jobs separately.
    pub async fn environment_is_being_changed(
        conn: &mut Conn,
        environment: Id,
    ) -> Result<Option<JobRow>, DbError> {
        super::changes::environment_change_hold(conn, environment).await
    }
}

// ---- Jobs about a provider's runtime ---------------------------------------------------------
//
// One block, kept together at the end of the file: the jobs that make, start, renew and remove the
// unit of a managed provider, and the phases each has. They are written here and not in the engine
// because the worker queues the renewal of a sign-in as well, and may not depend on the engine.

/// The jobs of a provider's runtime, and the phases each is carried out in.
pub mod provider_jobs {
    use super::{COLUMNS, JobRow, kind};
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_api_contract::workspace::{JobPhase, JobProgress, PhaseState};
    use orch_types::{Id, Json};

    /// A provider's runtime being made, started, or started to renew the sign-in it holds.
    pub const PROVISION: &str = kind::PROVIDER_PROVISION;
    /// A provider's runtime, or a retired unit of it, being taken away.
    pub const REMOVE: &str = kind::ENVIRONMENT_REMOVE;
    /// The kinds of which at most one is not over for a provider's runtime.
    pub const ONE_AT_A_TIME: [&str; 3] = [
        kind::PROVIDER_PROVISION,
        kind::PROVIDER_RECREATE,
        kind::ENVIRONMENT_REMOVE,
    ];

    /// The phases of making a provider's runtime, with the weights that shape its bar: the image first
    /// (a pull or a build is minutes), then the unit and the wait for its connector. A provider's
    /// runtime has no attach and no init, and only the one volume.
    pub const MAKE: [(&str, u32); 9] = [
        ("schedule", 2),
        ("image", 40),
        ("volumes", 2),
        ("tools", 8),
        ("create", 16),
        ("enroll", 12),
        ("after_up", 4),
        ("verify", 4),
        ("commit", 2),
    ];
    /// The phases of starting the unit of a provider's runtime that is stopped: the wait for the
    /// connector is the most of it.
    pub const START: [(&str, u32); 5] = [
        ("start", 20),
        ("enroll", 50),
        ("after_up", 10),
        ("verify", 10),
        ("commit", 5),
    ];
    /// The phases of starting a stopped runtime to renew its sign-in: a start, and the harness looking
    /// at what it holds.
    pub const RENEW: [(&str, u32); 6] = [
        ("start", 20),
        ("enroll", 40),
        ("after_up", 10),
        ("verify", 10),
        ("commit", 5),
        ("refresh", 25),
    ];
    /// The phases of taking a provider's runtime away.
    pub const REMOVAL: [(&str, u32); 4] =
        [("stop", 15), ("unit", 20), ("volumes", 55), ("forget", 10)];

    /// What the whole bar is: the weights of a job's phases are scaled to this.
    const WHOLE: u32 = 100;

    /// The progress of a job that has not begun: its phases in order, all pending, their weights
    /// scaled so that they add up to one hundred (what rounding leaves over goes to the heaviest).
    #[must_use]
    pub fn initial_progress(phases: &[(&str, u32)]) -> Json {
        let total: u32 = phases.iter().map(|(_, weight)| *weight).sum();
        let mut shares: Vec<u32> = phases
            .iter()
            .map(|(_, weight)| weight * WHOLE / total.max(1))
            .collect();
        let left = WHOLE.saturating_sub(shares.iter().sum());
        let heaviest = phases
            .iter()
            .enumerate()
            .max_by_key(|(index, (_, weight))| (*weight, std::cmp::Reverse(*index)))
            .map(|(index, _)| index);
        if let Some(index) = heaviest
            && let Some(share) = shares.get_mut(index)
        {
            *share += left;
        }
        let progress = JobProgress {
            activity: None,
            phase: phases
                .first()
                .map(|(name, _)| (*name).to_owned())
                .unwrap_or_default(),
            phases: phases
                .iter()
                .zip(shares)
                .map(|((name, _), weight)| JobPhase {
                    name: (*name).to_owned(),
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
        };
        serde_json::to_value(progress).unwrap_or(Json::Null)
    }

    /// The jobs of the kinds that run one at a time that are not over, for a provider's runtime: at
    /// most one is, and a request for another is refused naming it.
    pub async fn open_of_provider(conn: &mut Conn, provider: Id) -> Result<Vec<JobRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM jobs
              WHERE provider_id = $1 AND kind = ANY($2) AND state IN ('queued', 'running')
              ORDER BY created_at, id"
        )))
        .bind(provider)
        .bind(
            ONE_AT_A_TIME
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>(),
        )
        .fetch_all(conn)
        .await?)
    }

    /// The jobs about the runtimes of these providers that are not over, oldest first.
    pub async fn open_for(conn: &mut Conn, providers: &[Id]) -> Result<Vec<JobRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM jobs
              WHERE provider_id = ANY($1) AND kind = ANY($2) AND state IN ('queued', 'running')
              ORDER BY created_at, id"
        )))
        .bind(providers)
        .bind(
            ONE_AT_A_TIME
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>(),
        )
        .fetch_all(conn)
        .await?)
    }

    /// The latest job about a provider's runtime, over or not: what a page shows when none is under
    /// way, and what says why the last one stopped.
    pub async fn latest_of_provider(
        conn: &mut Conn,
        provider: Id,
    ) -> Result<Option<JobRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM jobs WHERE provider_id = $1 AND kind = ANY($2)
              ORDER BY created_at DESC, id DESC LIMIT 1"
        )))
        .bind(provider)
        .bind(
            ONE_AT_A_TIME
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>(),
        )
        .fetch_optional(conn)
        .await?)
    }

    /// The exact volume choice of this unit's latest removal, retained when the reaper retries it.
    /// Older provider-deletion jobs did not carry a choice and removed owned volumes.
    pub async fn removal_volumes(
        conn: &mut Conn,
        provider: Id,
        runtime: Id,
    ) -> Result<bool, DbError> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT COALESCE((plan ->> 'volumes')::boolean, true) FROM jobs
              WHERE provider_id = $1 AND kind = $2 AND plan ->> 'runtime' = $3
              ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(provider)
        .bind(REMOVE)
        .bind(runtime.to_string())
        .fetch_optional(conn)
        .await?
        .unwrap_or(true))
    }

    /// How many removals of a provider's unit have failed since the last that did not, for the pause
    /// before the next is tried.
    pub async fn failed_removals(
        conn: &mut Conn,
        provider: Id,
        runtime: Id,
    ) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM jobs
              WHERE provider_id = $1 AND kind = $2 AND state = 'failed' AND plan ->> 'runtime' = $3::text",
        )
        .bind(provider)
        .bind(REMOVE)
        .bind(runtime)
        .fetch_one(conn)
        .await?)
    }

    /// What queueing a job about a provider's runtime came to.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Queued {
        /// The job was made.
        Made(Box<JobRow>),
        /// One of the kinds that run one at a time is not over: this one.
        Running(Box<JobRow>),
        /// There is no such provider.
        NoProvider,
    }

    /// Queues a job about a provider's runtime in the caller's transaction, unless one of the kinds
    /// that run one at a time is not over: then that one is the answer. The provider's row is locked
    /// first, so that two requests cannot both find it free.
    pub async fn queue(
        conn: &mut Conn,
        making: &Making<'_>,
        now: DateTime<Utc>,
    ) -> Result<Queued, DbError> {
        let locked: Option<Id> =
            sqlx::query_scalar("SELECT id FROM providers WHERE id = $1 FOR UPDATE")
                .bind(making.provider)
                .fetch_optional(&mut *conn)
                .await?;
        if locked.is_none() {
            return Ok(Queued::NoProvider);
        }
        if let Some(running) = open_of_provider(conn, making.provider)
            .await?
            .into_iter()
            .next()
        {
            return Ok(Queued::Running(Box::new(running)));
        }
        let progress = initial_progress(making.phases);
        Ok(Queued::Made(Box::new(
            super::insert(
                conn,
                &super::NewJob {
                    id: orch_types::new_id(),
                    kind: making.kind,
                    node_id: None,
                    environment_id: None,
                    provider_id: Some(making.provider),
                    plan: &making.plan,
                    progress: &progress,
                    created_by: making.created_by,
                },
                now,
            )
            .await?,
        )))
    }

    /// Queues the job that starts a stopped runtime so that its harness looks at the sign-in it holds,
    /// unless a job about the provider's runtime is not over: then that one is the answer. This is the
    /// plan of the job `provider_provision` in the mode `renew`, for the schedule that has no engine.
    pub async fn queue_renewal(
        conn: &mut Conn,
        provider: Id,
        now: DateTime<Utc>,
    ) -> Result<Queued, DbError> {
        let unit =
            crate::queries::resources::provider_runtime::keeper_has_unit(conn, provider).await?;
        queue(
            conn,
            &Making {
                kind: PROVISION,
                provider,
                plan: serde_json::json!({ "mode": "renew", "by": null }),
                phases: if unit { &RENEW } else { &[("refresh", 100)] },
                created_by: None,
            },
            now,
        )
        .await
    }

    /// A job about a provider's runtime to make.
    #[derive(Debug, Clone)]
    pub struct Making<'a> {
        /// What it does.
        pub kind: &'static str,
        /// The provider whose runtime it works on.
        pub provider: Id,
        /// What it is asked to do.
        pub plan: Json,
        /// Its phases, with their weights.
        pub phases: &'a [(&'static str, u32)],
        /// Who made it.
        pub created_by: Option<Id>,
    }
}

/// What the state behind the feed of an environment and the feed of a topic reads of the jobs.
pub mod feeds {
    use super::{COLUMNS, JobRow, kind};
    use crate::{Conn, DbError};
    use orch_types::{Id, Seq};

    fn kinds() -> Vec<String> {
        kind::ONE_AT_A_TIME
            .iter()
            .map(|word| (*word).to_owned())
            .collect()
    }

    /// The jobs about an environment that make it or change it (a provision, a recreate or an
    /// update, a move, a removal) and that are not over: the first load of its feed.
    pub async fn open_of_environment(
        conn: &mut Conn,
        environment: Id,
    ) -> Result<Vec<JobRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM jobs
              WHERE environment_id = $1 AND kind = ANY($2) AND state IN ('queued', 'running')
              ORDER BY env_seq, id"
        )))
        .bind(environment)
        .bind(kinds())
        .fetch_all(conn)
        .await?)
    }

    /// The jobs of those kinds about an environment that were stamped on its feed in `(after, upto]`,
    /// over or not, in the order of their numbers, at most `limit`.
    pub async fn of_environment_between(
        conn: &mut Conn,
        environment: Id,
        after: Seq,
        upto: Seq,
        limit: i64,
    ) -> Result<Vec<JobRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM jobs
              WHERE environment_id = $1 AND kind = ANY($2) AND env_seq > $3 AND env_seq <= $4
              ORDER BY env_seq, id LIMIT $5"
        )))
        .bind(environment)
        .bind(kinds())
        .bind(after)
        .bind(upto)
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// The jobs about the threads of a topic that are not over: the first load of its feed.
    pub async fn open_of_topic(conn: &mut Conn, topic: Id) -> Result<Vec<JobRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM jobs
              WHERE node_id IN (SELECT node_id FROM threads WHERE topic_id = $1)
                AND state IN ('queued', 'running')
              ORDER BY created_at, id"
        )))
        .bind(topic)
        .fetch_all(conn)
        .await?)
    }

    /// The jobs about the threads of a topic that were stamped on its feed in `(after, upto]`, over or
    /// not, in the order of their numbers, at most `limit`.
    pub async fn of_topic_between(
        conn: &mut Conn,
        topic: Id,
        after: Seq,
        upto: Seq,
        limit: i64,
    ) -> Result<Vec<JobRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM jobs
              WHERE node_id IN (SELECT node_id FROM threads WHERE topic_id = $1)
                AND topic_seq > $2 AND topic_seq <= $3
              ORDER BY topic_seq, id LIMIT $4"
        )))
        .bind(topic)
        .bind(after)
        .bind(upto)
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// Changed jobs of the topic's already authorized threads, limited after permission filtering.
    pub async fn of_threads_between(
        conn: &mut Conn,
        threads: &[Id],
        after: Seq,
        upto: Seq,
        limit: i64,
    ) -> Result<Vec<JobRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM jobs WHERE node_id = ANY($1) AND topic_seq > $2 AND topic_seq <= $3 ORDER BY topic_seq, id LIMIT $4"
        )))
        .bind(threads).bind(after).bind(upto).bind(limit).fetch_all(conn).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::resources::testing::insert_user;
    use orch_testkit::database::TestDb;
    use orch_types::new_id;
    use serde_json::json;

    async fn environment(conn: &mut Conn) -> Id {
        let owner = insert_user(conn, "ada").await.unwrap();
        let (org, environment) = (new_id(), new_id());
        crate::queries::hierarchy::testing::insert_node(conn, org, "org", None, "Acme", owner)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root)
             VALUES ($1, 'Main', $2, $3, 'connected', $3, '/work')",
        )
        .bind(environment)
        .bind(owner)
        .bind(org)
        .execute(&mut *conn)
        .await
        .unwrap();
        environment
    }

    fn new_job(environment: Id, id: Id) -> NewJob<'static> {
        NewJob {
            id,
            kind: kind::ENVIRONMENT_PROVISION,
            node_id: None,
            environment_id: Some(environment),
            provider_id: None,
            plan: &Json::Null,
            progress: &Json::Null,
            created_by: None,
        }
    }

    #[tokio::test]
    async fn a_job_is_run_by_one_runner_at_a_time_and_taken_over_when_its_lease_lapses() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let environment = environment(&mut conn).await;
        let (job, one, two) = (new_id(), new_id(), new_id());
        let plan = json!({});
        let now = Utc::now();
        insert(
            &mut conn,
            &NewJob {
                plan: &plan,
                progress: &plan,
                ..new_job(environment, job)
            },
            now,
        )
        .await
        .unwrap();
        let until = now + chrono::Duration::seconds(30);
        assert!(claim(&mut conn, job, one, until, now).await.unwrap());
        assert!(
            !claim(&mut conn, job, two, until, now).await.unwrap(),
            "a lease that has not run out is not taken"
        );
        assert!(
            claim(&mut conn, job, one, until, now).await.unwrap(),
            "its holder renews it"
        );
        assert!(
            runnable(&mut conn, &[kind::ENVIRONMENT_PROVISION], now, 10)
                .await
                .unwrap()
                .is_empty()
        );
        let later = until + chrono::Duration::seconds(1);
        assert_eq!(
            runnable(&mut conn, &[kind::ENVIRONMENT_PROVISION], later, 10)
                .await
                .unwrap(),
            [job]
        );
        assert!(
            claim(
                &mut conn,
                job,
                two,
                later + chrono::Duration::seconds(30),
                later
            )
            .await
            .unwrap()
        );
        release(&mut conn, job, two, later).await.unwrap();
        assert!(claim(&mut conn, job, one, until, later).await.unwrap());
    }

    #[tokio::test]
    async fn every_change_of_a_job_takes_the_next_number_of_its_environments_feed() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let environment = environment(&mut conn).await;
        let job = new_id();
        let plan = json!({});
        let made = insert(
            &mut conn,
            &NewJob {
                plan: &plan,
                progress: &plan,
                ..new_job(environment, job)
            },
            Utc::now(),
        )
        .await
        .unwrap();
        let first = made.env_seq.unwrap();
        save_progress(&mut conn, job, &json!({ "percent": 10 }), Utc::now())
            .await
            .unwrap();
        let after = super::job(&mut conn, job).await.unwrap().unwrap();
        assert!(after.env_seq.unwrap() > first);
        assert!(
            finish(
                &mut conn,
                job,
                state::FAILED,
                &json!({}),
                None,
                Some(&json!({ "code": "x" })),
                Utc::now()
            )
            .await
            .unwrap()
        );
        assert!(
            !save_progress(&mut conn, job, &json!({ "percent": 90 }), Utc::now())
                .await
                .unwrap(),
            "a job that is over is not written to"
        );
    }

    #[tokio::test]
    async fn a_request_to_cancel_stands_until_the_job_has_passed_its_commit() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let environment = environment(&mut conn).await;
        let (job, committed, person) = (new_id(), new_id(), new_id());
        let plan = json!({});
        for id in [job, committed] {
            insert(
                &mut conn,
                &NewJob {
                    plan: &plan,
                    progress: &plan,
                    ..new_job(environment, id)
                },
                Utc::now(),
            )
            .await
            .unwrap();
        }
        merge_plan(
            &mut conn,
            committed,
            &json!({ "committed": true }),
            Utc::now(),
        )
        .await
        .unwrap();
        assert!(
            request_cancel(&mut conn, job, person, Utc::now())
                .await
                .unwrap()
        );
        assert!(
            request_cancel(&mut conn, job, person, Utc::now())
                .await
                .unwrap(),
            "a second request is the same request"
        );
        assert!(cancel_requested(&mut conn, job).await.unwrap());
        assert!(
            !request_cancel(&mut conn, committed, person, Utc::now())
                .await
                .unwrap()
        );
        assert!(!cancel_requested(&mut conn, committed).await.unwrap());
    }
}

// Runtime updates and moves: common holds, durable plans and bounded batches.

/// Holds and plans shared by thread switches, runtime updates and moves.
pub mod changes {
    use super::{JobRow, NewJob, forks};
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    /// The shared predicate for jobs holding a thread while its binding or runtime changes.
    pub(crate) fn thread_hold_predicate(thread: &str) -> String {
        "(j.state IN ('queued','running') OR
                (j.kind = 'workspace_restore' AND j.state = 'failed' AND j.plan ->> 'restore_recovery_required' = 'true')) AND
             ((j.kind = 'thread_fork' AND ((j.node_id = __THREAD__ AND j.plan ->> 'result' = 'replace')
                OR j.plan ->> 'new_thread' = (__THREAD__)::text))
              OR (j.kind='thread_continuation' AND j.node_id=__THREAD__)
              OR (j.kind IN ('environment_move','environment_recreate','environment_restart','workspace_restore') AND EXISTS
                  (SELECT 1 FROM threads t WHERE t.node_id = __THREAD__ AND t.environment_id = j.environment_id))
              OR (j.kind = 'provider_recreate' AND EXISTS
                  (SELECT 1 FROM threads t WHERE t.node_id = __THREAD__ AND t.provider_id = j.provider_id)))"
            .replace("__THREAD__", thread)
    }

    /// The open job holding a thread, including changes to its environment or provider.
    pub async fn thread_hold(conn: &mut Conn, thread: Id) -> Result<Option<JobRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT j.* FROM jobs j WHERE {} ORDER BY j.created_at, j.id LIMIT 1",
            thread_hold_predicate("$1")
        )))
        .bind(thread)
        .fetch_optional(conn)
        .await?)
    }

    /// A job using an environment, including a fork that copies from or into it.
    pub async fn environment_hold(
        conn: &mut Conn,
        environment: Id,
    ) -> Result<Option<JobRow>, DbError> {
        held_environment(conn, environment, true).await
    }

    /// A whole-environment hold, leaving thread-fork compatibility to the caller's batch check.
    pub async fn environment_change_hold(
        conn: &mut Conn,
        environment: Id,
    ) -> Result<Option<JobRow>, DbError> {
        held_environment(conn, environment, false).await
    }

    async fn held_environment(
        conn: &mut Conn,
        environment: Id,
        include_thread_jobs: bool,
    ) -> Result<Option<JobRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT j.* FROM jobs j WHERE (j.state IN ('queued','running') OR
                (j.kind = 'workspace_restore' AND j.state = 'failed' AND j.plan ->> 'restore_recovery_required' = 'true')) AND
             ((j.kind IN ('environment_provision','environment_remove','environment_move','environment_recreate','workspace_prepare','git_commit','git_push','workspace_restore')
                 AND j.environment_id = $1
                 AND NOT (j.kind = 'environment_remove' AND j.plan ->> 'runtime' IS NOT NULL))
              OR (j.kind='thread_continuation' AND j.environment_id=$1)
              OR ($2 AND j.kind = 'thread_fork' AND (j.environment_id = $1
                   OR j.plan ->> 'from_environment' = $1::text OR j.plan ->> 'to_environment' = $1::text
                   OR j.plan ->> 'new_environment' = $1::text))
              OR (j.kind = 'provider_recreate' AND EXISTS
                   (SELECT 1 FROM threads t WHERE t.environment_id = $1 AND t.provider_id = j.provider_id)))
             ORDER BY j.created_at, j.id LIMIT 1",
        ).bind(environment).bind(include_thread_jobs).fetch_optional(conn).await?)
    }

    /// A job using a provider's runtime or any thread bound to it.
    pub async fn provider_hold(conn: &mut Conn, provider: Id) -> Result<Option<JobRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT j.* FROM jobs j WHERE j.state IN ('queued','running') AND
             ((j.provider_id = $1 AND NOT (j.kind = 'environment_remove' AND j.plan ->> 'runtime' IS NOT NULL)) OR EXISTS
                (SELECT 1 FROM threads t WHERE t.provider_id = $1 AND
                  ((j.kind = 'thread_fork' AND j.node_id = t.node_id)
                   OR (j.kind IN ('environment_move','environment_recreate','environment_restart') AND j.environment_id = t.environment_id))))
             ORDER BY j.created_at, j.id LIMIT 1",
        ).bind(provider).fetch_optional(conn).await?)
    }

    /// Threads affected by a runtime change, locked in identifier order by its confirmation.
    pub async fn threads(
        conn: &mut Conn,
        environment: Option<Id>,
        provider: Option<Id>,
        lock: bool,
    ) -> Result<Vec<Id>, DbError> {
        let statement = if lock {
            "SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
             WHERE n.deleted_at IS NULL AND (($1::uuid IS NOT NULL AND t.environment_id = $1)
                OR ($2::uuid IS NOT NULL AND t.provider_id = $2)) ORDER BY t.node_id FOR UPDATE OF t"
        } else {
            "SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
             WHERE n.deleted_at IS NULL AND (($1::uuid IS NOT NULL AND t.environment_id = $1)
                OR ($2::uuid IS NOT NULL AND t.provider_id = $2)) ORDER BY t.node_id"
        };
        Ok(sqlx::query_scalar(statement)
            .bind(environment)
            .bind(provider)
            .fetch_all(conn)
            .await?)
    }

    /// Locks an unexpired plan to serialize two confirmations of it.
    pub async fn lock_plan(
        conn: &mut Conn,
        id: Id,
    ) -> Result<Option<forks::PreflightRow>, DbError> {
        Ok(sqlx::query_as("SELECT id, kind, node_id, environment_id, created_by, plan, expires_at, created_at FROM preflights WHERE id = $1 FOR UPDATE")
            .bind(id).fetch_optional(conn).await?)
    }

    /// Inserts a runtime job and records the batch it belongs to in the caller's transaction.
    pub async fn insert(
        conn: &mut Conn,
        job: &NewJob<'_>,
        batch: Option<Id>,
        now: DateTime<Utc>,
    ) -> Result<JobRow, DbError> {
        let mut row = super::insert(conn, job, now).await?;
        if let Some(batch) = batch {
            sqlx::query("UPDATE jobs SET batch_id = $2 WHERE id = $1")
                .bind(row.id)
                .bind(batch)
                .execute(conn)
                .await?;
            row.batch_id = Some(batch);
        }
        Ok(row)
    }

    /// Runtime changes ready to run, respecting each batch's concurrency limit.
    pub async fn runnable(
        conn: &mut Conn,
        now: DateTime<Utc>,
        parallel: i64,
        limit: i64,
    ) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT j.id FROM jobs j WHERE (j.kind IN ('environment_recreate','provider_recreate','environment_move') OR (j.kind='environment_provision' AND j.plan ? 'recovery'))
             AND j.state IN ('queued','running')
             AND (j.plan -> 'runner' IS NULL OR (j.plan -> 'runner' ->> 'until')::timestamptz <= $1)
             AND (j.state = 'running' OR j.batch_id IS NULL
                  OR (SELECT count(*) FROM jobs b WHERE b.batch_id = j.batch_id AND b.state = 'running') < $2)
             ORDER BY j.created_at, j.id LIMIT $3",
        ).bind(now).bind(parallel).bind(limit).fetch_all(conn).await?)
    }

    /// The definition recorded by the command that made a runtime, or by its job before sending it.
    pub async fn definition(conn: &mut Conn, runtime: Id) -> Result<Option<Json>, DbError> {
        let sent: Option<Json> = sqlx::query_scalar(
            "SELECT body FROM commands WHERE kind = 'node.runtime.create' AND body ->> 'runtime_id' = $1::text
             AND state IN ('queued','sent','accepted','succeeded') ORDER BY created_at DESC, id DESC LIMIT 1",
        ).bind(runtime).fetch_optional(&mut *conn).await?;
        let mut saved =
            match sent {
                Some(sent) => Some(sent),
                None => sqlx::query_scalar(
                    "SELECT plan -> 'created_spec' FROM jobs WHERE plan ->> 'runtime' = $1::text
             AND plan -> 'created_spec' IS NOT NULL ORDER BY created_at DESC, id DESC LIMIT 1",
                )
                .bind(runtime)
                .fetch_optional(&mut *conn)
                .await?,
            };
        if let Some(spec)=saved.as_mut()
            && let Some(resources)=sqlx::query_scalar::<_,Json>("SELECT body->'resources' FROM commands WHERE kind='node.resources.apply' AND body->>'runtime_id'=$1::text AND state='succeeded' ORDER BY created_at DESC,id DESC LIMIT 1").bind(runtime).fetch_optional(&mut *conn).await? {spec["resources"]=resources;}
        Ok(saved)
    }

    /// Builds recorded by the job that created this unit, including its connector.
    pub async fn tools_of(conn: &mut Conn, runtime: Id) -> Result<Option<Json>, DbError> {
        Ok(sqlx::query_scalar("SELECT plan -> 'tools' FROM jobs WHERE plan ->> 'runtime' = $1::text AND plan -> 'tools' IS NOT NULL AND state IN ('running','succeeded') ORDER BY created_at DESC,id DESC LIMIT 1")
            .bind(runtime).fetch_optional(conn).await?)
    }

    /// Gives a queued replacement the move's phases and destination in the same transaction.
    pub async fn make_move(
        conn: &mut Conn,
        id: Id,
        plan: &Json,
        progress: &Json,
        now: DateTime<Utc>,
    ) -> Result<JobRow, DbError> {
        Ok(sqlx::query_as("UPDATE jobs SET kind = 'environment_move', plan = $2, progress = $3, updated_at = $4 WHERE id = $1 AND state = 'queued' RETURNING *")
            .bind(id).bind(plan).bind(progress).bind(now).fetch_one(conn).await?)
    }

    /// The stored definitions of several runtime units, read together for fleet views.
    pub async fn definitions(
        conn: &mut Conn,
        runtimes: &[Id],
    ) -> Result<Vec<(Id, Option<Json>)>, DbError> {
        Ok(sqlx::query_as("SELECT r.id, frozen.spec || CASE WHEN live.resources IS NULL THEN '{}'::jsonb ELSE jsonb_build_object('resources',live.resources) END
          FROM runtimes r CROSS JOIN LATERAL (SELECT COALESCE(
            (SELECT c.body FROM commands c WHERE c.kind = 'node.runtime.create' AND c.body ->> 'runtime_id' = r.id::text AND c.state IN ('queued','sent','accepted','succeeded') ORDER BY c.created_at DESC,c.id DESC LIMIT 1),
            (SELECT j.plan -> 'created_spec' FROM jobs j WHERE j.plan ->> 'runtime' = r.id::text AND j.plan -> 'created_spec' IS NOT NULL ORDER BY j.created_at DESC,j.id DESC LIMIT 1))
          AS spec) frozen
          LEFT JOIN LATERAL (SELECT c.body->'resources' AS resources FROM commands c WHERE c.kind='node.resources.apply' AND c.body->>'runtime_id'=r.id::text AND c.state='succeeded' ORDER BY c.created_at DESC,c.id DESC LIMIT 1) live ON true
          WHERE r.id = ANY($1)")
            .bind(runtimes).fetch_all(conn).await?)
    }

    /// Every unit's frozen build list, using the same successful or running producer as a single read.
    pub async fn tools_among(
        conn: &mut Conn,
        runtimes: &[Id],
    ) -> Result<Vec<(Id, Option<Json>)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT r.id,
            (SELECT j.plan->'tools' FROM jobs j WHERE j.plan->>'runtime'=r.id::text
                AND j.plan->'tools' IS NOT NULL AND j.state IN ('running','succeeded')
                ORDER BY j.created_at DESC,j.id DESC LIMIT 1)
            FROM runtimes r WHERE r.id=ANY($1)",
        )
        .bind(runtimes)
        .fetch_all(conn)
        .await?)
    }
}

// Removed environments reserve their files through the same job lease and row lock.
/// Jobs that restore or purge kept environment files.
pub mod retained {
    use super::{Conn, DbError, JobRow};
    /// Restores a verified stopped generation.
    pub const RESTORE: &str = "environment_restore";
    /// Confirms physical volume removal.
    pub const PURGE: &str = "environment_purge";
    /// All serialized environment jobs, including the separate kept-file lifecycle.
    pub const SERIALIZED: [&str; 12] = [
        "capability_change",
        super::kind::ENVIRONMENT_PROVISION,
        super::kind::ENVIRONMENT_RECREATE,
        super::kind::ENVIRONMENT_RESTART,
        super::kind::ENVIRONMENT_MOVE,
        super::kind::ENVIRONMENT_REMOVE,
        RESTORE,
        PURGE,
        super::kind::WORKSPACE_PREPARE,
        super::kind::GIT_COMMIT,
        super::kind::GIT_PUSH,
        super::kind::WORKSPACE_RESTORE,
    ];

    /// Every active lifecycle job, including restore and purge, in stable creation order.
    pub async fn open_of_environment(
        conn: &mut Conn,
        environment: orch_types::Id,
    ) -> Result<Vec<JobRow>, DbError> {
        open_of_environment_except(conn, environment, None).await
    }

    /// A common cleanup excludes its own membership while retaining every other lifecycle owner.
    pub async fn open_of_environment_except(
        conn: &mut Conn,
        environment: orch_types::Id,
        except: Option<orch_types::Id>,
    ) -> Result<Vec<JobRow>, DbError> {
        Ok(sqlx::query_as("SELECT * FROM jobs WHERE ($3::uuid IS NULL OR id<>$3) AND
            ((environment_id=$1 AND kind=ANY($2) AND (state IN ('queued','running') OR (kind='workspace_restore' AND state='failed' AND plan->>'restore_recovery_required'='true')))
            OR (kind='storage_cleanup' AND state IN ('queued','running') AND plan->'recipe'->'work' @> jsonb_build_array(jsonb_build_object('choice','purge','target',jsonb_build_object('kind','environment','environment',$1::uuid::text))))) ORDER BY created_at,id")
            .bind(environment).bind(&SERIALIZED[..]).bind(except).fetch_all(conn).await?)
    }
}

// Lifecycle scheduling: a frozen future removal waits without occupying a runner's batch.
/// Runnable jobs excluding future cleanup plans, with cancellation eligible immediately.
pub async fn lifecycle_runnable(
    conn: &mut Conn,
    kinds: &[&str],
    now: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT id FROM jobs WHERE kind=ANY($1) AND
        (state IN ('queued','running') OR (kind = 'workspace_restore' AND state = 'failed' AND plan ->> 'restore_recovery_required' = 'true'))
        AND NOT (kind='environment_provision' AND plan ? 'recovery')
        AND (plan->'runner' IS NULL OR (plan->'runner'->>'until')::timestamptz <= $2)
        AND (plan ? 'cancel' OR COALESCE((plan->>'not_before')::timestamptz,'-infinity'::timestamptz) <= $2)
        ORDER BY created_at,id LIMIT $3")
        .bind(kinds.iter().map(|kind|(*kind).to_owned()).collect::<Vec<_>>())
        .bind(now).bind(limit).fetch_all(conn).await?)
}

/// One maker's active document notification jobs, in a bounded identifier page.
pub async fn document_notices_page(
    conn: &mut Conn,
    by: Id,
    document: Id,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<JobRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLUMNS} FROM jobs WHERE kind='document_notices' AND state IN ('queued','running')
        AND created_by=$1 AND plan#>>'{{notice,document}}'=$2::uuid::text AND ($3::uuid IS NULL OR id<$3) ORDER BY id DESC LIMIT $4")))
        .bind(by).bind(document).bind(after).bind(limit).fetch_all(conn).await?)
}

/// One next notification job; only four valid leases can precede a queued row under the durable slots.
pub const DOCUMENT_NOTICE_RUNNABLE_SQL: &str =
    "SELECT id FROM jobs WHERE kind='document_notices' AND state IN ('queued','running')
    AND (plan#>>'{runner,until}' IS NULL OR (plan#>>'{runner,until}')::timestamptz <= $1)
    ORDER BY created_at,id LIMIT 1 FOR UPDATE SKIP LOCKED";

/// Claims work only into the four durable slots. Caller holds one transaction; concurrent engines skip its slots.
pub async fn claim_document_notices(
    conn: &mut Conn,
    now: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<Vec<(Id, Id)>, DbError> {
    let slots: Vec<i16> = sqlx::query_scalar(
        "SELECT s.slot FROM document_notice_slots s LEFT JOIN jobs j ON j.id=s.job_id
        WHERE s.job_id IS NULL OR j.id IS NULL OR j.state NOT IN ('queued','running')
          OR j.plan#>>'{runner,until}' IS NULL OR (j.plan#>>'{runner,until}')::timestamptz <= $1
        ORDER BY s.slot FOR UPDATE OF s SKIP LOCKED",
    )
    .bind(now)
    .fetch_all(&mut *conn)
    .await?;
    let mut taken = Vec::with_capacity(slots.len());
    for slot in slots {
        let next: Option<Id> = sqlx::query_scalar(DOCUMENT_NOTICE_RUNNABLE_SQL)
            .bind(now)
            .fetch_optional(&mut *conn)
            .await?;
        let Some(id) = next else {
            break;
        };
        let runner = orch_types::new_id();
        if claim(conn, id, runner, until, now).await? {
            sqlx::query("UPDATE document_notice_slots SET job_id=$2 WHERE slot=$1")
                .bind(slot)
                .bind(id)
                .execute(&mut *conn)
                .await?;
            taken.push((id, runner));
        }
    }
    Ok(taken)
}
