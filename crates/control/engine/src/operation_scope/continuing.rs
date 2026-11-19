//! Exact captured-operation checkpoints over current typed native sources, without HTTP context.
//! Native owners keep their lock footprint, durable association, once-only IO and accepted cleanup.

use super::{
    BindingError, Prepared, account_policy,
    sources::{self, Facts},
};
use crate::EngineError;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::routes::Route;
use orch_authz::suspension::SecondStepHold;
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::{Conn, DbError, queries::transaction_guard};
use orch_types::{Clock, Time};
use std::collections::BTreeSet;
use std::sync::Arc;

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
    ) -> BoxFuture<'a, Result<(Self::Value, Vec<Facts<Self::Role>>), Error>>;
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
}

/// One synchronous live-key cut, consumed by the native first-effect callback after all locks.
/// Revocation/policy/ordinary authority is pinned/watched by the frame transaction, not this cut.
#[must_use = "consume the check at the native first effect"]
pub struct FirstEffect {
    caller: Caller,
}
impl FirstEffect {
    /// Invoke immediately before the native writer's actual first effect; never after releasing
    /// the frame transaction. A later new effect after another wait requires Opened::refresh.
    pub fn check(self, clock: &dyn Clock) -> Result<(), Error> {
        if self.caller.key_deadline_allows(clock) {
            Ok(())
        } else {
            Err(Error::Refused)
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
        if self.principal.caller().key_deadline_allows(clock) {
            Ok(())
        } else {
            Err(Error::Refused)
        }
    }
    /// Give the native writer its actual operands and one linear first-effect check.
    pub fn into_parts(self) -> Parts<F> {
        Parts {
            input: Arc::clone(&self.opened.input),
            value: Arc::clone(&self.opened.value),
            first_effect: FirstEffect {
                caller: self.principal.caller().clone(),
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
                .map_err(|_| Error::Refused)?,
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
        let fresh =
            prepare_input::<F>(conn, clock, &self.origin, Arc::clone(&self.input), ceiling).await?;
        if !fresh.opened.plan.covers_required_sources(&self.plan)
            || !self.plan.covers_required_sources(&fresh.opened.plan)
            || !F::check_opened(
                conn,
                &fresh.principal,
                &self.input,
                &self.value,
                &fresh.opened.value,
            )
            .await?
        {
            return Err(Error::Refused);
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
    prepare_input::<F>(conn, clock, origin, Arc::new(input), None).await
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
    prepare_input::<F>(conn, clock, origin, Arc::new(input), Some(caller.clone())).await
}

async fn prepare_input<F: Source>(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &CapturedOperation,
    input: Arc<F::Input>,
    ceiling: Option<Caller>,
) -> Result<Frame<F>, Error> {
    let earlier = load::<F>(conn, clock, origin, Arc::clone(&input), ceiling.as_ref()).await?;
    F::locks(conn, &earlier.principal, &input, &earlier.opened.value).await?;
    let fresh = load::<F>(conn, clock, origin, Arc::clone(&input), ceiling.as_ref()).await?;
    if !fresh
        .opened
        .plan
        .covers_required_sources(&earlier.opened.plan)
        || !earlier
            .opened
            .plan
            .covers_required_sources(&fresh.opened.plan)
        || !F::check_opened(
            conn,
            &fresh.principal,
            &input,
            &earlier.opened.value,
            &fresh.opened.value,
        )
        .await?
    {
        return Err(Error::Refused);
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
) -> Result<Frame<F>, Error> {
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
        .ok_or(Error::Refused)?;
    let caller = origin
        .restore_current(conn, clock)
        .await?
        .ok_or(Error::Refused)?;
    let caller = match ceiling {
        Some(ceiling) => {
            let current = orch_authz::CapturedCaller::capture(ceiling)
                .restore_current(conn, clock)
                .await?
                .ok_or(Error::Refused)?;
            caller
                .intersect_restrictions(&current)
                .map_err(|_| Error::Refused)?
        }
        None => caller,
    };
    if caller.user() != user.id
        || caller
            .grant()
            .is_some_and(|grant| grant.token_ids().next().is_none())
    {
        return Err(Error::Refused);
    }
    stage.compatible(&caller)?;
    let (value, facts) = F::prepare(conn, &caller, &input, clock.time()).await?;
    let row = F::declaration(&input, &value);
    if row.operation != origin.operation() {
        return Err(Error::Refused);
    }
    let plan = sources::bind_required(F::ROUTE, row, &facts)?;
    let caller = plan.admit(&caller, false)?;
    let mut principal = Principal::load(conn, caller, clock.time()).await?;
    if user.kind != "service" && !account_policy::configured(&user) {
        let requirement = account_policy::requirement(conn, &user).await?;
        if !requirement.orgs.is_empty() {
            principal = principal.held_for_second_step(SecondStepHold {
                orgs: requirement.orgs,
                setup: true,
                sources: requirement.sources,
                methods: Vec::new(),
            });
        }
    }
    if principal.caller().admitted_operation() != Some(origin.operation()) {
        return Err(Error::Refused);
    }
    if let Some(hold) = principal.second_step_hold() {
        let held = hold.orgs.iter().copied().collect::<BTreeSet<_>>();
        if plan.required_facets()?.iter().any(|facet| {
            facet.target.node_ancestors.iter().any(|node| {
                node.kind == KeyTargetKind::Object(ObjectKind::Org)
                    && node.id.is_some_and(|id| held.contains(&id))
            })
        }) {
            return Err(Error::Refused);
        }
    }
    F::require(conn, &principal, &input, &value).await?;
    let frame = Frame {
        principal,
        opened: Opened {
            origin: origin.clone(),
            ceiling: ceiling.cloned(),
            input,
            value: Arc::new(value),
            plan,
        },
    };
    frame.check_deadline(clock)?;
    Ok(frame)
}
