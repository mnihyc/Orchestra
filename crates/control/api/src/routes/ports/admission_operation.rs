//! Typed native admission operands; volatile tickets are minted once under the final guard.

use super::*;
use crate::key_operations::{
    native::{
        self,
        effects::{self, Apply, Effect, FirstEffect, Outcome},
    },
    targets,
};
use crate::realtime::ticket_origin::Origin;
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    operation_sources::Declaration,
    port_sources::{self as sources, Source},
};
use orch_authz::Caller;

pub(crate) struct Use;
pub(crate) struct Enter;
pub(crate) struct Admission {
    environment: EnvironmentFull,
    port: u16,
    transport: PortTransport,
    opening: Option<Id>,
    browser: bool,
}

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    transport: PortTransport,
    opening: Option<Id>,
    browser: bool,
) -> ApiResult<(Admission, Vec<native::Facts<Source>>)> {
    let id = ctx.id("id")?;
    let port = port(ctx)?;
    let environment = environment(conn, id).await?;
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id,
        },
    )
    .await?;
    Ok((
        Admission {
            environment,
            port,
            transport,
            opening,
            browser,
        },
        vec![native::Facts::one(
            Source::AdmissionEnvironment,
            KeyTargetKind::Object(ObjectKind::Environment),
            target,
        )],
    ))
}

async fn require(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Admission,
) -> ApiResult<()> {
    may_use(
        conn,
        principal,
        &value.environment,
        value.port,
        value.transport,
        value.opening,
    )
    .await?;
    if value.browser {
        if address(ctx, value.environment.id, value.port).is_none() {
            return Err(ApiError::new(
                ErrorCode::PortsUnavailable,
                "this installation has no ports domain: use the helper, or Here",
            ));
        }
    } else {
        listening(ctx, &value.environment, value.port, value.transport)?;
    }
    Ok(())
}

impl Effect for Use {
    type Source = Source;
    type Query = ();
    type Body = EnvironmentPortUseCreateBody;
    type Value = Admission;
    type Output = crate::middleware::idempotency::memory::Reservation<PortUse>;
    fn declaration(_value: &Admission) -> &'static Declaration<Source> {
        &sources::PORT_USE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Admission, Vec<native::Facts<Source>>)>> {
        Box::pin(async move { prepare(ctx, conn, body.transport, body.opening_id, false).await })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Admission,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(ctx, conn, principal, value))
    }
}
impl Apply for Use {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Admission,
        before: FirstEffect<'a>,
    ) -> BoxFuture<
        'a,
        ApiResult<Outcome<crate::middleware::idempotency::memory::Reservation<PortUse>>>,
    > {
        Box::pin(async move {
            let expires = Time::new(
                principal.now().get()
                    + chrono::Duration::seconds(crate::ports_runtime::USE_SECONDS),
            );
            let proof = entry_proof();
            let id = new_id();
            let usage = ctx.app.cache.ports().admit_before(
                ctx,
                &value.environment,
                value.port,
                value.transport,
                value.opening,
                proof,
                expires,
                id,
                move || before.check(),
            )?;
            let app = ctx.app.clone();
            let mut usage =
                crate::middleware::idempotency::memory::Reservation::new(id, usage, move || {
                    app.cache.ports().discard_use(id);
                });
            crate::routes::interactive_tickets::record::<
                crate::routes::interactive_tickets::PortUses,
            >(&mut usage, ctx, conn)
            .await?;
            Ok(Outcome::Applied(usage))
        })
    }
}
impl Effect for Enter {
    type Source = Source;
    type Query = ();
    type Body = EnvironmentPortEnterBody;
    type Value = Admission;
    type Output = crate::middleware::idempotency::memory::Reservation<EnvironmentPortEnterReply>;
    fn declaration(_value: &Admission) -> &'static Declaration<Source> {
        &sources::PORT_ENTER
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Admission, Vec<native::Facts<Source>>)>> {
        Box::pin(async move { prepare(ctx, conn, PortTransport::Tcp, body.opening_id, true).await })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Admission,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(ctx, conn, principal, value))
    }
}
impl Apply for Enter {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Admission,
        before: FirstEffect<'a>,
    ) -> BoxFuture<
        'a,
        ApiResult<
            Outcome<crate::middleware::idempotency::memory::Reservation<EnvironmentPortEnterReply>>,
        >,
    > {
        Box::pin(async move {
            let address = address(ctx, value.environment.id, value.port).ok_or_else(|| {
                ApiError::new(
                    ErrorCode::PortsUnavailable,
                    "this installation has no ports domain",
                )
            })?;
            let origin = Origin::capture(ctx)?;
            let now = principal.now();
            let expires_at =
                Time::new(now.get() + chrono::Duration::seconds(crate::ports_runtime::USE_SECONDS));
            let id = new_id();
            let entry = crate::ports_runtime::BrowserEntry {
                id,
                person: Some(origin.user()),
                origin: Some(origin),
                environment: value.environment.id,
                port: value.port,
                opening: value.opening,
                expires: expires_at,
            };
            let code = ctx
                .app
                .cache
                .ports()
                .enter_code_before(entry, now, move || before.check())?;
            let reply = EnvironmentPortEnterReply {
                url: format!("{address}{}?code={code}", crate::ports_browser::ENTER_PATH),
                expires_at,
            };
            let app = ctx.app.clone();
            let mut entered =
                crate::middleware::idempotency::memory::Reservation::new(id, reply, move || {
                    app.cache.ports().discard_code(&code, id);
                });
            crate::routes::interactive_tickets::record::<
                crate::routes::interactive_tickets::PortEntries,
            >(&mut entered, ctx, conn)
            .await?;
            Ok(Outcome::Applied(entered))
        })
    }
}

async fn run<F: Apply<Value = Admission>>(ctx: &Ctx) -> ApiResult<effects::Completed<F::Output>> {
    let (mut tx, initial) = transaction_guard::begin(ctx).await?;
    initial.check_deadline(ctx)?;
    if !rows::lock_environment(&mut tx, ctx.id("id")?).await? {
        return Err(ApiError::not_found());
    }
    let result = effects::apply::<F>(ctx, &mut tx).await?;
    // The same native reservation's Drop owns cancellation and every commit failure.
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(result)
}

pub(super) async fn use_port(ctx: Ctx) -> ApiResult<PortUseCreateResponse> {
    Ok(PortUseCreateResponse::full(
        run::<Use>(&ctx).await?.output.publish()?,
    ))
}
pub(crate) async fn enter(ctx: Ctx) -> ApiResult<EnvironmentPortEnterResponse> {
    Ok(EnvironmentPortEnterResponse::full(
        run::<Enter>(&ctx).await?.output.publish()?,
    ))
}
