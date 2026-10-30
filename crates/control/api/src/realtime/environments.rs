//! The environment feed: state, jobs and pending updates, for everyone who may see the environment.
//!
//! The feed is the environment's own sequence (`environments.env_seq`): whatever changes the
//! environment or a job about it stamps it, and the announcement arrives on every API process when
//! the transaction commits. A frame holds what `GET /environments/{id}/sync` answers from the
//! reader's number on, read by the same list of members in one snapshot, so that the socket and the
//! request never say two things: the environment whole, and the jobs stamped after the number.
//!
//! Who may follow it is who may read the environment: whoever holds a permission on it, and whoever
//! holds none and may view a thread bound to it, who is shown the thread's cut of it (the current
//! job's progress, and nothing of its logs, results or people). A reader whose standing changes is
//! told to load the feed again, as every durable feed's reader is.

use crate::middleware::authentication::{Authed, Caller, Via};
use crate::realtime::subscriptions::{Access, principal_of};
use crate::realtime::sync::{Frame, Head, Rows};
use crate::realtime::{Fail, Inner};
use crate::registry::Ctx;
use crate::routes::environments::{reading, sync as state};
use crate::syncs::{self, Reading};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::{EnvironmentSyncGet, EnvironmentSyncGetReply};
use orch_api_contract::routes::Endpoint;
use orch_authz::Principal;
use orch_db::Conn;
use orch_db::queries::{feeds, identity};
use orch_realtime_contract::feeds::{EnvData, SyncData};
use orch_realtime_contract::sync::FeedHead;
use orch_types::{Id, Seq};
use std::sync::Arc;

/// The environment is one row, and its jobs member says itself when more changed than it hands
/// over (the reader is then asked to read again), so the head sets no limit of its own.
const NO_LIMIT: i64 = i64::MAX;

/// How `principal` may read the environment: none where it is gone, `Err` where they may not.
async fn reading_of(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
) -> Result<Result<reading::Reading, Access>, Fail> {
    match reading::decide(conn, principal, environment).await {
        Ok(how) => Ok(Ok(how)),
        Err(error) => match error.code() {
            ErrorCode::NotFound => Ok(Err(Access::NotFound)),
            ErrorCode::Internal => Err(Fail::other(error)),
            // What the refusal names (what is lacking, who can give it) is the request's to say; a
            // feed is only refused.
            _ => Ok(Err(Access::Forbidden)),
        },
    }
}

/// Whether `user` may follow the environment's feed.
pub(crate) async fn decide(inner: &Inner, user: Id, environment: Id) -> Result<Access, Fail> {
    let mut conn = inner.db.acquire().await?;
    if feeds::environment_seqs(&mut conn, &[environment])
        .await?
        .is_empty()
    {
        return Ok(Access::NotFound);
    }
    if !feeds::gone_environments(&mut conn, &[environment])
        .await?
        .is_empty()
    {
        return Ok(Access::Gone);
    }
    let Some(principal) = principal_of(inner, &mut conn, user).await? else {
        return Ok(Access::Forbidden);
    };
    Ok(
        match reading_of(&mut conn, &principal, environment).await? {
            Ok(_) => Access::Allowed,
            Err(refused) => refused,
        },
    )
}

/// Where the environment's feed stands, and how many rows lie above `since`.
pub(crate) async fn head(
    inner: &Inner,
    environment: Id,
    since: Option<Seq>,
) -> Result<Option<Head>, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some((_, seq)) = feeds::environment_seqs(&mut conn, &[environment])
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

/// The environment and the jobs stamped after `after`, when its number lies in `(after, upto]`,
/// as `person` may see them.
pub(crate) async fn read(
    inner: &Inner,
    person: Id,
    environment: Id,
    after: Seq,
    upto: Seq,
) -> Result<Rows, Fail> {
    // The projection of an environment is the request's; a server that was never handed the
    // application (no socket has been served yet) has nobody to read for.
    let Some(app) = inner.app() else {
        return Ok(Rows::Nothing);
    };
    let mut conn = feeds::begin_reading(&inner.db).await?;
    let Some(principal) = principal_of(inner, &mut conn, person).await? else {
        return Ok(Rows::Nothing);
    };
    let Some((_, seq)) = feeds::environment_seqs(&mut conn, &[environment])
        .await?
        .pop()
    else {
        return Ok(Rows::Gone);
    };
    if seq <= after || seq > upto {
        return Ok(Rows::Nothing);
    }
    let progress_only = match reading_of(&mut conn, &principal, environment).await? {
        Ok(how) => how == reading::Reading::ThroughAThread,
        Err(Access::NotFound | Access::Gone) => return Ok(Rows::Gone),
        // Refused since the subscription was decided: the next check of its access ends it, and
        // nothing is sent meanwhile.
        Err(_) => return Ok(Rows::Nothing),
    };
    let Some(user) = identity::user_by_id(&mut conn, person).await? else {
        return Ok(Rows::Nothing);
    };
    // A context for the projection alone: the reader's authority is `principal`, and nothing is
    // written through it.
    let caller = Caller::User(Arc::new(Authed {
        as_admin: principal.caller().as_admin(),
        user,
        via: Via::Session {
            session_id: Id::nil(),
        },
    }));
    let ctx = Ctx::for_tool(app, caller, EnvironmentSyncGet::ROUTE, environment);
    let reply: EnvironmentSyncGetReply = {
        let mut reading = Reading {
            ctx: &ctx,
            conn: &mut conn,
            principal: &principal,
            progress_only,
            subject: environment,
            seq,
            since: Some(after),
        };
        syncs::put_together(state::MEMBERS, &mut reading)
            .await
            .map_err(Fail::other)?
    };
    if !reply.complete {
        return Ok(Rows::TooMany);
    }
    Ok(Rows::Frame(Box::new(Frame {
        data: SyncData::Env(EnvData {
            environment: Some(reply.environment),
            jobs: Some(reply.jobs),
        }),
        highest: reply.seq,
        held: Vec::new(),
    })))
}
