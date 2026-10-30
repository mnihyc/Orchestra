//! A provider's setup as its adapter declares it: the template and the host its runtime is made from, a sign-in with sealed values that wait for the runtime, a sign-in copied in again by the same method, its state, and a token's end.
//!
//! **What is built here: a provider that is a person's machine, and what a harness says it is.**
//!
//! * **Learning what a harness is.** A profile's descriptor is what a harness of it last declared, and
//!   nothing tells it before a harness ran: a machine reports which adapters it can drive, not what each
//!   declares. So the first time a profile is used with a provider, the harness is started once where the
//!   provider runs (else in the environment's machine) and the answer to `agent.start` is stored with the
//!   profile ([`Engine::learn_descriptor`]). It is asked for only when something needs the descriptor and
//!   the person asking may use what they asked for, so that nobody starts a harness by asking for what is
//!   not theirs. The harness stays up: the thread's first turn finds it so and starts nothing.
//! * **Looking at a provider's credential.** `provider.refresh` goes to the machine that keeps the
//!   provider, after the harness of a subscription or the provider role of a key is up; what it answers
//!   (the state, and the plan and the limits the harness reports) is stored as the provider's facts and
//!   moves its state the way the machine's own reports do ([`Engine::refresh_provider`]). A subscription
//!   that is registered from a machine is found signed in this way, before it is kept.
//!
//! * **Signing a native provider in.** A sign-in is begun at its credential keeper by an adapter method
//!   and given to the harness of the provider's own profile in the provider's runtime, never to an
//!   environment. What a person gave for a secret field is in a row of its own, sealed, from the moment the
//!   request has it; the command that gives it to the harness refers to the row and the link seals it to the
//!   machine when the frame is written, and the row is deleted as soon as the command is over. A sign-in
//!   whose runtime is not up waits for it ([`Engine::deliver_sign_in`]): the unit is started, and what waits
//!   is kept apart from every event, log and job. It is forgotten when the runtime has taken it, when the
//!   making of the runtime fails or is cancelled, and in any case after `[engine] login_pending_secs`. What
//!   the harness shows and asks for comes back as the events of the provider's stream and is kept as it
//!   comes (`Engine::on_login_event`); a person's answer goes the same way as the first values did.
//! * **A sign-in that is kept fresh, and a token that ends.** A runtime that has been stopped for
//!   `runtime.renew_days` is started by the job `provider_provision` in the mode `renew`, and its harness
//!   looks at what it holds ([`Engine::renew_sign_in`]); a sign-in that no longer works is told to the
//!   provider's managers (`provider.attention`) before a thread needs it. A token's end is read from the
//!   facts, and told `providers.expiry_notice_days` ahead and when it has passed (`Engine::watch_token_ends`).

use crate::commands::{Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::harnesses::{self, HarnessKey, Started};
use crate::ids;
use crate::leases;
use crate::provider_runtime::RuntimeStart;
use crate::unit::Unit;
use chrono::{DateTime, Utc};
use orch_db::queries::environments::thread_engine as environments;
use orch_db::queries::jobs::JobRow;
use orch_db::queries::resources::binding;
use orch_db::queries::resources::keeper;
use orch_db::queries::resources::provider_runtime::{self as runtime_rows, State};
use orch_db::queries::resources::thread_engine::{self as resources, AgentRow, MachinePlace};
use orch_link_contract::commands::{
    ProviderDiscoverModels, ProviderDiscoverModelsResult, ProviderLoginCancel, ProviderLoginInput,
    ProviderLoginStart, ProviderLogout, ProviderRefresh, ProviderRefreshResult, Secret,
    SecretOrText, SerialKey,
};
use orch_settings::{Place as SettingsPlace, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time, new_id};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The role a machine plays when it hosts harnesses.
const AGENT_ROLE: &str = "agent";
/// What a provider whose sign-in is gone says of its state: the machine looked, and nothing is signed
/// in there. A provider is not made ready by having a model while this stands.
pub const SIGNED_OUT: &str = "nothing is signed in there";

/// Why a harness could not be made to say what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unlearned {
    /// There is no such profile.
    NoSuchProfile,
    /// There is no such provider.
    NoSuchProvider,
    /// Neither the provider nor the environment is on a machine.
    NoMachine,
    /// The machine is away, or was revoked.
    Offline,
    /// The machine does not play the agent role, or did not report an adapter of this harness.
    NoAdapter,
    /// The machine answered that it could not, with this.
    Failed(String),
    /// The machine did not answer in time.
    TimedOut,
}

/// How learning what a harness is came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Learning {
    /// The profile says what its harness is.
    Known,
    /// It does not, for this reason.
    Unknown(Unlearned),
}

/// What looking at a provider's credential came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Refreshed {
    /// Stored availability and the fresh runtime report, when this read supplied one.
    Answered {
        /// Where the provider stands.
        status: String,
        /// The correlated runtime state; absent for a coalesced, untold or discarded observation.
        reported_status: Option<String>,
        /// What the harness reported of its plan, as it reported it.
        facts: Json,
    },
    /// There is no such provider.
    Gone,
    /// It has no machine to run on.
    NoMachine,
    /// Its machine is away, or was revoked.
    Offline,
    /// Its machine could not do it, or did not answer in time.
    Failed(String),
}

/// Whether a machine reported that it can drive the adapter of `kind`. A machine that reported no list
/// at all has not said otherwise, and is asked.
fn drives(place: &MachinePlace, kind: &str) -> bool {
    place
        .facts
        .get("adapters")
        .and_then(Json::as_array)
        .is_none_or(|adapters| {
            adapters
                .iter()
                .any(|each| each.get("kind").and_then(Json::as_str) == Some(kind))
        })
}

/// What an outcome that is no success says, in a sentence.
pub(crate) fn failure_text(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Succeeded(_) => "it succeeded".to_owned(),
        Outcome::Failed(error) => format!("{}: {}", error.code, error.detail),
        Outcome::Expired => "the machine did not take it up in time".to_owned(),
        Outcome::Cancelled => "it was withdrawn".to_owned(),
    }
}

impl Engine {
    /// Has the harness of the profile `agent` say what it is, when it never did: the harness is started
    /// once, on the machine of the provider `provider` or else on the environment's, and what it answers
    /// is stored with the profile. A profile that declared already is [`Learning::Known`] and nothing is
    /// sent.
    pub async fn learn_descriptor(
        &self,
        agent: Id,
        provider: Id,
        environment: Option<Id>,
    ) -> Result<Learning, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let Some(profile) = resources::agent(&mut conn, agent)
            .await?
            .filter(|row| row.deleted_at.is_none())
        else {
            return Ok(Learning::Unknown(Unlearned::NoSuchProfile));
        };
        if harnesses::descriptor_of(&profile)?.is_some() {
            return Ok(Learning::Known);
        }
        let Some(keeper) = resources::provider(&mut conn, provider)
            .await?
            .filter(|row| row.deleted_at.is_none())
        else {
            return Ok(Learning::Unknown(Unlearned::NoSuchProvider));
        };
        let mut places: Vec<Id> = leases::machine_of(&keeper).into_iter().collect();
        if let Some(environment) = environment
            && let Some(row) = environments::environment(&mut conn, environment).await?
            && let Some(machine) = row.machine_id
            && !places.contains(&machine)
        {
            places.push(machine);
        }
        drop(conn);
        let mut why = Unlearned::NoMachine;
        for (index, machine) in places.into_iter().enumerate() {
            // Learning is asked for on the way to something else: a machine that cannot be reached is
            // told as the reason, and the caller's own request says what it comes to.
            let learned = match self.start_to_learn(&profile, provider, machine).await {
                Ok(learned) => learned,
                Err(error) => Learning::Unknown(Unlearned::Failed(error.to_string())),
            };
            match learned {
                Learning::Known => return Ok(Learning::Known),
                // The first place is the one that should have answered; the others are only tried.
                Learning::Unknown(reason) if index == 0 => why = reason,
                Learning::Unknown(_) => {}
            }
        }
        Ok(Learning::Unknown(why))
    }

    /// Starts the harness of `profile` for `provider` on `machine`, if that machine can, and keeps what it
    /// declares.
    async fn start_to_learn(
        &self,
        profile: &AgentRow,
        provider: Id,
        machine: Id,
    ) -> Result<Learning, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let Some(place) = resources::machine_place(&mut conn, machine).await? else {
            return Ok(Learning::Unknown(Unlearned::NoMachine));
        };
        drop(conn);
        if place.deleted_at.is_some() || place.status != "active" || !place.online {
            return Ok(Learning::Unknown(Unlearned::Offline));
        }
        if !place.roles.iter().any(|role| role == AGENT_ROLE)
            || !drives(&place, &profile.harness_kind)
        {
            return Ok(Learning::Unknown(Unlearned::NoAdapter));
        }
        let key = HarnessKey {
            provider,
            agent: profile.id,
            machine,
        };
        // A process the system believes is up, whose profile says nothing of it, is asked once more:
        // the machine answers a harness that runs with the descriptor it declared.
        if self.harness_is_online(key).await? {
            let mut tx = self.inner.db.begin().await?;
            resources::set_harness_status(
                &mut tx,
                provider,
                profile.id,
                machine,
                &resources::HarnessReport {
                    status: harnesses::OFFLINE,
                    detail: None,
                    version: None,
                    sessions_open: 0,
                },
                self.inner.clock.now(),
            )
            .await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        // A new identifier each time: the machine starts a harness that runs already by answering with
        // it, and a start that failed is asked for again by the next try and not answered from the log.
        let command = new_id();
        let started = tokio::time::timeout(
            self.inner.config.start_timeout,
            self.ensure_harness(key, profile, command),
        )
        .await;
        match started {
            Err(_elapsed) => {
                self.inner.commands.cancel(command).await?;
                Ok(Learning::Unknown(Unlearned::TimedOut))
            }
            Ok(Err(error)) => Err(error),
            Ok(Ok(Started::Up)) => Ok(Learning::Known),
            Ok(Ok(Started::Failed(outcome))) => {
                Ok(Learning::Unknown(Unlearned::Failed(failure_text(&outcome))))
            }
        }
    }

    /// Has the machine a provider sits on run it, when the system holds its key: `provider.start` is what
    /// brings the key, sealed to the machine, and until it ran there the machine holds nothing that a
    /// question about the provider names. Answers why not when the provider cannot be run there.
    pub async fn ensure_provider_up(
        &self,
        provider: Id,
    ) -> Result<Result<(), String>, EngineError> {
        Ok(
            match self.ensure_provider_started(Id::nil(), provider).await? {
                crate::providers::Keeping::Up => Ok(()),
                crate::providers::Keeping::Failed(outcome) => Err(format!(
                    "the provider did not start: {}",
                    failure_text(&outcome)
                )),
                crate::providers::Keeping::Cannot(error) => Err(error.to_string()),
            },
        )
    }

    /// Has the machine that keeps a provider look at its credential and report: after the harness of a
    /// subscription, or the provider role of a key, is up. What comes back is kept as the provider's
    /// facts, and a provider the machine finds failing, exhausted or signed out moves to that state; one it
    /// finds well again moves back out of it.
    pub async fn refresh_provider(&self, provider: Id) -> Result<Refreshed, EngineError> {
        self.read_provider_facts(provider, false).await
    }

    /// A credential was changed or registered: it must be observed even just after a previous read.
    pub(crate) async fn refresh_provider_now(
        &self,
        provider: Id,
    ) -> Result<Refreshed, EngineError> {
        self.read_provider_facts(provider, true).await
    }

    async fn read_provider_facts(
        &self,
        provider: Id,
        force: bool,
    ) -> Result<Refreshed, EngineError> {
        self.read_provider_facts_with_renewal(provider, force, false)
            .await
    }

    fn read_provider_facts_with_renewal(
        &self,
        provider: Id,
        force: bool,
        renew: bool,
    ) -> impl std::future::Future<Output = Result<Refreshed, EngineError>> + Send + '_ {
        // Requests, completed sign-ins and renewal share one observation lifetime.
        // Construct its state before polling so supervisor callers do not embed it.
        // Allocating here also breaks the startup/renewal future cycle.
        Box::pin(self.read_provider_facts_inner(provider, force, renew))
    }

    async fn read_provider_facts_inner(
        &self,
        provider: Id,
        force: bool,
        renew: bool,
    ) -> Result<Refreshed, EngineError> {
        // One observation of a provider at a time, held by a lease row: no transaction stays open while the
        // machine answers. An ordinary reader that finds one in flight answers with the facts there
        // are; a forced one waits for it to end (at most two of its machine waits) and observes again.
        let name = format!("provider.facts:{provider}");
        let holder = new_id().to_string();
        let length = chrono::Duration::from_std(self.inner.config.start_timeout.saturating_mul(3))
            .unwrap_or_else(|_| chrono::Duration::minutes(5));
        let patience =
            tokio::time::Instant::now() + self.inner.config.start_timeout.saturating_mul(2);
        let admitted = loop {
            let now = self.inner.clock.now();
            let taken = {
                let mut conn = self.inner.db.acquire().await?;
                orch_db::queries::job_leases::take(&mut conn, &name, &holder, now, now + length)
                    .await?
            };
            if taken || !force || tokio::time::Instant::now() >= patience {
                break taken;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        };
        let observed = self
            .observe_provider_facts(provider, force, renew, admitted)
            .await;
        if admitted {
            let mut conn = self.inner.db.acquire().await?;
            orch_db::queries::job_leases::release(&mut conn, &name, &holder).await?;
        }
        observed
    }

    async fn observe_provider_facts(
        &self,
        provider: Id,
        force: bool,
        renew: bool,
        admitted: bool,
    ) -> Result<Refreshed, EngineError> {
        let mut observing =
            orch_db::queries::transaction_guard::scope(false, self.inner.db.begin()).await?;
        let Some(row) = resources::provider(&mut observing, provider)
            .await?
            .filter(|row| row.deleted_at.is_none())
        else {
            return Ok(Refreshed::Gone);
        };
        if !admitted || row.status == "disabled" {
            return Ok(Refreshed::Answered {
                status: row.status,
                reported_status: None,
                facts: row.facts,
            });
        }
        if renew
            && !orch_settings::provider_sign_in(&mut observing, provider)
                .await?
                .allowed
        {
            return Ok(Refreshed::Failed("provider.signin: the credential keeper has not permitted renewal in the selected existing harness home".into()));
        }
        let Some(machine) = leases::machine_of(&row) else {
            let runtime = runtime_rows::live_runtime(&mut observing, provider).await?;
            let stopped =
                if let Some(runtime) = runtime.filter(|runtime| runtime.state == "stopped") {
                    orch_db::queries::resources::hosting::host(&mut observing, runtime.host_id)
                        .await?
                        .is_some_and(|host| host.online && host.machine_status == "active")
                } else {
                    false
                };
            if stopped {
                let claimed_at = self.inner.clock.now();
                let mut attempt =
                    orch_db::queries::transaction_guard::scope(false, self.inner.db.begin())
                        .await?;
                let due = orch_db::queries::provider_facts::claim_read(
                    &mut attempt,
                    provider,
                    force,
                    claimed_at,
                    self.inner.config.provider_facts.page,
                )
                .await?;
                attempt.commit().await.map_err(orch_db::DbError::from)?;
                observing.commit().await.map_err(orch_db::DbError::from)?;
                if due {
                    let started = orch_db::queries::transaction_guard::scope(
                        true,
                        self.start_provider_runtime(provider, None),
                    )
                    .await;
                    orch_db::queries::provider_facts::discard_unread_claim(
                        &mut *self.inner.db.acquire().await?,
                        provider,
                        claimed_at,
                    )
                    .await?;
                    started.map_err(EngineError::unreadable)?;
                }
            }
            return Ok(Refreshed::NoMachine);
        };
        let place = resources::machine_place(&mut observing, machine)
            .await?
            .filter(|place| place.deleted_at.is_none() && place.status == "active");
        let Some(credential_epoch) = orch_db::queries::provider_facts::credential_epoch(
            &mut observing,
            provider,
            Some(row.revision),
        )
        .await?
        else {
            return Ok(Refreshed::Answered {
                status: row.status,
                reported_status: None,
                facts: row.facts,
            });
        };
        let profile = match row.agent_id {
            Some(agent) => resources::agent(&mut observing, agent).await?,
            None => None,
        };
        let facts_source = crate::provider_sources::spec(
            &mut observing,
            provider,
            &row.sources,
            orch_provider_contract::SourceElement::Facts,
        )
        .await?;
        let Some(_) = place.filter(|place| place.online) else {
            return Ok(Refreshed::Offline);
        };
        // Commit the stored observation before asking the machine; no transaction stays open across that wait.
        observing.commit().await.map_err(orch_db::DbError::from)?;
        // Claim the attempt in its own short transaction. Refused reads keep facts_at, yet remain
        // throttled. Release the row and authority locks before the runtime opens its transactions.
        let mut reading =
            orch_db::queries::transaction_guard::scope(false, self.inner.db.begin()).await?;
        let acquired = orch_db::queries::provider_facts::claim_read(
            &mut reading,
            provider,
            force,
            self.inner.clock.now(),
            self.inner.config.provider_facts.page,
        )
        .await?;
        reading.commit().await.map_err(orch_db::DbError::from)?;
        if !acquired {
            return Ok(Refreshed::Answered {
                status: row.status,
                reported_status: None,
                facts: row.facts,
            });
        }
        if row.kind == "subscription" {
            let Some(profile) = profile else {
                return Ok(Refreshed::Failed(
                    "the profile whose harness holds the sign-in is not there".to_owned(),
                ));
            };
            let key = HarnessKey {
                provider,
                agent: profile.id,
                machine,
            };
            let command = new_id();
            let started = tokio::time::timeout(
                self.inner.config.start_timeout,
                self.ensure_harness(key, &profile, command),
            )
            .await;
            match started {
                Err(_elapsed) => {
                    self.inner.commands.cancel(command).await?;
                    return Ok(Refreshed::Failed(
                        "the harness did not start in time".to_owned(),
                    ));
                }
                Ok(Err(error)) => return Err(error),
                Ok(Ok(Started::Up)) => {}
                Ok(Ok(Started::Failed(outcome))) => {
                    return Ok(Refreshed::Failed(format!(
                        "the harness did not start: {}",
                        failure_text(&outcome)
                    )));
                }
            }
        } else {
            let started = tokio::time::timeout(
                self.inner.config.start_timeout,
                self.ensure_provider_started(Id::nil(), provider),
            )
            .await;
            let started = match started {
                Ok(result) => result?,
                Err(_) => {
                    return Ok(Refreshed::Failed(
                        "the provider did not start in time".to_owned(),
                    ));
                }
            };
            match started {
                crate::providers::Keeping::Up => {}
                crate::providers::Keeping::Failed(outcome) => {
                    return Ok(Refreshed::Failed(format!(
                        "the provider did not start: {}",
                        failure_text(&outcome)
                    )));
                }
                crate::providers::Keeping::Cannot(error) => {
                    return Ok(Refreshed::Failed(error.to_string()));
                }
            }
        }
        let id = new_id();
        let command = Outgoing::new(
            id,
            machine,
            SerialKey::Provider(provider),
            &ProviderRefresh {
                provider_id: provider,
                retry: force.then_some(true),
                renew: renew.then_some(true),
                facts: facts_source,
            },
        )?;
        let asked = tokio::time::timeout(
            self.inner.config.start_timeout,
            crate::commands::run(self.inner.commands.as_ref(), command),
        )
        .await;
        let outcome = match asked {
            Err(_elapsed) => {
                self.inner.commands.cancel(id).await?;
                crate::provider_sources::failed(
                    self,
                    provider,
                    &credential_epoch,
                    &row.sources,
                    orch_provider_contract::SourceElement::Facts,
                    orch_provider_contract::SourceErrorCode::Timeout,
                    "The keeping machine did not answer the facts source read in time.",
                )
                .await?;
                return Ok(Refreshed::Failed(
                    "the machine did not answer in time".to_owned(),
                ));
            }
            Ok(answered) => answered?,
        };
        let Outcome::Succeeded(result) = outcome else {
            crate::provider_sources::failed(
                self,
                provider,
                &credential_epoch,
                &row.sources,
                orch_provider_contract::SourceElement::Facts,
                orch_provider_contract::SourceErrorCode::Unavailable,
                "The keeping machine could not finish the facts source read.",
            )
            .await?;
            return Ok(Refreshed::Failed(failure_text(&outcome)));
        };
        let result: ProviderRefreshResult = serde_json::from_value(result)?;
        let applied = self
            .keep_refreshed(provider, &result, &credential_epoch)
            .await?;
        let mut conn = self.inner.db.acquire().await?;
        Ok(match resources::provider(&mut conn, provider).await? {
            Some(current) => Refreshed::Answered {
                status: current.status,
                reported_status: (applied
                    && result.status != "not_told"
                    && result.source.as_ref().is_none_or(|source| {
                        source.error.is_none() || source.supported == Some(true)
                    }))
                .then_some(result.status),
                facts: current.facts,
            },
            None => Refreshed::Gone,
        })
    }

    /// Stores an observation and releases threads after a proven recovery.
    async fn keep_refreshed(
        &self,
        provider: Id,
        result: &ProviderRefreshResult,
        epoch: &Json,
    ) -> Result<bool, EngineError> {
        let mut tx =
            orch_db::queries::transaction_guard::scope(true, self.inner.db.begin()).await?;
        if !orch_db::queries::provider_facts::lock_provider(&mut tx, provider).await?
            || orch_db::queries::provider_facts::credential_epoch(&mut tx, provider, None)
                .await?
                .as_ref()
                != Some(epoch)
        {
            return Ok(false);
        }
        let changed = self
            .keep_provider_observation(&mut tx, provider, result, self.inner.clock.now())
            .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        if changed {
            self.reevaluate_provider(provider).await?;
        }
        Ok(true)
    }

    /// Keeps one whole runtime observation in the caller's authority transaction.
    /// Only the vendor's authenticated answer, never an earlier cached proof or local files, lifts a refusal.
    pub async fn keep_provider_observation(
        &self,
        conn: &mut orch_db::Conn,
        provider: Id,
        result: &ProviderRefreshResult,
        now: DateTime<Utc>,
    ) -> Result<bool, EngineError> {
        let mut previous = resources::provider(conn, provider).await?;
        if let (Some(row), Some(source)) = (previous.as_mut(), result.source.as_ref()) {
            crate::provider_sources::observed(
                conn,
                provider,
                &row.sources,
                orch_provider_contract::SourceElement::Facts,
                source.descriptor.as_ref(),
                source.error.as_ref(),
                Time::new(now),
            )
            .await?;
            if let Some(descriptor) = &source.descriptor {
                let mut choices: orch_provider_contract::ProviderSources =
                    serde_json::from_value(row.sources.clone())?;
                let choice = choices.facts.get_or_insert_with(Default::default);
                choice.descriptor = Some(descriptor.clone());
                row.sources = serde_json::to_value(choices)?;
            }
        }
        if result.status == "not_told" {
            if let (Some(row), Some(facts)) = (previous.as_ref(), result.facts.as_ref()) {
                let incoming = crate::provider_fact_reads::keeper_observation(
                    crate::provider_fact_reads::normalize_observation(&serde_json::to_value(
                        facts,
                    )?)?,
                );
                let known = crate::provider_fact_reads::normalize_observation(&row.facts)?;
                let merged = crate::provider_fact_reads::merge_observation(&known, incoming);
                if merged != known {
                    keeper::set_facts(conn, provider, &merged, now).await?;
                }
            }
            return Ok(false);
        }
        let may_verify = previous.as_ref().is_some_and(|row| {
            crate::provider_fact_reads::source_can_verify(&row.sources, row.agent_id.is_some())
        });
        // Admission facts may hold otherwise ready access, regardless of credential kind or role.
        // A missing sign-in keeps the keeper's reported state.
        // Balances and spend are information; neither closes a provider.
        let observed_status = if result.status == "ready"
            && result.facts.as_ref().is_some_and(|facts| {
                crate::provider_fact_reads::admission_exhausted(facts, Time::new(now))
            }) {
            "exhausted"
        } else {
            result.status.as_str()
        };
        if result.facts.is_some() {
            let known = previous
                .as_ref()
                .map(|row| &row.facts)
                .cloned()
                .unwrap_or_default();
            let known = crate::provider_fact_reads::normalize_observation(&known)?;
            let mut incoming = crate::provider_fact_reads::normalize_observation(
                &serde_json::to_value(&result.facts)?,
            )?;
            if !may_verify && let Some(facts) = incoming.as_object_mut() {
                facts.remove("credential_verified");
                if let Some(times) = facts.get_mut("field_times").and_then(Json::as_object_mut) {
                    times.remove("credential_verified");
                }
            }
            let merged = crate::provider_fact_reads::merge_observation(&known, incoming);
            keeper::set_facts(conn, provider, &merged, now).await?;
        }
        // Valid members of a partial report act normally; its safe note stays beside the source.
        if result
            .source
            .as_ref()
            .is_some_and(|source| source.error.is_some() && source.supported != Some(true))
        {
            return Ok(false);
        }
        let verified = result
            .facts
            .as_ref()
            .and_then(|facts| facts.credential_verified)
            == Some(true)
            && may_verify;
        if previous
            .as_ref()
            .is_some_and(|row| row.status == "exhausted")
            && observed_status == "ready"
            && result
                .facts
                .as_ref()
                .and_then(|facts| facts.ordinary_usage_allowed)
                != Some(true)
            && previous
                .as_ref()
                .and_then(|row| row.facts.pointer("/limit_reached/resets_at"))
                .and_then(Json::as_str)
                .and_then(|at| at.parse::<Time>().ok())
                .is_none_or(|at| at.get() > now)
        {
            return Ok(false);
        }
        let Some((mut to, from)) =
            crate::providers::refreshed_transition(observed_status, verified)
        else {
            return Ok(false);
        };
        let models = binding::enabled_models(conn, provider).await?;
        if to == "ready"
            && models == 0
            && previous.as_ref().is_some_and(|row| row.status == "error")
        {
            to = "needs_setup";
        }
        let detail = (to == "needs_setup").then_some(if result.status == "needs_setup" {
            SIGNED_OUT
        } else if models == 0 {
            "it needs an enabled model"
        } else {
            SIGNED_OUT
        });
        let mut changed = keeper::move_status(conn, provider, from, to, detail, now).await?;
        if to == "ready" && models > 0 {
            changed |=
                keeper::move_status(conn, provider, &["needs_setup"], "ready", None, now).await?;
        }
        if changed {
            orch_db::queries::access::bump_epoch(conn).await?;
            if to == "exhausted" {
                crate::notifications::provider_exhausted(conn, provider, None, now).await?;
            }
        }
        Ok(changed)
    }

    /// A manager's explicit retry observes an error immediately, bypassing the ordinary page interval.
    pub async fn refresh_provider_for_retry(&self, provider: Id) -> Result<Refreshed, EngineError> {
        self.read_provider_facts(provider, true).await
    }

    /// Reads the current model catalogue without changing the manager's accepted models or switches.
    /// A failed read keeps the previous list and its date; a credential change discards an obsolete answer.
    pub async fn discover_provider_models(
        &self,
        provider: Id,
    ) -> Result<Option<Vec<orch_agent_contract::discovery::ModelSuggestion>>, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let Some(row) = resources::provider(&mut conn, provider)
            .await?
            .filter(|row| row.deleted_at.is_none() && row.status != "disabled")
        else {
            return Ok(None);
        };
        let Some(machine) = leases::machine_of(&row) else {
            let stopped = runtime_rows::live_runtime(&mut conn, provider)
                .await?
                .is_some_and(|runtime| runtime.state == "stopped");
            drop(conn);
            if stopped {
                orch_db::queries::transaction_guard::scope(
                    true,
                    self.start_provider_runtime(provider, None),
                )
                .await
                .map_err(EngineError::unreadable)?;
            }
            return Ok(None);
        };
        if !resources::machine_place(&mut conn, machine)
            .await?
            .is_some_and(|place| {
                place.online && place.status == "active" && place.deleted_at.is_none()
            })
        {
            return Ok(None);
        }
        let epoch = orch_db::queries::provider_facts::credential_epoch(
            &mut conn,
            provider,
            Some(row.revision),
        )
        .await?;
        let models_source = crate::provider_sources::spec(
            &mut conn,
            provider,
            &row.sources,
            orch_provider_contract::SourceElement::Models,
        )
        .await?;
        drop(conn);
        let Some(epoch) = epoch else {
            return Ok(None);
        };
        if row.kind == "api" {
            match tokio::time::timeout(
                self.inner.config.start_timeout,
                self.ensure_provider_up(provider),
            )
            .await
            {
                Ok(ready) => {
                    if ready?.is_err() {
                        return Ok(None);
                    }
                }
                Err(_) => {
                    crate::provider_sources::failed(
                        self,
                        provider,
                        &epoch,
                        &row.sources,
                        orch_provider_contract::SourceElement::Models,
                        orch_provider_contract::SourceErrorCode::Timeout,
                        "The model reader did not become ready in time.",
                    )
                    .await?;
                    return Ok(None);
                }
            }
        }
        let id = new_id();
        let command = Outgoing::new(
            id,
            machine,
            SerialKey::Provider(provider),
            &ProviderDiscoverModels {
                provider_id: provider,
                models: models_source,
            },
        )?;
        let outcome = match tokio::time::timeout(
            self.inner.config.start_timeout,
            crate::commands::run(self.inner.commands.as_ref(), command),
        )
        .await
        {
            Ok(outcome) => outcome?,
            Err(_) => {
                self.inner.commands.cancel(id).await?;
                crate::provider_sources::failed(
                    self,
                    provider,
                    &epoch,
                    &row.sources,
                    orch_provider_contract::SourceElement::Models,
                    orch_provider_contract::SourceErrorCode::Timeout,
                    "The keeping machine did not answer the models source read in time.",
                )
                .await?;
                return Ok(None);
            }
        };
        let Outcome::Succeeded(value) = outcome else {
            crate::provider_sources::failed(
                self,
                provider,
                &epoch,
                &row.sources,
                orch_provider_contract::SourceElement::Models,
                orch_provider_contract::SourceErrorCode::Unavailable,
                "The keeping machine could not finish the models source read.",
            )
            .await?;
            return Ok(None);
        };
        let result: ProviderDiscoverModelsResult = serde_json::from_value(value)?;
        let mut tx =
            orch_db::queries::transaction_guard::scope(true, self.inner.db.begin()).await?;
        if !orch_db::queries::provider_facts::lock_provider(&mut tx, provider).await?
            || orch_db::queries::provider_facts::credential_epoch(&mut tx, provider, None)
                .await?
                .as_ref()
                != Some(&epoch)
        {
            return Ok(None);
        }
        let Some(current) = resources::provider(&mut tx, provider).await? else {
            return Ok(None);
        };
        let now = self.inner.clock.now();
        if let Some(source) = &result.source {
            crate::provider_sources::observed(
                &mut tx,
                provider,
                &current.sources,
                orch_provider_contract::SourceElement::Models,
                source.descriptor.as_ref(),
                source.error.as_ref(),
                Time::new(now),
            )
            .await?;
            if source.error.is_some() && source.supported != Some(true) {
                tx.commit().await.map_err(orch_db::DbError::from)?;
                return Ok(None);
            }
        }
        let suggestions = if result
            .source
            .as_ref()
            .is_some_and(|source| source.supported == Some(false))
        {
            None
        } else {
            Some(result.suggestions)
        };
        let facts = crate::provider_fact_reads::merge_observation(
            &current.facts,
            serde_json::json!({"models":suggestions,"field_times":{"models":Time::new(now)}}),
        );
        keeper::set_facts(&mut tx, provider, &facts, now).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(suggestions)
    }

    /// Whether the runtime declares an authenticated vendor read, independently of whether its latest attempt succeeded.
    pub async fn provider_has_credential_read(
        &self,
        conn: &mut orch_db::Conn,
        provider: Id,
    ) -> Result<bool, EngineError> {
        let Some(row) = resources::provider(conn, provider).await? else {
            return Ok(true);
        };
        if row.sources.get("facts").is_some_and(|choice| {
            choice.get("descriptor").is_some_and(Json::is_object)
                || choice
                    .get("by")
                    .and_then(Json::as_str)
                    .is_some_and(|by| by != "default")
        }) {
            return Ok(crate::provider_fact_reads::source_can_verify(
                &row.sources,
                row.agent_id.is_some(),
            ));
        }
        let Some(agent) = row.agent_id else {
            // Older stored observations are internal normalization inputs. An unknown source
            // gets its first read; an explicit refusal of support is not a credential check.
            return Ok(row.facts.get("key_usage_supported").and_then(Json::as_bool) != Some(false));
        };
        let Some(profile) = resources::agent(conn, agent).await? else {
            return Ok(true);
        };
        let descriptor: orch_agent_contract::descriptor::AgentDescriptor =
            serde_json::from_value(profile.descriptor)?;
        Ok(descriptor
            .capabilities
            .provider_facts
            .contains(&orch_agent_contract::descriptor::ProviderFactKind::CredentialVerification))
    }

    /// Only the source that lists reset credits may spend them, when it declares that operation.
    pub async fn provider_spends_resets(
        &self,
        conn: &mut orch_db::Conn,
        provider: Id,
    ) -> Result<bool, EngineError> {
        let Some(row) = resources::provider(conn, provider).await? else {
            return Ok(false);
        };
        let choices: orch_provider_contract::ProviderSources = serde_json::from_value(row.sources)?;
        if let Some(choice) = choices.get(orch_provider_contract::SourceElement::Facts) {
            if let Some(descriptor) = &choice.descriptor {
                return Ok(descriptor.spends_resets());
            }
            if !matches!(choice.by, orch_provider_contract::SourceBy::Default)
                && !(choice.by == orch_provider_contract::SourceBy::Builtin
                    && choice.name.as_deref()
                        == Some(orch_provider_contract::choice::builtin::ADAPTER))
            {
                return Ok(false);
            }
        }
        // Before a default adapter has been described, its existing declaration remains authoritative.
        let Some(agent) = row.agent_id else {
            return Ok(false);
        };
        let Some(profile) = resources::agent(conn, agent).await? else {
            return Ok(false);
        };
        let descriptor: orch_agent_contract::descriptor::AgentDescriptor =
            serde_json::from_value(profile.descriptor)?;
        Ok(descriptor
            .capabilities
            .provider_facts
            .contains(&orch_agent_contract::descriptor::ProviderFactKind::AvailableResets))
    }
}

// ---------------------------------------------------------------- signing a native provider in

/// The most sign-ins one look at the schedule takes up for each kind of work.
const LOOK_BATCH: i64 = 100;
/// The member of the state a harness reported in which the engine notes when it heard it.
const HEARD_AT: &str = "at";

/// What a person gave for one field of a sign-in, or for one a running sign-in asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum SignInValue {
    /// A value that is no secret.
    Text(String),
    /// A secret: it is in the row of that identifier, sealed, and nowhere else.
    Secret(Id),
}

/// One value of a sign-in as it is kept while it waits: a secret as the reference to its row, never as
/// its value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Kept {
    /// The row of a secret.
    Secret {
        /// Its identifier.
        #[serde(rename = "$secret")]
        id: Id,
    },
    /// A value that is no secret.
    Text(String),
}

impl From<&SignInValue> for Kept {
    fn from(value: &SignInValue) -> Self {
        match value {
            SignInValue::Text(text) => Self::Text(text.clone()),
            SignInValue::Secret(id) => Self::Secret { id: *id },
        }
    }
}

/// A sign-in that waits for the runtime: the method, what was given, and who began it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    /// The adapter's own name for the way of signing in.
    pub method: String,
    /// What was given for its fields.
    pub values: BTreeMap<String, Kept>,
    /// Who began it.
    pub by: Option<Id>,
    /// What names this sign-in among the others of the provider: the command that gives it to the harness
    /// is named by it, so that a second sign-in is a second command and the same one asked for again is
    /// not.
    #[serde(default)]
    pub attempt: Id,
}

impl Pending {
    /// A sign-in by `method` with these values.
    #[must_use]
    pub fn new(method: &str, values: &BTreeMap<String, SignInValue>, by: Id) -> Self {
        Self {
            method: method.to_owned(),
            values: values
                .iter()
                .map(|(name, value)| (name.clone(), Kept::from(value)))
                .collect(),
            by: Some(by),
            attempt: new_id(),
        }
    }

    /// The form it is kept in.
    ///
    /// # Errors
    ///
    /// When it cannot be written, which a map of text and references can always be.
    pub fn to_bytes(&self) -> Result<Vec<u8>, EngineError> {
        Ok(serde_json::to_vec(self)?)
    }

    fn command_values(&self) -> BTreeMap<String, SecretOrText> {
        self.values
            .iter()
            .map(|(name, kept)| {
                (
                    name.clone(),
                    match kept {
                        Kept::Secret { id } => SecretOrText::Secret(Secret::Ref(*id)),
                        Kept::Text(text) => SecretOrText::Text(text.clone()),
                    },
                )
            })
            .collect()
    }
}

/// What came of giving a waiting sign-in to the harness.
#[derive(Debug, Clone)]
pub enum Delivered {
    /// The harness was told. What it shows and asks for comes back as events of the provider's stream.
    Sent,
    /// The runtime is being started: the sign-in is given to the harness when it is up.
    Waits(Option<Box<JobRow>>),
    /// None waits.
    Nothing,
    /// It could not be given, for this reason; the sign-in is over and the harness's words are the
    /// reason, whole.
    Refused(String),
}

/// What came of asking a running sign-in for something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    /// The harness did it.
    Done,
    /// There is no runtime to ask: it is not up.
    NotUp,
    /// The harness refused, with these words.
    Refused(String),
}

/// What came of looking at the sign-in a stopped runtime holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Renewed {
    /// The harness still holds a sign-in that works.
    Held,
    /// It does not, for this reason; the provider's managers were told.
    Lapsed(String),
}

fn heard(state: &Json, at: DateTime<Utc>) -> Json {
    let mut state = state.clone();
    if let Some(members) = state.as_object_mut() {
        members.insert(
            HEARD_AT.to_owned(),
            serde_json::to_value(at).unwrap_or(Json::Null),
        );
    }
    state
}

impl Engine {
    /// The canonical native credential keeper when it can be spoken to: the provider has one, it is
    /// active, online, and takes the role of an agent.
    pub(crate) async fn runtime_machine(
        &self,
        provider: &State,
    ) -> Result<Option<Id>, EngineError> {
        let Some(machine) = provider.key_machine_id else {
            return Ok(None);
        };
        let mut conn = self.inner.db.acquire().await?;
        Ok(resources::machine_place(&mut conn, machine)
            .await?
            .filter(|place| {
                place.deleted_at.is_none()
                    && place.status == "active"
                    && place.online
                    && place.roles.iter().any(|role| role == AGENT_ROLE)
            })
            .map(|place| place.id))
    }

    /// Writes the state a sign-in is in, as the harness would have told it, when the engine itself
    /// knows it first (a sign-in that was asked for, one that was refused).
    pub(crate) async fn note_login(&self, provider: Id, state: &Json) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let mut conn = self.inner.db.acquire().await?;
        runtime_rows::set_login_state(&mut conn, provider, &heard(state, now), now).await?;
        Ok(())
    }

    /// Ends a sign-in that cannot go on: what waited is forgotten with its secrets, and the state says
    /// why.
    async fn give_up_sign_in(&self, provider: Id, why: &str) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let method = match runtime_rows::lock(&mut tx, provider).await? {
            Some(state) => state
                .login_pending
                .as_deref()
                .and_then(|bytes| serde_json::from_slice::<Pending>(bytes).ok())
                .map(|pending| pending.method),
            None => None,
        };
        runtime_rows::delete_login_secrets(&mut tx, provider, runtime_rows::LOGIN_SECRET_PREFIX)
            .await?;
        runtime_rows::clear_pending(&mut tx, provider).await?;
        let failed = serde_json::json!({
            "state": "failed", "method": method, "shows": [], "asks": null,
            "expires_at": null, "error": why,
        });
        runtime_rows::set_login_state(&mut tx, provider, &heard(&failed, now), now).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// Gives the sign-in that waits to the harness of the provider's profile in its runtime, starting
    /// the runtime when it is not up. The secrets in it travel as references to their rows and are
    /// sealed to the machine when the frame is written; once the command is over the rows are deleted
    /// and nothing is kept of what was given.
    pub async fn deliver_sign_in(&self, provider: Id) -> Result<Delivered, EngineError> {
        let state = {
            let mut conn = self.inner.db.acquire().await?;
            runtime_rows::state(&mut conn, provider).await?
        };
        let Some(state) = state.filter(|state| state.deleted_at.is_none()) else {
            return Ok(Delivered::Nothing);
        };
        let Some(pending) = state
            .login_pending
            .as_deref()
            .and_then(|bytes| serde_json::from_slice::<Pending>(bytes).ok())
        else {
            return Ok(Delivered::Nothing);
        };
        let permitted =
            orch_settings::provider_sign_in(&mut *self.inner.db.acquire().await?, provider)
                .await?
                .allowed;
        if !permitted {
            let why = "provider.signin: the credential keeper has not permitted changes in the selected existing harness home";
            self.give_up_sign_in(provider, why).await?;
            return Ok(Delivered::Refused(why.into()));
        }
        let Some(agent_id) = state.agent_id else {
            let why = "an API provider has no sign-in".to_owned();
            self.give_up_sign_in(provider, &why).await?;
            return Ok(Delivered::Refused(why));
        };
        let Some(machine) = self.runtime_machine(&state).await? else {
            return match self.start_provider_runtime(provider, pending.by).await {
                Ok(started) => {
                    // The sign-in has begun, though the harness is not there to be told: the page shows
                    // it as starting while the runtime comes up.
                    self.note_login(
                        provider,
                        &serde_json::json!({
                            "state": "starting", "method": pending.method, "shows": [], "asks": null,
                            "expires_at": null, "error": null,
                        }),
                    )
                    .await?;
                    Ok(Delivered::Waits(match started {
                        RuntimeStart::Job(job) => Some(job),
                        RuntimeStart::Up => None,
                    }))
                }
                Err(crate::provider_runtime::RuntimeError::Internal(error)) => Err(error),
                Err(other) => {
                    let why = other.to_string();
                    self.give_up_sign_in(provider, &why).await?;
                    Ok(Delivered::Refused(why))
                }
            };
        };
        let profile = {
            let mut conn = self.inner.db.acquire().await?;
            resources::agent(&mut conn, agent_id).await?
        };
        let Some(profile) = profile else {
            let why = "the profile whose harness holds the sign-in is not there".to_owned();
            self.give_up_sign_in(provider, &why).await?;
            return Ok(Delivered::Refused(why));
        };
        let key = HarnessKey {
            provider,
            agent: agent_id,
            machine,
        };
        // The harness of the provider's own profile is the one that holds the sign-in: it is started
        // first when it is not running, which also tells the engine what its adapter declares.
        let started = tokio::time::timeout(
            self.inner.config.start_timeout,
            self.ensure_harness(key, &profile, new_id()),
        )
        .await;
        match started {
            Err(_elapsed) => {
                let why = "the harness did not start in time".to_owned();
                self.give_up_sign_in(provider, &why).await?;
                return Ok(Delivered::Refused(why));
            }
            Ok(Err(error)) => return Err(error),
            Ok(Ok(Started::Failed(outcome))) => {
                let why = format!("the harness did not start: {}", failure_text(&outcome));
                self.give_up_sign_in(provider, &why).await?;
                return Ok(Delivered::Refused(why));
            }
            Ok(Ok(Started::Up)) => {}
        }
        let profile = {
            let mut conn = self.inner.db.acquire().await?;
            resources::agent(&mut conn, agent_id)
                .await?
                .unwrap_or(profile)
        };
        if let Err(why) = check_pending(&profile, &pending) {
            self.give_up_sign_in(provider, &why).await?;
            return Ok(Delivered::Refused(why));
        }
        self.note_login(
            provider,
            &serde_json::json!({
                "state": "starting", "method": pending.method, "shows": [], "asks": null,
                "expires_at": null, "error": null,
            }),
        )
        .await?;
        let command = Outgoing::new(
            ids::derived(
                "provider.login.start",
                &[
                    provider.as_bytes().as_slice(),
                    pending.attempt.as_bytes().as_slice(),
                ],
            ),
            machine,
            SerialKey::Harness {
                provider_id: provider,
                agent_id,
            },
            &ProviderLoginStart {
                provider_id: provider,
                agent_id,
                method: pending.method.clone(),
                values: Some(pending.command_values()),
                config: state.config.clone(),
            },
        )?;
        let outcome = crate::commands::run(self.inner.commands.as_ref(), command).await?;
        // The harness has what it was given, or has refused it: either way nothing is kept.
        {
            let mut tx = self.inner.db.begin().await?;
            runtime_rows::delete_login_secrets(
                &mut tx,
                provider,
                runtime_rows::LOGIN_SECRET_PREFIX,
            )
            .await?;
            runtime_rows::clear_pending(&mut tx, provider).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        match outcome {
            Outcome::Succeeded(_) => Ok(Delivered::Sent),
            other => {
                let why = failure_text(&other);
                self.note_login(
                    provider,
                    &serde_json::json!({
                        "state": "failed", "method": pending.method, "shows": [], "asks": null,
                        "expires_at": null, "error": why,
                    }),
                )
                .await?;
                Ok(Delivered::Refused(why))
            }
        }
    }

    /// Gives the harness what the person answered to a sign-in that asked for something. The values
    /// that are secrets are in rows of their own, which are deleted when the command is over.
    pub async fn send_login_input(
        &self,
        provider: Id,
        values: &BTreeMap<String, SignInValue>,
    ) -> Result<Asked, EngineError> {
        if !orch_settings::provider_sign_in(&mut *self.inner.db.acquire().await?, provider)
            .await?
            .allowed
        {
            return Ok(Asked::Refused("provider.signin: the credential keeper has not permitted sign-in input in this existing harness home".into()));
        }
        let (state, machine) = match self.sign_in_target(provider).await? {
            Ok(target) => target,
            Err(asked) => return Ok(asked),
        };
        let Some(agent_id) = state.agent_id else {
            return Ok(Asked::Refused("an API provider has no sign-in".to_owned()));
        };
        let given: BTreeMap<String, SecretOrText> = values
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    match value {
                        SignInValue::Text(text) => SecretOrText::Text(text.clone()),
                        SignInValue::Secret(id) => SecretOrText::Secret(Secret::Ref(*id)),
                    },
                )
            })
            .collect();
        let command = Outgoing::new(
            new_id(),
            machine,
            SerialKey::Harness {
                provider_id: provider,
                agent_id,
            },
            &ProviderLoginInput {
                provider_id: provider,
                agent_id,
                values: given,
            },
        )?;
        let outcome = crate::commands::run(self.inner.commands.as_ref(), command).await?;
        let mut conn = self.inner.db.acquire().await?;
        runtime_rows::delete_login_secrets(&mut conn, provider, runtime_rows::LOGIN_SECRET_PREFIX)
            .await?;
        Ok(match outcome {
            Outcome::Succeeded(_) => Asked::Done,
            other => Asked::Refused(failure_text(&other)),
        })
    }

    /// Gives a sign-in up: what waited is forgotten, and a running one is told to stop.
    pub async fn cancel_sign_in(&self, provider: Id) -> Result<Asked, EngineError> {
        {
            let mut tx = self.inner.db.begin().await?;
            runtime_rows::delete_login_secrets(
                &mut tx,
                provider,
                runtime_rows::LOGIN_SECRET_PREFIX,
            )
            .await?;
            runtime_rows::clear_pending(&mut tx, provider).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        let (state, machine) = match self.sign_in_target(provider).await? {
            Ok(target) => target,
            Err(asked) => {
                // The runtime is not up: nothing runs there to be told, and the state stands as idle.
                self.note_login(
                    provider,
                    &serde_json::json!({
                        "state": "idle", "method": null, "shows": [], "asks": null,
                        "expires_at": null, "error": null,
                    }),
                )
                .await?;
                return Ok(asked);
            }
        };
        let Some(agent_id) = state.agent_id else {
            return Ok(Asked::Done);
        };
        let command = Outgoing::new(
            new_id(),
            machine,
            SerialKey::Harness {
                provider_id: provider,
                agent_id,
            },
            &ProviderLoginCancel {
                provider_id: provider,
                agent_id,
            },
        )?;
        let outcome = crate::commands::run(self.inner.commands.as_ref(), command).await?;
        Ok(match outcome {
            Outcome::Succeeded(_) => Asked::Done,
            other => Asked::Refused(failure_text(&other)),
        })
    }

    /// The provider and the machine a command to its harness goes to, or why there is none.
    async fn sign_in_target(
        &self,
        provider: Id,
    ) -> Result<Result<(State, Id), Asked>, EngineError> {
        let state = {
            let mut conn = self.inner.db.acquire().await?;
            runtime_rows::state(&mut conn, provider).await?
        };
        let Some(state) = state.filter(|state| state.deleted_at.is_none()) else {
            return Ok(Err(Asked::Refused("there is no such provider".to_owned())));
        };
        Ok(match self.runtime_machine(&state).await? {
            Some(machine) => Ok((state, machine)),
            None => Err(Asked::NotUp),
        })
    }

    /// Signs the provider's harness out: it forgets the login it holds. A harness that cannot be asked
    /// because the runtime is not up is [`Asked::NotUp`]; the runtime is then not started here.
    pub async fn sign_out(&self, provider: Id) -> Result<Asked, EngineError> {
        if !orch_settings::provider_sign_in(&mut *self.inner.db.acquire().await?, provider)
            .await?
            .allowed
        {
            return Ok(Asked::Refused("provider.signin: the credential keeper has not permitted signing out in this existing harness home".into()));
        }
        let (state, machine) = match self.sign_in_target(provider).await? {
            Ok(target) => target,
            Err(asked) => return Ok(asked),
        };
        let Some(agent_id) = state.agent_id else {
            return Ok(Asked::Refused("an API provider has no sign-in".to_owned()));
        };
        let profile = {
            let mut conn = self.inner.db.acquire().await?;
            resources::agent(&mut conn, agent_id).await?
        };
        let Some(profile) = profile else {
            return Ok(Asked::Refused(
                "the profile whose harness holds the sign-in is not there".to_owned(),
            ));
        };
        let key = HarnessKey {
            provider,
            agent: agent_id,
            machine,
        };
        if let Started::Failed(outcome) = self.ensure_harness(key, &profile, new_id()).await? {
            return Ok(Asked::Refused(format!(
                "the harness did not start: {}",
                failure_text(&outcome)
            )));
        }
        let command = Outgoing::new(
            new_id(),
            machine,
            SerialKey::Harness {
                provider_id: provider,
                agent_id,
            },
            &ProviderLogout {
                provider_id: provider,
                agent_id,
            },
        )?;
        let outcome = crate::commands::run(self.inner.commands.as_ref(), command).await?;
        let Outcome::Succeeded(_) = outcome else {
            return Ok(Asked::Refused(failure_text(&outcome)));
        };
        self.note_login(
            provider,
            &serde_json::json!({
                "state": "idle", "method": null, "shows": [], "asks": null,
                "expires_at": null, "error": null,
            }),
        )
        .await?;
        // What the harness holds now decides where the provider stands.
        match self.refresh_provider_now(provider).await? {
            Refreshed::Answered { .. } | Refreshed::Gone => {}
            Refreshed::NoMachine | Refreshed::Offline => {}
            Refreshed::Failed(text) => {
                tracing::info!(%provider, %text, "the state of a provider that was signed out could not be read");
            }
        }
        Ok(Asked::Done)
    }

    /// What the harness reports of a sign-in, on the provider's stream: kept as it comes, with the
    /// moment the engine heard it. A completed sign-in is looked at later by the schedule, which asks
    /// the harness for its plan and moves the provider's state.
    pub(crate) async fn on_login_event(
        &self,
        unit: &mut Unit<'_>,
        provider: Id,
        state: &Json,
    ) -> Result<(), EngineError> {
        let now = unit.now();
        runtime_rows::set_login_state(&mut *unit.conn, provider, &heard(state, now), now).await?;
        Ok(())
    }

    /// Gives a sign-in that waits to the harness of a provider whose runtime is up now, for each
    /// provider that has one, and forgets the one that has waited longer than the system allows.
    pub(crate) async fn take_waiting_sign_ins(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let longest =
            chrono::Duration::from_std(self.inner.config.hosting.login_pending).unwrap_or_default();
        let (waiting, overdue) = {
            let mut conn = self.inner.db.acquire().await?;
            (
                runtime_rows::with_pending(&mut conn, LOOK_BATCH).await?,
                runtime_rows::pending_before(&mut conn, now - longest, LOOK_BATCH).await?,
            )
        };
        for provider in overdue {
            self.give_up_sign_in(
                provider,
                "the sign-in waited too long for the provider's runtime and was given up",
            )
            .await?;
        }
        for provider in waiting {
            let state = {
                let mut conn = self.inner.db.acquire().await?;
                runtime_rows::state(&mut conn, provider).await?
            };
            let up = match &state {
                Some(state) => self.runtime_machine(state).await?.is_some(),
                None => false,
            };
            if up && let Err(error) = self.deliver_sign_in(provider).await {
                tracing::warn!(%error, %provider, "a sign-in that waited for its runtime could not be given to the harness");
            }
        }
        Ok(())
    }

    /// For each provider whose sign-in completed since the schedule last looked: the harness is asked
    /// for what it holds, which moves the provider's state; and a sign-in that waits for the person
    /// beyond its end is told that it ran out.
    pub(crate) async fn look_after_sign_ins(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let (completed, overdue) = {
            let mut conn = self.inner.db.acquire().await?;
            (
                runtime_rows::completed_not_looked_at(&mut conn, LOOK_BATCH).await?,
                runtime_rows::overdue_logins(&mut conn, now, LOOK_BATCH).await?,
            )
        };
        for provider in overdue {
            let state = {
                let mut conn = self.inner.db.acquire().await?;
                runtime_rows::state(&mut conn, provider).await?
            };
            let method = state
                .and_then(|state| state.login_state)
                .and_then(|state| state.get("method").cloned())
                .unwrap_or(Json::Null);
            self.note_login(
                provider,
                &serde_json::json!({
                    "state": "expired", "method": method, "shows": [], "asks": null,
                    "expires_at": null, "error": null,
                }),
            )
            .await?;
        }
        for provider in completed {
            self.look_at_completed_sign_in(provider).await?;
        }
        Ok(())
    }

    /// Asks the harness what it holds after a sign-in completed, and notes that it was looked at.
    pub async fn look_at_completed_sign_in(&self, provider: Id) -> Result<Refreshed, EngineError> {
        let looked = self.refresh_provider_now(provider).await?;
        let now = self.inner.clock.now();
        let mut conn = self.inner.db.acquire().await?;
        runtime_rows::mark_looked_at(&mut conn, provider, now).await?;
        Ok(looked)
    }

    /// Has the harness of a provider whose runtime was started to renew its sign-in look at it. A
    /// sign-in that no longer works, or that cannot be looked at, is told to the provider's managers
    /// before a thread needs it.
    pub async fn renew_sign_in(&self, provider: Id) -> Result<Renewed, EngineError> {
        // One renewal and one further explicit request when its known lapse did not extend.
        const RENEWAL_REQUESTS: u32 = 2;
        let before = {
            let mut conn = self.inner.db.acquire().await?;
            let Some(state) = runtime_rows::state(&mut conn, provider).await? else {
                return Ok(Renewed::Held);
            };
            if state
                .facts
                .pointer("/credential/yields")
                .and_then(Json::as_str)
                .is_some_and(|yields| yields != "login")
            {
                return Ok(Renewed::Held);
            }
            state
                .facts
                .pointer("/credential/expires_at")
                .and_then(Json::as_str)
                .and_then(|time| time.parse::<Time>().ok())
        };
        let mut lapsed = "the credential expiry did not extend after renewal".to_owned();
        for _ in 0..RENEWAL_REQUESTS {
            let looked = self
                .read_provider_facts_with_renewal(provider, true, true)
                .await?;
            match looked {
                Refreshed::Answered {
                    reported_status: Some(status),
                    facts,
                    ..
                } if matches!(status.as_str(), "ready" | "exhausted") => {
                    let yields = facts.pointer("/credential/yields").and_then(Json::as_str);
                    if yields.is_some_and(|yields| yields != "login") {
                        return Ok(Renewed::Held);
                    }
                    let after = facts
                        .pointer("/credential/expires_at")
                        .and_then(Json::as_str)
                        .and_then(|time| time.parse::<Time>().ok());
                    let extended = match (before, after) {
                        (Some(before), Some(after)) => {
                            after > before && after.get() > self.inner.clock.now()
                        }
                        (None, Some(after)) => after.get() > self.inner.clock.now(),
                        (None, None) => yields == Some("login"),
                        (Some(_), None) => false,
                    };
                    if extended {
                        let mut conn = self.inner.db.acquire().await?;
                        runtime_rows::mark_credential_renewed(
                            &mut conn,
                            provider,
                            self.inner.clock.now(),
                        )
                        .await?;
                        return Ok(Renewed::Held);
                    }
                }
                Refreshed::Answered {
                    reported_status: Some(status),
                    ..
                } if status == "needs_setup" => {
                    lapsed = SIGNED_OUT.to_owned();
                    break;
                }
                Refreshed::Answered { .. } => {
                    lapsed = "the vendor did not supply a current renewal observation".to_owned();
                }
                Refreshed::Gone => return Ok(Renewed::Held),
                Refreshed::NoMachine => {
                    lapsed = "its runtime is not there".to_owned();
                    break;
                }
                Refreshed::Offline => {
                    lapsed = "its runtime is away".to_owned();
                    break;
                }
                Refreshed::Failed(text) => {
                    lapsed = text;
                    break;
                }
            }
        }
        self.tell_attention(
            provider,
            "renewal",
            "Its sign-in needs attention",
            &format!("When its native sign-in was renewed, the harness said: {lapsed}."),
        )
        .await?;
        Ok(Renewed::Lapsed(lapsed))
    }

    /// Tells the managers of a provider that it needs attention. Once for each `source`, however often
    /// it is told.
    async fn tell_attention(
        &self,
        provider: Id,
        source: &str,
        title: &str,
        body: &str,
    ) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let Some(state) = runtime_rows::state(&mut tx, provider).await? else {
            return Ok(());
        };
        let managers =
            crate::provider_fact_reads::managers(&mut tx, provider, state.owner_id, now).await?;
        // Expiry's source identifies the credential end, so subsequent days do not repeat it.
        let source = if source == "renewal" {
            format!("{provider}:{source}:{}", now.format("%Y-%m-%d"))
        } else {
            format!("{provider}:{source}")
        };
        crate::notifications::tell(
            &mut tx,
            &crate::notifications::Event {
                kind: crate::notifications::Kind::ProviderAttention,
                node: Some(state.home_id),
                thread: None,
                approval: None,
                title: format!("{}: {title}", state.name),
                body: body.to_owned(),
                data: serde_json::json!({
                    "provider": provider, "provider_name":state.name, "reason":source, "path": format!("/providers/{provider}"),
                }),
                source,
                actor: None,
                at: now,
            },
            &managers,
            Some(state.org_id),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// Tells the managers of a provider that holds a token of the end it will come to, ahead by
    /// `providers.expiry_notice_days`, and again when it has passed: the provider then needs a sign-in.
    pub(crate) async fn watch_token_ends(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let ends = {
            let mut conn = self.inner.db.acquire().await?;
            runtime_rows::token_ends(&mut conn, LOOK_BATCH).await?
        };
        for end in ends {
            let notice: i64 = {
                let mut conn = self.inner.db.acquire().await?;
                setting(
                    &mut conn,
                    SettingsPlace::Node(end.org_id),
                    keys::PROVIDERS_EXPIRY_NOTICE_DAYS,
                )
                .await?
                .unwrap_or(0)
            };
            let day = end.expires_at.format("%Y-%m-%d").to_string();
            if end.expires_at <= now {
                let moved = {
                    let mut tx =
                        orch_db::queries::transaction_guard::scope(true, self.inner.db.begin())
                            .await?;
                    let current_end =
                        orch_db::queries::provider_facts::credential_end(&mut tx, end.provider_id)
                            .await?;
                    // A fresh sign-in must not be demoted by the old token's clock after the scan.
                    if current_end.is_none_or(|expires| {
                        expires != end.expires_at || expires > self.inner.clock.now()
                    }) {
                        continue;
                    }
                    let moved = keeper::move_status(
                        &mut tx,
                        end.provider_id,
                        &["ready", "exhausted", "offline"],
                        "needs_setup",
                        Some(&format!("its token lapsed on {day}")),
                        now,
                    )
                    .await?;
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                    moved
                };
                self.tell_attention(
                    end.provider_id,
                    &format!("lapsed:{}", end.expires_at.to_rfc3339()),
                    "Its token has lapsed",
                    &format!("The token it is signed in with lapsed on {day}: it needs a sign-in."),
                )
                .await?;
                if moved {
                    self.reevaluate_provider(end.provider_id).await?;
                }
            } else if notice > 0
                && end.expires_at - now
                    <= expiry_notice_window(end.issued_at, end.expires_at, notice)
            {
                self.tell_attention(
                    end.provider_id,
                    &format!("soon:{}", end.expires_at.to_rfc3339()),
                    "Its token is about to lapse",
                    &format!("The token it is signed in with lapses on {day}."),
                )
                .await?;
            }
        }
        Ok(())
    }
}

/// A short token is warned when a quarter of its life remains; longer ones use the manager's setting.
fn expiry_notice_window(
    issued: Option<chrono::DateTime<chrono::Utc>>,
    expires: chrono::DateTime<chrono::Utc>,
    days: i64,
) -> chrono::Duration {
    // Twice the setting separates tokens that would otherwise be warned immediately after importing them.
    const SHORT_LIFE_FACTOR: i32 = 2;
    // One quarter leaves time to replace a short token without warning at sign-in.
    const LIFE_NOTICE_DIVISOR: i32 = 4;
    let configured = chrono::Duration::days(days);
    if let Some(issued) = issued.filter(|issued| *issued < expires) {
        let life = expires - issued;
        if life < configured * SHORT_LIFE_FACTOR {
            return life / LIFE_NOTICE_DIVISOR;
        }
    }
    configured
}

/// Whether the sign-in that waits can be given to the harness as its adapter declares: the method is
/// one of its, nothing is given that it does not ask for, and what it needs is given.
fn check_pending(profile: &AgentRow, pending: &Pending) -> Result<(), String> {
    let descriptor = harnesses::descriptor_of(profile)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "the harness has not said what it is".to_owned())?;
    let methods = descriptor
        .native_provider
        .map(|native| native.login_methods)
        .unwrap_or_default();
    let Some(method) = methods.iter().find(|method| method.id == pending.method) else {
        return Err(format!(
            "the harness has no way of signing in called {}",
            pending.method
        ));
    };
    for name in pending.values.keys() {
        if !method.fields.iter().any(|field| &field.name == name) {
            return Err(format!("{name} is not asked for by this way of signing in"));
        }
    }
    for field in &method.fields {
        if field.required && field.default.is_none() && !pending.values.contains_key(&field.name) {
            return Err(format!(
                "{} is needed by this way of signing in",
                field.name
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn place(facts: Json) -> MachinePlace {
        MachinePlace {
            id: new_id(),
            roles: vec!["agent".to_owned()],
            status: "active".to_owned(),
            online: true,
            placement: "connected".to_owned(),
            owner_id: None,
            facts,
            deleted_at: None,
        }
    }

    #[test]
    fn a_machine_is_asked_unless_it_said_that_it_cannot_drive_the_adapter() {
        let reported = place(json!({ "adapters": [{ "kind": "codex" }, { "kind": "fake" }] }));
        assert!(drives(&reported, "codex"));
        assert!(!drives(&reported, "claude-code"));
        assert!(
            drives(&place(json!({})), "claude-code"),
            "a machine that listed nothing has not said that it cannot"
        );
    }
}
