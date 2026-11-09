//! Complete factual preparation and one atomic accepted message, roster and audit row.
use super::*;
use crate::key_operations::{
    native::effects::{self, Apply, Effect, FirstEffect, Outcome},
    native::reply,
    publication,
};
use futures::future::BoxFuture;
use orch_access_contract::audit::AuditAction;
use orch_api_contract::{common::ReceiptProjection, operation_sources::Declaration};
use orch_authz::Caller;
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Json, new_id};
use serde_json::json;
use std::marker::PhantomData;

pub(super) trait Mode: Send + Sync + 'static {
    const ROW: &'static Declaration<Source>;
    fn org(ctx: &Ctx) -> ApiResult<Option<Id>>;
}
pub(super) struct Org;
impl Mode for Org {
    const ROW: &'static Declaration<Source> = &sources::ORG_CREATE;
    fn org(ctx: &Ctx) -> ApiResult<Option<Id>> {
        Ok(Some(ctx.id("id")?))
    }
}
pub(super) struct System;
impl Mode for System {
    const ROW: &'static Declaration<Source> = &sources::SYSTEM_CREATE;
    fn org(_ctx: &Ctx) -> ApiResult<Option<Id>> {
        Ok(None)
    }
}

pub(super) fn replay_binding<M: Mode>()
-> crate::middleware::idempotency::Creation<(), AdminMessageCreateBody, AdminMessageCreateResponse>
{
    crate::middleware::idempotency::Creation {
        kind: "message",
        replay: replay::<M>,
        identity: creation_identity,
    }
}
fn creation_identity(
    value: &AdminMessageCreateResponse,
) -> ApiResult<orch_db::idempotency::creation::Created> {
    let (id, count, job) = match value {
        AdminMessageCreateResponse::Full(value) => {
            (value.id, value.recipient_count, value.delivery_job_id)
        }
        AdminMessageCreateResponse::Receipt(value) => (
            value.message_id,
            value.recipient_count,
            value.delivery_job_id,
        ),
    };
    Ok(orch_db::idempotency::creation::Created {
        kind: "message".into(),
        id,
        data: json!({"count":count,"job":job}),
    })
}
fn replay<M: Mode>(
    ctx: Ctx,
    (): (),
    _body: AdminMessageCreateBody,
    created: orch_db::idempotency::creation::Created,
) -> BoxFuture<'static, ApiResult<AdminMessageCreateResponse>> {
    Box::pin(async move {
        let count = created
            .data
            .get("count")
            .and_then(Json::as_i64)
            .ok_or_else(|| ApiError::internal("the accepted message has no recorded count"))?;
        let job = created
            .data
            .get("job")
            .cloned()
            .map(serde_json::from_value::<Option<Id>>)
            .transpose()
            .map_err(ApiError::internal)?
            .flatten();
        let receipt = AdminMessageCreateResponse::Receipt(AdminMessageReceipt {
            projection: ReceiptProjection::Receipt,
            message_id: created.id,
            recipient_count: count,
            delivery_job_id: job,
        });
        publication::receipt(&ctx, &receipt)?;
        let row = {
            let mut conn = ctx.app.db.acquire().await?;
            rows::get(&mut conn, created.id).await?
        };
        let Some(row) = row else { return Ok(receipt) };
        match reply::render_completed_replay::<Content<M>>(&ctx, ctx.user()?.id(), row).await {
            Ok(Some(message)) => Ok(AdminMessageCreateResponse::Full(Box::new(message))),
            Ok(None) => Ok(receipt),
            Err(error)
                if matches!(
                    error.code(),
                    orch_api_contract::common::ErrorCode::NotFound
                        | orch_api_contract::common::ErrorCode::AuthzDenied
                ) =>
            {
                Ok(receipt)
            }
            Err(error) => Err(error),
        }
    })
}

pub(super) struct Sending<M>(PhantomData<M>);
pub(super) struct Prepared {
    org: Option<Id>,
    root: Option<Id>,
    name: Option<String>,
    body: AdminMessageCreateBody,
    sender: Id,
    attribution: Json,
}

impl<M: Mode> Effect for Sending<M> {
    type Source = Source;
    type Query = ();
    type Body = AdminMessageCreateBody;
    type Value = Prepared;
    type Output = MessageRow;
    fn declaration(_value: &Prepared) -> &'static Declaration<Source> {
        M::ROW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        mut body: AdminMessageCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let org = M::org(ctx)?;
            if org.is_some() && body.banner_until.is_some() {
                return Err(ApiError::invalid(
                    "banner_until",
                    "is available only for installation messages",
                ));
            }
            let (root, name) = match org {
                Some(org) => {
                    let (name, root): (String, Id) = sqlx::query_as("SELECT name,org_id FROM nodes WHERE id=$1 AND kind='org' AND deleted_at IS NULL FOR UPDATE")
                        .bind(org).fetch_optional(&mut *conn).await.map_err(DbError::from)?.ok_or_else(ApiError::not_found)?;
                    (Some(root), Some(name))
                }
                None => (None, None),
            };
            let target = scope_target(conn, org).await?;
            let facts = vec![native::Facts::one(
                Source::Scope,
                target.object.kind,
                target,
            )];
            let bound = native::bind_required(ctx, M::ROW, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &bound).await?;
            require_sender(ctx, conn, &principal, org).await?;
            body.title = body.title.trim().to_owned();
            if body.title.is_empty() {
                return Err(ApiError::invalid("title", "must not be empty"));
            }
            if body.body.trim().is_empty() {
                return Err(ApiError::invalid("body", "must not be empty"));
            }
            let most = orch_settings::get::<i64>(
                conn,
                org.map_or(Place::System, Place::Node),
                keys::MESSAGE_BODY_MAX_CHARS,
            )
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::internal("message length has no resolved value"))?;
            if i64::try_from(body.body.chars().count()).map_err(ApiError::internal)? > most {
                return Err(ApiError::invalid(
                    "body",
                    "exceeds the current message length setting",
                ));
            }
            let sender = principal.user();
            let attribution = orch_engine::views::user_ref_json(conn, sender)
                .await
                .map_err(ApiError::internal)?;
            Ok((
                Prepared {
                    org,
                    root,
                    name,
                    body,
                    sender,
                    attribution,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { require_sender(ctx, conn, principal, value.org).await })
    }
}

impl<M: Mode> Apply for Sending<M> {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Prepared,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<MessageRow>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            before.check()?;
            let mut row = MessageRow {
                id: new_id(),
                org_id: value.org,
                org_name: value.name.clone(),
                title: value.body.title.clone(),
                body: value.body.body.clone(),
                sender_id: value.sender,
                sender: value.attribution.clone(),
                created_at: ctx.now,
                recipient_count: 0,
                delivery_job_id: None,
            };
            rows::insert(conn, &row).await?;
            row.recipient_count =
                rows::capture_audience(conn, row.id, row.org_id, row.sender_id).await?;
            row.delivery_job_id = Some(
                orch_engine::jobs::document_notices::enqueue_message(conn, row.id)
                    .await
                    .map_err(ApiError::internal)?,
            );
            if let Some(until) = &value.body.banner_until {
                let origin = crate::sign_in::origin(ctx);
                // The native system sender has the same current standing as the settings writer.
                // Both writes share its guarded transaction, so a refused banner sends nothing.
                orch_settings::write::write_before(
                    conn,
                    &orch_settings::write::Write {
                        actor: orch_db::audit::Actor::User(value.sender),
                        target: orch_settings::Target::System,
                        key: keys::SYSTEM_BANNER,
                        value: json!({"text":value.body.title,"level":"info","until":until}),
                        locked: None,
                        expected_revision: None,
                        may_lock: false,
                        origin: &origin,
                    },
                    ctx.now,
                    || {
                        deadline
                            .check()
                            .map_err(|_| orch_settings::write::WriteError::OperationRefused)
                    },
                )
                .await
                .map_err(crate::routes::admin::settings::refused)?;
            }
            deadline.check()?;
            let mut entry = orch_db::audit::Entry::new(
                orch_db::audit::Actor::User(value.sender),
                orch_db::audit::action!(AuditAction::MessageSent).as_str(),
                "message",
                Some(row.id),
                "Sent an administrator message",
            );
            entry.node_id = value.org;
            entry.org_id = value.root;
            entry.details = json!({"org_id":row.org_id,"recipient_count":row.recipient_count});
            orch_db::audit::append(conn, &entry, &crate::sign_in::origin(ctx), ctx.now).await?;
            deadline.check()?;
            Ok(Outcome::Applied(row))
        })
    }
}

pub(super) async fn send<M: Mode>(
    ctx: Ctx,
    (): (),
    _body: AdminMessageCreateBody,
) -> ApiResult<AdminMessageCreateResponse> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Sending<M>>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = AdminMessageCreateResponse::Receipt(AdminMessageReceipt {
        projection: ReceiptProjection::Receipt,
        message_id: done.output.id,
        recipient_count: done.output.recipient_count,
        delivery_job_id: done.output.delivery_job_id,
    });
    publication::receipt(&done.ctx, &receipt)?;
    ctx.app.engine.kick();
    Ok(
        reply::render::<Content<M>>(&done.ctx, &done.accepted, done.output)
            .await?
            .map_or(receipt, |message| {
                AdminMessageCreateResponse::Full(Box::new(message))
            }),
    )
}

struct Content<M>(PhantomData<M>);
impl<M: Mode> reply::Projection for Content<M> {
    type Source = Source;
    type Input = MessageRow;
    type Subject = MessageRow;
    type Value = AdminMessage;
    type Reply = AdminMessage;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_input: &MessageRow) -> &'static Declaration<Source> {
        M::ROW
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a MessageRow,
    ) -> BoxFuture<'a, ApiResult<(MessageRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = rows::get(conn, input.id)
                .await?
                .filter(|row| row.org_id == input.org_id && row.sender_id == input.sender_id)
                .ok_or_else(ApiError::not_found)?;
            let target = scope_target(conn, row.org_id).await?;
            Ok((
                row,
                vec![
                    native::Facts::one(Source::Scope, target.object.kind, target.clone()),
                    native::Facts::one(Source::Content, target.object.kind, target),
                ],
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a MessageRow,
        subject: &'a MessageRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { require_reader(ctx, conn, principal, subject).await })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a MessageRow,
        subject: &'a MessageRow,
    ) -> BoxFuture<'a, ApiResult<(AdminMessage, Vec<native::Facts<Source>>)>> {
        Box::pin(async move { Ok((view(subject)?, Vec::new())) })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a MessageRow,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<AdminMessage>> {
        Box::pin(async move { Ok(prepared.value.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a MessageRow,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.subject == fresh.subject) })
    }
}
