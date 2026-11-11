//! Preparation provenance stays distinct from an accepted business operation in measurement rows.
use super::*;
use orch_authz::http_origin::HttpPreparation;

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum Origin {
    // Preserve the existing complete-operation private wire record exactly.
    Operation(CapturedOperation),
    Inventory(HttpPreparation),
}
impl Origin {
    pub(super) fn user(&self) -> Id {
        match self {
            Self::Operation(origin) => origin.user(),
            Self::Inventory(origin) => origin.user(),
        }
    }
    pub(super) fn operation(&self) -> &str {
        match self {
            Self::Operation(origin) => origin.operation(),
            Self::Inventory(origin) => origin.operation(),
        }
    }
    pub(super) fn complete(&self) -> Option<&CapturedOperation> {
        match self {
            Self::Operation(origin) => Some(origin),
            Self::Inventory(_) => None,
        }
    }
    pub(super) fn prefix(&self) -> Option<&HttpPreparation> {
        match self {
            Self::Inventory(origin) => Some(origin),
            _ => None,
        }
    }
    pub(super) fn admits(&self, caller: &Caller) -> bool {
        match self {
            Self::Operation(origin) => {
                caller.user() == origin.user()
                    && caller.admitted_operation() == Some(origin.operation())
                    && caller.admitted_required_roles().is_some()
            }
            Self::Inventory(origin) => {
                caller.user() == origin.user()
                    && crate::jobs::lifecycle_commands::host_inventory::prefix_caller(caller)
            }
        }
    }
    pub(super) async fn restore_current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Caller>, EngineError> {
        match self {
            Self::Operation(origin) => Ok(origin.restore_current(conn, clock).await?),
            Self::Inventory(origin) => Ok(Some(
                origin
                    .current(conn, clock)
                    .await
                    .map_err(raw)?
                    .caller()
                    .clone(),
            )),
        }
    }
    pub(super) fn narrowed_with(
        &self,
        caller: &Caller,
        until: Option<Time>,
    ) -> Result<Self, EngineError> {
        Ok(match self {
            Self::Operation(origin) => Self::Operation(
                origin
                    .narrowed_with(caller)
                    .map_err(EngineError::unreadable)?,
            ),
            Self::Inventory(origin) => {
                if !self.admits(caller) {
                    return Err(closed());
                }
                Self::Inventory(origin.accepted(caller, until).map_err(raw)?)
            }
        })
    }
    pub(super) fn narrow_caller(
        &self,
        retained: &Caller,
        proof: &Caller,
    ) -> Result<Caller, EngineError> {
        match self {
            Self::Operation(_) => retained
                .narrow_current_admission(proof)
                .map_err(|_| closed()),
            Self::Inventory(_) => {
                if !self.admits(proof) {
                    return Err(closed());
                }
                retained
                    .narrow_preparation_admission(proof)
                    .map_err(|_| closed())
            }
        }
    }
    pub(super) async fn retain(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        until: Option<Time>,
        callers: &[(Option<Caller>, Option<Time>)],
    ) -> Result<(Self, Option<Time>), EngineError> {
        if let Self::Operation(origin) = self {
            let (origin, until) =
                retain_current_callers(conn, clock, origin, until, callers).await?;
            return Ok((Self::Operation(origin), until));
        }
        let mut current = self
            .restore_current(conn, clock)
            .await?
            .ok_or_else(closed)?;
        let mut end = until;
        let mut accepted = false;
        for (caller, bound) in callers {
            end = end.into_iter().chain(*bound).min();
            if let Some(proof) = caller {
                current = self.narrow_caller(&current, proof)?;
                accepted = true;
            }
        }
        if !accepted
            || !current.key_deadline_allows(clock)
            || end.is_some_and(|until| clock.time() >= until)
        {
            return Err(closed());
        }
        Ok((self.narrowed_with(&current, end)?, end))
    }
}
pub(super) fn raw(error: orch_authz::http_origin::Error) -> EngineError {
    match crate::operation_scope::http_origin::mapped(error) {
        crate::operation_scope::continuing::Error::Native(error) => error,
        error => EngineError::BeforeEffect(Box::new(error)),
    }
}
