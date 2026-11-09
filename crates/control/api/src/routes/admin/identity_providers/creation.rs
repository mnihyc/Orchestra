//! Current masked sign-in configuration under the creation operation's own native read.
use super::*;
use crate::key_operations::{
    native::{self, reply},
    publication, targets,
};
use crate::middleware::idempotency::Creation;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_api_contract::{
    admin::{IdentityProviderCreateReceipt, IdentityProviderCreateResponse},
    admin_sources::{self, Source},
    operation_sources::Declaration,
};
use orch_authz::Principal;
use orch_db::idempotency::creation::Created;

pub(super) fn binding() -> Creation<(), IdentityProviderInput, IdentityProviderCreateResponse> {
    Creation {
        kind: "identity_provider",
        replay,
        identity: |value| {
            Ok(Created {
                kind: "identity_provider".into(),
                id: match value {
                    IdentityProviderCreateResponse::Full(value) => value.id,
                    IdentityProviderCreateResponse::Receipt(value) => value.identity_provider_id,
                },
                data: serde_json::Value::Null,
            })
        },
    }
}
fn replay(
    ctx: Ctx,
    (): (),
    _body: IdentityProviderInput,
    created: Created,
) -> BoxFuture<'static, ApiResult<IdentityProviderCreateResponse>> {
    Box::pin(async move {
        let receipt = IdentityProviderCreateResponse::Receipt(IdentityProviderCreateReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            identity_provider_id: created.id,
        });
        publication::receipt(&ctx, &receipt)?;
        match reply::render_completed_replay::<Content>(&ctx, ctx.user()?.id(), created.id).await {
            Ok(Some(value)) => Ok(IdentityProviderCreateResponse::full(value)),
            Ok(None) => Ok(receipt),
            Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => Ok(receipt),
            Err(error) => Err(error),
        }
    })
}
struct Content;
impl reply::Projection for Content {
    type Source = Source;
    type Input = Id;
    type Subject = IdentityProviderRow;
    type Value = IdentityProvider;
    type Reply = IdentityProvider;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Id) -> &'static Declaration<Source> {
        &admin_sources::IDENTITY_PROVIDER_CREATE
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        id: &'a Id,
    ) -> BoxFuture<'a, ApiResult<(IdentityProviderRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = identity::identity_provider(conn, *id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            Ok((
                row,
                vec![native::Facts::one(
                    Source::Content,
                    KeyTargetKind::Identity(KeyIdentityKind::System),
                    targets::system(),
                )],
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        _id: &'a Id,
        _row: &'a IdentityProviderRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { super::super::native::standing(ctx, principal) })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _id: &'a Id,
        row: &'a IdentityProviderRow,
    ) -> BoxFuture<'a, ApiResult<(IdentityProvider, Vec<native::Facts<Source>>)>> {
        Box::pin(async move { Ok((shown(&ctx.app.config, row.clone()), Vec::new())) })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _id: &'a Id,
        opened: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<IdentityProvider>> {
        Box::pin(async move { Ok(opened.value.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _id: &'a Id,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.id == fresh.subject.id
                && old.subject.revision == fresh.subject.revision
                && old.value == fresh.value)
        })
    }
}
