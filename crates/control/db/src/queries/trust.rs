//! What the people who manage an environment answered about a configuration of a harness's own that its workspace
//! holds (a `.codex` or `.claude` with something in it): the environment's `config_trust`.
//!
//! One member for each harness that found one, by its kind: `answer` (`trust`, `keep_out`, or null while the
//! question waits), who answered and when (`by`, `at`), `files` (the hash of what was found, which the answer
//! holds for: when the files change the question is asked again), `path` and `found` (the places, relative to
//! the workspace root). No member: nobody was asked. Every thread that meets the same configuration waits for the
//! same question.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};
use serde_json::json;

/// The words an answer is written in.
pub mod answer {
    /// The harness uses the configuration, as it does in a folder it trusts.
    pub const TRUST: &str = "trust";
    /// The harness works without it, as it does in a folder it does not trust.
    pub const KEEP_OUT: &str = "keep_out";
}

/// Why a thread waits while the question is open, as its blocked reason.
pub const WAITING_REASON: &str = "workspace_trust";

/// One harness's member of an environment's `config_trust`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trust {
    /// The harness, by its kind.
    pub harness: String,
    /// [`answer::TRUST`] or [`answer::KEEP_OUT`]; none while the question waits.
    pub answer: Option<String>,
    /// Who answered.
    pub by: Option<Id>,
    /// When.
    pub at: Option<DateTime<Utc>>,
    /// The hash of what was found, which the answer holds for.
    pub files: String,
    /// The places found, relative to the workspace root.
    pub found: Vec<String>,
}

impl Trust {
    /// The member as it is kept.
    fn of(harness: &str, value: &Json) -> Self {
        let text = |key: &str| value.get(key).and_then(Json::as_str).map(str::to_owned);
        Self {
            harness: harness.to_owned(),
            answer: text("answer"),
            by: text("by").and_then(|by| by.parse().ok()),
            at: text("at")
                .and_then(|at| DateTime::parse_from_rfc3339(&at).ok())
                .map(|at| at.with_timezone(&Utc)),
            files: text("files").unwrap_or_default(),
            found: value
                .get("found")
                .and_then(Json::as_array)
                .map(|found| {
                    found
                        .iter()
                        .filter_map(Json::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// The first place found, as the environment's page names it.
    #[must_use]
    pub fn path(&self) -> String {
        self.found.first().cloned().unwrap_or_default()
    }
}

/// Every member of an environment's `config_trust`, by harness.
pub async fn of(conn: &mut Conn, environment: Id) -> Result<Vec<Trust>, DbError> {
    let kept: Option<Json> =
        sqlx::query_scalar("SELECT config_trust FROM environments WHERE id = $1")
            .bind(environment)
            .fetch_optional(conn)
            .await?;
    Ok(members(kept.as_ref()))
}

/// The members of several environments' `config_trust`.
pub async fn of_each(
    conn: &mut Conn,
    environments: &[Id],
) -> Result<Vec<(Id, Vec<Trust>)>, DbError> {
    let kept: Vec<(Id, Json)> =
        sqlx::query_as("SELECT id, config_trust FROM environments WHERE id = ANY($1)")
            .bind(environments)
            .fetch_all(conn)
            .await?;
    Ok(kept
        .into_iter()
        .map(|(id, value)| (id, members(Some(&value))))
        .collect())
}

fn members(kept: Option<&Json>) -> Vec<Trust> {
    let mut members: Vec<Trust> = kept
        .and_then(Json::as_object)
        .map(|map| {
            map.iter()
                .map(|(harness, value)| Trust::of(harness, value))
                .collect()
        })
        .unwrap_or_default();
    members.sort_by(|a, b| a.harness.cmp(&b.harness));
    members
}

/// One harness's member, if the environment has one.
pub async fn of_harness(
    conn: &mut Conn,
    environment: Id,
    harness: &str,
) -> Result<Option<Trust>, DbError> {
    Ok(of(conn, environment)
        .await?
        .into_iter()
        .find(|member| member.harness == harness))
}

/// Asks the question for a harness: its member waits (no answer) for what was found, unless it waits already for
/// the same files. The answer says whether this asked it. An answer for other files is replaced: the files
/// changed, and the question is asked again.
pub async fn ask(
    conn: &mut Conn,
    environment: Id,
    harness: &str,
    files: &str,
    found: &[String],
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let member =
        json!({ "answer": null, "files": files, "found": found, "asked_at": at.to_rfc3339() });
    Ok(sqlx::query(
        "UPDATE environments SET config_trust = jsonb_set(config_trust, ARRAY[$2::text], $3)
          WHERE id = $1
            AND NOT (config_trust ? $2 AND config_trust -> $2 ->> 'files' = $4)",
    )
    .bind(environment)
    .bind(harness)
    .bind(member)
    .bind(files)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// Writes the answer for a harness that has a member, in place of any before it, for the files that member holds.
/// The answer is the member as it was, or none where the harness has none (nobody was asked).
pub async fn answer(
    conn: &mut Conn,
    environment: Id,
    harness: &str,
    answer: &str,
    by: Id,
    at: DateTime<Utc>,
) -> Result<Option<Trust>, DbError> {
    let Some(before) = of_harness(&mut *conn, environment, harness).await? else {
        return Ok(None);
    };
    let member = json!({
        "answer": answer,
        "by": by.to_string(),
        "at": at.to_rfc3339(),
        "files": before.files,
        "found": before.found,
    });
    sqlx::query(
        "UPDATE environments SET config_trust = jsonb_set(config_trust, ARRAY[$2::text], $3) WHERE id = $1",
    )
    .bind(environment)
    .bind(harness)
    .bind(member)
    .execute(conn)
    .await?;
    Ok(Some(before))
}

/// The threads of an environment that wait for its answer about a workspace configuration.
pub async fn threads_waiting(conn: &mut Conn, environment: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT node_id FROM threads
          WHERE environment_id = $1 AND state = 'blocked' AND blocked_reason = $2
          ORDER BY node_id",
    )
    .bind(environment)
    .bind(WAITING_REASON)
    .fetch_all(conn)
    .await?)
}
