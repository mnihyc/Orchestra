//! Indexed continuation ownership, immutable prefix membership and operation receipts.
//!
//! Callers hold the thread row for mutations. Every function uses their one
//! connection; no machine, timer, provider or second pooled connection is used.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Hash, Id, Seq};

pub mod reading;

pub const JOB_KIND: &str = "thread_continuation";

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ContinuationRow {
    pub id: Id,
    pub thread_id: Id,
    pub parent_id: Option<Id>,
    pub before_input_id: Option<Id>,
    pub status: String,
    pub created_by: Option<Id>,
    pub created_at: DateTime<Utc>,
    pub created_seq: Seq,
    pub current_segment_id: Option<Id>,
    pub projection_revision: i64,
    pub queue_held: bool,
    pub queue_held_by: Option<Id>,
    pub queue_held_at: Option<DateTime<Utc>>,
    pub queue_revision: i64,
    pub job_id: Option<Id>,
    pub native_source: Option<orch_types::Json>,
    pub native_prefix: Option<orch_types::Json>,
    pub open_result: Option<orch_types::Json>,
    pub native_creation: Option<orch_types::Json>,
    pub native_creation_result: Option<orch_types::Json>,
    pub failure: Option<orch_types::Json>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Projection {
    pub thread_id: Id,
    pub viewed_continuation_id: Id,
    pub active_continuation_id: Id,
    pub execution_revision: i64,
    pub projection_revision: i64,
    pub follows_active: bool,
    pub viewed_segment_id: Option<Id>,
}

impl Projection {
    /// Old clients can use their original root cursor; a changed selection may never guess its scope.
    pub fn accepts_cursor(&self, positioned: bool, revision: Option<i64>) -> bool {
        if let Some(revision) = revision {
            return revision == self.projection_revision;
        }
        !positioned || self.follows_active && self.execution_revision == 1
    }
}

pub async fn projection(
    conn: &mut Conn,
    thread: Id,
    viewed: Option<Id>,
) -> Result<Option<Projection>, DbError> {
    Ok(sqlx::query_as("SELECT t.node_id AS thread_id,c.id AS viewed_continuation_id,
        t.active_continuation_id,t.execution_revision,
        CASE WHEN $2::uuid IS NULL THEN t.execution_revision ELSE c.projection_revision END AS projection_revision,
        ($2::uuid IS NULL) AS follows_active,c.current_segment_id AS viewed_segment_id
        FROM threads t JOIN thread_continuations c ON c.thread_id=t.node_id
            AND c.id=COALESCE($2,t.active_continuation_id) WHERE t.node_id=$1")
        .bind(thread).bind(viewed).fetch_optional(conn).await?)
}

pub async fn continuation(
    conn: &mut Conn,
    thread: Id,
    id: Id,
) -> Result<Option<ContinuationRow>, DbError> {
    Ok(
        sqlx::query_as("SELECT * FROM thread_continuations WHERE thread_id=$1 AND id=$2")
            .bind(thread)
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct QueueRow {
    pub held: bool,
    pub held_by: Option<Id>,
    pub held_at: Option<DateTime<Utc>>,
    pub revision: i64,
}

pub struct NewContinuation {
    pub id: Id,
    pub thread: Id,
    pub parent: Id,
    pub before_input: Id,
    pub actor: Id,
    pub at: DateTime<Utc>,
    pub seq: Seq,
    pub queue: QueueRow,
}

pub async fn insert(conn: &mut Conn, new: &NewContinuation) -> Result<ContinuationRow, DbError> {
    Ok(sqlx::query_as(
        "INSERT INTO thread_continuations(id,thread_id,parent_id,before_input_id,status,
        created_by,created_at,created_seq,queue_held,queue_held_by,queue_held_at,queue_revision)
        VALUES($1,$2,$3,$4,'preparing',$5,$6,$7,$8,$9,$10,$11) RETURNING *",
    )
    .bind(new.id)
    .bind(new.thread)
    .bind(new.parent)
    .bind(new.before_input)
    .bind(new.actor)
    .bind(new.at)
    .bind(new.seq)
    .bind(new.queue.held)
    .bind(new.queue.held_by)
    .bind(new.queue.held_at)
    .bind(new.queue.revision)
    .fetch_one(conn)
    .await?)
}

/// A prompt/version lookup reads metadata only, not its full content or provider permissions.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PromptRow {
    pub input_id: Id,
    pub group_id: Id,
    pub continuation_id: Id,
    pub ordinal: i64,
    pub total: i64,
    pub author_id: Option<Id>,
    pub source: String,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub first_of_turn: bool,
    pub has_native_item: bool,
}

/// Only actual returned item input IDs enter this batch. Counts use indexed groups, not delivered bodies.
pub async fn prompt_summaries(
    conn: &mut Conn,
    thread: Id,
    ids: &[Id],
) -> Result<Vec<PromptRow>, DbError> {
    Ok(sqlx::query_as("WITH wanted AS (
        SELECT i.id AS input_id,COALESCE(v.group_id,i.id) AS group_id,i.continuation_id,
          COALESCE(v.ordinal,1::bigint) AS ordinal,i.author_id,i.source,i.state,i.created_at,i.turn_id
        FROM thread_inputs i LEFT JOIN thread_prompt_versions v ON v.input_id=i.id
        WHERE i.thread_id=$1 AND i.id=ANY($2))
      SELECT w.input_id,w.group_id,w.continuation_id,w.ordinal,COALESCE(g.total,1::bigint) AS total,
        w.author_id,w.source,w.state,w.created_at,
        COALESCE(t.input_id=w.input_id,false) AS first_of_turn,
        EXISTS(SELECT 1 FROM items x WHERE x.thread_id=$1 AND x.turn_id=w.turn_id AND x.kind='user_message'
          AND x.payload->>'input_id'=w.input_id::text AND x.native_ref IS NOT NULL AND NOT x.before_orch) AS has_native_item
      FROM wanted w LEFT JOIN thread_prompt_groups g ON g.id=w.group_id AND g.thread_id=$1
        LEFT JOIN turns t ON t.id=w.turn_id AND t.thread_id=$1
      ORDER BY w.input_id")
        .bind(thread).bind(ids).fetch_all(conn).await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct VersionRow {
    pub input_id: Id,
    pub group_id: Id,
    pub continuation_id: Id,
    pub ordinal: i64,
    pub author_id: Option<Id>,
    pub created_at: DateTime<Utc>,
    pub status: String,
    pub job_id: Option<Id>,
    pub job_snapshot: Option<orch_types::Json>,
}

/// The viewed child's own edit, excluding original singleton rows it owns for other groups.
pub async fn viewed_version(
    conn: &mut Conn,
    thread: Id,
    view: Id,
) -> Result<Option<VersionRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT v.input_id,v.group_id,v.continuation_id,v.ordinal,i.author_id,i.created_at,
        c.status,v.job_id,o.job_snapshot FROM thread_prompt_versions v
        JOIN thread_inputs i ON i.id=v.input_id AND i.thread_id=$1
        JOIN thread_continuations c ON c.id=v.continuation_id AND c.thread_id=$1
        LEFT JOIN thread_continuation_operations o ON o.id=v.operation_id
        WHERE v.continuation_id=$2 AND v.input_id<>v.group_id",
    )
    .bind(thread)
    .bind(view)
    .fetch_optional(conn)
    .await?)
}

/// Exact delivered anchors for a bounded version page (input, viewed continuation).
/// The ordinary user-message key uses its native index; retained/imported rows may use
/// the indexed actual input member only when it identifies exactly one visible row.
/// Pending placeholders and ambiguous data are never presented as delivered items.
pub async fn prompt_anchors(
    conn: &mut Conn,
    thread: Id,
    wanted: &[(Id, Id)],
) -> Result<std::collections::HashMap<Id, Id>, DbError> {
    if wanted.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let inputs: Vec<_> = wanted.iter().map(|(input, _)| *input).collect();
    let views: Vec<_> = wanted.iter().map(|(_, view)| *view).collect();
    let found:Vec<(Id,Id)>=sqlx::query_as("WITH wanted AS(SELECT * FROM unnest($2::uuid[],$3::uuid[]) AS w(input_id,view)),
      visible AS NOT MATERIALIZED(SELECT w.input_id,w.view,i.turn_id,i.continuation_id,t.segment_id,t.ordinal
        FROM wanted w JOIN thread_continuations c ON c.thread_id=$1 AND c.id=w.view
        JOIN thread_inputs i ON i.thread_id=$1 AND i.id=w.input_id AND i.state='delivered'
        JOIN turns t ON t.id=i.turn_id AND t.thread_id=$1 AND t.continuation_id=i.continuation_id)
      SELECT w.input_id,COALESCE(native.id,CASE WHEN retained.n=1 THEN retained.id END) AS id FROM visible w
      LEFT JOIN LATERAL(SELECT x.id FROM items x WHERE x.thread_id=$1 AND x.segment_id=w.segment_id
        AND x.native_ref='user:'||w.input_id::text AND x.kind='user_message' AND x.stream='main'
        AND x.status='completed' AND x.payload->>'input_id'=w.input_id::text AND x.turn_id=w.turn_id
        AND x.continuation_id=w.continuation_id
        AND (x.continuation_id=w.view OR EXISTS(SELECT 1 FROM thread_continuation_spans s
          WHERE s.continuation_id=w.view AND s.owner_continuation_id=x.continuation_id
            AND x.seq_created BETWEEN s.first_seq AND s.last_seq AND w.ordinal BETWEEN s.first_turn AND s.last_turn))) native ON true
      LEFT JOIN LATERAL(SELECT (array_agg(candidate.id))[1] AS id,count(*) AS n FROM (
        SELECT x.id FROM items x WHERE native.id IS NULL AND x.thread_id=$1 AND x.kind='user_message'
          AND x.payload->>'input_id'=w.input_id::text AND x.stream='main' AND x.status='completed'
          AND x.turn_id=w.turn_id AND x.continuation_id=w.continuation_id
          AND (x.continuation_id=w.view OR EXISTS(SELECT 1 FROM thread_continuation_spans s
            WHERE s.continuation_id=w.view AND s.owner_continuation_id=x.continuation_id
              AND x.seq_created BETWEEN s.first_seq AND s.last_seq AND w.ordinal BETWEEN s.first_turn AND s.last_turn))
        LIMIT 2) candidate) retained ON true
      WHERE COALESCE(native.id,CASE WHEN retained.n=1 THEN retained.id END) IS NOT NULL")
        .bind(thread).bind(inputs).bind(views).fetch_all(conn).await?;
    Ok(found.into_iter().collect())
}

pub async fn versions(
    conn: &mut Conn,
    thread: Id,
    group: Id,
    boundary: Option<i64>,
    forward: bool,
    limit: i64,
) -> Result<Vec<VersionRow>, DbError> {
    let order = if forward { "ASC" } else { "DESC" };
    let comparison = if forward { ">" } else { "<" };
    let sql = format!(
        "SELECT * FROM (SELECT v.input_id,v.group_id,v.continuation_id,v.ordinal,
        i.author_id,i.created_at,c.status,v.job_id,o.job_snapshot FROM thread_prompt_versions v
        JOIN thread_inputs i ON i.id=v.input_id AND i.thread_id=$1
        JOIN thread_continuations c ON c.id=v.continuation_id AND c.thread_id=$1
        LEFT JOIN thread_continuation_operations o ON o.id=v.operation_id
        WHERE v.group_id=$2 AND ($3::bigint IS NULL OR v.ordinal {comparison} $3)
        ORDER BY v.ordinal {order} LIMIT $4) page ORDER BY ordinal"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(group)
        .bind(boundary)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// The thread lock serializes group creation and ordinals; original identity remains version one.
pub async fn add_version(
    conn: &mut Conn,
    thread: Id,
    original: Id,
    input: Id,
    continuation: Id,
    job: Id,
    seq: Seq,
) -> Result<(Id, i64), DbError> {
    let group: Id = sqlx::query_scalar(
        "SELECT COALESCE(v.group_id,i.id) FROM thread_inputs i
        LEFT JOIN thread_prompt_versions v ON v.input_id=i.id WHERE i.id=$1 AND i.thread_id=$2",
    )
    .bind(original)
    .bind(thread)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query("INSERT INTO thread_prompt_groups(id,thread_id,total,seq_updated) VALUES($1,$2,1,$3) ON CONFLICT(id) DO NOTHING")
        .bind(group).bind(thread).bind(seq).execute(&mut *conn).await?;
    sqlx::query("INSERT INTO thread_prompt_versions(input_id,group_id,continuation_id,ordinal)
        SELECT id,id,continuation_id,1 FROM thread_inputs WHERE id=$1 AND thread_id=$2 ON CONFLICT(input_id) DO NOTHING")
        .bind(group).bind(thread).execute(&mut *conn).await?;
    let ordinal: i64 = sqlx::query_scalar(
        "UPDATE thread_prompt_groups SET total=total+1,seq_updated=$2 WHERE id=$1 RETURNING total",
    )
    .bind(group)
    .bind(seq)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query("INSERT INTO thread_prompt_versions(input_id,group_id,continuation_id,ordinal,job_id,operation_id) VALUES($1,$2,$3,$4,$5,$5)")
        .bind(input).bind(group).bind(continuation).bind(ordinal).bind(job).execute(conn).await?;
    Ok((group, ordinal))
}

/// Flatten once at admission; a read never walks ancestors or duplicates payloads.
pub async fn freeze_prefix(
    conn: &mut Conn,
    thread: Id,
    source: Id,
    target: Id,
    turn: i32,
    item: Seq,
    at: Seq,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO thread_continuation_spans(continuation_id,owner_continuation_id,
        first_turn,last_turn,first_seq,last_seq,frozen_at)
        SELECT $3,s.owner_continuation_id,s.first_turn,LEAST(s.last_turn,$4-1),
          s.first_seq,LEAST(s.last_seq,$5-1),s.frozen_at
        FROM thread_continuation_spans s JOIN thread_continuations c ON c.id=s.continuation_id AND c.thread_id=$1
        WHERE s.continuation_id=$2 AND s.first_turn-1<=$4-1 AND s.first_seq<=$5-1
        UNION ALL SELECT $3,c.id,1,$4-1,0,$5-1,$6
        FROM thread_continuations c WHERE c.id=$2 AND c.thread_id=$1")
        .bind(thread).bind(source).bind(target).bind(turn).bind(item).bind(at).execute(conn).await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OperationRow {
    pub id: Id,
    pub job_id: Option<Id>,
    pub job_snapshot: orch_types::Json,
    pub thread_id: Id,
    pub actor_id: Id,
    pub client_id: String,
    pub body_hash: Vec<u8>,
    pub kind: String,
    pub source_continuation_id: Id,
    pub target_continuation_id: Id,
    pub input_id: Option<Id>,
    pub segment_id: Id,
    pub expected_active_continuation_id: Id,
    pub expected_execution_revision: i64,
    pub expected_queue_revision: i64,
    pub status: String,
    pub activation_seq: Option<Seq>,
    pub created_at: DateTime<Utc>,
}

impl OperationRow {
    pub fn same_body(&self, hash: &Hash) -> bool {
        self.body_hash.as_slice() == hash.as_bytes()
    }
}

pub async fn replay(
    conn: &mut Conn,
    thread: Id,
    actor: Id,
    client: &str,
) -> Result<Option<OperationRow>, DbError> {
    Ok(sqlx::query_as("SELECT * FROM thread_continuation_operations WHERE thread_id=$1 AND actor_id=$2 AND client_id=$3")
        .bind(thread).bind(actor).bind(client).fetch_optional(conn).await?)
}

pub async fn operation(conn: &mut Conn, id: Id) -> Result<Option<OperationRow>, DbError> {
    Ok(
        sqlx::query_as("SELECT * FROM thread_continuation_operations WHERE id=$1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

pub async fn create_operation(conn: &mut Conn, row: &OperationRow) -> Result<(), DbError> {
    sqlx::query("INSERT INTO thread_continuation_operations(id,thread_id,actor_id,client_id,body_hash,kind,
        source_continuation_id,target_continuation_id,input_id,segment_id,expected_active_continuation_id,
        expected_execution_revision,expected_queue_revision,status,created_at,job_id,job_snapshot)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,'preparing',$14,$15,$16)")
        .bind(row.id).bind(row.thread_id).bind(row.actor_id).bind(&row.client_id).bind(&row.body_hash).bind(&row.kind)
        .bind(row.source_continuation_id).bind(row.target_continuation_id).bind(row.input_id).bind(row.segment_id)
        .bind(row.expected_active_continuation_id).bind(row.expected_execution_revision).bind(row.expected_queue_revision)
        .bind(row.created_at).bind(row.job_id).bind(&row.job_snapshot).execute(&mut *conn).await?;
    sqlx::query("UPDATE thread_continuations SET job_id=$2 WHERE id=$1 AND thread_id=$3")
        .bind(row.target_continuation_id)
        .bind(row.id)
        .bind(row.thread_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Only this operation's retained intent is released; a Stop lives on its owning continuation.
pub async fn fail(conn: &mut Conn, operation: Id, cancelled: bool) -> Result<(), DbError> {
    sqlx::query("UPDATE thread_continuation_operations SET status=CASE WHEN $2 THEN 'cancelled' ELSE 'failed' END
        WHERE id=$1 AND status='preparing'").bind(operation).bind(cancelled).execute(conn).await?;
    Ok(())
}

pub async fn mark_failed_version(conn: &mut Conn, operation: Id) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE thread_continuations c SET status='failed' FROM thread_continuation_operations o
        WHERE o.id=$1 AND c.id=o.target_continuation_id AND c.status='preparing'",
    )
    .bind(operation)
    .execute(conn)
    .await?;
    Ok(())
}

/// This is the only selected-pointer commit; native verification/authority/claim happen before it.
pub async fn activate(
    conn: &mut Conn,
    operation: &OperationRow,
    segment: Id,
    seq: Seq,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    let changed=sqlx::query("UPDATE threads SET active_continuation_id=$2,execution_revision=execution_revision+1,
        current_segment_id=$3,current_turn_id=NULL,
        state=CASE WHEN state='blocked' AND blocked_reason<>'switching' THEN 'blocked' ELSE 'idle' END,
        blocked_reason=CASE WHEN blocked_reason<>'switching' THEN blocked_reason ELSE NULL END,
        blocked_detail=CASE WHEN blocked_reason<>'switching' THEN blocked_detail ELSE NULL END,
        starting_step=NULL,native_catalog=NULL,seq_state=$4
        WHERE node_id=$1 AND active_continuation_id=$5 AND execution_revision=$6 AND queue_revision=$7
          AND current_turn_id IS NULL AND EXISTS(SELECT 1 FROM thread_continuation_operations o WHERE o.id=$8
            AND o.thread_id=$1 AND o.target_continuation_id=$2 AND o.status='preparing')")
        .bind(operation.thread_id).bind(operation.target_continuation_id).bind(segment).bind(seq)
        .bind(operation.expected_active_continuation_id).bind(operation.expected_execution_revision)
        .bind(operation.expected_queue_revision).bind(operation.id).execute(&mut *conn).await?.rows_affected()==1;
    if changed {
        sqlx::query(
            "UPDATE thread_continuations SET status='ready',current_segment_id=$2 WHERE id=$1",
        )
        .bind(operation.target_continuation_id)
        .bind(segment)
        .execute(&mut *conn)
        .await?;
        sqlx::query("UPDATE thread_continuation_operations SET status='committed',activation_seq=$2 WHERE id=$1 AND status='preparing'")
            .bind(operation.id).bind(seq).execute(&mut *conn).await?;
        reset_selected_observations(conn, operation.thread_id, now).await?;
    }
    Ok(changed)
}

/// Called in the transaction that stamped a changed execution selection. Lifetime turn/usage
/// records remain intact; excluded current context and the previous summary cursor do not migrate.
/// An older summary keeps its claim until it finishes, but cannot consume this reset cursor.
pub async fn reset_selected_observations(
    conn: &mut Conn,
    thread: Id,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE threads SET usage=NULL WHERE node_id=$1")
        .bind(thread)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE node_summary_state SET last_seq=0,reason=NULL,failures=0,due_at=$2,next_at=$2 WHERE node_id=$1")
        .bind(thread).bind(now).execute(conn).await?;
    Ok(())
}

pub async fn mark_read(
    conn: &mut Conn,
    user: Id,
    thread: Id,
    view: Id,
    through: Seq,
    at: DateTime<Utc>,
) -> Result<Seq, DbError> {
    Ok(sqlx::query_scalar("INSERT INTO thread_reads(user_id,thread_id,continuation_id,last_read_seq,last_read_at)
        SELECT $1,$2,c.id,$4,$5 FROM thread_continuations c WHERE c.id=$3 AND c.thread_id=$2
        ON CONFLICT(user_id,thread_id,continuation_id) DO UPDATE
        SET last_read_seq=GREATEST(thread_reads.last_read_seq,EXCLUDED.last_read_seq),
          last_read_at=CASE WHEN EXCLUDED.last_read_seq>thread_reads.last_read_seq THEN EXCLUDED.last_read_at ELSE thread_reads.last_read_at END
        RETURNING last_read_seq")
        .bind(user).bind(thread).bind(view).bind(through).bind(at).fetch_one(conn).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_scope_cannot_follow_a_different_selected_history_after_a_switch() {
        let thread = orch_types::new_id();
        let root = Projection {
            thread_id: thread,
            viewed_continuation_id: thread,
            active_continuation_id: thread,
            execution_revision: 1,
            projection_revision: 1,
            follows_active: true,
            viewed_segment_id: None,
        };
        assert!(root.accepts_cursor(true, None));
        let changed = Projection {
            execution_revision: 2,
            projection_revision: 2,
            ..root.clone()
        };
        assert!(!changed.accepts_cursor(true, None));
        assert!(!changed.accepts_cursor(true, Some(1)));
        assert!(changed.accepts_cursor(true, Some(2)));
        assert!(changed.accepts_cursor(false, None));
        let explicit = Projection {
            follows_active: false,
            projection_revision: 1,
            ..changed
        };
        assert!(!explicit.accepts_cursor(true, None));
        assert!(explicit.accepts_cursor(true, Some(1)));
    }
}
