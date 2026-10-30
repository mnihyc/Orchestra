//! Display reservations and lifecycle keep credentials separate from public rows.

pub use super::browsers::{Site, lock, session_live, site, token_live};
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

const COLUMNS: &str = "id,environment_id,env_generation,name,origin,opened_by,thread_id,endpoint,x_display,geometry,note,password,state,close_reason,opened_at,last_used_at,closed_at";

/// Reads removed environment metadata only for a known display's factual lifecycle audit.
pub async fn history_site(conn: &mut Conn, id: Id) -> Result<Option<Site>, DbError> {
    Ok(sqlx::query_as("SELECT e.id,e.owner_id,e.org_id,e.home_node_id,e.name,e.placement,e.workspace_root,e.generation,e.status,COALESCE(e.browser,true) AS browser,COALESCE(e.vnc,true) AS vnc,e.machine_id,m.online AS machine_online,COALESCE(hm.facts,m.facts)->'policy'->'environment.browser' AS machine_allows_browser,COALESCE(hm.facts,m.facts)->'policy'->'environment.vnc' AS machine_allows_vnc,e.facts FROM environments e LEFT JOIN machines m ON m.id=e.machine_id LEFT JOIN LATERAL (SELECT r.host_id FROM runtimes r WHERE r.environment_id=e.id AND r.state NOT IN ('retired','removed') AND r.successor_of IS NULL LIMIT 1) policy_runtime ON true LEFT JOIN hosts policy_host ON policy_host.id=policy_runtime.host_id LEFT JOIN machines hm ON hm.id=policy_host.machine_id WHERE e.id=$1").bind(id).fetch_optional(conn).await?)
}

/// A durable display registration; its password is only ciphertext.
#[derive(Clone, sqlx::FromRow)]
pub struct DisplayRow {
    /// Its identifier.
    pub id: Id,
    /// Its environment.
    pub environment_id: Id,
    /// The generation it belongs to.
    pub env_generation: i32,
    /// Its listed name.
    pub name: String,
    /// Started or announced.
    pub origin: String,
    /// Its human opener.
    pub opened_by: Option<Id>,
    /// Its associated thread.
    pub thread_id: Option<Id>,
    /// The local target.
    pub endpoint: String,
    /// Its X address where owned.
    pub x_display: Option<String>,
    /// Its dimensions.
    pub geometry: Option<String>,
    /// Its purpose line.
    pub note: Option<String>,
    /// The sealed server credential; never serialized or logged.
    pub password: Option<Vec<u8>>,
    /// Opening, open or closed.
    pub state: String,
    /// How it ended.
    pub close_reason: Option<String>,
    /// When it was reserved.
    pub opened_at: DateTime<Utc>,
    /// Its last use.
    pub last_used_at: DateTime<Utc>,
    /// When it ended.
    pub closed_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for DisplayRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisplayRow")
            .field("id", &self.id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// One reservation, with an already sealed password where announced.
pub struct NewDisplay<'a> {
    /// Its identifier.
    pub id: Id,
    /// Its environment.
    pub environment: Id,
    /// Its generation.
    pub generation: i32,
    /// Its name, empty until the X address is known where omitted.
    pub name: &'a str,
    /// Its origin.
    pub origin: &'a str,
    /// Its human opener.
    pub person: Option<Id>,
    /// Its associated thread.
    pub thread: Option<Id>,
    /// Its purpose line.
    pub note: Option<&'a str>,
    /// Its sealed password.
    pub password: Option<&'a [u8]>,
}

/// Reserves one screen against the environment's limit.
pub async fn insert(
    conn: &mut Conn,
    new: &NewDisplay<'_>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO display_sessions(id,environment_id,env_generation,name,origin,opened_by,thread_id,note,password,endpoint,opened_at,last_used_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'',$10,$10)")
        .bind(new.id).bind(new.environment).bind(new.generation).bind(new.name).bind(new.origin).bind(new.person).bind(new.thread).bind(new.note).bind(new.password).bind(at).execute(conn).await?;
    Ok(())
}

/// Reads one registration.
pub async fn get(conn: &mut Conn, id: Id) -> Result<Option<DisplayRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM display_sessions WHERE id=$1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Lists live screens first, then recently closed screens.
pub async fn list(
    conn: &mut Conn,
    environment: Id,
    since: DateTime<Utc>,
) -> Result<Vec<DisplayRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLUMNS} FROM display_sessions WHERE environment_id=$1 AND (state<>'closed' OR closed_at>=$2) ORDER BY (state='closed'),last_used_at DESC,id DESC"))).bind(environment).bind(since).fetch_all(conn).await?)
}

/// Counts owned reservations as well as open screens.
pub async fn owned_count(conn: &mut Conn, environment: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM display_sessions WHERE environment_id=$1 AND origin='started' AND state<>'closed'").bind(environment).fetch_one(conn).await?)
}

/// Whether a browser can reuse an owned screen without consuming another display slot.
pub async fn has_idle_window(
    conn: &mut Conn,
    environment: Id,
    generation: i32,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM display_sessions d WHERE d.environment_id=$1 AND d.env_generation=$2 AND d.origin='started' AND d.state='open' AND NOT EXISTS(SELECT 1 FROM browser_sessions b WHERE b.display_id=d.id AND b.state<>'closed'))").bind(environment).bind(generation).fetch_one(conn).await?)
}

/// Publishes a reservation once its server has answered.
pub async fn opened(
    conn: &mut Conn,
    id: Id,
    endpoint: &str,
    x_display: Option<&str>,
    name: Option<&str>,
    geometry: Option<&str>,
    at: DateTime<Utc>,
) -> Result<Option<DisplayRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("UPDATE display_sessions SET endpoint=$2,x_display=$3,name=CASE WHEN name='' THEN COALESCE($4,$3,'Display') ELSE name END,geometry=$5,state='open',last_used_at=$6 WHERE id=$1 AND state='opening' RETURNING {COLUMNS}"))).bind(id).bind(endpoint).bind(x_display).bind(name).bind(geometry).bind(at).fetch_optional(conn).await?)
}

/// Drops a reservation that never opened, including its sealed credential.
pub async fn drop_opening(conn: &mut Conn, id: Id) -> Result<(), DbError> {
    sqlx::query("DELETE FROM display_sessions WHERE id=$1 AND state='opening'")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Closes once, discarding credentials while retaining the safe history target.
pub async fn close(
    conn: &mut Conn,
    id: Id,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<Option<DisplayRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("UPDATE display_sessions SET state='closed',password=NULL,close_reason=$2,closed_at=$3 WHERE id=$1 AND state<>'closed' RETURNING {COLUMNS}"))).bind(id).bind(reason).bind(at).fetch_optional(conn).await?)
}

/// Marks activity without retaining what a viewer did.
pub async fn touch(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query("UPDATE display_sessions SET last_used_at=GREATEST(last_used_at,$2) WHERE id=$1 AND state='open'").bind(id).bind(at).execute(conn).await?;
    Ok(())
}

/// All live registrations for startup and lifecycle reconciliation.
pub async fn live(conn: &mut Conn) -> Result<Vec<DisplayRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM display_sessions WHERE state<>'closed' ORDER BY opened_at,id"
    )))
    .fetch_all(conn)
    .await?)
}

/// The browser windows attached to this display.
pub async fn browsers(conn: &mut Conn, display: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT id FROM browser_sessions WHERE display_id=$1 AND state<>'closed' ORDER BY opened_at,id").bind(display).fetch_all(conn).await?)
}
