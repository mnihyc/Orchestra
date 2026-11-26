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
    #[sqlx(try_from = "crate::storage::ExactJson")]
    pub body: Json,
    /// Its protected serial queue identity, for a command that uses one.
    #[sqlx(try_from = "crate::storage::OptionalText")]
    pub serial_key: Option<String>,
    /// Immutable physical target captured at admission and sent on each delivery.
    pub native_target: Option<sqlx::types::Json<orch_link_contract::commands::NativeTarget>>,
    /// Content uploaded for this command belongs only to its transfer until another owner adopts it.
    pub content_transit: bool,
    /// The original prerequisite a pipelined command waits for on its machine.
    pub after_command_id: Option<Id>,
    /// Where it stands: `queued`, `sent`, `accepted`, `succeeded`,
    /// `failed`, `expired` or `cancelled`.
    pub state: String,
    /// Queued, running with its key, or released, as the link acknowledgement reports.
    pub key_phase: i16,
    /// When the API first observed actual native work, rather than queue acceptance.
    pub work_started_at: Option<DateTime<Utc>>,
    /// What the machine answered, once it succeeded; none again once the
    /// answer of a read was cleared because the one who asked had it.
    #[sqlx(try_from = "crate::storage::OptionalJson")]
    pub result: Option<Json>,
    /// Why it failed.
    #[sqlx(try_from = "crate::storage::OptionalJson")]
    pub error: Option<Json>,
    /// How many times it was handed to a gateway.
    pub attempts: i32,
    /// The original creation-budget endpoint; subtract created_at to recover the work duration.
    pub deadline_at: DateTime<Utc>,
    /// Original credential/operation expiry, independent of the command's work budget.
    pub authority_until: Option<DateTime<Utc>>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When its state last changed.
    pub updated_at: DateTime<Utc>,
}

impl CommandRow {
    /// The authoritative queue identity is separate from command data.
    #[must_use]
    pub fn serial_key(&self) -> Option<&str> {
        self.serial_key.as_deref()
    }

    /// Whether nothing more is to happen to the command.
    #[must_use]
    pub fn is_over(&self) -> bool {
        !OPEN_STATES.contains(&self.state.as_str())
    }
}

/// The columns of a command, in the order [`CommandRow`] has them.
macro_rules! command_columns {
    ($($table:literal)?) => {
        concat!(
            $($table, ".",)? "id,",
            $($table, ".",)? "machine_id,",
            $($table, ".",)? "kind,",
            $($table, ".",)? "body,",
            $($table, ".",)? "serial_key,",
            $($table, ".",)? "tool_session->'native_target' AS native_target,",
            $($table, ".",)? "content_transit,",
            $($table, ".",)? "after_command_id,",
            $($table, ".",)? "state,",
            $($table, ".",)? "key_phase,",
            $($table, ".",)? "work_started_at,",
            $($table, ".",)? "result,",
            $($table, ".",)? "error,",
            $($table, ".",)? "attempts,",
            $($table, ".",)? "deadline_at,",
            $($table, ".",)? "authority_until,",
            $($table, ".",)? "created_at,",
            $($table, ".",)? "updated_at"
        )
    };
}
pub(crate) use command_columns;

/// What a command is made with.
#[derive(Debug, Clone, PartialEq)]
pub struct NewCommand<'a> {
    /// The original physical operand, when its producer captured it before a later queue phase.
    pub native_target: Option<&'a orch_link_contract::commands::NativeTarget>,
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
    /// Storage custody is protected command metadata, never inferred from native body members.
    pub content_transit: bool,
    /// The original creation-budget endpoint; subtract created_at to recover the work duration.
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
    insert_command_with_authority(conn, command, at, None).await
}

/// Stores private login evidence atomically with a new command. ON CONFLICT preserves every
/// canonical field, including absent evidence; retrying an ID cannot replace its accepted caller.
pub async fn insert_command_with_login_authority(
    conn: &mut Conn,
    command: &NewCommand<'_>,
    at: DateTime<Utc>,
    authority: Option<&Json>,
) -> Result<bool, DbError> {
    insert_command_with_authority(conn, command, at, authority).await
}

/// One private typed evidence slot, using the established column and canonical first-write rule.
pub async fn insert_command_with_authority(
    conn: &mut Conn,
    command: &NewCommand<'_>,
    at: DateTime<Utc>,
    authority: Option<&Json>,
) -> Result<bool, DbError> {
    insert_command_with_dependency(conn, command, at, authority, None).await
}

/// Preserve a command's prerequisite with its canonical first row.
pub async fn insert_command_with_dependency(
    conn: &mut Conn,
    command: &NewCommand<'_>,
    at: DateTime<Utc>,
    authority: Option<&Json>,
    after: Option<Id>,
) -> Result<bool, DbError> {
    if !crate::queries::transaction_guard::in_transaction(conn) {
        let mut tx = sqlx::Connection::begin(conn).await?;
        let inserted = insert_command_on(&mut tx, command, at, authority, after).await?;
        tx.commit().await?;
        return Ok(inserted);
    }
    insert_command_on(conn, command, at, authority, after).await
}

async fn insert_command_on(
    conn: &mut Conn,
    command: &NewCommand<'_>,
    at: DateTime<Utc>,
    authority: Option<&Json>,
    after: Option<Id>,
) -> Result<bool, DbError> {
    if command.native_target.is_some() {
        let existing: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM commands WHERE id=$1)")
                .bind(command.id)
                .fetch_one(&mut *conn)
                .await?;
        if existing {
            return Ok(false);
        }
        if matches!(
            command.native_target,
            Some(orch_link_contract::native_target::NativeTarget::Session(_))
        ) && orch_link_contract::native_target::session_action(command.kind, command.body)
            .is_none()
        {
            return Err(DbError::Rule {
                constraint: "native_target_command_kind".into(),
            });
        }
    }
    let lifecycle = crate::queries::runtime_controls::command_facts(&mut *conn, command).await?;
    let inserted = sqlx::query(
        "INSERT INTO commands (id, machine_id, kind, body, serial_key, deadline_at, created_at, updated_at, login_authority, authority_required, content_transit, after_command_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $7, $8, $9, $10, $11)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(command.id)
    .bind(command.machine_id)
    .bind(command.kind)
    .bind(orch_types::storable::encode_exact(command.body))
    .bind(command.serial_key.map(orch_types::storable::encode_key))
    .bind(command.deadline_at)
    .bind(at)
    .bind(authority)
    .bind(authority.is_some())
    .bind(command.content_transit)
    .bind(after)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if inserted > 0
        && let Some(target) = lifecycle
    {
        sqlx::query("UPDATE commands SET tool_session=COALESCE(tool_session,'{}'::jsonb)||jsonb_build_object('native_target',$2::jsonb) WHERE id=$1")
            .bind(command.id).bind(target).execute(&mut *conn).await?;
    } else if inserted > 0 {
        crate::queries::session_dependencies::admit(
            conn,
            command.id,
            command.kind,
            command.body,
            command.native_target,
        )
        .await?;
    }
    Ok(inserted > 0)
}

/// Private login-origin evidence is deliberately excluded from CommandRow and wire/DTO projections.
pub async fn command_login_authority(conn: &mut Conn, id: Id) -> Result<Option<Json>, DbError> {
    command_authority(conn, id).await
}

/// The established private value, never inferred from a wire body or a missing field.
pub async fn command_authority(conn: &mut Conn, id: Id) -> Result<Option<Json>, DbError> {
    Ok(
        sqlx::query_scalar::<_, Option<Json>>("SELECT login_authority FROM commands WHERE id=$1")
            .bind(id)
            .fetch_optional(conn)
            .await?
            .flatten(),
    )
}

/// Conjoin private origin restrictions on the same locked canonical command.
/// The typed owner verifies immutable identity/source first; no wire body or result is rewritten.
pub async fn narrow_command_authority(
    conn: &mut Conn,
    id: Id,
    expected: &Json,
    narrowed: &Json,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE commands SET login_authority=$3,updated_at=$4 WHERE id=$1 AND authority_required AND login_authority=$2")
        .bind(id).bind(expected).bind(narrowed).bind(at).execute(conn).await?.rows_affected()==1)
}

/// Independent private marker: losing required evidence cannot turn a new command into old work.
pub async fn command_authority_required(conn: &mut Conn, id: Id) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT authority_required FROM commands WHERE id=$1")
            .bind(id)
            .fetch_optional(conn)
            .await?
            .unwrap_or(false),
    )
}

/// Persist only a tighter original time bound; null is unbounded, never a reason to
/// replace an earlier finite bound. This fact supplies no command or permission.
pub async fn narrow_command_authority_until(
    conn: &mut Conn,
    id: Id,
    until: Option<DateTime<Utc>>,
) -> Result<Option<DateTime<Utc>>, DbError> {
    Ok(sqlx::query_scalar("UPDATE commands SET authority_until=LEAST(authority_until,$2) WHERE id=$1 RETURNING authority_until")
        .bind(id).bind(until).fetch_one(conn).await?)
}

/// Private controller completion of a real MCP copy, excluded from wire/public command rows.
pub async fn mcp_import_completion(conn: &mut Conn, id: Id) -> Result<Option<Json>, DbError> {
    Ok(sqlx::query_scalar::<_, Option<Json>>(
        "SELECT mcp_import_completion FROM commands WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?
    .flatten())
}

/// First completion stands in the same transaction as native outcome/resource/audit reconciliation.
pub async fn finish_mcp_import(conn: &mut Conn, id: Id, outcome: &Json) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE commands SET mcp_import_completion=$2 WHERE id=$1 AND kind='mcp.import' AND state='succeeded' AND mcp_import_completion IS NULL")
        .bind(id).bind(outcome).execute(conn).await?.rows_affected()==1)
}

/// Server-private row/envelope read; unlike CommandRow it cannot be serialized to a DTO.
#[derive(sqlx::FromRow)]
pub struct CommandWithAuthority {
    /// Existing canonical public/native fields.
    #[sqlx(flatten)]
    pub command: CommandRow,
    /// Original private evidence from the same statement.
    pub authority: Option<Json>,
}
impl std::fmt::Debug for CommandWithAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CommandWithAuthority([private])")
    }
}

/// Read canonical command and private value atomically without expanding public CommandRow.
pub async fn command_with_authority(
    conn: &mut Conn,
    id: Id,
) -> Result<Option<CommandWithAuthority>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        command_columns!(),
        ",login_authority AS authority FROM commands WHERE id=$1"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// A kind-specific native completion need not materialize unrelated command bodies or results.
pub async fn command_with_authority_kind(
    conn: &mut Conn,
    id: Id,
    kind: &str,
) -> Result<Option<CommandWithAuthority>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        command_columns!(),
        ",login_authority AS authority FROM commands WHERE id=$1 AND kind=$2"
    ))
    .bind(id)
    .bind(kind)
    .fetch_optional(conn)
    .await?)
}

/// Lock the same private canonical record at its existing unaccepted publication checkpoint.
pub async fn lock_command_with_authority(
    conn: &mut Conn,
    id: Id,
) -> Result<Option<CommandWithAuthority>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        command_columns!(),
        ",login_authority AS authority FROM commands WHERE id=$1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
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

/// Install the dispatch boundary in the original admission transaction.
pub async fn set_start_adoption_deadline(
    conn: &mut Conn,
    id: Id,
    due: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE commands SET start_adoption_until=$2 WHERE id=$1")
        .bind(id)
        .bind(due)
        .execute(conn)
        .await?;
    Ok(())
}

/// A side session may have admitted its command before ordinary turn preparation.
pub async fn set_side_start_adoption(conn: &mut Conn, id: Id, turn: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE commands c SET start_adoption_until=(t.start_wait->>'deadline')::timestamptz FROM turns t WHERE t.id=$2 AND c.id=$1 AND t.start_wait->>'command'=$1::uuid::text")
        .bind(id).bind(turn).execute(conn).await?;
    Ok(())
}

/// Transfer a queued start to the dispatcher exactly once. Taking the time after
/// the command lock also covers a delayed issuer racing recovery on another API.
pub async fn adopt_start(
    conn: &mut Conn,
    id: Id,
    clock: &dyn orch_types::Clock,
) -> Result<bool, DbError> {
    let due = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT start_adoption_until FROM commands WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .flatten();
    let Some(due) = due else { return Ok(true) };
    if clock.now() >= due {
        return Ok(false);
    }
    Ok(sqlx::query("UPDATE commands SET start_adoption_until=NULL WHERE id=$1 AND state='queued' AND NOT EXISTS (
        SELECT 1 FROM thread_stops st WHERE st.turn_id=COALESCE(commands.body->>'turn_id', commands.body#>>'{turn,turn_id}')::uuid AND st.applied_at IS NULL)")
        .bind(id).execute(conn).await?.rows_affected()==1)
}

/// Locks one canonical command while its deferred first-send authorization is checked.
pub async fn lock_command(conn: &mut Conn, id: Id) -> Result<Option<CommandRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        command_columns!(),
        " FROM commands WHERE id=$1 FOR UPDATE"
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
          WHERE machine_id = $1 AND start_adoption_until IS NULL AND state IN ('queued', 'sent', 'accepted')
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
    mark_accepted_phase(conn, id, 0, at).await
}

/// Key phases advance independently of the terminal result; replay cannot reacquire a released key.
pub async fn mark_accepted_phase(
    conn: &mut Conn,
    id: Id,
    phase: i16,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let changed = sqlx::query("UPDATE commands SET state = 'accepted', key_phase = GREATEST(key_phase, $3),
        work_started_at = CASE WHEN $3 >= 1 THEN COALESCE(work_started_at, $2) ELSE work_started_at END, updated_at = $2
        WHERE id = $1 AND (state IN ('queued', 'sent') OR state = 'accepted' AND key_phase < $3)")
        .bind(id).bind(at).bind(phase).execute(conn).await?.rows_affected();
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
    if !crate::queries::transaction_guard::in_transaction(conn) {
        let mut tx = sqlx::Connection::begin(conn).await?;
        let changed = finish_command_on(&mut tx, id, ending, at).await?;
        tx.commit().await?;
        return Ok(changed);
    }
    finish_command_on(conn, id, ending, at).await
}

async fn finish_command_on(
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
    let result = result.map(orch_types::storable::encode_exact);
    let error = error.map(orch_types::storable::encode_exact);
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
        let body = orch_types::storable::decode_exact(body.clone())?;
        crate::queries::session_dependencies::record(conn, id, kind, &body, *issued, at).await?;
    } else if let Some((kind, body, _)) = &changed {
        crate::queries::session_dependencies::failed_open(
            conn,
            id,
            kind,
            &orch_types::storable::decode_exact(body.clone())?,
        )
        .await?;
    }
    Ok(changed.is_some())
}

/// Retains the safe reason when a private native reply still cannot be stored.
pub async fn record_command_storage_gap(
    conn: &mut Conn,
    id: Id,
    code: &str,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO command_storage_gaps(command_id,sqlstate,recorded_at) VALUES($1,$2,$3) ON CONFLICT DO NOTHING")
        .bind(id).bind(code).bind(at).execute(conn).await?;
    Ok(())
}

/// The oldest unfinished command ahead of an accepted command in its machine's line.
/// A command already releasing its result holds no place in that line. An unsent command
/// waits for its machine rather than for work that machine has accepted.
pub async fn held_up_by(
    conn: &mut Conn,
    command: Id,
) -> Result<Option<(String, DateTime<Utc>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT ahead.kind, ahead.created_at
           FROM commands me
           JOIN commands ahead ON ahead.machine_id = me.machine_id AND ahead.serial_key = me.serial_key
          WHERE me.id = $1 AND me.serial_key IS NOT NULL AND me.state = 'accepted' AND me.key_phase = 0
            AND ahead.state = 'accepted' AND ahead.key_phase < 2
            AND (ahead.created_at, ahead.id) < (me.created_at, me.id)
          ORDER BY ahead.created_at, ahead.id
          LIMIT 1",
    )
    .bind(command)
    .fetch_optional(conn)
    .await?)
}

/// The machine's actual key owner, whose elapsed work may pause a queued command's patience.
/// Accepted commands that have not acquired the key still appear in the line, but cannot
/// grant unlimited time to another command merely because their acknowledgement arrived first.
pub async fn key_holder(
    conn: &mut Conn,
    command: Id,
) -> Result<Option<(String, DateTime<Utc>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT ahead.kind, ahead.created_at
           FROM commands me
           JOIN commands ahead ON ahead.machine_id = me.machine_id AND ahead.serial_key = me.serial_key
          WHERE me.id = $1 AND me.serial_key IS NOT NULL AND me.state = 'accepted' AND me.key_phase = 0
            AND ahead.id <> me.id AND ahead.state = 'accepted' AND ahead.key_phase = 1
          ORDER BY ahead.created_at, ahead.id
          LIMIT 1",
    )
    .bind(command)
    .fetch_optional(conn)
    .await?)
}

/// Expire an undelivered command when its original authority or its tool answer delivery period ends.
pub async fn expire_undelivered(
    conn: &mut Conn,
    now: DateTime<Utc>,
    followed: &[Id],
) -> Result<bool, DbError> {
    let expired = sqlx::query(
        "UPDATE commands SET state='expired',updated_at=$1
         WHERE state IN ('queued','sent') AND NOT (id=ANY($2))
           AND (authority_until <= $1 OR (kind='orch.tool_result' AND deadline_at <= $1))",
    )
    .bind(now)
    .bind(followed)
    .execute(conn)
    .await?;
    Ok(expired.rows_affected() != 0)
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
/// Successful custody-transfer receipts remain while their provider still holds that custody,
/// so a retry can prove its actual moved material; ordinary reaping resumes when custody ends.
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
              AND NOT EXISTS (SELECT 1 FROM commands dependent
                  WHERE dependent.after_command_id=c.id
                    AND dependent.state IN ('queued','sent','accepted'))
              AND NOT (c.kind IN ('agent.credential.return','provider.credential.accept')
                  AND c.state='succeeded' AND c.result IS NOT NULL
                  AND EXISTS(SELECT 1 FROM providers p WHERE p.id::text=c.body->>'provider_id'
                      AND p.deleted_at IS NULL AND p.held_by_environment_id IS NOT NULL
                      AND c.created_at >= p.held_since))
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
            content_transit: false,
            native_target: None,
            id: new_id(),
            machine_id: machine,
            kind: "machine.probe",
            body,
            serial_key: None,
            deadline_at: deadline,
        }
    }

    #[tokio::test]
    async fn an_offline_tool_answer_expires_with_its_body_available_for_the_next_attempt() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let owner = machine(&mut conn).await;
        let now = clock::start();
        let deadline = now + Duration::seconds(30);
        let body = json!({"call_ref":"repeat","result":{"ok":true}});
        let mut answer = new_command(owner, &body, deadline);
        answer.kind = "orch.tool_result";
        insert_command(&mut conn, &answer, now).await.unwrap();
        mark_sent(&mut conn, answer.id, now).await.unwrap();
        let native = new_command(owner, &body, deadline);
        insert_command(&mut conn, &native, now).await.unwrap();

        assert!(!expire_undelivered(&mut conn, now, &[]).await.unwrap());
        assert!(
            !expire_undelivered(&mut conn, deadline, &[answer.id])
                .await
                .unwrap()
        );
        assert!(expire_undelivered(&mut conn, deadline, &[]).await.unwrap());
        let expired = command(&mut conn, answer.id).await.unwrap().unwrap();
        assert_eq!(expired.state, "expired");
        assert_eq!(expired.body, body);
        assert_eq!(
            command(&mut conn, native.id).await.unwrap().unwrap().state,
            "queued"
        );
        assert!(!expire_undelivered(&mut conn, deadline, &[]).await.unwrap());
        narrow_command_authority_until(&mut conn, native.id, Some(deadline))
            .await
            .unwrap();
        assert!(expire_undelivered(&mut conn, deadline, &[]).await.unwrap());
        assert_eq!(
            command(&mut conn, native.id).await.unwrap().unwrap().state,
            "expired"
        );
    }

    #[tokio::test]
    async fn a_transfer_commands_content_lifetime_survives_reload_without_appearing_in_its_body() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let owner = machine(&mut conn).await;
        let body = json!({"content_transit":false});
        let mut asked = new_command(owner, &body, clock::start() + Duration::seconds(30));
        asked.content_transit = true;
        assert!(
            insert_command(&mut conn, &asked, clock::start())
                .await
                .unwrap()
        );
        asked.content_transit = false;
        assert!(
            !insert_command(&mut conn, &asked, clock::start())
                .await
                .unwrap()
        );
        drop(conn);
        let row = command(&mut test.pool.acquire().await.unwrap(), asked.id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            row.content_transit,
            "retrying an id preserves its original content custody"
        );
        assert_eq!(
            row.body, body,
            "the body is ordinary native data and cannot select storage custody"
        );
    }

    #[tokio::test]
    async fn a_command_reload_uses_only_its_protected_queue_key() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let owner = machine(&mut conn).await;
        let body = json!({"$key":"native field"});
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
            row.body, body,
            "native fields stay ordinary data beside the protected queue identity"
        );
        // Payload members never select a protected queue, including when the column has no value.
        let native = json!({"$key":"native field without a queue"});
        sqlx::query("UPDATE commands SET serial_key=NULL,body=$2 WHERE id=$1")
            .bind(id)
            .bind(&native)
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
        assert_eq!(row.serial_key(), None);
        assert_eq!(row.body, native, "reloading does not rewrite native fields");
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

    #[tokio::test]
    async fn an_uploading_recording_no_longer_holds_the_next_recordings_patience() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let owner = machine(&mut conn).await;
        let now = clock::start();
        let body = json!({});
        let mut first = new_command(owner, &body, now + Duration::seconds(30));
        first.kind = "fs.record";
        first.serial_key = Some("environment");
        let mut next = new_command(owner, &body, now + Duration::seconds(30));
        next.serial_key = first.serial_key;
        insert_command(&mut conn, &first, now).await.unwrap();
        insert_command(&mut conn, &next, now + Duration::seconds(1))
            .await
            .unwrap();
        mark_accepted(&mut conn, first.id, now).await.unwrap();
        mark_accepted(&mut conn, next.id, now).await.unwrap();
        assert!(held_up_by(&mut conn, next.id).await.unwrap().is_some());
        assert!(key_holder(&mut conn, next.id).await.unwrap().is_none());
        mark_accepted_phase(&mut conn, first.id, 1, now)
            .await
            .unwrap();
        assert!(held_up_by(&mut conn, next.id).await.unwrap().is_some());
        assert!(key_holder(&mut conn, next.id).await.unwrap().is_some());
        mark_accepted_phase(&mut conn, first.id, 2, now)
            .await
            .unwrap();
        assert!(
            held_up_by(&mut conn, next.id).await.unwrap().is_none(),
            "the older upload no longer owns the environment key"
        );
        assert!(key_holder(&mut conn, next.id).await.unwrap().is_none());
        assert!(
            !mark_accepted_phase(&mut conn, first.id, 0, now)
                .await
                .unwrap()
        );
        assert_eq!(
            command(&mut conn, first.id)
                .await
                .unwrap()
                .unwrap()
                .key_phase,
            2
        );
    }
}
