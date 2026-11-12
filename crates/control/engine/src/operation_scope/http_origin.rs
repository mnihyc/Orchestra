//! Native frame and command adapters over the one shared original raw HTTP identity carrier.
use super::{continuing, request_origin::Credential};
use crate::EngineError;
use orch_authz::{Caller, CapturedOperation};
use orch_db::{Conn, queries::identity::UserRow};
use orch_types::{Clock, Time};
use serde::{Deserialize, Serialize};
use std::{future::Future, sync::Arc};

pub(crate) fn mapped(error: orch_authz::http_origin::Error) -> continuing::Error {
    use orch_authz::http_origin::Error;
    match error {
        Error::Refused => continuing::Error::Refused,
        Error::TransactionRequired => {
            EngineError::unreadable("HTTP origin restoration requires its transaction").into()
        }
        Error::InvalidOrigin => {
            EngineError::unreadable("The raw HTTP command origin is invalid.").into()
        }
        Error::Db(error) => error.into(),
        Error::Access(error) => error.into(),
    }
}
/// Native transports retain the common private serialized identity without another evaluator.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HttpOrigin {
    shared: orch_authz::http_origin::HttpOrigin,
}
impl std::fmt::Debug for HttpOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.shared, f)
    }
}
/// Actual current identity; operation facts still come from the native Source.
pub struct Current {
    shared: orch_authz::http_origin::Current,
}
impl Current {
    pub fn caller(&self) -> &Caller {
        self.shared.caller()
    }
    pub fn until(&self) -> Option<Time> {
        self.shared.until()
    }
    pub fn user(&self) -> &UserRow {
        self.shared.user()
    }
    pub fn check(&self, clock: &dyn Clock) -> Result<(), continuing::Error> {
        self.shared.check(clock).map_err(mapped)
    }
}
tokio::task_local! {static ACTIVE:Arc<HttpOrigin>;}
pub(crate) fn active() -> Option<Arc<HttpOrigin>> {
    ACTIVE.try_with(Arc::clone).ok()
}
impl HttpOrigin {
    pub fn capture(
        origin: CapturedOperation,
        credential: Credential,
        as_admin: bool,
        system_admin: bool,
        initial: Option<Time>,
    ) -> Result<Self, continuing::Error> {
        Ok(Self {
            shared: orch_authz::http_origin::HttpOrigin::capture(
                origin,
                credential,
                as_admin,
                system_admin,
                initial,
            )
            .map_err(mapped)?,
        })
    }
    /// The same private carrier for a lower-level standalone consumer, without another codec or evaluator.
    pub fn shared(&self) -> &orch_authz::http_origin::HttpOrigin {
        &self.shared
    }
    pub fn credential(&self) -> &Credential {
        self.shared.credential()
    }
    pub fn as_admin(&self) -> bool {
        self.shared.as_admin()
    }
    pub fn origin(&self) -> &CapturedOperation {
        self.shared.origin()
    }
    pub fn until(&self) -> Option<Time> {
        self.shared.until()
    }
    pub fn session(&self) -> Option<orch_types::Id> {
        self.shared.session()
    }
    pub fn same_identity(&self, other: &Self) -> bool {
        self.shared.same_identity(&other.shared)
    }
    pub fn intersect(&self, other: &Self) -> Result<Self, continuing::Error> {
        Ok(Self {
            shared: self.shared.intersect(&other.shared).map_err(mapped)?,
        })
    }
    pub fn with_ceiling(
        &self,
        caller: &Caller,
        until: Option<Time>,
    ) -> Result<Self, continuing::Error> {
        Ok(Self {
            shared: self.shared.with_ceiling(caller, until).map_err(mapped)?,
        })
    }
    pub fn accepted(
        &self,
        caller: &Caller,
        until: Option<Time>,
    ) -> Result<Self, continuing::Error> {
        Ok(Self {
            shared: self.shared.accepted(caller, until).map_err(mapped)?,
        })
    }
    pub async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Current, continuing::Error> {
        Ok(Current {
            shared: self.shared.current(conn, clock).await.map_err(mapped)?,
        })
    }
    pub async fn lock_identity(&self, conn: &mut Conn) -> Result<(), EngineError> {
        self.shared
            .lock_identity(conn)
            .await
            .map_err(|error| match mapped(error) {
                continuing::Error::Native(error) => error,
                _ => EngineError::unreadable("The raw HTTP command origin is invalid."),
            })
    }
    /// Keeps independently prepared native sources isolated through this awaited boundary.
    pub async fn scope<T>(&self, future: impl Future<Output = T>) -> T {
        ACTIVE.scope(Arc::new(self.clone()), future).await
    }
    pub async fn prepare<F: continuing::Source>(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        input: F::Input,
    ) -> Result<continuing::Frame<F>, continuing::Error> {
        self.scope(continuing::prepare::<F>(conn, clock, self.origin(), input))
            .await
    }
}
