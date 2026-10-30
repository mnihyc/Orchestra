//! The state behind a topic's feed: the topic, its threads, the jobs about them and its announcements, or what changed after a sequence.
//!
//! A client that lost its place on the feed of a topic asks here for what it needs to be whole
//! again. The topic is always there. Its threads are all of those the caller may view, whatever the
//! number the client holds: a thread's own state is stamped on the feed of its thread and on the
//! tree of its org, and not on the topic's, so that the topic's number cannot say which of them
//! changed, and a client takes each row by the sequence it carries. The jobs about the threads are
//! those that are under way, or from a number on, those stamped on the feed after it, over or not,
//! for the threads the caller may view. The announcements are the last twenty that went to the
//! threads, or from a number on, those after it.
//!
//! What the state consists of is [`MEMBERS`], one list, and the only place that names the members.

use crate::middleware::authorization::{self, exact};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{placed, view};
use crate::syncs::{self, Piece};
use async_trait::async_trait;
use orch_access_contract::permissions::Permission;
use orch_api_contract::nodes::{Node, NodeKind};
use orch_api_contract::threads::Thread;
use orch_api_contract::topics::{TopicSync, TopicSyncGet, TopicSyncGetQuery};
use orch_authz::{Act, Policy, nodes};
use orch_db::DbError;
use orch_db::queries::jobs::feeds as job_rows;
use orch_db::queries::threads::tools as announcement_rows;
use orch_db::queries::{feeds, hierarchy};
use orch_types::{Id, Seq};
use serde_json::json;
use std::collections::HashSet;

/// How many children of a topic are read at a time: what a screen of a topic holds many times over.
const THREADS_AT_A_TIME: i64 = 200;

/// The most jobs one request hands over from a number on: more than that changed, and the client
/// reads from the start, which holds the jobs that are under way.
pub const JOBS_MAX: i64 = 200;

/// The announcements a first load holds: the last twenty.
pub const ANNOUNCEMENTS_FIRST: i64 = 20;

/// The most announcements one request hands over from a number on.
pub const ANNOUNCEMENTS_MAX: i64 = 200;

/// What the feed of a topic consists of, in the order it is put together. Each member is named as
/// the reply names it.
pub(crate) const MEMBERS: &[&dyn TopicMember] = &[
    &TheTopic,
    &TheThreads,
    &TheJobs,
    &TheAnnouncements,
    &TheInputSchedules,
];

/// What both the HTTP reader and a socket subscriber read a topic with.
pub(crate) struct Reading<'a> {
    conn: &'a mut orch_db::Conn,
    principal: &'a orch_authz::Principal,
    engine: &'a orch_engine::Engine,
    now: chrono::DateTime<chrono::Utc>,
    subject: Id,
    seq: Seq,
    since: Option<Seq>,
    viewable_threads: Vec<Id>,
}

/// One member of the same topic projection in HTTP and on the live socket.
#[async_trait]
pub(crate) trait TopicMember: Sync {
    /// Its member name in the reply.
    fn name(&self) -> &'static str;
    /// Reads this member in the shared snapshot, with this reader's authority.
    async fn read(&self, reading: &mut Reading<'_>) -> ApiResult<Piece>;
}

/// Reads every topic member for this reader in the caller's snapshot transaction.
pub(crate) async fn read_for(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    engine: &orch_engine::Engine,
    now: chrono::DateTime<chrono::Utc>,
    topic: Id,
    seq: Seq,
    since: Option<Seq>,
) -> ApiResult<TopicSync> {
    let ahead = since.is_some_and(|since| since > seq);
    let mut reading = Reading {
        conn,
        principal,
        engine,
        now,
        subject: topic,
        seq,
        since: if ahead { Some(seq) } else { since },
        viewable_threads: Vec::new(),
    };
    let mut pieces = Vec::with_capacity(MEMBERS.len());
    for member in MEMBERS {
        pieces.push((member.name(), member.read(&mut reading).await?));
    }
    syncs::assemble(pieces, seq, since, !ahead)
}

/// Binds this module's requests.
pub(super) fn mount(registry: &mut Registry) {
    registry.on(TopicSyncGet, sync);
}

fn value<T: serde::Serialize>(of: &T) -> ApiResult<serde_json::Value> {
    serde_json::to_value(of).map_err(ApiError::internal)
}

/// The topic, as the caller sees it.
struct TheTopic;

#[async_trait]
impl TopicMember for TheTopic {
    fn name(&self) -> &'static str {
        "topic"
    }

    async fn read(&self, reading: &mut Reading<'_>) -> ApiResult<Piece> {
        let found = view::one(
            reading.conn,
            reading.principal,
            reading.subject,
            reading.now,
            Policy::default(),
        )
        .await?;
        match found {
            Some(Node::Topic(topic)) => Ok(Piece::whole(value(&topic)?)),
            _ => Err(ApiError::not_found()),
        }
    }
}

/// Every thread of the topic that the caller may view.
struct TheThreads;

#[async_trait]
impl TopicMember for TheThreads {
    fn name(&self) -> &'static str {
        "threads"
    }

    async fn read(&self, reading: &mut Reading<'_>) -> ApiResult<Piece> {
        let mut threads: Vec<Thread> = Vec::new();
        let mut after: Option<Id> = None;
        loop {
            let rows =
                hierarchy::children_after(reading.conn, reading.subject, after, THREADS_AT_A_TIME)
                    .await?;
            let Some(last) = rows.last().map(|row| row.id) else {
                break;
            };
            let ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
            let seen = view::visible(
                reading.conn,
                reading.principal,
                &ids,
                reading.now,
                Policy::default(),
            )
            .await?;
            threads.extend(seen.into_iter().filter_map(|node| match node {
                Node::Thread(thread) => Some(thread),
                _ => None,
            }));
            if i64::try_from(rows.len()).unwrap_or(i64::MAX) < THREADS_AT_A_TIME {
                break;
            }
            after = Some(last);
        }
        reading.viewable_threads = threads.iter().map(|thread| thread.base.id).collect();
        Ok(Piece::whole(value(&threads)?))
    }
}

/// The jobs about the threads of the topic, for the threads the caller may view.
struct TheInputSchedules;

#[async_trait]
impl TopicMember for TheInputSchedules {
    fn name(&self) -> &'static str {
        "input_schedules"
    }
    async fn read(&self, reading: &mut Reading<'_>) -> ApiResult<Piece> {
        let rows = crate::input_schedules::for_topic(
            reading.conn,
            reading.principal,
            reading.subject,
            reading.since,
        )
        .await?;
        Ok(Piece::whole(value(&rows)?))
    }
}

/// The jobs about the threads of the topic, for the threads the caller may view.
struct TheJobs;

#[async_trait]
impl TopicMember for TheJobs {
    fn name(&self) -> &'static str {
        "jobs"
    }

    async fn read(&self, reading: &mut Reading<'_>) -> ApiResult<Piece> {
        let rows = match reading.since {
            None => job_rows::open_of_topic(reading.conn, reading.subject).await?,
            Some(since) => {
                let found = job_rows::of_threads_between(
                    reading.conn,
                    &reading.viewable_threads,
                    since,
                    reading.seq,
                    JOBS_MAX + 1,
                )
                .await?;
                if i64::try_from(found.len()).unwrap_or(i64::MAX) > JOBS_MAX {
                    return Ok(Piece::cut(json!([])));
                }
                found
            }
        };
        let named: Vec<Id> = rows
            .iter()
            .filter_map(|row| row.node_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let viewable: HashSet<Id> = if named.is_empty() {
            HashSet::new()
        } else {
            nodes::holding(
                reading.conn,
                reading.principal,
                &named,
                exact(Permission::ThreadView),
                Act::Use,
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|listed| listed.id)
            .collect()
        };
        let mut jobs = Vec::new();
        for row in rows
            .iter()
            .filter(|row| row.node_id.is_some_and(|thread| viewable.contains(&thread)))
        {
            jobs.push(crate::routes::environments::jobs::view(reading.conn, row).await?);
        }
        Ok(Piece::whole(value(&jobs)?))
    }
}

/// The announcements that went to the threads of the topic.
struct TheAnnouncements;

#[async_trait]
impl TopicMember for TheAnnouncements {
    fn name(&self) -> &'static str {
        "announcements"
    }

    async fn read(&self, reading: &mut Reading<'_>) -> ApiResult<Piece> {
        let rows = match reading.since {
            None => {
                announcement_rows::announcements_of_topic(
                    reading.conn,
                    reading.subject,
                    None,
                    false,
                    None,
                    ANNOUNCEMENTS_FIRST,
                )
                .await?
            }
            Some(since) => {
                // Newest first: one more than the most there may be, and if the oldest of them is
                // still above the number the client holds, there are more than that to tell.
                let found = announcement_rows::announcements_of_topic(
                    reading.conn,
                    reading.subject,
                    None,
                    false,
                    None,
                    ANNOUNCEMENTS_MAX + 1,
                )
                .await?;
                let crowded = i64::try_from(found.len()).unwrap_or(i64::MAX) > ANNOUNCEMENTS_MAX
                    && found.last().is_some_and(|oldest| oldest.topic_seq > since);
                if crowded {
                    return Ok(Piece::cut(json!([])));
                }
                found
                    .into_iter()
                    .filter(|row| row.topic_seq > since && row.topic_seq <= reading.seq)
                    .collect()
            }
        };
        let mut views = reading
            .engine
            .announcement_views_on(reading.conn, &rows)
            .await
            .map_err(ApiError::internal)?;
        let sources: Vec<Id> = rows.iter().filter_map(|row| row.from_thread_id).collect();
        let named =
            view::refs(reading.conn, reading.principal, &sources, Policy::default()).await?;
        for announcement in &mut views {
            if let orch_api_contract::threads::AnnouncementFrom::Thread(thread) =
                &mut announcement.from
            {
                thread.name = named
                    .get(&thread.id)
                    .and_then(|reference| reference.name.clone())
                    .unwrap_or_else(|| "a private thread".to_owned());
            }
        }
        Ok(Piece::whole(value(&views)?))
    }
}

/// Brings a client up to date on a topic.
async fn sync(ctx: Ctx, query: TopicSyncGetQuery, (): ()) -> ApiResult<TopicSync> {
    let topic = ctx.id("id")?;
    if query.since.is_some_and(|since| since < 0) {
        return Err(ApiError::invalid(
            "since",
            "is a number of the feed, from 0",
        ));
    }
    let mut tx = syncs::begin(&ctx).await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let (_, kind) = placed(&mut tx, topic).await?;
    if kind != NodeKind::Topic {
        return Err(ApiError::not_found());
    }
    let seq = feeds::topic_seqs(&mut tx, &[topic])
        .await?
        .pop()
        .map(|(_, seq)| seq)
        .ok_or_else(ApiError::not_found)?;
    let reply = read_for(
        &mut tx,
        &principal,
        &ctx.app.engine,
        ctx.now,
        topic,
        seq,
        query.since,
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(reply)
}
