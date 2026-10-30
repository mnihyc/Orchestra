//! A person's routes through channels: what waits for an unread delay or a frequency limit, sent as one message when its time has come, and what quiet hours held.
//!
//! Nothing leaves the application for a person until they switch a route on.
//! A route with neither an unread delay nor a limit is served in the
//! transaction of the event, by one row of the outbox for each notification.
//! A route with either is served here, by the `routes` job: the person's
//! notifications that the channel covers and the route's kinds take, that come
//! after the sequence the route has told up to, that are still unread and older
//! than the delay, are written as one row of the outbox when the last message
//! through the route went out at least the interval ago; then the route moves
//! past them. What was read meanwhile is never sent. While a person's quiet
//! hours last the job does nothing for them, and when they end everything that
//! waited goes together.
//!
//! The functions below the job are what a person's own requests call: what a
//! route needs before it can be on, changing it, and ending it.

use crate::channels::{self, ChannelError};
use crate::quiet_hours;
use crate::schedule::{self, Job};
use chrono::{DateTime, Duration, Utc};
use orch_db::queries::notifications::channels::ChannelRow;
use orch_db::queries::notifications::outbox::{self, NewRow, Payload};
use orch_db::queries::notifications::people::{self, Recipient};
use orch_db::queries::notifications::routes::{self, RouteRow};
use orch_db::{Conn, Db, DbError};
use orch_notifier_contract::delivery::NotificationKind;
use orch_notifier_contract::descriptor::{Needs, NotifierDescriptor};
use orch_types::{Clock, Id, Json};

/// The longest an unread delay or a limit may be, in minutes: a day, which is
/// also the bound the schema puts on both.
pub const LONGEST_WAIT_MINUTES: i32 = 24 * 60;

/// Why a route could not be changed.
#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    /// The database refused.
    #[error(transparent)]
    Db(#[from] DbError),
    /// A channel could not be read.
    #[error(transparent)]
    Channel(#[from] ChannelError),
    /// The route cannot be on yet: the person lacks what the channel reaches
    /// them by.
    #[error("the route cannot be switched on before the person has what it needs: {0:?}")]
    NotReady(Needs),
    /// A value is wrong.
    #[error("{field} {message}")]
    Invalid {
        /// The member.
        field: &'static str,
        /// What is wrong with it.
        message: String,
    },
}

/// What a run of the job did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Served {
    /// Routes that had something to tell.
    pub routes: u32,
    /// Notifications that were told, together.
    pub notifications: u32,
}

/// Serves the routes that wait, under the lock of the job: for each route that
/// is on and has an unread delay or a limit, what waits for it, as one row of
/// the outbox, when its time has come. First the pauses that ran out end, each
/// only where it is still the one that ran out, and what they held is made due
/// at once, so that it goes as one message.
pub async fn serve(db: &Db, clock: &dyn Clock) -> Result<Served, RouteError> {
    let Some(mut tx) = schedule::take(db, Job::Routes).await? else {
        return Ok(Served::default());
    };
    let now = clock.now();
    for person in orch_db::queries::notifications::inbox::expire_pauses(&mut tx, now).await? {
        outbox::rearm_held_of(&mut tx, person, now).await?;
    }
    let shared = channels::mail_may_be_shared(&mut tx).await?;
    let mut served = Served::default();
    for route in routes::due_routes(&mut tx).await? {
        let told = serve_one(&mut tx, &route, now, shared).await?;
        if told > 0 {
            served.routes += 1;
            served.notifications += told;
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(served)
}

/// Serves one route if its time has come. Says how many notifications went.
async fn serve_one(
    conn: &mut Conn,
    route: &RouteRow,
    now: DateTime<Utc>,
    mail_shared: bool,
) -> Result<u32, RouteError> {
    let Some(person) = people::recipient(conn, route.user_id).await? else {
        return Ok(0);
    };
    if quiet_hours::outward_held_until(
        &person.timezone,
        person.quiet_window(),
        person.pause_until,
        now,
    )
    .is_some()
    {
        return Ok(0);
    }
    if let (Some(minutes), Some(last)) = (route.min_interval_minutes, route.last_sent_at)
        && now < last + Duration::minutes(i64::from(minutes))
    {
        return Ok(0);
    }
    let older_than = route
        .unread_delay_minutes
        .map(|minutes| now - Duration::minutes(i64::from(minutes)));
    let waiting = routes::waiting_inbox(conn, route.id, older_than, mail_shared).await?;
    let Some(newest) = waiting.last().map(|each| each.inbox_seq) else {
        return Ok(0);
    };
    // Moving first is what keeps two runs from telling the same things: only
    // the one that finds the route where it was goes on.
    if !routes::move_on(conn, route.id, route.sent_seq, newest, now).await? {
        return Ok(0);
    }
    let Some(channel_id) = route.channel_id else {
        return Ok(0);
    };
    let Some(channel) =
        orch_db::queries::notifications::channels::channel(conn, channel_id).await?
    else {
        return Ok(0);
    };
    let snapshots: Vec<_> = waiting.iter().map(routes::WaitingRow::snapshot).collect();
    outbox::insert(
        conn,
        &NewRow {
            id: orch_types::new_id(),
            channel: &channel.kind,
            route_id: Some(route.id),
            destination_id: None,
            channel_id: None,
            payload: &Payload::of(&snapshots).to_json(),
            at: now,
        },
    )
    .await?;
    Ok(u32::try_from(snapshots.len()).unwrap_or(u32::MAX))
}

/// What a route through a channel needs of the person before it can be on, or
/// none if it needs nothing more: a confirmed address for mail, a browser that
/// took push, the link followed for a bot.
pub async fn needs(
    conn: &mut Conn,
    declared: &NotifierDescriptor,
    person: &Recipient,
    route: Option<&RouteRow>,
) -> Result<Option<Needs>, DbError> {
    Ok(match declared.reaches.needs() {
        None => None,
        Some(Needs::VerifiedEmail) => person
            .confirmed_email()
            .is_none()
            .then_some(Needs::VerifiedEmail),
        Some(Needs::Device) => people::push_subscriptions_of(conn, person.id)
            .await?
            .is_empty()
            .then_some(Needs::Device),
        Some(Needs::Link) => route
            .is_none_or(|route| {
                route.linked_at.is_none()
                    || route
                        .address
                        .as_object()
                        .is_none_or(serde_json::Map::is_empty)
            })
            .then_some(Needs::Link),
    })
}

/// What a person changes of their route; what is `None` stays. A delay or a
/// limit that is `Some(None)` is taken away.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    /// Switched on or off.
    pub enabled: Option<bool>,
    /// The kinds it tells; empty for everything the channel covers.
    pub kinds: Option<Vec<String>>,
    /// Tell only what is still unread after this many minutes.
    pub unread_delay_minutes: Option<Option<i32>>,
    /// Tell at most once in this many minutes.
    pub min_interval_minutes: Option<Option<i32>>,
    /// Send again, as one message, what failed through the route and is still unread.
    pub retry_failed: bool,
}

fn check_wait(field: &'static str, value: Option<Option<i32>>) -> Result<(), RouteError> {
    match value {
        Some(Some(minutes)) if !(1..=LONGEST_WAIT_MINUTES).contains(&minutes) => {
            Err(RouteError::Invalid {
                field,
                message: format!("is from 1 to {LONGEST_WAIT_MINUTES} minutes, or left out"),
            })
        }
        _ => Ok(()),
    }
}

/// Changes the route of `person` through `channel`, making it if they have
/// none. A new route is off. A route that is switched on has to be ready:
/// `Err(NotReady)` says what it lacks. When a route is switched on it starts
/// from the present: what came before is not told to the person as if it were
/// new.
pub async fn set(
    conn: &mut Conn,
    declared: &NotifierDescriptor,
    person: &Recipient,
    channel: &ChannelRow,
    change: &Change,
    now: DateTime<Utc>,
) -> Result<RouteRow, RouteError> {
    check_wait("unread_delay_minutes", change.unread_delay_minutes)?;
    check_wait("min_interval_minutes", change.min_interval_minutes)?;
    if let Some(kinds) = &change.kinds {
        for kind in kinds {
            serde_json::from_value::<NotificationKind>(Json::String(kind.clone())).map_err(
                |_| RouteError::Invalid {
                    field: "kinds",
                    message: format!("{kind:?} is not a kind of notification"),
                },
            )?;
        }
    }
    let before = routes::route_for(conn, person.id, channel.id).await?;
    let switching_on =
        change.enabled == Some(true) && before.as_ref().is_none_or(|route| !route.enabled);
    if switching_on && let Some(lacks) = needs(conn, declared, person, before.as_ref()).await? {
        return Err(RouteError::NotReady(lacks));
    }
    let route = routes::put_route(
        conn,
        orch_types::new_id(),
        person.id,
        channel.id,
        &routes::RouteChange {
            enabled: change.enabled,
            kinds: change.kinds.as_deref(),
            unread_delay_minutes: change.unread_delay_minutes,
            min_interval_minutes: change.min_interval_minutes,
        },
        now,
    )
    .await?;
    if switching_on {
        routes::catch_up(conn, route.id, now).await?;
    }
    if change.retry_failed {
        retry_failed(conn, declared, person, channel, &route, now).await?;
    }
    if switching_on || change.retry_failed {
        return Ok(routes::route(conn, route.id).await?.unwrap_or(route));
    }
    Ok(route)
}

/// A person's explicit retry of what failed through their route: only for a route that is on and
/// can reach them, and only what is still unread, as one message. The failed deliveries it
/// replaces are given up as joined into it. Nothing is sent that was read, and no other route is
/// switched on.
async fn retry_failed(
    conn: &mut Conn,
    declared: &NotifierDescriptor,
    person: &Recipient,
    channel: &ChannelRow,
    route: &RouteRow,
    now: DateTime<Utc>,
) -> Result<(), RouteError> {
    if !route.enabled || needs(conn, declared, person, Some(route)).await?.is_some() {
        return Err(RouteError::Invalid {
            field: "retry_failed",
            message: "is for a route that is on and can reach you".to_owned(),
        });
    }
    let sources = outbox::failed_sources_of_route(conn, route.id).await?;
    if sources.is_empty() {
        return Ok(());
    }
    let waiting = routes::unread_rows_of(conn, person.id, &sources).await?;
    if waiting.is_empty() {
        return Ok(());
    }
    let snapshots: Vec<_> = waiting.iter().map(routes::WaitingRow::snapshot).collect();
    outbox::insert(
        conn,
        &NewRow {
            id: orch_types::new_id(),
            channel: &channel.kind,
            route_id: Some(route.id),
            destination_id: None,
            channel_id: None,
            payload: &Payload::of(&snapshots).to_json(),
            at: now,
        },
    )
    .await?;
    Ok(())
}

/// Ends the route of `person` through `channel`: it is off, and where it was
/// linked is forgotten. Says whether they had one.
pub async fn end(
    conn: &mut Conn,
    person: Id,
    channel: Id,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    match routes::route_for(conn, person, channel).await? {
        Some(route) => routes::switch_off(conn, route.id, true, now).await,
        None => Ok(false),
    }
}
