//! Channels and destinations: where they are set up, the texts they carry, the secrets that belong to them and what the last delivery through each came to.
//!
//! A channel is a way out of the application to a person (a mail server, a
//! bot, a webhook, push); a destination is a place a node is told at. Both
//! name a notifier by its kind, keep its values without the secret ones, and
//! carry texts of their own where they do not send the shipped ones. The
//! secret values are rows of `secrets`, sealed, owned by the channel or the
//! destination and removed with it.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// The kinds of owner that `secrets` rows of this module have.
pub mod owner {
    /// A channel: a mail server's password, a bot's token, a webhook's key.
    pub const CHANNEL: &str = "notification_channel";
    /// A destination: the same for a node's place.
    pub const DESTINATION: &str = "destination";
}

macro_rules! channel_columns {
    () => {
        "id, org_id, user_id, kind, name, config, templates, enabled, link_cursor, last_ok_at, last_error, created_by, created_at, updated_at"
    };
}

macro_rules! destination_columns {
    () => {
        "id, node_id, channel, name, config, templates, kinds, level, enabled, last_ok_at, last_error, created_by, created_at, updated_at"
    };
}

/// A way out of the application to a person.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ChannelRow {
    /// Its identifier.
    pub id: Id,
    /// The root org it belongs to; none for the system's and for a person's.
    pub org_id: Option<Id>,
    /// The person it belongs to; none for the system's and for an org's.
    pub user_id: Option<Id>,
    /// The notifier it is made with: `email`, `telegram`, `webhook`, `push`
    /// or an installed one.
    pub kind: String,
    /// What it is called.
    pub name: String,
    /// The notifier's fields, without the secret ones.
    pub config: Json,
    /// Its own texts, by language; empty where it sends the shipped ones.
    pub templates: Json,
    /// Whether it is in use.
    pub enabled: bool,
    /// Where the last reading of who followed a link ended.
    pub link_cursor: Option<String>,
    /// When a delivery through it last succeeded.
    pub last_ok_at: Option<DateTime<Utc>>,
    /// Why the last delivery failed, until one succeeds.
    pub last_error: Option<String>,
    /// Who set it up.
    pub created_by: Id,
    /// When it was set up.
    pub created_at: DateTime<Utc>,
    /// When it was last changed.
    pub updated_at: DateTime<Utc>,
}

impl ChannelRow {
    /// Whether it is the system's own: it reaches everybody and is not held
    /// to the list of private hosts.
    #[must_use]
    pub fn is_the_systems(&self) -> bool {
        self.org_id.is_none() && self.user_id.is_none()
    }
}

/// The channel `id`.
pub async fn channel(conn: &mut Conn, id: Id) -> Result<Option<ChannelRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        channel_columns!(),
        " FROM notification_channels WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The channel `id` under a shared lock, the first lock a delivery takes: its removal, which
/// takes the channel alone, waits for the deliveries being reserved, and none is reserved after.
pub async fn lock_channel_shared(conn: &mut Conn, id: Id) -> Result<Option<ChannelRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        channel_columns!(),
        " FROM notification_channels WHERE id = $1 FOR SHARE"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The system's channel of a kind, if it has one (a system has one mail
/// server and one bot).
pub async fn system_channel(conn: &mut Conn, kind: &str) -> Result<Option<ChannelRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        channel_columns!(),
        " FROM notification_channels WHERE org_id IS NULL AND user_id IS NULL AND kind = $1"
    ))
    .bind(kind)
    .fetch_optional(conn)
    .await?)
}

/// A root org's channel of a kind, if it has one.
pub async fn org_channel(
    conn: &mut Conn,
    org: Id,
    kind: &str,
) -> Result<Option<ChannelRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        channel_columns!(),
        " FROM notification_channels WHERE org_id = $1 AND user_id IS NULL AND kind = $2",
        " ORDER BY created_at LIMIT 1"
    ))
    .bind(org)
    .bind(kind)
    .fetch_optional(conn)
    .await?)
}

/// What a new channel is made from.
#[derive(Debug, Clone, Copy)]
pub struct NewChannel<'a> {
    /// Its identifier.
    pub id: Id,
    /// The root org it belongs to.
    pub org_id: Option<Id>,
    /// The person it belongs to.
    pub user_id: Option<Id>,
    /// The notifier it is made with.
    pub kind: &'a str,
    /// What it is called.
    pub name: &'a str,
    /// The notifier's fields, without the secret ones.
    pub config: &'a Json,
    /// Its own texts.
    pub templates: &'a Json,
    /// Whether it is in use.
    pub enabled: bool,
    /// Who sets it up.
    pub created_by: Id,
    /// When.
    pub at: DateTime<Utc>,
}

/// Sets a channel up. A second mail server or bot for the same place is a
/// unique value that is taken (`notification_channels_one_idx`).
pub async fn insert_channel(conn: &mut Conn, new: &NewChannel<'_>) -> Result<ChannelRow, DbError> {
    Ok(sqlx::query_as(concat!(
        "INSERT INTO notification_channels (id, org_id, user_id, kind, name, config, templates, enabled, created_by, created_at, updated_at) ",
        "VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10) RETURNING ",
        channel_columns!()
    ))
    .bind(new.id)
    .bind(new.org_id)
    .bind(new.user_id)
    .bind(new.kind)
    .bind(new.name)
    .bind(new.config)
    .bind(new.templates)
    .bind(new.enabled)
    .bind(new.created_by)
    .bind(new.at)
    .fetch_one(conn)
    .await?)
}

/// What is changed of a channel; what is `None` stays.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChannelChange<'a> {
    /// A new name.
    pub name: Option<&'a str>,
    /// New values for the notifier's fields.
    pub config: Option<&'a Json>,
    /// New texts.
    pub templates: Option<&'a Json>,
    /// Switched on or off.
    pub enabled: Option<bool>,
}

/// Changes a channel. None where there is no such channel.
pub async fn update_channel(
    conn: &mut Conn,
    id: Id,
    change: &ChannelChange<'_>,
    at: DateTime<Utc>,
) -> Result<Option<ChannelRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "UPDATE notification_channels SET name = COALESCE($2, name), config = COALESCE($3::jsonb, config), ",
        "templates = COALESCE($4::jsonb, templates), enabled = COALESCE($5, enabled), updated_at = $6 ",
        "WHERE id = $1 RETURNING ",
        channel_columns!()
    ))
    .bind(id)
    .bind(change.name)
    .bind(change.config)
    .bind(change.templates)
    .bind(change.enabled)
    .bind(at)
    .fetch_optional(conn)
    .await?)
}

/// Deletes a channel and its sealed secrets. Its routes are kept, off, as records of what the
/// channel was, until their people remove them; what had not gone through them is cancelled, and
/// whoever had a route on, or work waiting, is told once in their Inbox. Says whether there was
/// such a channel.
pub async fn delete_channel(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
    // The removal takes the channel alone first: a delivery being reserved through it finishes
    // reserving, and none is reserved after. Its routes are kept as records, with their notices,
    // before the row goes, so that no route is lost with it.
    let locked: Option<Id> =
        sqlx::query_scalar("SELECT id FROM notification_channels WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    if locked.is_none() {
        return Ok(false);
    }
    super::people::keep_routes_and_tell(conn, id, at).await?;
    sqlx::query("DELETE FROM secrets WHERE owner_kind = $1 AND owner_id = $2")
        .bind(owner::CHANNEL)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    let done = sqlx::query("DELETE FROM notification_channels WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(done.rows_affected() == 1)
}

/// Notes that a delivery through the channel succeeded: the last error is
/// cleared.
pub async fn record_channel_ok(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE notification_channels SET last_ok_at = $2, last_error = NULL WHERE id = $1",
    )
    .bind(id)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Keeps why the last delivery through the channel failed.
pub async fn record_channel_error(conn: &mut Conn, id: Id, error: &str) -> Result<(), DbError> {
    sqlx::query("UPDATE notification_channels SET last_error = $2 WHERE id = $1")
        .bind(id)
        .bind(error)
        .execute(conn)
        .await?;
    Ok(())
}

/// Keeps where the last reading of who followed a link ended.
pub async fn set_link_cursor(conn: &mut Conn, id: Id, cursor: Option<&str>) -> Result<(), DbError> {
    sqlx::query("UPDATE notification_channels SET link_cursor = $2 WHERE id = $1")
        .bind(id)
        .bind(cursor)
        .execute(conn)
        .await?;
    Ok(())
}

/// The switched-on channels that have a route with a code that is still
/// good at `now`: the only ones whose notifier is asked who followed its
/// link.
pub async fn channels_with_waiting_codes(
    conn: &mut Conn,
    now: DateTime<Utc>,
) -> Result<Vec<ChannelRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        channel_columns!(),
        " FROM notification_channels c WHERE c.enabled AND EXISTS (",
        "SELECT 1 FROM notification_routes r WHERE r.channel_id = c.id ",
        "AND r.link_hash IS NOT NULL AND r.link_expires_at > $1) ORDER BY c.created_at"
    ))
    .bind(now)
    .fetch_all(conn)
    .await?)
}

/// Takes the lock that serialises the setting up of the system's channels of
/// a kind until the transaction ends, so that two operators who set up the
/// system's one mail server or one bot at the same moment do not each make
/// one: the second finds the first's and replaces it. The schema keeps the
/// built-in kinds apart by itself; a kind that was installed is kept apart
/// by this.
pub async fn lock_system_channels(conn: &mut Conn, kind: &str) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("system channels of {kind}"))
        .execute(conn)
        .await?;
    Ok(())
}

/// The channels of the system of a kind, the oldest first: a system has one
/// mail server and one bot, and any number of webhooks.
pub async fn system_channels_of(conn: &mut Conn, kind: &str) -> Result<Vec<ChannelRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        channel_columns!(),
        " FROM notification_channels WHERE org_id IS NULL AND user_id IS NULL AND kind = $1",
        " ORDER BY created_at, id"
    ))
    .bind(kind)
    .fetch_all(conn)
    .await?)
}

/// Puts new values over a channel that is set up again: its name, its fields,
/// and what the look at the far side found (`last_error`, none where it found
/// nothing wrong). It is switched on. Where the last reading of who followed a
/// link ended is forgotten, since the fields may name another bot. Its texts,
/// its routes and when a delivery through it last succeeded are left as they
/// were. None where there is no such channel.
pub async fn replace_channel(
    conn: &mut Conn,
    id: Id,
    name: &str,
    config: &Json,
    last_error: Option<&str>,
    at: DateTime<Utc>,
) -> Result<Option<ChannelRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "UPDATE notification_channels SET name = $2, config = $3, enabled = true, last_error = $4, link_cursor = NULL, updated_at = $5 ",
        "WHERE id = $1 RETURNING ",
        channel_columns!()
    ))
    .bind(id)
    .bind(name)
    .bind(config)
    .bind(last_error)
    .bind(at)
    .fetch_optional(conn)
    .await?)
}

/// A place a node is told at.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct DestinationRow {
    /// Its identifier.
    pub id: Id,
    /// The org, project or topic it is told what happens at.
    pub node_id: Id,
    /// The notifier it is made with.
    pub channel: String,
    /// What it is called.
    pub name: String,
    /// The notifier's fields, without the secret ones.
    pub config: Json,
    /// Its own texts.
    pub templates: Json,
    /// The kinds it is sent; empty for every kind it would be.
    pub kinds: Vec<String>,
    /// What a subscriber at this level would be sent: `all` or `important`.
    pub level: String,
    /// Whether it is in use.
    pub enabled: bool,
    /// When a delivery to it last succeeded.
    pub last_ok_at: Option<DateTime<Utc>>,
    /// Why the last delivery failed, until one succeeds.
    pub last_error: Option<String>,
    /// Who set it up.
    pub created_by: Id,
    /// When it was set up.
    pub created_at: DateTime<Utc>,
    /// When it was last changed.
    pub updated_at: DateTime<Utc>,
}

/// The destination `id`.
pub async fn destination(conn: &mut Conn, id: Id) -> Result<Option<DestinationRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        destination_columns!(),
        " FROM destinations WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// What a new destination is made from.
#[derive(Debug, Clone, Copy)]
pub struct NewDestination<'a> {
    /// Its identifier.
    pub id: Id,
    /// The node it is told what happens at.
    pub node_id: Id,
    /// The notifier it is made with.
    pub channel: &'a str,
    /// What it is called.
    pub name: &'a str,
    /// The notifier's fields, without the secret ones.
    pub config: &'a Json,
    /// Its own texts.
    pub templates: &'a Json,
    /// The kinds it is sent; empty for every kind it would be.
    pub kinds: &'a [String],
    /// `all` or `important`.
    pub level: &'a str,
    /// Whether it is in use.
    pub enabled: bool,
    /// Who sets it up.
    pub created_by: Id,
    /// When.
    pub at: DateTime<Utc>,
}

/// Sets a destination up.
pub async fn insert_destination(
    conn: &mut Conn,
    new: &NewDestination<'_>,
) -> Result<DestinationRow, DbError> {
    Ok(sqlx::query_as(concat!(
        "INSERT INTO destinations (id, node_id, channel, name, config, templates, kinds, level, enabled, created_by, created_at, updated_at) ",
        "VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $11) RETURNING ",
        destination_columns!()
    ))
    .bind(new.id)
    .bind(new.node_id)
    .bind(new.channel)
    .bind(new.name)
    .bind(new.config)
    .bind(new.templates)
    .bind(new.kinds)
    .bind(new.level)
    .bind(new.enabled)
    .bind(new.created_by)
    .bind(new.at)
    .fetch_one(conn)
    .await?)
}

/// Deletes a destination with its queued deliveries and its sealed secrets.
/// Says whether there was such a destination.
pub async fn delete_destination(conn: &mut Conn, id: Id) -> Result<bool, DbError> {
    sqlx::query("DELETE FROM secrets WHERE owner_kind = $1 AND owner_id = $2")
        .bind(owner::DESTINATION)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    let done = sqlx::query("DELETE FROM destinations WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(done.rows_affected() == 1)
}

/// Switches a destination off, as a delivery that found its place gone does.
/// Says whether it was on.
pub async fn switch_off_destination(
    conn: &mut Conn,
    id: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE destinations SET enabled = false, updated_at = $2 WHERE id = $1 AND enabled",
    )
    .bind(id)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Notes that a delivery to the destination succeeded: the last error is
/// cleared.
pub async fn record_destination_ok(
    conn: &mut Conn,
    id: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE destinations SET last_ok_at = $2, last_error = NULL WHERE id = $1")
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// Keeps why the last delivery to the destination failed.
pub async fn record_destination_error(conn: &mut Conn, id: Id, error: &str) -> Result<(), DbError> {
    sqlx::query("UPDATE destinations SET last_error = $2 WHERE id = $1")
        .bind(id)
        .bind(error)
        .execute(conn)
        .await?;
    Ok(())
}
