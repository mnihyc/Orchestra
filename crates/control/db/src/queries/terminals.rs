//! A person's shells in an environment: the sessions with whether somebody is attached, how each ended and the last of what it printed, the counts the limits are checked against, and what the sweeps look for.
//!
//! A shell is a row from the moment it is asked for. It is `opening` until its machine has started it, `open`
//! while it runs, attached to a screen or not, and `closed` for good; it moves in that one direction only, and
//! every move is a compare-and-set, so that two processes that both find a shell idle close it once. Nothing
//! here decides who may open or attach to a shell: these are the statements the rules are made of.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Hash, Id, Json};

/// The states of a session, as stored.
pub mod state {
    /// Being started: the row holds its place against the limits while the machine opens it.
    pub const OPENING: &str = "opening";
    /// Running.
    pub const OPEN: &str = "open";
    /// Over.
    pub const CLOSED: &str = "closed";
}

/// How a session ended, as stored.
pub mod reason {
    /// The shell's process ended.
    pub const EXITED: &str = "exited";
    /// Its person, or whoever manages its environment, closed it.
    pub const CLOSED_BY_USER: &str = "closed_by_user";
    /// Nobody was attached for as long as shells are kept.
    pub const DETACHED: &str = "detached";
    /// Its environment was stopped, recreated or removed.
    pub const ENVIRONMENT_STOPPED: &str = "environment_stopped";
    /// The connector of its environment was restarted under it.
    pub const LINK_LOST: &str = "link_lost";
    /// Its opener lost the permission, or a condition under which it was opened.
    pub const REVOKED: &str = "revoked";
    /// It could not be kept running.
    pub const ERROR: &str = "error";
}

/// The columns of a session, in the order [`SessionRow`] reads them.
const COLUMNS: &str = "id, thread_id, environment_id, env_generation, opened_by, shell, cwd, cols, rows, state, attached,
    detached_at, close_reason, exit_code, bytes_in, bytes_out, recording_blob, opened_at, last_io_at, closed_at";

/// A shell as it is stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct SessionRow {
    /// Its identifier.
    pub id: Id,
    /// The thread it was opened from; none when it was opened from the environment's own page, or the
    /// thread has gone.
    pub thread_id: Option<Id>,
    /// The environment it runs in.
    pub environment_id: Id,
    /// How many times the environment had been made when the shell was opened.
    pub env_generation: i32,
    /// Whose it is: the only one who attaches to it.
    pub opened_by: Id,
    /// The program it runs; empty until the machine has started it.
    pub shell: String,
    /// The directory it started in.
    pub cwd: String,
    /// The width of its screen.
    pub cols: i32,
    /// The height of its screen.
    pub rows: i32,
    /// `opening`, `open` or `closed`.
    pub state: String,
    /// Whether its person has it on a screen now.
    pub attached: bool,
    /// Since when nobody has been attached; none while somebody is, and once it is closed.
    pub detached_at: Option<DateTime<Utc>>,
    /// How it ended.
    pub close_reason: Option<String>,
    /// What its process exited with.
    pub exit_code: Option<i32>,
    /// How many bytes were typed into it through Orch.
    pub bytes_in: i64,
    /// How many bytes it printed.
    pub bytes_out: i64,
    /// The last of what it printed, as it was kept when it closed.
    pub recording_blob: Option<Vec<u8>>,
    /// When it was asked for.
    pub opened_at: DateTime<Utc>,
    /// When something last went in or out.
    pub last_io_at: DateTime<Utc>,
    /// When it ended.
    pub closed_at: Option<DateTime<Utc>>,
}

impl SessionRow {
    /// Whether it has not ended.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.state != state::CLOSED
    }

    /// The hash of the last of what it printed, where that was kept.
    #[must_use]
    pub fn recording(&self) -> Option<Hash> {
        self.recording_blob
            .as_deref()
            .and_then(|bytes| Hash::from_slice(bytes).ok())
    }
}

// ---------------------------------------------------------------- the environment a shell is opened in

/// What opening a shell needs to know of the environment and of the machine behind it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Site {
    /// The environment.
    pub id: Id,
    /// The root org of its home.
    pub org_id: Id,
    /// The node it takes its settings from.
    pub home_node_id: Id,
    /// `managed` or `connected`.
    pub placement: String,
    /// The absolute path of its workspace.
    pub workspace_root: String,
    /// How many times it was made anew.
    pub generation: i32,
    /// Where it stands.
    pub status: String,
    /// Whether shells may be opened in it: its own switch.
    pub terminal: bool,
    /// The machine it is on.
    pub machine_id: Option<Id>,
    /// Whether that machine has a session now.
    pub machine_online: Option<bool>,
    /// What the machine's local policy says of shells from outside (`environment.terminal`), where the
    /// machine has reported its policy.
    pub machine_allows_terminal: Option<Json>,
    /// When it was deleted.
    pub deleted_at: Option<DateTime<Utc>>,
}

/// The environment `id` with the machine it is on, not deleted.
pub async fn site(conn: &mut Conn, id: Id) -> Result<Option<Site>, DbError> {
    Ok(sqlx::query_as(
        "SELECT e.id, e.org_id, e.home_node_id, e.placement, e.workspace_root, e.generation, e.status, COALESCE(e.terminal,true) AS terminal,
                e.machine_id, m.online AS machine_online,
                COALESCE(hm.facts,m.facts) -> 'policy' -> 'environment.terminal' AS machine_allows_terminal, e.deleted_at
           FROM environments e LEFT JOIN machines m ON m.id = e.machine_id
           LEFT JOIN LATERAL (SELECT r.host_id FROM runtimes r WHERE r.environment_id=e.id AND r.state NOT IN ('retired','removed') AND r.successor_of IS NULL LIMIT 1) policy_runtime ON true
           LEFT JOIN hosts policy_host ON policy_host.id=policy_runtime.host_id LEFT JOIN machines hm ON hm.id=policy_host.machine_id
          WHERE e.id = $1 AND e.deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Takes the lock that orders what is done about `key` until the transaction ends: the openings of shells
/// in an environment, so that two requests that both find room for one more cannot both take it, or the
/// reads of one shell's last output, so that two readers write one line.
pub async fn lock(conn: &mut Conn, key: Id) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('terminals:' || $1::text, 0))")
        .bind(key)
        .execute(conn)
        .await?;
    Ok(())
}

/// Records that something happened in the environment, which keeps it from an idle stop: a shell's
/// input and output count as the environment's activity.
pub async fn touch_environment(
    conn: &mut Conn,
    environment: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE environments SET last_activity_at = GREATEST(COALESCE(last_activity_at, $2), $2)
          WHERE id = $1",
    )
    .bind(environment)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- opening

/// A shell that is asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct NewSession<'a> {
    /// Its identifier.
    pub id: Id,
    /// The thread it is opened from.
    pub thread_id: Option<Id>,
    /// The environment.
    pub environment_id: Id,
    /// The environment's generation now.
    pub env_generation: i32,
    /// Who opens it.
    pub opened_by: Id,
    /// The directory it starts in.
    pub cwd: &'a str,
    /// The width of its screen.
    pub cols: i32,
    /// The height of its screen.
    pub rows: i32,
}

/// Writes the row of a shell that is being opened. Nobody is attached to it yet, and it counts from
/// `at` as left alone, so that a shell whose person never comes is closed like any other.
pub async fn insert(
    conn: &mut Conn,
    new: &NewSession<'_>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO terminal_sessions (id, thread_id, environment_id, env_generation, opened_by, shell, cwd, cols,
                                        rows, state, attached, detached_at, opened_at, last_io_at)
         VALUES ($1, $2, $3, $4, $5, '', $6, $7, $8, 'opening', false, $9, $9, $9)",
    )
    .bind(new.id)
    .bind(new.thread_id)
    .bind(new.environment_id)
    .bind(new.env_generation)
    .bind(new.opened_by)
    .bind(new.cwd)
    .bind(new.cols)
    .bind(new.rows)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// The machine has started the shell: `opening` becomes `open`. False where it was closed meanwhile.
pub async fn opened(
    conn: &mut Conn,
    id: Id,
    shell: &str,
    at: DateTime<Utc>,
) -> Result<Option<SessionRow>, DbError> {
    let sql = format!(
        "UPDATE terminal_sessions SET state = 'open', shell = $2, detached_at = $3, last_io_at = $3
          WHERE id = $1 AND state = 'opening' RETURNING {COLUMNS}"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(shell)
        .bind(at)
        .fetch_optional(conn)
        .await?)
}

/// Forgets a shell that never opened: the machine could not start it, so nothing happened that a
/// history would say.
pub async fn drop_opening(conn: &mut Conn, id: Id) -> Result<bool, DbError> {
    Ok(
        sqlx::query("DELETE FROM terminal_sessions WHERE id = $1 AND state = 'opening'")
            .bind(id)
            .execute(conn)
            .await?
            .rows_affected()
            > 0,
    )
}

// ---------------------------------------------------------------- reading

/// The shell `id`.
pub async fn get(conn: &mut Conn, id: Id) -> Result<Option<SessionRow>, DbError> {
    let sql = format!("SELECT {COLUMNS} FROM terminal_sessions WHERE id = $1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// The shells `ids`, in no order.
pub async fn get_many(conn: &mut Conn, ids: &[Id]) -> Result<Vec<SessionRow>, DbError> {
    let sql = format!("SELECT {COLUMNS} FROM terminal_sessions WHERE id = ANY($1)");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(ids)
        .fetch_all(conn)
        .await?)
}

/// The shells of `environment` that have not ended, whoever's they are, the one used longest ago
/// first: what the limits count, and what a person at their limit is shown of their own.
pub async fn live_in(conn: &mut Conn, environment: Id) -> Result<Vec<SessionRow>, DbError> {
    let sql = format!(
        "SELECT {COLUMNS} FROM terminal_sessions WHERE environment_id = $1 AND state <> 'closed'
          ORDER BY last_io_at, id"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(environment)
        .fetch_all(conn)
        .await?)
}

/// Which of a person's shells a list is to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    /// Those that have not ended.
    Live,
    /// Those that have.
    Ended,
    /// All of them.
    Any,
}

/// Where a page of a list begins: after the shell opened at `at`, with the identifier `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct After {
    /// When the last shell of the page before was opened.
    pub at: DateTime<Utc>,
    /// Its identifier.
    pub id: Id,
}

/// The shells of `environment`, newest first, `limit` of them after `after`: those of `opener` alone,
/// or everybody's where none is named.
pub async fn list(
    conn: &mut Conn,
    environment: Id,
    opener: Option<Id>,
    which: Which,
    after: Option<After>,
    limit: i64,
) -> Result<Vec<SessionRow>, DbError> {
    let live = match which {
        Which::Live => Some(true),
        Which::Ended => Some(false),
        Which::Any => None,
    };
    let sql = format!(
        "SELECT {COLUMNS} FROM terminal_sessions
          WHERE environment_id = $1
            AND ($2::uuid IS NULL OR opened_by = $2)
            AND ($3::boolean IS NULL OR $3 = (state <> 'closed'))
            AND ($4::timestamptz IS NULL OR (opened_at, id) < ($4, $5::uuid))
          ORDER BY opened_at DESC, id DESC
          LIMIT $6"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(environment)
        .bind(opener)
        .bind(live)
        .bind(after.map(|after| after.at))
        .bind(after.map(|after| after.id))
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

// ---------------------------------------------------------------- attachment

/// A screen takes the shell up. None where it is not open (it ended, or is not started yet).
pub async fn attach(
    conn: &mut Conn,
    id: Id,
    at: DateTime<Utc>,
) -> Result<Option<SessionRow>, DbError> {
    let sql = format!(
        "UPDATE terminal_sessions SET attached = true, detached_at = NULL, last_io_at = GREATEST(last_io_at, $2)
          WHERE id = $1 AND state = 'open' RETURNING {COLUMNS}"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(at)
        .fetch_optional(conn)
        .await?)
}

/// Nobody has the shell on a screen any longer. The moment is kept if somebody had it; a shell that
/// was left already keeps the moment it was first left.
pub async fn detach(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE terminal_sessions SET attached = false, detached_at = COALESCE(detached_at, $2)
          WHERE id = $1 AND state = 'open' AND attached",
    )
    .bind(id)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Says that bytes went through an attachment: how many each way since the last word, and when. It
/// says nothing of whether the shell is attached: the screen that ends tells that, once.
pub async fn touch(
    conn: &mut Conn,
    id: Id,
    typed: i64,
    printed: i64,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE terminal_sessions
            SET last_io_at = GREATEST(last_io_at, $4), bytes_in = bytes_in + $2, bytes_out = bytes_out + $3
          WHERE id = $1 AND state = 'open'",
    )
    .bind(id)
    .bind(typed)
    .bind(printed)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// The screen has a new size.
pub async fn resized(conn: &mut Conn, id: Id, cols: i32, rows: i32) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE terminal_sessions SET cols = $2, rows = $3 WHERE id = $1 AND state <> 'closed'",
    )
    .bind(id)
    .bind(cols)
    .bind(rows)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

// ---------------------------------------------------------------- ending

/// How a shell ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Closing<'a> {
    /// One of [`reason`].
    pub reason: &'a str,
    /// What its process exited with, where that is known.
    pub exit_code: Option<i64>,
    /// How many bytes it printed in all, as its machine counted them.
    pub bytes_out: Option<i64>,
    /// The last of what it printed, where that was kept.
    pub recording: Option<Hash>,
}

/// The largest exit code that is stored: the column holds 32 bits.
fn stored_code(code: i64) -> i32 {
    i32::try_from(code).unwrap_or(if code < 0 { i32::MIN } else { i32::MAX })
}

/// Ends the shell. The row that was closed is returned, and none where it was closed already: of
/// two that end a shell at once, one writes its line of the history.
pub async fn close(
    conn: &mut Conn,
    id: Id,
    closing: &Closing<'_>,
    at: DateTime<Utc>,
) -> Result<Option<SessionRow>, DbError> {
    let sql = format!(
        "UPDATE terminal_sessions
            SET state = 'closed', attached = false, detached_at = NULL, close_reason = $2, exit_code = $3,
                bytes_out = GREATEST(bytes_out, COALESCE($4, 0)), recording_blob = $5, closed_at = $6
          WHERE id = $1 AND state <> 'closed' RETURNING {COLUMNS}"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(closing.reason)
        .bind(closing.exit_code.map(stored_code))
        .bind(closing.bytes_out)
        .bind(
            closing
                .recording
                .as_ref()
                .map(|hash| hash.as_bytes().to_vec()),
        )
        .bind(at)
        .fetch_optional(conn)
        .await?)
}

/// Keeps `hash` as the last output of a shell if the store has it, and holds it there: the content a
/// shell's line points at is not collected while the line exists. False where the store has no such
/// blob.
pub async fn pin_recording(conn: &mut Conn, hash: &Hash) -> Result<bool, DbError> {
    Ok(
        sqlx::query("UPDATE blobs SET pin_count = pin_count + 1 WHERE hash = $1")
            .bind(hash.as_bytes().to_vec())
            .execute(conn)
            .await?
            .rows_affected()
            > 0,
    )
}

/// Makes the store know a blob that was just written to it, and holds it for a shell's line.
pub async fn record_blob(
    conn: &mut Conn,
    hash: &Hash,
    size: i64,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO blobs (hash, size, stored_at, last_access_at, pin_count) VALUES ($1, $2, $3, $3, 1)
         ON CONFLICT (hash) DO UPDATE SET pin_count = blobs.pin_count + 1, last_access_at = $3",
    )
    .bind(hash.as_bytes().to_vec())
    .bind(size)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Lets go of a blob that a shell's line no longer points at.
pub async fn unpin_recording(conn: &mut Conn, hash: &Hash) -> Result<(), DbError> {
    sqlx::query("UPDATE blobs SET pin_count = GREATEST(pin_count - 1, 0) WHERE hash = $1")
        .bind(hash.as_bytes().to_vec())
        .execute(conn)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------- what the sweeps look for

/// The shells that are open and that nobody has been attached to since `before` or earlier. Which of
/// them are due depends on the setting of the place each was opened at, which the caller reads.
pub async fn left_since(
    conn: &mut Conn,
    before: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<SessionRow>, DbError> {
    let sql = format!(
        "SELECT {COLUMNS} FROM terminal_sessions
          WHERE state = 'open' AND NOT attached AND detached_at IS NOT NULL AND detached_at <= $1
          ORDER BY detached_at, id LIMIT $2"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(before)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// The shells that were asked for before `before` and are still being started: the process that asked
/// for them went away.
pub async fn stale_opening(
    conn: &mut Conn,
    before: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<SessionRow>, DbError> {
    let sql = format!(
        "SELECT {COLUMNS} FROM terminal_sessions WHERE state = 'opening' AND opened_at <= $1
          ORDER BY opened_at, id LIMIT $2"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(before)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// The shells that have not ended whose environment has: it is removed, made again since the shell
/// opened, or stands in a state in which nothing runs (stopped, being made again or moved, failed,
/// disabled). An environment that is `offline` is only out of reach for now, and its shells go on.
pub async fn of_ended_environments(
    conn: &mut Conn,
    limit: i64,
) -> Result<Vec<SessionRow>, DbError> {
    let columns = COLUMNS
        .split(", ")
        .map(|column| format!("s.{}", column.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT {columns} FROM terminal_sessions s JOIN environments e ON e.id = s.environment_id
          WHERE s.state <> 'closed'
            AND (e.deleted_at IS NOT NULL OR e.generation <> s.env_generation
                 OR e.status NOT IN ('running', 'offline'))
          ORDER BY s.opened_at, s.id LIMIT $1"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Every shell that has not ended, for the check of the gates it was opened under.
pub async fn all_live(conn: &mut Conn) -> Result<Vec<SessionRow>, DbError> {
    let sql = format!(
        "SELECT {COLUMNS} FROM terminal_sessions WHERE state <> 'closed' ORDER BY opened_at, id"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .fetch_all(conn)
        .await?)
}

// ---------------------------------------------------------------- reading what a shell last printed

/// Whether `reader` has read what `session` printed within `[from, until)`: a read by anybody but the
/// shell's opener is a line of the history once for each reader, shell and day.
pub async fn read_between(
    conn: &mut Conn,
    reader: Id,
    session: Id,
    from: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM audit_log
                         WHERE action = 'terminal.recording_read' AND actor_kind = 'user' AND actor_id = $1
                           AND object_id = $2 AND at >= $3 AND at < $4)",
    )
    .bind(reader)
    .bind(session)
    .bind(from)
    .bind(until)
    .fetch_one(conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::hierarchy::testing::insert_node;
    use crate::queries::identity::testing::insert_user;
    use orch_testkit::database::TestDb;
    use orch_types::new_id;

    /// A person, an org, and an environment of theirs with a machine behind it.
    async fn environment(conn: &mut Conn) -> (Id, Id) {
        let (person, org, machine, environment) = (new_id(), new_id(), new_id(), new_id());
        insert_user(conn, person, "ada").await.unwrap();
        insert_node(conn, org, "org", None, "Acme", person)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO machines (id, name, owner_id, placement, platform_os, platform_arch, connector_version,
                                   protocol_version, cert_serial, cert_fingerprint, cert_not_after)
             VALUES ($1, 'laptop', $2, 'connected', 'linux', 'x86_64', '0.1.0', 1, 'serial', $3, now() + interval '1 day')",
        )
        .bind(machine)
        .bind(person)
        .bind(new_id().as_bytes().to_vec())
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, machine_id, status)
             VALUES ($1, 'Work', $2, $3, 'connected', $3, $4, 'running')",
        )
        .bind(environment)
        .bind(person)
        .bind(org)
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
        (person, environment)
    }

    async fn shell(conn: &mut Conn, person: Id, environment: Id, at: DateTime<Utc>) -> Id {
        let id = new_id();
        insert(
            conn,
            &NewSession {
                id,
                thread_id: None,
                environment_id: environment,
                env_generation: 0,
                opened_by: person,
                cwd: "/work",
                cols: 80,
                rows: 24,
            },
            at,
        )
        .await
        .unwrap();
        opened(conn, id, "/bin/sh", at).await.unwrap().unwrap();
        id
    }

    fn closing(reason: &str) -> Closing<'_> {
        Closing {
            reason,
            exit_code: None,
            bytes_out: Some(5),
            recording: None,
        }
    }

    #[tokio::test]
    async fn of_two_that_close_a_shell_at_once_one_writes_its_end() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (person, environment) = environment(&mut conn).await;
        let now = orch_testkit::clock::start();
        let id = shell(&mut conn, person, environment, now).await;
        let first = close(&mut conn, id, &closing(reason::DETACHED), now)
            .await
            .unwrap();
        let second = close(&mut conn, id, &closing(reason::CLOSED_BY_USER), now)
            .await
            .unwrap();
        assert_eq!(
            first.and_then(|row| row.close_reason),
            Some(reason::DETACHED.to_owned())
        );
        assert!(second.is_none(), "the second finds it closed");
        let row = get(&mut conn, id).await.unwrap().unwrap();
        assert_eq!(row.close_reason.as_deref(), Some(reason::DETACHED));
        assert!(!row.attached && row.detached_at.is_none() && row.closed_at.is_some());
    }

    #[tokio::test]
    async fn a_shell_left_alone_keeps_the_moment_it_was_first_left() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (person, environment) = environment(&mut conn).await;
        let now = orch_testkit::clock::start();
        let id = shell(&mut conn, person, environment, now).await;
        attach(&mut conn, id, now).await.unwrap().unwrap();
        let first = now + chrono::Duration::minutes(5);
        assert!(detach(&mut conn, id, first).await.unwrap());
        assert!(
            !detach(&mut conn, id, first + chrono::Duration::minutes(5))
                .await
                .unwrap(),
            "a shell that nobody has is not left a second time"
        );
        let row = get(&mut conn, id).await.unwrap().unwrap();
        assert_eq!(row.detached_at, Some(first));
        // A word about the bytes that went through changes how many there were and nothing about who has it.
        touch(&mut conn, id, 3, 7, first).await.unwrap();
        let row = get(&mut conn, id).await.unwrap().unwrap();
        assert!(!row.attached && row.detached_at == Some(first));
        assert_eq!((row.bytes_in, row.bytes_out), (3, 7));
    }

    #[tokio::test]
    async fn the_shells_of_an_environment_that_ended_are_found_and_those_of_one_that_is_out_of_reach_are_not()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (person, environment) = environment(&mut conn).await;
        let now = orch_testkit::clock::start();
        let id = shell(&mut conn, person, environment, now).await;
        sqlx::query("UPDATE environments SET status = 'offline' WHERE id = $1")
            .bind(environment)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert!(
            of_ended_environments(&mut conn, 10)
                .await
                .unwrap()
                .is_empty()
        );
        sqlx::query("UPDATE environments SET status = 'stopped' WHERE id = $1")
            .bind(environment)
            .execute(&mut *conn)
            .await
            .unwrap();
        let found = of_ended_environments(&mut conn, 10).await.unwrap();
        assert_eq!(found.iter().map(|row| row.id).collect::<Vec<_>>(), [id]);
        sqlx::query(
            "UPDATE environments SET status = 'running', generation = generation + 1 WHERE id = $1",
        )
        .bind(environment)
        .execute(&mut *conn)
        .await
        .unwrap();
        assert_eq!(
            of_ended_environments(&mut conn, 10).await.unwrap().len(),
            1,
            "made again since the shell opened"
        );
    }

    #[tokio::test]
    async fn a_list_is_newest_first_and_a_page_begins_where_the_one_before_ended() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (person, environment) = environment(&mut conn).await;
        let start = orch_testkit::clock::start();
        let mut ids = Vec::new();
        for minute in 0..3 {
            let at = start + chrono::Duration::minutes(minute);
            ids.push(shell(&mut conn, person, environment, at).await);
        }
        let first = list(&mut conn, environment, Some(person), Which::Any, None, 2)
            .await
            .unwrap();
        assert_eq!(
            first.iter().map(|row| row.id).collect::<Vec<_>>(),
            [ids[2], ids[1]]
        );
        let last = first.last().unwrap();
        let second = list(
            &mut conn,
            environment,
            Some(person),
            Which::Any,
            Some(After {
                at: last.opened_at,
                id: last.id,
            }),
            2,
        )
        .await
        .unwrap();
        assert_eq!(
            second.iter().map(|row| row.id).collect::<Vec<_>>(),
            [ids[0]]
        );
        close(&mut conn, ids[1], &closing(reason::CLOSED_BY_USER), start)
            .await
            .unwrap();
        let ended = list(&mut conn, environment, None, Which::Ended, None, 10)
            .await
            .unwrap();
        assert_eq!(ended.iter().map(|row| row.id).collect::<Vec<_>>(), [ids[1]]);
        let live = live_in(&mut conn, environment).await.unwrap();
        assert_eq!(live.len(), 2);
    }
}
