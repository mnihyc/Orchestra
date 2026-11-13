//! A typed native preparation phase over its complete known source footprint.
//!
//! The domain declares every canonical required role needed by this native phase,
//! including its actual setup effects. Later unresolved business roles stay unresolved.
//! These values cannot become a complete business Frame, effect plan or response proof.
//! Fact selection, native locks, ordinary authority and original/current identity still
//! use the same continuing kernel. The owner consumes the cut at its actual native seam.

pub use super::{Error, Source};
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::Conn;
use orch_types::Clock;
use std::sync::Arc;

/// Current actual operands for native preparation only, never final business admission.
#[must_use = "consume only at the declared native preparation boundary"]
pub struct Frame<F: Source>(super::Frame<F>);

/// One immutable native preparation association, retained across its actual awaits.
#[must_use = "refresh the original native phase before further preparation"]
pub struct Opened<F: Source>(super::Opened<F>);

/// A purpose-specific cut, deliberately distinct from a full business first-effect check.
#[must_use = "consume at the actual native preparation effect"]
pub struct FirstEffect(super::FirstEffect);
impl FirstEffect {
    /// Invoke after the native phase's last wait on this same prepared transaction.
    pub fn check(self, clock: &dyn Clock) -> Result<(), Error> {
        self.0.check(clock)
    }
}

/// Only the actual native preparation operands and their once-only preparation cut.
#[must_use = "these parts authorize no later unresolved business effect"]
pub struct Parts<F: Source> {
    /// The original/current actor restricted to the declared actual native phase.
    pub principal: Principal,
    /// The same immutable server-owned native input.
    pub input: Arc<F::Input>,
    /// The same fresh factual operands the phase admitted.
    pub value: Arc<F::Value>,
    /// Consume only at this native preparation boundary.
    pub first_effect: FirstEffect,
    /// Reprepare this same native association before later preparation work.
    pub opened: Opened<F>,
}

impl<F: Source> Frame<F> {
    /// The actual raw credential and observed minima of this preparation, without complete admission.
    pub fn http_origin(&self) -> Option<&crate::operation_scope::http_origin::HttpOrigin> {
        self.0.http_origin()
    }
    /// The native actor for this phase, carrying no complete required-role proof.
    pub fn principal(&self) -> &Principal {
        self.0.principal()
    }
    /// The exact fresh native operands, without replaying their supplier.
    pub fn value(&self) -> &F::Value {
        self.0.value()
    }
    /// Keep the current original/all-key minimum at a later synchronous boundary.
    pub fn check_deadline(&self, clock: &dyn Clock) -> Result<(), Error> {
        self.0.check_deadline(clock)
    }
    /// A native preparation writer consumes these operands; a main writer still needs
    /// complete fresh required facts through the ordinary complete operation gate.
    pub fn into_parts(self) -> Parts<F> {
        let parts = self.0.into_parts();
        Parts {
            principal: parts.principal,
            input: parts.input,
            value: parts.value,
            first_effect: FirstEffect(parts.first_effect),
            opened: Opened(parts.opened),
        }
    }
}

impl<F: Source> Opened<F> {
    /// The exact trusted original operation, never a nearby creator/login alias.
    pub fn origin(&self) -> &CapturedOperation {
        self.0.origin()
    }
    /// Repeat this exact phase, known-role footprint and native association after a wait.
    pub async fn refresh(&self, conn: &mut Conn, clock: &dyn Clock) -> Result<Frame<F>, Error> {
        self.0.refresh(conn, clock).await.map(Frame)
    }
    /// Also intersect a genuinely restored transport/other original caller ceiling.
    pub async fn refresh_with_caller(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        caller: &Caller,
    ) -> Result<Frame<F>, Error> {
        self.0
            .refresh_with_caller(conn, clock, caller)
            .await
            .map(Frame)
    }
}

/// Check the declared native phase before preparation IO or setup. Its declaration must
/// contain every source needed by that phase, all factual and canonical Required roles.
/// Missing declared roles, response roles and a wholly empty footprint are refused.
pub async fn prepare<F: Source>(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &CapturedOperation,
    input: F::Input,
) -> Result<Frame<F>, Error> {
    super::prepare_input::<F>(
        conn,
        clock,
        origin,
        Arc::new(input),
        None,
        super::Phase::Preparation,
        super::Transport::Current(None),
    )
    .await
    .map(Frame)
}

/// The same phase with the actual additional caller, preserving all original/current keys.
pub async fn prepare_with_caller<F: Source>(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &CapturedOperation,
    caller: &Caller,
    input: F::Input,
) -> Result<Frame<F>, Error> {
    super::prepare_input::<F>(
        conn,
        clock,
        origin,
        Arc::new(input),
        Some(caller.clone()),
        super::Phase::Preparation,
        super::Transport::Current(None),
    )
    .await
    .map(Frame)
}
