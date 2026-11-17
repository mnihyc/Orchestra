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
    /// A host's Stop names its selected unit, including a kept one, rather than another live unit.
    pub async fn stop_host_environment_unit_in(
        &self,
        mut tx: Tx,
        principal: &Principal,
        origin: &Origin,
        host: Id,
        runtime: Id,
        interrupt: bool,
    ) -> Result<(), StopError> {
        let selected = environments::runtime(&mut tx, runtime)
            .await?
            .filter(|row| row.host_id == host && row.kind == "environment")
            .ok_or(StopError::NotFound)?;
        let environment = selected.environment_id.ok_or(StopError::NotFound)?;
        let registered = environments::lock_environment(&mut tx, environment)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or(StopError::NotFound)?;
        let selected = environments::runtime(&mut tx, runtime)
            .await?
            .filter(|row| row.host_id == host && row.environment_id == Some(environment))
            .ok_or(StopError::NotFound)?;
        if environments::live_runtime(&mut tx, environment)
            .await?
            .is_some_and(|row| row.id == runtime)
        {
            return self
                .stop_environment_in(
                    tx,
                    principal,
                    origin,
                    environment,
                    interrupt,
                    StopReason::Person(principal.user()),
                )
                .await;
        }
        let source = orch_db::queries::runtime_incarnations::current(&mut tx, runtime)
            .await?
            .filter(|source| source.incarnation.is_some())
            .ok_or_else(|| StopError::Unavailable(
                "the host cannot confirm the selected unit's original native start; its files were kept".into(),
            ))?;
        let host_row = orch_db::queries::resources::hosting::host(&mut tx, host)
            .await?
            .ok_or(StopError::NotFound)?;
        let node = host_row.machine_id;
        let control =
            orch_db::queries::runtime_controls::accept(&mut tx, runtime, orch_types::new_id())
                .await?;
        if control.source() != source {
            return Err(StopError::Unavailable(
                "the selected native start changed".into(),
            ));
        }
        let command = orch_types::new_id();
        let revision = orch_db::queries::runtime_controls::phase(
            &mut tx,
            &control,
            command,
            &json!({"kind":"node.runtime.stop","runtime_id":runtime,"source":source,
                "body":{"runtime_id":runtime}}),
        )
        .await?
        .ok_or_else(|| StopError::Unavailable("a newer control owns the selected unit".into()))?;
        let mut entry = Entry::new(
            Actor::User(principal.user()),
            AuditAction::HostUpdated.as_str(),
            "host",
            Some(host),
            "A host manager asked to stop the selected kept unit",
        );
        entry.history = true;
        entry.node_id = host_row.home_id;
        entry.org_id = host_row.org_id;
        entry.details = json!({"host":host,"runtime":runtime,"environment":environment,
            "interrupt":interrupt,"decision":"accepted"});
        audit::append(&mut tx, &entry, origin, principal.now().get()).await?;
        tx.commit().await.map_err(DbError::from)?;
        let outgoing = crate::commands::Outgoing::new(
            command,
            node,
            orch_link_contract::commands::SerialKey::Runtime(runtime),
            &orch_link_contract::commands::NodeRuntimeStop {
                runtime_id: runtime,
                reason: None,
            },
        )
        .map_err(EngineError::from)?
        .with_native_target(orch_link_contract::native_target::NativeTarget::Lifecycle(
            orch_link_contract::native_target::LifecycleTarget {
                subject: orch_link_contract::native_target::LifecycleSubject::Runtime {
                    runtime_id: runtime,
                },
                observed: source.incarnation,
                opening: None,
                revision,
            },
        ));
        let outcome = tokio::time::timeout(
            self.inner.config.hosting.stop_wait,
            crate::commands::run(self.inner.commands.as_ref(), outgoing),
        )
        .await;
        let mut tx = self.inner.db.begin().await?;
        let current = environments::lock_environment(&mut tx, environment)
            .await?
            .ok_or(StopError::NotFound)?;
        if orch_db::queries::runtime_incarnations::current(&mut tx, runtime).await? != Some(source)
            || !orch_db::queries::runtime_controls::is_current(&mut tx, &control).await?
        {
            return Err(StopError::Unavailable("the original selected unit or registration changed; its old reply was not published".into()));
        }
        match outcome {
            Ok(Ok(crate::commands::Outcome::Succeeded(_))) => {
                if !orch_db::queries::runtime_incarnations::observe(
                    &mut tx, host, runtime, source, true,
                )
                .await?
                {
                    return Err(StopError::Unavailable(
                        "the original selected start could not be confirmed".into(),
                    ));
                }
                if environments::live_runtime(&mut tx, environment)
                    .await?
                    .is_none()
                    && current.owner_id == registered.owner_id
                    && current.home_node_id == registered.home_node_id
                    && current.generation == registered.generation
                    && selected.successor_of.is_none()
                    && selected.generation == current.generation
                {
                    environments::set_status(
                        &mut tx,
                        environment,
                        &["running", "offline"],
                        "stopped",
                        None,
                        self.inner.clock.now(),
                    )
                    .await?;
                }
                tx.commit().await.map_err(DbError::from)?;
                Ok(())
            }
            Ok(Ok(crate::commands::Outcome::Failed(error))) => {
                Err(StopError::Host(format!("{}: {}", error.code, error.detail)))
            }
            _ => Err(StopError::Host(
                "the host did not confirm the selected unit stopped; its files were kept".into(),
            )),
        }
    }

    /// Accepts the exact provider unit a host manager selected, including an older kept unit.
    pub async fn stop_host_provider_unit_in(
        &self,
        mut tx: Tx,
        principal: &Principal,
        origin: &Origin,
        host: Id,
        runtime: Id,
        interrupt: bool,
    ) -> Result<(), RuntimeError> {
        let row = environments::runtime(&mut tx, runtime)
            .await?
            .filter(|row| row.host_id == host && row.kind == "provider")
            .ok_or(RuntimeError::NotFound)?;
        let provider = row.provider_id.ok_or(RuntimeError::NotFound)?;
        if orch_db::queries::jobs::changes::provider_hold(&mut tx, provider)
            .await?
            .is_some()
        {
            return Err(RuntimeError::Unavailable(
                "a job works on its provider runtime".to_owned(),
            ));
        }
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
        entry.details = json!({"host":host,"runtime":runtime,"provider":provider,"interrupt":interrupt,"decision":"accepted"});
        audit::append(&mut tx, &entry, origin, principal.now().get()).await?;
        tx.commit().await.map_err(DbError::from)?;
        self.stop_provider_unit(
            provider,
            runtime,
            interrupt,
            StopWhy::Person(principal.user()),
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
        provider_decision(tx, principal, origin, provider, "stop").await?;
        self.stop_provider_runtime(provider, interrupt, StopWhy::Person(principal.user()))
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
        let state = environments::lock_environment(&mut tx, environment)
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
        let runtime_source = if let Some(runtime) = &runtime {
            orch_db::queries::runtime_incarnations::current(&mut tx, runtime.id).await?
        } else {
            None
        };
        let control = if let Some(runtime) = &runtime {
            Some(
                orch_db::queries::runtime_controls::accept(
                    &mut tx,
                    runtime.id,
                    orch_types::new_id(),
                )
                .await?,
            )
        } else {
            None
        };
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
            runtime_source,
            control,
        })
    }
}
