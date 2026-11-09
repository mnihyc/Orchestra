//! Typed native preparation and application on the actual guarded connection.
//!
//! Preparation is effect-free and may repeat. It must hold the native writer's complete
//! serialization footprint before producing operands. The native apply method consumes
//! those same operands and invokes the opaque check at its existing first-effect seam.

use super::*;

use crate::key_operations::resolver::OperationData;

use crate::middleware::transaction_guard::Guard;

use crate::registry::OperationFuture;

use crate::registry::OperationInitialFuture;

use std::marker::PhantomData;

use std::sync::atomic::AtomicBool;

use std::sync::atomic::Ordering;
/// Native authenticated-machine entry; the real source/writer and same transaction remain owned by F.
pub async fn apply_owned_from_machine<F: ApplyOwned>(
    app: &crate::state::App,
    tx: orch_db::Tx,
    machine: Id,
    server: Id,
    request_id: Id,
    query: F::Query,
    body: F::Body,
) -> ApiResult<Completed<F::Output>> {
    machine::apply_owned::<F>(app, tx, machine, server, request_id, query, body).await
}

/// One native provider; operation/variant and role facts come from its real prepared intent.
pub trait Effect: Send + Sync + 'static {
    type Source: Eq + Send + Sync + 'static;
    type Query: Send + 'static;
    type Body: Send + 'static;
    type Value: Send + Sync + 'static;
    type Output: Send + 'static;

    fn declaration(value: &Self::Value) -> &'static Declaration<Self::Source>;

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: Self::Query,
        body: Self::Body,
    ) -> SourceFuture<'a, Self::Value, Self::Source>;

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>>;
}

/// Native writers that apply on their caller's existing action connection.
pub trait Apply: Effect {
    /// Reuse the native writer and its already landed opaque first-effect callback.
    /// New native awaits that acquire an additional authority footprint require a
    /// native staged preparation; this interface does not make an arbitrary writer safe.
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Self::Output>>>;
}

/// Native runners that already own their transaction and its commit/cleanup semantics.
pub trait ApplyOwned: Effect {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Self::Value,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Self::Output>>>;
}

struct Operands<F: Effect> {
    value: F::Value,
    response: OperationPlan,
}

/// The native no-op/effect distinction is explicit and preserves its ordinary semantics.
pub enum Outcome<T> {
    Unchanged(T),
    Applied(T),
}

/// A native writer's existing FnOnce callback consumes this at its actual first effect.
/// It grants nothing: full preparation/admission and ordinary authorization already ran
/// after the complete native lock footprint on this same guarded connection.
pub struct FirstEffect<'a> {
    boundary: Boundary<'a>,
    checked: &'a AtomicBool,
}

#[derive(Clone, Copy)]
enum Boundary<'a> {
    Http { ctx: &'a Ctx, guard: &'a Guard },
    Machine(&'a machine::Authority),
}

impl Boundary<'_> {
    fn check(self) -> ApiResult<()> {
        match self {
            Self::Http { ctx, guard } => guard.check_deadline(ctx),
            Self::Machine(guard) => guard.check(),
        }
    }
}

pub struct Deadline<'a> {
    boundary: Boundary<'a>,
}

impl Deadline<'_> {
    pub fn check(&self) -> ApiResult<()> {
        self.boundary.check()
    }
}

impl<'a> FirstEffect<'a> {
    /// Rollbackable SQL writers can retain the same real HTTP/key guard until their final
    /// commit boundary. This does not duplicate the once-only first-effect acknowledgement.
    pub fn deadline(&self) -> Deadline<'a> {
        Deadline {
            boundary: self.boundary,
        }
    }
    pub fn check(self) -> ApiResult<()> {
        self.boundary.check()?;
        self.checked.store(true, Ordering::Release);
        Ok(())
    }
}

/// Typed resolver used for browser, legacy and scoped callers of the same native producer.
/// Query/body types need no Clone or Serialize; guarded checkpoints decode the original input.
pub fn prepare<'a, F: Effect>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: F::Query,
    body: F::Body,
) -> OperationFuture<'a> {
    prepare_with::<F>(ctx, conn, caller, query, body, Preparation::Effect)
}

/// Bind every declared prerequisite before a domain enters its closed creator prefix.
/// Future canonical roles stay unresolved. This returns no effect operands and cannot
/// replace the complete guarded preparation required by either native apply method.
pub fn prepare_entry<'a, F: Effect>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: F::Query,
    body: F::Body,
) -> OperationFuture<'a> {
    prepare_with::<F>(ctx, conn, caller, query, body, Preparation::Prerequisites)
}

#[derive(Clone, Copy)]
enum Preparation {
    Effect,
    Prerequisites,
}

fn prepare_with<'a, F: Effect>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: F::Query,
    body: F::Body,
    preparation: Preparation,
) -> OperationFuture<'a> {
    Box::pin(async move {
        if orch_db::queries::transaction_guard::in_transaction(conn) {
            return prepare_on::<F>(ctx, conn, caller, query, body, preparation).await;
        }
        // The first resolver pass precedes the handler's action transaction. Native
        // checkpoints still require real transactional identity/epoch locks on this
        // connection. Its operands are provisional: the action guard prepares again.
        let mut tx = sqlx::Connection::begin(conn).await.map_err(DbError::from)?;
        let result = prepare_on::<F>(ctx, &mut tx, caller, query, body, preparation).await;
        // Preparation may use rollbackable trials; it never commits a business effect.
        tx.rollback().await.map_err(DbError::from)?;
        if result.is_ok() {
            check_initial_deadline(ctx, caller)?;
        }
        result
    })
}

fn prepare_on<'a, F: Effect>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: F::Query,
    body: F::Body,
    preparation: Preparation,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let restored;
        let caller = if let Some(initial) = initial_frame(ctx)? {
            if let InitialOrigin::Exact(origin) = &initial.origin {
                orch_engine::operation_scope::Prepared::new(ctx.route, origin.operation())
                    .map_err(admission::binding_error)?;
            }
            if initial.origin.user() != caller.user()
                || !authorization::same_credential_origin(ctx.user()?, caller)
            {
                return Err(ApiError::new(
                    orch_api_contract::common::ErrorCode::AuthzDenied,
                    "the native initial origin changed",
                ));
            }
            let original = initial
                .origin
                .restore_current(conn, ctx.app.clock.as_ref())
                .await
                .map_err(authorization::restoration_error)?
                .ok_or_else(|| {
                    ApiError::new(
                        orch_api_contract::common::ErrorCode::AuthzDenied,
                        "the native initial origin is closed",
                    )
                })?;
            restored = caller
                .intersect_restrictions(&original)
                .map_err(ApiError::internal)?;
            &restored
        } else {
            caller
        };
        if initial_frame(ctx)?.is_some() {
            check_initial_deadline(ctx, caller)?;
        }
        entering(ctx, "effect.prepare");
        let (value, facts) = observed(
            ctx,
            "effect.prepare",
            F::prepare(ctx, conn, caller, query, body).await,
        )?;
        let response = observed(
            ctx,
            "effect.bind",
            match preparation {
                Preparation::Effect => bind_required(ctx, F::declaration(&value), &facts),
                Preparation::Prerequisites => orch_engine::operation_scope::sources::bind_declared(
                    ctx.route,
                    F::declaration(&value),
                    &facts,
                )
                .map(OperationPlan::from_shared)
                .map_err(admission::binding_error),
            },
        )?;
        let response = match initial_frame(ctx)? {
            Some(initial) => {
                initial
                    .origin
                    .constrain(ctx, conn, caller, response)
                    .await?
            }
            None => response,
        };
        let principal = observed(
            ctx,
            "effect.principal",
            match preparation {
                Preparation::Effect => preparation_principal(ctx, conn, caller, &response).await,
                Preparation::Prerequisites => {
                    response.prerequisite_principal(ctx, conn, caller).await
                }
            },
        )?;
        observed(
            ctx,
            "effect.require",
            F::require(ctx, conn, &principal, &value).await,
        )?;
        match preparation {
            Preparation::Effect => Ok(response
                .clone()
                .with_data(Operands::<F> { value, response })),
            Preparation::Prerequisites => Ok(response),
        }
    })
}

/// Actual accepted outcome, kept opaque until the domain supplies its native receipt/projector.
pub struct Completed<T> {
    pub ctx: Ctx,
    pub accepted: Caller,
    pub output: T,
}

/// Use inside the existing native action transaction. Its fresh guarded resolver supplies
/// the exact operands; caller-side stale request preparation is never an apply input.
pub async fn apply<F: Apply>(ctx: &Ctx, conn: &mut Conn) -> ApiResult<Completed<F::Output>> {
    let (ctx, guard) = transaction_guard::after_wait(ctx, conn).await?;
    let operands = guard.operation_data::<Operands<F>>()?;
    if operands.response.variant().operation != F::declaration(&operands.value).operation {
        return Err(declaration_error());
    }
    F::require(&ctx, conn, guard.principal(), &operands.value).await?;
    let checked = AtomicBool::new(false);
    let before = FirstEffect {
        boundary: Boundary::Http {
            ctx: &ctx,
            guard: &guard,
        },
        checked: &checked,
    };
    let outcome = F::apply(&ctx, conn, guard.principal(), &operands.value, before).await?;
    finish(ctx, guard, &checked, outcome)
}

/// The native runner keeps ownership of its transaction, commit and accepted cleanup.
pub async fn apply_owned<F: ApplyOwned>(
    ctx: &Ctx,
    mut tx: orch_db::Tx,
) -> ApiResult<Completed<F::Output>> {
    let (ctx, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    let operands = guard.operation_data::<Operands<F>>()?;
    if operands.response.variant().operation != F::declaration(&operands.value).operation {
        return Err(declaration_error());
    }
    F::require(&ctx, &mut tx, guard.principal(), &operands.value).await?;
    let checked = AtomicBool::new(false);
    let before = FirstEffect {
        boundary: Boundary::Http {
            ctx: &ctx,
            guard: &guard,
        },
        checked: &checked,
    };
    let outcome = F::apply_owned(&ctx, tx, guard.principal(), &operands.value, before).await?;
    finish(ctx, guard, &checked, outcome)
}

fn finish<T>(
    ctx: Ctx,
    guard: Guard,
    checked: &AtomicBool,
    outcome: Outcome<T>,
) -> ApiResult<Completed<T>> {
    finish_native(ctx, guard.principal().caller().clone(), checked, outcome)
}

fn finish_native<T>(
    ctx: Ctx,
    accepted: Caller,
    checked: &AtomicBool,
    outcome: Outcome<T>,
) -> ApiResult<Completed<T>> {
    let output = match outcome {
        Outcome::Unchanged(output) => output,
        Outcome::Applied(output) => {
            if !checked.load(Ordering::Acquire) {
                return Err(ApiError::internal(
                    "the native effect did not consume its first-effect check",
                ));
            }
            output
        }
    };
    Ok(Completed {
        accepted,
        ctx,
        output,
    })
}

/// Existing bounded initial IO, authorized before waiting, separate from retryable preparation.
pub trait Initial: Effect {
    type Input: Send + 'static;
    type Fetched: Send + Sync + 'static;

    fn declaration(input: &Self::Input) -> &'static Declaration<Self::Source>;

    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: Self::Query,
        body: Self::Body,
    ) -> SourceFuture<'a, Self::Input, Self::Source>;

    fn require_initial<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<()>>;

    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Self::Input,
    ) -> BoxFuture<'a, ApiResult<Self::Fetched>>;
}

/// The initial supplier returns its native input with its factual source roles.
pub type InitialAuthorization<'a, I, S> = BoxFuture<'a, ApiResult<(I, Vec<Facts<S>>)>>;

/// The same initial phase when archive/native defaults determine the exact variant later.
/// Only identical required Read roles across every canonical candidate may be known here.
pub trait CommonInitial: Effect {
    type Input: Send + 'static;
    type Fetched: Send + Sync + 'static;
    const CANDIDATES: &'static [Declaration<Self::Source>];
    /// Native setup can require known non-Read roles. They remain preparation-only;
    /// every candidate must declare exactly the same supplied canonical prerequisites.
    const NATIVE_PREPARATION: bool = false;

    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: Self::Query,
        body: Self::Body,
    ) -> InitialAuthorization<'a, Self::Input, Self::Source>;

    fn require_initial<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// A common Read proof is not a selected operation and cannot be a deferred effect origin.
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Self::Input,
    ) -> BoxFuture<'a, ApiResult<Self::Fetched>>;
}

struct InitialValue<F: Effect, V: Send + Sync + 'static> {
    value: Arc<V>,
    family: PhantomData<F>,
}

struct InitialFrame {
    origin: InitialOrigin,
    value: OperationData,
}

// Cache a failed fetch as well: neither resolver retry nor another waiter may reopen it.
struct InitialResult(ApiResult<Arc<InitialFrame>>);

enum InitialOrigin {
    Exact(orch_authz::CapturedOperation),
    Common {
        caller: orch_authz::CapturedCaller,
        known: orch_engine::operation_scope::sources::common::Common,
    },
}

impl InitialOrigin {
    fn retain_current(&mut self, current: &Caller) -> ApiResult<()> {
        match self {
            Self::Exact(origin) => {
                *origin = origin.with_ceiling(current).map_err(ApiError::internal)?;
            }
            Self::Common { caller, .. } => {
                *caller = caller.narrowed_with(current).map_err(ApiError::internal)?;
            }
        }
        Ok(())
    }

    fn user(&self) -> Id {
        match self {
            Self::Exact(origin) => origin.user(),
            Self::Common { caller, .. } => caller.user(),
        }
    }

    async fn restore_current(
        &self,
        conn: &mut Conn,
        clock: &dyn orch_types::Clock,
    ) -> Result<Option<Caller>, orch_authz::Error> {
        match self {
            Self::Exact(origin) => origin.restore_current(conn, clock).await,
            Self::Common { caller, .. } => caller.restore_current(conn, clock).await,
        }
    }

    async fn constrain(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        caller: &Caller,
        response: OperationPlan,
    ) -> ApiResult<OperationPlan> {
        match self {
            Self::Exact(origin) => response.with_origin(ctx, conn, caller, origin).await,
            Self::Common { known, .. } => {
                if !known.covers(response.shared()) {
                    return Err(declaration_error());
                }
                // This is only a credential ceiling. The resolved producer still needs every
                // actual required role and its ordinary gate before capture or first effect.
                response.within_caller(ctx, caller)
            }
        }
    }
}

enum InitialBinding {
    Exact(Box<OperationPlan>),
    Common(orch_engine::operation_scope::sources::common::Common),
}

impl InitialBinding {
    async fn principal(&self, ctx: &Ctx, conn: &mut Conn, caller: &Caller) -> ApiResult<Principal> {
        match self {
            Self::Exact(plan) => preparation_principal(ctx, conn, caller, plan).await,
            Self::Common(known) => {
                ctx.checkpoint_disclosure(resolver::Disclosure::Pending);
                let accepted = known
                    .preparation_caller(caller)
                    .map_err(admission::binding_error)?;
                let principal =
                    authorization::load_principal(conn, accepted, ctx.app.clock.time()).await?;
                crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await
            }
        }
    }

    fn capture(self, caller: &Caller) -> ApiResult<(Caller, InitialOrigin)> {
        match self {
            Self::Exact(plan) => {
                let accepted = plan
                    .shared()
                    .preparation_caller(caller)
                    .map_err(admission::binding_error)?;
                let origin = orch_authz::CapturedOperation::capture(&accepted)
                    .map_err(ApiError::internal)?;
                Ok((accepted, InitialOrigin::Exact(origin)))
            }
            Self::Common(known) => {
                let accepted = known
                    .preparation_caller(caller)
                    .map_err(admission::binding_error)?;
                let origin = InitialOrigin::Common {
                    caller: orch_authz::CapturedCaller::capture(&accepted),
                    known,
                };
                Ok((accepted, origin))
            }
        }
    }
}

type Authorize<Q, B, S, I> = for<'a> fn(
    &'a Ctx,
    &'a mut Conn,
    &'a Caller,
    Q,
    B,
) -> BoxFuture<'a, ApiResult<(I, Vec<Facts<S>>)>>;

type RequireInitial<I> =
    for<'a> fn(&'a Ctx, &'a mut Conn, &'a Principal, &'a I) -> BoxFuture<'a, ApiResult<()>>;

type Fetch<I, V> = for<'a> fn(&'a Ctx, &'a Caller, I) -> BoxFuture<'a, ApiResult<V>>;

type BindInitial<S, I> = fn(&Ctx, &I, &[Facts<S>]) -> ApiResult<InitialBinding>;

struct InitialCallbacks<F: Effect, I, V> {
    authorize: Authorize<F::Query, F::Body, F::Source, I>,
    bind: BindInitial<F::Source, I>,
    require: RequireInitial<I>,
    fetch: Fetch<I, V>,
}

/// The Registry's exact-variant initial hook, once per logical request.
pub fn initial<'a, F: Initial>(
    ctx: &'a Ctx,
    caller: &'a Caller,
    query: F::Query,
    body: F::Body,
) -> OperationInitialFuture<'a> {
    initial_with::<F, _, _>(
        ctx,
        caller,
        query,
        body,
        InitialCallbacks {
            authorize: F::authorize,
            bind: |ctx, input, facts| {
                bind_known(ctx, <F as Initial>::declaration(input), facts)
                    .map(|plan| InitialBinding::Exact(Box::new(plan)))
            },
            require: F::require_initial,
            fetch: F::fetch,
        },
    )
}

/// The same once-only Initial with a finite, variant-neutral common Read preparation.
pub fn initial_common<'a, F: CommonInitial>(
    ctx: &'a Ctx,
    caller: &'a Caller,
    query: F::Query,
    body: F::Body,
) -> OperationInitialFuture<'a> {
    initial_with::<F, _, _>(
        ctx,
        caller,
        query,
        body,
        InitialCallbacks {
            authorize: F::authorize,
            bind: |ctx, _, facts| bind_common::<F>(ctx, facts).map(InitialBinding::Common),
            require: F::require_initial,
            fetch: F::fetch,
        },
    )
}

pub(super) fn bind_common<F: CommonInitial>(
    ctx: &Ctx,
    facts: &[Facts<F::Source>],
) -> ApiResult<orch_engine::operation_scope::sources::common::Common> {
    use orch_engine::operation_scope::sources::common::Common;
    let result = if F::NATIVE_PREPARATION {
        Common::bind_preparation(ctx.route, F::CANDIDATES, facts)
    } else {
        Common::bind(ctx.route, F::CANDIDATES, facts)
    };
    result.map_err(admission::binding_error)
}

fn initial_with<'a, F: Effect, I: Send + 'static, V: Send + Sync + 'static>(
    ctx: &'a Ctx,
    caller: &'a Caller,
    query: F::Query,
    body: F::Body,
    callbacks: InitialCallbacks<F, I, V>,
) -> OperationInitialFuture<'a> {
    Box::pin(async move {
        let data = ctx
            .operation_initial
            .get_or_init(|| async move {
                let result = async move {
                    let mut tx = ctx.app.db.begin_snapshot().await?;
                    let identity = transaction_guard::current_http_identity(
                        ctx,
                        &mut tx,
                        HttpIdentityMode::Read,
                    )
                    .await?;
                    transaction_guard::guard_user(&identity.user)?;
                    let caller = identity
                        .caller
                        .intersect_restrictions(caller)
                        .map_err(ApiError::internal)?;
                    let current =
                        ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
                    entering(&current, "initial.authorize");
                    let (input, facts) = observed(
                        &current,
                        "initial.authorize",
                        (callbacks.authorize)(&current, &mut tx, &caller, query, body).await,
                    )?;
                    let known = observed(
                        &current,
                        "initial.bind",
                        (callbacks.bind)(&current, &input, &facts),
                    )?;
                    let principal = observed(
                        &current,
                        "initial.principal",
                        known.principal(&current, &mut tx, &caller).await,
                    )?;
                    observed(
                        &current,
                        "initial.require",
                        (callbacks.require)(&current, &mut tx, &principal, &input).await,
                    )?;
                    let (accepted, mut origin) = known.capture(principal.caller())?;
                    ctx.publication.retain_ceiling(&accepted)?;
                    tx.commit().await.map_err(DbError::from)?;
                    identity.check_deadline(&ctx.app)?;
                    check_initial_deadline(ctx, &accepted)?;
                    entering(&current, "initial.fetch");
                    let value = observed(
                        &current,
                        "initial.fetch",
                        (callbacks.fetch)(&current, &accepted, input).await,
                    )?;
                    identity.check_deadline(&ctx.app)?;
                    check_initial_deadline(ctx, &accepted)?;
                    // Check the actual bearer again after the native source wait. Preserve every
                    // observed current ceiling and the original raw deadline in the same opening.
                    let mut tx = ctx.app.db.begin_snapshot().await?;
                    let fresh = transaction_guard::current_http_identity(
                        ctx,
                        &mut tx,
                        HttpIdentityMode::Read,
                    )
                    .await?;
                    transaction_guard::guard_user(&fresh.user)?;
                    let restored = origin
                        .restore_current(&mut tx, ctx.app.clock.as_ref())
                        .await
                        .map_err(authorization::restoration_error)?
                        .ok_or_else(initial_closed)?;
                    let current = fresh
                        .caller
                        .intersect_restrictions(&restored)
                        .map_err(ApiError::internal)?;
                    if let InitialOrigin::Common { known, .. } = &origin {
                        let _ = known
                            .preparation_caller(&current)
                            .map_err(admission::binding_error)?;
                    }
                    origin.retain_current(&current)?;
                    ctx.publication.retain_ceiling(&current)?;
                    tx.commit().await.map_err(DbError::from)?;
                    identity.check_deadline(&ctx.app)?;
                    fresh.check_deadline(&ctx.app)?;
                    check_initial_deadline(ctx, &current)?;
                    Ok(Arc::new(InitialFrame {
                        origin,
                        value: OperationData::new(InitialValue::<F, V> {
                            value: Arc::new(value),
                            family: PhantomData,
                        }),
                    }))
                }
                .await;
                OperationData::new(InitialResult(result))
            })
            .await;
        let result = data.get::<InitialResult>()?;
        let frame = result.0.as_ref().map_err(Clone::clone)?;
        let _ = frame.value.get::<InitialValue<F, V>>()?;
        Ok(())
    })
}

fn initial_closed() -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::AuthzDenied,
        "the native initial origin is closed",
    )
}

fn check_initial_deadline(ctx: &Ctx, caller: &Caller) -> ApiResult<()> {
    if !caller.key_deadline_allows(ctx.app.clock.as_ref()) {
        return Err(initial_closed());
    }
    if ctx
        .publication
        .retained_deadline()?
        .is_some_and(|end| ctx.app.clock.time().get() >= end.get())
    {
        return Err(initial_closed());
    }
    Ok(())
}

fn initial_frame(ctx: &Ctx) -> ApiResult<Option<Arc<InitialFrame>>> {
    ctx.operation_initial
        .get()
        .map(|data| {
            data.get::<InitialResult>()?
                .0
                .as_ref()
                .map(Arc::clone)
                .map_err(Clone::clone)
        })
        .transpose()
}

fn fetched<F: Effect, V: Send + Sync + 'static>(ctx: &Ctx) -> ApiResult<Arc<V>> {
    let frame = initial_frame(ctx)?
        .ok_or_else(|| ApiError::internal("the native initial operand is unavailable"))?;
    Ok(Arc::clone(&frame.value.get::<InitialValue<F, V>>()?.value))
}

/// Native preparation sees only its own immutable once-only initial operand.
pub fn initial_value<F: Initial>(ctx: &Ctx) -> ApiResult<Arc<F::Fetched>> {
    fetched::<F, F::Fetched>(ctx)
}

/// The resolved native producer consumes the same opened operand, without choosing a slot here.
pub fn common_initial_value<F: CommonInitial>(ctx: &Ctx) -> ApiResult<Arc<F::Fetched>> {
    fetched::<F, F::Fetched>(ctx)
}

#[path = "effects/machine.rs"]
pub mod machine;

#[cfg(any(test, feature = "test-support"))]
#[path = "effects/common_initial_tests.rs"]
pub mod common_initial_tests;

#[cfg(any(test, feature = "test-support"))]
#[path = "effects/entry_tests.rs"]
pub mod entry_tests;
