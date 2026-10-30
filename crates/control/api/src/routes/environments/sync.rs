//! The state behind an environment's feed: the environment as the caller sees it, and the jobs that make it or change it.
//!
//! A client that lost its place on the feed of an environment asks here for what it needs to be
//! whole again: the environment itself, always, and the jobs about it that are under way, or from
//! a number on, those that were stamped on the feed after it, whatever became of them since. It is
//! read by whoever may read the environment (`GET /environments/{id}`: a permission on it, or a
//! thread there that the caller may view) and is the same environment, with the same cut for a
//! caller who holds nothing on it and for one who does not manage it.
//!
//! What the state consists of is [`MEMBERS`], one list, and it is the only place that names the
//! members: a part that makes the feed carry something more (the browsers and the displays opened
//! in the environment, say) adds a member there and a member to the reply in the contract.

use super::jobs::view as job_view;
use super::{reading, show};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::syncs::{self, Member, Piece, Reading};
use async_trait::async_trait;
use orch_api_contract::environment::{
    EnvironmentSyncGet, EnvironmentSyncGetQuery, EnvironmentSyncGetReply,
};
use orch_db::DbError;
use orch_db::queries::feeds;
use orch_db::queries::jobs::feeds as job_rows;
use serde_json::json;

/// The most jobs one request hands over from a number on. A feed that has more than this to tell
/// of an environment since the client's number says that it is not complete, and the client reads
/// from the start, which holds only the jobs that are under way: there is one that runs at a time
/// for an environment, so what is left over are the few that ended.
pub const JOBS_MAX: i64 = 200;

/// What the feed of an environment consists of, in the order it is put together. Each member is
/// named as the reply names it.
pub const MEMBERS: &[&dyn Member] = &[&TheEnvironment, &TheJobs];

/// Binds this module's requests.
pub(super) fn mount(registry: &mut Registry) {
    registry.on(EnvironmentSyncGet, sync);
}

/// The environment, as whoever reads it sees it. It is always there: a client holds the row it is
/// told of, never none.
struct TheEnvironment;

#[async_trait]
impl Member for TheEnvironment {
    fn name(&self) -> &'static str {
        "environment"
    }

    async fn read(&self, reading: &mut Reading<'_>) -> ApiResult<Piece> {
        let environment = show(
            reading.ctx,
            reading.conn,
            reading.principal,
            reading.subject,
        )
        .await?;
        Ok(Piece::whole(
            serde_json::to_value(&environment).map_err(ApiError::internal)?,
        ))
    }
}

/// The jobs about the environment: those that are under way, or those stamped after the number the
/// client holds.
struct TheJobs;

#[async_trait]
impl Member for TheJobs {
    fn name(&self) -> &'static str {
        "jobs"
    }

    async fn read(&self, reading: &mut Reading<'_>) -> ApiResult<Piece> {
        if reading.progress_only {
            let current = show(
                reading.ctx,
                reading.conn,
                reading.principal,
                reading.subject,
            )
            .await?
            .job;
            return Ok(Piece::whole(
                serde_json::to_value(current.into_iter().collect::<Vec<_>>())
                    .map_err(ApiError::internal)?,
            ));
        }
        let rows = match reading.since {
            None => job_rows::open_of_environment(reading.conn, reading.subject).await?,
            Some(since) => {
                let found = job_rows::of_environment_between(
                    reading.conn,
                    reading.subject,
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
        let mut jobs = Vec::with_capacity(rows.len());
        for row in &rows {
            if orch_db::queries::jobs::kind::SOURCE.contains(&row.kind.as_str()) {
                let full = authorization::require_resource(
                    reading.conn,
                    reading.principal,
                    orch_access_contract::permissions::ResourceKind::Environment,
                    reading.subject,
                    orch_access_contract::permissions::Permission::FilesView,
                    orch_authz::Policy::default(),
                )
                .await;
                match full {
                    Ok(()) => jobs.push(
                        serde_json::to_value(job_view(reading.conn, row).await?)
                            .map_err(ApiError::internal)?,
                    ),
                    Err(error)
                        if error.code() == orch_api_contract::common::ErrorCode::Internal =>
                    {
                        return Err(error);
                    }
                    Err(_) => jobs.push(
                        serde_json::to_value(orch_engine::jobs::source::reference(row))
                            .map_err(ApiError::internal)?,
                    ),
                }
            } else {
                jobs.push(
                    serde_json::to_value(job_view(reading.conn, row).await?)
                        .map_err(ApiError::internal)?,
                );
            }
        }
        Ok(Piece::whole(
            serde_json::to_value(&jobs).map_err(ApiError::internal)?,
        ))
    }
}

/// Brings a client up to date on an environment.
async fn sync(
    ctx: Ctx,
    query: EnvironmentSyncGetQuery,
    (): (),
) -> ApiResult<EnvironmentSyncGetReply> {
    let id = ctx.id("id")?;
    if query.since.is_some_and(|since| since < 0) {
        return Err(ApiError::invalid(
            "since",
            "is a number of the feed, from 0",
        ));
    }
    let mut tx = syncs::begin(&ctx).await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    // The same door as reading the environment: a permission on it, or a thread there that the
    // caller may view, who is then shown as much of it as that is.
    let progress_only =
        reading::decide(&mut tx, &principal, id).await? == reading::Reading::ThroughAThread;
    let seq = feeds::environment_seqs(&mut tx, &[id])
        .await?
        .pop()
        .map(|(_, seq)| seq)
        .ok_or_else(ApiError::not_found)?;
    let reply = {
        let mut reading = Reading {
            ctx: &ctx,
            conn: &mut tx,
            principal: &principal,
            progress_only,
            subject: id,
            seq,
            since: query.since,
        };
        syncs::put_together(MEMBERS, &mut reading).await?
    };
    tx.commit().await.map_err(DbError::from)?;
    Ok(reply)
}
