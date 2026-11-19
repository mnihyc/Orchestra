//! The exact accepted operation of deferred work, beside its immutable caller boundary.
//!
//! This carries provenance, never reusable admission. The ordinary producer must resolve its
//! actual work again and pass the resulting facets through the shared operation gate.

use super::{Caller, CapturedCaller, KeyScopeRefusal};
use crate::Error;
use orch_db::Conn;
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};

/// One accepted operation. Keep separate origins separate: a create-only credential and an
/// update-only credential must each pass their own operation on the final materialized work.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturedOperation {
    operation: String,
    caller: CapturedCaller,
}

impl CapturedOperation {
    /// Captures the exact trusted operation selected by the shared gate. A name-only preliminary
    /// check, mixed selections, and an unadmitted caller cannot establish an accepted origin.
    pub fn capture(caller: &Caller) -> Result<Self, KeyScopeRefusal> {
        let operation = caller
            .admitted_operation()
            .filter(|operation| !operation.is_empty())
            .ok_or(KeyScopeRefusal::Unclassified)?;
        Ok(Self {
            operation: operation.to_owned(),
            caller: CapturedCaller::capture(caller),
        })
    }

    /// The exact original catalogue selection; a consumer must not replace it with a nearby route.
    #[must_use]
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// The existing acting account. This identity does not reconstruct a browser caller.
    #[must_use]
    pub fn user(&self) -> Id {
        self.caller.user()
    }

    /// Preserve every earlier ceiling while retaining this same admitted operation again.
    /// This retains restrictions only; targets and ordinary permission must still be rechecked.
    pub fn narrowed_with(&self, caller: &Caller) -> Result<Self, KeyScopeRefusal> {
        if caller.admitted_operation() != Some(self.operation()) {
            return Err(KeyScopeRefusal::Unclassified);
        }
        Ok(Self {
            operation: self.operation.clone(),
            caller: self.caller.narrowed_with(caller)?,
        })
    }

    /// Add a genuine restored transport ceiling without restoring any target admission.
    pub fn with_ceiling(&self, caller: &Caller) -> Result<Self, KeyScopeRefusal> {
        Ok(Self {
            operation: self.operation.clone(),
            caller: self.caller.narrowed_with(caller)?,
        })
    }

    /// Conjoin accepted source snapshots of this same act; no target admission is restored.
    pub fn intersect(&self, current: &Self) -> Result<Self, KeyScopeRefusal> {
        if self.operation != current.operation {
            return Err(KeyScopeRefusal::Unclassified);
        }
        Ok(Self {
            operation: self.operation.clone(),
            caller: self.caller.intersect(&current.caller)?,
        })
    }

    /// Rechecks current credential validity and policy, returning an unadmitted caller. The
    /// original selection remains on this carrier; current targets must come from the producer.
    pub async fn restore(&self, conn: &mut Conn, now: Time) -> Result<Option<Caller>, Error> {
        self.validate()?;
        self.caller.restore(conn, now).await
    }

    /// The same exact captured selection with live-clock original/current key restoration.
    /// Target facts and ordinary permission must still be resolved by its actual producer.
    pub async fn restore_current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Caller>, Error> {
        self.validate()?;
        self.caller.restore_current(conn, clock).await
    }

    /// Preliminary unadmitted source selection without row locks. This cannot stand in for
    /// the writer's locked restoration, factual binding or current permission checks.
    pub async fn preview_current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Caller>, Error> {
        self.validate()?;
        self.caller.preview_current(conn, clock).await
    }

    fn validate(&self) -> Result<(), Error> {
        if self.operation.is_empty() {
            return Err(Error::Stored("captured operation has no selection".into()));
        }
        Ok(())
    }
}
