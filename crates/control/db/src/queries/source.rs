//! Frozen source choices, explicit work observations and recorded file-state associations.

use crate::{Conn, DbError};
use orch_types::{Hash, Id, Json, Seq};

/// Replaces safe source facts and their private frozen choice under the thread's row lock.
pub async fn set_source(
    conn: &mut Conn,
    thread: Id,
    source: &Json,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE threads SET source = $2, seq_state = GREATEST(seq_state, $3) WHERE node_id = $1",
    )
    .bind(thread)
    .bind(source)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(())
}

/// The current stored token grant for a file job, without storing its credential hash in the job.
pub async fn job_token(
    conn: &mut Conn,
    token: Id,
    user: Id,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<super::identity::ApiTokenRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, user_id, scopes, node_id FROM api_tokens WHERE id = $1 AND user_id = $2
        AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > $3) FOR SHARE",
    )
    .bind(token)
    .bind(user)
    .bind(now)
    .fetch_optional(conn)
    .await?)
}

/// Only the most recent completed main answer supplies link suggestions; they are never accepted automatically.
pub async fn last_agent_message(conn: &mut Conn, thread: Id) -> Result<Option<Json>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT payload FROM items WHERE thread_id = $1 AND kind = 'agent_message'
        AND status = 'completed' AND stream = 'main' ORDER BY seq_created DESC, id DESC LIMIT 1",
    )
    .bind(thread)
    .fetch_optional(conn)
    .await?)
}

/// One declared attention projection, on the fixed `th` alias, shared by tree and finder reads.
pub const ATTENTION_SQL: &str = "CASE WHEN th.node_id IS NOT NULL THEN jsonb_build_object(
    'uncommitted', th.work_observed -> 'uncommitted', 'unpushed', th.work_observed -> 'unpushed',
    'as_of', th.work_observed -> 'as_of',
    'stale', COALESCE((th.work_observed ->> 'stale')::boolean, false) OR
        (th.work_observed IS NOT NULL AND COALESCE((SELECT e.status <> 'running' OR e.deleted_at IS NOT NULL
            OR m.id IS NULL OR NOT m.online OR m.status <> 'active' OR m.deleted_at IS NOT NULL
            FROM environments e LEFT JOIN machines m ON m.id = e.machine_id WHERE e.id = th.environment_id), true)),
    'reason', CASE WHEN th.work_observed IS NOT NULL AND COALESCE((SELECT e.status <> 'running' OR e.deleted_at IS NOT NULL
            OR m.id IS NULL OR NOT m.online OR m.status <> 'active' OR m.deleted_at IS NOT NULL
            FROM environments e LEFT JOIN machines m ON m.id = e.machine_id WHERE e.id = th.environment_id), true)
        THEN to_jsonb('source_offline'::text) ELSE th.work_observed -> 'reason' END,
    'failed', COALESCE((SELECT CASE WHEN last_turn.state = 'failed' THEN
        NOT COALESCE((SELECT max(i.seq_created) FROM thread_inputs i WHERE i.thread_id=th.node_id AND i.seq_created>0)
            > COALESCE(NULLIF(last_turn.completed_seq,0),last_turn.seq_updated),false)
        AND NOT COALESCE((SELECT max(i.created_at) FROM thread_inputs i WHERE i.thread_id=th.node_id AND i.seq_created=0)
            > last_turn.completed_at,false) ELSE false END
        FROM turns last_turn WHERE last_turn.thread_id = th.node_id ORDER BY last_turn.ordinal DESC LIMIT 1), false)) END";

/// Work flags are read together for only the authorized thread ids; no Git is invoked.
pub async fn attentions(conn: &mut Conn, threads: &[Id]) -> Result<Vec<(Id, Json)>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT th.node_id, {ATTENTION_SQL} FROM threads th WHERE th.node_id = ANY($1)"
    )))
    .bind(threads)
    .fetch_all(conn)
    .await?)
}

/// Current execution, queue, completion and read facts for one authorized thread.
#[derive(Debug, Clone, Default, sqlx::FromRow)]
pub struct ThreadDisplayFacts {
    /// The execution facts' thread sequence.
    pub seq: Seq,
    /// This reader's active-history position.
    pub read_seq: Seq,
    /// The thread's identity.
    pub id: Id,
    /// Its execution state.
    pub state: String,
    /// Its safe blocked reason.
    pub blocked_reason: Option<String>,
    /// The stable blocked-state detail used to recognize automatic recovery.
    pub blocked_detail: Option<String>,
    /// A binding or runtime job retains its existing lock precedence.
    pub recovery_has_lock: bool,
    /// The joined agent recovery's current state.
    pub recovery_state: Option<String>,
    /// The attempt within the recorded step.
    pub recovery_attempt: i64,
    /// When automatic handling may run again.
    pub recovery_due_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The recorded rule identifier.
    pub recovery_rule_id: Option<String>,
    /// The action type, without candidate parameters.
    pub recovery_step_type: Option<String>,
    /// The recorded step's attempt denominator.
    pub recovery_step_times: Option<i64>,
    /// Retained effects require a continuation.
    pub recovery_continuing: bool,
    /// The stable failure code, never the vendor message.
    pub recovery_failure_kind: Option<String>,
    /// The stable refusal or access reason.
    pub recovery_failure_reason: Option<String>,
    /// The recorded stop reason, before recognized-code projection.
    pub recovery_stop_reason: Option<String>,
    /// The repeat guard's recorded occurrences.
    pub recovery_count: Option<i64>,
    /// The repeat guard's recorded window.
    pub recovery_window_secs: Option<i64>,
    /// Whether the thread or an ancestor is archived.
    pub archived: bool,
    /// Unanswered requests that have not expired.
    pub approvals: i64,
    /// A native question or elicitation asks for an answer.
    pub waiting_for_answer: bool,
    /// Messages waiting to start, excluding personal holds.
    pub queued_inputs: i64,
    /// Messages held until Resume.
    pub held_inputs: i64,
    /// Messages waiting for a person's decision or recovery.
    pub waiting_inputs: i64,
    /// The persistent queue hold, including a hold before the next message arrives.
    pub queue_held: bool,
    /// The last turn failed with no later accepted input.
    pub failed: bool,
    /// A completed turn is ahead of this reader's read position.
    pub unread_completion: bool,
    /// The latest successful turn's completion time.
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Reads all icon facts in one statement for an already authorized batch, never one query per row.
pub async fn thread_display_facts(
    conn: &mut Conn,
    threads: &[Id],
    reader: Option<Id>,
    at: orch_types::Time,
) -> Result<Vec<ThreadDisplayFacts>, DbError> {
    if threads.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(
        "WITH pending AS MATERIALIZED (
          SELECT a.thread_id,count(*) approvals,
            bool_or(a.request->>'request_kind' IN ('user_input','mcp_elicitation')) waiting_for_answer
           FROM approvals a JOIN threads selected ON selected.node_id=a.thread_id
           WHERE a.thread_id=ANY($1) AND selected.node_id=ANY($1) AND a.state='pending' AND (a.expires_at IS NULL OR a.expires_at>$3)
            AND (a.turn_id IS NULL OR EXISTS(SELECT 1 FROM turns t WHERE t.id=a.turn_id AND t.continuation_id=selected.active_continuation_id))
           GROUP BY a.thread_id
         ), scheduled AS MATERIALIZED (
          SELECT s.target->>'thread_id' thread_id,count(*) queued_inputs FROM input_schedules s
           WHERE s.state='active' AND s.deleted_at IS NULL AND s.target->>'kind'='thread'
            AND s.target->>'thread_id' IN (SELECT id::text FROM unnest($1::uuid[]) id)
           GROUP BY s.target->>'thread_id'
         )
         SELECT th.node_id id, th.next_seq-1 seq, COALESCE(r.last_read_seq,0) read_seq, th.state, th.blocked_reason, th.blocked_detail, th.queue_held,
          CASE WHEN th.state='blocked' AND th.blocked_reason='error' AND th.blocked_detail='call_recovery'
           AND call_recovery.state IN ('scheduled','running','stopped','waiting')
           THEN EXISTS(SELECT 1 FROM jobs j WHERE __RECOVERY_HOLD__) ELSE false END recovery_has_lock,
          call_recovery.state recovery_state,
          COALESCE((call_recovery.action->>'attempt')::bigint,0) recovery_attempt,
          call_recovery.due_at recovery_due_at, call_recovery.rule->>'id' recovery_rule_id,
          call_recovery.action#>>'{step,type}' recovery_step_type,
          (call_recovery.action#>>'{step,times}')::bigint recovery_step_times,
          COALESCE((call_recovery.context->>'effects')::boolean,false) recovery_continuing,
          call_recovery.failure->>'code' recovery_failure_kind,
          call_recovery.failure->>'reason' recovery_failure_reason,
          COALESCE(call_recovery.context->>'detail',call_recovery.context->>'skipped',call_recovery.action->>'detail') recovery_stop_reason,
          (call_recovery.action->>'count')::bigint recovery_count,
          (call_recovery.action->>'window_secs')::bigint recovery_window_secs,
          EXISTS(SELECT 1 FROM nodes ancestor WHERE ancestor.id=ANY(n.path) AND ancestor.archived_at IS NOT NULL) archived,
          COALESCE(pending.approvals,0) approvals, COALESCE(pending.waiting_for_answer,false) waiting_for_answer,
          inputs.queued_inputs + COALESCE(scheduled.queued_inputs,0) queued_inputs,
          inputs.held_inputs, inputs.waiting_inputs,
          CASE WHEN last_turn.state='failed' THEN
            NOT COALESCE((SELECT max(i.seq_created) FROM thread_inputs i WHERE i.thread_id=th.node_id AND i.seq_created>0)
              > COALESCE(NULLIF(last_turn.completed_seq,0),last_turn.seq_updated),false)
            AND NOT COALESCE((SELECT max(i.created_at) FROM thread_inputs i WHERE i.thread_id=th.node_id AND i.seq_created=0)
              > last_turn.completed_at,false) ELSE false END failed,
          COALESCE($2::uuid IS NOT NULL AND completed.completed_seq>COALESCE(r.last_read_seq,0),false) unread_completion,
          completed.completed_at finished_at
         FROM threads th JOIN nodes n ON n.id=th.node_id
         LEFT JOIN model_call_recovery call_recovery ON call_recovery.node_id=th.node_id
          AND call_recovery.purpose='agent' AND call_recovery.call_key=th.node_id
          AND call_recovery.state NOT IN ('completed','dismissed')
         LEFT JOIN pending ON pending.thread_id=th.node_id
         LEFT JOIN scheduled ON scheduled.thread_id=th.node_id::text
         LEFT JOIN thread_reads r ON r.user_id=$2 AND r.thread_id=th.node_id AND r.continuation_id=th.active_continuation_id
         LEFT JOIN LATERAL (SELECT t.state,t.completed_seq,t.seq_updated,t.completed_at FROM turns t
           WHERE t.thread_id=th.node_id ORDER BY t.ordinal DESC LIMIT 1) last_turn ON true
         LEFT JOIN LATERAL (SELECT t.completed_seq,t.completed_at FROM turns t
           WHERE t.thread_id=th.node_id AND t.continuation_id=th.active_continuation_id AND t.state='completed'
           ORDER BY t.ordinal DESC LIMIT 1) completed ON true
         CROSS JOIN LATERAL (SELECT
           count(*) FILTER(WHERE NOT i.queue_held AND COALESCE(i.state_detail,'') NOT IN
             ('model_use_required','attachments','second_step_required','owner_lacks_access') AND NOT starts_with(COALESCE(i.state_detail,''),'native_action.')) queued_inputs,
           count(*) FILTER(WHERE i.queue_held) held_inputs,
           count(*) FILTER(WHERE NOT i.queue_held AND (i.state_detail IN
             ('model_use_required','attachments','second_step_required','owner_lacks_access') OR starts_with(i.state_detail,'native_action.'))) waiting_inputs
          FROM thread_inputs i WHERE i.thread_id=th.node_id AND i.continuation_id=th.active_continuation_id
            AND i.state IN ('pending','delivering')) inputs
         WHERE th.node_id=ANY($1)"
            .replace("__RECOVERY_HOLD__", &super::jobs::changes::thread_hold_predicate("th.node_id")),
    )).bind(threads).bind(reader).bind(at.get()).fetch_all(conn).await?)
}

/// A keyset page of contained metadata, before the caller applies each thread's View door.
pub async fn topic_threads_page(
    conn: &mut Conn,
    topics: &[Id],
    after: Option<(Id, Id)>,
    limit: i64,
) -> Result<Vec<(Id, Id, bool)>, DbError> {
    if let Some((topic, thread)) = after {
        Ok(sqlx::query_as("SELECT th.topic_id, th.node_id, th.side_of IS NULL FROM threads th JOIN nodes n ON n.id=th.node_id
            WHERE th.topic_id=ANY($1) AND (th.topic_id,th.node_id)>($2,$3) AND n.deleted_at IS NULL
            ORDER BY th.topic_id,th.node_id LIMIT $4")
            .bind(topics).bind(topic).bind(thread).bind(limit).fetch_all(conn).await?)
    } else {
        Ok(sqlx::query_as("SELECT th.topic_id, th.node_id, th.side_of IS NULL FROM threads th JOIN nodes n ON n.id=th.node_id
            WHERE th.topic_id=ANY($1) AND n.deleted_at IS NULL ORDER BY th.topic_id,th.node_id LIMIT $2")
            .bind(topics).bind(limit).fetch_all(conn).await?)
    }
}

/// SQL totals for an already authorized page of main threads; no per-thread flags enter the caller's memory.
pub async fn topic_attention_counts(
    conn: &mut Conn,
    threads: &[Id],
) -> Result<Vec<(Id, i64, i64, i64)>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH flags AS MATERIALIZED (SELECT th.topic_id, {ATTENTION_SQL} value FROM threads th WHERE th.node_id=ANY($1))
         SELECT topic_id,count(*) FILTER(WHERE value->'uncommitted'='true'::jsonb),
           count(*) FILTER(WHERE value->'unpushed'='true'::jsonb),count(*) FILTER(WHERE value->'failed'='true'::jsonb)
         FROM flags GROUP BY topic_id"
    ))).bind(threads).fetch_all(conn).await?)
}

/// The next turn number while the thread's row is held, before a turn or lease exists.
pub async fn next_turn_ordinal(conn: &mut Conn, thread: Id) -> Result<i32, DbError> {
    Ok(
        sqlx::query_scalar("SELECT COALESCE(MAX(ordinal), 0) + 1 FROM turns WHERE thread_id = $1")
            .bind(thread)
            .fetch_one(conn)
            .await?,
    )
}

/// Reserves the recorded state before the command, without turning it into a manual snapshot.
pub async fn begin_recorded(
    conn: &mut Conn,
    new: &super::storage::NewSnapshot<'_>,
    directory: &str,
    kind: &str,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO snapshots (id, thread_id, environment_id, env_generation, kind, storage,
        directory, label, selection, state, created_by, created_at)
        VALUES ($1, $2, $3, $4, $5, 'environment', $6, $7, $8, 'running', $9, $10) ON CONFLICT (id) DO NOTHING")
        .bind(new.id).bind(new.thread_id).bind(new.environment_id).bind(new.env_generation).bind(kind)
        .bind(directory).bind(new.label).bind(new.selection).bind(new.created_by).bind(new.at).execute(conn).await?;
    Ok(())
}

/// Finishes one reserved immutable state once; only its manifest is a server blob pin.
pub async fn finish_recorded(
    conn: &mut Conn,
    snapshot: Id,
    result: &orch_link_contract::source::FsRecordResult,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<bool, DbError> {
    let state = if result.state == orch_link_contract::source::RecordState::Complete {
        "complete"
    } else {
        "unavailable"
    };
    Ok(sqlx::query("UPDATE snapshots SET state = $2, tree_ref = $3, repository_head = $4,
        manifest_blob = $5, file_count = $6, total_bytes = $7, missing_count = 0,
        expires_at = $8, unavailable_reason = $9, completed_at = $10 WHERE id = $1 AND state = 'running'")
        .bind(snapshot).bind(state).bind(&result.tree_ref).bind(&result.repository_head)
        .bind(result.manifest_blob.map(|hash| hash.as_bytes().to_vec())).bind(result.files).bind(result.bytes)
        .bind(result.expires_at.map(|at| at.get())).bind(&result.reason).bind(at).execute(conn).await?.rows_affected() == 1)
}

/// Replaces accepted labelled links while the caller holds the thread's row and action guard.
pub async fn set_links(
    conn: &mut Conn,
    thread: Id,
    links: &Json,
    seq: Seq,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE threads SET links = $2, seq_state = GREATEST(seq_state, $3) WHERE node_id = $1",
    )
    .bind(thread)
    .bind(links)
    .bind(seq)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// Whether the latest turn failed without a later accepted input, preserving a held queue.
pub async fn unresolved_last_failure(conn: &mut Conn, thread: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE((SELECT CASE WHEN last_turn.state='failed' THEN
            NOT COALESCE((SELECT max(i.seq_created) FROM thread_inputs i WHERE i.thread_id=$1 AND i.seq_created>0)
              > COALESCE(NULLIF(last_turn.completed_seq,0),last_turn.seq_updated),false)
            AND NOT COALESCE((SELECT max(i.created_at) FROM thread_inputs i WHERE i.thread_id=$1 AND i.seq_created=0)
              > last_turn.completed_at,false) ELSE false END
            FROM turns last_turn WHERE last_turn.thread_id = $1 ORDER BY last_turn.ordinal DESC LIMIT 1), false)",
    ).bind(thread).fetch_one(conn).await?)
}

/// Recorded-state owning environments, for a per-entry authorization filter before pagination.
pub async fn snapshot_environments(conn: &mut Conn, thread: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT DISTINCT environment_id FROM snapshots WHERE thread_id = $1 AND storage = 'environment'")
        .bind(thread).fetch_all(conn).await?)
}

/// Keeps a turn's referenced metadata while releasing its server manifest association once.
pub async fn expire_recorded(
    conn: &mut Conn,
    snapshot: Id,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<Option<Hash>>, DbError> {
    let raw: Option<Option<Vec<u8>>> = sqlx::query_scalar("WITH prior AS (
        SELECT id, manifest_blob FROM snapshots WHERE id = $1 AND storage = 'environment' AND state <> 'running' FOR UPDATE
        ), changed AS (UPDATE snapshots s SET state = 'expired', unavailable_reason = 'expired', manifest_blob = NULL,
            expires_at = LEAST(COALESCE(s.expires_at, $2), $2) FROM prior p WHERE s.id = p.id RETURNING p.manifest_blob)
        SELECT manifest_blob FROM changed")
        .bind(snapshot).bind(now).fetch_optional(&mut *conn).await?;
    if raw.is_some() {
        sqlx::query("DELETE FROM snapshot_blob_refs WHERE snapshot_id=$1")
            .bind(snapshot)
            .execute(&mut *conn)
            .await?;
    }
    raw.map(|hash| {
        hash.map(|bytes| {
            Hash::from_slice(&bytes).map_err(|_| {
                DbError::Other(sqlx::Error::Decode(
                    "a recorded manifest hash is not 32 bytes".into(),
                ))
            })
        })
        .transpose()
    })
    .transpose()
}

/// The last explicitly requested Git facts; watcher observation never replaces them with guesses.
pub async fn set_work(
    conn: &mut Conn,
    thread: Id,
    observation: &Json,
    seq: Seq,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE threads SET work_observed = $2, seq_state = GREATEST(seq_state, $3) WHERE node_id = $1")
        .bind(thread).bind(observation).bind(seq).execute(conn).await?.rows_affected() == 1)
}

/// Reserves a requested observation without replacing the previous public facts.
pub async fn reserve_work(
    conn: &mut Conn,
    thread: Id,
    request: Id,
    at: orch_types::Time,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query("UPDATE threads SET work_observed = jsonb_set(COALESCE(work_observed,
        jsonb_build_object('uncommitted',NULL,'unpushed',NULL,'changed_files',NULL,'ahead',NULL,'as_of',to_jsonb($3::timestamptz),
            'stale',true,'reason','not_observed')), '{_requested}', to_jsonb($2::text)) - '_requested_changed' - '_requested_reason',
        seq_state = GREATEST(seq_state, $4) WHERE node_id = $1")
        .bind(thread).bind(request.to_string()).bind(at.get()).bind(seq).execute(conn).await?;
    Ok(())
}

/// An older or replayed answer never replaces a later request, even when the wall clock is held.
pub async fn finish_work(
    conn: &mut Conn,
    thread: Id,
    request: Id,
    observation: &Json,
    seq: Seq,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE threads SET work_observed = CASE WHEN work_observed->>'_requested_changed' = 'true'
        THEN jsonb_set(jsonb_set($3::jsonb,'{stale}','true'),'{reason}',COALESCE(work_observed->'_requested_reason','\"files_changed\"'::jsonb)) ELSE $3::jsonb END,
        seq_state = GREATEST(seq_state, $4) WHERE node_id = $1 AND work_observed->>'_requested' = $2")
        .bind(thread).bind(request.to_string()).bind(observation).bind(seq).execute(conn).await?.rows_affected() == 1)
}

/// Threads using exactly these files; a different nested repository is never guessed from a prefix.
pub async fn work_threads(
    conn: &mut Conn,
    environment: Id,
    directory: &str,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
        WHERE t.environment_id = $1 AND t.workspace_dir = $2 AND n.deleted_at IS NULL ORDER BY t.node_id")
        .bind(environment).bind(directory).fetch_all(conn).await?)
}

/// Distinct thread directories in a repository's subtree, for an explicit Git act's fresh reads.
pub async fn work_directories(
    conn: &mut Conn,
    environment: Id,
    repository: &str,
) -> Result<Vec<String>, DbError> {
    Ok(sqlx::query_scalar("SELECT DISTINCT t.workspace_dir FROM threads t JOIN nodes n ON n.id = t.node_id
        WHERE t.environment_id = $1 AND n.deleted_at IS NULL
          AND ($2 = '' OR t.workspace_dir = $2 OR starts_with(t.workspace_dir, $2 || '/')) ORDER BY t.workspace_dir")
        .bind(environment).bind(repository).fetch_all(conn).await?)
}

/// Included watcher changes make the previous requested facts stale; they never invent new Git flags.
pub async fn stale_work(
    conn: &mut Conn,
    thread: Id,
    at: chrono::DateTime<chrono::Utc>,
    seq: Seq,
) -> Result<(), DbError> {
    stale_work_reason(conn, thread, at, seq, "files_changed").await
}

/// Keeps the reason for a known change or unknown coverage through an in-flight requested read.
pub async fn stale_work_reason(
    conn: &mut Conn,
    thread: Id,
    at: chrono::DateTime<chrono::Utc>,
    seq: Seq,
    reason: &str,
) -> Result<(), DbError> {
    sqlx::query("UPDATE threads SET work_observed = jsonb_set(jsonb_set(work_observed, '{stale}', 'true'),
        '{reason}', to_jsonb($4::text)) || CASE WHEN work_observed ? '_requested' THEN jsonb_build_object('_requested_changed',true,'_requested_reason',$4::text) ELSE '{}'::jsonb END,
        seq_state = GREATEST(seq_state, $3)
        WHERE node_id = $1 AND work_observed IS NOT NULL AND (work_observed->>'as_of')::timestamptz <= $2")
        .bind(thread).bind(at).bind(seq).bind(reason).execute(conn).await?;
    Ok(())
}

/// Records a deferred invalidation without locking another thread inside this turn's transaction.
pub async fn work_pending(conn: &mut Conn, turn: Id, pending: bool) -> Result<(), DbError> {
    sqlx::query("UPDATE turns SET files_work_pending = $2 WHERE id = $1")
        .bind(turn)
        .bind(pending)
        .execute(conn)
        .await?;
    Ok(())
}

/// Leaves the end observation of `turn` to after the commit of the transaction that ends it:
/// the machine is asked with no transaction open, and the thread's next turn waits for the answer.
pub async fn end_due(conn: &mut Conn, turn: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE turns SET files_end_due = true WHERE id = $1")
        .bind(turn)
        .execute(conn)
        .await?;
    Ok(())
}

/// Takes the due end observation of `turn`, once: says whether it was still due. The caller holds the
/// thread's lock, so two processes never write one observation twice.
pub async fn take_end_due(conn: &mut Conn, turn: Id) -> Result<bool, DbError> {
    Ok(
        sqlx::query("UPDATE turns SET files_end_due = false WHERE id = $1 AND files_end_due")
            .bind(turn)
            .execute(conn)
            .await?
            .rows_affected()
            == 1,
    )
}

/// The oldest turn of `thread` whose end observation is due, if one is.
pub async fn end_due_turn(conn: &mut Conn, thread: Id) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM turns WHERE thread_id = $1 AND files_end_due ORDER BY ordinal LIMIT 1",
    )
    .bind(thread)
    .fetch_optional(conn)
    .await?)
}

/// The threads that have a turn whose end observation is due, for the engine's sweep: a process that
/// stopped between a turn's end and its observation leaves them to the next.
pub async fn threads_with_end_due(conn: &mut Conn, limit: i64) -> Result<Vec<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT DISTINCT thread_id FROM turns WHERE files_end_due LIMIT $1")
            .bind(limit)
            .fetch_all(conn)
            .await?,
    )
}

/// Keeps a turn's before-state association, allowing an unavailable state's explained fallback.
pub async fn set_before_state(conn: &mut Conn, turn: Id, snapshot: Id) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE turns SET before_snapshot_id = $2 WHERE id = $1 AND before_snapshot_id IS NULL",
    )
    .bind(turn)
    .bind(snapshot)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// Keeps complete end hashes or explicit unknown coverage before this thread's next turn.
pub async fn set_end_manifest(
    conn: &mut Conn,
    turn: Id,
    manifest: Option<&Json>,
    blob: Option<&Hash>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE turns SET files_end_manifest = $2, end_manifest_blob = $3 WHERE id = $1")
        .bind(turn)
        .bind(manifest)
        .bind(blob.map(|hash| hash.as_bytes().to_vec()))
        .execute(conn)
        .await?;
    Ok(())
}
