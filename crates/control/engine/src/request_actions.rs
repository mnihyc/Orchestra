//! Engine requests accepted in the caller's action transaction before commands are sent to machines.
//!
//! The HTTP caller supplies the transaction and principal of its current action
//! guard. The accepted decision is durable before the existing command workflow
//! starts, so that a refused caller changes nothing and sends no command. Machine
//! answers and recovery remain continuations of that committed decision.

use crate::Engine;
use crate::credentials::handover::{ReturnError, ReturnWhy};
use crate::environments::{StopError, StopReason};
use crate::error::EngineError;
use crate::harnesses::Restarted;
use crate::provider_runtime::{RuntimeError, RuntimeStart, StopWhy};
use crate::provider_setup::{Asked, Refreshed};
use orch_access_contract::audit::AuditAction;
use orch_authz::Principal;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::environments::hosting as environments;
use orch_db::queries::resources::routes as resources;
use orch_db::{DbError, Tx};
use orch_types::Id;
use serde_json::json;

/// Records an accepted provider operation using the caller's existing guard.
async fn provider_decision(
    mut tx: Tx,
    principal: &Principal,
    origin: &Origin,
    provider: Id,
    operation: &str,
) -> Result<(), EngineError> {
    let row = resources::providers(&mut tx, &[provider])
        .await?
        .pop()
        .ok_or_else(|| EngineError::unreadable("there is no such provider"))?;
    if operation == "sign out" && row.kind != "subscription" {
        tx.commit().await.map_err(DbError::from)?;
        return Ok(());
    }
    let summary = format!("A caller asked to {operation} the provider {}", row.name);
    let mut entry = Entry::new(
        Actor::User(principal.user()),
        AuditAction::ProviderUpdated.as_str(),
        "provider",
        Some(provider),
        &summary,
    );
    entry.history = true;
    entry.node_id = Some(row.home_id);
    entry.org_id = Some(row.org_id);
    entry.details = json!({"provider": provider, "operation": operation, "decision": "accepted"});
    audit::append(&mut tx, &entry, origin, principal.now().get()).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

impl Engine {
    /// Accepts the exact provider unit a host manager selected, including an older kept unit.
    #[expect(
        clippy::too_many_arguments,
        reason = "the request names the host and the unit and carries each choice the manager made"
    )]
    pub async fn stop_host_provider_unit_in(
        &self,
        mut tx: Tx,
        principal: &Principal,
        origin: &Origin,
        host: Id,
        runtime: Id,
        interrupt: bool,
        force: bool,
    ) -> Result<(), RuntimeError> {
        let row = environments::runtime(&mut tx, runtime)
            .await?
            .filter(|row| row.host_id == host && row.kind == "provider")
            .ok_or(RuntimeError::NotFound)?;
        let provider = row.provider_id.ok_or(RuntimeError::NotFound)?;
        tracing::info!(%provider, %runtime, %host, interrupt, force, state = %row.state, "a host manager asked to stop a provider's unit");
        // What can refuse comes first: nothing is ended for a stop that is then refused. Work of the provider's own on
        // the unit gives way to a person who forces the stop; work of something else does not.
        let in_the_way: Vec<Id> = if force {
            if orch_db::queries::jobs::changes::provider_hold_besides_own(&mut tx, provider)
                .await?
                .is_some()
            {
                return Err(RuntimeError::Held(crate::provider_runtime::Hold::of(
                    crate::provider_runtime::HoldWhy::OtherWork,
                )));
            }
            orch_db::queries::jobs::provider_jobs::open_of_provider(&mut tx, provider)
                .await?
                .into_iter()
                .map(|job| job.id)
                .collect()
        } else {
            if let Some(job) =
                orch_db::queries::jobs::changes::provider_hold(&mut tx, provider).await?
            {
                return Err(RuntimeError::Held(crate::provider_runtime::hold_of(&job)));
            }
            Vec::new()
        };
        if row.state != orch_db::queries::environments::hosting::runtime_state::RETIRED
            && !interrupt
        {
            let running = orch_db::queries::resources::provider_runtime::busy(&mut tx, provider)
                .await?
                .turns;
            if running > 0 {
                return Err(RuntimeError::Busy(running));
            }
        }
        let home = orch_db::queries::resources::hosting::host(&mut tx, host)
            .await?
            .ok_or(RuntimeError::NotFound)?;
        let mut entry = Entry::new(
            Actor::User(principal.user()),
            AuditAction::HostUpdated.as_str(),
            "host",
            Some(host),
            "A host manager asked to stop a provider unit",
        );
        entry.history = true;
        entry.node_id = home.home_id;
        entry.org_id = home.org_id;
        entry.details = json!({"host":host,"runtime":runtime,"provider":provider,"interrupt":interrupt,"force":force,"jobs_in_the_way":in_the_way,"decision":"accepted"});
        audit::append(&mut tx, &entry, origin, principal.now().get()).await?;
        tx.commit().await.map_err(DbError::from)?;
        self.stop_provider_unit(
            provider,
            runtime,
            interrupt,
            StopWhy::Person(principal.user()),
            force,
        )
        .await
    }

    /// Accepts returning or declaring a credential lost before changing its holder or sending commands.
    pub async fn return_credential_in(
        &self,
        mut tx: Tx,
        principal: &Principal,
        origin: &Origin,
        provider: Id,
        interrupt: bool,
        declare_lost: bool,
    ) -> Result<(), ReturnError> {
        // Classified/key callers cannot drop their operation by choosing the compatibility facade.
        if principal.caller().grant().is_some() || principal.caller().admitted_operation().is_some()
        {
            let authority = crate::credentials::handover_authority::HandoverAuthority::capture(
                &mut tx,
                principal.caller(),
                provider,
                interrupt,
                declare_lost,
                principal.now(),
            )
            .await?;
            return self
                .return_credential_operation_in(
                    tx,
                    principal,
                    origin,
                    provider,
                    interrupt,
                    declare_lost,
                    authority,
                )
                .await
                .map(|_| ());
        }
        provider_decision(
            tx,
            principal,
            origin,
            provider,
            "bring the credential home on",
        )
        .await?;
        if declare_lost {
            self.declare_credential_lost(provider, principal.user())
                .await
        } else {
            self.return_credential(
                provider,
                ReturnWhy::Person {
                    by: principal.user(),
                    interrupt,
                },
            )
            .await
        }
    }

    /// The same return writer with its exact accepted operation and custody source retained.
    #[expect(
        clippy::too_many_arguments,
        reason = "preserve the existing request facade and attach its captured operation"
    )]
    pub async fn return_credential_operation_in(
        &self,
        mut tx: Tx,
        principal: &Principal,
        origin: &Origin,
        provider: Id,
        interrupt: bool,
        declare_lost: bool,
        authority: crate::credentials::handover_authority::HandoverAuthority,
    ) -> Result<i64, ReturnError> {
        if !authority.matches(provider, principal.caller(), interrupt, declare_lost) {
            return Err(crate::credentials::handover_authority::refused());
        }
        authority.require(&mut tx, principal.now()).await?;
        provider_decision(
            tx,
            principal,
            origin,
            provider,
            "bring the credential home on",
        )
        .await?;
        let revision = if declare_lost {
            self.declare_credential_lost_with_authority(
                provider,
                principal.user(),
                Some(&authority),
            )
            .await
        } else {
            self.return_credential_with_authority(
                provider,
                ReturnWhy::Person {
                    by: principal.user(),
                    interrupt,
                },
                Some(&authority),
            )
            .await
        }?;
        // Actual terminal state, never a pre-return snapshot or guessed increment.
        revision.ok_or(ReturnError::NotHeld)
    }

    /// Accepts a runtime start in the caller's transaction before queuing the job that starts it.
    pub async fn start_provider_runtime_in(
        &self,
        tx: Tx,
        principal: &Principal,
        origin: &Origin,
        provider: Id,
    ) -> Result<RuntimeStart, RuntimeError> {
        provider_decision(tx, principal, origin, provider, "start").await?;
        self.start_provider_runtime(provider, Some(principal.user()))
            .await
    }

    /// Accepts a runtime stop in the caller's transaction before interrupting turns or telling its host.
    pub async fn stop_provider_runtime_in(
        &self,
        tx: Tx,
        principal: &Principal,
        origin: &Origin,
        provider: Id,
        interrupt: bool,
    ) -> Result<(), RuntimeError> {
        self.stop_provider_runtime_forced_in(tx, principal, origin, provider, interrupt, false)
            .await
    }

    /// As [`Engine::stop_provider_runtime_in`]; with `force`, a unit whose host does not confirm the stop is
    /// settled as stopped, and the job that works on it is ended.
    pub async fn stop_provider_runtime_forced_in(
        &self,
        tx: Tx,
        principal: &Principal,
        origin: &Origin,
        provider: Id,
        interrupt: bool,
        force: bool,
    ) -> Result<(), RuntimeError> {
        provider_decision(
            tx,
            principal,
            origin,
            provider,
            if force { "force the stop of" } else { "stop" },
        )
        .await?;
        self.stop_provider_runtime_with(
            provider,
            interrupt,
            StopWhy::Person(principal.user()),
            force,
        )
        .await
    }

    /// Accepts cancellation in the caller's transaction before forgetting pending values or telling its harness.
    pub async fn cancel_sign_in_in(
        &self,
        mut tx: Tx,
        principal: &Principal,
        origin: &Origin,
        provider: Id,
    ) -> Result<Asked, EngineError> {
        let plan = crate::provider_setup::login_operation(&mut tx, provider, "ProviderLoginCancel")
            .await?;
        let caller = plan
            .admit(principal.caller(), false)
            .map_err(EngineError::unreadable)?;
        let accepted =
            orch_authz::CapturedOperation::capture(&caller).map_err(EngineError::unreadable)?;
        provider_decision(tx, principal, origin, provider, "cancel the sign-in on").await?;
        self.cancel_sign_in_authorized(provider, accepted).await
    }

    /// Accepts a refresh in the caller's guarded transaction, then asks the machine after commit.
    pub async fn refresh_provider_in(
        &self,
        tx: Tx,
        principal: &Principal,
        origin: &Origin,
        provider: Id,
    ) -> Result<Refreshed, EngineError> {
        provider_decision(tx, principal, origin, provider, "refresh").await?;
        self.refresh_provider(provider).await
    }

    /// Accepts a manager's retry before making its forced credential observation.
    pub async fn refresh_provider_for_retry_in(
        &self,
        tx: Tx,
        principal: &Principal,
        origin: &Origin,
        provider: Id,
    ) -> Result<Refreshed, EngineError> {
        provider_decision(
            tx,
            principal,
            origin,
            provider,
            "retry the credential read on",
        )
        .await?;
        self.refresh_provider_for_retry(provider).await
    }

    /// Accepts a harness restart in the caller's guarded transaction before stopping any process.
    pub async fn restart_harness_in(
        &self,
        tx: Tx,
        principal: &Principal,
        origin: &Origin,
        provider: Id,
        agent: Id,
    ) -> Result<Restarted, EngineError> {
        provider_decision(tx, principal, origin, provider, "restart its harness on").await?;
        self.restart_harness(provider, agent).await
    }

    /// Accepts sign-out in the caller's guarded transaction before touching the harness or its login.
    pub async fn sign_out_in(
        &self,
        tx: Tx,
        principal: &Principal,
        origin: &Origin,
        provider: Id,
    ) -> Result<Asked, EngineError> {
        provider_decision(tx, principal, origin, provider, "sign out").await?;
        self.sign_out(provider).await
    }

    /// Accepts a stop in the caller's guarded transaction before interrupting turns or returning credentials.
    pub async fn stop_environment_in(
        &self,
        tx: Tx,
        principal: &Principal,
        origin: &Origin,
        environment: Id,
        interrupt: bool,
        reason: StopReason,
    ) -> Result<(), StopError> {
        let accepted = self
            .accept_environment_stop_in(
                tx,
                principal,
                origin,
                crate::environments::StopRequest {
                    environment,
                    interrupt,
                    reason,
                },
                || Ok(()),
                || Ok(()),
            )
            .await?;
        self.finish_accepted_environment_stop(accepted).await
    }

    /// Commit the native decision only after its actual preparation and last rollbackable wait.
    pub async fn accept_environment_stop_in<F, G>(
        &self,
        mut tx: Tx,
        principal: &Principal,
        origin: &Origin,
        request: crate::environments::StopRequest,
        before: F,
        before_commit: G,
    ) -> Result<crate::environments::AcceptedStop, StopError>
    where
        F: FnOnce() -> Result<(), EngineError>,
        G: FnOnce() -> Result<(), EngineError>,
    {
        let environment = request.environment;
        let interrupt = request.interrupt;
        let state = environments::environment(&mut tx, environment)
            .await?
            .filter(|state| state.deleted_at.is_none())
            .ok_or(StopError::NotFound)?;
        if !orch_db::queries::environment_facts::has_unit(&mut tx, environment).await? {
            return Err(StopError::NotManaged);
        }
        if let Some(job) =
            orch_db::queries::jobs::changes::environment_hold(&mut tx, environment).await?
        {
            return Err(StopError::Unavailable(format!("held by job {}", job.id)));
        }
        let running = environments::active_turns(&mut tx, environment).await?;
        if running > 0 && !interrupt {
            return Err(StopError::Busy(running));
        }
        let summary = format!("A caller asked to stop the environment {}", state.name);
        let mut entry = Entry::new(
            Actor::User(principal.user()),
            orch_db::audit::action!(AuditAction::EnvironmentStopped).as_str(),
            "environment",
            Some(environment),
            &summary,
        );
        entry.history = true;
        entry.node_id = Some(state.home_node_id);
        entry.org_id = Some(state.org_id);
        entry.environment_id = Some(environment);
        entry.details =
            json!({"environment": environment, "interrupt": interrupt, "decision": "accepted"});
        let runtime = environments::live_runtime(&mut tx, environment).await?;
        before()?;
        audit::append(&mut tx, &entry, origin, principal.now().get()).await?;
        before_commit()?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(crate::environments::AcceptedStop {
            request,
            owner: state.owner_id,
            home: state.home_node_id,
            machine: state.machine_id,
            generation: state.generation,
            runtime: runtime.map(|runtime| runtime.id),
        })
    }
}
