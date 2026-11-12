//! A bot link is read from its native kept generation; a replay never rotates its code.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::middleware::idempotency::ObservedCreation;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::notification_sources as sources;

use orch_api_contract::notification_sources::Source;

use orch_api_contract::notifications::NotificationRouteLinkReceipt;

use orch_api_contract::notifications::NotificationRouteLinkResponse;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Principal;

use orch_db::Conn;
struct Input {
    id: Id,
    channel: Id,
    hash: Vec<u8>,
    expires: Time,
}

struct Subject {
    route: route_rows::RouteRow,
    channel: ChannelRow,
}

struct Content;

pub(crate) fn binding() -> ObservedCreation<(), (), NotificationRouteLinkResponse> {
    ObservedCreation {
        kind: "notification_route_link",
        replay: |ctx, (), (), created| {
            Box::pin(async move {
                let data = created.data;
                let channel: Id =
                    serde_json::from_value(data.get("channel").cloned().ok_or_else(|| {
                        ApiError::internal("the accepted bot link has no native channel")
                    })?)
                    .map_err(ApiError::internal)?;
                let hash: Vec<u8> =
                    serde_json::from_value(data.get("hash").cloned().ok_or_else(|| {
                        ApiError::internal("the accepted bot link has no native generation")
                    })?)
                    .map_err(ApiError::internal)?;
                let expires: Time =
                    serde_json::from_value(data.get("expires").cloned().ok_or_else(|| {
                        ApiError::internal("the accepted bot link has no native expiry")
                    })?)
                    .map_err(ApiError::internal)?;
                let receipt =
                    NotificationRouteLinkResponse::Receipt(NotificationRouteLinkReceipt {
                        projection: orch_api_contract::common::ReceiptProjection::Receipt,
                        route_id: created.id,
                        expires_at: expires,
                    });
                publication::receipt(&ctx, &receipt)?;
                match reply::render_completed_replay::<Content>(
                    &ctx,
                    ctx.user()?.id(),
                    Input {
                        id: created.id,
                        channel,
                        hash,
                        expires,
                    },
                )
                .await
                {
                    Ok(Some(value)) => Ok(NotificationRouteLinkResponse::full(value)),
                    Ok(None) => Ok(receipt),
                    Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
                        Ok(receipt)
                    }
                    Err(error) => Err(error),
                }
            })
        },
    }
}

impl reply::Projection for Content {
    type Source = Source;
    type Input = Input;
    type Subject = Subject;
    type Value = String;
    type Reply = NotificationRouteLink;
    const READ_SOURCE: Source = Source::RouteAccount;
    fn declaration(_: &Input) -> &'static Declaration<Source> {
        &sources::ROUTE_LINK_CREATE
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let route = route_rows::route(conn, input.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if route.user_id != ctx.user()?.id()
                || route.channel_id != Some(input.channel)
                || ctx.id("channel_id")? != input.channel
                || route.link_hash.as_ref() != Some(&input.hash)
                || route.link_expires_at != Some(input.expires.get())
            {
                return Err(ApiError::not_found());
            }
            let channel = channel_rows::channel(conn, input.channel)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let account = targets::account(conn, route.user_id).await?;
            Ok((
                Subject { route, channel },
                vec![native::Facts::one(
                    Source::RouteAccount,
                    KeyTargetKind::Identity(KeyIdentityKind::Account),
                    account,
                )],
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        input: &'a Input,
        _: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            pages::channel_reaching(conn, p.user(), input.channel)
                .await?
                .ok_or_else(ApiError::not_found)?;
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Input,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(String, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if input.expires <= ctx.app.clock.time() {
                return Err(ApiError::not_found());
            }
            let master = crate::routes::resources::sealing::installed_master_key()?;
            let url = bot_links::kept_url(
                conn,
                master,
                notifiers(),
                &subject.channel,
                subject.route.id,
            )
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(ApiError::not_found)?;
            Ok((url, Vec::new()))
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        input: &'a Input,
        opened: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<NotificationRouteLink>> {
        Box::pin(async move {
            Ok(NotificationRouteLink {
                url: opened.value.clone(),
                expires_at: input.expires,
            })
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.route.id == fresh.subject.route.id
                && old.subject.route.user_id == fresh.subject.route.user_id
                && old.subject.route.channel_id == fresh.subject.route.channel_id
                && old.subject.route.link_hash == fresh.subject.route.link_hash
                && old.subject.channel.id == fresh.subject.channel.id
                && old.subject.channel.org_id == fresh.subject.channel.org_id
                && old.subject.channel.user_id == fresh.subject.channel.user_id
                && old.subject.channel.kind == fresh.subject.channel.kind
                && old.value == fresh.value)
        })
    }
}
