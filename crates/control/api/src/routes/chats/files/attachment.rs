//! The native attachment stage, selected byte witness and unchanged custody writer.

use super::{reference, source, too_large};
use crate::attachments::{self, Refused};
use crate::key_operations::{blob_sources, native, targets};
use crate::middleware::{
    requests::{ApiError, ApiResult},
    transaction_guard,
};
use crate::registry::{Ctx, OperationPlan};
use axum::body::Body;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_api_contract::{
    chat_sources::{self as sources, Source},
    chats::ChatAttachmentCreateReply,
    operation_sources::Declaration,
};
use orch_authz::{Caller, Principal};
use orch_chat::{chats::Mode, following::BlobOrigin};
use orch_db::{Conn, DbError};
use orch_types::{Hash, Id};
use std::sync::{Arc, Mutex};

pub(super) struct Attachment;
pub(super) struct Known {
    limit: i64,
    direct: bool,
}
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Input {
    chat: Id,
    staged: attachments::Staged,
}
pub(super) struct Initial {
    input: Arc<Input>,
    selected: Vec<(Hash, BlobOrigin)>,
    limit: i64,
}
enum Content {
    Ready(attachments::Taken, attachments::Described),
    TooLarge { limit: i64, by_request: bool },
}
pub(super) struct Fetched {
    initial: Initial,
    content: Content,
}
pub(super) struct Prepared {
    fetched: Arc<Fetched>,
    retention: Mutex<Option<Arc<attachments::Retention>>>,
}

async fn primary(ctx: &Ctx, conn: &mut Conn) -> ApiResult<native::Facts<Source>> {
    Ok(native::Facts::one(
        Source::Chat,
        KeyTargetKind::Identity(KeyIdentityKind::Chat),
        targets::chat(conn, ctx.id("id")?).await?,
    ))
}
async fn ordinary(ctx: &Ctx, conn: &mut Conn, principal: &Principal) -> ApiResult<i64> {
    let id = ctx.id("id")?;
    let work = source::work(ctx, principal);
    let loaded = super::super::load(conn, &work, id, Mode::Read).await;
    let loaded =
        super::super::operation::project_current_result(ctx, conn, principal.caller(), loaded)
            .await?;
    super::super::operation::refuse(&work, conn, id, loaded.ensure_send()).await?;
    orch_chat::attachments::largest(conn, &loaded.row)
        .await
        .map_err(ApiError::internal)
}
async fn root(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
) -> ApiResult<(OperationPlan, i64, Vec<native::Facts<Source>>)> {
    let facts = vec![primary(ctx, conn).await?];
    let base = native::bind_known(ctx, &sources::ATTACHMENT_CREATE, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &base).await?;
    let limit = ordinary(ctx, conn, &principal).await?;
    Ok((base, limit, facts))
}

impl native::raw::Staged for Attachment {
    type Known = Known;
    type Input = Input;
    fn known_declaration(_: &Known) -> &'static Declaration<Source> {
        &sources::ATTACHMENT_CREATE
    }
    fn known<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Known, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (_, limit, facts) = root(ctx, conn, caller).await?;
            Ok((
                Known {
                    limit,
                    direct: crate::routes::threads::uploads::filename_of(ctx).is_some(),
                },
                facts,
            ))
        })
    }
    fn require_known<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Known,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { ordinary(ctx, conn, principal).await.map(|_| ()) })
    }
    fn take<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        known: Known,
        body: Body,
    ) -> BoxFuture<'a, ApiResult<Input>> {
        Box::pin(async move {
            let staged = attachments::stage(ctx, body, known.limit, reference)
                .await
                .map_err(refused)?;
            if staged.supplied_bytes() != known.direct {
                return Err(ApiError::internal(
                    "the native attachment changed its body branch",
                ));
            }
            let input = Input {
                chat: ctx.id("id")?,
                staged,
            };
            native::effects::initial::<Attachment>(ctx, caller, (), Arc::new(input.clone()))
                .await?;
            Ok(input)
        })
    }
}
fn refused(error: Refused) -> ApiError {
    match error {
        Refused::Other(error) => error,
        Refused::TooLarge { limit, by_request } => too_large(limit, by_request),
    }
}

impl native::effects::Initial for Attachment {
    type Input = Initial;
    type Fetched = Fetched;
    fn declaration(_: &Initial) -> &'static Declaration<Source> {
        &sources::ATTACHMENT_CREATE
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        input: Arc<Input>,
    ) -> BoxFuture<'a, ApiResult<(Initial, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if input.chat != ctx.id("id")? {
                return Err(ApiError::internal("the attachment changed its chat"));
            }
            let (base, limit, mut facts) = root(ctx, conn, caller).await?;
            let (selected, bindings) = if input.staged.supplied_bytes() {
                (Vec::new(), Vec::new())
            } else {
                let prepared =
                    blob_sources::prepare(ctx, conn, caller, &base, &[input.staged.hash()], &[])
                        .await?;
                (prepared.selected, prepared.bindings)
            };
            facts.extend(source::byte_facts(&bindings));
            // Every byte role is now resolved, including actual empty direct-byte sources.
            let _complete = native::bind_required(ctx, &sources::ATTACHMENT_CREATE, &facts)?;
            Ok((
                Initial {
                    input,
                    selected,
                    limit,
                },
                facts,
            ))
        })
    }
    fn require_initial<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        initial: &'a Initial,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            ordinary(ctx, conn, principal).await?;
            if !initial.input.staged.supplied_bytes() {
                attachments::require(conn, principal, &initial.input.staged.hash()).await?;
            }
            Ok(())
        })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        _: &'a Caller,
        initial: Initial,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            let content = match initial.input.staged.measure(ctx, initial.limit).await {
                Ok(taken) => {
                    let described = attachments::describe(ctx, &taken.meta, &taken.name).await?;
                    Content::Ready(taken, described)
                }
                Err(Refused::TooLarge { limit, by_request }) => {
                    Content::TooLarge { limit, by_request }
                }
                Err(Refused::Other(error)) => return Err(error),
            };
            Ok(Fetched { initial, content })
        })
    }
}

impl native::effects::Effect for Attachment {
    type Source = Source;
    type Query = ();
    type Body = Arc<Input>;
    type Value = Prepared;
    type Output = ChatAttachmentCreateReply;
    fn declaration(_: &Prepared) -> &'static Declaration<Source> {
        &sources::ATTACHMENT_CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        input: Arc<Input>,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let fetched = native::effects::initial_value::<Attachment>(ctx)?;
            if *fetched.initial.input != *input || ctx.id("id")? != input.chat {
                return Err(ApiError::internal(
                    "the consumed chat attachment changed its native input",
                ));
            }
            let _known = root(ctx, conn, caller).await?;
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                let parent: Option<Id> =
                    sqlx::query_scalar("SELECT id FROM chats WHERE id=$1 FOR KEY SHARE")
                        .bind(input.chat)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(DbError::from)?;
                if parent.is_none() {
                    return Err(ApiError::not_found());
                }
                orch_db::queries::uploads::lock_blobs(conn, &[input.staged.hash()]).await?;
            }
            let (base, limit, mut facts) = root(ctx, conn, caller).await?;
            let bindings = if input.staged.supplied_bytes() {
                Vec::new()
            } else {
                blob_sources::prepare_selected(
                    ctx,
                    conn,
                    caller,
                    &base,
                    &[input.staged.hash()],
                    &[],
                    &fetched.initial.selected,
                )
                .await?
                .bindings
            };
            facts.extend(source::byte_facts(&bindings));
            let complete = native::bind_required(ctx, &sources::ATTACHMENT_CREATE, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &complete).await?;
            ordinary(ctx, conn, &principal).await?;
            let taken = match &fetched.content {
                Content::Ready(taken, _) => taken,
                Content::TooLarge { limit, by_request } => {
                    return Err(too_large(*limit, *by_request));
                }
            };
            if taken.meta.size > limit {
                return Err(too_large(limit, false));
            }
            let retention = attachments::prepare_retention(conn, &principal, taken).await?;
            Ok((
                Prepared {
                    fetched,
                    retention: Mutex::new(Some(Arc::new(retention))),
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
        Box::pin(async move {
            ordinary(ctx, conn, principal).await?;
            let retention = {
                let slot = value.retention.lock().map_err(|_| {
                    ApiError::internal("the native attachment custody plan is poisoned")
                })?;
                slot.as_ref().cloned().ok_or_else(|| {
                    ApiError::internal("the native attachment custody plan was consumed")
                })?
            };
            attachments::require_retention(conn, principal, &retention).await
        })
    }
}

impl native::effects::ApplyOwned for Attachment {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Prepared,
        before: native::effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<native::effects::Outcome<ChatAttachmentCreateReply>>> {
        Box::pin(async move {
            let retention = {
                let mut slot = value.retention.lock().map_err(|_| {
                    ApiError::internal("the native attachment custody plan is poisoned")
                })?;
                slot.take().ok_or_else(|| {
                    ApiError::internal("the native attachment custody plan was consumed")
                })?
            };
            let retention = Arc::try_unwrap(retention).map_err(|_| {
                ApiError::internal("the native attachment custody plan is still in use")
            })?;
            let Content::Ready(taken, described) = &value.fetched.content else {
                return Err(ApiError::internal(
                    "the native attachment content is missing",
                ));
            };
            let deadline = before.deadline();
            attachments::apply_retention(ctx, &mut tx, principal, retention, move || {
                before.check()
            })
            .await??;
            // Registration can wait even when its blob row did not exist. Every write is
            // rollbackable; refresh the same original witness before committing custody.
            let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
            ordinary(&current, &mut tx, guard.principal()).await?;
            guard.check_deadline(&current)?;
            deadline.check()?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(native::effects::Outcome::Applied(
                ChatAttachmentCreateReply {
                    blob: taken.meta.hash,
                    name: taken.name.clone(),
                    size: taken.meta.size,
                    media_type: described.media_type.clone(),
                },
            ))
        })
    }
}
