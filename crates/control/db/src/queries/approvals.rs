//! Approvals and decisions; announcements with their category and whom they went to.
//!
//! Harness approvals are what this file holds; the other two kinds are written beside them by the parts that
//! raise them.
//!
//! An approval leaves `pending` once: the move out is a compare-and-set, and
//! the answer of each function that makes one says whether this call was the
//! one that did, so that of two people deciding at once exactly one wins.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

const APPROVAL_SELECT: &str = "SELECT id, kind, state, node_id, object_kind, object_id, thread_id, turn_id, item_id,
        environment_id, requester_user_id, requester_thread_id, title, request, required_permission, approver_node_id,
        approver_scope, batch_id, native_ref, decided_by, decided_at, decision, expires_at, requested_expires_at, created_at, updated_at
   FROM approvals";

/// Something that waits for a decision, or was decided.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ApprovalRow {
    /// Its identifier.
    pub id: Id,
    /// `harness`, `capability` or `access`.
    pub kind: String,
    /// `pending`, `approved`, `answered`, `denied`, `cancelled` or `expired`.
    pub state: String,
    /// The node it is about.
    pub node_id: Option<Id>,
    /// For access to a resource: its kind.
    pub object_kind: Option<String>,
    /// For access to a resource: which.
    pub object_id: Option<Id>,
    /// The thread it came from.
    pub thread_id: Option<Id>,
    /// The turn that waits.
    pub turn_id: Option<Id>,
    /// The item that shows it.
    pub item_id: Option<Id>,
    /// The environment.
    pub environment_id: Option<Id>,
    /// The person who asked.
    pub requester_user_id: Option<Id>,
    /// The thread whose agent asked.
    pub requester_thread_id: Option<Id>,
    /// A line to show for it.
    pub title: String,
    /// What it asks.
    pub request: Json,
    /// The permission that decides it.
    pub required_permission: String,
    /// Where that permission must be held.
    pub approver_node_id: Option<Id>,
    /// For a capability: whose approvers decide.
    pub approver_scope: Option<String>,
    /// For access: the requests one person sent together.
    pub batch_id: Option<Id>,
    /// The harness's name for the request.
    pub native_ref: Option<String>,
    /// Who decided it; none with `approved` is a decision by policy.
    pub decided_by: Option<Id>,
    /// When.
    pub decided_at: Option<DateTime<Utc>>,
    /// What was decided.
    pub decision: Option<Json>,
    /// When it runs out.
    pub expires_at: Option<DateTime<Utc>>,
    /// The proposed end of access, distinct from the request's own deadline.
    pub requested_expires_at: Option<DateTime<Utc>>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it last changed.
    pub updated_at: DateTime<Utc>,
}

/// The permission that decides what a harness asks: whoever may set the
/// approval mode answers what it asks for one case.
pub const HARNESS_REQUIRED_PERMISSION: &str = "thread.manage";

/// Locks a request before announcing another recipient copy or recording its decision.
pub async fn lock(conn: &mut Conn, id: Id) -> Result<Option<ApprovalRow>, DbError> {
    let sql = format!("{APPROVAL_SELECT} WHERE id=$1 FOR UPDATE");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// Current records for source cards already authorized by their thread or queue.
pub async fn among(conn: &mut Conn, ids: &[Id]) -> Result<Vec<ApprovalRow>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!("{APPROVAL_SELECT} WHERE id=ANY($1)");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(ids)
        .fetch_all(conn)
        .await?)
}

/// A plan or model-use wait, stored in the same decision ledger as every other request.
pub struct NewSourceApproval<'a> {
    /// Its identity.
    pub id: Id,
    /// Plan or input.
    pub kind: &'a str,
    /// Its thread.
    pub thread: Id,
    /// Its source card.
    pub source: Id,
    /// The turn that proposed it, if any.
    pub turn: Option<Id>,
    /// The message's author, if any.
    pub requester: Option<Id>,
    /// The request as reviewed.
    pub request: &'a Json,
    /// Its deciding permission.
    pub permission: &'a str,
    /// Its short title.
    pub title: &'a str,
    /// When it began waiting.
    pub at: DateTime<Utc>,
}

/// Creates one open source request; a concurrent repeat returns the request already waiting.
pub async fn insert_source(
    conn: &mut Conn,
    new: &NewSourceApproval<'_>,
) -> Result<ApprovalRow, DbError> {
    sqlx::query("INSERT INTO approvals(id,kind,state,node_id,object_kind,object_id,thread_id,turn_id,requester_user_id,requester_thread_id,title,request,required_permission,created_at,updated_at) VALUES($1,$2,'pending',$3,CASE WHEN $2='plan' THEN 'item' ELSE 'input' END,$4,$3,$5,$6,CASE WHEN $6::uuid IS NULL THEN $3 ELSE NULL END,$7,$8,$9,$10,$10) ON CONFLICT DO NOTHING")
        .bind(new.id).bind(new.kind).bind(new.thread).bind(new.source).bind(new.turn).bind(new.requester).bind(new.title).bind(new.request).bind(new.permission).bind(new.at).execute(&mut *conn).await?;
    let sql = format!(
        "{APPROVAL_SELECT} WHERE kind=$1 AND object_id=$2 AND state='pending' ORDER BY created_at,id LIMIT 1"
    );
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(new.kind)
        .bind(new.source)
        .fetch_one(conn)
        .await
        .map_err(DbError::from)
}

/// Source waits not yet announced, or whose source no longer waits, bounded by the supervisor's batch.
pub async fn source_reconciliation(
    conn: &mut Conn,
    limit: i64,
) -> Result<Vec<ApprovalRow>, DbError> {
    let sql = format!(
        "{APPROVAL_SELECT} WHERE state='pending' AND kind IN ('plan','input') AND (
          (NOT EXISTS(SELECT 1 FROM notifications n WHERE n.approval_id=approvals.id)
           AND EXISTS(SELECT 1 FROM threads h JOIN nodes hn ON hn.id=h.node_id WHERE h.node_id=approvals.thread_id AND hn.archived_at IS NULL
               AND (approvals.kind='input' AND EXISTS(SELECT 1 FROM thread_inputs i WHERE i.id=approvals.object_id AND i.continuation_id=h.active_continuation_id)
                 OR approvals.kind='plan' AND EXISTS(SELECT 1 FROM items i WHERE i.id=approvals.object_id AND i.continuation_id=h.active_continuation_id))))
          OR kind='input' AND NOT EXISTS(SELECT 1 FROM thread_inputs i WHERE i.id=approvals.object_id AND i.state='pending' AND i.state_detail='model_use_required' AND i.approval_id=approvals.id)
          OR kind='plan' AND NOT EXISTS(SELECT 1 FROM items i JOIN turns t ON t.id=i.turn_id WHERE i.id=approvals.object_id AND i.payload->>'approval_id'=approvals.id::text AND t.plan AND t.state='completed'
             AND NOT EXISTS(SELECT 1 FROM turns later WHERE later.thread_id=i.thread_id AND later.continuation_id=i.continuation_id AND later.ordinal>t.ordinal)
             AND NOT EXISTS(SELECT 1 FROM thread_inputs q WHERE q.thread_id=i.thread_id AND q.continuation_id=i.continuation_id AND q.state IN ('pending','delivering')))) ORDER BY created_at,id LIMIT $1"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// The person who deliberately started a turn, excluding scheduled and automatic authorship.
pub async fn turn_starter(conn: &mut Conn, turn: Id) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT CASE WHEN a.kind='input' AND a.state='approved' THEN a.decided_by WHEN i.source='user' AND NOT EXISTS(SELECT 1 FROM input_schedule_runs r WHERE r.input_id=i.id) THEN i.author_id ELSE NULL END FROM turns t JOIN thread_inputs i ON i.id=t.input_id LEFT JOIN approvals a ON a.id=i.approval_id WHERE t.id=$1")
        .bind(turn).fetch_optional(conn).await?.flatten())
}

/// A harness approval to raise.
#[derive(Debug, Clone, PartialEq)]
pub struct NewHarnessApproval<'a> {
    /// Its identifier.
    pub id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The turn that waits.
    pub turn_id: Id,
    /// The harness's name for the request, unique in its session.
    pub native_ref: &'a str,
    /// A line to show for it.
    pub title: &'a str,
    /// What the harness asks.
    pub request: &'a Json,
    /// When.
    pub at: DateTime<Utc>,
}

/// Says, as a warning, that a request of a harness was dropped because the thread holds an approval under its
/// reference that belongs to another turn. The same request raised again is the ordinary reason for a drop and is not
/// said; two requests that a harness gave one name are not the same request, and nobody is asked about the second:
/// its turn waits for an answer nobody can give, and nothing else shows why.
async fn warn_of_a_reference_from_another_turn(
    conn: &mut Conn,
    new: &NewHarnessApproval<'_>,
) -> Result<(), DbError> {
    let earlier: Option<Option<Id>> = sqlx::query_scalar(
        "SELECT turn_id FROM approvals WHERE thread_id = $1 AND kind = 'harness' AND native_ref = $2 LIMIT 1",
    )
    .bind(new.thread_id)
    .bind(new.native_ref)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(turn) = earlier
        && turn != Some(new.turn_id)
    {
        tracing::warn!(
            thread_id = %new.thread_id,
            turn_id = %new.turn_id,
            earlier_turn_id = ?turn,
            native_ref = new.native_ref,
            "a request of the harness has the reference of one from another turn of the thread and was dropped: nobody is asked"
        );
    }
    Ok(())
}

/// Raises an approval of a harness. None where one for this request of this
/// thread exists already, which is the answer to a repeated event. The name of a
/// request is the one the harness's adapter gave it, unique in the thread for
/// good: an approval of another turn under the same name drops the request too
/// (and is warned of, since a harness that reuses a name hides its questions).
pub async fn insert_harness_approval(
    conn: &mut Conn,
    new: &NewHarnessApproval<'_>,
) -> Result<Option<ApprovalRow>, DbError> {
    let made: Option<Id> = sqlx::query_scalar(
        "INSERT INTO approvals (id, kind, state, node_id, thread_id, turn_id, title, request, required_permission,
                                native_ref, created_at, updated_at)
         SELECT $1, 'harness', 'pending', $2, $2, $3, $4, $5, $6, $7, $8, $8
          WHERE NOT EXISTS (SELECT 1 FROM approvals WHERE thread_id = $2 AND kind = 'harness' AND native_ref = $7)
         RETURNING id",
    )
        .bind(new.id)
        .bind(new.thread_id)
        .bind(new.turn_id)
        .bind(new.title)
        .bind(new.request)
        .bind(HARNESS_REQUIRED_PERMISSION)
        .bind(new.native_ref)
        .bind(new.at)
        .fetch_optional(&mut *conn)
        .await?;
    match made {
        Some(id) => approval(conn, id).await,
        None => {
            warn_of_a_reference_from_another_turn(conn, new).await?;
            Ok(None)
        }
    }
}

/// The approval `id`.
pub async fn approval(conn: &mut Conn, id: Id) -> Result<Option<ApprovalRow>, DbError> {
    let sql = format!("{APPROVAL_SELECT} WHERE id = $1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// Harness requests named by this thread's items, including already answered requests.
pub async fn harness_among(
    conn: &mut Conn,
    thread: Id,
    ids: &[Id],
) -> Result<Vec<ApprovalRow>, DbError> {
    let sql =
        format!("{APPROVAL_SELECT} WHERE thread_id = $1 AND kind = 'harness' AND id = ANY($2)");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(ids)
        .fetch_all(conn)
        .await?)
}

/// The titles of requests of this thread, including decided ones, for its conversation outline.
pub async fn titles_in_thread(
    conn: &mut Conn,
    thread: Id,
    ids: &[Id],
) -> Result<Vec<(Id, String)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, title FROM approvals WHERE thread_id = $1 AND id = ANY($2)")
            .bind(thread)
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// Current pending approvals named by notification rows, loaded together for their decision projection.
pub async fn pending_among(conn: &mut Conn, ids: &[Id]) -> Result<Vec<ApprovalRow>, DbError> {
    let sql = format!("{APPROVAL_SELECT} WHERE id = ANY($1) AND state = 'pending'");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(ids)
        .fetch_all(conn)
        .await?)
}

/// The approval a harness raised under a name, in a thread.
pub async fn harness_approval_by_ref(
    conn: &mut Conn,
    thread: Id,
    native_ref: &str,
) -> Result<Option<ApprovalRow>, DbError> {
    let sql =
        format!("{APPROVAL_SELECT} WHERE thread_id = $1 AND kind = 'harness' AND native_ref = $2");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(native_ref)
        .fetch_optional(conn)
        .await?)
}

/// Says which item shows an approval.
pub async fn set_item(conn: &mut Conn, approval: Id, item: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE approvals SET item_id = $2 WHERE id = $1")
        .bind(approval)
        .bind(item)
        .execute(conn)
        .await?;
    Ok(())
}

/// Decides a pending approval: content is `answered`, permission is `approved` or `denied`,
/// and `decided_by` none for a decision by policy. The answer says whether
/// this call decided it; of two decisions at once one answers `false`.
pub async fn decide(
    conn: &mut Conn,
    id: Id,
    state: &str,
    decided_by: Option<Id>,
    decision: &Json,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE approvals SET state = $2, decided_by = $3, decision = $4, decided_at = $5, updated_at = $5
          WHERE id = $1 AND state = 'pending'",
    )
    .bind(id)
    .bind(state)
    .bind(decided_by)
    .bind(decision)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 1 {
        super::notifications::inbox::settle_approval(conn, id, at).await?;
    }
    Ok(done.rows_affected() == 1)
}

/// Cancels an approval that is pending (the harness withdrew it, or its turn
/// ended). The answer says whether this call did.
pub async fn cancel_pending(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE approvals SET state = 'cancelled', decided_at = $2, updated_at = $2 WHERE id = $1 AND state = 'pending'",
    )
    .bind(id)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Cancels a decision that could not be handed to the harness: the approval
/// that was approved, answered or denied becomes cancelled, keeping who decided it.
pub async fn cancel_undelivered(
    conn: &mut Conn,
    id: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE approvals SET state = 'cancelled', updated_at = $2 WHERE id = $1 AND state IN ('approved', 'answered', 'denied')",
    )
    .bind(id)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Cancels every approval of a turn that is still pending, and returns them.
pub async fn cancel_pending_of_turn(
    conn: &mut Conn,
    turn: Id,
    at: DateTime<Utc>,
) -> Result<Vec<ApprovalRow>, DbError> {
    Ok(sqlx::query_as(
        "UPDATE approvals SET state = 'cancelled', decided_at = $2, updated_at = $2
          WHERE turn_id = $1 AND kind = 'harness' AND state = 'pending'
          RETURNING id, kind, state, node_id, object_kind, object_id, thread_id, turn_id, item_id, environment_id,
                    requester_user_id, requester_thread_id, title, request, required_permission, approver_node_id,
                    approver_scope, batch_id, native_ref, decided_by, decided_at, decision, expires_at, requested_expires_at, created_at,
                    updated_at",
    )
    .bind(turn)
    .bind(at)
    .fetch_all(conn)
    .await?)
}

/// The approvals of a harness that wait in a thread, oldest first.
pub async fn pending_of_thread(conn: &mut Conn, thread: Id) -> Result<Vec<ApprovalRow>, DbError> {
    let sql = format!(
        "{APPROVAL_SELECT} WHERE thread_id = $1 AND kind IN ('harness','plan','input') AND state = 'pending' ORDER BY created_at, id"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .fetch_all(conn)
        .await?)
}

/// How many approvals of a harness wait in a thread.
pub async fn count_pending_of_thread(conn: &mut Conn, thread: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM approvals WHERE thread_id = $1 AND kind = 'harness' AND state = 'pending'",
    )
    .bind(thread)
    .fetch_one(conn)
    .await?)
}

/// The approvals that were decided and not yet handed to their harness: what
/// a restart between the decision and the command leaves.
pub async fn decided_since(
    conn: &mut Conn,
    after: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<ApprovalRow>, DbError> {
    let sql = format!(
        "{APPROVAL_SELECT} WHERE kind = 'harness' AND state IN ('approved', 'answered', 'denied') AND decided_at > $1
           ORDER BY decided_at LIMIT $2"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(after)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

// ---------------------------------------------------------------- what the routes of approvals read

/// Which approvals a list holds. Every member narrows the list; the list is
/// newest first.
#[derive(Debug, Clone, Default)]
pub struct Listing<'a> {
    /// The states wanted.
    pub states: &'a [&'a str],
    /// Only this kind.
    pub kind: Option<&'a str>,
    /// Only what this person asked for.
    pub requested_by: Option<Id>,
    /// Only what is attached to this node or to anything inside it.
    pub under: Option<Id>,
    /// Only what comes before this approval in the list.
    pub before: Option<Id>,
    /// How many, at most.
    pub limit: i64,
}

/// The approvals that match, newest first.
pub async fn list(conn: &mut Conn, filter: &Listing<'_>) -> Result<Vec<ApprovalRow>, DbError> {
    let states: Vec<String> = filter
        .states
        .iter()
        .map(|state| (*state).to_owned())
        .collect();
    let sql = format!(
        "{APPROVAL_SELECT} WHERE state = ANY($1)
            AND ($2::text IS NULL OR kind = $2)
            AND ($3::uuid IS NULL OR requester_user_id = $3)
            AND ($4::uuid IS NULL OR node_id IN (SELECT id FROM nodes WHERE $4 = ANY(path)))
            AND ($5::uuid IS NULL OR id < $5)
          ORDER BY id DESC LIMIT $6"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(states)
        .bind(filter.kind)
        .bind(filter.requested_by)
        .bind(filter.under)
        .bind(filter.before)
        .bind(filter.limit)
        .fetch_all(conn)
        .await?)
}

/// Outstanding requests for an already-authorized batch of tree threads. Expired rows never ask a
/// reader for a decision; the tree's page/delta bounds the set of threads, without a per-thread query.
pub async fn pending_for_threads(
    conn: &mut Conn,
    threads: &[Id],
    at: DateTime<Utc>,
) -> Result<Vec<ApprovalRow>, DbError> {
    if threads.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "{APPROVAL_SELECT} WHERE thread_id = ANY($1) AND state = 'pending'
            AND (expires_at IS NULL OR expires_at > $2) ORDER BY id DESC"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(threads)
        .bind(at)
        .fetch_all(conn)
        .await?)
}

/// Takes back an access request that is still pending, for the person who
/// asked. The answer says whether this call did.
pub async fn cancel_request(
    conn: &mut Conn,
    id: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE approvals SET state = 'cancelled', decided_at = $3, updated_at = $3
          WHERE id = $1 AND kind = 'access' AND state = 'pending' AND requester_user_id = $2",
    )
    .bind(id)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

// ---------------------------------------------------------------- what a request for a capability reads and writes
//
// One block, kept together at the end of the file: the approval an agent's request for a capability raises, the one
// that already waits for the same capability of the same environment, and the ones nobody decided in time. Nothing
// here decides anything.

/// The approvals that an agent's `request_capability` raises.
pub mod capability {
    use super::{APPROVAL_SELECT, ApprovalRow, approval};
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    /// The first decision of an identical request by this agent during the denial hold, if one exists.
    pub async fn recent_denial(
        conn: &mut Conn,
        environment: Id,
        thread: Id,
        capability: &str,
        reason: &str,
        since: DateTime<Utc>,
    ) -> Result<Option<ApprovalRow>, DbError> {
        let sql = format!("{APPROVAL_SELECT} WHERE kind = 'capability' AND state = 'denied' AND environment_id = $1
            AND requester_thread_id = $2 AND request ->> 'capability' = $3 AND request ->> 'reason' = $4
            AND decided_at > $5 ORDER BY decided_at, id LIMIT 1");
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(environment)
            .bind(thread)
            .bind(capability)
            .bind(reason)
            .bind(since)
            .fetch_optional(conn)
            .await?)
    }

    /// A request for a capability to raise.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewCapabilityApproval<'a> {
        /// Its identifier: derived from the call that raised it, so that the same call raises it once.
        pub id: Id,
        /// The thread whose agent asks.
        pub thread_id: Id,
        /// The turn the call was made in.
        pub turn_id: Option<Id>,
        /// The environment that would be given it.
        pub environment_id: Id,
        /// A line to show for it.
        pub title: &'a str,
        /// What is asked, as the wire has it.
        pub request: &'a Json,
        /// Whose approvers decide: `host`, `org` or `system`.
        pub approver_scope: &'a str,
        /// When it runs out.
        pub expires_at: DateTime<Utc>,
        /// When.
        pub at: DateTime<Utc>,
    }

    /// Raises the request. None where it was raised before, which is the answer to the same call asked again.
    pub async fn insert(
        conn: &mut Conn,
        new: &NewCapabilityApproval<'_>,
    ) -> Result<Option<ApprovalRow>, DbError> {
        let made: Option<Id> = sqlx::query_scalar(
            "INSERT INTO approvals (id, kind, state, node_id, thread_id, turn_id, environment_id, requester_thread_id,
                                    title, request, required_permission, approver_scope, expires_at, created_at,
                                    updated_at)
             VALUES ($1, 'capability', 'pending', $2, $2, $3, $4, $2, $5, $6, '', $7, $8, $9, $9)
             ON CONFLICT (id) DO NOTHING
             RETURNING id",
        )
        .bind(new.id)
        .bind(new.thread_id)
        .bind(new.turn_id)
        .bind(new.environment_id)
        .bind(new.title)
        .bind(new.request)
        .bind(new.approver_scope)
        .bind(new.expires_at)
        .bind(new.at)
        .fetch_optional(&mut *conn)
        .await?;
        match made {
            Some(id) => approval(conn, id).await,
            None => Ok(None),
        }
    }

    /// The request for `capability` of `environment` that waits for a decision, the oldest if several do.
    pub async fn pending_for(
        conn: &mut Conn,
        environment: Id,
        capability: &str,
    ) -> Result<Option<ApprovalRow>, DbError> {
        let sql = format!(
            "{APPROVAL_SELECT} WHERE kind = 'capability' AND state = 'pending' AND environment_id = $1
                AND request ->> 'capability' = $2
              ORDER BY created_at, id LIMIT 1"
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(environment)
            .bind(capability)
            .fetch_optional(conn)
            .await?)
    }

    /// The requests that waited past their time, oldest first.
    pub async fn overdue(
        conn: &mut Conn,
        now: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<ApprovalRow>, DbError> {
        let sql = format!(
            "{APPROVAL_SELECT} WHERE kind = 'capability' AND state = 'pending' AND expires_at <= $1
              ORDER BY expires_at, id LIMIT $2"
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(now)
            .bind(limit)
            .fetch_all(conn)
            .await?)
    }

    /// Lets a request lapse. The answer says whether this call did.
    pub async fn expire(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
        let done = sqlx::query(
            "UPDATE approvals SET state = 'expired', decided_at = $2, updated_at = $2
              WHERE id = $1 AND kind = 'capability' AND state = 'pending'",
        )
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(done.rows_affected() == 1)
    }
}

// ---------------------------------------------------------------- what a request for access reads and writes
//
// One block, kept together at the end of the file: the approvals that a request for access raises, one for each
// thing asked for and all of them with the batch they were sent in, the batches a person has waiting, and the
// requests nobody decided in time. Nothing here decides who may decide.

/// The approvals that a request for access raises.
pub mod access {
    use super::{APPROVAL_SELECT, ApprovalRow, approval};
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    /// The permission that decides a request for access: whoever may change the page of what is
    /// asked for.
    pub const REQUIRED_PERMISSION: &str = "access.manage";

    /// Serializes one person's sends so the open-request and batch limits cannot race.
    pub async fn lock_requester(conn: &mut Conn, requester: Id) -> Result<(), DbError> {
        // Keep requester serialization separate from Inbox sequence rows, which decisions also lock.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("access-request:{requester}"))
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Open requests on one known thing, before checking the current reader's deciding authority.
    pub async fn pending_on(
        conn: &mut Conn,
        object: Id,
        at: DateTime<Utc>,
    ) -> Result<Vec<ApprovalRow>, DbError> {
        let sql = format!(
            "{APPROVAL_SELECT} WHERE kind='access' AND COALESCE(object_id,node_id)=$1 AND state='pending' AND (expires_at IS NULL OR expires_at>$2) ORDER BY created_at"
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(object)
            .bind(at)
            .fetch_all(conn)
            .await?)
    }

    /// The one still-open request by this person on this thing, independent of the requested rows.
    pub async fn pending_for_person(
        conn: &mut Conn,
        requester: Id,
        object: Id,
        at: DateTime<Utc>,
    ) -> Result<Option<ApprovalRow>, DbError> {
        let sql = format!(
            "{APPROVAL_SELECT} WHERE kind='access' AND requester_user_id=$1 AND COALESCE(object_id,node_id)=$2 AND state='pending' AND (expires_at IS NULL OR expires_at>$3) ORDER BY created_at LIMIT 1"
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(requester)
            .bind(object)
            .bind(at)
            .fetch_optional(conn)
            .await?)
    }

    /// A request for one thing to raise.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewAccessApproval<'a> {
        /// Its identifier.
        pub id: Id,
        /// The batch it was sent in, with the others of the same request.
        pub batch_id: Id,
        /// The node, where what is asked for is a node or lies beside one.
        pub node_id: Option<Id>,
        /// For a resource or a document: its kind.
        pub object_kind: Option<&'a str>,
        /// For a resource or a document: which.
        pub object_id: Option<Id>,
        /// The thread the person was attempting to act on.
        pub thread_id: Option<Id>,
        /// The person who asks.
        pub requester: Id,
        /// A line to show for it.
        pub title: &'a str,
        /// What is asked, as it is kept.
        pub request: &'a Json,
        /// The named boundary; membership also uses the ordinary add-member rule.
        pub required_permission: &'a str,
        /// When it runs out.
        pub expires_at: DateTime<Utc>,
        /// The proposed end of the access requested.
        pub requested_expires_at: Option<DateTime<Utc>>,
        /// When.
        pub at: DateTime<Utc>,
    }

    /// Raises the approval and returns it.
    pub async fn insert(
        conn: &mut Conn,
        new: &NewAccessApproval<'_>,
    ) -> Result<ApprovalRow, DbError> {
        sqlx::query(
            "INSERT INTO approvals (id, kind, state, node_id, object_kind, object_id, thread_id, requester_user_id,
                                    title, request, required_permission, batch_id, expires_at, requested_expires_at, created_at, updated_at)
             VALUES ($1, 'access', 'pending', $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $13)",
        )
        .bind(new.id)
        .bind(new.node_id)
        .bind(new.object_kind)
        .bind(new.object_id)
        .bind(new.thread_id)
        .bind(new.requester)
        .bind(new.title)
        .bind(new.request)
        .bind(new.required_permission)
        .bind(new.batch_id)
        .bind(new.expires_at)
        .bind(new.requested_expires_at)
        .bind(new.at)
        .execute(&mut *conn)
        .await?;
        approval(conn, new.id)
            .await?
            .ok_or(DbError::Other(sqlx::Error::RowNotFound))
    }

    /// The approvals of a batch, in the order they were made.
    pub async fn batch(conn: &mut Conn, batch: Id) -> Result<Vec<ApprovalRow>, DbError> {
        let sql = format!(
            "{APPROVAL_SELECT} WHERE kind = 'access' AND batch_id = $1 ORDER BY created_at, id"
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(batch)
            .fetch_all(conn)
            .await?)
    }

    /// How many batches of requests a person has waiting: the ones with an approval that is
    /// pending and has not run out. A batch counts once however many things it asks for.
    pub async fn waiting_batches(
        conn: &mut Conn,
        requester: Id,
        now: DateTime<Utc>,
    ) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT count(DISTINCT batch_id) FROM approvals
              WHERE kind = 'access' AND state = 'pending' AND requester_user_id = $1
                AND (expires_at IS NULL OR expires_at > $2)",
        )
        .bind(requester)
        .bind(now)
        .fetch_one(conn)
        .await?)
    }

    /// The requests that waited past their time, oldest first.
    pub async fn overdue(
        conn: &mut Conn,
        now: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<ApprovalRow>, DbError> {
        let sql = format!(
            "{APPROVAL_SELECT} WHERE kind = 'access' AND state = 'pending' AND expires_at <= $1
              ORDER BY expires_at, id LIMIT $2"
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(now)
            .bind(limit)
            .fetch_all(conn)
            .await?)
    }

    /// Lets a request lapse. The answer says whether this call did.
    pub async fn expire(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
        let done = sqlx::query(
            "UPDATE approvals SET state = 'expired', decided_at = $2, updated_at = $2
              WHERE id = $1 AND kind = 'access' AND state = 'pending'",
        )
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(done.rows_affected() == 1)
    }
}

/// Links an already recorded content request to its native tool in the same thread/turn.
/// The ordinary item-parent edge exposes the source without copying its input or result.
pub async fn link_content_source(
    conn: &mut Conn,
    thread: Id,
    turn: Id,
    native_ref: &str,
    source: Id,
    at: DateTime<Utc>,
    seq: i64,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE items AS item SET parent_item_id=$4, updated_at=$5, seq_updated=$6
        FROM approvals AS approval
        WHERE approval.item_id=item.id AND approval.thread_id=$1 AND approval.turn_id=$2
          AND approval.kind='harness'
          AND approval.request->>'request_kind' IN ('user_input','mcp_elicitation')
          AND approval.request #>> '{detail,native_item_ref}'=$3
          AND item.thread_id=$1 AND item.turn_id=$2 AND item.kind='approval'
          AND item.parent_item_id IS NULL",
    )
    .bind(thread)
    .bind(turn)
    .bind(native_ref)
    .bind(source)
    .bind(at)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(())
}

/// A recovered tool completion can outlive its turn. Its explicit content request supplies that turn,
/// but only when this segment has exactly one matching native source identity.
pub async fn content_source_turn(
    conn: &mut Conn,
    thread: Id,
    segment: Id,
    native_ref: &str,
) -> Result<Option<Id>, DbError> {
    let turns: Vec<Id> = sqlx::query_scalar(
        "SELECT DISTINCT approval.turn_id FROM approvals AS approval
        JOIN turns AS turn ON turn.id=approval.turn_id
        WHERE approval.thread_id=$1 AND turn.segment_id=$2 AND approval.kind='harness'
          AND approval.request->>'request_kind' IN ('user_input','mcp_elicitation')
          AND approval.request #>> '{detail,native_item_ref}'=$3 LIMIT 2",
    )
    .bind(thread)
    .bind(segment)
    .bind(native_ref)
    .fetch_all(conn)
    .await?;
    Ok(match turns.as_slice() {
        [turn] => Some(*turn),
        _ => None,
    })
}
