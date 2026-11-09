//! Immutable accepted caller restrictions for existing jobs, inputs and continuations.
//!
//! Restoring rechecks the actual credentials and intersects their current policies. Operation
//! admission and resolved target paths are deliberately absent; the shared operation must
//! resolve targets and admit its facets again at execution/delivery.

use super::{Caller, KeptOut, KeyScopeRefusal, Scopes, TokenGrant};
use crate::Error;
use orch_access_contract::key_scope::KeyScope;
use orch_db::Conn;
use orch_db::queries::tokens;
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};

/// Accepted restrictions, tied to the original acting account rather than a reconstructed session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturedCaller {
    user: Id,
    credentials: Vec<Credential>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Credential {
    OwnerRead {
        source: Id,
    },
    MachineOwner {
        machine: Id,
        transaction: i64,
    },
    Session {
        as_admin: bool,
    },
    Token {
        token_ids: Vec<Id>,
        scopes: Vec<String>,
        node_id: Option<Id>,
        #[serde(default)]
        other_subtrees: Vec<Id>,
        #[serde(default)]
        operation_scope: Option<KeyScope>,
        #[serde(default)]
        legacy_ceiling: Option<Vec<String>>,
        #[serde(default)]
        expires_at: Option<Time>,
        #[serde(default)]
        kept_out: Vec<KeptOut>,
    },
}

fn captured(caller: &Caller) -> Credential {
    if let Some(source) = caller.owner_read_source() {
        return Credential::OwnerRead { source };
    }
    if let Some(machine) = caller.native_machine() {
        return Credential::MachineOwner {
            machine,
            transaction: caller.native_transaction().unwrap_or_default(),
        };
    }
    match caller.grant() {
        None => Credential::Session {
            as_admin: caller.as_admin(),
        },
        Some(grant) => Credential::Token {
            token_ids: grant.token_ids().collect(),
            scopes: grant.scopes.to_strings(),
            node_id: grant.subtree,
            other_subtrees: grant.other_subtrees.clone(),
            operation_scope: grant.operation_scope.clone(),
            legacy_ceiling: grant.legacy_ceiling.as_ref().map(Scopes::to_strings),
            expires_at: grant.expires_at,
            kept_out: grant.kept_out.clone(),
        },
    }
}

tokio::task_local! { static LIVE_OWNER_READ: (Id, Id); }

/// Runs only while the native entry retains its real owner-only source.
/// The API checks that sealed source before every snapshot and final publication;
/// a captured caller cannot turn this request-local scope into a background grant.
pub async fn with_owner_read_source<T>(
    owner: Id,
    source: Id,
    work: impl std::future::Future<Output = T>,
) -> T {
    LIVE_OWNER_READ.scope((owner, source), work).await
}

fn current_owner_read(owner: Id, source: Id) -> Option<Caller> {
    LIVE_OWNER_READ
        .try_with(|actual| *actual == (owner, source))
        .ok()
        .filter(|current| *current)
        .map(|_| Caller::owner_read(owner, source))
}

impl CapturedCaller {
    /// Captures policy and credential identity, never the current operation's admission proof.
    #[must_use]
    pub fn capture(caller: &Caller) -> Self {
        Self {
            user: caller.user(),
            credentials: vec![captured(caller)],
        }
    }

    /// The acting account whose authority still has to pass the existing authorizer.
    #[must_use]
    pub fn user(&self) -> Id {
        self.user
    }

    /// Retains earlier ceilings when an accepted request is edited with another credential.
    pub fn narrowed_with(&self, caller: &Caller) -> Result<Self, KeyScopeRefusal> {
        if caller.user() != self.user {
            return Err(KeyScopeRefusal::DifferentAccount);
        }
        let mut result = self.clone();
        let next = captured(caller);
        if !result.credentials.contains(&next) {
            result.credentials.push(next);
        }
        Ok(result)
    }

    /// Conjoin two retained snapshots of this actor without removing any earlier credential.
    pub fn intersect(&self, current: &Self) -> Result<Self, KeyScopeRefusal> {
        if self.user != current.user {
            return Err(KeyScopeRefusal::DifferentAccount);
        }
        let mut result = self.clone();
        for credential in &current.credentials {
            if !result.credentials.contains(credential) {
                result.credentials.push(credential.clone());
            }
        }
        Ok(result)
    }

    /// Restores the same original/current restrictions using the application clock. A row
    /// lock or policy lookup may wait, so the minimum recorded key deadline is checked again
    /// AFTER all restoration awaits. This returns no operation/target admission.
    /// A consumer that waits again must check the retained caller at its final boundary too.
    pub async fn restore_current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Caller>, Error> {
        let restored = self.restore(conn, clock.time()).await?;
        Ok(restored.filter(|caller| caller.key_deadline_allows(clock)))
    }

    /// Rechecks every original key's current validity and policy. Missing/revoked/expired keys
    /// return None. Invalid stored policy fails closed. Returned callers are unadmitted.
    /// Mutation consumers call this inside their existing action transaction/guard.
    pub async fn restore(&self, conn: &mut Conn, now: Time) -> Result<Option<Caller>, Error> {
        if self.credentials.is_empty() {
            return Err(Error::Stored(
                "captured caller has no credential boundary".into(),
            ));
        }
        let mut restored: Option<Caller> = None;
        for credential in &self.credentials {
            let mut next = match credential {
                Credential::OwnerRead { source } => match current_owner_read(self.user, *source) {
                    Some(current) => current,
                    None => return Ok(None),
                },
                Credential::MachineOwner {
                    machine,
                    transaction,
                } => {
                    if !orch_db::queries::transaction_guard::in_transaction(conn) {
                        return Ok(None);
                    }
                    let actual =
                        orch_db::queries::transaction_guard::current_transaction_id(conn).await?;
                    if actual != *transaction {
                        return Ok(None);
                    }
                    orch_db::queries::transaction_guard::share_machine(conn, *machine).await?;
                    let row = orch_db::queries::resources::machine(conn, *machine).await?;
                    if row.is_none_or(|row| {
                        row.owner_id != Some(self.user)
                            || row.placement != "connected"
                            || !row.is_active()
                    }) {
                        return Ok(None);
                    }
                    if orch_db::queries::identity::user_by_id(conn, self.user)
                        .await?
                        .is_none_or(|user| user.status != "active")
                    {
                        return Ok(None);
                    }
                    Caller::machine_owner(*machine, self.user, *transaction)
                }
                Credential::Session { as_admin } => {
                    Caller::session(self.user).with_admin_mode(*as_admin)
                }
                Credential::Token {
                    token_ids,
                    scopes,
                    node_id,
                    other_subtrees,
                    operation_scope,
                    legacy_ceiling,
                    expires_at,
                    kept_out,
                } => {
                    if expires_at.is_some_and(|end| end.get() <= now.get()) {
                        return Ok(None);
                    }
                    if operation_scope
                        .as_ref()
                        .is_some_and(|scope| !scope.validate().is_empty())
                        || (operation_scope.is_some() && !scopes.is_empty())
                        || (operation_scope.is_none() && legacy_ceiling.is_some())
                        || token_ids.iter().any(|id| id.is_nil())
                    {
                        return Err(Error::Stored("invalid captured key boundary".into()));
                    }
                    let mut grant = TokenGrant::from_stored(scopes, *node_id)
                        .map_err(|error| Error::Stored(error.to_string()))?;
                    grant.operation_scope = operation_scope.clone();
                    grant.legacy_ceiling = legacy_ceiling
                        .as_deref()
                        .map(Scopes::parse)
                        .transpose()
                        .map_err(|error| Error::Stored(error.to_string()))?;
                    grant.other_subtrees = other_subtrees.clone();
                    grant.expires_at = *expires_at;
                    grant.kept_out = kept_out.clone();
                    if token_ids.len() == 1 {
                        grant.token = token_ids.first().copied();
                    } else {
                        grant.other_tokens = token_ids.clone();
                    }
                    Caller::token(self.user, grant)
                }
            };
            if let Credential::Token { token_ids, .. } = credential {
                for token in token_ids {
                    let current =
                        tokens::current_boundary(conn, *token, self.user, now.get()).await?;
                    let Some(current) = current else {
                        return Ok(None);
                    };
                    let scope = current
                        .operation_scope
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(|error| {
                            Error::Stored(format!("invalid stored key scope: {error}"))
                        })?;
                    let mut grant = TokenGrant::from_stored_scope(
                        &current.scopes,
                        current.node_id,
                        scope,
                        current.expires_at.map(Time::new),
                    )?
                    .for_token(*token);
                    for blocked in tokens::blocked(conn, *token).await? {
                        match (blocked.refused, blocked.org, blocked.max_days) {
                            (true, Some(org), _) => grant.kept_out.push(KeptOut::Refused { org }),
                            (false, place, Some(max_days)) => {
                                grant.kept_out.push(KeptOut::Lifetime { place, max_days })
                            }
                            _ => {}
                        }
                    }
                    next = next
                        .intersect_restrictions(&Caller::token(self.user, grant))
                        .map_err(|error| Error::Stored(error.to_string()))?;
                }
            }
            restored = Some(match restored {
                Some(accepted) => accepted
                    .intersect_restrictions(&next)
                    .map_err(|error| Error::Stored(error.to_string()))?,
                None => next,
            });
        }
        Ok(restored)
    }
}

#[cfg(test)]
mod owner_read_tests {
    use super::*;
    #[tokio::test]
    async fn native_owner_reads_have_no_session_token_admin_or_restorable_lease()
    -> Result<(), KeyScopeRefusal> {
        let owner = orch_types::new_id();
        let source = orch_types::new_id();
        let native = Caller::owner_read(owner, source).with_admin_mode(true);
        assert!(native.grant().is_none());
        assert!(!native.as_admin());
        assert_eq!(
            native.credential(),
            orch_access_contract::roles::Credential::OwnerRead
        );
        assert!(
            native
                .intersect_restrictions(&Caller::session(owner))
                .is_err()
        );
        assert!(
            native
                .intersect_restrictions(&Caller::owner_read(owner, orch_types::new_id()))
                .is_err()
        );
        assert_eq!(
            native.intersect_restrictions(&native)?.owner_read_source(),
            Some(source)
        );
        assert!(
            matches!(captured(&native), Credential::OwnerRead {source: actual} if actual == source)
        );
        assert!(current_owner_read(owner, source).is_none());
        with_owner_read_source(owner, source, async {
            assert!(current_owner_read(owner, source).is_some());
            assert!(current_owner_read(owner, orch_types::new_id()).is_none());
        })
        .await;
        assert!(current_owner_read(owner, source).is_none());
        Ok(())
    }
}
