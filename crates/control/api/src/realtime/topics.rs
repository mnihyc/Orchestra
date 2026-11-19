//! The topic feed: the topic as the caller sees it, by the topic's own sequence.
//!
//! The projection is the same member list as the HTTP sync, read in a snapshot for each reader.
//! Threads carry their own sequence; jobs and announcements resume from the topic cursor. If
//! any member cannot fit its changed rows, the subscriber is asked to load the state again.
//!
//! Who may follow it is the node's View permission.

use crate::realtime::subscriptions::{Access, may_view, principal_of};
use crate::realtime::sync::{Frame, Head, Rows};
use crate::realtime::{Fail, Inner};
use orch_db::queries::feeds;
use orch_realtime_contract::feeds::{SyncData, TopicData};
use orch_realtime_contract::sync::FeedHead;
use orch_types::{Id, Seq};

/// The topic itself is one row; each member enforces its own changed-row limit while reading.
const NO_LIMIT: i64 = i64::MAX;

/// Whether `user` may follow the topic.
pub(crate) async fn decide(inner: &Inner, user: Id, topic: Id) -> Result<Access, Fail> {
    let mut conn = inner.db.acquire().await?;
    if feeds::topic_seqs(&mut conn, &[topic]).await?.is_empty() {
        return Ok(Access::NotFound);
    }
    if !feeds::gone_nodes(&mut conn, &[topic]).await?.is_empty() {
        return Ok(Access::Gone);
    }
    let Some(principal) = principal_of(inner, &mut conn, user).await? else {
        return Ok(Access::Forbidden);
    };
    may_view(&mut conn, &principal, topic).await
}

/// Where the topic's feed stands, and how many rows lie above `since`.
pub(crate) async fn head(
    inner: &Inner,
    topic: Id,
    since: Option<Seq>,
) -> Result<Option<Head>, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some((_, seq)) = feeds::topic_seqs(&mut conn, &[topic])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    Ok(Some(Head {
        feed: FeedHead {
            seq,
            purged_seq: 0,
            incremental_limit: NO_LIMIT,
        },
        rows_after: i64::from(since.is_some_and(|since| since < seq)),
    }))
}

/// The topic, when its number lies in `(after, upto]`.
pub(crate) async fn read(
    inner: &Inner,
    person: Id,
    topic: Id,
    after: Seq,
    _upto: Seq,
) -> Result<Rows, Fail> {
    let mut conn = feeds::begin_reading(&inner.db).await?;
    let Some(principal) = principal_of(inner, &mut conn, person).await? else {
        return Ok(Rows::Nothing);
    };
    let Some((_, seq)) = feeds::topic_seqs(&mut conn, &[topic]).await?.pop() else {
        return Ok(Rows::Gone);
    };
    if seq <= after {
        return Ok(Rows::Nothing);
    }
    let row = crate::routes::topics::sync::read_for(
        &mut conn,
        &principal,
        inner.engine()?,
        inner.clock.now(),
        topic,
        seq,
        Some(after),
    )
    .await
    .map_err(Fail::other)?;
    if !row.complete {
        return Ok(Rows::TooMany);
    }
    let highest = row.seq;
    Ok(Rows::Frame(Box::new(Frame {
        data: SyncData::Topic(TopicData {
            topic: Some(row.topic),
            threads: Some(row.threads),
            jobs: Some(row.jobs),
            announcements: Some(row.announcements),
        }),
        highest,
        held: Vec::new(),
    })))
}
