//! A person's routes through the channels that reach them: switched on or off, with the kinds they tell, the unread delay and the limit, the link of a route that has to be linked, and ended.
//!
//! Every route starts off, for every channel. Nobody switches a route on for
//! somebody else, and a route cannot be on before it can reach its person: a
//! confirmed address for mail, a browser that took push, a bot that was
//! started. Until then it says what it needs.

use super::views::{self, RouteFacts, needs_view, route_view};
use super::{master_key, notifiers};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::Ctx;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::notifications::{
    MeNotificationRouteSetBody, NotificationRoute, NotificationRouteLink, ROUTE_MINUTES_MAX,
    ROUTE_MINUTES_MIN,
};
use orch_db::DbError;
use orch_db::queries::notifications::channels::{self as channel_rows, ChannelRow};
use orch_db::queries::notifications::routes as route_rows;
use orch_db::queries::notifications::{pages, people};
use orch_types::{Id, Time};
use orch_worker_core::bot_links::{self, LinkError};
use orch_worker_core::routes::{self as route_logic, Change, RouteError};
use std::collections::BTreeMap;

/// The channel the path names, if it reaches the caller: a channel of
/// somebody else's is not there for them.
async fn reaching(ctx: &Ctx, conn: &mut orch_db::Conn) -> ApiResult<(Id, ChannelRow)> {
    let channel = ctx.id("channel_id")?;
    let row = pages::channel_reaching(conn, ctx.user()?.id(), channel)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok((channel, row))
}

/// A route's minutes as given: absent (unchanged), null (none), or a number of minutes within the contract's
/// bounds, which the error names; nothing outside them reaches the database.
fn minutes(field: &str, value: Option<Option<u32>>) -> ApiResult<Option<Option<i32>>> {
    match value {
        None => Ok(None),
        Some(None) => Ok(Some(None)),
        Some(Some(minutes)) => (ROUTE_MINUTES_MIN..=ROUTE_MINUTES_MAX)
            .contains(&minutes)
            .then(|| i32::try_from(minutes).ok())
            .flatten()
            .map(|minutes| Some(Some(minutes)))
            .ok_or_else(|| {
                ApiError::invalid(
                    field,
                    &format!("is from {ROUTE_MINUTES_MIN} to {ROUTE_MINUTES_MAX} minutes, or null"),
                )
            }),
    }
}

/// Changes the caller's own switch for a channel: on or off, what it tells,
/// how long a notification stays unread before it goes out, and how often at
/// most a message goes.
pub async fn set(
    ctx: Ctx,
    (): (),
    body: MeNotificationRouteSetBody,
) -> ApiResult<NotificationRoute> {
    let me = ctx.user()?.id();
    let change = Change {
        enabled: body.enabled,
        kinds: body.kinds.as_ref().map(|kinds| {
            kinds
                .iter()
                .map(|kind| crate::routes::notifications::kind_word(*kind))
                .collect()
        }),
        unread_delay_minutes: minutes("unread_delay_minutes", body.unread_delay_minutes)?,
        min_interval_minutes: minutes("min_interval_minutes", body.min_interval_minutes)?,
        retry_failed: body.retry_failed.unwrap_or(false),
    };
    let mut tx = ctx.app.db.begin().await?;
    let (channel, row) = reaching(&ctx, &mut tx).await?;
    let declared = notifiers()
        .get(&row.kind)
        .map(|notifier| notifier.describe())
        .ok_or_else(|| {
            ApiError::invalid(
                "channel_id",
                &format!("is made from {}, which this system does not have", row.kind),
            )
        })?;
    let person = people::recipient(&mut tx, me)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let route = match route_logic::set(&mut tx, &declared, &person, &row, &change, ctx.now).await {
        Ok(route) => route,
        Err(RouteError::NotReady(needs)) => {
            return Err(ApiError::new(
                ErrorCode::RouteNotReady,
                "the route cannot be switched on before it can reach you",
            )
            .with("needs", needs_view(needs)));
        }
        Err(RouteError::Invalid { field, message }) => {
            return Err(ApiError::invalid(field, &message));
        }
        Err(RouteError::Db(error)) => return Err(error.into()),
        Err(RouteError::Channel(error)) => return Err(ApiError::internal(error)),
    };
    let needs = route_logic::needs(&mut tx, &declared, &person, Some(&route)).await?;
    let waiting_count = route_rows::waiting_count(&mut tx, route.id).await?;
    let link = super::channels::own_link(&mut tx, &route, ctx.now).await?;
    let orgs = orgs_of(&mut tx, row.org_id.into_iter().collect()).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(route_view(RouteFacts {
        channel_id: Some(channel),
        channel: Some(views::route_channel_of(&row, &orgs)),
        route: Some(&route),
        needs,
        waiting_count,
        link,
    }))
}

/// The root orgs named, as the caller may know them.
async fn orgs_of(conn: &mut orch_db::Conn, ids: Vec<Id>) -> ApiResult<BTreeMap<Id, NodeRef>> {
    Ok(people::places(conn, &ids)
        .await?
        .into_iter()
        .map(|place| {
            (
                place.id,
                NodeRef {
                    thread_display: None,
                    id: place.id,
                    kind: NodeKind::Org,
                    name: Some(place.name),
                    redacted: false,
                },
            )
        })
        .collect())
}

/// The caller's own routes: one for each channel they changed theirs for, and those kept after
/// their channel was removed, until they remove them. A removed channel's org is named only
/// while the caller is still a member of it.
pub async fn list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<NotificationRoute>> {
    let me = ctx.user()?.id();
    let mut conn = ctx.app.db.acquire().await?;
    let person = people::recipient(&mut conn, me)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let kept = pages::routes_of(&mut conn, me).await?;
    let reaching: BTreeMap<Id, ChannelRow> = pages::channels_reaching(&mut conn, me)
        .await?
        .into_iter()
        .map(|row| (row.id, row))
        .collect();
    let org_ids: Vec<Id> = kept
        .iter()
        .filter_map(|route| {
            route
                .kept_channel_text("org_id")
                .and_then(|text| text.parse().ok())
        })
        .filter(|org| reaching.values().any(|row| row.org_id == Some(*org)))
        .collect();
    let orgs = orgs_of(&mut conn, org_ids).await?;
    let mut shown = Vec::with_capacity(kept.len());
    for route in &kept {
        let current = route.channel_id.and_then(|channel| reaching.get(&channel));
        // A route through a channel that no longer reaches the person is not theirs to see.
        if route.channel_id.is_some() && current.is_none() {
            continue;
        }
        let needs = match current {
            Some(row) => match notifiers().get(&row.kind) {
                Some(notifier) => {
                    route_logic::needs(&mut conn, &notifier.describe(), &person, Some(route))
                        .await?
                }
                None => None,
            },
            None => None,
        };
        let waiting_count = route_rows::waiting_count(&mut conn, route.id).await?;
        let link = super::channels::own_link(&mut conn, route, ctx.now).await?;
        shown.push(route_view(RouteFacts {
            channel_id: route.channel_id,
            channel: match current {
                Some(row) => Some(views::route_channel_of(row, &orgs)),
                None => views::route_channel_kept(route, &orgs),
            },
            route: Some(route),
            needs,
            waiting_count,
            link,
        }));
    }
    Ok(shown)
}

/// Removes one of the caller's own routes by its identifier: one that is off, or kept after its
/// channel was removed. Nobody is told of a failure for a removal they made themselves.
pub async fn remove(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let route = ctx.id("route_id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let me = guard.user().id();
    let Some(row) = route_rows::route(&mut tx, route).await? else {
        return Err(ApiError::not_found());
    };
    if row.user_id != me {
        return Err(ApiError::not_found());
    }
    if row.enabled {
        return Err(ApiError::invalid(
            "route_id",
            "is on: switch it off before removing it",
        ));
    }
    if let Some(channel) = row.kept_channel_id() {
        orch_db::queries::settings::delete_secret(
            &mut tx,
            channel_rows::owner::CHANNEL,
            channel,
            &bot_links::link_secret_name(row.id),
        )
        .await?;
    }
    if !route_rows::remove_own(&mut tx, me, route).await? {
        return Err(ApiError::not_found());
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Makes the link that opens a bot with a code, good for ten minutes, for a
/// channel whose notifier links people. When the caller presses Start there,
/// the route is linked and their feed says so.
pub async fn link_create(ctx: Ctx, (): (), (): ()) -> ApiResult<NotificationRouteLink> {
    let me = ctx.user()?.id();
    let mut tx = ctx.app.db.begin().await?;
    let (_, row) = reaching(&ctx, &mut tx).await?;
    if !row.enabled {
        return Err(ApiError::invalid("channel_id", "is switched off"));
    }
    let started = bot_links::start(&mut tx, notifiers(), Some(master_key()?), me, &row, ctx.now)
        .await
        .map_err(|error| match error {
            LinkError::NotLinkable(kind) => ApiError::invalid(
                "channel_id",
                &format!("is made from {kind}, which does not link people"),
            ),
            LinkError::Db(error) => error.into(),
            LinkError::Channel(error) => ApiError::internal(error),
        })?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(NotificationRouteLink {
        url: started.url,
        expires_at: Time::new(started.expires_at),
    })
}

/// Switches the caller's route off and unlinks it: where a bot had them, it
/// forgets the chat.
pub async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let mut tx = ctx.app.db.begin().await?;
    let (channel, _) = reaching(&ctx, &mut tx).await?;
    route_logic::end(&mut tx, ctx.user()?.id(), channel, ctx.now).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
