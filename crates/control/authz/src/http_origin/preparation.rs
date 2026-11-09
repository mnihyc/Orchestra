//! Raw HTTP restrictions for native discovery, without a captured business operation.
use super::*;

/// Preparation provenance has no operation/required-role admission and no conversion to one.
/// A native factual source must admit its exact known preparation roles on every use.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpPreparation {
    version: Version,
    caller: CapturedCaller,
    request_operation: String,
    credential: Credential,
    as_admin: bool,
    system_admin: bool,
    initial: Lifetime,
    retained: Lifetime,
}
impl std::fmt::Debug for HttpPreparation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HttpPreparation([private])")
    }
}
impl HttpPreparation {
    /// Capture only the actual authenticated restriction and raw metadata, never a source permit.
    pub fn capture(
        request_operation: &str,
        caller: &Caller,
        credential: Credential,
        as_admin: bool,
        system_admin: bool,
        initial: Option<Time>,
    ) -> Result<Self, Error> {
        if !credential.matches(caller) || caller.as_admin() && !as_admin {
            return Err(refused());
        }
        let result = Self {
            version: Version::One,
            caller: CapturedCaller::capture(caller),
            request_operation: request_operation.to_owned(),
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
        if self.request_operation.is_empty() {
            return Err(refused());
        }
        validate_identity(self.user(), &self.credential, self.as_admin, self.initial)
    }
    /// Existing acting account; this does not construct a browser Session.
    /// Original raw request namespace, never an admitted or complete operation proof.
    pub fn operation(&self) -> &str {
        &self.request_operation
    }
    pub fn user(&self) -> Id {
        self.caller.user()
    }
    pub fn until(&self) -> Option<Time> {
        minimum(self.initial.until(), self.retained.until())
    }
    pub fn session(&self) -> Option<Id> {
        self.credential.browser_session()
    }
    pub fn credential(&self) -> &Credential {
        &self.credential
    }
    pub fn as_admin(&self) -> bool {
        self.as_admin
    }
    pub fn same_identity(&self, other: &Self) -> bool {
        self.user() == other.user()
            && self.request_operation == other.request_operation
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
            caller: self
                .caller
                .intersect(&other.caller)
                .map_err(|_| refused())?,
            retained: Lifetime::of(minimum(self.until(), other.until())),
            ..self.clone()
        })
    }
    /// Keep all genuinely observed restrictions; no target admission is serialized.
    pub fn accepted(&self, caller: &Caller, until: Option<Time>) -> Result<Self, Error> {
        if !self.credential.matches(caller) || caller.as_admin() && !self.as_admin {
            return Err(refused());
        }
        Ok(Self {
            caller: self.caller.narrowed_with(caller).map_err(|_| refused())?,
            retained: Lifetime::of(minimum(self.until(), until)),
            ..self.clone()
        })
    }
    /// The identical raw credential/account/mode/standing/key/lifetime kernel as a full origin.
    pub async fn current(&self, conn: &mut Conn, clock: &dyn Clock) -> Result<Current, Error> {
        self.validate()?;
        if !transaction_guard::in_transaction(conn) {
            return Err(Error::TransactionRequired);
        }
        let (caller, until, identity) = current_boundary_identity(
            conn,
            clock,
            Boundary::Preparation(&self.caller),
            &self.credential,
            self.until(),
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
    pub async fn lock_identity(&self, conn: &mut Conn) -> Result<(), Error> {
        self.validate()?;
        let raw = self.credential.borrowed().ok_or_else(refused)?;
        let _ = transaction_guard::lock_identity(conn, self.user(), raw, false).await?;
        Ok(())
    }
}
