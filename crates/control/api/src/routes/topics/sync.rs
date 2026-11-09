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

use crate::key_operations::{admission::Prepared, targets};
use crate::middleware::authorization::{self, exact};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationFuture, Registry};
use crate::routes::nodes::{placed, view};
use crate::syncs::{self, Piece};
use async_trait::async_trait;
use orch_access_contract::key_scope::{
    KeyIdentityKind, KeyObjectRef, KeyResolvedTarget, KeyTargetKind,
};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::nodes::{Node, NodeKind};
use orch_api_contract::routes::Endpoint;
use orch_api_contract::threads::Thread;
use orch_api_contract::topics::{TopicSync, TopicSyncGet, TopicSyncGetQuery};
use orch_authz::{Act, Caller, Policy, nodes};
use orch_db::DbError;
use orch_db::queries::jobs::feeds as job_rows;
use orch_db::queries::threads::tools as announcement_rows;
use orch_db::queries::{feeds, hierarchy};
use orch_types::{Id, Seq};
use serde_json::json;
use std::collections::{HashMap, HashSet};

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

// These slots follow the TopicSyncGet declaration. Each collection records only rows
// actually returned by the ordinary projection, under that same operation selection.
const THREAD_ROWS: usize = 1;
const JOB_ROWS: usize = 2;
const SCHEDULE_ROWS: usize = 3;

pub(crate) async fn root_plan(conn: &mut orch_db::Conn, topic: Id) -> ApiResult<Prepared> {
    let target = targets::node(conn, topic).await?;
    if target.object.kind != KeyTargetKind::Object(ObjectKind::Topic) {
        return Err(ApiError::not_found());
    }
    let mut plan = Prepared::new(TopicSyncGet::ROUTE, TopicSyncGet::ROUTE.name)?;
    plan.bind(0, target.object.kind, &[target])?;
    Ok(plan)
}

struct ReadScope {
    plan: Prepared,
    caller: Caller,
    // Request-local current facts from this same snapshot, never a global holder cache.
    nodes: HashMap<Id, KeyResolvedTarget>,
    returned: [Vec<KeyResolvedTarget>; 3],
}

impl ReadScope {
    async fn new(conn: &mut orch_db::Conn, caller: &Caller, topic: Id) -> ApiResult<Self> {
        Ok(Self {
            plan: root_plan(conn, topic).await?,
            caller: caller.clone(),
            nodes: HashMap::new(),
            returned: std::array::from_fn(|_| Vec::new()),
        })
    }

    async fn load_nodes(&mut self, conn: &mut orch_db::Conn, ids: &[Id]) -> ApiResult<()> {
        let missing: Vec<_> = ids
            .iter()
            .copied()
            .filter(|id| !self.nodes.contains_key(id))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        let graph = match orch_authz::engine::Graph::load(conn, &missing).await {
            Ok(graph) => graph,
            Err(error) => return Err(authorization::refusals::problem(conn, error).await),
        };
        for id in missing {
            let site = graph.site_of(id).ok_or_else(ApiError::not_found)?;
            self.nodes.insert(id, targets::from_site(&graph, &site)?);
        }
        Ok(())
    }

    fn permits(&self, slot: usize, target: &KeyResolvedTarget) -> ApiResult<bool> {
        self.plan.permits_binding(
            &self.caller,
            slot,
            target.object.kind,
            std::slice::from_ref(target),
        )
    }

    fn include(&mut self, slot: usize, target: KeyResolvedTarget) {
        self.returned[slot - THREAD_ROWS].push(target);
    }

    fn finish(mut self) -> ApiResult<Prepared> {
        for (index, rows) in self.returned.into_iter().enumerate() {
            let slot = index + THREAD_ROWS;
            if rows.is_empty() {
                self.plan.bind_empty(slot)?;
            } else {
                self.plan.bind(slot, rows[0].object.kind, &rows)?;
            }
        }
        Ok(self.plan)
    }
}

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
    scope: ReadScope,
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
    let scope = ReadScope::new(conn, principal.caller(), topic).await?;
    let (reply, plan) = read_scoped(
        conn,
        principal,
        engine,
        now,
        TopicWindow { topic, seq, since },
        scope,
    )
    .await?;
    // The complete footprint belongs to this current read, not the subscription's old admission.
    plan.admit(principal.caller())?;
    Ok(reply)
}

struct TopicWindow {
    topic: Id,
    seq: Seq,
    since: Option<Seq>,
}

async fn read_scoped(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    engine: &orch_engine::Engine,
    now: chrono::DateTime<chrono::Utc>,
    window: TopicWindow,
    scope: ReadScope,
) -> ApiResult<(TopicSync, Prepared)> {
    let TopicWindow { topic, seq, since } = window;
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
        scope,
    };
    let mut pieces = Vec::with_capacity(MEMBERS.len());
    for member in MEMBERS {
        pieces.push((member.name(), member.read(&mut reading).await?));
    }
    let reply = syncs::assemble(pieces, seq, since, !ahead)?;
    Ok((reply, reading.scope.finish()?))
}

/// Binds this module's requests.
pub(super) fn mount(registry: &mut Registry) {
    registry.on_with_operation(TopicSyncGet, operation, sync);
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
            let viewable = nodes::holding(
                reading.conn,
                reading.principal,
                &ids,
                exact(Permission::ThreadView),
                Act::Use,
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?;
            let visible_ids: Vec<_> = viewable.into_iter().map(|row| row.id).collect();
            // Job visibility remains ordinary ThreadView even when this key omits Thread rows.
            reading.viewable_threads.extend(visible_ids.iter().copied());
            reading.scope.load_nodes(reading.conn, &visible_ids).await?;
            let mut allowed = Vec::new();
            for id in visible_ids {
                let target = reading
                    .scope
                    .nodes
                    .get(&id)
                    .ok_or_else(ApiError::not_found)?;
                if reading.scope.permits(THREAD_ROWS, target)? {
                    allowed.push(id);
                }
            }
            let seen = view::visible(
                reading.conn,
                reading.principal,
                &allowed,
                reading.now,
                Policy::default(),
            )
            .await?;
            for node in seen {
                if let Node::Thread(thread) = node {
                    let target = reading
                        .scope
                        .nodes
                        .get(&thread.base.id)
                        .cloned()
                        .ok_or_else(ApiError::not_found)?;
                    reading.scope.include(THREAD_ROWS, target);
                    threads.push(thread);
                }
            }
            if i64::try_from(rows.len()).unwrap_or(i64::MAX) < THREADS_AT_A_TIME {
                break;
            }
            after = Some(last);
        }
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
        let mut allowed = Vec::new();
        reading
            .scope
            .load_nodes(reading.conn, &[reading.subject])
            .await?;
        for row in rows {
            // for_topic selected this stored topic_id; do not use the DTO's optional redacted name.
            let mut target = reading
                .scope
                .nodes
                .get(&reading.subject)
                .cloned()
                .ok_or_else(ApiError::not_found)?;
            target.object = KeyObjectRef {
                kind: KeyTargetKind::Identity(KeyIdentityKind::Schedule),
                id: Some(row.id),
            };
            if reading.scope.permits(SCHEDULE_ROWS, &target)? {
                reading.scope.include(SCHEDULE_ROWS, target);
                allowed.push(row);
            }
        }
        Ok(Piece::whole(value(&allowed)?))
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
        let named: Vec<_> = viewable.iter().copied().collect();
        reading.scope.load_nodes(reading.conn, &named).await?;
        for row in rows
            .iter()
            .filter(|row| row.node_id.is_some_and(|thread| viewable.contains(&thread)))
        {
            let thread = row.node_id.ok_or_else(ApiError::not_found)?;
            let parent = reading
                .scope
                .nodes
                .get(&thread)
                .ok_or_else(ApiError::not_found)?;
            let target = crate::routes::environments::jobs::subject::identity(row, Some(parent));
            if reading.scope.permits(JOB_ROWS, &target)? {
                jobs.push(crate::routes::environments::jobs::view(reading.conn, row).await?);
                reading.scope.include(JOB_ROWS, target);
            }
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

/// The ordinary projection and its completed footprint from one read snapshot.
/// Retaining both lets HTTP finish disclosure without running the member readers twice.
struct PreparedRead {
    reply: TopicSync,
    plan: Prepared,
}

fn operation<'a>(
    ctx: &'a Ctx,
    conn: &'a mut orch_db::Conn,
    caller: &'a Caller,
    query: TopicSyncGetQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let topic = ctx.id("id")?;
        if query.since.is_some_and(|since| since < 0) {
            return Err(ApiError::invalid(
                "since",
                "is a number of the feed, from 0",
            ));
        }
        let scope = ReadScope::new(conn, caller, topic).await?;
        let principal = scope.plan.preparation_principal(ctx, conn, caller).await?;
        let (_, kind) = placed(conn, topic).await?;
        if kind != NodeKind::Topic {
            return Err(ApiError::not_found());
        }
        let seq = feeds::topic_seqs(conn, &[topic])
            .await?
            .pop()
            .map(|(_, seq)| seq)
            .ok_or_else(ApiError::not_found)?;
        let (reply, plan) = read_scoped(
            conn,
            &principal,
            &ctx.app.engine,
            ctx.now,
            TopicWindow {
                topic,
                seq,
                since: query.since,
            },
            scope,
        )
        .await?;
        Ok(plan.clone().with_data(PreparedRead { reply, plan }))
    })
}

/// Brings a client up to date on a topic, using the producer freshly rerun on this snapshot.
async fn sync(ctx: Ctx, _query: TopicSyncGetQuery, (): ()) -> ApiResult<TopicSync> {
    let mut tx = syncs::begin(&ctx).await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<PreparedRead>()?;
    // Preparation cannot disclose rows whose targets are still unknown. The same
    // snapshot now holds their finished set; use the shared response checkpoint to
    // recheck the current credential and complete Pending without widening Denied.
    let reply = prepared
        .plan
        .clone()
        .read_response(&ctx, &mut tx, principal.caller(), |_, _| {
            let reply = prepared.reply.clone();
            Box::pin(async move { Ok(reply) })
        })
        .await?
        .ok_or_else(|| {
            ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "the key does not allow this topic response",
            )
        })?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(reply)
}
