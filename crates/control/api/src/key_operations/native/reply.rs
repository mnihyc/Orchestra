//! Typed native projections after an accepted effect, including jobs and genuine no-job results.

use super::*;
use orch_access_contract::key_scope::KeyObjectRef;
use orch_api_contract::common::ErrorCode;

/// Native response sources and projection, independent of a writer or its preflight.
/// Subject lookup and preparation must be effect-free. Rendering runs only after
/// same-operation Read admission; validation never repeats rendering or business work.
pub(crate) trait Projection: Sized + Send + Sync + 'static {
    type Source: Eq + Send + Sync + 'static;
    type Input: Send + Sync + 'static;
    type Subject: Send + Sync + 'static;
    type Value: Send + Sync + 'static;
    type Reply: Send + 'static;

    const READ_SOURCE: Self::Source;

    fn declaration(input: &Self::Input) -> &'static Declaration<Self::Source>;

    /// Resolve the actual stored response subject and known mandatory/root facts.
    /// Input is the immutable native outcome, never a replayable request body.
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<Facts<Self::Source>>)>>;

    /// Keep the native ordinary response gate; completed Write/Manage is not re-admitted.
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        subject: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// Prepare remaining real response sources after the primary Read and native gate.
    /// These are additional roles; an already supplied role cannot be overwritten.
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        subject: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<Facts<Self::Source>>)>>;

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        prepared: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>>;

    /// Preserve actual native association and disclosure gates of the producing snapshot.
    /// This hook checks retained values, not a newly rendered replacement response.
    fn check_opened<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        opened: &'a Prepared<Self>,
        fresh: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>>;
}

pub(crate) struct Prepared<F: Projection> {
    pub subject: F::Subject,
    pub value: F::Value,
}

struct Opened<F: Projection> {
    prepared: Prepared<F>,
    response: OperationPlan,
    primary: (KeyTargetKind, KeyObjectRef),
}

async fn prepare<F: Projection>(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    input: &F::Input,
) -> ApiResult<Option<(Opened<F>, Principal)>> {
    let declaration = F::declaration(input);
    let (subject, mut facts) = F::subject(ctx, conn, input).await?;
    let primary = resource_reply::primary(&facts, &F::READ_SOURCE)?;
    let known = bind_required(ctx, declaration, &facts)?;
    let Some(reader) = resource_reply::reader(
        ctx,
        conn,
        current,
        &known,
        declaration,
        &F::READ_SOURCE,
        &facts,
    )
    .await?
    else {
        return Ok(None);
    };
    F::require(ctx, conn, &reader, input, &subject).await?;
    let (value, additional) = F::prepare(ctx, conn, &reader, input, &subject).await?;
    facts.extend(additional);
    let response = bind(ctx, declaration, &facts)?;
    Ok(Some((
        Opened {
            prepared: Prepared { subject, value },
            response,
            primary,
        },
        reader,
    )))
}

/// Read a completed native outcome without rerunning its writer or preflight.
/// Outer None means optional disclosure closed. A native Reply of Option<T> still
/// represents a genuine no-result outcome as Some(None). The domain owns its typed
/// receipt and must retain that fallback immediately after its effect commits.
pub(crate) async fn render<F: Projection>(
    ctx: &Ctx,
    accepted: &Caller,
    input: F::Input,
) -> ApiResult<Option<F::Reply>> {
    let input = Arc::new(input);
    let producing_input = Arc::clone(&input);
    let initial = completed(ctx, accepted, move |ctx, conn, current| {
        Box::pin(async move {
            let Some((opened, reader)) = prepare::<F>(ctx, conn, current, &producing_input).await?
            else {
                return Ok(None);
            };
            let projection = ctx.clone();
            let response = opened.response.clone();
            if modern(reader.caller()) {
                response
                    .read_response(ctx, conn, reader.caller(), move |conn, reader| {
                        Box::pin(async move {
                            produce::<F>(&projection, conn, reader, &producing_input, opened).await
                        })
                    })
                    .await
            } else {
                produce::<F>(&projection, conn, &reader, &producing_input, opened)
                    .await
                    .map(Some)
            }
        })
    })
    .await?;
    let Some((value, opened)) = initial else {
        ctx.complete_disclosure(resolver::Disclosure::Denied);
        return Ok(None);
    };
    crate::key_operations::publication::retain(
        ctx,
        accepted,
        (input, Arc::new(opened)),
        |ctx, conn, current, retained| {
            Box::pin(async move {
                let (input, opened) = retained;
                if validate::<F>(ctx, conn, current, input, opened).await? {
                    Ok(())
                } else {
                    Err(ApiError::new(
                        ErrorCode::AuthzDenied,
                        "the native response source is closed",
                    ))
                }
            })
        },
    )?;
    Ok(Some(value))
}

async fn produce<F: Projection>(
    ctx: &Ctx,
    conn: &mut Conn,
    reader: &Principal,
    input: &F::Input,
    opened: Opened<F>,
) -> ApiResult<(F::Reply, Opened<F>)> {
    F::require(ctx, conn, reader, input, &opened.prepared.subject).await?;
    let value = F::render(ctx, conn, reader, input, &opened.prepared).await?;
    Ok((value, opened))
}

async fn validate<F: Projection>(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    input: &Arc<F::Input>,
    opened: &Arc<Opened<F>>,
) -> ApiResult<bool> {
    let Some((fresh, reader)) = prepare::<F>(ctx, conn, current, input).await? else {
        return Ok(false);
    };
    if opened.primary != fresh.primary
        || !fresh
            .response
            .shared()
            .covers_sources(opened.response.shared())
    {
        return Ok(false);
    }
    if modern(reader.caller()) {
        let response = fresh.response.clone();
        let opened = Arc::clone(opened);
        let projection = ctx.clone();
        let input = Arc::clone(input);
        Ok(response
            .read_response(ctx, conn, reader.caller(), move |conn, reader| {
                Box::pin(async move {
                    F::require(&projection, conn, reader, &input, &fresh.prepared.subject).await?;
                    F::check_opened(
                        &projection,
                        conn,
                        reader,
                        &input,
                        &opened.prepared,
                        &fresh.prepared,
                    )
                    .await
                })
            })
            .await?
            .unwrap_or(false))
    } else {
        F::require(ctx, conn, &reader, input, &fresh.prepared.subject).await?;
        F::check_opened(ctx, conn, &reader, input, &opened.prepared, &fresh.prepared).await
    }
}
