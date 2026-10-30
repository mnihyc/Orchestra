//! Browser session reservations, their lifecycle and the environment facts that bound them.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

const COLUMNS: &str = "id, environment_id, env_generation, profile, origin, opened_by, thread_id, preset, display_id, endpoint, program, note, state, close_reason, opened_at, last_used_at, closed_at";

/// Whether a browser or its kept virtual display needs lifecycle maintenance after startup.
pub async fn needs_maintenance(conn: &mut Conn) -> Result<bool, DbError> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM browser_sessions WHERE state <> 'closed')
             OR EXISTS(SELECT 1 FROM display_sessions d
                       JOIN browser_sessions b ON b.display_id=d.id WHERE d.state <> 'closed')",
    )
    .fetch_one(conn)
    .await
    .map_err(DbError::from)
}

/// A known browser, including rows retained after it ended.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct BrowserRow {
    /// Its identifier.
    pub id: Id,
    /// Its environment.
    pub environment_id: Id,
    /// The unit generation in which it was opened.
    pub env_generation: i32,
    /// Its named profile, where it has one.
    pub profile: Option<String>,
    /// Started by Orch or announced by an agent.
    pub origin: String,
    /// Its human opener.
    pub opened_by: Option<Id>,
    /// The thread it was opened beside or by.
    pub thread_id: Option<Id>,
    /// Its launch preset, where known.
    pub preset: Option<String>,
    /// The display of a window.
    pub display_id: Option<Id>,
    /// Its local driving address.
    pub endpoint: String,
    /// Its own name and version.
    pub program: Option<String>,
    /// The opener's short line about its purpose, without page activity.
    pub note: Option<String>,
    /// Opening, open or closed.
    pub state: String,
    /// How it ended.
    pub close_reason: Option<String>,
    /// When it was reserved.
    pub opened_at: DateTime<Utc>,
    /// Its last observed activity.
    pub last_used_at: DateTime<Utc>,
    /// When it ended.
    pub closed_at: Option<DateTime<Utc>>,
}

/// The facts a browser operation needs about its environment.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Site {
    /// The environment.
    pub id: Id,
    /// Its first owner.
    pub owner_id: Id,
    /// Its root org.
    pub org_id: Id,
    /// Its home in the tree.
    pub home_node_id: Id,
    /// Its display name.
    pub name: String,
    /// Managed or connected.
    pub placement: String,
    /// Its workspace root.
    pub workspace_root: String,
    /// Its generation.
    pub generation: i32,
    /// Its current status.
    pub status: String,
    /// Its own browser switch.
    pub browser: bool,
    /// Its own display switch.
    pub vnc: bool,
    /// The machine behind it.
    pub machine_id: Option<Id>,
    /// Whether that machine has a link.
    pub machine_online: Option<bool>,
    /// Its locally enforced browser policy.
    pub machine_allows_browser: Option<Json>,
    /// Its locally enforced display policy.
    pub machine_allows_vnc: Option<Json>,
    /// The newly found programs.
    pub facts: Json,
}

/// The undeleted environment and the machine behind it.
pub async fn site(conn: &mut Conn, id: Id) -> Result<Option<Site>, DbError> {
    // A restored directory has no managed unit to start. Its verified attachment permits a
    // subsequent explicit browser/display operation while restoration itself starts none.
    Ok(sqlx::query_as("SELECT e.id,e.owner_id,e.org_id,e.home_node_id,e.name,e.placement,e.workspace_root,e.generation,CASE WHEN m.facts->'environment_descriptor'->>'lifecycle'='attach' AND e.status='stopped' AND e.attached_session IS NOT NULL AND e.attached_session IS NOT DISTINCT FROM COALESCE(m.session_started_at,m.last_seen_at) THEN 'running' ELSE e.status END AS status,COALESCE(e.browser,true) AS browser,COALESCE(e.vnc,true) AS vnc,e.machine_id,(m.online AND m.status='active' AND m.deleted_at IS NULL) AS machine_online,COALESCE(hm.facts,m.facts)->'policy'->'environment.browser' AS machine_allows_browser,COALESCE(hm.facts,m.facts)->'policy'->'environment.vnc' AS machine_allows_vnc,e.facts FROM environments e LEFT JOIN machines m ON m.id=e.machine_id LEFT JOIN LATERAL (SELECT r.host_id FROM runtimes r WHERE r.environment_id=e.id AND r.state NOT IN ('retired','removed') AND r.successor_of IS NULL LIMIT 1) policy_runtime ON true LEFT JOIN hosts policy_host ON policy_host.id=policy_runtime.host_id LEFT JOIN machines hm ON hm.id=policy_host.machine_id WHERE e.id=$1 AND e.deleted_at IS NULL").bind(id).fetch_optional(conn).await?)
}

/// Orders profile handouts and limit checks within one environment until the transaction ends.
pub async fn lock(conn: &mut Conn, environment: Id) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('browsers:' || $1::text,0))")
        .bind(environment)
        .execute(conn)
        .await?;
    Ok(())
}

/// A reserved browser row, before its local endpoint is known.
pub struct NewBrowser<'a> {
    /// Its identifier.
    pub id: Id,
    /// The environment.
    pub environment: Id,
    /// Its unit generation.
    pub generation: i32,
    /// Its profile, where known.
    pub profile: Option<&'a str>,
    /// Started or announced.
    pub origin: &'a str,
    /// Its person, where opened by one.
    pub person: Option<Id>,
    /// Its thread, where opened by or beside one.
    pub thread: Option<Id>,
    /// Its preset, where Orch starts it.
    pub preset: Option<&'a str>,
    /// Its window's display.
    pub display: Option<Id>,
    /// The agent's line about what it is for.
    pub note: Option<&'a str>,
}

/// Reserves the profile and a place against the environment's limit.
pub async fn insert(
    conn: &mut Conn,
    new: &NewBrowser<'_>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO browser_sessions(id,environment_id,env_generation,profile,origin,opened_by,thread_id,preset,display_id,note,endpoint,opened_at,last_used_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'',$11,$11)")
        .bind(new.id).bind(new.environment).bind(new.generation).bind(new.profile).bind(new.origin).bind(new.person).bind(new.thread).bind(new.preset).bind(new.display).bind(new.note).bind(at).execute(conn).await?;
    Ok(())
}

/// One browser row.
pub async fn get(conn: &mut Conn, id: Id) -> Result<Option<BrowserRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM browser_sessions WHERE id=$1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Open browsers first, then the browsers closed since the supplied cutoff, by latest use.
pub async fn list(
    conn: &mut Conn,
    environment: Id,
    since: DateTime<Utc>,
) -> Result<Vec<BrowserRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLUMNS} FROM browser_sessions WHERE environment_id=$1 AND (state<>'closed' OR closed_at >= $2) ORDER BY (state='closed'), last_used_at DESC,id DESC"))).bind(environment).bind(since).fetch_all(conn).await?)
}

/// Every browser an environment ever had, open or closed.
pub async fn ids_of_environment(conn: &mut Conn, environment: Id) -> Result<Vec<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT id FROM browser_sessions WHERE environment_id = $1")
            .bind(environment)
            .fetch_all(conn)
            .await?,
    )
}

/// A profile's existing live reservation, regardless of who opened it.
pub async fn on_profile(
    conn: &mut Conn,
    environment: Id,
    profile: &str,
) -> Result<Option<BrowserRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLUMNS} FROM browser_sessions WHERE environment_id=$1 AND lower(profile)=lower($2) AND state<>'closed'"))).bind(environment).bind(profile).fetch_optional(conn).await?)
}

/// Counts only browsers Orch started, including reservations that are still opening.
pub async fn owned_count(conn: &mut Conn, environment: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM browser_sessions WHERE environment_id=$1 AND origin='started' AND state<>'closed'").bind(environment).fetch_one(conn).await?)
}

/// Completes a reservation only if it was not closed meanwhile.
pub async fn opened(
    conn: &mut Conn,
    id: Id,
    endpoint: &str,
    program: &str,
    profile: Option<&str>,
    at: DateTime<Utc>,
) -> Result<Option<BrowserRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("UPDATE browser_sessions SET endpoint=$2,program=$3,profile=COALESCE($4,profile),state='open',last_used_at=$5 WHERE id=$1 AND state='opening' RETURNING {COLUMNS}"))).bind(id).bind(endpoint).bind(program).bind(profile).bind(at).fetch_optional(conn).await?)
}

/// Forgets a reservation that never became a browser.
pub async fn drop_opening(conn: &mut Conn, id: Id) -> Result<(), DbError> {
    sqlx::query("DELETE FROM browser_sessions WHERE id=$1 AND state='opening'")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Closes a row once; only the caller receiving it writes its history line.
pub async fn close(
    conn: &mut Conn,
    id: Id,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<Option<BrowserRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("UPDATE browser_sessions SET state='closed',close_reason=$2,closed_at=$3 WHERE id=$1 AND state<>'closed' RETURNING {COLUMNS}"))).bind(id).bind(reason).bind(at).fetch_optional(conn).await?)
}

/// Marks use without storing anything that was done in a page.
pub async fn touch(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query("UPDATE browser_sessions SET last_used_at=GREATEST(last_used_at,$2) WHERE id=$1 AND state<>'closed'").bind(id).bind(at).execute(conn).await?;
    Ok(())
}

/// All live browsers for the keeper and connector reconciliation.
pub async fn live(conn: &mut Conn) -> Result<Vec<BrowserRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM browser_sessions WHERE state<>'closed' ORDER BY opened_at,id"
    )))
    .fetch_all(conn)
    .await?)
}

/// Stores fresh support facts without touching any page data.
pub async fn facts(
    conn: &mut Conn,
    environment: Id,
    browser: &Json,
    vnc: &Json,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE environments SET facts=jsonb_set(jsonb_set(jsonb_set(COALESCE(facts,'{}'::jsonb),'{browser}',$2),'{vnc}',$3),'{looked_at}',to_jsonb($4::timestamptz)) WHERE id=$1").bind(environment).bind(browser).bind(vnc).bind(at).execute(conn).await?;
    Ok(())
}

/// Names a live node for a canonical publication that will be cut down at delivery.
pub async fn node_name(conn: &mut Conn, id: Id) -> Result<Option<String>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT name FROM nodes WHERE id=$1 AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// Whether the browser session that gave a ticket still exists and is live.
pub async fn session_live(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=$1 AND revoked_at IS NULL AND expires_at>$2)").bind(id).bind(at).fetch_one(conn).await?)
}

/// Whether the API token that gave a ticket still exists and is live.
pub async fn token_live(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_tokens WHERE id=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>$2))").bind(id).bind(at).fetch_one(conn).await?)
}

/// Reuses an idle display or reserves the next browser window under the environment's lock.
pub async fn reserve_window(
    conn: &mut Conn,
    environment: Id,
    generation: i32,
    profile: &str,
    person: Option<Id>,
    thread: Option<Id>,
    at: DateTime<Utc>,
) -> Result<(Id, bool), DbError> {
    if let Some(id)=sqlx::query_scalar::<_,Id>("SELECT d.id FROM display_sessions d WHERE d.environment_id=$1 AND d.env_generation=$2 AND d.origin='started' AND d.state='open' AND NOT EXISTS(SELECT 1 FROM browser_sessions b WHERE b.display_id=d.id AND b.state<>'closed') ORDER BY d.last_used_at LIMIT 1").bind(environment).bind(generation).fetch_optional(&mut *conn).await? {return Ok((id,false));}
    let id = orch_types::new_id();
    sqlx::query("INSERT INTO display_sessions(id,environment_id,env_generation,name,origin,opened_by,thread_id,endpoint,opened_at,last_used_at) VALUES($1,$2,$3,$4,'started',$5,$6,'',$7,$7)").bind(id).bind(environment).bind(generation).bind(profile).bind(person).bind(thread).bind(at).execute(conn).await?;
    Ok((id, true))
}

/// Connects a browser reservation to its display.
pub async fn set_window(conn: &mut Conn, browser: Id, display: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE browser_sessions SET display_id=$2 WHERE id=$1 AND state='opening'")
        .bind(browser)
        .bind(display)
        .execute(conn)
        .await?;
    Ok(())
}

/// Marks the virtual screen ready with only its local service address.
pub async fn window_ready(
    conn: &mut Conn,
    display: Id,
    endpoint: &str,
    x_display: &str,
    geometry: &str,
) -> Result<(), DbError> {
    sqlx::query("UPDATE display_sessions SET endpoint=$2,x_display=$3,geometry=$4,state='open' WHERE id=$1 AND state<>'closed'").bind(display).bind(endpoint).bind(x_display).bind(geometry).execute(conn).await?;
    Ok(())
}

/// Closes browser-created displays when their environment or unit generation has ended.
pub async fn close_ended_windows(conn: &mut Conn, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query("UPDATE display_sessions d SET state='closed',close_reason='environment_stopped',closed_at=$1 WHERE d.state<>'closed' AND d.origin='started' AND EXISTS(SELECT 1 FROM browser_sessions b WHERE b.display_id=d.id) AND NOT EXISTS(SELECT 1 FROM environments e WHERE e.id=d.environment_id AND e.deleted_at IS NULL AND e.generation=d.env_generation AND e.status IN ('running','offline'))")
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}
