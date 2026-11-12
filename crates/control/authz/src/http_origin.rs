//! The original raw HTTP identity and current accepted ceilings, shared by native execution and standalone delivery.
use crate::{Caller, CapturedCaller, CapturedOperation};
use orch_db::{
    Conn,
    queries::{identity::UserRow, transaction_guard},
};
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};

/// The raw identity ended or its common restoration failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Current raw identity, mode, source namespace or lifetime no longer matches.
    #[error("the original HTTP identity is no longer available")]
    Refused,
    /// A native source must retain its real transaction.
    #[error("HTTP origin restoration requires its transaction")]
    TransactionRequired,
    /// Stored raw command provenance is malformed.
    #[error("the raw HTTP command origin is invalid")]
    InvalidOrigin,
    /// A current stored identity lookup failed.
    #[error(transparent)]
    Db(#[from] orch_db::DbError),
    /// The same account/key source could not be restored.
    #[error(transparent)]
    Access(#[from] crate::Error),
}
/// Server-private association with a real raw credential, separate from accepted key bounds.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Credential {
    /// An actual browser access token on this session; its digest is never public data.
    Session { id: Id, hash: Vec<u8> },
    /// An actual API key, independent of any browser session.
    Token { id: Id, hash: Vec<u8> },
    /// An explicitly admitted in-process call, with no raw bearer or browser session claim.
    Native,
}
impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RequestCredential([private])")
    }
}
impl Credential {
    pub fn borrowed(&self) -> Option<transaction_guard::Credential<'_>> {
        match self {
            Self::Session { id, hash } => {
                Some(transaction_guard::Credential::Session { id: *id, hash })
            }
            Self::Token { id, hash } => {
                Some(transaction_guard::Credential::Token { id: *id, hash })
            }
            Self::Native => None,
        }
    }

    pub fn matches(&self, caller: &Caller) -> bool {
        match self {
            Self::Native => true,
            Self::Session { .. } => caller.grant().is_none(),
            Self::Token { id, .. } => caller
                .grant()
                .is_some_and(|grant| grant.token_ids().any(|actual| actual == *id)),
        }
    }

    /// Only an actual recorded browser Session supplies the native second-step policy input.
    pub fn browser_session(&self) -> Option<Id> {
        match self {
            Self::Session { id, .. } => Some(*id),
            Self::Token { .. } | Self::Native => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Version {
    #[serde(rename = "raw_http_v1")]
    One,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "at",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum Lifetime {
    Unbounded,
    Until(Time),
}
impl Lifetime {
    fn of(until: Option<Time>) -> Self {
        until.map_or(Self::Unbounded, Self::Until)
    }
    fn until(self) -> Option<Time> {
        match self {
            Self::Unbounded => None,
            Self::Until(at) => Some(at),
        }
    }
}
fn minimum(a: Option<Time>, b: Option<Time>) -> Option<Time> {
    a.into_iter().chain(b).min()
}
fn refused() -> Error {
    Error::Refused
}

/// The actual authenticator's private identity, never a saved admission or a bearer token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpOrigin {
    version: Version,
    origin: CapturedOperation,
    credential: Credential,
    as_admin: bool,
    system_admin: bool,
    initial: Lifetime,
    retained: Lifetime,
}
impl std::fmt::Debug for HttpOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HttpOrigin([private])")
    }
}
/// Actual current raw identity with no operation/target admission.
pub struct Current {
    caller: Caller,
    until: Option<Time>,
    user: UserRow,
}
impl Current {
    pub fn caller(&self) -> &Caller {
        &self.caller
    }
    pub fn until(&self) -> Option<Time> {
        self.until
    }
    pub fn user(&self) -> &UserRow {
        &self.user
    }
    /// After later awaits, consume no effect on an expired raw/key boundary.
    pub fn check(&self, clock: &dyn Clock) -> Result<(), Error> {
        if !self.caller.key_deadline_allows(clock)
            || self.until.is_some_and(|end| clock.time() >= end)
        {
            return Err(refused());
        }
        Ok(())
    }
}

impl HttpOrigin {
    /// The HTTP adapter supplies actual authenticated metadata, never a user-id-only session.
    /// This captures restrictions only; current() and the native Source remain mandatory.
    pub fn capture(
        origin: CapturedOperation,
        credential: Credential,
        as_admin: bool,
        system_admin: bool,
        initial: Option<Time>,
    ) -> Result<Self, Error> {
        let result = Self {
            version: Version::One,
            origin,
            credential,
            as_admin,
            system_admin,
            initial: Lifetime::of(initial),
            retained: Lifetime::of(initial),
        };
        result.validate()?;
        Ok(result)
    }
    fn validate(&self) -> Result<(), Error> {
        validate_identity(
            self.origin.user(),
            &self.credential,
            self.as_admin,
            self.initial,
        )
    }
    pub fn credential(&self) -> &Credential {
        &self.credential
    }
    pub fn as_admin(&self) -> bool {
        self.as_admin
    }
    pub fn origin(&self) -> &CapturedOperation {
        &self.origin
    }
    pub fn until(&self) -> Option<Time> {
        minimum(self.initial.until(), self.retained.until())
    }
    pub fn session(&self) -> Option<orch_types::Id> {
        self.credential.browser_session()
    }
    /// Exact user/credential/hash/explicit mode and initial lifetime; accepted ceilings only narrow.
    pub fn same_identity(&self, other: &Self) -> bool {
        self.origin.user() == other.origin.user()
            && self.origin.operation() == other.origin.operation()
            && self.credential == other.credential
            && self.as_admin == other.as_admin
            && self.system_admin == other.system_admin
            && self.initial == other.initial
    }
    pub fn intersect(&self, other: &Self) -> Result<Self, Error> {
        if !self.same_identity(other) {
            return Err(refused());
        }
        Ok(Self {
            origin: self
                .origin
                .intersect(&other.origin)
                .map_err(|_| refused())?,
            retained: Lifetime::of(minimum(self.until(), other.until())),
            ..self.clone()
        })
    }
    /// Delivery adds restrictions without requiring or replaying completed action admission.
    pub fn with_ceiling(&self, caller: &Caller, until: Option<Time>) -> Result<Self, Error> {
        if !self.credential.matches(caller) || caller.as_admin() && !self.as_admin {
            return Err(refused());
        }
        Ok(Self {
            origin: self.origin.with_ceiling(caller).map_err(|_| refused())?,
            retained: Lifetime::of(minimum(self.until(), until)),
            ..self.clone()
        })
    }
    /// Retains the genuine current native frame's key/raw minima; this creates no new admission.
    pub fn accepted(&self, caller: &Caller, until: Option<Time>) -> Result<Self, Error> {
        if !self.credential.matches(caller) || caller.as_admin() && !self.as_admin {
            return Err(refused());
        }
        Ok(Self {
            origin: self.origin.narrowed_with(caller).map_err(|_| refused())?,
            retained: Lifetime::of(minimum(self.until(), until)),
            ..self.clone()
        })
    }
    /// The common credential/account restoration checks the original bearer digest and all keys.
    /// Delivery uses these restrictions without replaying a completed mutation's permission gate.
    pub async fn current(&self, conn: &mut Conn, clock: &dyn Clock) -> Result<Current, Error> {
        self.validate()?;
        if !transaction_guard::in_transaction(conn) {
            return Err(Error::TransactionRequired);
        }
        let (caller, until, identity) = current_identity(
            conn,
            clock,
            &self.origin,
            &self.credential,
            self.until(),
            self.origin.operation(),
        )
        .await?;
        checked_current(
            caller,
            until,
            identity,
            self.as_admin,
            self.system_admin,
            clock,
        )
    }
    /// Factual raw/source fences before a canonical command lock; accepted receipts are not refused.
    pub async fn lock_identity(&self, conn: &mut Conn) -> Result<(), Error> {
        self.validate().map_err(|_| Error::InvalidOrigin)?;
        if let Some(raw) = self.credential.borrowed() {
            let _ = transaction_guard::lock_identity(conn, self.origin.user(), raw, false).await?;
        }
        Ok(())
    }
}

pub async fn current_identity(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &CapturedOperation,
    credential: &Credential,
    original_until: Option<Time>,
    operation: &str,
) -> Result<(Caller, Option<Time>, Option<transaction_guard::Identity>), Error> {
    if origin.operation() != operation {
        return Err(Error::Refused);
    }
    current_boundary_identity(
        conn,
        clock,
        Boundary::Operation(origin),
        credential,
        original_until,
    )
    .await
}

type RestoredBoundary<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Option<Caller>, crate::Error>> + Send + 'a>,
>;

enum Boundary<'a> {
    Operation(&'a CapturedOperation),
    Preparation(&'a CapturedCaller),
}
impl Boundary<'_> {
    fn user(&self) -> Id {
        match self {
            Self::Operation(origin) => origin.user(),
            Self::Preparation(origin) => origin.user(),
        }
    }
    fn restore<'a>(&'a self, conn: &'a mut Conn, clock: &'a dyn Clock) -> RestoredBoundary<'a> {
        Box::pin(async move {
            match self {
                Self::Operation(origin) => origin.restore_current(conn, clock).await,
                Self::Preparation(origin) => origin.restore_current(conn, clock).await,
            }
        })
    }
}
async fn current_boundary_identity(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: Boundary<'_>,
    credential: &Credential,
    original_until: Option<Time>,
) -> Result<(Caller, Option<Time>, Option<transaction_guard::Identity>), Error> {
    let identity = match credential.borrowed() {
        Some(raw) => Some(
            transaction_guard::lock_identity(conn, origin.user(), raw, false)
                .await?
                .ok_or(Error::Refused)?,
        ),
        None => {
            if transaction_guard::lock_native_actor(conn, origin.user())
                .await?
                .is_none()
            {
                return Err(Error::Refused);
            }
            None
        }
    };
    let Some(mut caller) = origin.restore(conn, clock).await? else {
        return Err(Error::Refused);
    };
    if !credential.matches(&caller) {
        return Err(Error::Refused);
    }
    if let Some(identity) = &identity {
        let enabled = caller.as_admin()
            && identity.as_admin
            && matches!(credential, Credential::Session { .. });
        caller = caller.with_admin_mode(enabled);
    }
    let current_until = identity
        .as_ref()
        .and_then(|identity| identity.expires_at.map(Time::new));
    let until = match (current_until, original_until) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    if !caller.key_deadline_allows(clock)
        || until.is_some_and(|end| clock.time().get() >= end.get())
    {
        return Err(Error::Refused);
    }
    Ok((caller, until, identity))
}

fn checked_current(
    caller: Caller,
    until: Option<Time>,
    identity: Option<transaction_guard::Identity>,
    as_admin: bool,
    system_admin: bool,
    clock: &dyn Clock,
) -> Result<Current, Error> {
    let identity = identity.ok_or_else(refused)?;
    if identity.user.suspended() {
        return Err(refused());
    }
    let enabled = caller.as_admin() && as_admin && identity.user.is_system_admin == system_admin;
    let current = Current {
        caller: caller.with_admin_mode(enabled),
        until,
        user: identity.user,
    };
    current.check(clock)?;
    orch_db::audit::set_admin_mode(current.caller.user(), current.caller.as_admin());
    Ok(current)
}

fn validate_identity(
    user: Id,
    credential: &Credential,
    as_admin: bool,
    initial: Lifetime,
) -> Result<(), Error> {
    let valid = match credential {
        Credential::Session { id, hash } => {
            !id.is_nil() && hash.len() == 32 && initial.until().is_some()
        }
        Credential::Token { id, hash } => !id.is_nil() && hash.len() == 32 && !as_admin,
        Credential::Native => false,
    };
    if !valid || user.is_nil() {
        return Err(refused());
    }
    Ok(())
}

mod preparation;
pub use preparation::HttpPreparation;

/// The same account/session hold for native and standalone readers; a key has no browser session.
pub async fn session_hold(
    conn: &mut Conn,
    user: &UserRow,
    session: Option<Id>,
) -> Result<Option<crate::suspension::SecondStepHold>, orch_settings::account_policy::Error> {
    if user.kind == "service" {
        return Ok(None);
    }
    let set_up = orch_settings::account_policy::configured(user);
    if set_up {
        match session {
            None => return Ok(None),
            Some(session)
                if orch_db::queries::second_step::session_given_at(conn, session)
                    .await?
                    .is_some() =>
            {
                return Ok(None);
            }
            Some(_) => {}
        }
    }
    let requirement = orch_settings::account_policy::requirement(conn, user).await?;
    if requirement.orgs.is_empty() {
        return Ok(None);
    }
    Ok(Some(crate::suspension::SecondStepHold {
        orgs: requirement.orgs,
        setup: !set_up,
        sources: requirement.sources,
        methods: Vec::new(),
    }))
}
