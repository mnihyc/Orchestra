//! Exact captured-operation checkpoints over current typed native sources, without HTTP context.
//! Native owners keep their lock footprint, durable association, once-only IO and accepted cleanup.

use super::{
    BindingError, Prepared, account_policy,
    http_origin::HttpOrigin,
    sources::{self, Facts},
};
use crate::EngineError;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::routes::Route;
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::{Conn, DbError, queries::transaction_guard};
use orch_types::{Clock, Id, Time};
use std::collections::BTreeSet;
use std::sync::Arc;

pub mod preparation;

#[derive(Clone, Copy)]
enum Phase {
    Complete,
    Preparation,
}
impl Phase {
    fn covers(self, current: &Prepared, earlier: &Prepared) -> bool {
        match self {
            Self::Complete => current.covers_required_sources(earlier),
            Self::Preparation => current.covers_known_required_sources(earlier),
        }
    }
}

#[derive(Clone)]
enum Transport {
    Current(Option<Arc<HttpOrigin>>),
    Checkpoint(Arc<HttpOrigin>),
}
impl Transport {
    fn http(&self) -> Option<&Arc<HttpOrigin>> {
        match self {
            Self::Current(http) => http.as_ref(),
            Self::Checkpoint(http) => Some(http),
        }
    }
    fn resolve(self) -> Result<Self, Error> {
        match self {
            Self::Current(retained) => {
                let http = match (retained, super::http_origin::active()) {
                    (Some(old), Some(current)) => Some(Arc::new(old.intersect(&current)?)),
                    (old, current) => old.or(current),
                };
                Ok(Self::Current(http))
            }
            checkpoint => Ok(checkpoint),
        }
    }
    fn accepted(self, http: Option<Arc<HttpOrigin>>) -> Result<Self, Error> {
        match self {
            Self::Current(_) => Ok(Self::Current(http)),
            Self::Checkpoint(_) => {
                Ok(Self::Checkpoint(http.ok_or_else(|| {
                    refused_at("checkpoint", "transport.missing")
                })?))
            }
        }
    }
}

/// A known original operation that closed never becomes an unclassified or ordinary caller.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Current original identity, source association, scope or ordinary standing refused work.
    #[error("the original operation no longer permits this native work")]
    Refused,
    /// The shared canonical source binding failed.
    #[error(transparent)]
    Binding(#[from] BindingError),
    /// The existing native factory or account policy failed.
    #[error(transparent)]
    Native(#[from] EngineError),
}

/// Opt-in native test diagnostics contain only the operation and the failed checkpoint.
pub(crate) fn diagnostic(operation: &str, checkpoint: &'static str, error: Error) -> Error {
    if std::env::var_os("ORCH_TEST_NATIVE_OPERATIONS").is_some() {
        let outcome = match &error {
            Error::Refused => "refused",
            Error::Binding(BindingError::Refused) => "scope_refused",
            Error::Binding(_) => "binding_error",
            Error::Native(_) => "native_error",
        };
        eprintln!("native operation {operation}: checkpoint={checkpoint} outcome={outcome}");
    }
    error
}

fn refused_at(operation: &str, checkpoint: &'static str) -> Error {
    diagnostic(operation, checkpoint, Error::Refused)
}
impl From<DbError> for Error {
    fn from(error: DbError) -> Self {
        Self::Native(error.into())
    }
}
impl From<orch_authz::Error> for Error {
    fn from(error: orch_authz::Error) -> Self {
        Self::Native(error.into())
    }
}

/// Native operands together with the factual bindings of their source roles.
pub type PreparedSource<V, R> = (V, Vec<Facts<R>>);
/// The prepared native source a scoped continuation resolves to.
pub type PreparedFuture<'a, V, R> = BoxFuture<'a, Result<PreparedSource<V, R>, Error>>;

/// One existing native source factory, with the same declaration on HTTP and later work.
/// Preparation and validation are effect-free; model/connector/stream IO is started by the
/// owner once outside retries, and every later effect/publication obtains a fresh frame.
pub trait Source: Send + Sync + 'static {
    /// The domain's server-owned roles beside its canonical operation contract.
    type Role: Eq + Send + Sync + 'static;
    /// Immutable actual native job/generation/request association, never caller authority.
    type Input: Send + Sync + 'static;
    /// Actual selected operands, retained for the native writer and later association checks.
    type Value: Send + Sync + 'static;
    /// The native stage's typed canonical endpoint, independent of the current transport.
    const ROUTE: Route;
    /// The default canonical source row; variants come only from actual native preparation.
    const DECLARATION: &'static Declaration<Self::Role>;

    /// Select the exact declaration from actual immutable native intent/operands.
    fn declaration(_input: &Self::Input, _value: &Self::Value) -> &'static Declaration<Self::Role> {
        Self::DECLARATION
    }
    /// Reuse the effect-free native selector; the common frame repeats it after native locks.
    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Self::Input,
        now: Time,
    ) -> PreparedFuture<'a, Self::Value, Self::Role>;
    /// The existing native ordinary prerequisite, including current system standing where
    /// that native act requires it. Scope admission never supplies that standing/private mode.
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, Result<(), Error>>;
    /// The existing complete native operand lock footprint, after the preliminary ordinary
    /// gate. This may wait but performs no business mutation or external dispatch. The common
    /// frame then restores/reselects/rechecks before offering its first-effect cut.
    fn locks<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, Result<(), Error>>;
    /// Validate the actual original native association, never render a replacement result or
    /// dispatch work. A new generation/runtime/subject cannot borrow the previous frame.
    fn check_opened<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        opened: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, Result<bool, Error>>;
}

/// Current required native admission on the owner's still-open guarded transaction.
#[must_use = "use the current native operands or validate their original source"]
pub struct Frame<F: Source> {
    principal: Principal,
    raw_until: Option<Time>,
    opened: Opened<F>,
}

/// Immutable native source witness across waits. This supplies no reusable admission.
/// Durable native carriers persist the exact captured origin/actual association separately;
/// this in-memory witness is neither Serialize nor a substitute for fresh preparation.
#[must_use = "retain the actual native source for the next continuing boundary"]
pub struct Opened<F: Source> {
    origin: CapturedOperation,
    ceiling: Option<Caller>,
    input: Arc<F::Input>,
    value: Arc<F::Value>,
    plan: Prepared,
    phase: Phase,
    transport: Transport,
}

/// One synchronous live-key cut, consumed by the native first-effect callback after all locks.
/// Revocation/policy/ordinary authority is pinned/watched by the frame transaction, not this cut.
#[must_use = "consume the check at the native first effect"]
pub struct FirstEffect {
    caller: Caller,
    raw_until: Option<Time>,
}
impl FirstEffect {
    /// Invoke immediately before the native writer's actual first effect; never after releasing
    /// the frame transaction. A later new effect after another wait requires Opened::refresh.
    pub fn check(self, clock: &dyn Clock) -> Result<(), Error> {
        if self.caller.key_deadline_allows(clock)
            && self.raw_until.is_none_or(|end| clock.time() < end)
        {
            Ok(())
        } else {
            Err(refused_at(
                self.caller.admitted_operation().unwrap_or("unadmitted"),
                "first_effect.deadline",
            ))
        }
    }
}
impl std::fmt::Debug for FirstEffect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FirstEffect([opaque])")
    }
}

/// Actual native operands plus the once-only first-effect cut from their current frame.
#[must_use = "the native writer must consume its actual operands and first-effect check"]
pub struct Parts<F: Source> {
    /// Current original/current-intersected ordinary actor, not a user-id Session fallback.
    pub principal: Principal,
    /// The original immutable native association.
    pub input: Arc<F::Input>,
    /// The same fresh operands the factual scope gate admitted.
    pub value: Arc<F::Value>,
    /// Consume at the existing native first-effect seam on this same transaction.
    pub first_effect: FirstEffect,
    /// Reprepare/validate the original sources before later work or old-result publication.
    pub opened: Opened<F>,
}

impl<F: Source> Frame<F> {
    /// Actual accepted raw identity/minima when this frame originated in HTTP.
    pub fn http_origin(&self) -> Option<&HttpOrigin> {
        self.opened.transport.http().map(Arc::as_ref)
    }
    /// The actual fresh native ordinary actor.
    pub fn principal(&self) -> &Principal {
        &self.principal
    }
    /// The exact fresh native operands.
    pub fn value(&self) -> &F::Value {
        &self.opened.value
    }
    /// Keep the earliest actual accepted/current key deadline across every retained key.
    pub fn check_deadline(&self, clock: &dyn Clock) -> Result<(), Error> {
        if self.principal.caller().key_deadline_allows(clock)
            && self.raw_until.is_none_or(|end| clock.time() < end)
        {
            Ok(())
        } else {
            Err(refused_at(
                self.principal
                    .caller()
                    .admitted_operation()
                    .unwrap_or("unadmitted"),
                "frame.deadline",
            ))
        }
    }
    /// Give the native writer its actual operands and one linear first-effect check.
    pub fn into_parts(self) -> Parts<F> {
        Parts {
            input: Arc::clone(&self.opened.input),
            value: Arc::clone(&self.opened.value),
            first_effect: FirstEffect {
                caller: self.principal.caller().clone(),
                raw_until: self.raw_until,
            },
            principal: self.principal,
            opened: self.opened,
        }
    }
}

impl<F: Source> Opened<F> {
    /// Exact native provenance; it carries no raw transport identity or reusable target proof.
    pub fn origin(&self) -> &CapturedOperation {
        &self.origin
    }

    /// Fresh original operation/current authority with the same original native input/value.
    /// No writer, native renderer, IO supplier or accepted cleanup is replayed here.
    pub async fn refresh(&self, conn: &mut Conn, clock: &dyn Clock) -> Result<Frame<F>, Error> {
        self.refresh_with_ceiling(conn, clock, self.ceiling.clone())
            .await
    }

    /// Add a freshly restored transport/other genuine caller ceiling before native preparation.
    /// This does not replace the original actor, operation, immutable input or old source value.
    pub async fn refresh_with_caller(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        caller: &Caller,
    ) -> Result<Frame<F>, Error> {
        let ceiling = match &self.ceiling {
            Some(old) => old
                .intersect_restrictions(caller)
                .map_err(|_| refused_at(self.origin.operation(), "refresh.ceiling"))?,
            None => caller.clone(),
        };
        self.refresh_with_ceiling(conn, clock, Some(ceiling)).await
    }

    async fn refresh_with_ceiling(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        ceiling: Option<Caller>,
    ) -> Result<Frame<F>, Error> {
        let fresh = prepare_input::<F>(
            conn,
            clock,
            &self.origin,
            Arc::clone(&self.input),
            ceiling,
            self.phase,
            self.transport.clone(),
        )
        .await?;
        if !self.phase.covers(&fresh.opened.plan, &self.plan)
            || !self.phase.covers(&self.plan, &fresh.opened.plan)
            || !F::check_opened(
                conn,
                &fresh.principal,
                &self.input,
                &self.value,
                &fresh.opened.value,
            )
            .await?
        {
            return Err(refused_at(self.origin.operation(), "refresh.sources"));
        }
        fresh.check_deadline(clock)?;
        Ok(fresh)
    }
}

/// Prepare the exact captured stage on a real native transaction. Identity/preliminary admission
/// precedes the factory's native locks; fresh admission follows every lock wait. The owner keeps
/// this transaction through the first effect. This never commits/retries an effect or IO.
pub async fn prepare<F: Source>(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &CapturedOperation,
    input: F::Input,
) -> Result<Frame<F>, Error> {
    prepare_input::<F>(
        conn,
        clock,
        origin,
        Arc::new(input),
        None,
        Phase::Complete,
        Transport::Current(None),
    )
    .await
}

/// Recheck a saved request through its own captured transport, independently of an enclosing act.
/// The enclosing request keeps its account and current raw/key fences before and after this cut.
/// A frame's ordinary preparation and refresh still require their own exact transport namespace.
pub async fn checkpoint<F: Source>(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &HttpOrigin,
    input: F::Input,
) -> Result<Frame<F>, Error> {
    prepare_input::<F>(
        conn,
        clock,
        origin.origin(),
        Arc::new(input),
        None,
        Phase::Complete,
        Transport::Checkpoint(Arc::new(origin.clone())),
    )
    .await
}

/// The same native source frame with an actual additional restored caller ceiling.
/// Transport owners keep their own real credential locks and raw lifetime on this connection.
pub async fn prepare_with_caller<F: Source>(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &CapturedOperation,
    caller: &Caller,
    input: F::Input,
) -> Result<Frame<F>, Error> {
    prepare_input::<F>(
        conn,
        clock,
        origin,
        Arc::new(input),
        Some(caller.clone()),
        Phase::Complete,
        Transport::Current(None),
    )
    .await
}

async fn prepare_input<F: Source>(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &CapturedOperation,
    input: Arc<F::Input>,
    ceiling: Option<Caller>,
    phase: Phase,
    transport: Transport,
) -> Result<Frame<F>, Error> {
    let enclosing = match &transport {
        Transport::Checkpoint(_) => super::http_origin::active(),
        Transport::Current(_) => None,
    };
    if let Some(enclosing) = &enclosing {
        if !transaction_guard::in_transaction(conn) {
            return Err(EngineError::unreadable(
                "native operation preparation requires its transaction",
            )
            .into());
        }
        transaction_guard::lock_action(conn).await?;
        if enclosing.origin().user() != origin.user() {
            return Err(refused_at(origin.operation(), "checkpoint.actor"));
        }
        enclosing.current(conn, clock).await?.check(clock)?;
    }
    let prepared = prepare_frame::<F>(
        conn,
        clock,
        origin,
        input,
        ceiling,
        phase,
        transport.resolve()?,
    )
    .await;
    // Recheck the enclosing identity and restore its audit mode even on a refused checkpoint.
    // Its active transport remains untouched; the saved frame admits only its own operation.
    if let Some(enclosing) = enclosing {
        enclosing.current(conn, clock).await?.check(clock)?;
    }
    let frame = prepared?;
    frame.check_deadline(clock)?;
    Ok(frame)
}

async fn prepare_frame<F: Source>(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &CapturedOperation,
    input: Arc<F::Input>,
    ceiling: Option<Caller>,
    phase: Phase,
    transport: Transport,
) -> Result<Frame<F>, Error> {
    let earlier = load::<F>(
        conn,
        clock,
        origin,
        Arc::clone(&input),
        ceiling.as_ref(),
        phase,
        transport,
    )
    .await?;
    F::locks(conn, &earlier.principal, &input, &earlier.opened.value)
        .await
        .map_err(|error| diagnostic(origin.operation(), "native.locks", error))?;
    let fresh = load::<F>(
        conn,
        clock,
        origin,
        Arc::clone(&input),
        ceiling.as_ref(),
        phase,
        earlier.opened.transport.clone(),
    )
    .await?;
    if !phase.covers(&fresh.opened.plan, &earlier.opened.plan)
        || !phase.covers(&earlier.opened.plan, &fresh.opened.plan)
        || !F::check_opened(
            conn,
            &fresh.principal,
            &input,
            &earlier.opened.value,
            &fresh.opened.value,
        )
        .await?
    {
        return Err(refused_at(origin.operation(), "locked.sources"));
    }
    fresh.check_deadline(clock)?;
    Ok(fresh)
}

async fn load<F: Source>(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &CapturedOperation,
    input: Arc<F::Input>,
    ceiling: Option<&Caller>,
    phase: Phase,
    transport: Transport,
) -> Result<Frame<F>, Error> {
    let http = transport.http().cloned();
    if !transaction_guard::in_transaction(conn) {
        return Err(EngineError::unreadable(
            "native operation preparation requires its transaction",
        )
        .into());
    }
    let stage = Prepared::new(F::ROUTE, origin.operation())?;
    transaction_guard::lock_action(conn).await?;
    let user = transaction_guard::lock_native_actor(conn, origin.user())
        .await?
        .ok_or_else(|| refused_at(origin.operation(), "load.actor_missing"))?;
    let caller = origin
        .restore_current(conn, clock)
        .await?
        .ok_or_else(|| refused_at(origin.operation(), "load.origin_unavailable"))?;
    let (caller, raw_until, session) = match &http {
        Some(http) => {
            if http.origin().user() != origin.user()
                || http.origin().operation() != origin.operation()
            {
                return Err(refused_at(origin.operation(), "load.transport_namespace"));
            }
            let current = http
                .current(conn, clock)
                .await
                .map_err(|error| diagnostic(origin.operation(), "load.http_current", error))?;
            (
                caller
                    .intersect_restrictions(current.caller())
                    .map_err(|_| refused_at(origin.operation(), "load.transport_intersection"))?,
                current.until(),
                http.session(),
            )
        }
        None => (caller, None, None),
    };
    let caller = match ceiling {
        Some(ceiling) => {
            let current = orch_authz::CapturedCaller::capture(ceiling)
                .restore_current(conn, clock)
                .await?
                .ok_or_else(|| refused_at(origin.operation(), "load.ceiling_unavailable"))?;
            caller
                .intersect_restrictions(&current)
                .map_err(|_| refused_at(origin.operation(), "load.ceiling_intersection"))?
        }
        None => caller,
    };
    if caller.user() != user.id
        || caller.grant().is_some_and(|grant| {
            grant.token_ids().next().is_none() && caller.native_machine().is_none()
        })
    {
        return Err(refused_at(origin.operation(), "load.caller_identity"));
    }
    stage
        .compatible(&caller)
        .map_err(|error| diagnostic(origin.operation(), "load.compatible", error.into()))?;
    let (value, facts) = F::prepare(conn, &caller, &input, clock.time())
        .await
        .map_err(|error| diagnostic(origin.operation(), "native.prepare", error))?;
    let row = F::declaration(&input, &value);
    if row.operation != origin.operation() {
        return Err(refused_at(origin.operation(), "load.declaration_operation"));
    }
    let (plan, caller) =
        match phase {
            Phase::Complete => {
                let plan = sources::bind_required(F::ROUTE, row, &facts).map_err(|error| {
                    diagnostic(origin.operation(), "load.bind_required", error.into())
                })?;
                let caller = plan
                    .admit(&caller, false)
                    .map_err(|error| diagnostic(origin.operation(), "load.admit", error.into()))?;
                (plan, caller)
            }
            Phase::Preparation => {
                // A native phase declares every source it actually needs. Later canonical
                // roles remain unknown; optional response content is not a preparation effect.
                if row.bindings.iter().any(|binding| {
                    binding.usage != orch_api_contract::operations::FacetUsage::Required
                }) {
                    return Err(refused_at(origin.operation(), "preparation.response_role"));
                }
                let plan = sources::bind_declared(F::ROUTE, row, &facts)?;
                if !plan.covers_known_required_sources(&plan) {
                    return Err(refused_at(
                        origin.operation(),
                        "preparation.source_coverage",
                    ));
                }
                let caller = plan.preparation_caller(&caller)?;
                (plan, caller)
            }
        };
    let principal = Principal::load(conn, caller, clock.time()).await?;
    if principal.caller().admitted_operation() != Some(origin.operation()) {
        return Err(refused_at(origin.operation(), "load.admitted_operation"));
    }
    let actual = match phase {
        Phase::Complete => plan.required_facets()?,
        Phase::Preparation => plan.known_required_facets(),
    };
    let principal = hold_principal(conn, &user, session, principal, &actual).await?;
    F::require(conn, &principal, &input, &value)
        .await
        .map_err(|error| diagnostic(origin.operation(), "native.require", error))?;
    let retained_origin = if http.is_some() {
        origin
            .narrowed_with(principal.caller())
            .map_err(|_| refused_at(origin.operation(), "load.retained_origin"))?
    } else {
        origin.clone()
    };
    let http = http
        .map(|http| http.accepted(principal.caller(), raw_until).map(Arc::new))
        .transpose()?;
    let frame = Frame {
        principal,
        raw_until,
        opened: Opened {
            origin: retained_origin,
            ceiling: ceiling.cloned(),
            input,
            value: Arc::new(value),
            plan,
            phase,
            transport: transport.accepted(http)?,
        },
    };
    frame.check_deadline(clock)?;
    Ok(frame)
}

/// The existing native account/session setup hold on an already resolved factual scope.
pub(crate) async fn hold_principal(
    conn: &mut Conn,
    user: &orch_db::queries::identity::UserRow,
    session: Option<Id>,
    mut principal: Principal,
    actual: &[orch_access_contract::key_scope::KeyFacet],
) -> Result<Principal, Error> {
    if principal.user() != user.id {
        return Err(refused_at("account", "hold.actor"));
    }
    if let Some(hold) = account_policy::session_hold(conn, user, session).await? {
        principal = principal.held_for_second_step(hold);
    }
    if let Some(hold) = principal.second_step_hold() {
        let held = hold.orgs.iter().copied().collect::<BTreeSet<_>>();
        if actual.iter().any(|facet| {
            facet.target.node_ancestors.iter().any(|node| {
                node.kind == KeyTargetKind::Object(ObjectKind::Org)
                    && node.id.is_some_and(|id| held.contains(&id))
            })
        }) {
            return Err(refused_at("account", "hold.second_step"));
        }
    }
    Ok(principal)
}
