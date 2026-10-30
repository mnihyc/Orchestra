//! The Inbox by place: the same notifications grouped under the nearest place the caller may read, what they can act on first, paged under a fixed cut; a group marked read up to its cut, and undone by a sealed ticket.
//!
//! **Groups.** A notification is grouped under the nearest project on its path that the caller
//! may read now, else the nearest org they may read, else the place it is about where they may
//! read that; what is about the person alone (no place, no chat) is under Other. A place the
//! caller may not read is never named and never counted. Groups with something the caller can
//! act on now come first, then the group with the newest row, then the group's key; within a
//! group what waits for the caller comes first, then the newest. A list is paged under a cut of
//! the caller's Inbox sequence, given in the cursor, so that rows that arrive while it is paged
//! do not move it; counts are those of the whole group under the cut and the filters, never sums
//! of pages. A list asked for again after access or a decision changed starts again.
//!
//! **Reading a group.** The group key and its cut name what is marked: the rows that are unread,
//! at or below the cut, and in that group as the caller may read them now. They take one batch,
//! and the answer carries a ticket that undoes exactly that batch, sealed with the system's key
//! for this purpose and this person, and good for as long as an idempotency key replays an
//! answer. An ordinary read of a row, or its renewal, takes it out of the batch: an Undo then
//! leaves it as it is. Undo changes no thread's or chat's read position, no subscription, no
//! request and no decision.

use super::master_key;
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::realtime::user::notification_view;
use crate::registry::Ctx;
use crate::routes::nodes::view;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use orch_api_contract::common::{ErrorCode, Page, PageQuery};
use orch_api_contract::nodes::NodeRef;
use orch_api_contract::notifications::{
    Notification, NotificationGroup, NotificationListQuery, NotificationReadBody,
    NotificationReadReply,
};
use orch_authz::{Policy, Principal};
use orch_db::Conn;
use orch_db::queries::feeds::InboxRow;
use orch_db::queries::notifications::inbox::{self, After, ListFilter};
use orch_db::queries::notifications::{pages, people};
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Read source rows in the API's maximum page size; every page is visited so older groups stay reachable.
const GROUP_SCAN_PAGE: i64 = orch_api_contract::common::MAX_LIMIT as i64;

/// The key of the group of what is about the person alone.
const OTHER: &str = "other";

/// What an undo ticket is sealed for.
const UNDO_PURPOSE: &str = "notification_read_undo";

/// What a ticket that undoes a group read carries, sealed: the batch, whose it is, and until
/// when it holds. No notification text.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Ticket {
    /// The batch the read gave the rows.
    b: Id,
    /// Whose read it was.
    u: Id,
    /// Until when it holds, in seconds since the epoch.
    e: i64,
}

/// Where a list by place has come to: the cut of the caller's sequence it is paged under, and how
/// many rows came before.
fn cursor_of(cut: i64, offset: usize, state: &str) -> String {
    format!("{cut}.{offset}.{state}")
}

fn read_cursor(text: &str) -> Option<(i64, usize, &str)> {
    let mut parts = text.split('.');
    let cut = parts.next()?.parse().ok()?;
    let offset = parts.next()?.parse().ok()?;
    let state = parts.next()?;
    if parts.next().is_some() || cut < 0 || state.is_empty() {
        return None;
    }
    Some((cut, offset, state))
}

async fn source_rows(
    conn: &mut Conn,
    user: Id,
    filter: &ListFilter<'_>,
    cut: i64,
) -> ApiResult<Vec<InboxRow>> {
    let mut rows = Vec::new();
    let mut after = None;
    loop {
        let page = inbox::list_within_cut(conn, user, filter, cut, after, GROUP_SCAN_PAGE).await?;
        let more = i64::try_from(page.len()).is_ok_and(|count| count == GROUP_SCAN_PAGE);
        after = page.last().map(|row| After {
            created_at: row.created_at,
            id: row.id,
        });
        rows.extend(page);
        if !more {
            return Ok(rows);
        }
    }
}

/// The group of each row, as the caller may read the places now: its key and its place.
async fn groups_of(
    conn: &mut Conn,
    principal: &Principal,
    rows: &[InboxRow],
) -> ApiResult<HashMap<Id, (String, Option<NodeRef>)>> {
    let node_ids: Vec<Id> = rows.iter().filter_map(|row| row.node_id).collect();
    let (paths, kinds) = pages::node_paths(conn, &node_ids).await?;
    let paths: HashMap<Id, Vec<Id>> = paths.into_iter().map(|each| (each.id, each.path)).collect();
    let mut chat_orgs: HashMap<Id, Id> = HashMap::new();
    for chat in rows
        .iter()
        .filter(|row| row.node_id.is_none())
        .filter_map(|row| row.chat_id)
    {
        if let std::collections::hash_map::Entry::Vacant(slot) = chat_orgs.entry(chat)
            && let Some((org, _)) = people::org_of_chat(conn, chat).await?
        {
            slot.insert(org);
        }
    }
    let mut asked: Vec<Id> = paths.values().flatten().copied().collect();
    asked.extend(chat_orgs.values().copied());
    asked.sort_unstable();
    asked.dedup();
    let refs = view::refs(conn, principal, &asked, Policy::default()).await?;
    let readable = |id: &Id| refs.get(id).filter(|node| !node.redacted).cloned();
    let mut groups = HashMap::new();
    for row in rows {
        let group = match (row.node_id, row.chat_id) {
            (Some(node), _) => {
                let path = paths.get(&node).cloned().unwrap_or_default();
                let nearest = |kind: &str| {
                    path.iter().rev().find(|id| {
                        kinds.get(*id).is_some_and(|known| known == kind) && readable(id).is_some()
                    })
                };
                if let Some(project) = nearest("project") {
                    (format!("project:{project}"), readable(project))
                } else if let Some(org) = nearest("org") {
                    (format!("org:{org}"), readable(org))
                } else if let Some(place) = readable(&node) {
                    (format!("place:{node}"), Some(place))
                } else {
                    continue;
                }
            }
            (None, Some(chat)) => match chat_orgs
                .get(&chat)
                .and_then(|org| readable(org).map(|place| (*org, place)))
            {
                Some((org, place)) => (format!("org:{org}"), Some(place)),
                None => (OTHER.to_owned(), None),
            },
            (None, None) => (OTHER.to_owned(), None),
        };
        groups.insert(row.id, group);
    }
    Ok(groups)
}

/// The rows that ask for something the caller can do now: a request of an agent or for a
/// capability, access or port request that still waits and that this person can decide now.
pub(crate) async fn waiting_for(
    engine: &orch_engine::Engine,
    conn: &mut Conn,
    principal: &Principal,
    rows: &[InboxRow],
) -> ApiResult<HashSet<Id>> {
    let approvals: Vec<Id> = rows
        .iter()
        .filter(|row| row.read_at.is_none())
        .filter_map(|row| row.approval_id)
        .collect();
    if approvals.is_empty() {
        return Ok(HashSet::new());
    }
    let pending = orch_db::queries::approvals::pending_among(conn, &approvals).await?;
    let decidable: HashSet<Id> =
        crate::routes::approvals::views(conn, principal, &pending, principal.now().get(), engine)
            .await?
            .into_iter()
            .filter(|approval| approval.can_decide)
            .map(|approval| approval.id)
            .collect();
    Ok(rows
        .iter()
        .filter(|row| row.read_at.is_none())
        .filter(|row| {
            row.approval_id
                .is_some_and(|approval| decidable.contains(&approval))
        })
        .map(|row| row.id)
        .collect())
}

/// One group as a page shows it.
struct Shown {
    key: String,
    place: Option<NodeRef>,
    rows: Vec<InboxRow>,
    unread: i64,
    waiting: i64,
    through_seq: i64,
    newest: DateTime<Utc>,
}

/// The caller's notifications by place, what they can act on first, under a fixed cut.
pub async fn list_by_place(
    ctx: Ctx,
    query: NotificationListQuery,
) -> ApiResult<Page<Notification>> {
    let kinds = super::inbox::kinds_asked(query.kind.as_deref())?;
    let since = query.since.map(Time::get);
    let limit = usize::try_from(
        PageQuery {
            limit: query.limit,
            cursor: query.cursor.clone(),
        }
        .limit(),
    )
    .unwrap_or(usize::MAX);
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let me = principal.user();
    let (cut, offset, previous_state) = match query.cursor.as_deref() {
        Some(text) => read_cursor(text)
            .map(|(cut, offset, state)| (cut, offset, Some(state)))
            .ok_or_else(|| ApiError::invalid("cursor", "is not a cursor of this list"))?,
        None => (
            inbox::counts(&mut conn, me)
                .await?
                .map_or(0, |counts| counts.seq),
            0,
            None,
        ),
    };
    let filter = ListFilter {
        unread_only: query.unread.unwrap_or(false),
        kinds: &kinds,
        since,
    };
    let mut rows = Vec::new();
    for row in source_rows(&mut conn, me, &filter, cut).await? {
        if orch_authz::documents::notice_readable(&mut conn, &principal, &row.kind, &row.data)
            .await
            .map_err(ApiError::internal)?
        {
            rows.push(row);
        }
    }
    let groups = groups_of(&mut conn, &principal, &rows).await?;
    let waiting = waiting_for(&ctx.app.engine, &mut conn, &principal, &rows).await?;
    let mut by_key: HashMap<String, Shown> = HashMap::new();
    for row in rows {
        let Some((key, place)) = groups.get(&row.id).cloned() else {
            continue;
        };
        let shown = by_key.entry(key.clone()).or_insert_with(|| Shown {
            key,
            place,
            rows: Vec::new(),
            unread: 0,
            waiting: 0,
            through_seq: 0,
            newest: row.created_at,
        });
        shown.unread += i64::from(row.read_at.is_none());
        shown.waiting += i64::from(waiting.contains(&row.id));
        shown.through_seq = shown.through_seq.max(row.inbox_seq);
        shown.newest = shown.newest.max(row.created_at);
        shown.rows.push(row);
    }
    let mut ordered: Vec<Shown> = by_key.into_values().collect();
    ordered.sort_by(|a, b| {
        (b.waiting > 0)
            .cmp(&(a.waiting > 0))
            .then(b.newest.cmp(&a.newest))
            .then(a.key.cmp(&b.key))
    });
    let mut flat: Vec<(InboxRow, NotificationGroup, bool)> = Vec::new();
    for mut shown in ordered {
        shown.rows.sort_by(|a, b| {
            waiting
                .contains(&b.id)
                .cmp(&waiting.contains(&a.id))
                .then(b.created_at.cmp(&a.created_at))
                .then(b.id.cmp(&a.id))
        });
        let group = NotificationGroup {
            key: shown.key.clone(),
            place: shown.place.clone(),
            unread: shown.unread,
            waiting_for_me: shown.waiting,
            through_seq: shown.through_seq,
        };
        for row in shown.rows {
            let mine = waiting.contains(&row.id);
            flat.push((row, group.clone(), mine));
        }
    }
    let total = flat.len();
    // A new row above the cut leaves this fingerprint alone. Changes to access, decisions,
    // grouping or existing rows invalidate offsets instead of silently skipping or repeating rows.
    let state_rows: Vec<_> = flat
        .iter()
        .map(|(row, group, waiting)| {
            (
                row.id,
                row.inbox_seq,
                row.read_at,
                &group.key,
                &group.place,
                waiting,
            )
        })
        .collect();
    let state = orch_types::Hash::of(&serde_json::to_vec(&state_rows).map_err(ApiError::internal)?)
        .to_string();
    if previous_state.is_some_and(|previous| previous != state) {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the grouped Inbox changed; start its list again",
        )
        .with("resync", true));
    }
    let mut page: Vec<_> = flat.into_iter().skip(offset).take(limit).collect();
    let mut places: Vec<Id> = page
        .iter()
        .flat_map(|(row, _, _)| [row.node_id, row.thread_id])
        .flatten()
        .collect();
    places.sort_unstable();
    places.dedup();
    let nodes = view::ways(&mut conn, &principal, &places, Policy::default()).await?;
    let actors = crate::realtime::user::notification_people(
        &mut conn,
        &mut page.iter_mut().map(|(row, _, _)| row).collect::<Vec<_>>(),
    )
    .await?;
    crate::realtime::user::notification_files(
        &mut conn,
        &principal,
        &mut page.iter_mut().map(|(row, _, _)| row).collect::<Vec<_>>(),
    )
    .await?;
    let mut items = Vec::with_capacity(page.len());
    for (row, group, mine) in page {
        if let Some(mut shown) = notification_view(&row, &nodes, &actors) {
            shown.group = Some(group);
            shown.waiting_for_me = mine;
            items.push(shown);
        }
    }
    let next = offset.saturating_add(limit);
    Ok(Page {
        items,
        next_cursor: (next < total).then(|| cursor_of(cut, next, &state)),
    })
}

/// Marks a group read up to its cut: only the rows that are unread, at or below the cut and in
/// that group as the caller may read them now. The answer carries the ticket that undoes it.
pub async fn read_group(ctx: Ctx, body: NotificationReadBody) -> ApiResult<NotificationReadReply> {
    let (Some(key), Some(through)) = (body.group_key.clone(), body.through_seq) else {
        return Err(ApiError::invalid(
            "group_key",
            "a group is read with its key and its cut, through_seq",
        ));
    };
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let me = principal.user();
    // Announcements and renewals allocate their sequence on this row too. Holding it keeps
    // an arrival after the cut from being renewed between selection and the batch update.
    inbox::lock_sequence(&mut tx, me).await?;
    let filter = ListFilter {
        unread_only: true,
        kinds: &[],
        since: None,
    };
    let mut rows = Vec::new();
    for row in source_rows(&mut tx, me, &filter, through).await? {
        if orch_authz::documents::notice_readable(&mut tx, &principal, &row.kind, &row.data)
            .await
            .map_err(ApiError::internal)?
        {
            rows.push(row);
        }
    }
    let groups = groups_of(&mut tx, &principal, &rows).await?;
    let ids: Vec<Id> = rows
        .iter()
        .filter(|row| groups.get(&row.id).is_some_and(|(group, _)| *group == key))
        .map(|row| row.id)
        .collect();
    let batch = orch_types::new_id();
    let changed = if ids.is_empty() {
        0
    } else {
        inbox::mark_batch_read(&mut tx, me, &ids, batch, ctx.now).await?
    };
    let unread = inbox::counts(&mut tx, me)
        .await?
        .map_or(0, |counts| counts.unread);
    tx.commit().await.map_err(orch_db::DbError::from)?;
    if changed == 0 {
        return Ok(NotificationReadReply {
            unread,
            changed,
            undo_token: None,
            undo_expires_at: None,
        });
    }
    let hours = i64::try_from(ctx.app.config.api.idempotency_ttl_hours).unwrap_or(i64::MAX);
    let until = ctx.now + Duration::hours(hours);
    let ticket = seal_ticket(
        me,
        &Ticket {
            b: batch,
            u: me,
            e: until.timestamp(),
        },
    )?;
    Ok(NotificationReadReply {
        unread,
        changed,
        undo_token: Some(ticket),
        undo_expires_at: Some(Time::new(until)),
    })
}

/// Undoes a group read by its ticket: the rows of the batch that still carry it and that the
/// caller may still read are unread again. A second Undo changes nothing.
pub async fn undo_group(ctx: Ctx, body: NotificationReadBody) -> ApiResult<NotificationReadReply> {
    let Some(text) = body.undo_token.as_deref() else {
        return Err(ApiError::invalid("undo_token", "is needed to undo a read"));
    };
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let me = principal.user();
    let ticket = open_ticket(me, text)
        .ok_or_else(|| ApiError::invalid("undo_token", "is not a ticket of yours"))?;
    if ticket.u != me {
        return Err(ApiError::invalid("undo_token", "is not a ticket of yours"));
    }
    if ticket.e <= ctx.now.timestamp() {
        return Err(ApiError::new(
            ErrorCode::Gone,
            "the time to undo this has passed",
        ));
    }
    let carried = inbox::rows_of_batch(&mut tx, me, ticket.b).await?;
    let groups = groups_of(&mut tx, &principal, &carried).await?;
    let mut readable = Vec::new();
    for row in &carried {
        if groups.contains_key(&row.id)
            && orch_authz::documents::notice_readable(&mut tx, &principal, &row.kind, &row.data)
                .await
                .map_err(ApiError::internal)?
        {
            readable.push(row.id);
        }
    }
    let changed = if readable.is_empty() {
        0
    } else {
        inbox::undo_batch(&mut tx, me, ticket.b, &readable).await?
    };
    let unread = inbox::counts(&mut tx, me)
        .await?
        .map_or(0, |counts| counts.unread);
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(NotificationReadReply {
        unread,
        changed,
        undo_token: None,
        undo_expires_at: None,
    })
}

/// What a ticket is bound to: its purpose and its person.
fn binding(user: Id) -> Vec<u8> {
    format!("{UNDO_PURPOSE}/{user}").into_bytes()
}

fn seal_ticket(user: Id, ticket: &Ticket) -> ApiResult<String> {
    let plain = serde_json::to_vec(ticket).map_err(ApiError::internal)?;
    let sealed = master_key()?
        .seal(&plain, &binding(user))
        .map_err(|_| ApiError::internal("the ticket could not be sealed"))?;
    Ok(URL_SAFE_NO_PAD.encode(sealed))
}

fn open_ticket(user: Id, text: &str) -> Option<Ticket> {
    let sealed = URL_SAFE_NO_PAD.decode(text.trim()).ok()?;
    let plain = master_key().ok()?.open(&sealed, &binding(user)).ok()?;
    let ticket: Json = serde_json::from_slice(&plain).ok()?;
    serde_json::from_value(ticket).ok()
}
