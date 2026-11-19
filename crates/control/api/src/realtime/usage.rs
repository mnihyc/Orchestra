//! Changed model requests projected under each subscriber's current usage and record rights.

use super::server::Connection;
use super::subscriptions::{Access, principal_for_connection, principal_of};
use super::{Fail, Inner};
use crate::routes::usage::{events, scope};
use chrono::Duration as Span;
use orch_api_contract::common::Page;
use orch_api_contract::usage::UsageOutcomeFilter;
use orch_db::usage::ledger::{self, Listing};
use orch_realtime_contract::feeds::Feed;
use orch_types::{Id, Json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// Each query and outgoing packet stays within the ordinary record page bound.
const LIVE_ROWS: i64 = 200;
/// Initial completions cover the last day; the history cursor repairs longer gaps.
const LIVE_DAYS: i64 = 1;
/// One cheap shared revision read per second, only while a usage feed is followed.
const EVERY: Duration = Duration::from_secs(1);
/// Re-evaluate a moving health window at the previous status cadence, with no browser polling.
const HEALTH_EVERY: Duration = Duration::from_secs(10);

fn parts(text: &str) -> Result<(&str, Option<Id>), Fail> {
    match text.split_once(":org:") {
        Some((scope, org)) => Ok((scope, Some(org.parse().map_err(Fail::other)?))),
        None => Ok((text, None)),
    }
}

pub(crate) async fn decide(inner: &Inner, user: Id, text: &str) -> Result<Access, Fail> {
    let health = text.ends_with(":health");
    let (text, org) = parts(text.strip_suffix(":health").unwrap_or(text))?;
    if health && org.is_some() {
        return Ok(Access::NotFound);
    }
    let mut conn = inner.db.acquire().await?;
    let Some(principal) = principal_of(inner, &mut conn, user).await? else {
        return Ok(Access::Forbidden);
    };
    let Ok(asked) = scope::parse(text) else {
        return Ok(Access::NotFound);
    };
    if health && !matches!(asked, scope::Scope::Provider(_)) {
        return Ok(Access::NotFound);
    }
    Ok(if scope::open(&mut conn, &principal, asked).await.is_ok() {
        Access::Allowed
    } else {
        Access::Forbidden
    })
}

/// A server-only commit cursor avoids exposing installation-wide activity to the browser.
/// Each delivery retakes authority; unchanged subscriptions produce no packet and no name lookups.
pub(crate) async fn project(
    inner: &Arc<Inner>,
    connection: &Connection,
    text: &str,
    read: &mut Option<i64>,
) -> Result<Vec<Json>, Fail> {
    if let Some(plain) = text.strip_suffix(":health") {
        let scope::Scope::Provider(provider) = scope::parse(plain).map_err(Fail::other)? else {
            return Ok(Vec::new());
        };
        return Ok(super::resource_health::project(inner, connection, provider)
            .await?
            .into_iter()
            .collect());
    }
    let (text, org) = parts(text)?;
    let app = inner
        .app
        .get()
        .and_then(std::sync::Weak::upgrade)
        .ok_or_else(|| Fail::other("the application is unavailable"))?;
    let mut tx = inner.db.begin().await?;
    // A reader who may no longer read the scope is sent nothing, as the health projection does: the
    // change that took it away moved the epoch, whose recheck closes the feed (revoked). It is no failure
    // of the connection, which a subscription seeded just as access was taken would otherwise end.
    let Some(principal) = principal_for_connection(inner, &mut tx, connection).await? else {
        return Ok(Vec::new());
    };
    let opened = match scope::open(
        &mut tx,
        &principal,
        scope::parse(text).map_err(Fail::other)?,
    )
    .await
    {
        Ok(opened) => opened,
        Err(error) if error.code() == orch_api_contract::common::ErrorCode::Internal => {
            return Err(Fail::other(error));
        }
        Err(_) => return Ok(Vec::new()),
    };
    let mut filter = scope::filter(
        &mut tx,
        &principal,
        &opened,
        &scope::Wanted::default(),
        true,
    )
    .await
    .map_err(Fail::other)?;
    filter.org = org;
    let all_places = scope::all_record_places(&mut tx, &principal, &opened)
        .await
        .map_err(Fail::other)?;
    let through = ledger::revision(&mut tx).await?;
    let mut packets = Vec::new();
    if let Some(mut after) = *read {
        while after < through {
            let rows = ledger::live_changes(&mut tx, &filter, after, through, LIVE_ROWS).await?;
            let Some(last) = rows.last() else {
                break;
            };
            after = last.change_seq;
            let more = rows.len() == LIVE_ROWS as usize;
            let rows = scope::visible_records(&mut tx, &principal, all_places, rows)
                .await
                .map_err(Fail::other)?;
            let items = events::events(&app, &mut tx, &principal, &rows)
                .await
                .map_err(Fail::other)?;
            if !items.is_empty() {
                packets.push(
                    serde_json::to_value(Page {
                        items,
                        next_cursor: None,
                    })
                    .map_err(Fail::other)?,
                );
            }
            if !more {
                break;
            }
        }
    } else {
        let until = inner.clock.now() + Span::milliseconds(1);
        let mut rows = events::record_page(
            &mut tx,
            &principal,
            all_places,
            &Listing {
                from: Some(until - Span::days(LIVE_DAYS)),
                until: Some(until),
                filter: &filter,
                counted: None,
                newest_first: true,
                after: None,
                limit: LIVE_ROWS,
            },
            false,
            true,
            Some(through),
        )
        .await
        .map_err(Fail::other)?;
        let next_cursor = rows
            .last()
            .map(|row| format!("{}.{}", row.at.timestamp_micros(), row.id));
        let mut running = filter.clone();
        running.outcome = Some(UsageOutcomeFilter::Running);
        rows.extend(
            events::record_page(
                &mut tx,
                &principal,
                all_places,
                &Listing {
                    from: None,
                    until: Some(until),
                    filter: &running,
                    counted: None,
                    newest_first: true,
                    after: None,
                    limit: LIVE_ROWS,
                },
                true,
                true,
                Some(through),
            )
            .await
            .map_err(Fail::other)?,
        );
        rows.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.id.cmp(&a.id)));
        let items = events::events(&app, &mut tx, &principal, &rows)
            .await
            .map_err(Fail::other)?;
        packets.push(serde_json::to_value(Page { items, next_cursor }).map_err(Fail::other)?);
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    *read = Some(through);
    Ok(packets)
}

/// The coordinator reads one indexed counter, never a whole ledger per viewer.
#[derive(Default)]
pub(crate) struct Sources {
    sampled: Option<tokio::time::Instant>,
    heads: HashMap<Feed, i64>,
    health: HashMap<Feed, tokio::time::Instant>,
}

impl Sources {
    pub(crate) async fn reconcile(
        &mut self,
        inner: &Arc<Inner>,
        active: &HashSet<Feed>,
    ) -> Result<(), Fail> {
        self.heads.retain(|feed, _| active.contains(feed));
        self.health.retain(|feed, _| active.contains(feed));
        if !active.iter().any(|feed| matches!(feed, Feed::Usage(_))) {
            return Ok(());
        }
        if self.sampled.is_some_and(|at| at.elapsed() < EVERY) {
            return Ok(());
        }
        self.sampled = Some(tokio::time::Instant::now());
        // Return the read connection before broadcast takes its own database connection.
        let revision = {
            let mut conn = inner.db.acquire().await?;
            ledger::revision(&mut conn).await?
        };
        for feed in active.iter().filter(|feed| matches!(feed, Feed::Usage(_))) {
            if matches!(feed,Feed::Usage(scope) if scope.ends_with(":health")) {
                if self
                    .health
                    .get(feed)
                    .is_some_and(|at| at.elapsed() < HEALTH_EVERY)
                {
                    continue;
                }
                self.health
                    .insert(feed.clone(), tokio::time::Instant::now());
            } else if self.heads.get(feed) == Some(&revision) {
                continue;
            }
            super::ephemeral::broadcast(
                &inner.db,
                &super::ephemeral::Message::List {
                    feed: feed.clone(),
                    data: serde_json::json!({}),
                },
            )
            .await?;
            self.heads.insert(feed.clone(), revision);
        }
        Ok(())
    }
}
