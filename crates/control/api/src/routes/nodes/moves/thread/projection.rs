//! Stored native review and moved Thread readback, with no repeated trial or writer.

use super::*;
use crate::key_operations::native;
use crate::key_operations::native::reply::{Prepared, Projection};
use crate::routes::nodes::{content_sources, read_sources};
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::thread_move_sources::{self as sources, Source};

pub(super) struct Review;
pub(super) struct Moved;
pub(super) struct ReviewSubject {
    base: operation::Base,
    kept: operation::Kept,
}

async fn content(
    conn: &mut Conn,
    source: Source,
    objects: impl IntoIterator<Item = (ObjectKind, Id)>,
) -> ApiResult<native::Facts<Source>> {
    let objects = objects
        .into_iter()
        .map(|(kind, id)| ObjectRef { kind, id })
        .collect::<Vec<_>>();
    let identities = read_sources::Sources::from_objects(objects.iter().copied())
        .identity_targets(conn)
        .await?;
    let mut groups = BTreeMap::<KeyTargetKind, Vec<_>>::new();
    for object in objects {
        groups
            .entry(KeyTargetKind::Object(object.kind))
            .or_default()
            .push(
                identities
                    .get(&object)
                    .cloned()
                    .ok_or_else(ApiError::not_found)?,
            );
    }
    Ok(native::Facts::collection(
        source,
        groups.into_iter().collect(),
    ))
}

impl Projection for Review {
    type Source = Source;
    type Input = Reviewed;
    type Subject = ReviewSubject;
    type Value = ();
    type Reply = ThreadMovePlan;
    const READ_SOURCE: Source = Source::Main;
    fn declaration(_input: &Reviewed) -> &'static Declaration<Source> {
        &sources::PREVIEW
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Reviewed,
    ) -> BoxFuture<'a, ApiResult<(ReviewSubject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = jobs::forks::preflight(conn, input.id)
                .await?
                .filter(|row| {
                    row.kind == KIND
                        && row.node_id == Some(input.sources.main)
                        && row.created_by == input.actor
                })
                .ok_or_else(ApiError::not_found)?;
            let kept: operation::Kept =
                serde_json::from_value(row.plan).map_err(ApiError::internal)?;
            if kept.review != input.plan {
                return Err(ApiError::not_found());
            }
            let base = operation::base(conn, input.sources.main, input.sources.target).await?;
            if !input.sources.preserves(&base) {
                return Err(ApiError::not_found());
            }
            let facts = base.required.clone();
            Ok((ReviewSubject { base, kept }, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Reviewed,
        subject: &'a ReviewSubject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if principal.user() != input.actor {
                return Err(ApiError::not_found());
            }
            // Reading the stored private access/settings comparison retains its native
            // controller door. This is ordinary standing, never a new scoped Manage act.
            ordinary(conn, principal, subject.base.row.node_id).await?;
            may_move(
                conn,
                principal,
                NodeKind::Thread,
                subject.base.row.node_id,
                subject.base.target,
            )
            .await
        })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Reviewed,
        _subject: &'a ReviewSubject,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            Ok((
                (),
                vec![
                    content(
                        conn,
                        Source::ReviewContent,
                        input.sources.content.iter().copied(),
                    )
                    .await?,
                ],
            ))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Reviewed,
        prepared: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ThreadMovePlan>> {
        Box::pin(async move { Ok(prepared.subject.kept.review.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Reviewed,
        old: &'a Prepared<Self>,
        fresh: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.kept.review == fresh.subject.kept.review
                && input.sources.preserves(&fresh.subject.base))
        })
    }
}

impl Projection for Moved {
    type Source = Source;
    type Input = MovedThread;
    type Subject = operation::Base;
    type Value = content_sources::Prepared;
    type Reply = Thread;
    const READ_SOURCE: Source = Source::Main;
    fn declaration(_input: &MovedThread) -> &'static Declaration<Source> {
        &sources::MOVE
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a MovedThread,
    ) -> BoxFuture<'a, ApiResult<(operation::Base, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let value =
                operation::base(conn, input.receipt.thread_id, input.receipt.parent_id).await?;
            if value.row.topic_id != input.receipt.parent_id {
                return Err(ApiError::not_found());
            }
            let facts = value.required.clone();
            Ok((value, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a MovedThread,
        subject: &'a operation::Base,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if native::modern(principal.caller()) {
                crate::middleware::authorization::require(
                    conn,
                    principal,
                    subject.row.node_id,
                    crate::middleware::authorization::VIEW,
                    orch_authz::Act::Use,
                    Policy::default(),
                )
                .await?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a MovedThread,
        subject: &'a operation::Base,
    ) -> BoxFuture<'a, ApiResult<(content_sources::Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let value = content_sources::prepare(conn, principal, subject.row.node_id).await?;
            let fact = content(conn, Source::ThreadContent, value.objects.iter().copied()).await?;
            Ok((value, vec![fact]))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a MovedThread,
        prepared: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Thread>> {
        Box::pin(async move {
            match content_sources::render(
                ctx,
                conn,
                principal,
                prepared.subject.row.node_id,
                &prepared.value,
            )
            .await?
            {
                Node::Thread(thread) => Ok(thread),
                _ => Err(ApiError::not_found()),
            }
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a MovedThread,
        old: &'a Prepared<Self>,
        fresh: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.row.topic_id == fresh.subject.row.topic_id
                && old.subject.row.environment_id == fresh.subject.row.environment_id
                && old.subject.row.current_segment_id == fresh.subject.row.current_segment_id
                && old.value.preserves(&fresh.value))
        })
    }
}
