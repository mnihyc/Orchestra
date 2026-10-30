//! Threads with their side threads, segments, inputs and turns with what each was let past, items, reads and drafts.
//!
//! Every function takes a connection, so that it runs alone or inside the
//! transaction of whoever calls it. A thread's rows are changed under the
//! lock of the thread's own row: the caller takes it with [`lock`], asks
//! [`crate::sequences::next`] for a number for each change, and stamps the
//! rows it writes with it. A change of state is a compare-and-set: the
//! functions that change one say, by their answer, whether this call was the
//! one that changed it, and a caller that was not does nothing more.
//!
//! Enumerated columns are read and written as the words the schema checks;
//! the engine turns them into its own types.

use crate::queries::hierarchy;
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json, Seq};

pub mod notices;
pub mod reading;

/// The words of the `threads.state` column.
pub mod thread_state {
    /// Waiting for something to be said.
    pub const IDLE: &str = "idle";
    /// Getting ready to take a turn.
    pub const STARTING: &str = "starting";
    /// A turn is under way.
    pub const RUNNING: &str = "running";
    /// A turn waits for an approval.
    pub const WAITING_APPROVAL: &str = "waiting_approval";
    /// It cannot go on.
    pub const BLOCKED: &str = "blocked";
}

/// The words of the `thread_inputs.state` column.
pub mod input_state {
    /// Waiting its turn.
    pub const PENDING: &str = "pending";
    /// Being handed to the agent.
    pub const DELIVERING: &str = "delivering";
    /// Handed over.
    pub const DELIVERED: &str = "delivered";
    /// Cancelled before it was.
    pub const CANCELLED: &str = "cancelled";
    /// It could not be.
    pub const FAILED: &str = "failed";
}

/// The words of the `turns.state` column.
pub mod turn_state {
    /// Getting ready.
    pub const STARTING: &str = "starting";
    /// Under way.
    pub const RUNNING: &str = "running";
    /// Done.
    pub const COMPLETED: &str = "completed";
    /// Stopped.
    pub const INTERRUPTED: &str = "interrupted";
    /// Failed.
    pub const FAILED: &str = "failed";
}

// ---------------------------------------------------------------- the thread

/// The columns of a thread and of its node that the engine reads.
const THREAD_SELECT: &str = "SELECT t.node_id, t.topic_id, t.environment_id, t.workspace_dir, t.state, t.blocked_reason,
        t.source, t.links, t.work_observed,
        t.blocked_detail, t.starting_step, t.agent_id, t.provider_id, t.arrangement, t.executor, t.tool_set,
        t.current_segment_id, t.current_turn_id, t.active_continuation_id, t.execution_revision, t.next_seq, t.seq_state, t.activity_seq,
        COALESCE((SELECT jsonb_agg(notice ORDER BY position) FROM
          (SELECT notice,position FROM thread_pending_notices WHERE thread_id=t.node_id ORDER BY position LIMIT 64) head),'[]'::jsonb) AS pending_notices,
        t.usage, t.last_activity_at, t.side_of, t.side_from_turn,
        t.queue_held, t.queue_held_by, t.queue_held_at, t.queue_revision, t.side_reading,
        n.name, n.owner_id, n.org_id, n.parent_id, n.archived_at, n.deleted_at
   FROM threads t JOIN nodes n ON n.id = t.node_id";

/// A thread with the facts of its node that the engine needs.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ThreadRow {
    /// The thread's node.
    pub node_id: Id,
    /// The topic it lies in.
    pub topic_id: Id,
    /// Where its tools run.
    pub environment_id: Option<Id>,
    /// Its directory there.
    pub workspace_dir: String,
    /// Safe source facts and the frozen preparation choice.
    pub source: Option<Json>,
    /// The accepted links of this conversation.
    pub links: Json,
    /// The last explicitly requested Git work observation.
    pub work_observed: Option<Json>,
    /// What it is doing.
    pub state: String,
    /// Why it cannot go on.
    pub blocked_reason: Option<String>,
    /// A sentence about that.
    pub blocked_detail: Option<String>,
    /// What a starting thread waits for.
    pub starting_step: Option<String>,
    /// Its agent profile.
    pub agent_id: Option<Id>,
    /// Its provider.
    pub provider_id: Option<Id>,
    /// Its arrangement type.
    pub arrangement: Option<String>,
    /// What carries out its tool calls.
    pub executor: Option<String>,
    /// The tool set of that executor.
    pub tool_set: Option<String>,
    /// The segment under way.
    pub current_segment_id: Option<Id>,
    /// The turn under way.
    pub current_turn_id: Option<Id>,
    /// The one continuation selected for execution, independent of a reader's view.
    pub active_continuation_id: Id,
    /// Monotonic selection revision, including away-and-back transitions.
    pub execution_revision: i64,
    /// The next number of the thread sequence.
    pub next_seq: i64,
    /// The sequence of the last change to the thread row.
    pub seq_state: i64,
    /// The sequence of the last change worth a reader's attention.
    pub activity_seq: i64,
    /// The first bounded page of queued notices; native recovery reads its own scoped lane.
    pub pending_notices: Json,
    /// What it used in all.
    pub usage: Option<Json>,
    /// When something last happened in it.
    pub last_activity_at: Option<DateTime<Utc>>,
    /// For a side thread: its parent.
    pub side_of: Option<Id>,
    /// For a side thread: the last turn of the parent it took.
    pub side_from_turn: Option<i32>,
    /// Whether a person's Stop holds what waited in its queue.
    pub queue_held: bool,
    /// Who stopped, while held.
    pub queue_held_by: Option<Id>,
    /// When, while held.
    pub queue_held_at: Option<DateTime<Utc>>,
    /// The revision a resume names.
    pub queue_revision: i64,
    /// For a side thread: how it reads without acting (`read_only`, `approval_gated`, `no_tools`).
    pub side_reading: Option<String>,
    /// Its name.
    pub name: String,
    /// Its first owner.
    pub owner_id: Id,
    /// Its root org.
    pub org_id: Id,
    /// The node it lies in.
    pub parent_id: Option<Id>,
    /// When it was archived itself.
    pub archived_at: Option<DateTime<Utc>>,
    /// When it was deleted itself.
    pub deleted_at: Option<DateTime<Utc>>,
}

impl ThreadRow {
    /// The highest number of the sequence given out so far.
    #[must_use]
    pub const fn seq(&self) -> Seq {
        self.next_seq - 1
    }
}

/// The thread whose node is `id`.
pub async fn thread(conn: &mut Conn, id: Id) -> Result<Option<ThreadRow>, DbError> {
    let sql = format!("{THREAD_SELECT} WHERE t.node_id = $1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// The threads whose nodes are `ids`, in one statement: what a plan about many threads reads instead of a query for each.
pub async fn threads_among(conn: &mut Conn, ids: &[Id]) -> Result<Vec<ThreadRow>, DbError> {
    let sql = format!("{THREAD_SELECT} WHERE t.node_id = ANY($1)");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(ids)
        .fetch_all(conn)
        .await?)
}

/// The thread whose node is `id`, with its row locked until the transaction
/// ends: whoever changes a thread's rows takes this first, and so the changes
/// of a thread are made one after the other.
pub async fn lock(conn: &mut Conn, id: Id) -> Result<Option<ThreadRow>, DbError> {
    let sql = format!("{THREAD_SELECT} WHERE t.node_id = $1 FOR UPDATE OF t");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// What a thread is set to: the state, and with it the reason, the detail
/// and the step that belong to that state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateChange<'a> {
    /// The new state.
    pub state: &'a str,
    /// For `blocked`: why.
    pub reason: Option<&'a str>,
    /// A sentence about that.
    pub detail: Option<&'a str>,
    /// For `starting`: what it waits for.
    pub step: Option<&'a str>,
}

impl<'a> StateChange<'a> {
    /// The thread is in `state`, which needs nothing more said.
    #[must_use]
    pub const fn plain(state: &'a str) -> Self {
        Self {
            state,
            reason: None,
            detail: None,
            step: None,
        }
    }

    /// The thread is blocked for `reason`.
    #[must_use]
    pub const fn blocked(reason: &'a str, detail: Option<&'a str>) -> Self {
        Self {
            state: thread_state::BLOCKED,
            reason: Some(reason),
            detail,
            step: None,
        }
    }

    /// The thread is starting, waiting for `step`.
    #[must_use]
    pub const fn starting(step: Option<&'a str>) -> Self {
        Self {
            state: thread_state::STARTING,
            reason: None,
            detail: None,
            step,
        }
    }
}

/// Sets the state of a thread, where it is one of `from`. The answer says
/// whether this call changed it. `seq` is the number the change is stamped
/// with, and `at` the moment of the thread's last activity when the change
/// is one. A change is also stamped on the tree of the thread's org, whose
/// rows say what each thread is doing, so that those who follow the tree are
/// told.
pub async fn set_state(
    conn: &mut Conn,
    thread: Id,
    from: &[&str],
    to: &StateChange<'_>,
    seq: Seq,
) -> Result<bool, DbError> {
    let from: Vec<String> = from.iter().map(|state| (*state).to_owned()).collect();
    let done = sqlx::query(
        "UPDATE threads SET state = $3, blocked_reason = $4, blocked_detail = $5, starting_step = $6, seq_state = $7
          WHERE node_id = $1 AND state = ANY($2)",
    )
    .bind(thread)
    .bind(from)
    .bind(to.state)
    .bind(to.reason)
    .bind(to.detail)
    .bind(to.step)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    let changed = done.rows_affected() == 1;
    if changed {
        hierarchy::stamp(conn, thread).await?;
    }
    Ok(changed)
}

/// Says what a `starting` thread waits for; nothing is done to a thread in
/// another state. The answer says whether the step changed.
pub async fn set_starting_step(
    conn: &mut Conn,
    thread: Id,
    step: Option<&str>,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE threads SET starting_step = $2, seq_state = $3
          WHERE node_id = $1 AND state = 'starting' AND starting_step IS DISTINCT FROM $2",
    )
    .bind(thread)
    .bind(step)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Sets the segment and the turn under way (none for none), stamping the
/// thread row.
pub async fn set_current(
    conn: &mut Conn,
    thread: Id,
    segment: Option<Id>,
    turn: Option<Id>,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE threads SET native_catalog=CASE WHEN current_segment_id IS DISTINCT FROM $2 THEN NULL ELSE native_catalog END, current_segment_id = $2, current_turn_id = $3, seq_state = $4 WHERE node_id = $1",
    )
    .bind(thread)
    .bind(segment)
    .bind(turn)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(())
}

/// What a thread is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding<'a> {
    /// The agent profile.
    pub agent: Id,
    /// The provider.
    pub provider: Id,
    /// The arrangement type, in the word the thread's column has.
    pub arrangement: &'a str,
    /// The executor that carries out the tool calls; none is the harness itself.
    pub executor: Option<&'a str>,
    /// The tool set of that executor.
    pub tool_set: Option<&'a str>,
}

/// Binds a thread to an agent profile, a provider and an arrangement,
/// stamping the thread row.
pub async fn set_binding(
    conn: &mut Conn,
    thread: Id,
    binding: &Binding<'_>,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE threads SET agent_id = $2, provider_id = $3, arrangement = $4, executor = $5, tool_set = $6, seq_state = $7
          WHERE node_id = $1",
    )
    .bind(thread)
    .bind(binding.agent)
    .bind(binding.provider)
    .bind(binding.arrangement)
    .bind(binding.executor)
    .bind(binding.tool_set)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(())
}

/// Records that something happened in the thread that a reader's attention
/// is worth: the sequence of it, and the moment. The tree of the thread's org
/// carries both, so it is stamped too.
pub async fn mark_activity(
    conn: &mut Conn,
    thread: Id,
    seq: Seq,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE threads SET activity_seq = GREATEST(activity_seq, $2), last_activity_at = GREATEST(last_activity_at,$3), seq_state = GREATEST(seq_state, $2)
          WHERE node_id = $1",
    )
    .bind(thread)
    .bind(seq)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    hierarchy::stamp(conn, thread).await?;
    hierarchy::touch_activity(conn, thread, at).await?;
    Ok(())
}

/// Records the moment of the thread's last activity without moving what is
/// worth attention.
pub async fn touch(conn: &mut Conn, thread: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    // To the second: a thread that streams is touched by every piece of text, and each write is a new version of
    // its row; a time within the second it already holds changes no order a reader sees.
    let touched = sqlx::query(
        "UPDATE threads SET last_activity_at = $2
          WHERE node_id = $1 AND (last_activity_at IS NULL OR last_activity_at <= $2 - interval '1 second')",
    )
    .bind(thread)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    if touched.rows_affected() > 0 {
        hierarchy::touch_activity(conn, thread, at).await?;
    }
    Ok(())
}

/// Replaces what the thread used in all, stamping the thread row.
pub async fn set_usage(conn: &mut Conn, thread: Id, usage: &Json, seq: Seq) -> Result<(), DbError> {
    sqlx::query("UPDATE threads SET usage = $2, seq_state = $3 WHERE node_id = $1")
        .bind(thread)
        .bind(usage)
        .bind(seq)
        .execute(conn)
        .await?;
    Ok(())
}

/// Makes the node and the thread rows of a new thread: a thread node of
/// `parent` (a topic), named `name`, first owned and made by `owner`, with
/// the topic it lies in. Whatever else a node has (its page, its owners
/// row, its chat) is the hierarchy's to add in the same transaction.
pub async fn insert_thread(
    conn: &mut Conn,
    id: Id,
    parent: Id,
    name: &str,
    owner: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let made = sqlx::query(
        "INSERT INTO nodes (id, kind, parent_id, org_id, path, name, owner_id, created_by, created_at, updated_at)
         SELECT $1, 'thread', p.id, p.org_id, p.path || $1, $2, $3, $3, $4, $4
           FROM nodes p WHERE p.id = $5 AND p.kind = 'topic' AND p.deleted_at IS NULL",
    )
    .bind(id)
    .bind(name)
    .bind(owner)
    .bind(at)
    .bind(parent)
    .execute(&mut *conn)
    .await?;
    if made.rows_affected() == 0 {
        return Ok(false);
    }
    sqlx::query("INSERT INTO threads (node_id, topic_id) VALUES ($1, $2)")
        .bind(id)
        .bind(parent)
        .execute(conn)
        .await?;
    Ok(true)
}

/// Puts a thread in an environment and a directory of it, stamping the
/// thread row.
pub async fn set_environment(
    conn: &mut Conn,
    thread: Id,
    environment: Id,
    directory: &str,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE threads SET environment_id = $2, workspace_dir = $3, seq_state = $4 WHERE node_id = $1",
    )
    .bind(thread)
    .bind(environment)
    .bind(directory)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(())
}

/// Renames the node of a thread from what automation or the harness says,
/// where the title is still automatic. The answer says whether it changed;
/// the caller then stamps the thread row with [`set_seq_state`].
pub async fn rename_if_automatic(
    conn: &mut Conn,
    thread: Id,
    name: &str,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE nodes SET name = $2, auto_updated_at = $3, revision = revision + 1, updated_at = $3
          WHERE id = $1 AND title_source = 'auto' AND name <> $2",
    )
    .bind(thread)
    .bind(name)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Stamps the thread row with the number of a change of its node.
pub async fn set_seq_state(conn: &mut Conn, thread: Id, seq: Seq) -> Result<(), DbError> {
    sqlx::query("UPDATE threads SET seq_state = GREATEST(seq_state, $2) WHERE node_id = $1")
        .bind(thread)
        .bind(seq)
        .execute(conn)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------- what the supervisor looks for

/// The threads in `state`, oldest first, at most `limit`.
pub async fn threads_in_state(
    conn: &mut Conn,
    state: &str,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
          WHERE t.state = $1 AND n.deleted_at IS NULL ORDER BY t.node_id LIMIT $2",
    )
    .bind(state)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// The threads that are idle or blocked and have an input that waits, which
/// a pre-flight may start: the work that a restart, or a cause that cleared
/// while nobody listened, leaves undone.
pub async fn threads_with_waiting_inputs(conn: &mut Conn, limit: i64) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
          WHERE t.state IN ('idle', 'blocked') AND n.deleted_at IS NULL
            AND EXISTS (SELECT 1 FROM thread_inputs i WHERE i.thread_id = t.node_id AND i.state = 'pending')
          ORDER BY t.node_id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Blocked threads whose block is one of `reasons`, a cause of their own (what they are bound to, where they run),
/// whether or not anything waits: the recheck looks at them again, so that a cause mended without an act the
/// engine hears of still lets such a thread be idle.
pub async fn threads_blocked_by_their_own_cause(
    conn: &mut Conn,
    reasons: &[&str],
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
          WHERE t.state = 'blocked' AND t.blocked_reason = ANY($1) AND n.deleted_at IS NULL
          ORDER BY t.node_id LIMIT $2",
    )
    .bind(reasons)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Every live thread with pending input bound to a provider, including side threads, for policy rechecks.
pub async fn pending_threads_of_provider(
    conn: &mut Conn,
    provider: Id,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
        WHERE t.provider_id = $1 AND n.deleted_at IS NULL
          AND EXISTS (SELECT 1 FROM thread_inputs i WHERE i.thread_id = t.node_id AND i.state = 'pending')
        ORDER BY t.node_id").bind(provider).fetch_all(conn).await?)
}

/// The threads blocked for `reason` that are bound to `provider` and, when
/// the id is given, to `agent`, which a change of that provider may have
/// freed.
pub async fn blocked_threads_of_provider(
    conn: &mut Conn,
    provider: Id,
    agent: Option<Id>,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
          WHERE t.state = 'blocked' AND t.provider_id = $1 AND ($2::uuid IS NULL OR t.agent_id = $2)
            AND n.deleted_at IS NULL ORDER BY t.node_id",
    )
    .bind(provider)
    .bind(agent)
    .fetch_all(conn)
    .await?)
}

/// The blocked threads that run in `environment`.
pub async fn blocked_threads_of_environment(
    conn: &mut Conn,
    environment: Id,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
          WHERE t.state = 'blocked' AND t.environment_id = $1 AND n.deleted_at IS NULL ORDER BY t.node_id",
    )
    .bind(environment)
    .fetch_all(conn)
    .await?)
}

/// The blocked threads whose harness runs on `machine` (the machine of the
/// environment they run in, or of the provider they are bound to), which
/// coming online may have freed.
pub async fn blocked_threads_of_machine(conn: &mut Conn, machine: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.node_id FROM threads t
           JOIN nodes n ON n.id = t.node_id
           LEFT JOIN environments e ON e.id = t.environment_id
           LEFT JOIN providers p ON p.id = t.provider_id
          WHERE t.state = 'blocked' AND n.deleted_at IS NULL AND (e.machine_id = $1 OR p.machine_id = $1)
          ORDER BY t.node_id",
    )
    .bind(machine)
    .fetch_all(conn)
    .await?)
}

/// The threads with a turn under way whose harness runs on `machine`: what
/// going offline reaches.
pub async fn active_threads_of_machine(conn: &mut Conn, machine: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT t.node_id FROM threads t
           JOIN nodes n ON n.id = t.node_id
           JOIN turns u ON u.id = t.current_turn_id AND u.state IN ('starting', 'running')
           LEFT JOIN environments e ON e.id = t.environment_id
           LEFT JOIN providers p ON p.id = t.provider_id
          WHERE n.deleted_at IS NULL AND (
                  (t.arrangement = 'separated' AND p.machine_id = $1)
               OR (t.arrangement IS DISTINCT FROM 'separated' AND (e.machine_id = $1 OR p.machine_id = $1)))
          ORDER BY t.node_id",
    )
    .bind(machine)
    .fetch_all(conn)
    .await?)
}

/// The threads that are blocked as the machine of their harness is offline
/// and have a turn under way, which events resuming bring back.
pub async fn offline_threads_with_a_turn(conn: &mut Conn) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.node_id FROM threads t
          WHERE t.state = 'blocked' AND t.blocked_reason = 'agent_offline' AND t.current_turn_id IS NOT NULL
          ORDER BY t.node_id",
    )
    .fetch_all(conn)
    .await?)
}

// ---------------------------------------------------------------- segments

pub(crate) const SEGMENT_SELECT: &str = "SELECT id, thread_id, continuation_id, ordinal, agent_id, provider_id, environment_id, env_generation,
        arrangement, executor, tool_set, credential_path, native_session_ref, started_by, started_at, ended_at,
        end_reason, import_fidelity, import_report, seq_updated
   FROM segments";

/// One segment with required execution bindings, or optional historical bindings.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct SegmentRow<Binding = Id> {
    /// Its identifier.
    pub id: Id,
    /// The thread.
    pub thread_id: Id,
    /// Immutable suffix ownership; included prefixes reference this owner.
    pub continuation_id: Id,
    /// Its number in the thread.
    pub ordinal: i32,
    /// The agent profile.
    pub agent_id: Binding,
    /// The provider.
    pub provider_id: Binding,
    /// The environment.
    pub environment_id: Binding,
    /// Which unit of the environment it ran in.
    pub env_generation: i32,
    /// The arrangement type.
    pub arrangement: String,
    /// The executor that carried out its tool calls.
    pub executor: Option<String>,
    /// The tool set.
    pub tool_set: Option<String>,
    /// How its harness reached the model.
    pub credential_path: String,
    /// The harness's own name for the conversation.
    pub native_session_ref: Option<String>,
    /// Who started it.
    pub started_by: Option<Id>,
    /// When it began.
    pub started_at: DateTime<Utc>,
    /// When it ended.
    pub ended_at: Option<DateTime<Utc>>,
    /// Why it ended.
    pub end_reason: Option<String>,
    /// How well the history it took in was kept.
    pub import_fidelity: Option<String>,
    /// What was kept, converted and dropped.
    pub import_report: Option<Json>,
    /// The sequence at which it last changed.
    pub seq_updated: i64,
}

/// A historical segment can have no local agent, provider or environment.
pub type HistorySegmentRow = SegmentRow<Option<Id>>;

/// A segment to begin.
#[derive(Debug, Clone, PartialEq)]
pub struct NewSegment<'a> {
    /// Its identifier.
    pub id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The agent profile.
    pub agent_id: Id,
    /// The provider.
    pub provider_id: Id,
    /// The environment.
    pub environment_id: Id,
    /// Its generation now.
    pub env_generation: i32,
    /// The arrangement type.
    pub arrangement: &'a str,
    /// The executor.
    pub executor: Option<&'a str>,
    /// The tool set.
    pub tool_set: Option<&'a str>,
    /// The credential path.
    pub credential_path: &'a str,
    /// A session to take up: the harness's name for it.
    pub native_session_ref: Option<&'a str>,
    /// Who starts it.
    pub started_by: Option<Id>,
    /// When.
    pub started_at: DateTime<Utc>,
}

/// Begins the next segment of a thread: its number follows the last. The
/// thread's row must be locked.
pub async fn insert_segment(
    conn: &mut Conn,
    new: &NewSegment<'_>,
    seq: Seq,
) -> Result<SegmentRow, DbError> {
    insert_segment_in(conn, new, None, seq).await
}

/// Prepares a suffix segment without changing execution selection.
pub async fn insert_segment_in(
    conn: &mut Conn,
    new: &NewSegment<'_>,
    continuation: Option<Id>,
    seq: Seq,
) -> Result<SegmentRow, DbError> {
    let sql = "INSERT INTO segments (id, thread_id, ordinal, agent_id, provider_id, environment_id, env_generation,
                    arrangement, executor, tool_set, credential_path, native_session_ref, started_by, started_at, seq_updated,continuation_id)
               VALUES ($1, $2, (SELECT COALESCE(MAX(ordinal), 0) + 1 FROM segments WHERE thread_id = $2),
                       $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,COALESCE($15,(SELECT active_continuation_id FROM threads WHERE node_id=$2)))
               RETURNING id, thread_id, continuation_id, ordinal, agent_id, provider_id, environment_id, env_generation,
                    arrangement, executor, tool_set, credential_path, native_session_ref, started_by, started_at,
                    ended_at, end_reason, import_fidelity, import_report, seq_updated";
    Ok(sqlx::query_as(sql)
        .bind(new.id)
        .bind(new.thread_id)
        .bind(new.agent_id)
        .bind(new.provider_id)
        .bind(new.environment_id)
        .bind(new.env_generation)
        .bind(new.arrangement)
        .bind(new.executor)
        .bind(new.tool_set)
        .bind(new.credential_path)
        .bind(new.native_session_ref)
        .bind(new.started_by)
        .bind(new.started_at)
        .bind(seq)
        .bind(continuation)
        .fetch_one(conn)
        .await?)
}

/// The segment `id`.
pub async fn segment(conn: &mut Conn, id: Id) -> Result<Option<SegmentRow>, DbError> {
    let sql = format!(
        "{SEGMENT_SELECT} WHERE id = $1 AND agent_id IS NOT NULL AND provider_id IS NOT NULL AND environment_id IS NOT NULL"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// Every segment of a thread, in order.
pub async fn segments_of(conn: &mut Conn, thread: Id) -> Result<Vec<HistorySegmentRow>, DbError> {
    let sql = format!("{SEGMENT_SELECT} WHERE thread_id = $1 ORDER BY ordinal");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .fetch_all(conn)
        .await?)
}

/// A bounded page of a thread's segments, in their permanent ordinal order.
pub async fn segments_page(
    conn: &mut Conn,
    thread: Id,
    after: i32,
    limit: i64,
) -> Result<Vec<HistorySegmentRow>, DbError> {
    let sql =
        format!("{SEGMENT_SELECT} WHERE thread_id = $1 AND ordinal > $2 ORDER BY ordinal LIMIT $3");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(after)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// The segments of a thread changed after `since`.
pub async fn segments_since(
    conn: &mut Conn,
    thread: Id,
    since: Seq,
) -> Result<Vec<HistorySegmentRow>, DbError> {
    let sql =
        format!("{SEGMENT_SELECT} WHERE thread_id = $1 AND seq_updated > $2 ORDER BY ordinal");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(since)
        .fetch_all(conn)
        .await?)
}

/// Keeps what a segment's session was opened with, source by source (`instruction_sources`): Orch's own texts and the
/// files its harness reads, in their order. A session opened again replaces them.
pub async fn set_instruction_sources(
    conn: &mut Conn,
    segment: Id,
    sources: &Json,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query("UPDATE segments SET instruction_sources = $2, seq_updated = $3 WHERE id = $1")
        .bind(segment)
        .bind(sources)
        .bind(seq)
        .execute(conn)
        .await?;
    Ok(())
}

/// What the last session of a thread was opened with: its segment, when it was last opened, and its sources. None
/// where no session of the thread was opened yet.
pub async fn opened_instruction_sources(
    conn: &mut Conn,
    thread: Id,
) -> Result<Option<(Id, Option<DateTime<Utc>>, Json)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT s.id,
                GREATEST((SELECT max(m.sent_at) FROM model_inputs m
                  WHERE m.segment_id = s.id AND m.purpose = 'session' AND m.state = 'sent'),
                  (SELECT max((source->>'observed_at')::timestamptz) FROM jsonb_array_elements(s.instruction_sources) source)),
                s.instruction_sources
           FROM segments s
          WHERE s.thread_id = $1 AND s.native_session_ref IS NOT NULL
          ORDER BY s.ordinal DESC
          LIMIT 1",
    )
    .bind(thread)
    .fetch_optional(conn)
    .await?)
}

/// Records the harness's name for the conversation of a segment, where it
/// has none or has this one. The answer says whether this call set it.
pub async fn set_native_session_ref(
    conn: &mut Conn,
    segment: Id,
    native_ref: &str,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE segments SET native_session_ref = $2, seq_updated = $3
          WHERE id = $1 AND native_session_ref IS NULL",
    )
    .bind(segment)
    .bind(native_ref)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Ends a segment that is still going. The answer says whether this call
/// ended it.
pub async fn end_segment(
    conn: &mut Conn,
    segment: Id,
    reason: &str,
    at: DateTime<Utc>,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE segments SET ended_at = $3, end_reason = $2, seq_updated = $4 WHERE id = $1 AND ended_at IS NULL",
    )
    .bind(segment)
    .bind(reason)
    .bind(at)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

// ---------------------------------------------------------------- inputs

pub(crate) const INPUT_SELECT: &str = "SELECT id, thread_id, continuation_id, author_id, source, content, native_action, delivery, state, state_detail, limit_bypass,
        position::text AS position, turn_id, client_id, created_at, delivered_at, cancelled_by, seq_updated,
        attachment_issues, queue_held, run_as, plan, submission_authority, run_as_authority, approval_id
   FROM thread_inputs";

/// What a person said to an agent, queued for it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct InputRow {
    /// Its identifier.
    pub id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The queue and suffix to which acceptance bound this input permanently.
    pub continuation_id: Id,
    /// Who said it; none when nobody did.
    pub author_id: Option<Id>,
    /// `user`, `announcement` or `system`.
    pub source: String,
    /// The parts of what was said.
    pub content: Json,
    /// Explicit native selection, with arguments kept only in the conversation.
    #[sqlx(default)]
    pub native_action: Option<Json>,
    /// `queue`, `steer` or `interrupt`.
    pub delivery: String,
    /// Where it stands.
    pub state: String,
    /// More about the state.
    pub state_detail: Option<String>,
    /// Who let it past which limits.
    pub limit_bypass: Option<Json>,
    /// Its place in the queue, as a decimal string.
    pub position: String,
    /// The turn it started or joined.
    pub turn_id: Option<Id>,
    /// What its sender chose to make sending it again safe.
    pub client_id: Option<String>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it was delivered.
    pub delivered_at: Option<DateTime<Utc>>,
    /// Who cancelled it.
    pub cancelled_by: Option<Id>,
    /// The sequence at which it last changed.
    pub seq_updated: i64,
    /// The files that hold it before delivery (`AttachmentIssue[]`), empty when it is ready.
    pub attachment_issues: Json,
    /// Whether a person's Stop holds it.
    pub queue_held: bool,
    /// Who answers for its turn when that is not its author: somebody who may use the thread's model and ran
    /// it on their own access, or the thread's first owner where the place lets such inputs run on theirs.
    #[sqlx(default)]
    pub run_as: Option<Id>,
    /// Whether it is worked in plan mode: the agent proposes a plan and changes nothing until somebody proceeds.
    #[sqlx(default)]
    pub plan: bool,
    /// The credential boundary at submission; absent on older/imported records, never an implied session.
    #[sqlx(default)]
    pub submission_authority: Option<Json>,
    /// The credential boundary of the person who explicitly ran it on their access.
    #[sqlx(default)]
    pub run_as_authority: Option<Json>,
    /// The canonical approval for this model-use wait, retained after it is decided.
    #[sqlx(default)]
    pub approval_id: Option<Id>,
}

impl InputRow {
    /// Nobody authored this work; a removed author reference does not make an authored input anonymous.
    #[must_use]
    pub fn is_authorless(&self) -> bool {
        self.author_id.is_none() && self.submission_authority.is_none() && self.source != "user"
    }

    /// Whether the input is held: it keeps its place and pre-flight passes
    /// it over. A Stop's hold and a hold of its files wait for a person; the
    /// others for room in a cap, or for the thread's owner.
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.state == input_state::PENDING
            && (self.queue_held
                || self
                    .state_detail
                    .as_deref()
                    .is_some_and(|detail| detail.starts_with("native_action."))
                || matches!(
                    self.state_detail.as_deref(),
                    Some(
                        HELD_CAP_REACHED
                            | HELD_OWNER_LACKS_ACCESS
                            | HELD_ATTACHMENTS
                            | HELD_SECOND_STEP
                            | HELD_MODEL_USE
                    )
                ))
    }

    /// Who answers for its turn: whoever it was run on, otherwise its author. Usage, caps and the provider's
    /// entry are this person's; the author stays the author.
    #[must_use]
    pub fn answers(&self) -> Option<Id> {
        self.run_as.or(self.author_id)
    }

    /// Whether what holds it waits for a person (a Stop, its files, or somebody who may use the model), not for
    /// the thread or its limits.
    #[must_use]
    pub fn waits_for_a_person(&self) -> bool {
        self.state == input_state::PENDING
            && (self.queue_held
                || matches!(
                    self.state_detail.as_deref(),
                    Some(HELD_ATTACHMENTS | HELD_MODEL_USE)
                )
                || self
                    .state_detail
                    .as_deref()
                    .is_some_and(|detail| detail.starts_with("native_action.")))
    }
}

/// The `state_detail` of an input held because its author's own cap has no
/// room.
pub const HELD_CAP_REACHED: &str = "cap_reached";
/// The `state_detail` of an input without an author, held while the
/// thread's first owner may not use its provider.
pub const HELD_OWNER_LACKS_ACCESS: &str = "owner_lacks_access";
/// The `state_detail` of an input whose files cannot reach the agent: it keeps its place with its
/// `attachment_issues` until its author tries again, removes them, or the model changes.
pub const HELD_ATTACHMENTS: &str = "attachments";
/// The `state_detail` of an input held because the team org it lies in requires a second step of its
/// author, who has none set up: it keeps its place, the next input goes ahead, and it is released
/// when the setup is there. It is never cancelled for this.
pub const HELD_SECOND_STEP: &str = "second_step_required";

/// The `state_detail` of an input whose author may write in the thread but may not use its model: it keeps its
/// place until somebody who may use the model runs it on their own access, or the author is given the model.
/// It is never cancelled for this.
pub const HELD_MODEL_USE: &str = "model_use_required";

/// Names who answers for an input's turn, and releases it if it waited for that.
pub async fn set_input_run_as(
    conn: &mut Conn,
    id: Id,
    person: Option<Id>,
    authority: Option<&Json>,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE thread_inputs
            SET run_as = $2, run_as_authority = CASE WHEN $2::uuid IS NULL THEN NULL ELSE $5 END,
                state_detail = CASE WHEN state_detail = $4 THEN NULL ELSE state_detail END,
                seq_updated = $3
          WHERE id = $1 AND state = 'pending'",
    )
    .bind(id)
    .bind(person)
    .bind(seq)
    .bind(HELD_MODEL_USE)
    .bind(authority)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 1 {
        touch_input_queue(conn, id, seq).await?;
    }
    Ok(done.rows_affected() == 1)
}

/// Saves a restrictive authored edit and withdraws approval of the earlier words or native selection.
pub async fn set_input_submission_authority(
    conn: &mut Conn,
    id: Id,
    authority: Option<&Json>,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query("UPDATE thread_inputs SET submission_authority=$2,run_as=NULL,run_as_authority=NULL,seq_updated=$3
                  WHERE id=$1 AND state='pending'")
        .bind(id).bind(authority).bind(seq).execute(&mut *conn).await?;
    touch_input_queue(conn, id, seq).await?;
    Ok(())
}

/// The threads where inputs wait held for a second step: of `author` alone where one is named.
pub async fn threads_held_for_second_step(
    conn: &mut Conn,
    author: Option<Id>,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT thread_id FROM thread_inputs
          WHERE state = 'pending' AND state_detail = $1
            AND ($2::uuid IS NULL OR author_id = $2)",
    )
    .bind(HELD_SECOND_STEP)
    .bind(author)
    .fetch_all(conn)
    .await?)
}

/// Where a new input goes in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// After every input that waits.
    End,
    /// Before every input that waits.
    Front,
}

/// An input to queue.
#[derive(Debug, Clone, PartialEq)]
pub struct NewInput<'a> {
    /// Its identifier.
    pub id: Id,
    /// The thread.
    pub thread_id: Id,
    /// Who says it.
    pub author_id: Option<Id>,
    /// The accepted caller's credential boundary; none only for authorless or historical inputs.
    pub submission_authority: Option<&'a Json>,
    /// The explicit execution actor inherited from an approved origin, when there is one.
    pub run_as: Option<Id>,
    /// That actor's retained credential boundary.
    pub run_as_authority: Option<&'a Json>,
    /// `user`, `announcement` or `system`.
    pub source: &'a str,
    /// The parts of what is said.
    pub content: &'a Json,
    /// `queue`, `steer` or `interrupt`.
    pub delivery: &'a str,
    /// Its state in an open queue: `pending`, or `delivering` for a steer that
    /// goes straight into the turn under way. A Stop hold always keeps it pending.
    pub state: &'a str,
    /// The turn it joins, for such a steer.
    pub turn_id: Option<Id>,
    /// What lets its sender send it again.
    pub client_id: Option<&'a str>,
    /// Who let it past which limits.
    pub limit_bypass: Option<&'a Json>,
    /// Where in an open queue. An active Stop hold appends every arrival instead.
    pub place: Place,
    /// When it is made.
    pub at: DateTime<Utc>,
    /// Whether it is worked in plan mode.
    pub plan: bool,
}

/// Queues an input. The answer is the input, or none where the author has
/// used this `client_id` in this thread before: the input they sent then
/// stands, and is read with [`input_by_client`]. The thread's row must be
/// locked. A live Stop hold is inherited atomically: every arrival stays pending,
/// has no delivery turn, and joins behind the existing queue until Resume.
pub async fn insert_input(
    conn: &mut Conn,
    new: &NewInput<'_>,
    seq: Seq,
) -> Result<Option<InputRow>, DbError> {
    insert_input_in(conn, new, None, seq).await
}

/// Explicit preparation binds words to their new suffix without making it runnable.
pub async fn insert_input_in(
    conn: &mut Conn,
    new: &NewInput<'_>,
    continuation: Option<Id>,
    seq: Seq,
) -> Result<Option<InputRow>, DbError> {
    let end_position = "(SELECT COALESCE(MAX(position), 0) + 1 FROM thread_inputs WHERE thread_id = $2 AND continuation_id=c.id AND (state = 'pending' OR (state = 'delivering' AND delivery = 'steer')))";
    let position = match new.place {
        Place::End => end_position,
        Place::Front => {
            "(SELECT COALESCE(MIN(position), 1) - 1 FROM thread_inputs WHERE thread_id = $2 AND continuation_id=c.id AND (state = 'pending' OR (state = 'delivering' AND delivery = 'steer')))"
        }
    };
    let sql = format!(
        "INSERT INTO thread_inputs (id, thread_id, author_id, source, content, delivery, state, limit_bypass, position,
                                    turn_id, client_id, created_at, seq_created, seq_updated, queue_held,continuation_id, plan, submission_authority, run_as, run_as_authority)
         SELECT $1, $2, $3, $4, $5, $6, CASE WHEN c.queue_held THEN 'pending' ELSE $7::text END, $8,
                CASE WHEN c.queue_held THEN {end_position} ELSE {position} END,
                CASE WHEN c.queue_held THEN NULL ELSE $9::uuid END, $10, $11, $12, $12, c.queue_held,c.id, $14, $15, $16, $17
           FROM threads t JOIN thread_continuations c ON c.thread_id=t.node_id AND c.id=COALESCE($13,t.active_continuation_id) WHERE t.node_id = $2
         ON CONFLICT (thread_id, author_id, client_id) DO NOTHING
         RETURNING id"
    );
    let inserted: Option<Id> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(new.id)
        .bind(new.thread_id)
        .bind(new.author_id)
        .bind(new.source)
        .bind(new.content)
        .bind(new.delivery)
        .bind(new.state)
        .bind(new.limit_bypass)
        .bind(new.turn_id)
        .bind(new.client_id)
        .bind(new.at)
        .bind(seq)
        .bind(continuation)
        .bind(new.plan)
        .bind(new.submission_authority)
        .bind(new.run_as)
        .bind(new.run_as_authority)
        .fetch_optional(&mut *conn)
        .await?;
    match inserted {
        Some(id) => {
            touch_input_queue(conn, id, seq).await?;
            input(conn, id).await
        }
        None => Ok(None),
    }
}

/// The input `id`.
pub async fn input(conn: &mut Conn, id: Id) -> Result<Option<InputRow>, DbError> {
    let sql = format!("{INPUT_SELECT} WHERE id = $1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// The input an author sent with a `client_id`.
pub async fn input_by_client(
    conn: &mut Conn,
    thread: Id,
    author: Option<Id>,
    client_id: &str,
) -> Result<Option<InputRow>, DbError> {
    let sql = format!(
        "{INPUT_SELECT} WHERE thread_id = $1 AND author_id IS NOT DISTINCT FROM $2 AND client_id = $3"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(author)
        .bind(client_id)
        .fetch_optional(conn)
        .await?)
}

/// The inputs of a thread that wait, in delivery order.
pub async fn pending_inputs(conn: &mut Conn, thread: Id) -> Result<Vec<InputRow>, DbError> {
    let sql = format!(
        "{INPUT_SELECT} WHERE thread_id = $1 AND continuation_id=(SELECT active_continuation_id FROM threads WHERE node_id=$1) AND state = 'pending' ORDER BY position, created_at, id"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .fetch_all(conn)
        .await?)
}

/// Queue ordering retains a steer's reserved place without loading the messages' content.
pub async fn queue_position_ids(conn: &mut Conn, thread: Id) -> Result<Vec<(Id, bool)>, DbError> {
    Ok(sqlx::query_as("SELECT id,state='pending' FROM thread_inputs WHERE thread_id=$1
        AND continuation_id=(SELECT active_continuation_id FROM threads WHERE node_id=$1)
        AND (state='pending' OR (state='delivering' AND delivery='steer')) ORDER BY position,created_at,id")
        .bind(thread).fetch_all(conn).await?)
}

/// How many inputs of a thread wait.
pub async fn count_pending_inputs(conn: &mut Conn, thread: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM thread_inputs WHERE thread_id = $1 AND continuation_id=(SELECT active_continuation_id FROM threads WHERE node_id=$1) AND state = 'pending'",
    )
    .bind(thread)
    .fetch_one(conn)
    .await?)
}

/// The inputs of a thread that are being handed to a running turn (a steer
/// on its way), oldest first.
pub async fn delivering_inputs(conn: &mut Conn, thread: Id) -> Result<Vec<InputRow>, DbError> {
    let sql = format!(
        "{INPUT_SELECT} WHERE thread_id = $1 AND continuation_id=(SELECT active_continuation_id FROM threads WHERE node_id=$1) AND state = 'delivering' ORDER BY created_at, id"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .fetch_all(conn)
        .await?)
}

/// Whether a dispatched steer still needs its durable command result.
pub async fn unsettled_steer(conn: &mut Conn, thread: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM thread_inputs i JOIN threads t ON t.node_id=i.thread_id
          WHERE i.thread_id=$1 AND i.continuation_id=t.active_continuation_id
            AND i.state='delivering' AND i.delivery='steer' AND i.state_detail='steering'
            AND i.turn_id IS NOT NULL)",
    )
    .bind(thread)
    .fetch_one(conn)
    .await?)
}

/// The inputs, of every thread, that are being handed to a turn that is
/// running, which a restart may have interrupted.
pub async fn delivering_inputs_of_running_turns(
    conn: &mut Conn,
    limit: i64,
) -> Result<Vec<InputRow>, DbError> {
    let sql = format!(
        "{INPUT_SELECT} WHERE state = 'delivering' AND turn_id IS NOT NULL AND delivery = 'steer'
           ORDER BY created_at, id LIMIT $1"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// What an input changes to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InputChange<'a> {
    /// The new state.
    pub state: &'a str,
    /// The detail of the state; none clears it.
    pub detail: Option<&'a str>,
    /// The turn it started or joined; kept when none.
    pub turn_id: Option<Id>,
    /// When it was delivered.
    pub delivered_at: Option<DateTime<Utc>>,
    /// Who cancelled it.
    pub cancelled_by: Option<Id>,
}

/// Advances the queue revision and the thread state seen by sync/live readers.
/// The thread row is already locked by the input mutation's transaction.
pub async fn touch_queue(conn: &mut Conn, thread: Id, seq: Seq) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE threads SET queue_revision = queue_revision + 1, seq_state = GREATEST(seq_state, $2)
          WHERE node_id = $1",
    )
    .bind(thread)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(())
}

/// The same revision change when an input's identifier is already at hand.
async fn touch_input_queue(conn: &mut Conn, input: Id, seq: Seq) -> Result<(), DbError> {
    sqlx::query("UPDATE thread_continuations c SET queue_revision=c.queue_revision+1 FROM thread_inputs i,threads t
        WHERE i.id=$1 AND c.id=i.continuation_id AND t.node_id=i.thread_id AND c.id<>t.active_continuation_id")
        .bind(input).execute(&mut *conn).await?;
    sqlx::query(
        "UPDATE threads SET queue_revision = queue_revision + 1, seq_state = GREATEST(seq_state, $2)
          WHERE node_id = (SELECT thread_id FROM thread_inputs WHERE id = $1 AND continuation_id=threads.active_continuation_id)",
    )
    .bind(input)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(())
}

/// Moves an input from one of the states `from` to another, and the answer
/// says whether this call did. `seq` stamps the change.
pub async fn change_input(
    conn: &mut Conn,
    id: Id,
    from: &[&str],
    to: &InputChange<'_>,
    seq: Seq,
) -> Result<bool, DbError> {
    let from: Vec<String> = from.iter().map(|state| (*state).to_owned()).collect();
    let done = sqlx::query(
        "UPDATE thread_inputs
            SET state = $3, state_detail = $4, turn_id = COALESCE($5, turn_id),
                delivered_at = COALESCE($6, delivered_at), cancelled_by = COALESCE($7, cancelled_by), seq_updated = $8
          WHERE id = $1 AND state = ANY($2)",
    )
    .bind(id)
    .bind(from)
    .bind(to.state)
    .bind(to.detail)
    .bind(to.turn_id)
    .bind(to.delivered_at)
    .bind(to.cancelled_by)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 1 {
        touch_input_queue(conn, id, seq).await?;
    }
    Ok(done.rows_affected() == 1)
}

/// Returns an input to the queue, before every input that waits (a steer
/// whose turn ended first), clearing its turn. The answer says whether this
/// call did.
pub async fn return_input_to_front(
    conn: &mut Conn,
    id: Id,
    from: &[&str],
    seq: Seq,
) -> Result<bool, DbError> {
    let from: Vec<String> = from.iter().map(|state| (*state).to_owned()).collect();
    let done = sqlx::query(
        "UPDATE thread_inputs i
            SET state = 'pending', state_detail = NULL, turn_id = NULL, seq_updated = $3,
                position = (SELECT COALESCE(MIN(p.position), 1) - 1 FROM thread_inputs p
                             WHERE p.thread_id = i.thread_id AND p.state = 'pending')
          WHERE i.id = $1 AND i.state = ANY($2)",
    )
    .bind(id)
    .bind(from)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 1 {
        touch_input_queue(conn, id, seq).await?;
    }
    Ok(done.rows_affected() == 1)
}

/// Returns an input to the queue where it stood (a turn that could not
/// start), clearing its turn. The answer says whether this call did.
pub async fn return_input_in_place(
    conn: &mut Conn,
    id: Id,
    from: &[&str],
    seq: Seq,
) -> Result<bool, DbError> {
    let from: Vec<String> = from.iter().map(|state| (*state).to_owned()).collect();
    let done = sqlx::query(
        "UPDATE thread_inputs SET state = 'pending', state_detail = NULL, turn_id = NULL, seq_updated = $3
          WHERE id = $1 AND state = ANY($2)",
    )
    .bind(id)
    .bind(from)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 1 {
        touch_input_queue(conn, id, seq).await?;
    }
    Ok(done.rows_affected() == 1)
}

/// Marks a waiting input as held for `detail`, or frees it (none). The
/// answer says whether the mark changed.
pub async fn hold_input(
    conn: &mut Conn,
    id: Id,
    detail: Option<&str>,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE thread_inputs SET state_detail = $2, seq_updated = $3
          WHERE id = $1 AND state = 'pending' AND state_detail IS DISTINCT FROM $2",
    )
    .bind(id)
    .bind(detail)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 1 {
        touch_input_queue(conn, id, seq).await?;
    }
    Ok(done.rows_affected() == 1)
}

/// Holds, for a person's Stop, the inputs of a thread that wait now: each is marked `queue_held`, and the
/// thread records who stopped and when, with a new revision of its queue. The thread's row must be locked.
/// The answer is how many inputs this held and the queue's revision after; where nothing waits nothing
/// changes, and no empty hold is made.
pub async fn hold_queue(
    conn: &mut Conn,
    thread: Id,
    by: Id,
    at: DateTime<Utc>,
    seq: Seq,
) -> Result<(i64, i64), DbError> {
    let held = sqlx::query(
        "UPDATE thread_inputs SET queue_held = true, seq_updated = $2
          WHERE thread_id = $1 AND continuation_id=(SELECT active_continuation_id FROM threads WHERE node_id=$1) AND state = 'pending' AND NOT queue_held",
    )
    .bind(thread)
    .bind(seq)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    let held = i64::try_from(held).unwrap_or(i64::MAX);
    if held == 0 {
        let revision: i64 =
            sqlx::query_scalar("SELECT queue_revision FROM threads WHERE node_id = $1")
                .bind(thread)
                .fetch_one(&mut *conn)
                .await?;
        return Ok((0, revision));
    }
    let revision: i64 = sqlx::query_scalar(
        "UPDATE threads SET queue_held = true, queue_held_by = $2, queue_held_at = $3,
                            queue_revision = queue_revision + 1, seq_state = $4
          WHERE node_id = $1
         RETURNING queue_revision",
    )
    .bind(thread)
    .bind(by)
    .bind(at)
    .bind(seq)
    .fetch_one(conn)
    .await?;
    Ok((held, revision))
}

/// Records how a side thread reads without acting, as its harness verified it when the side thread was made.
pub async fn set_side_reading(conn: &mut Conn, thread: Id, mode: &str) -> Result<(), DbError> {
    sqlx::query("UPDATE threads SET side_reading = $2 WHERE node_id = $1")
        .bind(thread)
        .bind(mode)
        .execute(conn)
        .await?;
    Ok(())
}

/// Resumes what a person's Stop held: the inputs that wait keep their places and are eligible again, every
/// other hold of theirs stays, and the thread's queue is no longer held, with a new revision. The thread's row
/// must be locked. The answer is how many inputs this let go and the revision after.
pub async fn resume_queue(conn: &mut Conn, thread: Id, seq: Seq) -> Result<(i64, i64), DbError> {
    let freed = sqlx::query(
        "UPDATE thread_inputs SET queue_held = false, seq_updated = $2
          WHERE thread_id = $1 AND continuation_id=(SELECT active_continuation_id FROM threads WHERE node_id=$1) AND state IN ('pending','delivering') AND queue_held",
    )
    .bind(thread)
    .bind(seq)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    let revision: i64 = sqlx::query_scalar(
        "UPDATE threads SET queue_held = false, queue_held_by = NULL, queue_held_at = NULL,
                            queue_revision = queue_revision + 1, seq_state = $2
          WHERE node_id = $1
         RETURNING queue_revision",
    )
    .bind(thread)
    .bind(seq)
    .fetch_one(conn)
    .await?;
    Ok((i64::try_from(freed).unwrap_or(i64::MAX), revision))
}

/// How many inputs of a thread that wait a person's Stop holds.
pub async fn count_queue_held(conn: &mut Conn, thread: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM thread_inputs WHERE thread_id = $1 AND continuation_id=(SELECT active_continuation_id FROM threads WHERE node_id=$1) AND state = 'pending' AND queue_held",
    )
    .bind(thread)
    .fetch_one(conn)
    .await?)
}

/// Ends a thread's queue hold when no input it held still waits (the last was removed or failed), with a new
/// revision. The thread's row must be locked. The answer says whether this ended it.
pub async fn clear_empty_queue_hold(
    conn: &mut Conn,
    thread: Id,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE threads SET queue_held = false, queue_held_by = NULL, queue_held_at = NULL,
                            queue_revision = queue_revision + 1, seq_state = $2
          WHERE node_id = $1 AND queue_held
            AND NOT EXISTS (SELECT 1 FROM thread_inputs
                             WHERE thread_id = $1 AND continuation_id=threads.active_continuation_id
                               AND state = 'pending' AND queue_held)",
    )
    .bind(thread)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Removing an inactive queue's last held input clears only that empty queue, never the selected Stop.
pub async fn clear_empty_inactive_hold(conn: &mut Conn, input: Id) -> Result<bool, DbError> {
    let done=sqlx::query("UPDATE thread_continuations c SET queue_held=false,queue_held_by=NULL,queue_held_at=NULL,
      queue_revision=c.queue_revision+1 FROM thread_inputs i,threads t
      WHERE i.id=$1 AND c.id=i.continuation_id AND t.node_id=i.thread_id AND c.id<>t.active_continuation_id AND c.queue_held
        AND NOT EXISTS(SELECT 1 FROM thread_inputs pending WHERE pending.continuation_id=c.id
          AND pending.state='pending' AND pending.queue_held)")
        .bind(input).execute(conn).await?;
    Ok(done.rows_affected() == 1)
}

/// Holds an input that waits because its files cannot reach the agent: it keeps its place, with each file's
/// issue, until its author tries again, removes them, or the model changes. The answer says whether this held it.
pub async fn hold_for_attachments(
    conn: &mut Conn,
    id: Id,
    issues: &Json,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE thread_inputs SET state = 'pending', state_detail = 'attachments', attachment_issues = $2,
                                  turn_id = NULL, seq_updated = $3
          WHERE id = $1 AND state IN ('pending', 'delivering')",
    )
    .bind(id)
    .bind(issues)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 1 {
        touch_input_queue(conn, id, seq).await?;
    }
    Ok(done.rows_affected() == 1)
}

/// Lets an input that its files held be looked at again: its issues are cleared and it waits as it did. The
/// answer says whether it was held so.
pub async fn release_attachment_hold(conn: &mut Conn, id: Id, seq: Seq) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE thread_inputs SET state_detail = NULL, attachment_issues = '[]'::jsonb, seq_updated = $2
          WHERE id = $1 AND state = 'pending' AND state_detail = 'attachments'",
    )
    .bind(id)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 1 {
        touch_input_queue(conn, id, seq).await?;
    }
    Ok(done.rows_affected() == 1)
}

/// Replaces the content of an input that still waits. The answer says
/// whether this call did.
pub async fn update_input_content(
    conn: &mut Conn,
    id: Id,
    content: &Json,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE thread_inputs SET content = $2, seq_updated = $3 WHERE id = $1 AND state = 'pending'",
    )
    .bind(id)
    .bind(content)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 1 {
        touch_input_queue(conn, id, seq).await?;
    }
    Ok(done.rows_affected() == 1)
}

/// Changes a pending input's delivery without clearing any hold or changing its author.
/// Its thread is locked, and any state/position change is made in the same transaction.
pub async fn set_input_delivery(
    conn: &mut Conn,
    id: Id,
    delivery: &str,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE thread_inputs SET delivery = $2, seq_updated = $3 WHERE id = $1 AND state = 'pending'",
    )
    .bind(id)
    .bind(delivery)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 1 {
        touch_input_queue(conn, id, seq).await?;
    }
    Ok(done.rows_affected() == 1)
}

/// Replaces the record of who let an input past which limits.
pub async fn set_input_bypass(
    conn: &mut Conn,
    id: Id,
    bypass: &Json,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE thread_inputs SET limit_bypass = $2, seq_updated = $3 WHERE id = $1 AND state = 'pending'",
    )
    .bind(id)
    .bind(bypass)
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 1 {
        touch_input_queue(conn, id, seq).await?;
    }
    Ok(done.rows_affected() == 1)
}

/// Gives the inputs that wait the places `1`, `2`, … in the order of `ids`
/// (all of the pending ones, in the order they are to be in), writing only
/// those whose place changes. The answer says how many changed.
pub async fn place_pending_inputs(
    conn: &mut Conn,
    thread: Id,
    ids: &[Id],
    seq: Seq,
) -> Result<u64, DbError> {
    let mut changed = 0;
    for (index, id) in ids.iter().enumerate() {
        let place = i64::try_from(index).map_or(i64::MAX, |index| index + 1);
        let done = sqlx::query(
            "UPDATE thread_inputs SET position = $3::int8, seq_updated = $4
              WHERE id = $2 AND thread_id = $1 AND (state = 'pending' OR (state = 'delivering' AND delivery = 'steer')) AND position <> $3::int8",
        )
        .bind(thread)
        .bind(id)
        .bind(place)
        .bind(seq)
        .execute(&mut *conn)
        .await?;
        changed += done.rows_affected();
    }
    if changed > 0 {
        touch_queue(conn, thread, seq).await?;
    }
    Ok(changed)
}

/// The inputs of a thread changed after `since`.
pub async fn inputs_since(
    conn: &mut Conn,
    thread: Id,
    since: Seq,
) -> Result<Vec<InputRow>, DbError> {
    let sql = format!(
        "{INPUT_SELECT} WHERE thread_id = $1 AND seq_updated > $2 ORDER BY seq_updated, created_at, id"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(since)
        .fetch_all(conn)
        .await?)
}

/// The inputs of a thread that are pending, delivering or failed: what a
/// first load of the thread shows.
pub async fn open_inputs(conn: &mut Conn, thread: Id) -> Result<Vec<InputRow>, DbError> {
    let sql = format!(
        "{INPUT_SELECT} WHERE thread_id = $1 AND state IN ('pending', 'delivering', 'failed')
           ORDER BY position, created_at, id"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .fetch_all(conn)
        .await?)
}

/// How many distinct people have had an input delivered in the thread, counted up to two: whether anybody has, and
/// whether two people have, is all that is asked of it. The first and the last of them by identifier are two reads
/// of an index, where counting them all read every input the thread ever had.
pub async fn count_delivering_authors(conn: &mut Conn, thread: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT (CASE WHEN first.author_id IS NULL THEN 0 WHEN first.author_id = last.author_id THEN 1 ELSE 2 END)::bigint
           FROM (SELECT (SELECT author_id FROM thread_inputs
                          WHERE thread_id = $1 AND author_id IS NOT NULL AND state IN ('delivering', 'delivered')
                          ORDER BY author_id LIMIT 1) AS author_id) first,
                (SELECT (SELECT author_id FROM thread_inputs
                          WHERE thread_id = $1 AND author_id IS NOT NULL AND state IN ('delivering', 'delivered')
                          ORDER BY author_id DESC LIMIT 1) AS author_id) last",
    )
    .bind(thread)
    .fetch_one(conn)
    .await?)
}

// ---------------------------------------------------------------- turns

pub(crate) const TURN_SELECT: &str = "SELECT id, thread_id, segment_id, ordinal, state, input_id, model, effort, params, lease_id,
        before_snapshot_id, end_manifest_blob, files_end_manifest,
        native_turn_ref, error, usage, usage_detail, context, limit_bypass, phase, phase_at, interrupted_by, command_cleanup,
        waiting_since, waited_ms, started_at, completed_at, created_at, seq_updated, plan
   FROM turns";

/// One turn of the agent: its answer to one input.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct TurnRow {
    /// Its identifier.
    pub id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The segment.
    pub segment_id: Id,
    /// Its number in the thread.
    pub ordinal: i32,
    /// How far it has come.
    pub state: String,
    /// The input it answers.
    pub input_id: Id,
    /// Recorded before-state, including unavailable metadata.
    pub before_snapshot_id: Option<Id>,
    /// The turn's verified end manifest blob, or unknown coverage.
    pub end_manifest_blob: Option<Vec<u8>>,
    /// Included end hashes/types/executable bits, or unknown watcher coverage.
    pub files_end_manifest: Option<Json>,
    /// The model it ran with.
    pub model: String,
    /// The effort it ran with.
    pub effort: Option<String>,
    /// The parameters it ran with.
    pub params: Json,
    /// The lease it ran under.
    pub lease_id: Option<Id>,
    /// The harness's name for it.
    pub native_turn_ref: Option<String>,
    /// What ended it, if it failed.
    pub error: Option<Json>,
    /// What it used.
    pub usage: Option<Json>,
    /// What its harness reported of its use beyond the totals (`TurnUsageDetail`).
    #[sqlx(default)]
    pub usage_detail: Option<Json>,
    /// What filled the context at its end, by part (`ContextFill`).
    #[sqlx(default)]
    pub context: Option<Json>,
    /// What it was let past.
    pub limit_bypass: Option<Json>,
    /// What it is doing now.
    pub phase: Option<String>,
    /// Since when.
    pub phase_at: Option<DateTime<Utc>>,
    /// The beginning of the current human wait, if any.
    pub waiting_since: Option<DateTime<Utc>>,
    /// Completed human waits, with overlapping requests counted once.
    pub waited_ms: i64,
    /// Who stopped it.
    pub interrupted_by: Option<Id>,
    /// Actual stopped-turn command outcomes, including a native report after its completion.
    #[sqlx(default)]
    pub command_cleanup: Option<Json>,
    /// When it began.
    pub started_at: Option<DateTime<Utc>>,
    /// When it ended.
    pub completed_at: Option<DateTime<Utc>>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// The sequence at which it last changed.
    pub seq_updated: i64,
    /// Whether it plans: the agent proposes a plan and changes nothing until somebody proceeds.
    #[sqlx(default)]
    pub plan: bool,
}

impl TurnRow {
    /// Whether the turn is under way: it can still change.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state == turn_state::STARTING || self.state == turn_state::RUNNING
    }
}

/// A turn to begin.
#[derive(Debug, Clone, PartialEq)]
pub struct NewTurn<'a> {
    /// Its identifier.
    pub id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The segment.
    pub segment_id: Id,
    /// The input it answers.
    pub input_id: Id,
    /// The model.
    pub model: &'a str,
    /// The effort.
    pub effort: Option<&'a str>,
    /// The parameters.
    pub params: &'a Json,
    /// The lease.
    pub lease_id: Option<Id>,
    /// What it was let past.
    pub limit_bypass: Option<&'a Json>,
    /// When.
    pub at: DateTime<Utc>,
}

/// Begins the next turn of a thread, `starting`: its number follows the
/// last. The thread's row must be locked.
pub async fn insert_turn(conn: &mut Conn, new: &NewTurn<'_>, seq: Seq) -> Result<TurnRow, DbError> {
    let sql = "INSERT INTO turns (id, thread_id, segment_id, ordinal, state, input_id, model, effort, params, lease_id,
                    limit_bypass, created_at, seq_updated, audit_for_user_id, plan)
               VALUES ($1, $2, $3, (SELECT COALESCE(MAX(ordinal), 0) + 1 FROM turns WHERE thread_id = $2), 'starting',
                       $4, $5, $6, $7, $8, $9, $10, $11,
                       COALESCE((SELECT COALESCE(run_as, author_id) FROM thread_inputs WHERE id=$4), (SELECT owner_id FROM nodes WHERE id=$2)),
                       COALESCE((SELECT plan FROM thread_inputs WHERE id=$4), false))
               RETURNING id, thread_id, segment_id, ordinal, state, input_id, model, effort, params, lease_id,
                    before_snapshot_id, end_manifest_blob, files_end_manifest,
                    native_turn_ref, error, usage, limit_bypass, phase, phase_at, interrupted_by, command_cleanup, started_at,
                    completed_at, created_at, seq_updated, waiting_since, waited_ms, plan";
    Ok(sqlx::query_as(sql)
        .bind(new.id)
        .bind(new.thread_id)
        .bind(new.segment_id)
        .bind(new.input_id)
        .bind(new.model)
        .bind(new.effort)
        .bind(new.params)
        .bind(new.lease_id)
        .bind(new.limit_bypass)
        .bind(new.at)
        .bind(seq)
        .fetch_one(conn)
        .await?)
}

/// The turn `id`.
pub async fn turn(conn: &mut Conn, id: Id) -> Result<Option<TurnRow>, DbError> {
    let sql = format!("{TURN_SELECT} WHERE id = $1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// The first timeline item of one numbered turn, including its originating input when present.
pub async fn first_item_of_turn(
    conn: &mut Conn,
    thread: Id,
    ordinal: i64,
) -> Result<Option<(Id, Seq)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT i.id, i.seq_created FROM turns t JOIN items i ON i.thread_id = t.thread_id
        AND (i.turn_id = t.id OR i.native_ref = 'user:' || t.input_id::text)
        WHERE t.thread_id = $1 AND t.ordinal = $2 ORDER BY i.seq_created, i.id LIMIT 1",
    )
    .bind(thread)
    .bind(ordinal)
    .fetch_optional(conn)
    .await?)
}

/// The latest turn of a thread, whatever its state.
pub async fn latest_turn(conn: &mut Conn, thread: Id) -> Result<Option<TurnRow>, DbError> {
    let sql = format!("{TURN_SELECT} WHERE thread_id = $1 ORDER BY ordinal DESC LIMIT 1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .fetch_optional(conn)
        .await?)
}

/// The newest turn owned by the thread's active history, without taking a newer turn of another history.
pub async fn latest_active_turn(conn: &mut Conn, thread: Id) -> Result<Option<TurnRow>, DbError> {
    let sql = format!(
        "{TURN_SELECT} WHERE thread_id=$1 AND continuation_id=(SELECT active_continuation_id FROM threads WHERE node_id=$1) ORDER BY ordinal DESC LIMIT 1"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .fetch_optional(conn)
        .await?)
}

/// The turn of a thread that is under way, if there is one.
pub async fn active_turn(conn: &mut Conn, thread: Id) -> Result<Option<TurnRow>, DbError> {
    let sql = format!(
        "{TURN_SELECT} WHERE thread_id=$1 AND state IN ('starting','running')
          AND id=(SELECT current_turn_id FROM threads WHERE node_id=$1)
          AND continuation_id=(SELECT active_continuation_id FROM threads WHERE node_id=$1)"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .fetch_optional(conn)
        .await?)
}

/// The turns of a thread changed after `since`.
pub async fn turns_since(conn: &mut Conn, thread: Id, since: Seq) -> Result<Vec<TurnRow>, DbError> {
    let sql = format!("{TURN_SELECT} WHERE thread_id = $1 AND seq_updated > $2 ORDER BY ordinal");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(since)
        .fetch_all(conn)
        .await?)
}

/// The turns of a thread that are not over, and the last `ended` that are:
/// what a first load shows. Oldest first.
pub async fn turns_for_first_load(
    conn: &mut Conn,
    thread: Id,
    ended: i64,
) -> Result<Vec<TurnRow>, DbError> {
    let sql = format!(
        "{TURN_SELECT} WHERE thread_id = $1 AND (state IN ('starting', 'running') OR ordinal > (
              SELECT COALESCE(MAX(ordinal), 0) - $2 FROM turns WHERE thread_id = $1 AND state NOT IN ('starting', 'running')))
          ORDER BY ordinal"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(ended)
        .fetch_all(conn)
        .await?)
}

/// The turns, of every thread, that are under way.
pub async fn active_turns(conn: &mut Conn, limit: i64) -> Result<Vec<TurnRow>, DbError> {
    let sql = format!(
        "{TURN_SELECT} WHERE state IN ('starting', 'running') ORDER BY created_at, id LIMIT $1"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// The turns, of every thread, that are under way and that somebody asked
/// to stop.
pub async fn interrupted_active_turns(
    conn: &mut Conn,
    limit: i64,
) -> Result<Vec<TurnRow>, DbError> {
    let sql = format!(
        "{TURN_SELECT} WHERE state IN ('starting', 'running') AND interrupted_by IS NOT NULL
           ORDER BY created_at, id LIMIT $1"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Marks a turn that is under way as stopped by a person; the answer says
/// whether this call was the first to.
pub async fn mark_interrupted_by(
    conn: &mut Conn,
    turn: Id,
    by: Id,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE turns SET interrupted_by = $2, seq_updated = $3
          WHERE id = $1 AND state IN ('starting', 'running') AND interrupted_by IS NULL",
    )
    .bind(turn)
    .bind(by)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Records what cuts a turn that is under way (`person`, `usage_limit`, `environment_change`, `topic_setting`): the
/// first cause accepted for it stands, and a turn that already ended takes none.
pub async fn set_interrupt_cause(conn: &mut Conn, turn: Id, cause: &str) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE turns SET interrupt_cause = $2
          WHERE id = $1 AND state IN ('starting', 'running') AND interrupt_cause IS NULL",
    )
    .bind(turn)
    .bind(cause)
    .execute(conn)
    .await?;
    Ok(())
}

/// A cut the next input has not been told of: the turn before `ordinal`, where it was cut with a recorded cause and
/// its notice was not sent yet, with its cause and whoever stopped it, by the name they are shown with and their
/// handle, as an agent reads about a person.
pub async fn untold_cut(
    conn: &mut Conn,
    thread: Id,
    ordinal: i32,
) -> Result<Option<(Id, String, Option<String>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT t.id, t.interrupt_cause, u.display_name || ' (@' || u.username || ')'
           FROM (SELECT id, state, interrupt_cause, cut_notice_sent, interrupted_by FROM turns
                  WHERE thread_id = $1 AND ordinal < $2 ORDER BY ordinal DESC LIMIT 1) t
           LEFT JOIN users u ON u.id = t.interrupted_by
          WHERE t.state = 'interrupted' AND t.interrupt_cause IS NOT NULL AND NOT t.cut_notice_sent",
    )
    .bind(thread)
    .bind(ordinal)
    .fetch_optional(conn)
    .await?)
}

/// The next input was handed over with the notice of the cut of `turn`: it is not said again.
pub async fn cut_told(conn: &mut Conn, turn: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE turns SET cut_notice_sent = true WHERE id = $1")
        .bind(turn)
        .execute(conn)
        .await?;
    Ok(())
}

/// What a segment's agent was given of where it is (`segments.context_sent`): its lines by key, `resend` after the
/// harness shortened the conversation, or nothing yet.
pub async fn context_sent(conn: &mut Conn, segment: Id) -> Result<Json, DbError> {
    Ok(
        sqlx::query_scalar("SELECT context_sent FROM segments WHERE id = $1")
            .bind(segment)
            .fetch_optional(conn)
            .await?
            .unwrap_or(Json::Null),
    )
}

/// Notes, before a delivery is sent, the lines of where the agent is that go with it (`sending`, under the delivery's
/// reference): they count as given once the delivery is taken ([`context_given`]); what was given before stays.
pub async fn context_sending(
    conn: &mut Conn,
    segment: Id,
    reference: &str,
    lines: &Json,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE segments
            SET context_sent = jsonb_set(context_sent, '{sending}',
                                         jsonb_build_object('reference', $2::text, 'lines', $3::jsonb))
          WHERE id = $1",
    )
    .bind(segment)
    .bind(reference)
    .bind(lines)
    .execute(conn)
    .await?;
    Ok(())
}

/// The delivery `reference` was taken by the harness: the lines that went with it are what the agent was given. A
/// delivery noted after it, or a compaction since, leaves this alone.
pub async fn context_given(conn: &mut Conn, segment: Id, reference: &str) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE segments
            SET context_sent = jsonb_build_object('lines', context_sent->'sending'->'lines')
          WHERE id = $1 AND context_sent->'sending'->>'reference' = $2",
    )
    .bind(segment)
    .bind(reference)
    .execute(conn)
    .await?;
    Ok(())
}

/// Keeps what a segment's agent was given of where it is.
pub async fn set_context_sent(conn: &mut Conn, segment: Id, sent: &Json) -> Result<(), DbError> {
    sqlx::query("UPDATE segments SET context_sent = $2 WHERE id = $1")
        .bind(segment)
        .bind(sent)
        .execute(conn)
        .await?;
    Ok(())
}

/// The places whose standing instructions a change at `node` can make longer: the topics at and below it, where a
/// thread made there starts with what the topic's places say, and the threads at and below it that say something of
/// their own (one that does not reads what its topic does). Outermost first.
pub async fn standing_places_under(conn: &mut Conn, node: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT n.id FROM nodes n
          WHERE $1 = ANY(n.path) AND n.deleted_at IS NULL
            AND (n.kind = 'topic'
                 OR (n.kind = 'thread'
                     AND EXISTS (SELECT 1 FROM settings s WHERE s.node_id = n.id AND s.key = 'agent.instructions')))
          ORDER BY cardinality(n.path), n.id",
    )
    .bind(node)
    .fetch_all(conn)
    .await?)
}

/// Records, before the words of an input are sent, whether they name their author: it counts once the input is
/// delivered, and an attempt made again records what it sends.
pub async fn set_input_named(conn: &mut Conn, input: Id, named: bool) -> Result<(), DbError> {
    sqlx::query("UPDATE thread_inputs SET named = $2 WHERE id = $1 AND named <> $2")
        .bind(input)
        .bind(named)
        .execute(conn)
        .await?;
    Ok(())
}

/// Who sent the people's words delivered in the thread that named nobody, in the order they first spoke, where the
/// person's input delivered last before `input` was one of them: what the agent is told when names begin. None
/// where no person's input was delivered before, or where the last one named its sender already.
pub async fn unnamed_authors_before(
    conn: &mut Conn,
    thread: Id,
    input: Id,
) -> Result<Vec<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT u.display_name || ' (@' || u.username || ')'
           FROM (SELECT author_id, min(delivered_at) AS first_at FROM thread_inputs
                  WHERE thread_id = $1 AND id <> $2 AND author_id IS NOT NULL AND state = 'delivered' AND NOT named
                  GROUP BY author_id) a
           JOIN users u ON u.id = a.author_id
          WHERE NOT COALESCE((SELECT named FROM thread_inputs
                               WHERE thread_id = $1 AND id <> $2 AND author_id IS NOT NULL AND state = 'delivered'
                               ORDER BY delivered_at DESC NULLS LAST, id DESC LIMIT 1), true)
          ORDER BY a.first_at NULLS LAST, u.display_name",
    )
    .bind(thread)
    .bind(input)
    .fetch_all(conn)
    .await?)
}

/// The turn is running: `starting` → `running`, with the harness's name for
/// it. The answer says whether this call did.
pub async fn start_running(
    conn: &mut Conn,
    turn: Id,
    native_turn_ref: Option<&str>,
    at: DateTime<Utc>,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE turns SET state = 'running', native_turn_ref = COALESCE($2, native_turn_ref), started_at = $3, seq_updated = $4
          WHERE id = $1 AND state = 'starting'",
    )
    .bind(turn)
    .bind(native_turn_ref)
    .bind(at)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Ends a turn that is under way in `state` with `error`; terminal states
/// never change. The answer says whether this call ended it.
pub async fn finish_turn(
    conn: &mut Conn,
    turn: Id,
    state: &str,
    error: Option<&Json>,
    at: DateTime<Utc>,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE turns SET state = $2, error = $3, completed_at = $4, completed_seq = $5, phase = NULL, phase_at = NULL, seq_updated = $5,
                waited_ms = waited_ms + COALESCE(floor(GREATEST(0, extract(epoch FROM ($4::timestamptz - waiting_since)) * 1000))::bigint, 0),
                waiting_since = NULL
          WHERE id = $1 AND state IN ('starting', 'running')",
    )
    .bind(turn)
    .bind(state)
    .bind(error)
    .bind(at)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Retains one immutable follow-up with the terminal transition on the caller's thread lock.
pub async fn keep_turn_end_work(conn: &mut Conn, turn: Id, work: &Json) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE turns SET end_work=$2 WHERE id=$1 AND end_work IS NULL
        AND state NOT IN ('starting','running')",
    )
    .bind(turn)
    .bind(work)
    .execute(conn)
    .await?;
    Ok(())
}

/// The oldest pending follow-up; its owner must hold the thread lock through completion.
pub async fn turn_end_work(conn: &mut Conn, thread: Id) -> Result<Option<(Id, Json)>, DbError> {
    Ok(sqlx::query_as("SELECT id,end_work FROM turns WHERE thread_id=$1 AND end_work IS NOT NULL ORDER BY ordinal LIMIT 1")
        .bind(thread).fetch_optional(conn).await?)
}

/// New turns cannot outrun accounting and notification work from their preceding turn.
pub async fn turn_end_work_pending(conn: &mut Conn, thread: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM turns WHERE thread_id=$1 AND end_work IS NOT NULL)",
    )
    .bind(thread)
    .fetch_one(conn)
    .await?)
}

/// A bounded recovery source, including terminal pointers left by an older partial end.
pub async fn threads_with_turn_end_work(conn: &mut Conn, limit: i64) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT thread_id FROM (
        (SELECT thread_id,MIN(COALESCE((end_work->>'attempted_at')::timestamptz,completed_at,created_at)) AS due_at
            FROM turns WHERE end_work IS NOT NULL GROUP BY thread_id ORDER BY due_at,thread_id LIMIT $1)
        UNION ALL
        (SELECT t.node_id,COALESCE(u.completed_at,u.created_at) AS due_at FROM threads t
            JOIN turns u ON u.id=t.current_turn_id AND u.thread_id=t.node_id
            WHERE u.state NOT IN ('starting','running') ORDER BY due_at,t.node_id LIMIT $1)
        ) due GROUP BY thread_id ORDER BY MIN(due_at),thread_id LIMIT $1").bind(limit).fetch_all(conn).await?)
}

/// Retains the frozen snapshot and rotates failed work behind older unattempted threads.
pub async fn turn_end_work_attempted(
    conn: &mut Conn,
    thread: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE turns SET end_work=jsonb_set(end_work,'{attempted_at}',to_jsonb($2::timestamptz))
        WHERE thread_id=$1 AND end_work IS NOT NULL",
    )
    .bind(thread)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Deletes only the exact work successfully applied by this thread-locked transaction.
pub async fn finish_turn_end_work(conn: &mut Conn, turn: Id, work: &Json) -> Result<bool, DbError> {
    Ok(
        sqlx::query("UPDATE turns SET end_work=NULL WHERE id=$1 AND end_work=$2")
            .bind(turn)
            .bind(work)
            .execute(conn)
            .await?
            .rows_affected()
            == 1,
    )
}

/// Starts the first human wait or closes the last, while the thread's row is locked.
/// Repeated and overlapping requests do not restart or double-count the same interval.
/// Both ends are counted to the millisecond: a wait may begin at a moment as precise as the
/// clock gives and end at a person's decision, whose moment is cut to the millisecond, and a
/// wait of whole minutes is not a millisecond short.
pub async fn set_turn_waiting(
    conn: &mut Conn,
    thread: Id,
    waiting: bool,
    at: DateTime<Utc>,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query("UPDATE turns SET
            waited_ms = waited_ms + CASE WHEN $2 THEN 0 ELSE COALESCE(floor(GREATEST(0, extract(epoch FROM (date_trunc('milliseconds', $3::timestamptz) - date_trunc('milliseconds', waiting_since))) * 1000))::bigint, 0) END,
            waiting_since = CASE WHEN $2 THEN GREATEST(started_at, $3::timestamptz) ELSE NULL END,
            seq_updated = $4
        WHERE id = (SELECT current_turn_id FROM threads WHERE node_id = $1)
          AND state = 'running' AND (waiting_since IS NOT NULL) IS DISTINCT FROM $2")
        .bind(thread).bind(waiting).bind(at).bind(seq).execute(conn).await?;
    Ok(())
}

/// Replaces what a turn used, where it is under way or has just ended.
pub async fn set_turn_usage(
    conn: &mut Conn,
    turn: Id,
    usage: &Json,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query("UPDATE turns SET usage = $2, seq_updated = $3 WHERE id = $1")
        .bind(turn)
        .bind(usage)
        .bind(seq)
        .execute(conn)
        .await?;
    Ok(())
}

/// Sets what a running turn is doing (`working` or `compacting`), and since
/// when. The answer says whether this call changed it.
pub async fn set_turn_phase(
    conn: &mut Conn,
    turn: Id,
    phase: &str,
    at: DateTime<Utc>,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE turns SET phase = $2, phase_at = $3, seq_updated = $4
          WHERE id = $1 AND state IN ('starting', 'running') AND phase IS DISTINCT FROM $2",
    )
    .bind(turn)
    .bind(phase)
    .bind(at)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The lease a turn runs under.
pub async fn set_turn_lease(
    conn: &mut Conn,
    turn: Id,
    lease: Option<Id>,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query("UPDATE turns SET lease_id = $2, seq_updated = $3 WHERE id = $1")
        .bind(turn)
        .bind(lease)
        .bind(seq)
        .execute(conn)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------- items

const ITEM_SELECT: &str = "SELECT id, thread_id, continuation_id, segment_id, turn_id, model_input_id, stream, parent_item_id, kind, status, author_id,
        payload, native_ref, before_orch, search_text, pinned_at, pinned_by, chat_refs, seq_created, seq_updated, created_at, updated_at
   FROM items";

/// An entry of a thread's timeline.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ItemRow {
    /// Its identifier.
    pub id: Id,
    /// The thread.
    pub thread_id: Id,
    /// Actual immutable suffix owner; other continuations can reference its frozen body.
    pub continuation_id: Id,
    /// The segment.
    pub segment_id: Id,
    /// The turn.
    pub turn_id: Option<Id>,
    /// The exact recorded context associated with this item.
    pub model_input_id: Option<Id>,
    /// `main`, or `sub:` and a sub-agent's name.
    pub stream: String,
    /// The item it was made under.
    pub parent_item_id: Option<Id>,
    /// What kind of item it is.
    pub kind: String,
    /// How far it has come.
    pub status: String,
    /// Who it is by.
    pub author_id: Option<Id>,
    /// What it holds.
    pub payload: Json,
    /// The adapter's name for it.
    pub native_ref: Option<String>,
    /// Whether it is from the session before Orch: an earlier turn of an adopted session, which is read-only
    /// and never sent to the agent again. A query that does not read it takes it for an ordinary item.
    #[sqlx(default)]
    pub before_orch: bool,
    /// Its plain-text projection for search.
    pub search_text: Option<String>,
    /// When it was pinned.
    pub pinned_at: Option<DateTime<Utc>>,
    /// Who pinned it.
    pub pinned_by: Option<Id>,
    /// How many messages of the chat quote it.
    pub chat_refs: i32,
    /// The sequence at which it was made.
    pub seq_created: i64,
    /// The sequence at which it last changed.
    pub seq_updated: i64,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it last changed.
    pub updated_at: DateTime<Utc>,
}

/// An item to add.
#[derive(Debug, Clone, PartialEq)]
pub struct NewItem<'a> {
    /// Its identifier.
    pub id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The segment.
    pub segment_id: Id,
    /// The turn.
    pub turn_id: Option<Id>,
    /// The line of the conversation.
    pub stream: &'a str,
    /// The item it is made under.
    pub parent_item_id: Option<Id>,
    /// The kind.
    pub kind: &'a str,
    /// Its status.
    pub status: &'a str,
    /// Who it is by.
    pub author_id: Option<Id>,
    /// What it holds.
    pub payload: &'a Json,
    /// The adapter's name for it.
    pub native_ref: Option<&'a str>,
    /// Its projection for search.
    pub search_text: Option<&'a str>,
    /// When.
    pub at: DateTime<Utc>,
}

/// Adds an item, numbered `seq`. None where the segment already has an item
/// with this `native_ref`: it is there already, and nothing was written.
pub async fn insert_item(
    conn: &mut Conn,
    new: &NewItem<'_>,
    seq: Seq,
) -> Result<Option<ItemRow>, DbError> {
    let sql = "INSERT INTO items (id, thread_id, segment_id, turn_id, stream, parent_item_id, kind, status, author_id,
                    payload, native_ref, search_text, seq_created, seq_updated, created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $13, $14, $14)
               ON CONFLICT (segment_id, native_ref) WHERE native_ref IS NOT NULL DO NOTHING
               RETURNING id, thread_id, continuation_id, segment_id, turn_id, model_input_id, stream, parent_item_id, kind, status, author_id,
                    payload, native_ref, before_orch, search_text, pinned_at, pinned_by, chat_refs, seq_created, seq_updated,
                    created_at, updated_at";
    Ok(sqlx::query_as(sql)
        .bind(new.id)
        .bind(new.thread_id)
        .bind(new.segment_id)
        .bind(new.turn_id)
        .bind(new.stream)
        .bind(new.parent_item_id)
        .bind(new.kind)
        .bind(new.status)
        .bind(new.author_id)
        .bind(new.payload)
        .bind(new.native_ref)
        .bind(new.search_text)
        .bind(seq)
        .bind(new.at)
        .fetch_optional(conn)
        .await?)
}

/// The item `id`.
pub async fn item(conn: &mut Conn, id: Id) -> Result<Option<ItemRow>, DbError> {
    let sql = format!("{ITEM_SELECT} WHERE id = $1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// The item of a segment that the adapter calls `native_ref`.
pub async fn item_by_native_ref(
    conn: &mut Conn,
    segment: Id,
    native_ref: &str,
) -> Result<Option<ItemRow>, DbError> {
    // A native fork may replay its carried history when resumed. Its old ids resolve to the
    // immutable copy in this thread, scoped to the destination session rather than another segment.
    let sql = format!("{ITEM_SELECT} WHERE (segment_id = $1 AND native_ref = $2) OR
        (thread_id = (SELECT thread_id FROM segments WHERE id=$1) AND native_ref = 'fork-history:' || $1::uuid::text || ':' || $2)
        ORDER BY (segment_id=$1) DESC LIMIT 1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(segment)
        .bind(native_ref)
        .fetch_optional(conn)
        .await?)
}

/// The item of a person's words whose place in a thread was kept before they were sent (`in_progress`), by the
/// engine's key for it, in whichever segment it stands.
pub async fn user_message_kept(
    conn: &mut Conn,
    thread: Id,
    native_ref: &str,
) -> Result<Option<ItemRow>, DbError> {
    let sql = format!(
        "{ITEM_SELECT} WHERE thread_id = $1 AND native_ref = $2 AND kind = 'user_message' AND status = 'in_progress'"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(native_ref)
        .fetch_optional(conn)
        .await?)
}

/// What an item is changed to.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemChange<'a> {
    /// The new status; kept when none.
    pub status: Option<&'a str>,
    /// The new payload.
    pub payload: &'a Json,
    /// The new projection for search; kept when none.
    pub search_text: Option<&'a str>,
    /// When.
    pub at: DateTime<Utc>,
}

/// Changes an item, stamping it `seq`. The answer says whether there was
/// such an item.
pub async fn change_item(
    conn: &mut Conn,
    id: Id,
    change: &ItemChange<'_>,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE items SET status = COALESCE($2, status), payload = $3, search_text = COALESCE($4, search_text),
                seq_updated = $5, updated_at = $6
          WHERE id = $1",
    )
    .bind(id)
    .bind(change.status)
    .bind(change.payload)
    .bind(change.search_text)
    .bind(seq)
    .bind(change.at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Sets the status of an item whose status is one of `from`. The answer says
/// whether this call did.
pub async fn set_item_status(
    conn: &mut Conn,
    id: Id,
    from: &[&str],
    to: &str,
    at: DateTime<Utc>,
    seq: Seq,
) -> Result<bool, DbError> {
    let from: Vec<String> = from.iter().map(|status| (*status).to_owned()).collect();
    let done = sqlx::query(
        "UPDATE items SET status = $3, seq_updated = $4, updated_at = $5 WHERE id = $1 AND status = ANY($2)",
    )
    .bind(id)
    .bind(from)
    .bind(to)
    .bind(seq)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Sets the items of a turn that are still in progress to `status` (a turn
/// that ended with an item unfinished), stamping each. The answer is how
/// many changed.
/// Background commands remain open until their own completion arrives, and a
/// person's words whose place was kept before they were sent are settled by
/// what becomes of their input, not by the turn.
pub async fn settle_items_of_turn(
    conn: &mut Conn,
    turn: Id,
    status: &str,
    at: DateTime<Utc>,
    seq: Seq,
) -> Result<u64, DbError> {
    let done = sqlx::query(
        "UPDATE items SET status = $2, seq_updated = $4, updated_at = $3
          WHERE turn_id = $1 AND status = 'in_progress' AND kind <> 'user_message'
            AND NOT (kind = 'tool_call'
                     AND payload @> '{\"tool\":{\"kind\":\"command\",\"background\":true}}'::jsonb)",
    )
    .bind(turn)
    .bind(status)
    .bind(at)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(done.rows_affected())
}

/// Two bounded windows, in creation order: the newest `limit` overall and the newest
/// `limit` main items. At most twice the requested limit, with duplicates returned once;
/// a noisy child stream cannot evict the conversation it belongs to.
pub async fn latest_items(
    conn: &mut Conn,
    thread: Id,
    limit: i64,
) -> Result<Vec<ItemRow>, DbError> {
    let sql = format!(
        "{ITEM_SELECT} WHERE thread_id = $1 AND id IN (
             (SELECT id FROM items WHERE thread_id = $1 ORDER BY seq_created DESC LIMIT $2)
             UNION
             (SELECT id FROM items WHERE thread_id = $1 AND stream = 'main' ORDER BY seq_created DESC LIMIT $2)
         ) ORDER BY seq_created"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Whether the thread has items made before `seq`.
pub async fn has_items_before(conn: &mut Conn, thread: Id, seq: Seq) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM items WHERE thread_id = $1 AND seq_created < $2)",
    )
    .bind(thread)
    .bind(seq)
    .fetch_one(conn)
    .await?)
}

/// Whether the thread has items made after `seq`.
pub async fn has_items_after(conn: &mut Conn, thread: Id, seq: Seq) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM items WHERE thread_id = $1 AND seq_created > $2)",
    )
    .bind(thread)
    .bind(seq)
    .fetch_one(conn)
    .await?)
}

/// Whether this stream has any item before the boundary, independently of other streams.
pub async fn has_stream_items_before(
    conn: &mut Conn,
    thread: Id,
    seq: Seq,
    stream: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM items WHERE thread_id = $1 AND stream = $3 AND seq_created < $2)")
        .bind(thread).bind(seq).bind(stream).fetch_one(conn).await?)
}

/// Whether this stream has any item after the boundary, independently of other streams.
pub async fn has_stream_items_after(
    conn: &mut Conn,
    thread: Id,
    seq: Seq,
    stream: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM items WHERE thread_id = $1 AND stream = $3 AND seq_created > $2)")
        .bind(thread).bind(seq).bind(stream).fetch_one(conn).await?)
}

/// The items made before `seq`, newest `limit` of them, oldest first.
pub async fn items_before(
    conn: &mut Conn,
    thread: Id,
    seq: Seq,
    limit: i64,
    stream: Option<&str>,
) -> Result<Vec<ItemRow>, DbError> {
    let sql = format!(
        "SELECT * FROM ({ITEM_SELECT} WHERE thread_id = $1 AND seq_created < $2 AND ($4::text IS NULL OR stream = $4)
           ORDER BY seq_created DESC LIMIT $3) older ORDER BY seq_created"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(seq)
        .bind(limit)
        .bind(stream)
        .fetch_all(conn)
        .await?)
}

/// The items made after `seq`, oldest `limit` of them, oldest first.
pub async fn items_after(
    conn: &mut Conn,
    thread: Id,
    seq: Seq,
    limit: i64,
    stream: Option<&str>,
) -> Result<Vec<ItemRow>, DbError> {
    let sql = format!(
        "{ITEM_SELECT} WHERE thread_id = $1 AND seq_created > $2 AND ($4::text IS NULL OR stream = $4)
           ORDER BY seq_created LIMIT $3"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(seq)
        .bind(limit)
        .bind(stream)
        .fetch_all(conn)
        .await?)
}

/// A bounded page for a JSON export: all streams except reasoning, keeping
/// loose history and only the turns in the requested ordinal range.
pub async fn items_for_export(
    conn: &mut Conn,
    thread: Id,
    after: Seq,
    limit: i64,
    range: (Option<i64>, Option<i64>),
) -> Result<Vec<ItemRow>, DbError> {
    let sql = format!(
        "{ITEM_SELECT} WHERE thread_id = $1 AND seq_created > $2 AND kind <> 'reasoning'
          AND (turn_id IS NULL OR EXISTS (
              SELECT 1 FROM turns t WHERE t.id = items.turn_id
                AND ($4::bigint IS NULL OR t.ordinal >= $4)
                AND ($5::bigint IS NULL OR t.ordinal <= $5)))
          ORDER BY seq_created LIMIT $3"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(after)
        .bind(limit)
        .bind(range.0)
        .bind(range.1)
        .fetch_all(conn)
        .await?)
}

/// The items of a thread changed after `since`, oldest change first.
pub async fn items_since(conn: &mut Conn, thread: Id, since: Seq) -> Result<Vec<ItemRow>, DbError> {
    let sql =
        format!("{ITEM_SELECT} WHERE thread_id = $1 AND seq_updated > $2 ORDER BY seq_created");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(since)
        .fetch_all(conn)
        .await?)
}

/// How many rows of a thread changed after `since`: segments, turns, inputs
/// and items together.
pub async fn count_changed_since(conn: &mut Conn, thread: Id, since: Seq) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM segments WHERE thread_id = $1 AND seq_updated > $2)
              + (SELECT count(*) FROM turns WHERE thread_id = $1 AND seq_updated > $2)
              + (SELECT count(*) FROM thread_inputs WHERE thread_id = $1 AND seq_updated > $2)
              + (SELECT count(*) FROM items WHERE thread_id = $1 AND seq_updated > $2)",
    )
    .bind(thread)
    .bind(since)
    .fetch_one(conn)
    .await?)
}

// ---------------------------------------------------------------- people

/// A person as the timeline names them.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PersonRow {
    /// The user.
    pub id: Id,
    /// `human` or `service`.
    pub kind: String,
    /// The name they sign in with.
    pub username: String,
    /// The name shown beside what they did.
    pub display_name: String,
    /// The colour of their avatar.
    pub avatar_color: i16,
}

/// The people `ids`, each once; someone who does not exist is not among them.
/// Which of the given nodes are side threads (threads with a parent they were opened beside).
pub async fn side_threads_among(conn: &mut Conn, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT node_id FROM threads WHERE node_id=ANY($1) AND side_of IS NOT NULL",
    )
    .bind(ids)
    .fetch_all(&mut *conn)
    .await?)
}

pub async fn people(conn: &mut Conn, ids: &[Id]) -> Result<Vec<PersonRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, kind, username, display_name, avatar_color FROM users WHERE id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

// ---------------------------------------------------------------- reads and drafts

/// Records how far a person has read a thread; it only ever increases. The
/// answer is the position now stored.
pub async fn mark_read(
    conn: &mut Conn,
    user: Id,
    thread: Id,
    seq: Seq,
    at: DateTime<Utc>,
) -> Result<Seq, DbError> {
    let view: Id =
        sqlx::query_scalar("SELECT active_continuation_id FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(&mut *conn)
            .await?;
    super::thread_continuations::mark_read(conn, user, thread, view, seq, at).await
}

/// How far a person has read a thread.
pub async fn last_read(conn: &mut Conn, user: Id, thread: Id) -> Result<Option<Seq>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT r.last_read_seq FROM thread_reads r JOIN threads t ON t.node_id=r.thread_id
          AND t.active_continuation_id=r.continuation_id WHERE r.user_id = $1 AND r.thread_id = $2",
    )
    .bind(user)
    .bind(thread)
    .fetch_optional(conn)
    .await?)
}

/// A person's unsent content for a thread.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct DraftRow {
    /// Private native selection, preserved on text-only saves.
    #[sqlx(default)]
    pub native_action: Option<Json>,
    /// The parts.
    pub content: Json,
    /// How it is to be sent.
    pub delivery: String,
    /// Frozen reviewed hunks and anchors, visible only to their author.
    pub review_comments: Json,
    /// When it was last saved.
    pub updated_at: DateTime<Utc>,
}

/// A person's draft for a thread.
pub async fn draft(conn: &mut Conn, user: Id, thread: Id) -> Result<Option<DraftRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT content, delivery, review_comments, native_action, updated_at FROM drafts WHERE user_id = $1 AND thread_id = $2",
    )
    .bind(user)
    .bind(thread)
    .fetch_optional(conn)
    .await?)
}

/// Saves a person's draft for a thread, replacing the one there was.
pub async fn set_draft(
    conn: &mut Conn,
    user: Id,
    thread: Id,
    content: &Json,
    delivery: &str,
    at: DateTime<Utc>,
) -> Result<DraftRow, DbError> {
    Ok(sqlx::query_as(
        "INSERT INTO drafts (user_id, thread_id, content, delivery, updated_at) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (user_id, thread_id) DO UPDATE
            SET content = EXCLUDED.content, delivery = EXCLUDED.delivery, updated_at = EXCLUDED.updated_at
         RETURNING content, delivery, review_comments, native_action, updated_at",
    )
    .bind(user)
    .bind(thread)
    .bind(content)
    .bind(delivery)
    .bind(at)
    .fetch_one(conn)
    .await?)
}

/// Saves frozen comments when supplied, preserving them on a text-only save from another device.
pub async fn set_draft_with_comments(
    conn: &mut Conn,
    user: Id,
    thread: Id,
    content: &Json,
    delivery: &str,
    comments: Option<&Json>,
    at: DateTime<Utc>,
) -> Result<DraftRow, DbError> {
    Ok(sqlx::query_as(
        "INSERT INTO drafts (user_id, thread_id, content, delivery, review_comments, updated_at)
         VALUES ($1, $2, $3, $4, COALESCE($5::jsonb, '[]'::jsonb), $6)
         ON CONFLICT (user_id, thread_id) DO UPDATE SET content = EXCLUDED.content,
            delivery = EXCLUDED.delivery, updated_at = EXCLUDED.updated_at,
            review_comments = CASE WHEN $5::jsonb IS NULL THEN drafts.review_comments ELSE EXCLUDED.review_comments END
         RETURNING content, delivery, review_comments, native_action, updated_at",
    ).bind(user).bind(thread).bind(content).bind(delivery).bind(comments).bind(at).fetch_one(conn).await?)
}

/// Throws a person's draft for a thread away. The answer says whether there was one.
pub async fn set_draft_selection(
    conn: &mut Conn,
    user: Id,
    thread: Id,
    selection: Option<&Json>,
) -> Result<DraftRow, DbError> {
    Ok(sqlx::query_as("UPDATE drafts SET native_action=$3 WHERE user_id=$1 AND thread_id=$2 RETURNING content,delivery,review_comments,native_action,updated_at")
        .bind(user).bind(thread).bind(selection).fetch_one(conn).await?)
}

/// Throws a person's draft for a thread away. The answer says whether there was one.
pub async fn delete_draft(conn: &mut Conn, user: Id, thread: Id) -> Result<bool, DbError> {
    let done = sqlx::query("DELETE FROM drafts WHERE user_id = $1 AND thread_id = $2")
        .bind(user)
        .bind(thread)
        .execute(conn)
        .await?;
    Ok(done.rows_affected() == 1)
}

// ---------------------------------------------------------------- leases

/// A lease: one turn's leave to use a provider's credential.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct LeaseRow {
    /// Its identifier.
    pub id: Id,
    /// The provider.
    pub provider_id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The turn.
    pub turn_id: Option<Id>,
    /// The entry through which the author uses the provider.
    pub entry_id: Option<Id>,
    /// `forwarded`, `shared` or `handover`.
    pub mode: String,
    /// The credential form.
    pub form: Option<String>,
    /// The models it allows.
    pub allowed_models: Vec<String>,
    /// How much it lets the turn use.
    pub allowance: Option<Json>,
    /// When it was opened.
    pub opened_at: DateTime<Utc>,
    /// When it was closed.
    pub closed_at: Option<DateTime<Utc>>,
    /// Why.
    pub close_reason: Option<String>,
}

/// A lease to open.
#[derive(Debug, Clone, PartialEq)]
pub struct NewLease<'a> {
    /// Its identifier.
    pub id: Id,
    /// The provider.
    pub provider_id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The mode.
    pub mode: &'a str,
    /// The credential form.
    pub form: Option<&'a str>,
    /// The models it allows.
    pub allowed_models: &'a [String],
    /// How much it lets the turn use.
    pub allowance: Option<&'a Json>,
    /// When.
    pub at: DateTime<Utc>,
}

/// Opens a lease for a thread. A thread has one open lease at most; the
/// answer is none where it has one.
pub async fn open_lease(conn: &mut Conn, new: &NewLease<'_>) -> Result<Option<LeaseRow>, DbError> {
    Ok(sqlx::query_as(
        "INSERT INTO leases (id, provider_id, thread_id, mode, form, allowed_models, allowance, opened_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (thread_id) WHERE closed_at IS NULL DO NOTHING
         RETURNING id, provider_id, thread_id, turn_id, entry_id, mode, form, allowed_models, allowance,
                   opened_at, closed_at, close_reason",
    )
    .bind(new.id)
    .bind(new.provider_id)
    .bind(new.thread_id)
    .bind(new.mode)
    .bind(new.form)
    .bind(new.allowed_models)
    .bind(new.allowance)
    .bind(new.at)
    .fetch_optional(conn)
    .await?)
}

/// Names the turn a lease is for.
pub async fn set_lease_turn(conn: &mut Conn, lease: Id, turn: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE leases SET turn_id = $2 WHERE id = $1")
        .bind(lease)
        .bind(turn)
        .execute(conn)
        .await?;
    Ok(())
}

/// The open lease of a thread.
pub async fn open_lease_of(conn: &mut Conn, thread: Id) -> Result<Option<LeaseRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, provider_id, thread_id, turn_id, entry_id, mode, form, allowed_models, allowance,
                opened_at, closed_at, close_reason
           FROM leases WHERE thread_id = $1 AND closed_at IS NULL",
    )
    .bind(thread)
    .fetch_optional(conn)
    .await?)
}

/// The lease `id`.
pub async fn lease(conn: &mut Conn, id: Id) -> Result<Option<LeaseRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, provider_id, thread_id, turn_id, entry_id, mode, form, allowed_models, allowance,
                opened_at, closed_at, close_reason
           FROM leases WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Closes a lease that is open. The answer says whether this call did.
pub async fn close_lease(
    conn: &mut Conn,
    lease: Id,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE leases SET closed_at = $3, close_reason = $2 WHERE id = $1 AND closed_at IS NULL",
    )
    .bind(lease)
    .bind(reason)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Leases that are open and whose turn is over or gone: what a crash between
/// the end of a turn and the closing of its lease leaves.
pub async fn stale_open_leases(conn: &mut Conn, limit: i64) -> Result<Vec<LeaseRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT l.id, l.provider_id, l.thread_id, l.turn_id, l.entry_id, l.mode, l.form, l.allowed_models,
                l.allowance, l.opened_at, l.closed_at, l.close_reason
           FROM leases l LEFT JOIN turns t ON t.id = l.turn_id
          WHERE l.closed_at IS NULL AND l.turn_id IS NOT NULL AND (t.id IS NULL OR t.state NOT IN ('starting', 'running'))
          ORDER BY l.opened_at LIMIT $1",
    )
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

// ---------------------------------------------------------------- caps

/// A usage cap that covers a turn, as far as the engine needs to know.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CoveringCap {
    /// The cap.
    pub id: Id,
    /// `tokens`, `credits` or `running`.
    pub meter: String,
    /// The amount; none is no limit.
    pub amount_text: Option<String>,
    /// `everyone`, `user`, `group` or `role`.
    pub subject_kind: String,
    /// Whose it is, for a user or a group.
    pub subject_id: Option<Id>,
    /// `shared` or `each`.
    pub split: String,
}

/// The enabled caps that cover a turn started in the thread's place, on the
/// provider, with the model and the harness kind, for the person it is put
/// down to (none for nobody): by the node (the cap's node lies on `path`, or
/// it names none), the provider, the model patterns (a bare name, or a
/// family that ends in `*`), the harness kind and the subject of the cap.
/// A cap with no amount limits nothing and is left out.
pub async fn covering_caps(
    conn: &mut Conn,
    path: &[Id],
    provider: Id,
    model: &str,
    harness_kind: Option<&str>,
    person: Option<Id>,
) -> Result<Vec<CoveringCap>, DbError> {
    // A gateway names a model by its vendor and then its own name
    // (`openai/gpt-6-sol`): a pattern for the last part covers it too.
    let tail = model.rsplit('/').next().unwrap_or(model);
    Ok(sqlx::query_as(
        "SELECT c.id, c.meter, c.amount::text AS amount_text, c.subject_kind, c.subject_id, c.split
           FROM usage_caps c
          WHERE c.enabled AND c.amount IS NOT NULL
            AND (c.node_id IS NULL OR c.node_id = ANY($1))
            AND (c.provider_id IS NULL OR c.provider_id = $2)
            AND (cardinality(c.models) = 0 OR EXISTS (
                   SELECT 1 FROM unnest(c.models) pattern
                    WHERE pattern IN ($3, $6)
                       OR (right(pattern, 1) = '*' AND (
                              left($3, length(pattern) - 1) = left(pattern, length(pattern) - 1)
                           OR left($6, length(pattern) - 1) = left(pattern, length(pattern) - 1)))))
            AND (cardinality(c.harness_kinds) = 0 OR ($4::text IS NOT NULL AND $4 = ANY(c.harness_kinds)))
            AND (c.subject_kind = 'everyone'
                 OR c.subject_kind = 'role'
                 OR (c.subject_kind = 'user' AND $5::uuid IS NOT NULL AND c.subject_id = $5)
                 OR (c.subject_kind = 'group' AND $5::uuid IS NOT NULL AND EXISTS (
                       SELECT 1 FROM group_members g WHERE g.group_id = c.subject_id AND g.user_id = $5)))
          ORDER BY c.id",
    )
    .bind(path)
    .bind(provider)
    .bind(model)
    .bind(harness_kind)
    .bind(person)
    .bind(tail)
    .fetch_all(conn)
    .await?)
}

// ---------------------------------------------------------------- what the preamble says of the place

/// The name of a node that is not deleted.
pub async fn node_name(conn: &mut Conn, id: Id) -> Result<Option<String>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT name FROM nodes WHERE id = $1 AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// The names of the other threads of `topic` that run in `environment` and
/// are not side threads, which share its files with the thread asked about.
pub async fn topic_threads_in_environment(
    conn: &mut Conn,
    topic: Id,
    environment: Id,
    except: Id,
) -> Result<Vec<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT n.name FROM threads t JOIN nodes n ON n.id = t.node_id
          WHERE t.topic_id = $1 AND t.environment_id = $2 AND t.node_id <> $3
            AND t.side_of IS NULL AND n.deleted_at IS NULL
          ORDER BY n.name, n.id",
    )
    .bind(topic)
    .bind(environment)
    .bind(except)
    .fetch_all(conn)
    .await?)
}

/// How many threads of topics other than `topic` run in `environment`.
pub async fn other_topic_threads_in_environment(
    conn: &mut Conn,
    topic: Id,
    environment: Id,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM threads t JOIN nodes n ON n.id = t.node_id
          WHERE t.topic_id <> $1 AND t.environment_id = $2 AND n.deleted_at IS NULL",
    )
    .bind(topic)
    .bind(environment)
    .fetch_one(conn)
    .await?)
}

/// The short names of the orgs from the root org down to the nearest org at
/// or above `node`, which an address begins with.
pub async fn org_slugs_above(conn: &mut Conn, node: Id) -> Result<Vec<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT o.slug FROM nodes n
           CROSS JOIN LATERAL unnest(n.path) WITH ORDINALITY AS step(id, place)
           JOIN orgs o ON o.node_id = step.id
          WHERE n.id = $1
          ORDER BY step.place",
    )
    .bind(node)
    .fetch_all(conn)
    .await?)
}

/// The nodes from the root org down to a node, both included.
pub async fn node_path(conn: &mut Conn, node: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT path FROM nodes WHERE id = $1")
        .bind(node)
        .fetch_optional(conn)
        .await?
        .unwrap_or_default())
}

/// How many turns of an input failed, besides `except`.
pub async fn count_failed_turns_of_input(
    conn: &mut Conn,
    input: Id,
    except: Id,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM turns WHERE input_id = $1 AND state = 'failed' AND id <> $2",
    )
    .bind(input)
    .bind(except)
    .fetch_one(conn)
    .await?)
}

/// A page of the leases a provider's side may not have been told are closed: closed after `after` (a closing time
/// and the identifier of the last lease of the page before; the start of the window and the nil identifier for the
/// first), oldest first. Of one thread where `thread` is given; else the forwarded ones of every thread, the only
/// ones whose provider's side holds anything. Each read is a range of an index, so a page costs its rows and not
/// every lease ever closed.
pub async fn leases_closed_after(
    conn: &mut Conn,
    thread: Option<Id>,
    after: (DateTime<Utc>, Id),
    limit: i64,
) -> Result<Vec<LeaseRow>, DbError> {
    const COLUMNS: &str =
        "id, provider_id, thread_id, turn_id, entry_id, mode, form, allowed_models, allowance,
                opened_at, closed_at, close_reason";
    let sql = match thread {
        Some(_) => format!(
            "SELECT {COLUMNS} FROM leases WHERE thread_id = $3 AND closed_at IS NOT NULL AND (closed_at, id) > ($1, $2)
              ORDER BY closed_at, id LIMIT $4"
        ),
        None => format!(
            "SELECT {COLUMNS} FROM leases WHERE mode = 'forwarded' AND closed_at IS NOT NULL AND (closed_at, id) > ($1, $2)
                AND $3::uuid IS NULL ORDER BY closed_at, id LIMIT $4"
        ),
    };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(after.0)
        .bind(after.1)
        .bind(thread)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Moves a turn that is starting to another segment of its thread, which
/// the session it begins in was lost for.
pub async fn move_turn_to_segment(
    conn: &mut Conn,
    turn: Id,
    segment: Id,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE turns SET segment_id = $2, seq_updated = $3 WHERE id = $1 AND state = 'starting'",
    )
    .bind(turn)
    .bind(segment)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(())
}

/// The threads with a turn under way (running, or waiting for an approval)
/// whose harness runs on a machine that is not online, which the machine's
/// absence blocks.
pub async fn running_threads_on_offline_machines(
    conn: &mut Conn,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.node_id FROM threads t
           JOIN nodes n ON n.id = t.node_id
           LEFT JOIN environments e ON e.id = t.environment_id
           LEFT JOIN providers p ON p.id = t.provider_id
           JOIN machines m ON m.id = CASE WHEN t.arrangement = 'separated' THEN p.machine_id ELSE e.machine_id END
          WHERE t.state IN ('running', 'waiting_approval') AND n.deleted_at IS NULL AND NOT m.online
          ORDER BY t.node_id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// The threads blocked as `agent_offline` with a turn under way whose
/// harness runs on a machine that is online again.
pub async fn offline_threads_on_online_machines(
    conn: &mut Conn,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.node_id FROM threads t
           JOIN nodes n ON n.id = t.node_id
           LEFT JOIN environments e ON e.id = t.environment_id
           LEFT JOIN providers p ON p.id = t.provider_id
           JOIN machines m ON m.id = CASE WHEN t.arrangement = 'separated' THEN p.machine_id ELSE e.machine_id END
          WHERE t.state = 'blocked' AND t.blocked_reason = 'agent_offline' AND t.current_turn_id IS NOT NULL
            AND n.deleted_at IS NULL AND m.online AND m.status = 'active'
          ORDER BY t.node_id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

// ---------------------------------------------------------------- what the routes of threads read

/// How many threads other than `except` work in `directory` of `environment`:
/// the threads that share its instruction file and what an agent noted there.
pub async fn threads_in_directory(
    conn: &mut Conn,
    environment: Id,
    directory: &str,
    except: Id,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM threads t JOIN nodes n ON n.id = t.node_id
          WHERE t.environment_id = $1 AND t.workspace_dir = $2 AND t.node_id <> $3
            AND n.deleted_at IS NULL AND t.side_of IS NULL",
    )
    .bind(environment)
    .bind(directory)
    .bind(except)
    .fetch_one(conn)
    .await?)
}

/// Leaves the title of a thread to automation, which renames it from what
/// the conversation turns out to be about, until a person names it.
pub async fn leave_title_to_automation(conn: &mut Conn, thread: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE nodes SET title_source = 'auto' WHERE id = $1")
        .bind(thread)
        .execute(conn)
        .await?;
    Ok(())
}

/// Makes the chat that every node has, in the transaction that makes the node.
pub async fn make_node_chat(
    conn: &mut Conn,
    node: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO chats (id, kind, node_id, org_id, created_by, created_at)
         SELECT $1, 'node', n.id, n.org_id, $3, $4 FROM nodes n WHERE n.id = $2",
    )
    .bind(orch_types::new_id())
    .bind(node)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// How many side threads of `thread` are open: not archived and not deleted.
pub async fn open_side_threads(conn: &mut Conn, thread: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM threads t JOIN nodes n ON n.id = t.node_id
          WHERE t.side_of = $1 AND n.deleted_at IS NULL AND n.archived_at IS NULL",
    )
    .bind(thread)
    .fetch_one(conn)
    .await?)
}

/// Pins an item (`pinned` names who and when) or takes its pin off (`None`),
/// stamping it. The answer says whether there was such an item.
pub async fn pin_item(
    conn: &mut Conn,
    item: Id,
    pinned: Option<(Id, DateTime<Utc>)>,
    seq: Seq,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE items SET pinned_by = $2, pinned_at = $3, seq_updated = $4 WHERE id = $1",
    )
    .bind(item)
    .bind(pinned.map(|(by, _)| by))
    .bind(pinned.map(|(_, at)| at))
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The outline's conversation points, oldest first: main-stream messages, milestones,
/// approvals, and each turn's first visible item. Internal history and duplicate pins stay out.
pub async fn outline_items(conn: &mut Conn, thread: Id) -> Result<Vec<ItemRow>, DbError> {
    let sql = format!(
        "{ITEM_SELECT} WHERE thread_id = $1 AND stream = 'main' AND (
             kind IN ('user_message', 'agent_message', 'approval')
          OR (kind = 'announcement' AND payload->>'category' = 'milestone')
          OR id IN (SELECT DISTINCT ON (turn_id) id FROM items
                    WHERE thread_id = $1 AND stream = 'main' AND turn_id IS NOT NULL AND kind <> 'marker'
                    ORDER BY turn_id, seq_created)
          OR (kind = 'tool_call' AND payload #>> '{{tool,kind}}' = 'orch'
              AND payload #>> '{{tool,tool}}' = 'announce'
              AND payload #>> '{{tool,arguments,category}}' = 'milestone'))
         ORDER BY seq_created"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .fetch_all(conn)
        .await?)
}

/// What happened in a thread after a sequence, counted.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CatchupCounts {
    /// Items made.
    pub items: i64,
    /// Turns that ended well.
    pub turns_completed: i64,
    /// Turns that failed.
    pub turns_failed: i64,
    /// Commands that ran.
    pub commands: i64,
    /// Files that changed.
    pub files_changed: i64,
    /// Errors.
    pub errors: i64,
}

/// What happened in a thread after the sequence `since`, counted.
pub async fn catchup_counts(
    conn: &mut Conn,
    thread: Id,
    since: Seq,
) -> Result<CatchupCounts, DbError> {
    Ok(sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM items WHERE thread_id = $1 AND seq_created > $2) AS items,
            (SELECT count(*) FROM turns WHERE thread_id = $1 AND state = 'completed' AND seq_updated > $2) AS turns_completed,
            (SELECT count(*) FROM turns WHERE thread_id = $1 AND state = 'failed' AND seq_updated > $2) AS turns_failed,
            (SELECT count(*) FROM items WHERE thread_id = $1 AND seq_created > $2 AND kind = 'tool_call'
                AND payload #>> '{tool,kind}' = 'command') AS commands,
            (SELECT COALESCE(sum(jsonb_array_length(payload #> '{tool,changes}')), 0)::bigint FROM items
              WHERE thread_id = $1 AND seq_created > $2 AND kind = 'tool_call'
                AND payload #>> '{tool,kind}' = 'file_change') AS files_changed,
            (SELECT count(*) FROM items WHERE thread_id = $1 AND seq_created > $2 AND kind = 'error') AS errors",
    )
    .bind(thread)
    .bind(since)
    .fetch_one(conn)
    .await?)
}

/// An item and the line that names it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Titled {
    /// The item.
    pub item_id: Id,
    /// Its title.
    pub title: String,
}

/// The milestones the agent of a thread announced after a sequence.
pub async fn milestones_since(
    conn: &mut Conn,
    thread: Id,
    since: Seq,
) -> Result<Vec<Titled>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id AS item_id, COALESCE(payload #>> '{tool,arguments,title}', '') AS title FROM items
          WHERE thread_id = $1 AND seq_created > $2 AND kind = 'tool_call'
            AND payload #>> '{tool,kind}' = 'orch' AND payload #>> '{tool,tool}' = 'announce'
            AND payload #>> '{tool,arguments,category}' = 'milestone'
          ORDER BY seq_created",
    )
    .bind(thread)
    .bind(since)
    .fetch_all(conn)
    .await?)
}

/// The announcements that reached a thread after a sequence.
pub async fn announcements_since(
    conn: &mut Conn,
    thread: Id,
    since: Seq,
) -> Result<Vec<Titled>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id AS item_id, COALESCE(payload ->> 'title', '') AS title FROM items
          WHERE thread_id = $1 AND seq_created > $2 AND kind = 'announcement'
          ORDER BY seq_created",
    )
    .bind(thread)
    .bind(since)
    .fetch_all(conn)
    .await?)
}

// ---------------------------------------------------------------- what putting a thread to work reads and writes
//
// One block, kept together at the end of the file: whether a thread has a history yet, the segment
// of one that has none put on something else, where the environments a person could choose are
// used, the threads bound to a provider, and the attribution of a turn that has no lease (a harness
// that is signed in on its own machine). Nothing here decides anything.

/// What the binding of a thread, the choices offered for it and the usage a harness reports read
/// and write.
pub mod binding {
    use crate::usage::ledger::Attribution;
    use crate::{Conn, DbError};
    use orch_types::{Id, Seq};

    /// Whether anything has happened in a thread that a switch, and nothing less, may change the
    /// ground of: a turn was made, a session was opened or taken in, a bound segment followed
    /// another bound segment, or something was said here besides system lines. Read-only imported
    /// history has no execution binding and leaves initial local setup available.
    pub async fn has_history(conn: &mut Conn, thread: Id) -> Result<bool, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM turns WHERE thread_id = $1)
                 OR EXISTS (SELECT 1 FROM segments
                             WHERE thread_id = $1
                               AND agent_id IS NOT NULL
                               AND (native_session_ref IS NOT NULL OR import_fidelity IS NOT NULL
                                    OR EXISTS (SELECT 1 FROM segments earlier WHERE earlier.thread_id=$1
                                        AND earlier.agent_id IS NOT NULL AND earlier.ordinal<segments.ordinal)))
                 OR EXISTS (SELECT 1 FROM items WHERE thread_id = $1 AND NOT before_orch AND kind NOT IN ('marker', 'announcement'))",
        )
        .bind(thread)
        .fetch_one(conn)
        .await?)
    }

    /// What a segment that has not begun is put on.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SegmentBinding<'a> {
        /// The agent profile.
        pub agent: Id,
        /// The provider.
        pub provider: Id,
        /// The environment.
        pub environment: Id,
        /// The generation of the environment now.
        pub env_generation: i32,
        /// The arrangement type.
        pub arrangement: &'a str,
        /// The executor that carries out its tool calls.
        pub executor: Option<&'a str>,
        /// The tool set of that executor.
        pub tool_set: Option<&'a str>,
        /// How its harness reaches the model.
        pub credential_path: &'a str,
    }

    /// Puts a segment that is going and has no session yet on something else, stamping it. The
    /// answer says whether there was such a segment: one that has a session is a stretch of a
    /// conversation, and is ended by a switch, never rewritten.
    pub async fn rebind_segment(
        conn: &mut Conn,
        segment: Id,
        to: &SegmentBinding<'_>,
        seq: Seq,
    ) -> Result<bool, DbError> {
        let done = sqlx::query(
            "UPDATE segments SET agent_id = $2, provider_id = $3, environment_id = $4, env_generation = $5,
                    arrangement = $6, executor = $7, tool_set = $8, credential_path = $9, seq_updated = $10
              WHERE id = $1 AND ended_at IS NULL AND native_session_ref IS NULL",
        )
        .bind(segment)
        .bind(to.agent)
        .bind(to.provider)
        .bind(to.environment)
        .bind(to.env_generation)
        .bind(to.arrangement)
        .bind(to.executor)
        .bind(to.tool_set)
        .bind(to.credential_path)
        .bind(seq)
        .execute(conn)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Where an environment is used, seen from one topic.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct EnvironmentUse {
        /// The environment.
        pub environment: Id,
        /// How many threads work in it: not deleted, not archived, not a side thread.
        pub threads: i64,
        /// The topics those threads are in.
        pub topics: Vec<Id>,
        /// Whether a thread of the topic works in it.
        pub in_topic: bool,
        /// Whether a thread of another topic of the topic's project does.
        pub in_project: bool,
        /// The directory the newest thread of the topic has in it.
        pub topic_directory: Option<String>,
    }

    /// Where each of the environments `environments` is used, seen from `topic`. An environment
    /// that no thread works in is not among them.
    pub async fn environment_uses(
        conn: &mut Conn,
        environments: &[Id],
        topic: Id,
    ) -> Result<Vec<EnvironmentUse>, DbError> {
        Ok(sqlx::query_as(
            "SELECT t.environment_id AS environment, count(*) AS threads,
                    array_agg(DISTINCT t.topic_id) AS topics,
                    bool_or(t.topic_id = $2) AS in_topic,
                    bool_or(t.topic_id <> $2 AND own.parent_id = (SELECT parent_id FROM nodes WHERE id = $2)) AS in_project,
                    (array_agg(t.workspace_dir ORDER BY n.created_at DESC, n.id) FILTER (WHERE t.topic_id = $2))[1]
                      AS topic_directory
               FROM threads t
               JOIN nodes n ON n.id = t.node_id
               JOIN nodes own ON own.id = t.topic_id
              WHERE t.environment_id = ANY($1) AND n.deleted_at IS NULL AND n.archived_at IS NULL
                AND t.side_of IS NULL
              GROUP BY t.environment_id",
        )
        .bind(environments)
        .bind(topic)
        .fetch_all(conn)
        .await?)
    }

    /// A thread bound to a provider.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct ProviderThread {
        /// The thread's node.
        pub thread_id: Id,
        /// Its name.
        pub name: String,
        /// The topic it lies in.
        pub topic_id: Id,
        /// Its root org.
        pub org_id: Id,
        /// What it is doing.
        pub state: String,
        /// Why it cannot go on, when it cannot.
        pub blocked_reason: Option<String>,
        /// Its first owner.
        pub owner_id: Id,
    }

    /// The threads bound to a provider, in the order of their identifiers: every thread that is
    /// not deleted, of whatever org, side threads left out. Whether a reader may see one is for
    /// the access engine to say.
    pub async fn threads_on_provider(
        conn: &mut Conn,
        provider: Id,
    ) -> Result<Vec<ProviderThread>, DbError> {
        Ok(sqlx::query_as(
            "SELECT t.node_id AS thread_id, n.name, t.topic_id, n.org_id, t.state, t.blocked_reason,
                    n.owner_id
               FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.provider_id = $1 AND n.deleted_at IS NULL AND t.side_of IS NULL
              ORDER BY t.node_id",
        )
        .bind(provider)
        .fetch_all(conn)
        .await?)
    }

    /// The threads that adopted a conversation a harness holds, by the harness's reference to it: for
    /// each reference, the first thread whose segments hold it.
    pub async fn adopting_threads(
        conn: &mut Conn,
        references: &[String],
    ) -> Result<Vec<(String, Id)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT DISTINCT ON (s.native_session_ref) s.native_session_ref, s.thread_id
               FROM segments s JOIN nodes n ON n.id = s.thread_id AND n.deleted_at IS NULL
              WHERE s.native_session_ref = ANY($1)
              ORDER BY s.native_session_ref, s.thread_id",
        )
        .bind(references)
        .fetch_all(conn)
        .await?)
    }

    /// The attribution of the turn `turn`, read as it stands: the person and the place, what the
    /// turn ran on and with. The entry is the lease's, and none for a turn that had no lease (a
    /// harness on its own machine, signed in there). None where the turn is gone. The topic's row is
    /// held against a change while the transaction lasts, so that a topic that is moved waits for
    /// the use being written.
    pub async fn attribution_of_turn(
        conn: &mut Conn,
        turn: Id,
    ) -> Result<Option<Attribution>, DbError> {
        Ok(sqlx::query_as(
            "SELECT topic.org_id AS org_id, topic.parent_id AS project_id, th.topic_id AS topic_id,
                    t.thread_id AS thread_id, t.segment_id AS segment_id, t.id AS turn_id,
                    s.provider_id AS provider_id, l.entry_id AS entry_id, i.author_id AS user_id,
                    s.agent_id AS agent_id, a.harness_kind AS harness_kind,
                    s.environment_id AS environment_id, e.placement AS environment_kind,
                    s.arrangement AS arrangement, t.model AS turn_model, t.effort AS effort, t.params AS params
               FROM turns t
               JOIN segments s ON s.id = t.segment_id
               JOIN threads th ON th.node_id = t.thread_id
               JOIN nodes topic ON topic.id = th.topic_id
               LEFT JOIN leases l ON l.id = t.lease_id
               LEFT JOIN thread_inputs i ON i.id = t.input_id
               LEFT JOIN agents a ON a.id = s.agent_id
               LEFT JOIN environments e ON e.id = s.environment_id
              WHERE t.id = $1
              FOR SHARE OF topic",
        )
        .bind(turn)
        .fetch_optional(conn)
        .await?)
    }
}

// ---------------------------------------------------------------- what the tools of the orch server read and write
//
// One block, kept together at the end of the file: the announcements that people and agents send to the threads of a
// topic and the messages one thread's agent hands another, the threads of a topic as those find them, and the rate of
// what one thread's agent sends. Nothing here decides anything.

/// What an announcement is made of and what finds it again.
pub mod tools {
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json, Seq};

    /// A thread of a topic as an announcement or a message finds it.
    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    pub struct TopicThread {
        /// The thread's node.
        pub node_id: Id,
        /// Its name.
        pub name: String,
        /// What it is doing.
        pub state: String,
        /// Whether it was archived itself.
        pub archived: bool,
        /// For a side thread: its parent.
        pub side_of: Option<Id>,
        /// Its first owner.
        pub owner_id: Id,
        /// The turn under way.
        pub current_turn_id: Option<Id>,
        /// Its agent profile.
        pub agent_id: Option<Id>,
    }

    /// The threads of a topic that are not deleted, oldest first: those an announcement can reach, and those a
    /// message can be addressed to by name.
    pub async fn topic_threads(conn: &mut Conn, topic: Id) -> Result<Vec<TopicThread>, DbError> {
        Ok(sqlx::query_as(
            "SELECT t.node_id, n.name, t.state, (n.archived_at IS NOT NULL) AS archived, t.side_of, n.owner_id,
                    t.current_turn_id, t.agent_id
               FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.topic_id = $1 AND n.deleted_at IS NULL
              ORDER BY n.created_at, t.node_id",
        )
        .bind(topic)
        .fetch_all(conn)
        .await?)
    }

    /// Takes the rows of these threads, in the order of their identifiers: whoever changes several threads in one
    /// transaction takes them in this order, so that two of them never wait for each other.
    pub async fn lock_threads(conn: &mut Conn, threads: &[Id]) -> Result<(), DbError> {
        sqlx::query(
            "SELECT node_id FROM threads WHERE node_id = ANY($1) ORDER BY node_id FOR UPDATE",
        )
        .bind(threads)
        .fetch_all(conn)
        .await?;
        Ok(())
    }

    /// An announcement, or a message from one thread to another.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct AnnouncementRow {
        /// Its identifier.
        pub id: Id,
        /// The topic.
        pub topic_id: Id,
        /// The thread whose agent sent it.
        pub from_thread_id: Option<Id>,
        /// The person who sent it.
        pub from_user_id: Option<Id>,
        /// `notice` or `milestone`.
        pub category: String,
        /// Its title; empty for a message to one thread.
        pub title: String,
        /// Its text.
        pub body: String,
        /// What a message to one thread carries.
        pub files: Json,
        /// `info` or `important`.
        pub severity: String,
        /// `interrupt`, `steer` or `next_turn`, as applied.
        pub delivery: String,
        /// `next_turn` or `interrupt`.
        pub fallback: String,
        /// Whether it wakes a thread that is idle.
        pub wake_idle: bool,
        /// `all`, `active`, `thread` or `none`.
        pub targets: String,
        /// The one thread of a message.
        pub to_thread_id: Option<Id>,
        /// Set on the rows of one project-wide announcement.
        pub group_id: Option<Id>,
        /// Its place in the topic's feed.
        pub topic_seq: Seq,
        /// When it was sent.
        pub created_at: DateTime<Utc>,
    }

    const ANNOUNCEMENT_COLUMNS: &str = "id, topic_id, from_thread_id, from_user_id, category, title, COALESCE(reference_body,body) AS body, files,
        severity, delivery, fallback, wake_idle, targets, to_thread_id, group_id, topic_seq, created_at";

    /// An announcement to make.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewAnnouncement<'a> {
        /// Its identifier: derived from the call that made it, so that the same call makes it once.
        pub id: Id,
        /// The topic.
        pub topic_id: Id,
        /// The thread whose agent sends it.
        pub from_thread_id: Option<Id>,
        /// The person who sends it.
        pub from_user_id: Option<Id>,
        /// `notice` or `milestone`.
        pub category: &'a str,
        /// Its title.
        pub title: &'a str,
        /// Its text.
        pub body: &'a str,
        /// What it carries.
        pub files: &'a Json,
        /// `info` or `important`.
        pub severity: &'a str,
        /// How it reaches a thread at work.
        pub delivery: &'a str,
        /// What it does where it may not reach into a running turn.
        pub fallback: &'a str,
        /// Whether it wakes a thread that is idle.
        pub wake_idle: bool,
        /// Whom it goes to.
        pub targets: &'a str,
        /// The one thread of a message.
        pub to_thread_id: Option<Id>,
        /// The group of a project-wide announcement.
        pub group_id: Option<Id>,
        /// When.
        pub at: DateTime<Utc>,
    }

    /// Makes an announcement and gives it the next place in the topic's feed. None where it was made before, which
    /// is the answer to the same call asked again.
    pub async fn insert_announcement(
        conn: &mut Conn,
        new: &NewAnnouncement<'_>,
    ) -> Result<Option<AnnouncementRow>, DbError> {
        insert_referenced_announcement(conn, new, new.body).await
    }

    /// Stores the checked source body beside its resolved form without changing source length limits.
    pub async fn insert_referenced_announcement(
        conn: &mut Conn,
        new: &NewAnnouncement<'_>,
        authored: &str,
    ) -> Result<Option<AnnouncementRow>, DbError> {
        if announcement(conn, new.id).await?.is_some() {
            return Ok(None);
        }
        let seq: Seq = sqlx::query_scalar(
            "UPDATE topics SET topic_seq = topic_seq + 1 WHERE node_id = $1 RETURNING topic_seq",
        )
        .bind(new.topic_id)
        .fetch_one(&mut *conn)
        .await?;
        let made = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "INSERT INTO announcements (id, topic_id, from_thread_id, from_user_id, category, title, body, files,
                                       severity, delivery, fallback, wake_idle, targets, to_thread_id, group_id,
                                       topic_seq, created_at, reference_body)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
             ON CONFLICT (id) DO NOTHING
             RETURNING {ANNOUNCEMENT_COLUMNS}"
        )))
        .bind(new.id)
        .bind(new.topic_id)
        .bind(new.from_thread_id)
        .bind(new.from_user_id)
        .bind(new.category)
        .bind(new.title)
        .bind(authored)
        .bind(new.files)
        .bind(new.severity)
        .bind(new.delivery)
        .bind(new.fallback)
        .bind(new.wake_idle)
        .bind(new.targets)
        .bind(new.to_thread_id)
        .bind(new.group_id)
        .bind(seq)
        .bind(new.at)
        .bind((new.body != authored).then_some(new.body))
        .fetch_optional(&mut *conn)
        .await?;
        crate::sequences::notify_topic(conn, new.topic_id, seq).await?;
        Ok(made)
    }

    /// The announcement `id`.
    pub async fn announcement(conn: &mut Conn, id: Id) -> Result<Option<AnnouncementRow>, DbError> {
        let sql = format!("SELECT {ANNOUNCEMENT_COLUMNS} FROM announcements WHERE id = $1");
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(conn)
            .await?)
    }

    /// When the announcements and messages a thread's agent sent after `since` were sent, one moment for each: a
    /// project-wide one is one, however many topics it went to, and one that only the followers heard of is none.
    pub async fn sent_since(
        conn: &mut Conn,
        thread: Id,
        since: DateTime<Utc>,
    ) -> Result<Vec<DateTime<Utc>>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT min(created_at) FROM announcements
              WHERE from_thread_id = $1 AND targets <> 'none' AND created_at > $2
              GROUP BY COALESCE(group_id, id) ORDER BY 1",
        )
        .bind(thread)
        .bind(since)
        .fetch_all(conn)
        .await?)
    }

    /// The announcements of a topic, newest first.
    pub async fn announcements_of_topic(
        conn: &mut Conn,
        topic: Id,
        category: Option<&str>,
        everything: bool,
        before: Option<Seq>,
        limit: i64,
    ) -> Result<Vec<AnnouncementRow>, DbError> {
        let sql = format!(
            "SELECT {ANNOUNCEMENT_COLUMNS} FROM announcements
              WHERE topic_id = $1 AND ($2::text IS NULL OR category = $2)
                AND ($3 OR targets IN ('all', 'active'))
                AND ($4::bigint IS NULL OR topic_seq < $4)
              ORDER BY topic_seq DESC LIMIT $5"
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(topic)
            .bind(category)
            .bind(everything)
            .bind(before)
            .bind(limit)
            .fetch_all(conn)
            .await?)
    }

    /// Announcements anywhere in a place, ordered by time and identifier across topic sequences.
    pub async fn announcements_of_place(
        conn: &mut Conn,
        place: Id,
        category: Option<&str>,
        everything: bool,
        before: Option<(DateTime<Utc>, Id)>,
        limit: i64,
    ) -> Result<Vec<AnnouncementRow>, DbError> {
        let (at, id) = before.map_or((None, None), |(at, id)| (Some(at), Some(id)));
        let sql = format!(
            "SELECT {ANNOUNCEMENT_COLUMNS} FROM announcements
            WHERE topic_id IN (SELECT id FROM nodes WHERE $1=ANY(path) AND deleted_at IS NULL)
              AND ($2::text IS NULL OR category=$2) AND ($3 OR targets IN ('all','active'))
              AND ($4::timestamptz IS NULL OR (created_at,id)<($4,$5))
            ORDER BY created_at DESC,id DESC LIMIT $6"
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(place)
            .bind(category)
            .bind(everything)
            .bind(at)
            .bind(id)
            .bind(limit)
            .fetch_all(conn)
            .await?)
    }

    /// How many threads each announcement reached in each way, from the lines it left in them.
    pub async fn delivered_counts(
        conn: &mut Conn,
        announcements: &[Id],
    ) -> Result<Vec<(Id, String, i64)>, DbError> {
        let wanted: Vec<String> = announcements.iter().map(ToString::to_string).collect();
        let found: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT payload ->> 'announcement_id', payload ->> 'delivered_as', count(*) FROM items
              WHERE kind = 'announcement' AND payload ->> 'announcement_id' = ANY($1)
                AND payload ->> 'delivered_as' IS NOT NULL
              GROUP BY 1, 2",
        )
        .bind(wanted)
        .fetch_all(conn)
        .await?;
        Ok(found
            .into_iter()
            .filter_map(|(id, how, count)| Some((id.parse().ok()?, how, count)))
            .collect())
    }

    /// The open topics of a project and of the projects inside it, at any depth, that are not archived
    /// (themselves or through a folder they lie in) and not deleted: where a project-wide announcement
    /// goes before the ones that were made private are passed over.
    pub async fn open_topics_of(conn: &mut Conn, project: Id) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT n.id FROM nodes n JOIN topics t ON t.node_id = n.id
              WHERE n.kind = 'topic' AND $1 = ANY(n.path) AND t.status = 'open'
                AND NOT EXISTS (SELECT 1 FROM nodes a
                                 WHERE a.id = ANY(n.path) AND (a.deleted_at IS NOT NULL OR a.archived_at IS NOT NULL))
              ORDER BY n.created_at, n.id",
        )
        .bind(project)
        .fetch_all(conn)
        .await?)
    }

    /// The parent of a node.
    pub async fn node_parent(conn: &mut Conn, node: Id) -> Result<Option<Id>, DbError> {
        Ok(
            sqlx::query_scalar("SELECT parent_id FROM nodes WHERE id = $1")
                .bind(node)
                .fetch_optional(conn)
                .await?
                .flatten(),
        )
    }

    /// What kind of node `node` is: `org`, `project`, `topic` or `thread`; none where there is no such node.
    pub async fn node_kind(conn: &mut Conn, node: Id) -> Result<Option<String>, DbError> {
        Ok(
            sqlx::query_scalar("SELECT kind FROM nodes WHERE id = $1 AND deleted_at IS NULL")
                .bind(node)
                .fetch_optional(conn)
                .await?,
        )
    }

    /// How many topics a project has that are not deleted, those of the folders inside it among them.
    pub async fn count_topics_of(conn: &mut Conn, project: Id) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM nodes n
              WHERE n.kind = 'topic' AND $1 = ANY(n.path)
                AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL)",
        )
        .bind(project)
        .fetch_one(conn)
        .await?)
    }

    /// The nearest org on the path of a node, the node itself if it is one: the home of what the node is the
    /// home of.
    pub async fn nearest_org(conn: &mut Conn, node: Id) -> Result<Option<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT o.id FROM nodes h JOIN nodes o ON o.id = ANY(h.path) AND o.kind = 'org'
              WHERE h.id = $1 ORDER BY array_position(h.path, o.id) DESC LIMIT 1",
        )
        .bind(node)
        .fetch_optional(conn)
        .await?)
    }

    /// The threads that work in an environment and are not deleted.
    pub async fn threads_in_environment(
        conn: &mut Conn,
        environment: Id,
    ) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.environment_id = $1 AND n.deleted_at IS NULL AND t.side_of IS NULL
              ORDER BY t.node_id",
        )
        .bind(environment)
        .fetch_all(conn)
        .await?)
    }

    /// The name of a host.
    pub async fn host_name(conn: &mut Conn, host: Id) -> Result<Option<String>, DbError> {
        Ok(sqlx::query_scalar("SELECT name FROM hosts WHERE id = $1")
            .bind(host)
            .fetch_optional(conn)
            .await?)
    }
}

// ---- Fork, switch and side threads: the reads and writes the part that makes one thread of another needs ----
//
// Everything below belongs to that part; it is kept apart so that it can be read at once.

/// Segments that take in a history, the turns a history is made of, and side threads.
pub mod forking {
    use super::ItemRow;
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json, Seq};

    /// Says how a segment took in the history it began with: how faithfully, and what was kept, converted
    /// and dropped.
    pub async fn set_segment_import(
        conn: &mut Conn,
        segment: Id,
        fidelity: Option<&str>,
        report: Option<&Json>,
        seq: Seq,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE segments SET import_fidelity = $2, import_report = $3, seq_updated = $4 WHERE id = $1",
        )
        .bind(segment)
        .bind(fidelity)
        .bind(report)
        .bind(seq)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// How many turns a thread has had.
    pub async fn turn_count(conn: &mut Conn, thread: Id) -> Result<i64, DbError> {
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM turns WHERE thread_id = $1")
                .bind(thread)
                .fetch_one(conn)
                .await?,
        )
    }

    /// The number of the last turn of a thread that completed.
    pub async fn last_completed_turn(conn: &mut Conn, thread: Id) -> Result<Option<i32>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT max(ordinal) FROM turns WHERE thread_id = $1 AND state = 'completed'",
        )
        .bind(thread)
        .fetch_one(conn)
        .await?)
    }

    /// The turns of a thread numbered `from` to `to`, both counted in: the identifier, the number and the
    /// state of each, oldest first.
    pub async fn turn_range(
        conn: &mut Conn,
        thread: Id,
        from: i32,
        to: i32,
    ) -> Result<Vec<(Id, i32, String)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, ordinal, state FROM turns
              WHERE thread_id = $1 AND ordinal BETWEEN $2 AND $3 ORDER BY ordinal",
        )
        .bind(thread)
        .bind(from)
        .bind(to)
        .fetch_all(conn)
        .await?)
    }

    /// Every item of the main stream that was made in these turns, oldest first: what a history that
    /// travels is made of, with the kinds that do not travel counted by whoever reads it.
    pub async fn items_of_turns(
        conn: &mut Conn,
        thread: Id,
        turns: &[Id],
    ) -> Result<Vec<ItemRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{} WHERE thread_id = $1 AND turn_id = ANY($2) AND stream = 'main' ORDER BY seq_created",
            super::ITEM_SELECT
        )))
        .bind(thread)
        .bind(turns)
        .fetch_all(conn)
        .await?)
    }

    /// How many items of a thread's main stream belong to no turn.
    pub async fn count_turnless_items(conn: &mut Conn, thread: Id) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM items WHERE thread_id = $1 AND turn_id IS NULL AND stream = 'main'",
        )
        .bind(thread)
        .fetch_one(conn)
        .await?)
    }

    /// A side thread to make.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewSide<'a> {
        /// Its identifier.
        pub id: Id,
        /// The thread it is taken from.
        pub parent: Id,
        /// Its name.
        pub name: &'a str,
        /// Whose it is.
        pub owner: Id,
        /// The last turn of the parent whose history it takes.
        pub from_turn: i32,
        /// When.
        pub at: DateTime<Utc>,
    }

    /// Makes the node and the thread rows of a side thread: a node of kind `thread` that lies in its
    /// parent, in the topic its parent is in. The answer says whether the parent was there, was a thread
    /// that is not itself a side thread, and was not deleted.
    pub async fn insert_side_thread(conn: &mut Conn, new: &NewSide<'_>) -> Result<bool, DbError> {
        let made = sqlx::query(
            "INSERT INTO nodes (id, kind, parent_id, org_id, path, name, owner_id, created_by, created_at, updated_at)
             SELECT $1, 'thread', p.id, p.org_id, p.path || $1, $2, $3, $3, $4, $4
               FROM nodes p JOIN threads t ON t.node_id = p.id
              WHERE p.id = $5 AND p.kind = 'thread' AND p.deleted_at IS NULL AND t.side_of IS NULL",
        )
        .bind(new.id)
        .bind(new.name)
        .bind(new.owner)
        .bind(new.at)
        .bind(new.parent)
        .execute(&mut *conn)
        .await?;
        if made.rows_affected() == 0 {
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO threads (node_id, topic_id, side_of, side_from_turn)
             SELECT $1, t.topic_id, t.node_id, $3 FROM threads t WHERE t.node_id = $2",
        )
        .bind(new.id)
        .bind(new.parent)
        .bind(new.from_turn)
        .execute(conn)
        .await?;
        Ok(true)
    }

    /// The side threads of a thread, oldest first: those that are open, or those that were closed and
    /// are not purged yet.
    pub async fn side_threads_of(
        conn: &mut Conn,
        parent: Id,
        closed: bool,
    ) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.side_of = $1 AND (n.deleted_at IS NOT NULL) = $2
              ORDER BY n.created_at, t.node_id",
        )
        .bind(parent)
        .bind(closed)
        .fetch_all(conn)
        .await?)
    }

    /// Makes the closed side threads among `ids` look open to the transaction `conn` belongs to, so that
    /// they can be shown as they were: a deleted node has no view. The caller rolls that transaction back
    /// and never commits it.
    pub async fn show_closed(conn: &mut Conn, ids: &[Id]) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE nodes SET deleted_at = NULL, deleted_by = NULL
              WHERE id = ANY($1) AND deleted_at IS NOT NULL
                AND EXISTS (SELECT 1 FROM threads t WHERE t.node_id = nodes.id AND t.side_of IS NOT NULL)",
        )
        .bind(ids)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// The side thread of `parent` that `author` started with the message `client_id`, if there is one and
    /// it is open: sending the same request again is the same request.
    pub async fn side_thread_started_with(
        conn: &mut Conn,
        parent: Id,
        author: Id,
        client_id: &str,
    ) -> Result<Option<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT t.node_id FROM threads t
               JOIN nodes n ON n.id = t.node_id
               JOIN thread_inputs i ON i.thread_id = t.node_id
              WHERE t.side_of = $1 AND n.deleted_at IS NULL AND i.author_id = $2 AND i.client_id = $3
              ORDER BY n.created_at LIMIT 1",
        )
        .bind(parent)
        .bind(author)
        .bind(client_id)
        .fetch_optional(conn)
        .await?)
    }

    /// The names of a thread's side threads, open or closed: what the next one is numbered against.
    pub async fn side_thread_names(conn: &mut Conn, parent: Id) -> Result<Vec<String>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT n.name FROM threads t JOIN nodes n ON n.id = t.node_id WHERE t.side_of = $1",
        )
        .bind(parent)
        .fetch_all(conn)
        .await?)
    }

    /// A side thread that is open and has no turn under way, with the moment it was last busy: when its
    /// last turn ended, else when it was made.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct IdleSide {
        /// The side thread.
        pub thread: Id,
        /// The thread it was taken from.
        pub parent: Id,
        /// When it was last busy.
        pub idle_since: DateTime<Utc>,
    }

    /// The open side threads that have no turn under way and nothing waiting to be said to them, oldest
    /// idle first.
    pub async fn idle_side_threads(conn: &mut Conn, limit: i64) -> Result<Vec<IdleSide>, DbError> {
        Ok(sqlx::query_as(
            "SELECT t.node_id AS thread, t.side_of AS parent,
                    COALESCE((SELECT max(u.completed_at) FROM turns u WHERE u.thread_id = t.node_id), n.created_at)
                      AS idle_since
               FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.side_of IS NOT NULL AND n.deleted_at IS NULL AND n.archived_at IS NULL
                AND t.state NOT IN ('starting', 'running', 'waiting_approval')
                AND NOT EXISTS (SELECT 1 FROM turns u WHERE u.thread_id = t.node_id
                                  AND u.state IN ('starting', 'running'))
                AND NOT EXISTS (SELECT 1 FROM thread_inputs i WHERE i.thread_id = t.node_id
                                  AND i.state IN ('pending', 'delivering'))
              ORDER BY idle_since, t.node_id LIMIT $1",
        )
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// Closes a side thread: marks its node deleted, so that it can be brought back until it is purged.
    /// The answer says whether this call closed it.
    pub async fn close_side_thread(
        conn: &mut Conn,
        side: Id,
        by: Option<Id>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let done = sqlx::query(
            "UPDATE nodes SET deleted_at = $2, deleted_by = $3, updated_at = $2
              WHERE id = $1 AND deleted_at IS NULL
                AND EXISTS (SELECT 1 FROM threads t WHERE t.node_id = nodes.id AND t.side_of IS NOT NULL)",
        )
        .bind(side)
        .bind(at)
        .bind(by)
        .execute(conn)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// The threads that stand in a directory of an environment, other than `except`: those that share its
    /// instruction file and what its agent noted. Side threads are none of them.
    pub async fn threads_in_directory_except(
        conn: &mut Conn,
        environment: Id,
        directory: &str,
        except: Id,
    ) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.environment_id = $1 AND t.workspace_dir = $2 AND t.node_id <> $3
                AND t.side_of IS NULL AND n.deleted_at IS NULL
              ORDER BY t.node_id",
        )
        .bind(environment)
        .bind(directory)
        .bind(except)
        .fetch_all(conn)
        .await?)
    }

    /// Whether a thread is a side thread, and whose.
    pub async fn side_parent(conn: &mut Conn, thread: Id) -> Result<Option<Id>, DbError> {
        Ok(
            sqlx::query_scalar("SELECT side_of FROM threads WHERE node_id = $1")
                .bind(thread)
                .fetch_optional(conn)
                .await?
                .flatten(),
        )
    }

    /// What closing a side thread found.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Closed {
        /// The thread it was taken from.
        pub parent: Id,
        /// Its name.
        pub name: String,
        /// How many turns it had.
        pub turns: i64,
    }

    /// The thread a side thread was closed into, when it was kept.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct KeptAs {
        /// The thread.
        pub thread: Id,
        /// Its name.
        pub name: String,
    }

    /// Closes a side thread: its node is marked deleted, so that it can be brought back until it is purged,
    /// and its parent's history gets the line that says so (who closed it, why, and how many turns it had;
    /// never what was said in it). Nothing happens, and none is returned, for a thread that is not an open
    /// side thread. `why` is `closed`, `idle`, `kept` or `parent`.
    pub async fn close_with_marker(
        conn: &mut Conn,
        side: Id,
        why: &str,
        by: Option<Id>,
        kept_as: Option<&KeptAs>,
        at: DateTime<Utc>,
    ) -> Result<Option<Closed>, DbError> {
        let found: Option<(Id, String)> = sqlx::query_as(
            "SELECT t.side_of, n.name FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.node_id = $1 AND t.side_of IS NOT NULL AND n.deleted_at IS NULL FOR UPDATE OF t",
        )
        .bind(side)
        .fetch_optional(&mut *conn)
        .await?;
        let Some((parent, name)) = found else {
            return Ok(None);
        };
        let turns = turn_count(conn, side).await?;
        if !close_side_thread(conn, side, by, at).await? {
            return Ok(None);
        }
        crate::queries::hierarchy::stamp(conn, side).await?;
        let segment: Option<Id> =
            sqlx::query_scalar("SELECT current_segment_id FROM threads WHERE node_id = $1")
                .bind(parent)
                .fetch_one(&mut *conn)
                .await?;
        if let Some(segment) = segment
            && let Some(seq) = crate::sequences::next(conn, parent).await?
        {
            let people = match by {
                Some(person) => super::people(conn, &[person]).await?,
                None => Vec::new(),
            };
            let by_ref = people.first().map(|person| {
                serde_json::json!({
                    "id": person.id, "kind": if person.kind == "service" { "service" } else { "human" },
                    "username": person.username, "display_name": person.display_name,
                    "avatar": {
                        "text": person.display_name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(),
                        "color": person.avatar_color,
                    }
                })
            });
            let payload = serde_json::json!({
                "marker": "side_closed",
                "data": {
                    "side": { "id": side, "kind": "thread", "name": name, "redacted": false },
                    "turns": turns, "by": by_ref, "why": why,
                    "into": kept_as.map(|kept| serde_json::json!({
                        "id": kept.thread, "kind": "thread", "name": kept.name, "redacted": false
                    })),
                }
            });
            super::insert_item(
                conn,
                &super::NewItem {
                    id: orch_types::new_id(),
                    thread_id: parent,
                    segment_id: segment,
                    turn_id: None,
                    stream: "main",
                    parent_item_id: None,
                    kind: "marker",
                    status: "completed",
                    author_id: None,
                    payload: &payload,
                    native_ref: Some(&format!("side_closed:{side}")),
                    search_text: None,
                    at,
                },
                seq,
            )
            .await?;
            super::set_seq_state(conn, parent, seq).await?;
            crate::sequences::notify(conn, parent, seq).await?;
        }
        Ok(Some(Closed {
            parent,
            name,
            turns,
        }))
    }
}
