//! One current native tag projection for first acceptance and completed creation replay.
use super::*;
use crate::key_operations::{
    native::{self, reply},
    publication, targets,
};
use crate::middleware::idempotency::Creation;
use futures::future::BoxFuture;
use orch_api_contract::{
    directory::{TagCreateReceipt, TagCreateResponse},
    directory_sources::{self as sources, Source},
    operation_sources::Declaration,
};
use orch_authz::{Caller, Principal};
use orch_db::idempotency::creation::Created;

struct Tags;
struct Accepted {
    id: Id,
    first: bool,
}
fn receipt(id: Id) -> TagCreateResponse {
    TagCreateResponse::Receipt(TagCreateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        tag_id: id,
    })
}
pub(super) fn replay_binding() -> Creation<(), NodeTagCreateBody, TagCreateResponse> {
    Creation {
        kind: "tag",
        replay,
        identity: |value| {
            Ok(Created {
                kind: "tag".into(),
                id: match value {
                    TagCreateResponse::Full(tag) => tag.id,
                    TagCreateResponse::Receipt(tag) => tag.tag_id,
                },
                data: serde_json::Value::Null,
            })
        },
    }
}
fn replay(
    ctx: Ctx,
    (): (),
    _: NodeTagCreateBody,
    created: Created,
) -> BoxFuture<'static, ApiResult<TagCreateResponse>> {
    Box::pin(async move {
        let safe = receipt(created.id);
        publication::receipt(&ctx, &safe)?;
        match reply::render_completed_replay::<Tags>(
            &ctx,
            ctx.user()?.id(),
            Accepted {
                id: created.id,
                first: false,
            },
        )
        .await
        {
            Ok(Some(tag)) => Ok(TagCreateResponse::full(tag)),
            Ok(None) => Ok(safe),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(safe)
            }
            Err(error) => Err(error),
        }
    })
}
pub(super) async fn response(ctx: &Ctx, accepted: &Caller, id: Id) -> ApiResult<TagCreateResponse> {
    let safe = receipt(id);
    publication::receipt(ctx, &safe)?;
    match reply::render::<Tags>(ctx, accepted, Accepted { id, first: true }).await {
        Ok(Some(tag)) => Ok(TagCreateResponse::full(tag)),
        Ok(None) => Ok(safe),
        Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
            Ok(safe)
        }
        Err(error) => Err(error),
    }
}
impl reply::Projection for Tags {
    type Source = Source;
    type Input = Accepted;
    type Subject = TagRow;
    type Value = ();
    type Reply = Tag;
    const READ_SOURCE: Source = Source::TagNode;
    fn declaration(_: &Accepted) -> &'static Declaration<Source> {
        &sources::TAG_CREATE
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Accepted,
    ) -> BoxFuture<'a, ApiResult<(TagRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = hierarchy::tags_by_id(conn, &[input.id])
                .await?
                .pop()
                .filter(|row| Some(row.node_id) == ctx.id("id").ok())
                .ok_or_else(ApiError::not_found)?;
            let target = targets::node(conn, row.node_id).await?;
            let facts = vec![native::Facts::one(
                Source::TagNode,
                target.object.kind,
                target,
            )];
            Ok((row, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Accepted,
        row: &'a TagRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if input.first && !native::modern(principal.caller()) {
                let (_, kind) = placed(conn, row.node_id).await?;
                require_tag_manager(conn, principal, row.node_id, kind).await
            } else {
                authorization::require(
                    conn,
                    principal,
                    row.node_id,
                    authorization::VIEW,
                    Act::Use,
                    Policy::default(),
                )
                .await
            }
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Accepted,
        _: &'a TagRow,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Accepted,
        value: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Tag>> {
        Box::pin(async move {
            shown(conn, principal, std::slice::from_ref(&value.subject))
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Accepted,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.id == fresh.subject.id
                && old.subject.node_id == fresh.subject.node_id
                && old.subject.name == fresh.subject.name
                && old.subject.color == fresh.subject.color)
        })
    }
}
