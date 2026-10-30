//! Credentials components.
//!
//! A credential that leaves a provider's runtime for the harness of a thread: a copy that is shared, and the login itself that is handed over, with what the engine remembers of both.
//!
//! A harness that runs away from its provider, to stay beside its own tools, and that the forwarder cannot
//! serve, runs when it is given a credential. [`sharing`] gives it a copy (the provider's runtime stays the
//! owner and issues a fresh copy whenever one is about to lapse) and [`handover`] moves the whole login to
//! the environment, from where it must come home. Neither has a value in its hands: the provider's runtime
//! seals what it issues to the machine where the harness runs, and the engine carries the sealed form from
//! the one to the other. No row, event or log of the engine ever holds a credential in the clear.
//!
//! What the engine remembers here is small and lost with the process, which is safe: how many copies were
//! issued under a lease and when (a new one is not issued within thirty seconds of the last, nor while
//! one is in flight), what the harness said it lacks, and which providers have a return under way (a
//! return and a declaration that a credential is lost never run together).

pub mod handover;
pub mod sharing;

use orch_agent_contract::credentials::{CredentialPath, may_issue};
use orch_link_contract::commands::{MachineKey, SealedCredential};
use orch_link_contract::events::CredentialNeed;
use orch_types::{Id, Time};
use std::collections::{HashMap, HashSet};

/// What the engine remembers of the copies it issued under one lease.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Issued {
    /// When the last copy was issued.
    pub last: Option<Time>,
    /// When the copy the harness holds lapses, where it does.
    pub expires_at: Option<Time>,
    /// Whether a copy is being issued now.
    pub in_flight: bool,
    /// What the harness said it lacks, until a copy has been given.
    pub needed: Option<CredentialNeed>,
    /// How many copies were issued under the lease: the number that makes each command's identifier its own.
    pub count: u32,
}

/// What a return that did not finish keeps, so that trying again goes on from where it stopped: the
/// number of the attempt (which names the commands, since a command asked for twice under one identifier
/// is made once, and that would answer a retry with the failure again), and the credential the
/// environment gave back, sealed to the provider's runtime, until that has taken it.
#[derive(Debug, Clone, Default)]
pub struct Unfinished {
    /// How many attempts failed.
    pub attempt: u32,
    /// What the environment gave back, still sealed.
    pub sealed: Option<SealedCredential>,
}

/// What the engine remembers of the credentials that left their providers.
#[derive(Debug, Default)]
pub struct Issuing {
    leases: HashMap<Id, Issued>,
    returning: HashSet<Id>,
    unfinished: HashMap<Id, Unfinished>,
}

impl Issuing {
    /// What is remembered of a lease.
    #[must_use]
    pub fn of(&self, lease: Id) -> Issued {
        self.leases.get(&lease).copied().unwrap_or_default()
    }

    /// Restores only an uninitialized lease, preserving an in-flight issue or a harness's renewal request.
    pub fn restore(&mut self, lease: Id, expires_at: Option<Time>, last: Option<Time>, count: u32) {
        let held = self.leases.entry(lease).or_default();
        if held.last.is_none() && !held.in_flight {
            held.expires_at = expires_at;
            held.last = last;
            held.count = held.count.max(count);
        }
    }

    /// Begins an issue under a lease, if one may be made now: not while one is in flight, and not within
    /// thirty seconds of the last. The answer is the number of the issue, which names its commands.
    pub fn begin(&mut self, lease: Id, now: Time) -> Option<u32> {
        let held = self.leases.entry(lease).or_default();
        if !may_issue(held.in_flight, held.last, now) {
            return None;
        }
        held.in_flight = true;
        Some(held.count)
    }

    /// The issue that was begun gave a copy that lapses at `expires_at`.
    pub fn done(&mut self, lease: Id, expires_at: Option<Time>, now: Time) {
        let held = self.leases.entry(lease).or_default();
        held.in_flight = false;
        held.last = Some(now);
        held.expires_at = expires_at;
        held.needed = None;
        held.count = held.count.saturating_add(1);
    }

    /// The issue that was begun gave nothing.
    pub fn failed(&mut self, lease: Id) {
        if let Some(held) = self.leases.get_mut(&lease) {
            held.in_flight = false;
        }
    }

    /// The harness says it lacks a credential.
    pub fn needs(&mut self, lease: Id, need: CredentialNeed) {
        self.leases.entry(lease).or_default().needed = Some(need);
    }

    /// The lease is closed: nothing more is remembered of it.
    pub fn forget(&mut self, lease: Id) {
        self.leases.remove(&lease);
    }

    /// Only the leases that are open are remembered.
    pub fn retain(&mut self, open: &[Id]) {
        self.leases.retain(|lease, _| open.contains(lease));
    }

    /// Begins a return or a declaration of loss for a provider's credential; false while one is under
    /// way, so that a credential comes home or is declared lost, never both.
    pub fn begin_return(&mut self, provider: Id) -> bool {
        self.returning.insert(provider)
    }

    /// The return or declaration that was begun is over.
    pub fn end_return(&mut self, provider: Id) {
        self.returning.remove(&provider);
    }

    /// What an earlier attempt to bring the provider's credential home left.
    #[must_use]
    pub fn unfinished(&self, provider: Id) -> Unfinished {
        self.unfinished.get(&provider).cloned().unwrap_or_default()
    }

    /// An attempt to bring the credential home failed; the next goes on from `sealed`, when there is any.
    pub fn failed_return(&mut self, provider: Id, sealed: Option<SealedCredential>) {
        let kept = self.unfinished.entry(provider).or_default();
        kept.attempt = kept.attempt.saturating_add(1);
        if sealed.is_some() {
            kept.sealed = sealed;
        }
    }

    /// The credential is home, or given up: nothing of the attempts is kept.
    pub fn forget_return(&mut self, provider: Id) {
        self.unfinished.remove(&provider);
    }
}

/// What came of giving a credential to a harness.
#[derive(Debug)]
pub enum Given {
    /// The harness holds it, or needs nothing new.
    Ready,
    /// It could not be given, in the words a failed start carries.
    Failed(Box<crate::turns::StartFailure>),
}

impl crate::engine::Engine {
    /// Gives the harness of a starting turn what its arrangement lets leave the provider, when the path is
    /// one that does: a copy of the credential (`shared`), or the login itself (`handover`). The lease is
    /// opened here, once the machine where the harness runs is known: it is the one a copy may be issued
    /// to. A turn on any other path is left as it is.
    pub(crate) async fn give_credential(
        &self,
        snap: &crate::sessions::StartSnapshot,
    ) -> Result<Given, crate::error::EngineError> {
        let handover = match snap.path {
            CredentialPath::Shared => false,
            CredentialPath::Handover => true,
            CredentialPath::Native | CredentialPath::Forwarded => return Ok(Given::Ready),
        };
        if snap.arrangement.protocol.is_none() {
            let mut conn = self.inner.db.acquire().await?;
            let home = crate::provider_runtime::home_for_harness(
                &mut conn,
                snap.provider.id,
                snap.agent.id,
                snap.key.machine,
            )
            .await?;
            let home_owned = match home {
                Some(orch_driver_contract::runtime::EnvironmentDataSource::MadeByOrch) => true,
                Some(orch_driver_contract::runtime::EnvironmentDataSource::ExistingDirectory {
                    ..
                }) => false,
                None => {
                    orch_settings::harness_home_owned(
                        &mut conn,
                        snap.key.machine,
                        &snap.agent.harness_kind,
                    )
                    .await?
                }
            };
            let policy = if snap.provider.machine_id == Some(snap.key.machine) {
                orch_db::queries::environment_facts::of_provider(&mut conn, snap.provider.id)
                    .await?
                    .and_then(|machine| machine.policy)
            } else if snap.provider.key_machine_id == Some(snap.key.machine) {
                orch_db::queries::environment_facts::credential_policy(&mut conn, snap.provider.id)
                    .await?
            } else {
                orch_db::queries::environment_facts::of(&mut conn, snap.environment.id)
                    .await?
                    .and_then(|machine| machine.policy)
            };
            if !home_owned
                && policy
                    .as_ref()
                    .and_then(|policy| policy.get("provider.signin"))
                    .and_then(orch_types::Json::as_bool)
                    != Some(true)
            {
                let message="provider.signin: the receiving machine has not permitted installing a native sign-in in this existing harness home".to_owned();
                return Ok(Given::Failed(Box::new(crate::turns::StartFailure {
                    code: "policy.signin_not_allowed",
                    retryable: false,
                    blocked: crate::preflight::Blocked {
                        reason: "binding_invalid",
                        detail: Some(message.clone()),
                    },
                    message,
                })));
            }
        }
        // A native issuer is a service of its keeper. Starting another runtime never starts it implicitly.
        if snap.provider.key_machine_id != snap.provider.machine_id
            && let (Some(agent), Some(machine)) = (
                snap.provider.agent_id,
                crate::leases::machine_of(&snap.provider),
            )
        {
            let profile = {
                let mut conn = self.inner.db.acquire().await?;
                orch_db::queries::resources::thread_engine::agent(&mut conn, agent).await?
            };
            let Some(profile) = profile else {
                return Ok(Given::Failed(Box::new(no_form())));
            };
            let started = self
                .ensure_harness(
                    crate::harnesses::HarnessKey {
                        provider: snap.provider.id,
                        agent,
                        machine,
                    },
                    &profile,
                    orch_types::new_id(),
                )
                .await?;
            if let crate::harnesses::Started::Failed(outcome) = started {
                return Ok(Given::Failed(Box::new(crate::turns::classify(&outcome))));
            }
        }
        let Some(form) = snap.arrangement.form.as_ref().map(|form| form.form.clone()) else {
            return Ok(Given::Failed(Box::new(no_form())));
        };
        let lease = match &snap.lease {
            Some(lease) if lease.closed_at.is_none() => lease.clone(),
            _ => match self
                .open_lease_for_credential(snap, &form, handover)
                .await?
            {
                Some(lease) => lease,
                None => return Ok(Given::Failed(Box::new(no_form()))),
            },
        };
        if handover {
            self.hand_over_credential(snap, &lease, &form).await
        } else {
            self.share_credential(snap, &lease, &form).await
        }
    }

    /// Opens the lease of a turn that is given a credential, with the form and the machine where its
    /// harness runs, and names it on the turn.
    async fn open_lease_for_credential(
        &self,
        snap: &crate::sessions::StartSnapshot,
        form: &str,
        handover: bool,
    ) -> Result<Option<orch_db::queries::threads::LeaseRow>, crate::error::EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let lease = {
            let mut unit = crate::unit::Unit::new(&mut tx, self.inner.clock.now());
            let lease = crate::leases::open_for_credential(
                &mut unit,
                snap.thread.node_id,
                snap.provider.id,
                &snap.turn.model,
                form,
                handover,
                // A copy that is shared names its recipient from the start, as the table requires; a
                // credential that is handed over names it once it has been delivered.
                (!handover).then_some(snap.key.machine),
            )
            .await?;
            if let Some(lease) = &lease {
                orch_db::queries::threads::set_lease_turn(&mut *unit.conn, lease.id, snap.turn.id)
                    .await?;
                orch_db::queries::resources::provider_runtime::set_turn_lease(
                    &mut *unit.conn,
                    snap.turn.id,
                    lease.id,
                )
                .await?;
            }
            lease
        };
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(lease)
    }

    /// A machine, with the key that what is sealed for it is sealed to.
    pub(crate) async fn machine_key(
        &self,
        machine: Id,
    ) -> Result<MachineKey, crate::error::EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let row = orch_db::queries::resources::machine(&mut conn, machine)
            .await?
            .ok_or_else(|| {
                crate::error::EngineError::unreadable(format!("there is no machine {machine}"))
            })?;
        Ok(MachineKey {
            machine_id: row.id,
            public_key: row.link().public_key,
        })
    }
}

/// The words of a turn whose arrangement leaves the provider with no form of the credential.
fn no_form() -> crate::turns::StartFailure {
    let message =
        "the arrangement names no form of the credential that leaves the provider".to_owned();
    crate::turns::StartFailure {
        code: "harness_error",
        blocked: crate::preflight::Blocked {
            reason: "binding_invalid",
            detail: Some(message.clone()),
        },
        message,
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn at(seconds: i64) -> Time {
        Time::new(chrono::DateTime::from_timestamp(1_800_000_000 + seconds, 0).unwrap())
    }

    #[test]
    fn a_copy_is_not_issued_while_one_is_in_flight_nor_within_thirty_seconds_of_the_last() {
        let lease = new_id();
        let mut issuing = Issuing::default();
        assert_eq!(issuing.begin(lease, at(0)), Some(0));
        assert_eq!(issuing.begin(lease, at(1)), None, "one is in flight");
        issuing.done(lease, Some(at(3_600)), at(2));
        assert_eq!(
            issuing.begin(lease, at(20)),
            None,
            "the last was issued 18 seconds ago"
        );
        assert_eq!(
            issuing.begin(lease, at(32)),
            Some(1),
            "30 seconds have passed"
        );
        issuing.failed(lease);
        assert_eq!(
            issuing.begin(lease, at(33)),
            Some(1),
            "an issue that gave nothing is not counted"
        );
    }

    #[test]
    fn what_the_harness_lacks_is_remembered_until_a_copy_is_given() {
        let lease = new_id();
        let mut issuing = Issuing::default();
        issuing.needs(lease, CredentialNeed::Expiring);
        assert_eq!(issuing.of(lease).needed, Some(CredentialNeed::Expiring));
        issuing.begin(lease, at(0));
        issuing.done(lease, None, at(1));
        assert_eq!(issuing.of(lease).needed, None);
        issuing.forget(lease);
        assert_eq!(issuing.of(lease), Issued::default());
    }

    #[test]
    fn a_return_that_failed_goes_on_from_the_credential_the_environment_gave_back() {
        let provider = new_id();
        let mut issuing = Issuing::default();
        assert_eq!(issuing.unfinished(provider).attempt, 0);
        let sealed: SealedCredential = serde_json::from_value(serde_json::json!({
            "form": "login", "credential": { "$sealed": "c2VhbGVk" }, "expires_at": null
        }))
        .unwrap();
        issuing.failed_return(provider, Some(sealed.clone()));
        issuing.failed_return(provider, None);
        let kept = issuing.unfinished(provider);
        assert_eq!(kept.attempt, 2, "each failure names the next attempt");
        assert_eq!(
            kept.sealed,
            Some(sealed),
            "and what was given back is kept sealed"
        );
        issuing.forget_return(provider);
        assert!(issuing.unfinished(provider).sealed.is_none());
    }

    #[test]
    fn a_return_and_a_declaration_of_loss_do_not_run_together() {
        let provider = new_id();
        let mut issuing = Issuing::default();
        assert!(issuing.begin_return(provider));
        assert!(!issuing.begin_return(provider));
        issuing.end_return(provider);
        assert!(issuing.begin_return(provider));
    }
}
