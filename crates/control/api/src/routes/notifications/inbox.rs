//! The caller's Inbox: the list newest first, the badge's counters, what is marked read, what they chose about being notified, the browsers that take push, and the kinds of notifier this system has.
//!
//! The counters are two: how many notifications wait unread, and how many
//! approvals wait in the caller's own list of approvals. The second is counted
//! as that list is made (`routes/approvals`); it is the number of the Approvals
//! badge, and the same one the caller's feed carries.

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::realtime::user::notification_view;
use crate::registry::Ctx;
use crate::routes::nodes::view;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_api_contract::chats::ChatLevel;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::common::{Page, PageQuery};
use orch_api_contract::notifications::{
    MeNotificationPrefsSetBody, MePushSubscriptionCreateBody, MePushSubscriptionDeleteBody,
    Notification, NotificationGrouping, NotificationKind, NotificationListQuery, NotificationPrefs,
    NotificationReadBody, NotificationReadReply, NotificationSummaryGetQuery,
    NotificationSummaryGetReply, Notifier, OutsideStatus, OutsideWay, OutsideWayState, PushDevice,
    QUIET_MINUTE_MAX, QuietHours,
};
use orch_authz::{Act, Policy, Principal};
use orch_db::Conn;
use orch_db::queries::notifications::inbox::{self, After, PrefsChange, PrefsRow, ReadFilter};
use orch_db::queries::notifications::outbox;
use orch_db::queries::notifications::pages;
use orch_db::queries::notifications::people::{self, NewPushSubscription};
use orch_types::{Id, Json, Seq, Time};

/// The most bytes of a push endpoint: what push services make, with room.
const ENDPOINT_MAX_BYTES: usize = 2048;
/// The length of a browser's public key for push: an uncompressed point of
/// the P-256 curve.
const P256DH_BYTES: usize = 65;
/// The first byte of an uncompressed point.
const UNCOMPRESSED_POINT: u8 = 0x04;
/// The length of the secret a browser shares with the sender of its push.
const AUTH_BYTES: usize = 16;
/// The most characters kept of the name a browser gives itself.
const USER_AGENT_MAX_CHARS: usize = 256;

/// The counters of a person's Inbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// Notifications not read yet.
    pub unread: i64,
    /// Approvals that wait in the person's own list of approvals.
    pub approvals_pending: i64,
    /// The highest number their Inbox has given out.
    pub seq: Seq,
}

/// The counters of the person's Inbox now.
pub async fn summary(
    conn: &mut Conn,
    principal: &Principal,
    engine: &orch_engine::Engine,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<Summary> {
    let counts = inbox::counts(conn, principal.user())
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Summary {
        unread: counts.unread,
        approvals_pending: crate::routes::approvals::waiting_for(conn, principal, now, engine)
            .await?,
        seq: counts.seq,
    })
}

/// The kinds a list asks for: one, or several separated by commas. A word that is no kind of
/// notification is refused.
pub(super) fn kinds_asked(asked: Option<&str>) -> ApiResult<Vec<String>> {
    let Some(asked) = asked else {
        return Ok(Vec::new());
    };
    asked
        .split(',')
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .map(|word| {
            serde_json::from_value::<NotificationKind>(Json::String(word.to_owned()))
                .map(|_| word.to_owned())
                .map_err(|_| {
                    ApiError::invalid("kind", &format!("{word:?} is not a kind of notification"))
                })
        })
        .collect()
}

/// The caller's notifications, newest first, of the kinds and since the moment asked for.
pub async fn list(ctx: Ctx, query: NotificationListQuery, (): ()) -> ApiResult<Page<Notification>> {
    if query.group_by == Some(NotificationGrouping::Place) {
        return super::grouping::list_by_place(ctx, query).await;
    }
    let kinds = kinds_asked(query.kind.as_deref())?;
    let since = query.since.map(Time::get);
    let page = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = i64::from(page.limit());
    let after = query
        .cursor
        .as_deref()
        .map(|cursor| {
            After::from_cursor(cursor)
                .ok_or_else(|| ApiError::invalid("cursor", "is not a cursor of this list"))
        })
        .transpose()?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let mut rows = inbox::list(
        &mut conn,
        principal.user(),
        &inbox::ListFilter {
            unread_only: query.unread.unwrap_or(false),
            kinds: &kinds,
            since,
        },
        after,
        limit + 1,
    )
    .await?;
    let more = i64::try_from(rows.len()).is_ok_and(|count| count > limit);
    rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let mut places: Vec<Id> = rows
        .iter()
        .flat_map(|row| [row.node_id, row.thread_id])
        .flatten()
        .collect();
    places.sort_unstable();
    places.dedup();
    let nodes = view::ways(&mut conn, &principal, &places, Policy::default()).await?;
    let actors = crate::realtime::user::notification_people(
        &mut conn,
        &mut rows.iter_mut().collect::<Vec<_>>(),
    )
    .await?;
    crate::realtime::user::notification_files(
        &mut conn,
        &principal,
        &mut rows.iter_mut().collect::<Vec<_>>(),
    )
    .await?;
    let next_cursor = more
        .then(|| {
            rows.last().map(|last| {
                After {
                    created_at: last.created_at,
                    id: last.id,
                }
                .to_cursor()
            })
        })
        .flatten();
    let waiting =
        super::grouping::waiting_for(&ctx.app.engine, &mut conn, &principal, &rows).await?;
    let mut items = Vec::new();
    for row in &rows {
        if orch_authz::documents::notice_readable(
            &mut conn,
            &principal,
            &row.kind,
            &row.data,
            row.chat_id,
            row.thread_id,
        )
        .await
        .map_err(ApiError::internal)?
            && let Some(mut shown) = notification_view(row, &nodes, &actors)
        {
            shown.waiting_for_me = waiting.contains(&row.id);
            items.push(shown);
        }
    }
    Ok(Page { items, next_cursor })
}

/// How many notifications wait and how far the caller's Inbox has come, for
/// the badge, with whether the caller is told outside: their pause, and their
/// ways out that are on and ready or that need attention, in the scope asked
/// for. A scope is a place the caller may read.
pub async fn summary_get(
    ctx: Ctx,
    query: NotificationSummaryGetQuery,
    (): (),
) -> ApiResult<NotificationSummaryGetReply> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    summary_for(
        &mut conn,
        &principal,
        &ctx.app.engine,
        query.node_id,
        ctx.now,
    )
    .await
}

/// The summary shared by the badge's read and the shell bootstrap.
pub(crate) async fn summary_for(
    conn: &mut Conn,
    principal: &Principal,
    engine: &orch_engine::Engine,
    node: Option<Id>,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<NotificationSummaryGetReply> {
    let counted = summary(conn, principal, engine, now).await?;
    let outside = outside_status(conn, principal, node, now).await?;
    Ok(NotificationSummaryGetReply {
        unread: counted.unread,
        approvals_pending: counted.approvals_pending,
        seq: counted.seq,
        outside,
    })
}

/// The caller's pause and their ways out, in a scope: with a place, only the ways that cover it
/// (the system's and their own, and those of the root org it lies in). A way is named when it
/// is on and ready, or when it needs attention; what it accepted is no proof that a device
/// showed anything.
pub async fn outside_status(
    conn: &mut Conn,
    principal: &Principal,
    node: Option<Id>,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<OutsideStatus> {
    let me = principal.user();
    let (scope, scope_org) = match node {
        Some(node) => {
            authorization::require(
                conn,
                principal,
                node,
                authorization::VIEW,
                Act::Use,
                Policy::default(),
            )
            .await?;
            let refs = view::refs(conn, principal, &[node], Policy::default()).await?;
            (
                refs.get(&node).cloned(),
                Some(pages::org_of_node(conn, node).await?),
            )
        }
        None => (None, None),
    };
    let prefs = inbox::prefs(conn, me).await?;
    let mut ways = Vec::new();
    for route in pages::routes_of(conn, me).await? {
        let attention = route.failure_episode.is_some();
        if !(route.enabled && route.channel_id.is_some()) && !attention {
            continue;
        }
        if let Some(org) = &scope_org {
            let route_org: Option<Id> = route
                .kept_channel_text("org_id")
                .and_then(|text| text.parse().ok());
            if route_org.is_some() && route_org != *org {
                continue;
            }
        }
        ways.push(OutsideWay {
            route_id: route.id,
            name: route
                .kept_channel_text("name")
                .unwrap_or_default()
                .to_owned(),
            kind: route
                .kept_channel_text("kind")
                .unwrap_or_default()
                .to_owned(),
            state: if attention {
                OutsideWayState::NeedsAttention
            } else {
                OutsideWayState::Ready
            },
            last_ok_at: route.last_ok_at.map(Time::new),
        });
    }
    Ok(OutsideStatus {
        scope,
        pause_until: prefs.pause_at(now).map(Time::new),
        ways,
    })
}

/// Marks notifications of the caller's Inbox read, in exactly one form: those listed, those of a
/// node or a chat, those written at or before a moment; a group up to its cut; or the undoing of
/// a group read by its ticket.
pub async fn read(
    ctx: Ctx,
    (): (),
    body: NotificationReadBody,
) -> ApiResult<NotificationReadReply> {
    let ordinary = body.ids.is_some()
        || body.node_id.is_some()
        || body.chat_id.is_some()
        || body.before.is_some();
    let group = body.group_key.is_some() || body.through_seq.is_some();
    let undo = body.undo_token.is_some();
    match (ordinary, group, undo) {
        (false, true, false) => return super::grouping::read_group(ctx, body).await,
        (false, false, true) => return super::grouping::undo_group(ctx, body).await,
        (true, false, false) => {}
        _ => {
            return Err(ApiError::invalid(
                "",
                "exactly one form is needed: ids, node_id, chat_id or before; group_key with through_seq; or undo_token",
            ));
        }
    }
    let user = ctx.user()?.id();
    let mut tx = ctx.app.db.begin().await?;
    let ids = body.ids.clone().unwrap_or_default();
    let before = inbox::counts(&mut tx, user)
        .await?
        .map_or(0, |counts| counts.unread);
    inbox::mark_read(
        &mut tx,
        user,
        &ReadFilter {
            ids: &ids,
            node: body.node_id,
            chat: body.chat_id,
            before: body.before.map(orch_types::Time::get),
        },
        ctx.now,
    )
    .await?;
    let unread = inbox::counts(&mut tx, user)
        .await?
        .map_or(0, |counts| counts.unread);
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(NotificationReadReply {
        unread,
        changed: (before - unread).max(0),
        undo_token: None,
        undo_expires_at: None,
    })
}

fn level_word(level: ChatLevel) -> String {
    serde_json::to_value(level)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn level_of(word: &str, otherwise: ChatLevel) -> ChatLevel {
    serde_json::from_value(Json::String(word.to_owned())).unwrap_or(otherwise)
}

fn prefs_view(row: &PrefsRow, now: chrono::DateTime<chrono::Utc>) -> NotificationPrefs {
    let minute = |value: Option<i32>| value.and_then(|minute| u32::try_from(minute).ok());
    NotificationPrefs {
        disabled_kinds: serde_json::from_value(row.disabled_kinds.clone()).unwrap_or_default(),
        revision: row.revision,
        // A pause whose moment has passed holds nothing, even before the minute job clears it.
        pause_until: row.pause_at(now).map(Time::new),
        approvals_always: row.approvals_always,
        chat_node_level: level_of(&row.chat_node_level, ChatLevel::Mentions),
        chat_direct_level: level_of(&row.chat_direct_level, ChatLevel::All),
        quiet_hours: match (minute(row.quiet_start_minute), minute(row.quiet_end_minute)) {
            (Some(start_minute), Some(end_minute)) => Some(QuietHours {
                start_minute,
                end_minute,
            }),
            _ => None,
        },
    }
}

/// What the caller chose about being notified.
pub async fn prefs_get(ctx: Ctx, (): (), (): ()) -> ApiResult<NotificationPrefs> {
    let mut conn = ctx.app.db.acquire().await?;
    let row = inbox::prefs(&mut conn, ctx.user()?.id()).await?;
    Ok(prefs_view(&row, ctx.now))
}

/// Changes what the caller chose about being notified; what is left out stays. A change names
/// the revision it was made from, and one made from another is refused with the choices as they
/// are. A pause is one moment in the future, and Resume clears it alone: quiet hours and every
/// route stay as they are. What a pause held is looked at again at once when it changes.
pub async fn prefs_set(
    ctx: Ctx,
    (): (),
    body: MeNotificationPrefsSetBody,
) -> ApiResult<NotificationPrefs> {
    let pause = match body.pause_until {
        None => None,
        Some(None) => Some(None),
        Some(Some(until)) => {
            if until.get() <= ctx.now {
                return Err(ApiError::invalid(
                    "pause_until",
                    "is a moment in the future; null resumes",
                ));
            }
            Some(Some(until.get()))
        }
    };
    let quiet = match body.quiet_hours {
        None => None,
        Some(None) => Some(None),
        Some(Some(hours)) => {
            for (path, minute) in [
                ("quiet_hours.start_minute", hours.start_minute),
                ("quiet_hours.end_minute", hours.end_minute),
            ] {
                if minute > QUIET_MINUTE_MAX {
                    return Err(ApiError::invalid(
                        path,
                        &format!("is a minute of the day, from 0 to {QUIET_MINUTE_MAX}"),
                    ));
                }
            }
            let start = i32::try_from(hours.start_minute).map_err(ApiError::internal)?;
            let end = i32::try_from(hours.end_minute).map_err(ApiError::internal)?;
            Some(Some((start, end)))
        }
    };
    let node_level = body.chat_node_level.map(level_word);
    let direct_level = body.chat_direct_level.map(level_word);
    let me = ctx.user()?.id();
    let disabled_kinds = body
        .disabled_kinds
        .map(serde_json::to_value)
        .transpose()
        .map_err(ApiError::internal)?;
    let mut tx = ctx.app.db.begin().await?;
    let changed = inbox::put_prefs_at(
        &mut tx,
        me,
        body.expected_revision,
        &PrefsChange {
            disabled_kinds: disabled_kinds.as_ref(),
            approvals_always: body.approvals_always,
            chat_node_level: node_level.as_deref(),
            chat_direct_level: direct_level.as_deref(),
            quiet,
            pause,
        },
    )
    .await?;
    let row = match changed {
        Ok(row) => row,
        Err(current) => {
            tx.rollback().await.map_err(orch_db::DbError::from)?;
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "your choices were changed meanwhile",
            )
            .with("current", prefs_view(&current, ctx.now)));
        }
    };
    if pause.is_some() || quiet.is_some() {
        outbox::rearm_held_of(&mut tx, me, ctx.now).await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(prefs_view(&row, ctx.now))
}

/// Bytes of a key or a secret given in base64url, with or without padding.
fn base64url(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text.trim_end_matches('=')).ok()
}

/// Subscribes a browser of the caller's to push. The route of the push channel
/// is what switches push on.
pub async fn push_create(ctx: Ctx, (): (), body: MePushSubscriptionCreateBody) -> ApiResult<()> {
    let endpoint = body.endpoint.trim();
    let host = endpoint.strip_prefix("https://").unwrap_or_default();
    if host.is_empty()
        || endpoint.len() > ENDPOINT_MAX_BYTES
        || endpoint
            .chars()
            .any(|each| each.is_whitespace() || each.is_control())
    {
        return Err(ApiError::invalid(
            "endpoint",
            "is the https address of a push service",
        ));
    }
    let key = base64url(&body.keys.p256dh);
    if !key.is_some_and(|bytes| {
        bytes.len() == P256DH_BYTES && bytes.first() == Some(&UNCOMPRESSED_POINT)
    }) {
        return Err(ApiError::invalid(
            "keys.p256dh",
            "is the browser's public key, 65 bytes in base64url",
        ));
    }
    if base64url(&body.keys.auth).is_none_or(|bytes| bytes.len() != AUTH_BYTES) {
        return Err(ApiError::invalid(
            "keys.auth",
            "is the browser's secret, 16 bytes in base64url",
        ));
    }
    let user_agent: String = ctx
        .headers
        .get(http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .chars()
        .take(USER_AGENT_MAX_CHARS)
        .collect();
    let mut conn = ctx.app.db.acquire().await?;
    people::put_push_subscription(
        &mut conn,
        &NewPushSubscription {
            id: orch_types::new_id(),
            user_id: ctx.user()?.id(),
            endpoint,
            p256dh: body.keys.p256dh.trim_end_matches('='),
            auth: body.keys.auth.trim_end_matches('='),
            user_agent: &user_agent,
            at: ctx.now,
        },
    )
    .await?;
    Ok(())
}

/// Takes a browser out of the caller's push subscriptions. A browser that is
/// not theirs is left alone, and the answer is the same.
pub async fn push_delete(ctx: Ctx, (): (), body: MePushSubscriptionDeleteBody) -> ApiResult<()> {
    let mut conn = ctx.app.db.acquire().await?;
    pages::remove_push_subscription_of(&mut conn, ctx.user()?.id(), body.endpoint.trim()).await?;
    Ok(())
}

/// The caller's own browsers that took push, without their endpoints or keys: each with a name
/// from what it said of itself and the hash a browser finds itself by.
pub async fn push_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<PushDevice>> {
    let mut conn = ctx.app.db.acquire().await?;
    Ok(pages::push_devices_of(&mut conn, ctx.user()?.id())
        .await?
        .into_iter()
        .map(|device| PushDevice {
            id: device.id,
            label: pages::device_label(&device.user_agent),
            endpoint_hash: device.endpoint_hash,
            last_ok_at: device.last_ok_at.map(Time::new),
        })
        .collect())
}

/// Takes one of the caller's own browsers out of push; their other browsers and their route stay.
/// Where it was the last one, their push route can no longer reach them and says it needs one.
pub async fn push_remove(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let device = ctx.id("id")?;
    let (mut tx, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    if !pages::remove_push_device_of(&mut tx, guard.user().id(), device).await? {
        return Err(ApiError::not_found());
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// The kinds of channel and of destination this system has, with the texts
/// each ships in the caller's language.
pub async fn notifier_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<Notifier>> {
    let language = ctx.user()?.user.locale.clone();
    Ok(super::notifiers()
        .descriptors()
        .iter()
        .map(|descriptor| super::views::notifier_view(descriptor, &language))
        .collect())
}
