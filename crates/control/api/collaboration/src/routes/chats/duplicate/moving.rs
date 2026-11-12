//! Native group-ending preview retains the existing exact reviewed-recipient mechanism.

use super::*;

use crate::key_operations::native::reply;

use crate::key_operations::native::reply::Projection;

use orch_api_contract::duplicates::ChatDuplicateAudience;

use orch_api_contract::notifications::PERSON_NOTIFY_MAX_PEOPLE;
struct Preview;

struct Reviewed {
    row: ChatRow,
    body: DuplicateBody,
    reply: ChatMoveOutPlan,
    users: Vec<Id>,
    facts: Vec<native::Facts<Source>>,
}

pub(super) fn mount(registry: &mut Registry) {
    registry.on_with_operation(ChatMoveOutPreview, effects::prepare::<Preview>, preview);
}

impl effects::Effect for Preview {
    type Source = Source;
    type Query = ();
    type Body = DuplicateBody;
    type Value = Prepared;
    type Output = Reviewed;
    fn declaration(_: &Prepared) -> &'static Declaration<Source> {
        &sources::CHAT_MOVE_PREVIEW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: DuplicateBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare_native(ctx, conn, caller, Intent::Preview(body)))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_native(ctx, conn, principal, value))
    }
}

impl effects::Apply for Preview {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Reviewed>>> {
        Box::pin(async move {
            let plan = value.delete.clone().ok_or_else(|| {
                ApiError::internal("the group Move preview has no original ending plan")
            })?;
            let audience = value.audience.as_ref().ok_or_else(|| {
                ApiError::internal("the group Move preview has no member audience")
            })?;
            let row = rows::chat(conn, value.source)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let facts = value.factual.clone();
            let limit = u32::try_from(PERSON_NOTIFY_MAX_PEOPLE).map_err(ApiError::internal)?;
            let mut reply = ChatMoveOutPlan {
                delete_plan: plan,
                notification: ChatDuplicateAudience {
                    people: audience.people,
                    notify_limit: limit,
                    preview_id: None,
                },
            };
            let deadline = before.deadline();
            let changed = audience.people > 0 && audience.people <= u64::from(limit);
            if changed {
                let id = orch_types::new_id();
                let ttl = chrono::Duration::seconds(
                    i64::try_from(ctx.app.config.engine.preflight_ttl_secs)
                        .map_err(ApiError::internal)?,
                );
                let expires = ctx.app.clock.now().checked_add_signed(ttl).ok_or_else(|| {
                    ApiError::internal("the native Move preview expiry overflowed")
                })?;
                before.check()?;
                orch_db::queries::jobs::forks::save_preflight(conn,&orch_db::queries::jobs::forks::PreflightRow {
                    id,kind:"chat_move_out".into(),node_id:Some(row.home()),environment_id:None,created_by:principal.user(),
                    plan:serde_json::json!({"source_chat":row.id,"delete_stamp":reply.delete_plan.stamp}),expires_at:expires,created_at:ctx.now,
                }).await?;
                deadline.check()?;
                orch_db::queries::notifications::operations::preview_for(
                    conn,
                    id,
                    &audience.users,
                    "person.notify",
                )
                .await?;
                deadline.check()?;
                reply.notification.preview_id = Some(id);
            }
            let output = Reviewed {
                row,
                body: value.copy.clone(),
                reply,
                users: audience.users.clone(),
                facts,
            };
            Ok(if changed {
                effects::Outcome::Applied(output)
            } else {
                effects::Outcome::Unchanged(output)
            })
        })
    }
}

struct Response;

impl Projection for Response {
    type Source = Source;
    type Input = Reviewed;
    type Subject = ChatRow;
    type Value = ();
    type Reply = ChatMoveOutPlan;
    const READ_SOURCE: Source = Source::PreviewContent;
    fn declaration(_: &Reviewed) -> &'static Declaration<Source> {
        &sources::CHAT_MOVE_PREVIEW
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Reviewed,
    ) -> BoxFuture<'a, ApiResult<(ChatRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = rows::chat(conn, input.row.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            Ok((row, input.facts.clone()))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Reviewed,
        _: &'a ChatRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            move_owner(ctx, conn, principal, input.row.id).await?;
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Reviewed,
        _: &'a ChatRow,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (current, _) = prepare_native(
                ctx,
                conn,
                principal.caller(),
                Intent::Preview(input.body.clone()),
            )
            .await?;
            let audience = current.audience.as_ref().ok_or_else(|| {
                ApiError::internal("the current group member audience disappeared")
            })?;
            if current.delete.as_ref() != Some(&input.reply.delete_plan)
                || audience.users != input.users
                || audience.people != input.reply.notification.people
            {
                return Err(duplicates::moving::stale());
            }
            Ok(((), Vec::new()))
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        input: &'a Reviewed,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ChatMoveOutPlan>> {
        Box::pin(async move { Ok(input.reply.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Reviewed,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.subject == fresh.subject) })
    }
}

async fn preview(ctx: Ctx, (): (), _: DuplicateBody) -> ApiResult<ChatMoveOutPlan> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Preview>(&ctx, &mut tx).await?;
    guard.check_deadline(&done.ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    reply::render::<Response>(&done.ctx, &done.accepted, done.output)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "The current group source cannot publish its Move preview.",
            )
        })
}
