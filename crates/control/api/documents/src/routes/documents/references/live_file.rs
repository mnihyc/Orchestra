//! Once-only native reference reads retain the actual raw HTTP and physical file source.

use super::*;

use crate::key_operations::native::http_origin;

use orch_engine::commands::Outgoing;

use orch_engine::operation_scope::continuing;

use orch_engine::operation_scope::http_origin::HttpOrigin;

use orch_engine::operation_scope::reference_preview::FileSource;

use orch_engine::operation_scope::reference_preview::Reader;

use orch_link_contract::commands::CommandBody;

use orch_link_contract::commands::SerialKey;
#[derive(Clone)]
pub(super) struct Opening {
    source: FileSource,
    raw: HttpOrigin,
}

pub(super) fn mapped(error: continuing::Error) -> ApiError {
    match error {
        continuing::Error::Native(error) => ApiError::engine(error),
        _ => ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "the reference's original file source is no longer current",
        ),
    }
}

impl Opening {
    pub async fn capture(ctx: &Ctx, environment: Id, path: &str) -> ApiResult<Self> {
        let (mut tx, guard) = crate::middleware::transaction_guard::begin(ctx).await?;
        let source = FileSource::capture(&mut tx, guard.principal().caller(), environment, path)
            .await
            .map_err(ApiError::engine)?;
        let raw = http_origin::capture_operation(ctx, &mut tx, source.origin()).await?;
        let frame = raw
            .prepare::<Reader>(&mut tx, ctx.app.clock.as_ref(), source.clone())
            .await
            .map_err(mapped)?;
        let raw = frame
            .http_origin()
            .cloned()
            .ok_or_else(|| ApiError::internal("the reference lost its HTTP origin"))?;
        tx.commit().await.map_err(DbError::from)?;
        guard.check_deadline(ctx)?;
        frame
            .check_deadline(ctx.app.clock.as_ref())
            .map_err(mapped)?;
        Ok(Self { source, raw })
    }
    pub async fn current(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
    ) -> ApiResult<continuing::Frame<Reader>> {
        let raw = http_origin::current(ctx, conn, &self.raw).await?;
        let identity = crate::middleware::transaction_guard::current_http_identity(
            ctx,
            conn,
            crate::middleware::transaction_guard::HttpIdentityMode::Read,
        )
        .await?;
        let until = ctx
            .publication
            .retained_deadline()?
            .into_iter()
            .chain(identity.expires_at)
            .chain(raw.until())
            .min();
        let origin = self
            .raw
            .with_ceiling(&identity.caller, until)
            .map_err(mapped)?;
        origin
            .prepare::<Reader>(conn, ctx.app.clock.as_ref(), self.source.clone())
            .await
            .map_err(mapped)
    }
    pub async fn run<B: CommandBody>(
        &mut self,
        ctx: &Ctx,
        machine: Id,
        body: &B,
    ) -> ApiResult<Result<B::Result, crate::routes::workspace::problems::Failure>> {
        let mut command = Outgoing::new(orch_types::new_id(), machine, SerialKey::None, body)
            .map_err(ApiError::internal)?;
        command.deadline = Some(Time::new(
            ctx.app.clock.now() + chrono::Duration::seconds(i64::from(B::KIND.deadline_secs())),
        ));
        let mut tx = ctx.app.db.begin().await?;
        let frame = self.current(ctx, &mut tx).await?;
        self.raw = frame
            .http_origin()
            .cloned()
            .ok_or_else(|| ApiError::internal("the reference lost its HTTP source"))?;
        let authority = http_origin::bind_source(
            &self.raw,
            &command,
            self.source.command(&command).map_err(ApiError::engine)?,
        )?;
        tx.commit().await.map_err(DbError::from)?;
        let dispatcher = ctx.app.link.dispatcher();
        let queued = dispatcher
            .issue_stored_before(
                crate::dispatch::StoredCommand {
                    id: Some(command.id),
                    machine: command.machine,
                    kind: command.kind.as_str().into(),
                    key: command.key.wire(),
                    deadline: command.deadline,
                    body: command.body.clone(),
                    authority: Some(authority.clone()),
                },
                Box::new(|| {
                    frame
                        .check_deadline(ctx.app.clock.as_ref())
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                }),
            )
            .await;
        match queued {
            Ok(_) => {}
            Err(
                error @ (crate::dispatch::DispatchError::UnknownMachine
                | crate::dispatch::DispatchError::Revoked
                | crate::dispatch::DispatchError::Stopping),
            ) => {
                return Ok(Err(crate::routes::workspace::live::command_failure(
                    crate::dispatch::RunError::Dispatch(error),
                )));
            }
            Err(error) => return Err(ApiError::internal(error)),
        }
        let answer = dispatcher.wait_for::<B>(command.id).await;
        let mut tx = ctx.app.db.begin().await?;
        let (delivered, current) =
            http_origin::delivery(ctx, &mut tx, &authority, command.id).await?;
        self.raw = delivered;
        let frame = self.current(ctx, &mut tx).await?;
        self.raw = frame
            .http_origin()
            .cloned()
            .ok_or_else(|| ApiError::internal("the reference lost its delivered source"))?;
        tx.commit().await.map_err(DbError::from)?;
        http_origin::check(ctx, &current)?;
        frame
            .check_deadline(ctx.app.clock.as_ref())
            .map_err(mapped)?;
        Ok(answer.map_err(crate::routes::workspace::live::command_failure))
    }
    pub async fn retain(self, ctx: &Ctx) -> ApiResult<()> {
        let mut tx = ctx.app.db.begin().await?;
        let frame = self.current(ctx, &mut tx).await?;
        let caller = frame.principal().caller().clone();
        tx.commit().await.map_err(DbError::from)?;
        frame
            .check_deadline(ctx.app.clock.as_ref())
            .map_err(mapped)?;
        crate::key_operations::publication::retain(
            ctx,
            &caller,
            self,
            |ctx, conn, _current, source| {
                Box::pin(async move {
                    let frame = source.current(ctx, conn).await?;
                    frame.check_deadline(ctx.app.clock.as_ref()).map_err(mapped)
                })
            },
        )
    }
}
