//! Committed stream cursors and commands.
//!
//! A machine's events are taken in once each and in order because the
//! cursor of a stream is moved in the same transaction as what the events
//! change: either both happened or neither did, and a stream that is sent
//! again after a break is judged against the cursor that was committed. A
//! cursor moves by compare-and-set, from the value the caller applied on top
//! of, so that two writers can never both believe they took the same events
//! in.
//!
//! A command is a row before it is sent, and the row says where it stands
//! until it is answered, has expired or was cancelled. Every change of its
//! state is a compare-and-set that only moves it forward, so that a late
//! word (a second answer, a resend that crossed an acknowledgement) can
//! never take a command back to an earlier state.
//!
//! A command that is over is kept for a while and then removed, except the
//! kinds that are read back as a record ([`RECORD_KINDS`]); the answer of a
//! read whose one reader has it can be cleared before that ([`forget_result`]).

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};
use std::collections::BTreeMap;

/// The states of a command that is not over.
pub const OPEN_STATES: [&str; 3] = ["queued", "sent", "accepted"];

/// How far each stream of a machine was taken in.
pub async fn cursors(conn: &mut Conn, machine: Id) -> Result<BTreeMap<String, i64>, DbError> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT stream, last_seq FROM link_cursors WHERE machine_id = $1")
            .bind(machine)
            .fetch_all(conn)
            .await?;
    Ok(rows.into_iter().collect())
}

/// Moves the cursor of `stream` from `from` to `to`. Returns whether it
/// moved: it does not when the stored cursor is not `from`, which means that
/// somebody else took events in since the caller looked. A stream that has
/// no cursor yet stands at 0.
pub async fn advance_cursor(
    conn: &mut Conn,
    machine: Id,
    stream: &str,
    from: i64,
    to: i64,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let moved = sqlx::query(
        "UPDATE link_cursors SET last_seq = $4, updated_at = $5
          WHERE machine_id = $1 AND stream = $2 AND last_seq = $3",
    )
    .bind(machine)
    .bind(stream)
    .bind(from)
    .bind(to)
    .bind(at)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if moved > 0 {
        return Ok(true);
    }
    if from != 0 {
        return Ok(false);
    }
    let inserted = sqlx::query(
        "INSERT INTO link_cursors (machine_id, stream, last_seq, updated_at) VALUES ($1, $2, $3, $4)
         ON CONFLICT (machine_id, stream) DO NOTHING",
    )
    .bind(machine)
    .bind(stream)
    .bind(to)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(inserted > 0)
}

/// Sets the cursor of `stream` to `to` whatever it was: after a machine said
/// that events of the stream are gone, the stream goes on from the number it
/// names.
pub async fn set_cursor(
    conn: &mut Conn,
    machine: Id,
    stream: &str,
    to: i64,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO link_cursors (machine_id, stream, last_seq, updated_at) VALUES ($1, $2, $3, $4)
         ON CONFLICT (machine_id, stream) DO UPDATE SET last_seq = EXCLUDED.last_seq, updated_at = EXCLUDED.updated_at",
    )
    .bind(machine)
    .bind(stream)
    .bind(to)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// A command, as stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct CommandRow {
    /// Its identifier, which is the same every time it is sent.
    pub id: Id,
    /// The machine it is for.
    pub machine_id: Id,
    /// Its kind.
    pub kind: String,
    /// Its body in the form the system keeps it: references to secrets and
    /// tickets in place of their values.
    pub body: Json,
    /// Its protected serial queue identity; absent on an older retained row.
    pub serial_key: Option<String>,
    /// Where it stands: `queued`, `sent`, `accepted`, `succeeded`,
    /// `failed`, `expired` or `cancelled`.
    pub state: String,
    /// What the machine answered, once it succeeded; none again once the
    /// answer of a read was cleared because the one who asked had it.
    pub result: Option<Json>,
    /// Why it failed.
    pub error: Option<Json>,
    /// How many times it was handed to a gateway.
    pub attempts: i32,
    /// The moment after which it is not to be started.
    pub deadline_at: DateTime<Utc>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When its state last changed.
    pub updated_at: DateTime<Utc>,
}

impl CommandRow {
    /// The authoritative queue identity, reading an older stored member only when no column was supplied.
    #[must_use]
    pub fn serial_key(&self) -> Option<&str> {
        self.serial_key
            .as_deref()
            .or_else(|| self.body.get("$key").and_then(Json::as_str))
    }

    /// Whether nothing more is to happen to the command.
    #[must_use]
    pub fn is_over(&self) -> bool {
        !OPEN_STATES.contains(&self.state.as_str())
    }
}

/// The columns of a command, in the order [`CommandRow`] has them.
macro_rules! command_columns {
    () => {
        "id, machine_id, kind, body, serial_key, state, result, error, attempts, deadline_at, created_at, updated_at"
    };
}

/// What a command is made with.
#[derive(Debug, Clone, PartialEq)]
pub struct NewCommand<'a> {
    /// Its identifier.
    pub id: Id,
    /// The machine it is for.
    pub machine_id: Id,
    /// Its kind.
    pub kind: &'a str,
    /// Its body, with references in place of secrets.
    pub body: &'a Json,
    /// Its serial queue identity, stored separately from the machine request.
    pub serial_key: Option<&'a str>,
    /// The moment after which it is not to be started.
    pub deadline_at: DateTime<Utc>,
}

/// Stores a command as `queued`. Returns whether it was stored: a command
/// that is there already, under the same identifier, is left as it is, so
/// that asking twice for the same command makes one.
pub async fn insert_command(
    conn: &mut Conn,
    command: &NewCommand<'_>,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let inserted = sqlx::query(
        "INSERT INTO commands (id, machine_id, kind, body, serial_key, deadline_at, created_at, updated_at)
         VALUES ($1, $2, $3, CASE WHEN jsonb_typeof($4::jsonb) = 'object'
                    THEN $4::jsonb - '$key' ELSE $4::jsonb END, $5, $6, $7, $7)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(command.id)
    .bind(command.machine_id)
    .bind(command.kind)
    .bind(command.body)
    .bind(command.serial_key.or_else(|| command.body.get("$key").and_then(Json::as_str)))
    .bind(command.deadline_at)
    .bind(at)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if inserted > 0 {
        crate::queries::session_dependencies::admit(conn, command.id, command.kind, command.body)
            .await?;
    }
    Ok(inserted > 0)
}

/// The command `id`.
pub async fn command(conn: &mut Conn, id: Id) -> Result<Option<CommandRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        command_columns!(),
        " FROM commands WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The commands of a machine that are not over, oldest first: what is sent
/// again whenever the machine has a session.
pub async fn open_commands(conn: &mut Conn, machine: Id) -> Result<Vec<CommandRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        command_columns!(),
        " FROM commands
          WHERE machine_id = $1 AND state IN ('queued', 'sent', 'accepted')
          ORDER BY created_at,
                   CASE kind WHEN 'turn.interrupt' THEN 0 WHEN 'lease.close' THEN 1 ELSE 2 END, id"
    ))
    .bind(machine)
    .fetch_all(conn)
    .await?)
}

/// Notes that a command was handed to a gateway: a `queued` one becomes
/// `sent`, and every hand-over is counted.
pub async fn mark_sent(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
    let changed = sqlx::query(
        "UPDATE commands SET state = 'sent', attempts = attempts + 1, updated_at = $2
          WHERE id = $1 AND state IN ('queued', 'sent')",
    )
    .bind(id)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

/// Notes that a command is queued again, because the session it was sent on
/// ended before the machine took it.
pub async fn mark_queued(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
    let changed = sqlx::query(
        "UPDATE commands SET state = 'queued', updated_at = $2 WHERE id = $1 AND state = 'sent'",
    )
    .bind(id)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

/// Notes that the machine wrote the command down.
pub async fn mark_accepted(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
    let changed = sqlx::query(
        "UPDATE commands SET state = 'accepted', updated_at = $2
          WHERE id = $1 AND state IN ('queued', 'sent')",
    )
    .bind(id)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

/// How a command ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Ending<'a> {
    /// The machine answered.
    Succeeded(&'a Json),
    /// The machine failed it, or did not answer.
    Failed(&'a Json),
    /// Its deadline passed before a machine took it.
    Expired,
    /// The system cancelled it.
    Cancelled,
}

/// Ends a command that is not over. Returns whether it ended now: the first
/// ending stands.
pub async fn finish_command(
    conn: &mut Conn,
    id: Id,
    ending: &Ending<'_>,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let (state, result, error) = match ending {
        Ending::Succeeded(result) => ("succeeded", Some(*result), None),
        Ending::Failed(error) => ("failed", None, Some(*error)),
        Ending::Expired => ("expired", None, None),
        Ending::Cancelled => ("cancelled", None, None),
    };
    let changed: Option<(String, Json, DateTime<Utc>)> = sqlx::query_as(
        "UPDATE commands SET state = $2, result = $3, error = $4, updated_at = $5
          WHERE id = $1 AND state IN ('queued', 'sent', 'accepted') RETURNING kind,body,created_at",
    )
    .bind(id)
    .bind(state)
    .bind(result)
    .bind(error)
    .bind(at)
    .fetch_optional(&mut *conn)
    .await?;
    if state == "succeeded"
        && let Some((kind, body, issued)) = &changed
    {
        crate::queries::session_dependencies::record(conn, id, kind, body, *issued, at).await?;
    }
    Ok(changed.is_some())
}

/// Expires the commands that no machine took before their deadline, on any
/// machine, and returns them. A command the machine has written down stays
/// open: the machine owes an answer. The commands in `followed` are left to
/// the link that follows them, whose row may not yet say that their machine
/// took them.
pub async fn expire_unstarted(
    conn: &mut Conn,
    now: DateTime<Utc>,
    followed: &[Id],
) -> Result<Vec<(Id, Id)>, DbError> {
    Ok(sqlx::query_as(
        "UPDATE commands SET state = 'expired', updated_at = $1
          WHERE state IN ('queued', 'sent') AND deadline_at <= $1 AND NOT (id = ANY($2))
      RETURNING machine_id, id",
    )
    .bind(now)
    .bind(followed)
    .fetch_all(conn)
    .await?)
}

/// Cancels every open command of a machine, and returns them: a machine
/// that was revoked will answer none.
pub async fn cancel_open_of(
    conn: &mut Conn,
    machine: Id,
    at: DateTime<Utc>,
) -> Result<Vec<Id>, DbError> {
    let rows: Vec<(Id,)> = sqlx::query_as(
        "UPDATE commands SET state = 'cancelled', updated_at = $2
          WHERE machine_id = $1 AND state IN ('queued', 'sent', 'accepted')
      RETURNING id",
    )
    .bind(machine)
    .bind(at)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

// ---- What is kept of a command that is over ----

/// The kinds of command whose rows stay after they are over, because what a unit or an environment was
/// last given is read back from them: the definition a runtime's unit was made with, and the limits and
/// the mounts applied to it since; the last attach of an environment, which also names the machine that
/// may report on it; the state of each tool server; the machine a harness was started on, which may
/// report on it; and that a provider runs on its machine with what it carries now. Each is made once for
/// such an act, not for each look at a machine, and goes with its machine.
pub const RECORD_KINDS: [&str; 7] = [
    "node.runtime.create",
    "node.resources.apply",
    "node.mounts.apply",
    "env.attach",
    "mcp.start",
    "agent.start",
    "provider.start",
];

/// Clears the answer a command succeeded with, for a read whose one reader has it. The row stays as it
/// is otherwise, its state included, until finished commands are removed; whoever reads it later is told
/// that its answer expired. Returns whether there was an answer to clear.
pub async fn forget_result(conn: &mut Conn, id: Id) -> Result<bool, DbError> {
    let cleared = sqlx::query(
        "UPDATE commands SET result = NULL
          WHERE id = $1 AND state = 'succeeded' AND result IS NOT NULL",
    )
    .bind(id)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(cleared > 0)
}

/// Deletes at most `limit` commands that ended before `before`, and returns how many. Neither one of the
/// [`RECORD_KINDS`] goes, nor one that names a job that is not over: a job that resumes reads the end of
/// each of its commands again by its identifier, and would otherwise send it once more. A row another
/// transaction holds is left for the next time. Unsettled own-tool native effects and their
/// read-only queue fences remain until completion is known, even after their caller's job ends.
pub async fn purge_finished(
    conn: &mut Conn,
    before: DateTime<Utc>,
    limit: i64,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM commands WHERE id IN (
           SELECT c.id FROM commands c
            WHERE c.state IN ('succeeded', 'failed', 'expired', 'cancelled')
              AND c.updated_at < $1
              AND c.kind <> ALL($2)
              AND NOT (c.kind='provider.stop' AND COALESCE(c.body->>'purge'='true', false)
                       AND c.state IN ('failed','expired','cancelled')
                       AND c.error#>>'{details,purge_retry}' IS NULL)
              AND NOT EXISTS (SELECT 1 FROM thread_tool_commands e JOIN thread_tool_calls t ON t.answer_id=e.answer_id
                                WHERE (NOT e.settled OR (NOT t.completed AND NOT t.retired)) AND (e.command_id=c.id OR e.fence_id=c.id OR
                                     (c.kind='node.runtime.remove' AND c.machine_id=e.retirement_machine_id
                                       AND c.body->>'runtime_id'=e.runtime_id::text)))
              AND NOT EXISTS (SELECT 1 FROM jobs j
                               WHERE j.state IN ('queued', 'running')
                                 AND j.id::text = c.body->>'job_id')
              AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.state IN ('queued','running')
                    AND j.plan->'active_command'->>'id'=c.id::text)
              AND NOT EXISTS (SELECT 1 FROM thread_history_cut_commands h WHERE h.command_id=c.id OR h.fence_id=c.id
                OR (NOT h.settled AND c.kind='node.runtime.remove' AND c.machine_id=h.retirement_machine_id
                  AND c.body->>'runtime_id'=h.runtime_id::text))
            LIMIT $3
              FOR UPDATE OF c SKIP LOCKED)",
    )
    .bind(before)
    .bind(&RECORD_KINDS[..])
    .bind(limit)
    .execute(conn)
    .await?
    .rows_affected())
}

/// A stored secret, as far as sending a command needs it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct StoredSecret {
    /// What owns it.
    pub owner_kind: String,
    /// The owner.
    pub owner_id: Id,
    /// Its name.
    pub name: String,
    /// Its value, sealed under the system's key.
    pub sealed: Vec<u8>,
}

/// The secret `id`, with its sealed value.
pub async fn stored_secret(conn: &mut Conn, id: Id) -> Result<Option<StoredSecret>, DbError> {
    Ok(
        sqlx::query_as("SELECT owner_kind, owner_id, name, sealed FROM secrets WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

// ---- Management over HTTP: what waits on the machines ----

/// How many commands to machines are not over, whichever machine they wait for: queued, sent or
/// accepted.
pub async fn count_open_commands(conn: &mut Conn) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM commands WHERE state = ANY($1)")
            .bind(&OPEN_STATES[..])
            .fetch_one(conn)
            .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;
    use serde_json::json;

    async fn machine(conn: &mut Conn) -> Id {
        crate::queries::resources::testing::insert_machine(conn, "m", "connected")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_cursor_moves_only_from_where_the_caller_saw_it() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (machine, now) = (machine(&mut conn).await, clock::start());
        assert!(
            advance_cursor(&mut conn, machine, "machine", 0, 4, now)
                .await
                .unwrap()
        );
        assert!(
            !advance_cursor(&mut conn, machine, "machine", 0, 9, now)
                .await
                .unwrap(),
            "somebody else took events in since the caller looked"
        );
        assert!(
            !advance_cursor(&mut conn, machine, "env:x", 3, 5, now)
                .await
                .unwrap(),
            "a stream that was never taken in stands at 0"
        );
        assert!(
            advance_cursor(&mut conn, machine, "machine", 4, 6, now)
                .await
                .unwrap()
        );
        assert_eq!(cursors(&mut conn, machine).await.unwrap()["machine"], 6);
    }

    #[tokio::test]
    async fn a_reset_sets_the_cursor_wherever_it_was() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (machine, now) = (machine(&mut conn).await, clock::start());
        advance_cursor(&mut conn, machine, "machine", 0, 40, now)
            .await
            .unwrap();
        set_cursor(&mut conn, machine, "machine", 9, now)
            .await
            .unwrap();
        assert_eq!(cursors(&mut conn, machine).await.unwrap()["machine"], 9);
        set_cursor(&mut conn, machine, "env:y", 0, now)
            .await
            .unwrap();
        assert_eq!(cursors(&mut conn, machine).await.unwrap()["env:y"], 0);
    }

    fn new_command<'a>(machine: Id, body: &'a Json, deadline: DateTime<Utc>) -> NewCommand<'a> {
        NewCommand {
            id: new_id(),
            machine_id: machine,
            kind: "machine.probe",
            body,
            serial_key: None,
            deadline_at: deadline,
        }
    }

    #[tokio::test]
    async fn a_command_reload_preserves_its_protected_queue_key_and_reads_an_older_retained_body() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let owner = machine(&mut conn).await;
        let body = json!({"$key":"obsolete"});
        let mut asked = new_command(owner, &body, clock::start() + Duration::seconds(30));
        asked.serial_key = Some("authoritative");
        insert_command(&mut conn, &asked, clock::start())
            .await
            .unwrap();
        let id = asked.id;
        drop(conn);
        let mut conn = test.pool.acquire().await.unwrap();
        let row = command(&mut conn, id).await.unwrap().unwrap();
        assert_eq!(row.serial_key(), Some("authoritative"));
        assert_eq!(
            row.body,
            json!({}),
            "storage metadata never belongs to a machine request"
        );
        // Older durable commands have no column value; their queue order still survives a restart.
        sqlx::query("UPDATE commands SET serial_key=NULL,body=$2 WHERE id=$1")
            .bind(id)
            .bind(json!({"$key":"older"}))
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        let mut conn = test.pool.acquire().await.unwrap();
        let row = open_commands(&mut conn, owner)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(row.id, id);
        assert_eq!(row.serial_key(), Some("older"));
    }

    #[tokio::test]
    async fn a_command_asked_for_twice_under_one_identifier_is_made_once() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (machine, now) = (machine(&mut conn).await, clock::start());
        let body = json!({});
        let command = new_command(machine, &body, now + Duration::seconds(30));
        assert!(insert_command(&mut conn, &command, now).await.unwrap());
        assert!(!insert_command(&mut conn, &command, now).await.unwrap());
        assert_eq!(open_commands(&mut conn, machine).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_command_only_moves_forward_and_its_first_ending_stands() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (machine, now) = (machine(&mut conn).await, clock::start());
        let body = json!({});
        let command = new_command(machine, &body, now + Duration::seconds(30));
        insert_command(&mut conn, &command, now).await.unwrap();
        assert!(mark_sent(&mut conn, command.id, now).await.unwrap());
        assert!(mark_queued(&mut conn, command.id, now).await.unwrap());
        assert!(mark_sent(&mut conn, command.id, now).await.unwrap());
        assert!(mark_accepted(&mut conn, command.id, now).await.unwrap());
        assert!(
            !mark_sent(&mut conn, command.id, now).await.unwrap(),
            "an accepted command is not sent again as if new"
        );
        assert!(
            !mark_queued(&mut conn, command.id, now).await.unwrap(),
            "and does not go back to the queue"
        );
        let result = json!({ "ok": 1 });
        assert!(
            finish_command(&mut conn, command.id, &Ending::Succeeded(&result), now)
                .await
                .unwrap()
        );
        assert!(
            !finish_command(&mut conn, command.id, &Ending::Cancelled, now)
                .await
                .unwrap()
        );
        let stored = super::command(&mut conn, command.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, "succeeded");
        assert_eq!(stored.result, Some(result));
        assert_eq!(stored.attempts, 2);
        assert!(stored.is_over());
        assert!(open_commands(&mut conn, machine).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn only_commands_no_machine_took_expire_at_their_deadline() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (machine, now) = (machine(&mut conn).await, clock::start());
        let body = json!({});
        let waiting = new_command(machine, &body, now + Duration::seconds(10));
        let taken = new_command(machine, &body, now + Duration::seconds(10));
        for command in [&waiting, &taken] {
            insert_command(&mut conn, command, now).await.unwrap();
        }
        mark_accepted(&mut conn, taken.id, now).await.unwrap();
        let later = now + Duration::seconds(11);
        let expired = expire_unstarted(&mut conn, later, &[]).await.unwrap();
        assert_eq!(expired, vec![(machine, waiting.id)]);
        let still = open_commands(&mut conn, machine).await.unwrap();
        assert_eq!(still.len(), 1);
        assert_eq!(still[0].id, taken.id);
    }

    #[tokio::test]
    async fn the_open_commands_of_a_machine_are_cancelled_together() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (machine, now) = (machine(&mut conn).await, clock::start());
        let body = json!({});
        let one = new_command(machine, &body, now + Duration::seconds(10));
        let two = new_command(machine, &body, now + Duration::seconds(10));
        for command in [&one, &two] {
            insert_command(&mut conn, command, now).await.unwrap();
        }
        finish_command(&mut conn, two.id, &Ending::Expired, now)
            .await
            .unwrap();
        let cancelled = cancel_open_of(&mut conn, machine, now).await.unwrap();
        assert_eq!(cancelled, vec![one.id]);
    }

    #[tokio::test]
    async fn only_the_answer_of_a_command_that_succeeded_is_cleared_and_the_row_stays() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (machine, now) = (machine(&mut conn).await, clock::start());
        let body = json!({});
        let answered = new_command(machine, &body, now + Duration::seconds(10));
        let failed = new_command(machine, &body, now + Duration::seconds(10));
        for command in [&answered, &failed] {
            insert_command(&mut conn, command, now).await.unwrap();
        }
        let listing = json!({ "entries": [], "truncated": false });
        finish_command(&mut conn, answered.id, &Ending::Succeeded(&listing), now)
            .await
            .unwrap();
        let why = json!({ "code": "not_found", "detail": "gone", "retryable": false });
        finish_command(&mut conn, failed.id, &Ending::Failed(&why), now)
            .await
            .unwrap();
        assert!(forget_result(&mut conn, answered.id).await.unwrap());
        assert!(
            !forget_result(&mut conn, answered.id).await.unwrap(),
            "there is nothing left to clear"
        );
        assert!(!forget_result(&mut conn, failed.id).await.unwrap());
        let kept = command(&mut conn, answered.id).await.unwrap().unwrap();
        assert_eq!((kept.state.as_str(), kept.result), ("succeeded", None));
        assert_eq!(kept.updated_at, now, "the moment it ended is kept");
        let failure = command(&mut conn, failed.id).await.unwrap().unwrap();
        assert_eq!(failure.error, Some(why), "a failure keeps what it says");
    }

    #[tokio::test]
    async fn finished_commands_go_at_most_a_batch_at_a_time_and_records_and_open_ones_stay() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (machine, now) = (machine(&mut conn).await, clock::start());
        let body = json!({});
        let mut ended = Vec::new();
        for _ in 0..3 {
            let command = new_command(machine, &body, now + Duration::seconds(10));
            insert_command(&mut conn, &command, now).await.unwrap();
            finish_command(&mut conn, command.id, &Ending::Expired, now)
                .await
                .unwrap();
            ended.push(command.id);
        }
        let open = new_command(machine, &body, now + Duration::seconds(10));
        insert_command(&mut conn, &open, now).await.unwrap();
        mark_accepted(&mut conn, open.id, now).await.unwrap();
        let attach_body = json!({ "environment_id": new_id() });
        let mut record = new_command(machine, &attach_body, now + Duration::seconds(10));
        record.kind = "env.attach";
        insert_command(&mut conn, &record, now).await.unwrap();
        finish_command(&mut conn, record.id, &Ending::Cancelled, now)
            .await
            .unwrap();
        let later = now + Duration::days(2);
        assert_eq!(purge_finished(&mut conn, later, 2).await.unwrap(), 2);
        assert_eq!(purge_finished(&mut conn, later, 2).await.unwrap(), 1);
        assert_eq!(purge_finished(&mut conn, later, 2).await.unwrap(), 0);
        for id in ended {
            assert!(command(&mut conn, id).await.unwrap().is_none());
        }
        assert!(
            command(&mut conn, open.id).await.unwrap().is_some(),
            "a command a machine still owes an answer to stays"
        );
        assert!(
            command(&mut conn, record.id).await.unwrap().is_some(),
            "what an environment was last given stays"
        );
    }
}
