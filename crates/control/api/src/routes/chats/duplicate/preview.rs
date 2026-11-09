//! The contributor preview stores only its native reviewed recipient set, then publishes through the shared Read boundary.
use super::*;
use crate::key_operations::native::reply::{self, Projection};
use orch_api_contract::duplicates::{ChatDuplicateAudience, ChatDuplicatePreview};
use orch_api_contract::notifications::PERSON_NOTIFY_MAX_PEOPLE;
struct Preview;
struct Value {
    row: ChatRow,
    users: Vec<Id>,
    people: u64,
}
#[derive(Clone)]
struct Outcome {
    row: ChatRow,
    users: Vec<Id>,
    reply: ChatDuplicateAudience,
}
pub(super) fn mount(registry: &mut Registry) {
    registry.on_with_operation(ChatDuplicatePreview, effects::prepare::<Preview>, preview);
}
async fn subject(conn: &mut Conn, id: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    let target = targets::chat(conn, id).await?;
    Ok(vec![
        native::Facts::one(Source::Subject, CHAT, target.clone()),
        native::Facts::one(Source::PreviewContent, CHAT, target),
    ])
}
async fn readable(ctx: &Ctx, conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<()> {
    let work = creation::work(ctx, principal);
    let source = chats::load(conn, &work.acting(), id, Mode::Read).await;
    let source = work.refuse(conn, Some(id), source).await?;
    if source.row.kind != rows::KIND_GROUP {
        return Err(ApiError::invalid(
            "source",
            "only a group chat is duplicated",
        ));
    }
    Ok(())
}
impl effects::Effect for Preview {
    type Source = Source;
    type Query = ();
    type Body = ();
    type Value = Value;
    type Output = Outcome;
    fn declaration(_value: &Value) -> &'static Declaration<Source> {
        &sources::CHAT_PREVIEW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let facts = subject(conn, id).await?;
            let plan = native::bind(ctx, &sources::CHAT_PREVIEW, &facts)?;
            let principal = plan.preparation_principal(ctx, conn, caller).await?;
            readable(ctx, conn, &principal, id).await?;
            let row = rows::lock(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let audience =
                crate::routes::notifications::person::copy_audience(ctx, conn, &principal, id)
                    .await?;
            Ok((
                Value {
                    row,
                    users: audience.users,
                    people: audience.people,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(readable(ctx, conn, principal, value.row.id))
    }
}
impl effects::Apply for Preview {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Outcome>>> {
        Box::pin(async move {
            let limit = u32::try_from(PERSON_NOTIFY_MAX_PEOPLE).map_err(ApiError::internal)?;
            let mut result = Outcome {
                row: value.row.clone(),
                users: value.users.clone(),
                reply: ChatDuplicateAudience {
                    people: value.people,
                    notify_limit: limit,
                    preview_id: None,
                },
            };
            if value.people == 0 || value.people > u64::from(limit) {
                return Ok(effects::Outcome::Unchanged(result));
            }
            let id = orch_types::new_id();
            let deadline = before.deadline();
            before.check()?;
            let ttl = chrono::Duration::seconds(
                i64::try_from(ctx.app.config.engine.preflight_ttl_secs)
                    .map_err(ApiError::internal)?,
            );
            let expires = ctx
                .app
                .clock
                .now()
                .checked_add_signed(ttl)
                .ok_or_else(|| ApiError::internal("the native preview expiry overflowed"))?;
            orch_db::queries::jobs::forks::save_preflight(
                conn,
                &orch_db::queries::jobs::forks::PreflightRow {
                    id,
                    kind: "chat_duplicate".into(),
                    node_id: Some(value.row.home()),
                    environment_id: None,
                    created_by: principal.user(),
                    plan: serde_json::json!({"source_chat":value.row.id}),
                    expires_at: expires,
                    created_at: ctx.app.clock.now(),
                },
            )
            .await?;
            deadline.check()?;
            orch_db::queries::notifications::operations::preview_for(
                conn,
                id,
                &value.users,
                "person.notify",
            )
            .await?;
            deadline.check()?;
            result.reply.preview_id = Some(id);
            Ok(effects::Outcome::Applied(result))
        })
    }
}
struct Response;
impl Projection for Response {
    type Source = Source;
    type Input = Outcome;
    type Subject = ChatRow;
    type Value = ();
    type Reply = ChatDuplicateAudience;
    const READ_SOURCE: Source = Source::PreviewContent;
    fn declaration(_input: &Outcome) -> &'static Declaration<Source> {
        &sources::CHAT_PREVIEW
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Outcome,
    ) -> BoxFuture<'a, ApiResult<(ChatRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = rows::chat(conn, input.row.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let facts = subject(conn, row.id).await?;
            Ok((row, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Outcome,
        _subject: &'a ChatRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(readable(ctx, conn, principal, input.row.id))
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Outcome,
        source: &'a ChatRow,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let audience = crate::routes::notifications::person::copy_audience(
                ctx, conn, principal, source.id,
            )
            .await?;
            if source.next_seq != input.row.next_seq
                || source.seq_state != input.row.seq_state
                || source.home() != input.row.home()
                || audience.users != input.users
                || audience.people != input.reply.people
            {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "the contributors changed while their notice preview was being published",
                ));
            }
            Ok(((), Vec::new()))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Outcome,
        _prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ChatDuplicateAudience>> {
        Box::pin(async move { Ok(input.reply.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Outcome,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.subject == fresh.subject) })
    }
}
async fn preview(ctx: Ctx, (): (), (): ()) -> ApiResult<ChatDuplicateAudience> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Preview>(&ctx, &mut tx).await?;
    guard.check_deadline(&done.ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    reply::render::<Response>(&done.ctx, &done.accepted, done.output)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the current source cannot publish this notice preview",
            )
        })
}
