//! What the engine reads to tell an event, and the one row it writes beside the Inbox: the first line of what an agent last said, the job that ended, and the delivery to a node's destination, written once.
//!
//! A notification is a title, a first line and a link, never a whole message,
//! a file or an output. The first line of what an agent last said is cut here
//! so that no caller carries more than that into a row.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// Safe provider metadata identifying one exhausted-state episode.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ExhaustedProvider {
    /// The provider.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// The first owner, used to load the provider's permission graph.
    pub owner_id: Id,
    /// Its root org.
    pub org_id: Id,
    /// Its current home.
    pub home_id: Id,
    /// When the current exhausted state was established.
    pub status_at: DateTime<Utc>,
}

/// The current exhausted provider, with its stable status time rather than changing fact revisions.
pub async fn exhausted_provider(
    conn: &mut Conn,
    provider: Id,
) -> Result<Option<ExhaustedProvider>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, name, owner_id, org_id, home_id, status_at FROM providers
        WHERE id = $1 AND status = 'exhausted' AND deleted_at IS NULL",
    )
    .bind(provider)
    .fetch_optional(conn)
    .await?)
}

/// The most characters of the first line a notification shows of what an
/// agent said: long enough to read as a sentence in a list, short enough for
/// a message on a phone.
pub const FIRST_LINE_MAX_CHARS: usize = 200;

/// The first line of the text, cut at [`FIRST_LINE_MAX_CHARS`] characters and
/// marked with an ellipsis where it was cut. Empty for a text with no words.
#[must_use]
pub fn first_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty());
    let Some(line) = line else {
        return String::new();
    };
    if line.chars().count() <= FIRST_LINE_MAX_CHARS {
        return line.to_owned();
    }
    let mut cut: String = line.chars().take(FIRST_LINE_MAX_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// The first line of the last thing the agent said in a turn; none where it
/// said nothing in words.
pub async fn last_agent_line(conn: &mut Conn, turn: Id) -> Result<Option<String>, DbError> {
    let text: Option<Option<String>> = sqlx::query_scalar(
        "SELECT payload ->> 'text' FROM items
          WHERE turn_id = $1 AND kind = 'agent_message' AND stream = 'main'
          ORDER BY seq_created DESC LIMIT 1",
    )
    .bind(turn)
    .fetch_optional(conn)
    .await?;
    Ok(text
        .flatten()
        .map(|text| first_line(&text))
        .filter(|line| !line.is_empty()))
}

/// The person who authored the input this turn answers, without loading its message or files.
/// An automatic or agent-authored input is nobody's own turn for notification suppression.
pub async fn input_author(conn: &mut Conn, input: Id) -> Result<Option<Id>, DbError> {
    let author: Option<Option<Id>> =
        sqlx::query_scalar("SELECT author_id FROM thread_inputs WHERE id = $1")
            .bind(input)
            .fetch_optional(conn)
            .await?;
    Ok(author.flatten())
}

/// A long operation as the notification of its end needs it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct JobRow {
    /// Its identifier.
    pub id: Id,
    /// What it does: `thread_fork`, `environment_recreate` and so on.
    pub kind: String,
    /// The thread it is about.
    pub node_id: Option<Id>,
    /// How far it has come.
    pub state: String,
    /// Why it failed, where it did.
    pub error: Option<Json>,
    /// Who asked for it.
    pub created_by: Option<Id>,
    /// The name of what it is about: the thread, or else the environment or
    /// the provider.
    pub subject: Option<String>,
}

/// The job `id`.
pub async fn job(conn: &mut Conn, id: Id) -> Result<Option<JobRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT j.id, j.kind, j.node_id, j.state, j.error, j.created_by,
                COALESCE(n.name, e.name, p.name) AS subject
           FROM jobs j
           LEFT JOIN nodes n ON n.id = j.node_id
           LEFT JOIN environments e ON e.id = j.environment_id
           LEFT JOIN providers p ON p.id = j.provider_id
          WHERE j.id = $1",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// A machine as the notification that it went away needs it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct MachineRow {
    /// What it is called.
    pub name: String,
    /// The person it belongs to; none for a machine the system runs.
    pub owner_id: Option<Id>,
}

/// The machine `id`.
pub async fn machine(conn: &mut Conn, id: Id) -> Result<Option<MachineRow>, DbError> {
    Ok(
        sqlx::query_as("SELECT name, owner_id FROM machines WHERE id = $1 AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// Writes the delivery of an event to a node's destination, unless the row
/// `id` is there already. The identifier is made from the destination and the
/// event, so that an event that is told twice is delivered once. Says whether
/// the row was written.
pub async fn tell_destination_once(
    conn: &mut Conn,
    id: Id,
    destination: Id,
    channel: &str,
    payload: &Json,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "INSERT INTO outbox (id, channel, route_id, destination_id, channel_id, payload, state, attempts, next_attempt_at, created_at)
         VALUES ($1, $2, NULL, $3, NULL, $4, 'pending', 0, $5, $5)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(id)
    .bind(channel)
    .bind(destination)
    .bind(payload)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_an_agent_said_is_shown_as_its_first_line_and_no_more() {
        assert_eq!(
            first_line("\n  \nFixed the login.\nSecond line"),
            "Fixed the login."
        );
        assert_eq!(first_line("   \n\t"), "");
        let long = "x".repeat(FIRST_LINE_MAX_CHARS + 50);
        let shown = first_line(&long);
        assert_eq!(shown.chars().count(), FIRST_LINE_MAX_CHARS);
        assert!(shown.ends_with('…'));
        let exact = "y".repeat(FIRST_LINE_MAX_CHARS);
        assert_eq!(first_line(&exact), exact);
    }
}
