//! Exclusive custody transfer, return, retry and lost-credential handling.
//!
//! The login of a provider handed over to an environment, and brought home: taken in one step when a turn starts, delivered to the harness there, returned by a person or by the engine, or given up.
//!
//! A handover moves the credential. The provider says where it is (`held_by_environment_id`, empty at
//! home); while it is away the provider is the holding environment's alone, and the provider's runtime
//! no longer uses the login. It comes back in one of a few ways: a person asks, the environment is about to
//! stop, move or go, no thread there is bound to the provider as `handed_over` any longer, or the provider is
//! switched off. Every way comes down to [`Engine::return_credential`]: the environment's harness gives back
//! its current material, sealed to the provider's runtime, and the runtime signs its harness in again. When
//! that fails the credential stays where it is and the operation that needed it fails with
//! `credential.return_failed`; a person may try again, or give the credential up
//! ([`Engine::declare_credential_lost`]), which clears the holder and puts the provider in `needs_setup`.
//! A credential comes home or is given up, never both: one of them runs at a time for a provider, and the
//! holder is cleared by a compare-and-set that only one of them wins.

use super::Given;
use super::handover_authority::{
    HandoverAuthority, HandoverCommandAuthority, ReturnSource, refused,
};
use super::sharing::Issue;
use crate::commands::{Outcome, Outgoing};
use crate::engine::{Engine, lock};
use crate::error::EngineError;
use crate::ids;
use crate::provider_setup::failure_text;
use crate::providers::classify_keeper;
use crate::sessions::StartSnapshot;
use crate::unit::Unit;
use orch_access_contract::audit::AuditAction;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::environments::thread_engine::{self as environments, EnvironmentRow};
use orch_db::queries::resources::provider_runtime as rows;
use orch_db::queries::resources::thread_engine::ProviderRow;
use orch_db::queries::threads::{self, LeaseRow};
use orch_link_contract::commands::{
    AgentCredentialReturn, ProviderCredentialAccept, SealedCredential, SerialKey,
};
use orch_types::{Id, new_id};
use serde_json::json;
use std::time::Duration;

/// How long a return waits for the provider's runtime to be up, so that the credential can be given back
/// to it: a unit is started in well under a minute when its image and volume are there, and three minutes
/// leaves room for the rest.
const RUNTIME_WAIT: Duration = Duration::from_secs(180);
/// How often the provider's runtime is looked at while a return waits for it.
const LOOK_EVERY: Duration = Duration::from_millis(250);
/// How long a return that interrupts turns waits for them to end, beyond the engine's own wait for an
/// interrupt to be answered.
const INTERRUPT_MARGIN: Duration = Duration::from_secs(5);
/// How many handovers one look at the schedule takes up.
const SCHEDULE_BATCH: i64 = 100;
/// What a provider says when its credential was given up.
const LOST_DETAIL: &str =
    "the credential that was handed over was given up: the provider has to be signed in again";

/// Bookkeeping ownership ends even if the HTTP future is cancelled; durable commands keep their rows.
struct Returning {
    engine: Engine,
    provider: Id,
}
impl Drop for Returning {
    fn drop(&mut self) {
        lock(&self.engine.inner.credentials).end_return(self.provider);
    }
}

/// Why a credential is brought home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnWhy {
    /// A person asked. The turns of the holding environment that run on the provider are interrupted when
    /// the request says so, and refuse the return otherwise.
    Person {
        /// Who.
        by: Id,
        /// Whether turns that run may be interrupted.
        interrupt: bool,
    },
    /// The engine does it: the environment is about to stop, move or be removed, no thread there needs the
    /// login any longer, or the provider is switched off. Turns that run are not interrupted by this.
    Engine,
}

impl ReturnWhy {
    fn person(self) -> Option<Id> {
        match self {
            Self::Person { by, .. } => Some(by),
            Self::Engine => None,
        }
    }
}

/// Why a credential could not be brought home or given up.
#[derive(Debug, thiserror::Error)]
pub enum ReturnError {
    /// The provider's credential is not handed over.
    #[error("the credential of this provider is not handed over")]
    NotHeld,
    /// Turns of the holding environment run on the provider, and the request did not say to interrupt.
    #[error("{0} turns run on the provider in the environment that holds its credential")]
    Busy(i64),
    /// The credential could not be brought home: the environment was not reached, the material was
    /// refused, or the provider's runtime did not take it.
    #[error("the credential could not be brought home: {0}")]
    Failed(String),
    /// The system failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<orch_db::DbError> for ReturnError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<crate::commands::CommandsError> for ReturnError {
    fn from(error: crate::commands::CommandsError) -> Self {
        Self::Internal(error.into())
    }
}

impl Engine {
    // ------------------------------------------------------------ taking it

    /// Takes a provider's credential for an environment, in the transaction that starts the turn: one
    /// compare-and-set on the credential being at home. Whether this call took it.
    pub(crate) async fn take_credential(
        &self,
        unit: &mut Unit<'_>,
        provider: &ProviderRow,
        environment: &EnvironmentRow,
        thread: Id,
        by: Option<Id>,
    ) -> Result<bool, EngineError> {
        let now = unit.now();
        if !rows::take_credential(&mut *unit.conn, provider.id, environment.id, now).await? {
            return Ok(false);
        }
        let mut entry = Entry::new(
            by.map_or(Actor::System, Actor::User),
            orch_db::audit::action!(AuditAction::ProviderHandedOver).as_str(),
            "provider",
            Some(provider.id),
            "handed the credential of a provider over to an environment",
        );
        entry.history = AuditAction::ProviderHandedOver.is_history_line();
        entry.node_id = Some(thread);
        entry.environment_id = Some(environment.id);
        entry.details = json!({ "environment": environment.id, "thread": thread });
        audit::append(&mut *unit.conn, &entry, &Origin::default(), now).await?;
        Ok(true)
    }

    /// Gives the harness of a starting turn the login itself, when the credential was taken for its
    /// environment and has not been delivered yet. The sessions the provider's own harness holds are
    /// closed, the lease is opened where the credential is kept, and the provider's runtime issues the
    /// transferred form, sealed to the machine of the harness. A turn in an environment that holds the
    /// credential already finds it there and needs nothing. A credential that could not be delivered is
    /// not held: it is still at home, unless the provider's runtime issued it and the harness could not
    /// take it, which only a person can mend.
    pub(crate) async fn hand_over_credential(
        &self,
        snap: &StartSnapshot,
        lease: &LeaseRow,
        form: &str,
    ) -> Result<Given, EngineError> {
        let delivered = {
            let mut conn = self.inner.db.acquire().await?;
            rows::credential_delivered(&mut conn, snap.provider.id).await?
        };
        if delivered {
            return Ok(Given::Ready);
        }
        if let Some(home) = snap.provider.machine_id {
            self.close_sessions_on(snap.provider.id, home).await;
        }
        let recipient = self.machine_key(snap.key.machine).await?;
        let opened = self
            .open_credential_lease_at_provider(lease, &snap.provider, &recipient)
            .await?;
        if !matches!(opened, Outcome::Succeeded(_)) {
            self.undo_take(snap.provider.id, snap.environment.id)
                .await?;
            return Ok(Given::Failed(Box::new(classify_keeper(
                "lease.open",
                &opened,
            ))));
        }
        let issued = self
            .issue_copy(
                lease,
                &snap.provider,
                snap.key,
                form,
                snap.arrangement.protocol.as_deref(),
            )
            .await?;
        match issued {
            Issue::Given(_) | Issue::NotNow => {
                let mut conn = self.inner.db.acquire().await?;
                rows::set_lease_recipient(&mut conn, lease.id, snap.key.machine).await?;
                Ok(Given::Ready)
            }
            Issue::Failed(failure) => {
                self.undo_take(snap.provider.id, snap.environment.id)
                    .await?;
                Ok(Given::Failed(failure))
            }
            Issue::Lost(failure) => Ok(Given::Failed(failure)),
        }
    }

    /// The credential was taken for an environment and then not issued: it is still at home, and the
    /// holder is cleared again.
    async fn undo_take(&self, provider: Id, environment: Id) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        if rows::release_credential(&mut tx, provider, environment, now).await? {
            let mut entry = Entry::new(
                Actor::System,
                orch_db::audit::action!(AuditAction::ProviderReturned).as_str(),
                "provider",
                Some(provider),
                "the credential of a provider was not handed over after all",
            );
            entry.history = AuditAction::ProviderReturned.is_history_line();
            entry.environment_id = Some(environment);
            entry.details = json!({ "environment": environment, "undone": true });
            audit::append(&mut tx, &entry, &Origin::default(), now).await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// Closes the sessions the harness processes of a provider hold open on a machine, so that none is
    /// left on a login that has moved. The harness is not waited for: it may well be gone.
    async fn close_sessions_on(&self, provider: Id, machine: Id) {
        let _ = self
            .close_sessions_for_return(provider, machine, None)
            .await;
    }

    async fn close_sessions_for_return(
        &self,
        provider: Id,
        machine: Id,
        authority: Option<&HandoverAuthority>,
    ) -> Result<(), ReturnError> {
        let open = lock(&self.inner.sessions).of_provider(provider);
        for (segment, thread, key) in open {
            if key.machine != machine {
                continue;
            }
            self.check_return_authority(authority).await?;
            let command = {
                match self.db().acquire().await {
                    Ok(mut conn) => {
                        crate::commands::session_close(
                            &mut conn,
                            new_id(),
                            machine,
                            thread,
                            Some(segment),
                        )
                        .await
                    }
                    Err(error) => Err(error.into()),
                }
            };
            match command {
                Ok(command) => {
                    match crate::commands::run(self.inner.commands.as_ref(), command).await {
                        Ok(crate::commands::Outcome::Succeeded(_)) => {}
                        Ok(outcome) => {
                            tracing::warn!(?outcome,%thread,"a session refused to close before a credential moved")
                        }
                        Err(error) => {
                            tracing::warn!(%error,%thread,"a session could not be closed before a credential moved")
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error,%thread,"the exact session cleanup could not be prepared before a credential moved")
                }
            }
            lock(&self.inner.sessions).closed(segment);
        }
        Ok(())
    }

    // ------------------------------------------------------------ bringing it home

    /// Brings the credential of a provider home from the environment that holds it: the turns that run
    /// there on the provider are interrupted where the request says so (and refuse it otherwise), the
    /// harness gives back its material sealed to the provider's runtime, which is started if it must be,
    /// and the runtime signs its harness in again. Only then is the holder cleared. A return that failed
    /// leaves the credential where it was, and what the environment gave back is kept sealed for the next
    /// try, so that trying again goes on from where it stopped.
    pub async fn return_credential(&self, provider: Id, why: ReturnWhy) -> Result<(), ReturnError> {
        self.return_credential_with_authority(provider, why, None)
            .await
            .map(|_| ())
    }

    pub(crate) async fn return_credential_with_authority(
        &self,
        provider: Id,
        why: ReturnWhy,
        authority: Option<&HandoverAuthority>,
    ) -> Result<Option<i64>, ReturnError> {
        if !lock(&self.inner.credentials).begin_return(provider) {
            return Err(ReturnError::Failed(
                "another return of this credential, or a declaration that it is lost, is under way"
                    .to_owned(),
            ));
        }
        let _returning = Returning {
            engine: self.clone(),
            provider,
        };
        self.bring_home(provider, why, authority).await
    }

    async fn check_return_authority(
        &self,
        authority: Option<&HandoverAuthority>,
    ) -> Result<(), ReturnError> {
        if let Some(authority) = authority {
            let mut tx = self.inner.db.begin().await?;
            orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
            authority.require(&mut tx, self.inner.clock.time()).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        Ok(())
    }

    async fn bring_home(
        &self,
        provider: Id,
        why: ReturnWhy,
        authority: Option<&HandoverAuthority>,
    ) -> Result<Option<i64>, ReturnError> {
        self.check_return_authority(authority).await?;
        let (state, since) = {
            let mut conn = self.inner.db.acquire().await?;
            (
                rows::state(&mut conn, provider).await?,
                rows::held_since(&mut conn, provider).await?,
            )
        };
        let Some(state) = state.filter(|state| state.deleted_at.is_none()) else {
            return Err(ReturnError::NotHeld);
        };
        let (Some(environment_id), Some(since)) = (state.held_by_environment_id, since) else {
            return Err(ReturnError::NotHeld);
        };
        self.clear_handover_turns(provider, environment_id, why, authority)
            .await?;
        let environment = {
            let mut conn = self.inner.db.acquire().await?;
            environments::environment(&mut conn, environment_id).await?
        };
        let Some(machine) = environment.and_then(|environment| environment.machine_id) else {
            return Err(ReturnError::Failed(
                "the environment that holds it has no machine to take it back from".to_owned(),
            ));
        };
        self.close_sessions_for_return(provider, machine, authority)
            .await?;
        let agent = {
            let mut conn = self.inner.db.acquire().await?;
            rows::harness_agent_on(&mut conn, provider, machine).await?
        }
        .or(state.agent_id)
        .ok_or_else(|| {
            ReturnError::Failed("no profile's harness is known to hold the credential".to_owned())
        })?;
        let keeper = self
            .provider_machine(&state, why.person(), authority)
            .await?;
        let custodian = self.machine_key(keeper).await?;
        let kept = lock(&self.inner.credentials).unfinished(provider);
        let stamp = since.timestamp_micros().to_le_bytes();
        let attempt = kept.attempt.to_le_bytes();
        let return_id = ids::derived(
            "agent.credential.return",
            &[provider.as_bytes().as_slice(), &stamp, &attempt],
        );
        let source = match (kept.source.as_ref(), kept.sealed.as_ref()) {
            (Some(source), Some(_)) => {
                let mut conn = self.inner.db.acquire().await?;
                if source.provider != provider
                    || source.holder != environment_id
                    || source.since != since
                    || source.custodian != custodian
                    || !source.current(&mut conn, false).await?
                {
                    return Err(refused());
                }
                if let Some(authority) = authority {
                    authority
                        .require(&mut conn, self.inner.clock.time())
                        .await?;
                }
                Some(source.clone())
            }
            (None, Some(_)) => return Err(refused()),
            (_, None) => {
                if let Some(source) = self
                    .inner
                    .commands
                    .retained_return_source(return_id)
                    .await?
                {
                    let mut conn = self.inner.db.acquire().await?;
                    let issuing =
                        self.inner
                            .commands
                            .status(return_id)
                            .await?
                            .is_none_or(|status| {
                                matches!(
                                    status.stage,
                                    crate::commands::Stage::Queued | crate::commands::Stage::Sent
                                )
                            });
                    if source.provider != provider
                        || source.holder != environment_id
                        || source.since != since
                        || source.return_command != return_id
                        || source.custodian != custodian
                        || !source.current(&mut conn, issuing).await?
                    {
                        return Err(refused());
                    }
                    Some(source)
                } else {
                    // A retained command without its actual incarnation is not a source for a new one.
                    if self
                        .inner
                        .commands
                        .retained_command(return_id)
                        .await?
                        .is_some()
                    {
                        return Err(refused());
                    }
                    let mut conn = self.inner.db.acquire().await?;
                    Some(
                        ReturnSource::capture(
                            &mut conn,
                            provider,
                            environment_id,
                            since,
                            agent,
                            custodian.clone(),
                            return_id,
                        )
                        .await?,
                    )
                }
            }
        };
        // A retained accepted result belongs to its actual profile, even if another is now preferred.
        let agent = source.as_ref().map_or(agent, |source| source.agent);
        let sealed = match kept.sealed {
            Some(sealed) => sealed,
            None => {
                self.check_return_authority(authority).await?;
                let mut command = Outgoing::new(
                    return_id,
                    machine,
                    SerialKey::Harness {
                        provider_id: provider,
                        agent_id: agent,
                    },
                    &AgentCredentialReturn {
                        provider_id: provider,
                        agent_id: agent,
                        custodian,
                    },
                )
                .map_err(EngineError::from)?;
                if let Some(source) = source.as_ref() {
                    command = command.with_login_authority(
                        crate::provider_setup::LoginCommandAuthority::handover(
                            HandoverCommandAuthority::returning(authority, source),
                        ),
                    );
                }
                match crate::commands::run(self.inner.commands.as_ref(), command).await? {
                    Outcome::Succeeded(result) => {
                        serde_json::from_value::<SealedCredential>(result)
                            .map_err(EngineError::from)?
                    }
                    other => {
                        lock(&self.inner.credentials).failed_return(provider, None);
                        return Err(ReturnError::Failed(format!(
                            "the environment did not give it back: {}",
                            failure_text(&other)
                        )));
                    }
                }
            }
        };
        // Retain actual moved material before any later await or current-authority refusal.
        if let Some(source) = source.as_ref() {
            lock(&self.inner.credentials).keep_return(provider, sealed.clone(), source.clone());
        }
        if let Err(error) = self.check_return_authority(authority).await {
            lock(&self.inner.credentials).failed_return(provider, Some(sealed));
            return Err(error);
        }
        if let Some(source) = source.as_ref() {
            let mut conn = self.inner.db.acquire().await?;
            if !source.current(&mut conn, false).await? {
                return Err(refused());
            }
        }
        let accept_id = ids::derived(
            "provider.credential.accept",
            &[provider.as_bytes().as_slice(), &stamp, &attempt],
        );
        let mut accept = Outgoing::new(
            accept_id,
            keeper,
            SerialKey::Provider(provider),
            &ProviderCredentialAccept {
                provider_id: provider,
                form: sealed.form.clone(),
                credential: sealed.credential.clone(),
                expires_at: sealed.expires_at,
            },
        )
        .map_err(EngineError::from)?;
        if let Some(source) = source.as_ref() {
            accept = accept.with_login_authority(
                crate::provider_setup::LoginCommandAuthority::handover(
                    HandoverCommandAuthority::accepting(authority, source, accept_id, &sealed)?,
                ),
            );
        }
        match crate::commands::run(self.inner.commands.as_ref(), accept).await? {
            Outcome::Succeeded(_) => {}
            other => {
                lock(&self.inner.credentials).failed_return(provider, Some(sealed));
                return Err(ReturnError::Failed(format!(
                    "the provider's runtime did not take it: {}",
                    failure_text(&other)
                )));
            }
        }
        let revision = self
            .credential_is_home(provider, environment_id, since, why, source.as_ref())
            .await?;
        lock(&self.inner.credentials).forget_return(provider);
        Ok(revision)
    }

    /// The provider's runtime is home to the credential: the holder is cleared, the return is written
    /// down, and the threads that waited for it are looked at again.
    async fn credential_is_home(
        &self,
        provider: Id,
        environment: Id,
        since: chrono::DateTime<chrono::Utc>,
        why: ReturnWhy,
        source: Option<&ReturnSource>,
    ) -> Result<Option<i64>, ReturnError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        if let Some(source) = source {
            if !source.current(&mut tx, false).await? {
                return Err(refused());
            }
        }
        // Reconcile a completed native accept even after caller/key loss, only for its exact custody.
        let released = rows::release_credential_incarnation(
            &mut tx,
            provider,
            environment,
            since,
            source.map(|source| source.generation),
            source.map(|source| source.holder_machine),
            now,
        )
        .await?;
        if released {
            let mut entry = Entry::new(
                why.person().map_or(Actor::System, Actor::User),
                AuditAction::ProviderReturned.as_str(),
                "provider",
                Some(provider),
                "the credential of a provider came home",
            );
            entry.history = AuditAction::ProviderReturned.is_history_line();
            entry.environment_id = Some(environment);
            entry.details = json!({ "environment": environment });
            audit::append(&mut tx, &entry, &Origin::default(), now).await?;
        }
        let blocked = rows::blocked_threads_of(&mut tx, provider).await?;
        // Capture the actual accepted revision in this terminal transaction. Automatic
        // callers retain their prior unit result even if a removed row has no revision.
        let revision = rows::state(&mut tx, provider)
            .await?
            .map(|state| state.revision);
        tx.commit().await.map_err(orch_db::DbError::from)?;
        for thread in blocked {
            self.wake(thread);
        }
        Ok(revision)
    }

    /// Gives a credential up: the environment cannot give it back, and its holder is cleared with the
    /// provider in `needs_setup`; it has to be signed in again, and whatever copy remains in the
    /// environment is no longer Orch's to vouch for. Only for a credential that is away.
    pub async fn declare_credential_lost(&self, provider: Id, by: Id) -> Result<(), ReturnError> {
        self.declare_credential_lost_with_authority(provider, by, None)
            .await
            .map(|_| ())
    }

    pub(crate) async fn declare_credential_lost_with_authority(
        &self,
        provider: Id,
        by: Id,
        authority: Option<&HandoverAuthority>,
    ) -> Result<Option<i64>, ReturnError> {
        if !lock(&self.inner.credentials).begin_return(provider) {
            return Err(ReturnError::Failed(
                "a return of this credential is under way".to_owned(),
            ));
        }
        let _returning = Returning {
            engine: self.clone(),
            provider,
        };
        self.give_up_credential(provider, by, authority).await
    }

    async fn give_up_credential(
        &self,
        provider: Id,
        by: Id,
        authority: Option<&HandoverAuthority>,
    ) -> Result<Option<i64>, ReturnError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        if let Some(authority) = authority {
            orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
            authority.require(&mut tx, self.inner.clock.time()).await?;
        }
        let state = rows::lock(&mut tx, provider)
            .await?
            .filter(|state| state.deleted_at.is_none());
        // The row lock can wait; bind the loss to the accepted custody again after it.
        if let Some(authority) = authority {
            authority.require(&mut tx, self.inner.clock.time()).await?;
        }
        let Some(environment) = state.and_then(|state| state.held_by_environment_id) else {
            return Err(ReturnError::NotHeld);
        };
        if !rows::declare_credential_lost(&mut tx, provider, environment, LOST_DETAIL, now).await? {
            return Err(ReturnError::NotHeld);
        }
        rows::close_handover_leases(&mut tx, provider, "error", now).await?;
        let mut entry = Entry::new(
            Actor::User(by),
            orch_db::audit::action!(AuditAction::ProviderHandoverLost).as_str(),
            "provider",
            Some(provider),
            "gave up the credential of a provider that could not be brought home",
        );
        entry.history = AuditAction::ProviderHandoverLost.is_history_line();
        entry.environment_id = Some(environment);
        entry.details = json!({ "environment": environment });
        audit::append(&mut tx, &entry, &Origin::default(), now).await?;
        let blocked = rows::blocked_threads_of(&mut tx, provider).await?;
        // Capture the actual accepted revision in this terminal transaction. Automatic
        // callers retain their prior unit result even if a removed row has no revision.
        let revision = rows::state(&mut tx, provider)
            .await?
            .map(|state| state.revision);
        tx.commit().await.map_err(orch_db::DbError::from)?;
        lock(&self.inner.credentials).forget_return(provider);
        for thread in blocked {
            self.wake(thread);
        }
        Ok(revision)
    }

    /// The machine of the provider's runtime, started and waited for if it is not up: the credential is
    /// given back to it.
    async fn provider_machine(
        &self,
        state: &rows::State,
        by: Option<Id>,
        authority: Option<&HandoverAuthority>,
    ) -> Result<Id, ReturnError> {
        let began = tokio::time::Instant::now();
        let mut started = false;
        loop {
            self.check_return_authority(authority).await?;
            let current = {
                let mut conn = self.inner.db.acquire().await?;
                rows::state(&mut conn, state.id).await?
            };
            let Some(current) = current.filter(|current| current.deleted_at.is_none()) else {
                return Err(ReturnError::Failed("the provider is gone".to_owned()));
            };
            if let Some(machine) = self.runtime_machine(&current).await? {
                return Ok(machine);
            }
            let res = {
                let mut conn = self.inner.db.acquire().await?;
                orch_db::queries::environment_facts::of_provider(&mut conn, current.id)
                    .await?
                    .is_some_and(|facts| facts.attaches())
            };
            if res {
                // A provider on a person's machine is where that machine is.
                return current.machine_id.ok_or_else(|| {
                    ReturnError::Failed("the machine of the provider is away".to_owned())
                });
            }
            if !started {
                started = true;
                match self
                    .start_provider_runtime_for_return(current.id, by, authority)
                    .await
                {
                    Ok(_) => {}
                    Err(crate::provider_runtime::RuntimeError::Internal(error)) => {
                        return Err(ReturnError::Internal(error));
                    }
                    Err(other) => {
                        return Err(ReturnError::Failed(format!(
                            "the provider's runtime cannot be started: {other}"
                        )));
                    }
                }
            }
            if began.elapsed() >= RUNTIME_WAIT {
                return Err(ReturnError::Failed(
                    "the provider's runtime did not come up in time".to_owned(),
                ));
            }
            tokio::time::sleep(LOOK_EVERY).await;
        }
    }

    /// Makes sure no turn of the holding environment runs on the provider: it fails when there is one and
    /// the request did not say to interrupt; otherwise the turns are asked to stop, and waited for.
    async fn clear_handover_turns(
        &self,
        provider: Id,
        environment: Id,
        why: ReturnWhy,
        authority: Option<&HandoverAuthority>,
    ) -> Result<(), ReturnError> {
        let running = {
            let mut conn = self.inner.db.acquire().await?;
            rows::threads_running_in(&mut conn, provider, environment).await?
        };
        if running.is_empty() {
            return Ok(());
        }
        let ReturnWhy::Person {
            by,
            interrupt: true,
        } = why
        else {
            return Err(ReturnError::Busy(
                i64::try_from(running.len()).unwrap_or(i64::MAX),
            ));
        };
        for thread in running {
            let mut tx = self.inner.db.begin().await?;
            if let Some(authority) = authority {
                orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
                authority.require(&mut tx, self.inner.clock.time()).await?;
            }
            let finished = {
                let mut unit = Unit::new(&mut tx, self.inner.clock.now());
                if threads::lock(&mut *unit.conn, thread).await?.is_none() {
                    continue;
                }
                if let Some(authority) = authority {
                    authority
                        .require(&mut *unit.conn, self.inner.clock.time())
                        .await?;
                }
                if !rows::threads_running_in(&mut *unit.conn, provider, environment)
                    .await?
                    .contains(&thread)
                {
                    continue;
                }
                if let Some(turn) = threads::active_turn(&mut *unit.conn, thread).await? {
                    let seq = unit.stamp(thread).await?;
                    threads::mark_interrupted_by(&mut *unit.conn, turn.id, by, seq).await?;
                    unit.wake(thread);
                }
                unit.finish().await?
            };
            self.commit(tx, finished).await?;
            self.drive_interrupt(thread).await?;
        }
        let wait = self.inner.config.interrupt_force + INTERRUPT_MARGIN;
        let began = tokio::time::Instant::now();
        loop {
            self.check_return_authority(authority).await?;
            let left = {
                let mut conn = self.inner.db.acquire().await?;
                rows::threads_running_in(&mut conn, provider, environment)
                    .await?
                    .len()
            };
            if left == 0 {
                return Ok(());
            }
            if began.elapsed() >= wait {
                return Err(ReturnError::Busy(i64::try_from(left).unwrap_or(i64::MAX)));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Brings home every credential that was handed over to an environment, as the first thing that is
    /// done before it stops, moves or goes. The first that cannot be brought home ends it, and the
    /// environment is left as it is.
    pub(crate) async fn return_credentials_of(&self, environment: Id) -> Result<(), ReturnError> {
        let held = {
            let mut conn = self.inner.db.acquire().await?;
            orch_db::queries::environments::hosting::providers_holding(&mut conn, environment)
                .await?
        };
        for provider in held {
            match self.return_credential(provider, ReturnWhy::Engine).await {
                Ok(()) | Err(ReturnError::NotHeld) => {}
                Err(other) => return Err(other),
            }
        }
        Ok(())
    }

    /// Brings home the credentials that no environment needs any longer: no thread there is bound to the
    /// provider as `handed_over` (archived or removed ones do not count), or the provider is switched
    /// off. One whose turn still runs is left to it. Looked at on the schedule's pace.
    pub(crate) async fn return_unneeded_handovers(&self) -> Result<(), EngineError> {
        let due = {
            let mut conn = self.inner.db.acquire().await?;
            rows::handovers_no_longer_needed(&mut conn, SCHEDULE_BATCH).await?
        };
        for (provider, environment) in due {
            let running = {
                let mut conn = self.inner.db.acquire().await?;
                rows::threads_running_in(&mut conn, provider, environment).await?
            };
            if !running.is_empty() {
                continue;
            }
            match self.return_credential(provider, ReturnWhy::Engine).await {
                Ok(()) | Err(ReturnError::NotHeld) => {}
                Err(ReturnError::Internal(error)) => return Err(error),
                Err(other) => {
                    tracing::warn!(%provider, %other, "a credential that is not needed was not brought home; it is looked at again");
                }
            }
        }
        Ok(())
    }
}
