//! What the pages of channels, routes and destinations read and change: the ways out that reach a person, their routes through them, the destinations of a node, and the browsers that take push.
//!
//! A way out reaches a person when it is the system's, when it belongs to a
//! root org they are a member of, or when it is their own. The pages show a
//! channel together with the person's own route through it; a route is made
//! when the person first changes it, so most channels have none for most
//! people, and the page shows the route every route starts as: off.

use super::channels::{ChannelRow, DestinationRow};
use super::routes::RouteRow;
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

macro_rules! channel_columns {
    () => {
        "c.id, c.org_id, c.user_id, c.kind, c.name, c.config, c.templates, c.enabled, c.link_cursor, c.last_ok_at, c.last_error, c.created_by, c.created_at, c.updated_at"
    };
}

macro_rules! destination_columns {
    () => {
        "d.id, d.node_id, d.channel, d.name, d.config, d.templates, d.kinds, d.level, d.enabled, d.last_ok_at, d.last_error, d.created_by, d.created_at, d.updated_at"
    };
}

/// The root orgs a person is a member of.
pub async fn root_orgs_of(conn: &mut Conn, user: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT org_id FROM org_members WHERE user_id = $1 ORDER BY joined_at, org_id",
    )
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// The ways out that reach a person: the system's, those of the root orgs they
/// are a member of, and their own; the system's first, then each org's in the
/// order they were set up, then their own.
pub async fn channels_reaching(conn: &mut Conn, user: Id) -> Result<Vec<ChannelRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        channel_columns!(),
        " FROM notification_channels c ",
        "WHERE (c.org_id IS NULL AND c.user_id IS NULL) ",
        "OR c.user_id = $1 ",
        "OR c.org_id IN (SELECT m.org_id FROM org_members m WHERE m.user_id = $1) ",
        "ORDER BY CASE WHEN c.user_id IS NOT NULL THEN 2 WHEN c.org_id IS NOT NULL THEN 1 ELSE 0 END, c.created_at, c.id"
    ))
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// The channel `id` if it reaches the person: it is the system's, or it is
/// their own, or it belongs to a root org they are a member of.
pub async fn channel_reaching(
    conn: &mut Conn,
    user: Id,
    id: Id,
) -> Result<Option<ChannelRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        channel_columns!(),
        " FROM notification_channels c WHERE c.id = $2 AND ((c.org_id IS NULL AND c.user_id IS NULL) ",
        "OR c.user_id = $1 OR c.org_id IN (SELECT m.org_id FROM org_members m WHERE m.user_id = $1))"
    ))
    .bind(user)
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The ways out of the system, in the order they were set up.
pub async fn system_channels(conn: &mut Conn) -> Result<Vec<ChannelRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        channel_columns!(),
        " FROM notification_channels c WHERE c.org_id IS NULL AND c.user_id IS NULL ORDER BY c.created_at, c.id"
    ))
    .fetch_all(conn)
    .await?)
}

/// The ways out of one root org, in the order they were set up.
pub async fn channels_of_org(conn: &mut Conn, org: Id) -> Result<Vec<ChannelRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        channel_columns!(),
        " FROM notification_channels c WHERE c.org_id = $1 ORDER BY c.created_at, c.id"
    ))
    .bind(org)
    .fetch_all(conn)
    .await?)
}

/// Forgets why the last delivery through a channel failed: its fields were
/// looked at again and found right.
pub async fn clear_channel_error(conn: &mut Conn, id: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE notification_channels SET last_error = NULL WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Forgets why the last delivery to a destination failed: its fields were
/// looked at again and found right.
pub async fn clear_destination_error(conn: &mut Conn, id: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE destinations SET last_error = NULL WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Every route a person has, one for each channel they have changed theirs for, and those kept
/// after their channel was removed.
pub async fn routes_of(conn: &mut Conn, user: Id) -> Result<Vec<RouteRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM notification_routes WHERE user_id = $1 ORDER BY created_at, id",
        super::routes::ROUTE_COLUMNS
    )))
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// The root org a node lies in; none for a node that is gone.
pub async fn org_of_node(conn: &mut Conn, node: Id) -> Result<Option<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM nodes WHERE id = $1 AND deleted_at IS NULL")
            .bind(node)
            .fetch_optional(conn)
            .await?,
    )
}

/// A node with its kind and the nodes from its root org down to it, itself last.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct NodePath {
    /// The node.
    pub id: Id,
    /// `org`, `project`, `topic` or `thread`.
    pub kind: String,
    /// From its root org down to it.
    pub path: Vec<Id>,
}

/// The paths of these nodes, with the kind of every node on them.
pub async fn node_paths(
    conn: &mut Conn,
    ids: &[Id],
) -> Result<(Vec<NodePath>, std::collections::HashMap<Id, String>), DbError> {
    let paths: Vec<NodePath> = sqlx::query_as(
        "SELECT id, kind, path FROM nodes WHERE id = ANY($1) AND deleted_at IS NULL",
    )
    .bind(ids)
    .fetch_all(&mut *conn)
    .await?;
    let on_them: Vec<Id> = paths.iter().flat_map(|each| each.path.clone()).collect();
    let kinds: Vec<(Id, String)> = sqlx::query_as("SELECT id, kind FROM nodes WHERE id = ANY($1)")
        .bind(&on_them)
        .fetch_all(conn)
        .await?;
    Ok((paths, kinds.into_iter().collect()))
}

/// Those of these approvals that still wait for a decision, with the thread each is about.
pub async fn pending_approvals(
    conn: &mut Conn,
    ids: &[Id],
) -> Result<Vec<(Id, Option<Id>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, thread_id FROM approvals WHERE id = ANY($1) AND state = 'pending'",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The most characters of a browser's name as a list or a notice shows it.
const DEVICE_LABEL_MAX_CHARS: usize = 60;

/// A short name for a browser from what it said of itself: the browser and the system it runs
/// on, where they are told, else the start of what it said. Never the endpoint.
#[must_use]
pub fn device_label(user_agent: &str) -> String {
    let browser = ["Edg/", "OPR/", "Firefox/", "Chrome/", "Safari/"]
        .iter()
        .find(|marker| user_agent.contains(**marker))
        .map(|marker| match *marker {
            "Edg/" => "Edge",
            "OPR/" => "Opera",
            "Firefox/" => "Firefox",
            "Chrome/" => "Chrome",
            _ => "Safari",
        });
    let system = [
        ("Android", "Android"),
        ("iPhone", "iOS"),
        ("iPad", "iPadOS"),
        ("Windows", "Windows"),
        ("Mac OS X", "macOS"),
        ("Linux", "Linux"),
    ]
    .iter()
    .find(|(marker, _)| user_agent.contains(*marker))
    .map(|(_, name)| *name);
    match (browser, system) {
        (Some(browser), Some(system)) => format!("{browser} on {system}"),
        (Some(browser), None) => browser.to_owned(),
        (None, Some(system)) => system.to_owned(),
        (None, None) => user_agent.chars().take(DEVICE_LABEL_MAX_CHARS).collect(),
    }
}

/// A person's own browser that takes push, as their list shows it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct PushDeviceRow {
    /// Its identifier.
    pub id: Id,
    /// The browser it is, as it named itself.
    pub user_agent: String,
    /// The SHA-256 of its endpoint, lower-case hex: the endpoint itself is never shown.
    pub endpoint_hash: String,
    /// When a message last reached it.
    pub last_ok_at: Option<DateTime<Utc>>,
}

/// The browsers of a person that take push, the oldest first.
pub async fn push_devices_of(conn: &mut Conn, user: Id) -> Result<Vec<PushDeviceRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, user_agent, encode(sha256(convert_to(endpoint, 'UTF8')), 'hex') AS endpoint_hash, last_ok_at
           FROM push_subscriptions WHERE user_id = $1 ORDER BY created_at, id",
    )
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// Takes one of a person's own browsers out of push by its identifier. A browser of another
/// person is not touched. Says whether it was theirs and there.
pub async fn remove_push_device_of(conn: &mut Conn, user: Id, device: Id) -> Result<bool, DbError> {
    let done = sqlx::query("DELETE FROM push_subscriptions WHERE id = $1 AND user_id = $2")
        .bind(device)
        .bind(user)
        .execute(conn)
        .await?;
    Ok(done.rows_affected() == 1)
}

/// How many people have a route through each of these channels switched on: a number for whoever
/// looks after a channel, never who they are.
pub async fn enabled_route_counts(
    conn: &mut Conn,
    channels: &[Id],
) -> Result<std::collections::HashMap<Id, i64>, DbError> {
    let counted: Vec<(Id, i64)> = sqlx::query_as(
        "SELECT channel_id, count(*) FROM notification_routes
          WHERE channel_id = ANY($1) AND enabled GROUP BY channel_id",
    )
    .bind(channels)
    .fetch_all(conn)
    .await?;
    Ok(counted.into_iter().collect())
}

/// The people who have a route through a channel.
pub async fn people_routed_through(conn: &mut Conn, channel: Id) -> Result<Vec<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT user_id FROM notification_routes WHERE channel_id = $1")
            .bind(channel)
            .fetch_all(conn)
            .await?,
    )
}

/// Whether a person has a route through a channel, switched on or not.
pub async fn is_routed(conn: &mut Conn, user: Id, channel: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM notification_routes WHERE user_id = $1 AND channel_id = $2)",
    )
    .bind(user)
    .bind(channel)
    .fetch_one(conn)
    .await?)
}

/// The destinations of a node, in the order they were set up. Those that are
/// switched off are among them.
pub async fn destinations_of(conn: &mut Conn, node: Id) -> Result<Vec<DestinationRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        destination_columns!(),
        " FROM destinations d WHERE d.node_id = $1 ORDER BY d.created_at, d.id"
    ))
    .bind(node)
    .fetch_all(conn)
    .await?)
}

/// What is changed of a destination; what is `None` stays.
#[derive(Debug, Clone, Copy, Default)]
pub struct DestinationChange<'a> {
    /// A new name.
    pub name: Option<&'a str>,
    /// New values for the notifier's fields.
    pub config: Option<&'a Json>,
    /// New texts.
    pub templates: Option<&'a Json>,
    /// The kinds it is sent.
    pub kinds: Option<&'a [String]>,
    /// `all` or `important`.
    pub level: Option<&'a str>,
    /// Switched on or off. Switching it on takes away why it was switched off.
    pub enabled: Option<bool>,
}

/// Changes a destination. None where there is no such destination.
pub async fn update_destination(
    conn: &mut Conn,
    id: Id,
    change: &DestinationChange<'_>,
    at: DateTime<Utc>,
) -> Result<Option<DestinationRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "UPDATE destinations d SET name = COALESCE($2, d.name), config = COALESCE($3::jsonb, d.config), ",
        "templates = COALESCE($4::jsonb, d.templates), kinds = COALESCE($5::text[], d.kinds), ",
        "level = COALESCE($6, d.level), enabled = COALESCE($7, d.enabled), ",
        "last_error = CASE WHEN $7 IS TRUE AND NOT d.enabled THEN NULL ELSE d.last_error END, ",
        "updated_at = $8 WHERE d.id = $1 RETURNING ",
        destination_columns!()
    ))
    .bind(id)
    .bind(change.name)
    .bind(change.config)
    .bind(change.templates)
    .bind(change.kinds)
    .bind(change.level)
    .bind(change.enabled)
    .bind(at)
    .fetch_optional(conn)
    .await?)
}

/// Takes a browser out of a person's push subscriptions. Says whether it was
/// theirs and there. A browser of another person is not touched.
pub async fn remove_push_subscription_of(
    conn: &mut Conn,
    user: Id,
    endpoint: &str,
) -> Result<bool, DbError> {
    let done = sqlx::query("DELETE FROM push_subscriptions WHERE user_id = $1 AND endpoint = $2")
        .bind(user)
        .bind(endpoint)
        .execute(conn)
        .await?;
    Ok(done.rows_affected() == 1)
}
