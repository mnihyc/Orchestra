//! The actual raw request credential at native boundaries, without an invented session.

use super::continuing::Error;
use orch_authz::{Caller, CapturedOperation};
use orch_db::{Conn, queries::transaction_guard};
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};

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
    fn borrowed(&self) -> Option<transaction_guard::Credential<'_>> {
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

    pub(crate) fn matches(&self, caller: &Caller) -> bool {
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

/// Restore the original operation/current key minimum and its exact raw credential together.
/// The domain still supplies current source facts and ordinary authorization on this transaction.
pub(crate) async fn current(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &CapturedOperation,
    credential: &Credential,
    original_until: Option<Time>,
    operation: &str,
) -> Result<(Caller, Option<Time>), Error> {
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
    let Some(mut caller) = origin.restore_current(conn, clock).await? else {
        return Err(Error::Refused);
    };
    if origin.operation() != operation || !credential.matches(&caller) {
        return Err(Error::Refused);
    }
    if let Some(identity) = &identity {
        let enabled = caller.as_admin()
            && identity.as_admin
            && matches!(credential, Credential::Session { .. });
        caller = caller.with_admin_mode(enabled);
    }
    let current_until = identity.and_then(|identity| identity.expires_at.map(Time::new));
    let until = match (current_until, original_until) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    if !caller.key_deadline_allows(clock)
        || until.is_some_and(|end| clock.time().get() >= end.get())
    {
        return Err(Error::Refused);
    }
    Ok((caller, until))
}
