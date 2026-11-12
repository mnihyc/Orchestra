//! One-time native reset-link outcomes retain identities, never their secret URLs.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::middleware::idempotency::ObservedCreation;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::admin::AdminUserCreateReceipt;

use orch_api_contract::admin::AdminUserCreateResponse;

use orch_api_contract::admin_sources;

use orch_api_contract::admin_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Principal;

use orch_db::idempotency::creation::Created;
pub(super) fn user_binding() -> ObservedCreation<(), AdminUserCreateBody, AdminUserCreateResponse> {
    ObservedCreation {
        kind: "admin_user",
        replay: user_replay,
    }
}

pub(super) fn link_binding()
-> ObservedCreation<(), AdminUserResetLinkCreateBody, AdminUserResetLinkCreateReply> {
    ObservedCreation {
        kind: "admin_reset_link",
        replay: |ctx, (), _body, created| {
            Box::pin(async move {
                let expires_at = expires(&created)?;
                let reply = AdminUserResetLinkCreateReply {
                    url: None,
                    expires_at,
                    already_created: true,
                };
                publication::receipt(&ctx, &reply)?;
                Ok(reply)
            })
        },
    }
}

fn expires(created: &Created) -> ApiResult<Time> {
    serde_json::from_value(
        created
            .data
            .get("expires")
            .cloned()
            .ok_or_else(|| ApiError::internal("the accepted one-time link has no native expiry"))?,
    )
    .map_err(ApiError::internal)
}

fn user_replay(
    ctx: Ctx,
    (): (),
    _body: AdminUserCreateBody,
    created: Created,
) -> BoxFuture<'static, ApiResult<AdminUserCreateResponse>> {
    Box::pin(async move {
        let expires_at = expires(&created)?;
        let reset_link_id =
            serde_json::from_value(created.data.get("link").cloned().ok_or_else(|| {
                ApiError::internal("the accepted account has no native link identity")
            })?)
            .map_err(ApiError::internal)?;
        let receipt = AdminUserCreateResponse::Receipt(AdminUserCreateReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            user_id: created.id,
            reset_link_id,
            expires_at,
            already_created: true,
        });
        publication::receipt(&ctx, &receipt)?;
        match reply::render_completed_replay::<Users>(&ctx, ctx.user()?.id(), created.id).await {
            Ok(Some(user)) => Ok(AdminUserCreateResponse::full(AdminUserCreateReply {
                user,
                reset_url: None,
                expires_at,
                already_created: true,
            })),
            Ok(None) => Ok(receipt),
            Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => Ok(receipt),
            Err(error) => Err(error),
        }
    })
}

struct Users;

impl reply::Projection for Users {
    type Source = Source;
    type Input = Id;
    type Subject = UserRow;
    type Value = AdminUser;
    type Reply = AdminUser;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Id) -> &'static Declaration<Source> {
        &admin_sources::USER_CREATE
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        id: &'a Id,
    ) -> BoxFuture<'a, ApiResult<(UserRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = identity::user_by_id(conn, *id)
                .await?
                .filter(|row| row.kind == "human")
                .ok_or_else(ApiError::not_found)?;
            Ok((
                row,
                vec![native::Facts::one(
                    Source::Content,
                    KeyTargetKind::Identity(KeyIdentityKind::Account),
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
        _row: &'a UserRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { super::super::native::standing(ctx, principal) })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        _id: &'a Id,
        row: &'a UserRow,
    ) -> BoxFuture<'a, ApiResult<(AdminUser, Vec<native::Facts<Source>>)>> {
        Box::pin(async move { Ok((read_one(ctx, conn, row.id).await?, Vec::new())) })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _id: &'a Id,
        opened: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<AdminUser>> {
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
        Box::pin(async move { Ok(old.subject.id == fresh.subject.id && old.value == fresh.value) })
    }
}
