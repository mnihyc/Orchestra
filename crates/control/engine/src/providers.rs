//! Provider runtimes as a pool of their own: made when a provider is added and kept as long, started, stopped, stopped when idle, and started while stopped so that a sign-in is renewed; sign-in on the provider, kept on its own volume through every successor; the arrangement types its owner allows, and what its runtime has fallen behind on.
//!
//! **What is built here: a provider on a key.** An API provider is a key and the endpoints it is good at. The
//! machine that keeps it is the one that forwards the model calls of a turn, adds the key and counts what each
//! call used (`lease.usage`). Where the key is kept decides where the provider is run:
//!
//! * a key that a person put on a connected machine stays there (`key_machine_id`), and the machine is asked to
//!   run the provider without it (`provider.start` with `credential: null`);
//! * a key that the server holds is sealed in the store and goes to the machine that runs the provider as a
//!   reference to its secret (`{"$secret": id}`), which the link seals to that machine when the frame is
//!   written. The reference is all that passes through the engine and all that is stored in a command: the key
//!   itself is in no command, no event, no log and no row but its own, sealed.
//!
//! **Started when a turn needs it.** Before the first turn that goes through the forwarder, and again when what
//! the start carries changed (the key was rotated, an endpoint was edited), the engine sends `provider.start`.
//! The command's identifier is made of the provider, the machine and a digest of what it carries, so that a
//! second turn finds the first one's start in the link's rows and sends nothing, and a process that starts
//! again sends nothing either. A start that failed is asked for under the next attempt's identifier by the
//! next turn. A machine that has lost the provider since (its store was wiped, the provider was stopped and
//! switched on again) answers `lease.open` with `not_found`: the turn asks for the start once more under an
//! identifier of its own lease and opens the lease again.
//!
//! **Stopped when switched off or deleted.** A provider whose status is `disabled` (which a deleted one also
//! is) is stopped on its machine (`provider.stop`, once for each act that switched it off, found by looking at
//! the providers changed within the look-back of the supervisor); its open leases are closed with the reason
//! `disabled`, and a turn that was running under one is interrupted by the system and its thread is blocked,
//! since its next call would be refused at the wire.
//!
//! **Its status and facts come from the machine that keeps it** (`provider.status`, `provider.facts` on the
//! provider's stream), accepted only from that machine. A key the upstream refuses is `error`, and one that it
//! takes again is `ready`; the turns that run under the provider's leases when it goes to `error` or
//! `exhausted` end as failed with the reason, and the threads that were blocked for want of it are looked at
//! again when it returns. Presence moves the status as well: an API provider whose keeping machine is away is
//! `offline` and is `ready` again when the machine returns.
//!
//! **Usage and denials arrive here.** Every `lease.usage` becomes one row of the usage ledger, and
//! `lease.denied` fails the turn it was for with its reason (`on_lease_usage` and `on_lease_denied` below).
//!
//! **Not built here:** a provider's managed runtime (made, started on a host, stopped when idle, updated), the
//! sign-in of a subscription, and a credential that leaves the provider. A provider with no machine to run on
//! blocks its threads with that reason.

use crate::commands::{Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use crate::ingest;
use crate::leases;
use crate::preflight::Blocked;
use crate::rates::RateBook;
use crate::turns::{InputAfter, StartFailure, TurnEnd, error_json};
use crate::unit::Unit;
use chrono::{DateTime, Utc};
use orch_agent_contract::events::TokenUsage;
use orch_agent_contract::parameters::{ModelList, ModelParameters, ParameterDef, ParameterOptions};
use orch_db::queries::resources::keeper::{self, SecretRef, StartRow};
use orch_db::queries::resources::thread_engine::{self as resources, ProviderRow};
use orch_db::queries::resources::{binding, provider_runtime as runtime_rows};
use orch_db::queries::threads::{self, LeaseRow, turn_state};
use orch_db::usage::ledger::{self, Attribution, NewEvent, Tokens};
use orch_link::streams::DeliveredEvent;
use orch_link_contract::commands::{
    ErrorCode, ProviderStart, ProviderStartUpstream, ProviderStop, Secret, SerialKey,
};
use orch_link_contract::events::{Event, LeaseDenied, LeaseUsage, ProviderStatus, TurnError};
use orch_link_contract::streams::Stream;
use orch_types::{Hash, Id, Json, Time};
use serde_json::json;
use std::collections::BTreeMap;

/// The name under which a provider's key is stored among its secrets.
const API_KEY: &str = "api_key";
/// What the name of a secret header of an endpoint begins with among a provider's secrets.
const HEADER_PREFIX: &str = "header:";
/// How many times a start that failed is asked for again under the same configuration: far more than a
/// working system needs, so that the probe for the latest attempt stays short and a configuration that
/// never starts ends in its last failure and not in a loop.
const START_ATTEMPTS: u32 = 32;
/// The reasons for which a refused request fails the turn it was for; the reason is the turn's error code.
const DENIED_REASONS: [&str; 2] = ["model_not_allowed", "cap_reached"];
/// The `leases.close_reason` of a lease closed because its provider was switched off.
pub const DISABLED: &str = "disabled";

// ---------------------------------------------------------------- what provider.start carries

/// Why a provider has no `provider.start` to send.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    /// It is no provider on a key: a subscription is signed in on a harness, not started.
    #[error("only a provider on an API key is started")]
    NotKeyed,
    /// No machine keeps it and none runs it.
    #[error("the provider has no machine to run on: its runtime is not running")]
    NoMachine,
    /// Its endpoints do not read.
    #[error("the endpoints of the provider do not read: {0}")]
    Upstreams(String),
    /// The selected key presentation could not be read.
    #[error("the credential presentation choice does not read: {0}")]
    Presentation(String),
}

/// A provider's `provider.start`, made whole, and the digest that says when it has to be sent again.
#[derive(Debug, Clone, PartialEq)]
pub struct StartPlan {
    /// The provider.
    pub provider: Id,
    /// The machine it is started on: where the key is kept, else where the provider runs.
    pub machine: Id,
    /// The command's body, with references where the secrets are.
    pub body: ProviderStart,
    /// A digest of everything the command carries, in hexadecimal.
    pub digest: String,
}

/// The `provider.start` of the provider `row`, whose secrets are `secrets`. A key the server holds travels as
/// a reference to its secret; one kept on a machine does not travel at all.
pub fn plan_start(row: &StartRow, secrets: &[SecretRef]) -> Result<StartPlan, PlanError> {
    if row.kind != "api" {
        return Err(PlanError::NotKeyed);
    }
    let machine = row.key_machine_id.ok_or(PlanError::NoMachine)?;
    let upstreams: Vec<ProviderStartUpstream> = serde_json::from_value(row.upstreams.clone())
        .map_err(|error| PlanError::Upstreams(error.to_string()))?;
    let credential = secrets
        .iter()
        .find(|secret| secret.name == API_KEY)
        .map(|secret| Secret::Ref(secret.id));
    let headers: BTreeMap<String, Secret> = secrets
        .iter()
        .filter_map(|secret| {
            secret
                .name
                .strip_prefix(HEADER_PREFIX)
                .map(|name| (name.to_owned(), Secret::Ref(secret.id)))
        })
        .collect();
    let choices: orch_provider_contract::ProviderSources = if row.sources.is_null() {
        Default::default()
    } else {
        serde_json::from_value(row.sources.clone())
            .map_err(|error| PlanError::Presentation(error.to_string()))?
    };
    let presenter = choices.presenter.as_ref().map(|choice| {
        let mut selected = choice.spec(orch_provider_contract::SourceElement::Presenter, &|name| {
            secrets
                .iter()
                .find(|secret| secret.name == name)
                .map(|secret| secret.id)
        });
        selected.provider = Some(orch_provider_contract::source::SourceAskProvider {
            id: row.id,
            name: row.name.clone(),
        });
        selected
    });
    let mut hasher = Hash::hasher();
    hasher.update(machine.as_bytes());
    hasher.update(row.protocols.join("\n").as_bytes());
    hasher.update(row.upstreams.to_string().as_bytes());
    hasher.update(
        &serde_json::to_vec(&presenter)
            .map_err(|error| PlanError::Presentation(error.to_string()))?,
    );
    for secret in secrets {
        hasher.update(secret.name.as_bytes());
        hasher.update(secret.id.as_bytes());
        hasher.update(&secret.version.timestamp_micros().to_le_bytes());
    }
    hasher.update(&[u8::from(credential.is_some())]);
    let digest = Hash::finish(&hasher).to_string();
    Ok(StartPlan {
        provider: row.id,
        machine,
        body: ProviderStart {
            provider_id: row.id,
            protocols: row.protocols.clone(),
            upstreams,
            credential,
            secrets: headers,
            presenter,
        },
        digest,
    })
}

/// How asking a machine to start a provider came out.
#[derive(Debug)]
pub(crate) enum Keeping {
    /// It runs there.
    Up,
    /// The command ended otherwise.
    Failed(Outcome),
    /// There is no command to send.
    Cannot(PlanError),
}

fn keeping(outcome: Outcome) -> Keeping {
    match outcome {
        Outcome::Succeeded(_) => Keeping::Up,
        other => Keeping::Failed(other),
    }
}

/// What a failed start of a provider, or a lease opened with it, means for the turn it was for: the thread
/// waits as `provider_unavailable`, or `binding_invalid` where the machine cannot serve what it was asked.
#[must_use]
pub fn classify_keeper(what: &str, outcome: &Outcome) -> StartFailure {
    let failure = |message: String, retryable: bool, reason: &'static str| StartFailure {
        code: "harness_error",
        blocked: Blocked {
            reason,
            detail: Some(message.clone()),
        },
        message,
        retryable,
    };
    match outcome {
        Outcome::Succeeded(_) => {
            failure(format!("{what} succeeded"), false, "provider_unavailable")
        }
        Outcome::Expired => failure(
            format!("{what}: the machine that keeps the key did not take it up in time"),
            true,
            "provider_unavailable",
        ),
        Outcome::Cancelled => failure(
            format!(
                "{what}: the machine that keeps the key was revoked or the request was withdrawn"
            ),
            false,
            "provider_unavailable",
        ),
        Outcome::Failed(error) => {
            let message = format!("{what}: {}: {}", error.code, error.detail);
            match error.known() {
                // What the machine cannot serve, or the policy of the person it belongs to does not let it
                // serve, is a binding that does not hold; no waiting changes it.
                Some(
                    ErrorCode::Unsupported
                    | ErrorCode::PolicyRootNotAllowed
                    | ErrorCode::PolicyAutoApprovalNotAllowed
                    | ErrorCode::PolicyUpstreamNotAllowed
                    | ErrorCode::PolicyArrangementNotAllowed
                    | ErrorCode::PolicyExecNotOffered,
                ) => failure(message, false, "binding_invalid"),
                Some(ErrorCode::OfflinePeer | ErrorCode::Timeout) => {
                    failure(message, true, "provider_unavailable")
                }
                _ => failure(message, error.retryable, "provider_unavailable"),
            }
        }
    }
}

/// What a provider that cannot be started means for the turn.
#[must_use]
pub fn classify_plan(error: &PlanError) -> StartFailure {
    let message = format!("the provider cannot be started: {error}");
    StartFailure {
        code: "harness_error",
        blocked: Blocked {
            reason: match error {
                PlanError::Upstreams(_) | PlanError::Presentation(_) | PlanError::NotKeyed => {
                    "binding_invalid"
                }
                PlanError::NoMachine => "provider_unavailable",
            },
            detail: Some(message.clone()),
        },
        message,
        retryable: false,
    }
}

// ---------------------------------------------------------------- tokens

fn count(usage: &Json, member: &str) -> i64 {
    usage.get(member).and_then(Json::as_i64).unwrap_or(0).max(0)
}

/// The tokens a usage the wire carries holds. A part that is larger than the whole it is a part of is cut to
/// it, and a total that is not given is the input plus the output.
#[must_use]
pub fn tokens_of(usage: &Json) -> Tokens {
    let input = count(usage, "input_tokens");
    let output = count(usage, "output_tokens");
    let cached_input = count(usage, "cached_input_tokens").min(input);
    let cache_write = count(usage, "cache_write_tokens").min(input - cached_input);
    let reasoning = count(usage, "reasoning_tokens").min(output);
    let total = match usage.get("total_tokens").and_then(Json::as_i64) {
        Some(total) if total >= 0 => total,
        _ => input + output,
    };
    Tokens {
        input,
        cached_input,
        cache_write,
        output,
        reasoning,
        total,
    }
}

/// One request, or a lease's remainder, as the forwarder counted it.
struct Forwarded<'a> {
    timing: Option<&'a orch_api_contract::usage::RequestTiming>,
    harness: bool,
    counted: bool,
    /// The row's identifier, the same however often it is reported.
    id: Id,
    /// When it was counted.
    at: DateTime<Utc>,
    /// The model it ran on.
    model: &'a str,
    /// What it used.
    tokens: Tokens,
    /// The upstream's name for it, or the word for a lease's remainder.
    request_ref: Option<&'a str>,
}

/// What the request reference of a lease's remainder reads.
const CLOSE_REF: &str = "lease.close";

/// What `total` has beyond `counted`, member by member; nothing where it has no more.
#[must_use]
pub fn shortfall(total: &Tokens, counted: &Tokens) -> Tokens {
    let beyond = |total: i64, counted: i64| (total - counted).max(0);
    let input = beyond(total.input, counted.input);
    let output = beyond(total.output, counted.output);
    Tokens {
        input,
        cached_input: beyond(total.cached_input, counted.cached_input).min(input),
        cache_write: beyond(total.cache_write, counted.cache_write).min(input),
        output,
        reasoning: beyond(total.reasoning, counted.reasoning).min(output),
        total: input + output,
    }
}

/// The usage of a turn as the wire has it.
#[must_use]
pub fn usage_json(tokens: &Tokens) -> Json {
    serde_json::to_value(TokenUsage {
        input_tokens: tokens.input,
        cached_input_tokens: tokens.cached_input,
        cache_write_tokens: tokens.cache_write,
        output_tokens: tokens.output,
        reasoning_tokens: tokens.reasoning,
        total_tokens: tokens.total,
    })
    .unwrap_or(Json::Null)
}

/// The larger of two usages, part by part.
#[must_use]
pub fn larger(a: &Tokens, b: &Tokens) -> Tokens {
    Tokens {
        input: a.input.max(b.input),
        cached_input: a.cached_input.max(b.cached_input),
        cache_write: a.cache_write.max(b.cache_write),
        output: a.output.max(b.output),
        reasoning: a.reasoning.max(b.reasoning),
        total: a.total.max(b.total),
    }
}

/// The service tier a turn ran with: the value of the parameter whose options are the model's service tiers.
pub(crate) fn service_tier(parameters: &[ParameterDef], params: &Json) -> Option<String> {
    parameters
        .iter()
        .find(|definition| {
            matches!(
                definition.options,
                Some(ParameterOptions::FromModel {
                    from_model: ModelList::ServiceTiers
                })
            )
        })
        .and_then(|definition| params.get(&definition.key))
        .and_then(Json::as_str)
        .map(str::to_owned)
}

/// Programme identity follows the adapter declaration, including an external adapter's chosen key.
pub(crate) fn access_program(parameters: &[ParameterDef], params: &Json) -> Option<String> {
    parameters
        .iter()
        .find(|definition| {
            matches!(
                definition.options,
                Some(ParameterOptions::FromModel {
                    from_model: ModelList::AccessPrograms
                })
            )
        })
        .and_then(|definition| params.get(&definition.key))
        .and_then(Json::as_str)
        .map(str::to_owned)
}

// ---------------------------------------------------------------- starting a provider

impl Engine {
    /// The `provider.start` of a provider as the database holds it now.
    async fn start_plan(&self, provider: Id) -> Result<Result<StartPlan, PlanError>, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let Some(row) = keeper::start_row(&mut conn, provider).await? else {
            return Ok(Err(PlanError::NotKeyed));
        };
        let secrets = keeper::secret_refs(&mut conn, provider).await?;
        Ok(plan_start(&row, &secrets))
    }

    /// Makes sure the provider runs on the machine that keeps its key: `provider.start` where the link's
    /// rows do not say that it was, for what it carries now. The command is the same for every thread and
    /// every turn that needs the provider, and the machine answers one it has run with itself.
    pub(crate) async fn ensure_provider_started(
        &self,
        thread: Id,
        provider: Id,
    ) -> Result<Keeping, EngineError> {
        let plan = match self.start_plan(provider).await? {
            Ok(plan) => plan,
            Err(error) => return Ok(Keeping::Cannot(error)),
        };
        let mut attempt: u32 = 0;
        loop {
            let id = ids::provider_start(plan.provider, plan.machine, &plan.digest, attempt);
            match self.inner.commands.status(id).await? {
                Some(info) => match info.outcome {
                    Some(Outcome::Succeeded(_)) => return Ok(Keeping::Up),
                    Some(failed) if attempt + 1 >= START_ATTEMPTS => {
                        return Ok(Keeping::Failed(failed));
                    }
                    Some(_) => attempt += 1,
                    None => return Ok(keeping(self.inner.commands.wait(id).await?)),
                },
                None => {
                    self.set_step(thread, "provider").await?;
                    return self.send_provider_start(&plan, id).await;
                }
            }
        }
    }

    /// Asks the machine to start the provider again, because it has lost it: for the lease that found it
    /// without, and once for that lease.
    pub(crate) async fn restart_provider(
        &self,
        provider: Id,
        lease: Id,
    ) -> Result<Keeping, EngineError> {
        let plan = match self.start_plan(provider).await? {
            Ok(plan) => plan,
            Err(error) => return Ok(Keeping::Cannot(error)),
        };
        let id = ids::provider_restart(plan.provider, plan.machine, &plan.digest, lease);
        self.send_provider_start(&plan, id).await
    }

    async fn send_provider_start(&self, plan: &StartPlan, id: Id) -> Result<Keeping, EngineError> {
        let command = Outgoing::new(
            id,
            plan.machine,
            SerialKey::Provider(plan.provider),
            &plan.body,
        )?;
        Ok(keeping(
            crate::commands::run(self.inner.commands.as_ref(), command).await?,
        ))
    }
}

// ---------------------------------------------------------------- what the provider's stream says

/// Only a vendor-authenticated observation can recover a refused credential; local files cannot.
pub(crate) fn refreshed_transition(
    status: &str,
    credential_verified: bool,
) -> Option<(&'static str, &'static [&'static str])> {
    match status {
        "ready" if credential_verified => Some(("ready", &["error", "exhausted", "offline"])),
        "exhausted" if credential_verified => Some(("exhausted", &["ready", "error", "offline"])),
        "ready" => Some(("ready", &["exhausted", "offline"])),
        "exhausted" => Some(("exhausted", &["ready", "offline"])),
        "needs_setup" => Some(("needs_setup", &["ready", "exhausted", "offline"])),
        _ => None,
    }
}

/// A refused provider's waiting threads name the provider and its recorded reason.
pub(crate) fn unavailable_detail(provider: &ProviderRow) -> String {
    if provider.status == "error" {
        format!(
            "Provider {} ({}): {}",
            provider.name,
            provider.id,
            provider
                .status_detail
                .as_deref()
                .unwrap_or("its credential was refused")
        )
    } else {
        provider.status.clone()
    }
}

impl Engine {
    /// Records the first refusal in an episode and tells current managers in the same transaction.
    pub(crate) async fn reject_provider(
        &self,
        unit: &mut Unit<'_>,
        provider: Id,
        reason: &str,
    ) -> Result<bool, EngineError> {
        let now = unit.now();
        if !keeper::move_status(
            &mut *unit.conn,
            provider,
            &["ready", "needs_setup", "exhausted", "offline"],
            "error",
            Some(reason),
            now,
        )
        .await?
        {
            return Ok(false);
        }
        orch_db::queries::access::bump_epoch(&mut *unit.conn).await?;
        let Some(state) = runtime_rows::state(&mut *unit.conn, provider).await? else {
            return Ok(true);
        };
        let managers =
            crate::provider_fact_reads::managers(&mut *unit.conn, provider, state.owner_id, now)
                .await?;
        crate::notifications::tell(
            &mut *unit.conn,
            &crate::notifications::Event {
                kind: crate::notifications::Kind::ProviderAttention,
                node: Some(state.home_id),
                thread: None,
                approval: None,
                title: format!("{}: its credential needs attention", state.name),
                body: reason.to_owned(),
                data: json!({ "provider": provider, "path": format!("/providers/{provider}") }),
                source: format!("{provider}:credential_refused:{}", state.revision),
                actor: None,
                at: now,
            },
            &managers,
            Some(state.org_id),
        )
        .await?;
        Ok(true)
    }

    /// A new completed sign-in or accepted model work is proof that a rejected credential works again.
    pub(crate) async fn accept_provider(
        &self,
        unit: &mut Unit<'_>,
        provider: Id,
    ) -> Result<(), EngineError> {
        let Some(row) = resources::provider(&mut *unit.conn, provider).await? else {
            return Ok(());
        };
        if row.status != "error" {
            return Ok(());
        }
        let to = if binding::enabled_models(&mut *unit.conn, provider).await? > 0 {
            "ready"
        } else {
            "needs_setup"
        };
        let now = unit.now();
        if keeper::move_status(&mut *unit.conn, provider, &["error"], to, None, now).await? {
            orch_db::queries::access::bump_epoch(&mut *unit.conn).await?;
            for thread in
                threads::blocked_threads_of_provider(&mut *unit.conn, provider, None).await?
            {
                unit.wake(thread);
            }
        }
        Ok(())
    }

    /// Applies the events of a provider's stream, in order, inside the transaction of the delivery. Only the
    /// machine that keeps the provider may speak for it: what any other machine says of it is left unheard.
    pub(crate) async fn apply_provider_events(
        &self,
        unit: &mut Unit<'_>,
        machine: Id,
        provider_id: Id,
        events: &[DeliveredEvent],
    ) -> Result<(), EngineError> {
        let Some(provider) = resources::provider(&mut *unit.conn, provider_id).await? else {
            tracing::warn!(%provider_id, "events of a provider that is not there");
            return Ok(());
        };
        if leases::machine_of(&provider) != Some(machine) {
            tracing::warn!(%provider_id, %machine, "a machine spoke for a provider it does not keep; it is not heard");
            return Ok(());
        }
        let stream = Stream::Provider { provider_id }.to_string();
        for delivered in events {
            let event = match Event::decode(&delivered.kind, delivered.body.clone()) {
                Ok(event) => event,
                Err(error) => {
                    tracing::warn!(%error, kind = %delivered.kind, "an event of a provider does not read");
                    continue;
                }
            };
            match event {
                Event::ProviderStatus(status) => {
                    self.on_provider_status(unit, &provider, &status).await?;
                }
                Event::ProviderFacts(facts) => {
                    let now = unit.now();
                    let Some(provider) = resources::provider(&mut *unit.conn, provider_id).await?
                    else {
                        continue;
                    };
                    let choice = provider.sources.get("facts");
                    let custom = match choice
                        .and_then(|choice| choice.get("by"))
                        .and_then(Json::as_str)
                    {
                        Some("program" | "address" | "none") => true,
                        Some("builtin") => {
                            choice
                                .and_then(|choice| choice.get("name"))
                                .and_then(Json::as_str)
                                != Some("adapter")
                        }
                        _ => false,
                    };
                    let incoming = crate::provider_fact_reads::normalize_observation(
                        &serde_json::to_value(facts)?,
                    )?;
                    let incoming = if provider.harness_kind.is_some() && custom {
                        crate::provider_fact_reads::keeper_observation(incoming)
                    } else {
                        incoming
                    };
                    let tells_admission = incoming
                        .get("ordinary_usage_allowed")
                        .is_some_and(|value| !value.is_null())
                        || incoming
                            .get("limit_reached")
                            .is_some_and(|value| !value.is_null());
                    let reports_allowed = incoming
                        .get("ordinary_usage_allowed")
                        .and_then(Json::as_bool)
                        == Some(true);
                    let known = crate::provider_fact_reads::normalize_observation(&provider.facts)?;
                    let facts = crate::provider_fact_reads::merge_observation(&known, incoming);
                    let facts = keep_standing_limit(&provider.status, &provider.facts, facts, now);
                    keeper::set_facts(&mut *unit.conn, provider.id, &facts, now).await?;
                    let observation: orch_agent_contract::facts::ProviderFacts =
                        serde_json::from_value(facts)?;
                    let exhausted = crate::provider_fact_reads::admission_exhausted(
                        &observation,
                        Time::new(now),
                    );
                    if tells_admission
                        && (exhausted || (provider.status == "exhausted" && reports_allowed))
                    {
                        self.on_provider_status(
                            unit,
                            &provider,
                            &ProviderStatus {
                                status: if exhausted { "exhausted" } else { "ready" }.to_owned(),
                                detail: None,
                            },
                        )
                        .await?;
                    }
                }
                Event::LeaseUsage(usage) => {
                    let ctx = UsageContext {
                        machine,
                        stream: &stream,
                        seq: delivered.seq,
                        at: delivered.at.get(),
                    };
                    self.on_lease_usage(unit, &provider, &ctx, &usage).await?;
                }
                Event::LeaseDenied(denied) => {
                    self.on_lease_denied(unit, &provider, &denied).await?;
                }
                // What a sign-in shows and asks for, step by step (the part that keeps providers).
                Event::ProviderLogin(state) => {
                    let told = serde_json::to_value(&state)?;
                    self.on_login_event(unit, provider.id, &told).await?;
                    if told.get("state").and_then(Json::as_str) == Some("completed") {
                        self.accept_provider(unit, provider.id).await?;
                    }
                }
                _ => {
                    tracing::debug!(kind = %delivered.kind, "an event of a provider the engine has nothing to do with");
                }
            }
        }
        Ok(())
    }

    /// `provider.status`: the machine says where the provider stands. A key the upstream refuses is `error`,
    /// one it takes again is `ready`. The threads that were blocked for want of the provider are looked at
    /// again, and the turns that run under its leases when it is not ready end as failed with the reason.
    async fn on_provider_status(
        &self,
        unit: &mut Unit<'_>,
        provider: &ProviderRow,
        status: &ProviderStatus,
    ) -> Result<(), EngineError> {
        let (to, from): (&str, &[&str]) = match status.status.as_str() {
            "ready" if provider.protocols.is_empty() => ("ready", &["exhausted", "offline"]),
            "ready" => ("ready", &["error", "exhausted", "offline"]),
            "error" => ("error", &["ready", "error", "exhausted", "offline"]),
            "exhausted" => ("exhausted", &["ready", "exhausted", "offline"]),
            other => {
                tracing::warn!(provider = %provider.id, status = other, "a provider is told to be in a state only its owner puts it in; it is left as it is");
                return Ok(());
            }
        };
        let now = unit.now();
        let moved = if to == "error" {
            self.reject_provider(
                unit,
                provider.id,
                status
                    .detail
                    .as_deref()
                    .unwrap_or("the upstream refused the provider's credential"),
            )
            .await?
        } else {
            keeper::move_status(
                &mut *unit.conn,
                provider.id,
                from,
                to,
                status.detail.as_deref(),
                now,
            )
            .await?
        };
        if !moved {
            return Ok(());
        }
        if to == "exhausted" {
            crate::notifications::provider_exhausted(&mut *unit.conn, provider.id, None, now)
                .await?;
        }
        for thread in
            threads::blocked_threads_of_provider(&mut *unit.conn, provider.id, None).await?
        {
            unit.wake(thread);
        }
        if to == "ready" {
            return Ok(());
        }
        let (code, fallback) = if to == "error" {
            (
                "provider_unauthorized",
                "the upstream refused the provider's key",
            )
        } else {
            ("provider_exhausted", "the provider's allowance is used up")
        };
        let message = status.detail.clone().unwrap_or_else(|| fallback.to_owned());
        for lease in keeper::open_leases_of(&mut *unit.conn, provider.id).await? {
            if lease.mode != "forwarded" {
                continue;
            }
            self.end_leased_turn(
                unit,
                &lease,
                Ending {
                    state: turn_state::FAILED,
                    code,
                    message: message.clone(),
                    input: InputAfter::Retry { retryable: true },
                    item: true,
                },
            )
            .await?;
        }
        Ok(())
    }

    /// `lease.denied`: the forwarder refused a request. For two reasons, a model the lease does not allow and a
    /// cap that is reached, the turn fails with that reason and the thread goes on with the next input; the
    /// timeline says what was refused, once. Any other reason is a diagnostic only.
    async fn on_lease_denied(
        &self,
        unit: &mut Unit<'_>,
        provider: &ProviderRow,
        denied: &LeaseDenied,
    ) -> Result<(), EngineError> {
        let Some(lease) = threads::lease(&mut *unit.conn, denied.lease_id).await? else {
            tracing::warn!(lease = %denied.lease_id, "a lease was refused that the system does not know");
            return Ok(());
        };
        if lease.provider_id != provider.id {
            tracing::warn!(lease = %lease.id, "a machine told of a lease that is not its provider's");
            return Ok(());
        }
        let Some(reason) = DENIED_REASONS
            .into_iter()
            .find(|reason| *reason == denied.reason)
        else {
            tracing::debug!(lease = %lease.id, reason = %denied.reason, "a request was refused");
            return Ok(());
        };
        let message = format!(
            "The forwarder refused a request for the model {}: {}.",
            denied.model,
            reason.replace('_', " ")
        );
        let ended = self
            .end_leased_turn(
                unit,
                &lease,
                Ending {
                    state: turn_state::FAILED,
                    code: reason,
                    message: message.clone(),
                    input: InputAfter::Retry { retryable: false },
                    item: true,
                },
            )
            .await?;
        if !ended && let Some(turn) = lease.turn_id {
            // The turn is over already (the harness failed it first): the refusal is still told.
            if let Some(turn) = threads::turn(&mut *unit.conn, turn).await? {
                ingest::add_error_item(
                    unit,
                    lease.thread_id,
                    turn.segment_id,
                    Some(turn.id),
                    reason,
                    &message,
                    false,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Ends the turn a lease is for, if it is under way, in the way `ending` says, and asks its harness to
    /// stop what it does. Says whether it ended it.
    async fn end_leased_turn(
        &self,
        unit: &mut Unit<'_>,
        lease: &LeaseRow,
        ending: Ending,
    ) -> Result<bool, EngineError> {
        let Some(turn_id) = lease.turn_id else {
            return Ok(false);
        };
        let Some(thread) = threads::lock(&mut *unit.conn, lease.thread_id).await? else {
            return Ok(false);
        };
        let Some(turn) = threads::turn(&mut *unit.conn, turn_id).await? else {
            return Ok(false);
        };
        if !turn.is_active() {
            return Ok(false);
        }
        if ending.item {
            ingest::add_error_item(
                unit,
                thread.node_id,
                turn.segment_id,
                Some(turn.id),
                ending.code,
                &ending.message,
                false,
            )
            .await?;
        }
        let ended = self
            .end_turn(
                unit,
                &thread,
                &turn,
                TurnEnd {
                    state: ending.state,
                    error: Some(error_json(ending.code, &ending.message, false)),
                    input: ending.input,
                    then_block: None,
                },
            )
            .await?;
        if ended.changed
            && let Some(machine) = self
                .harness_machine_of(&mut *unit.conn, thread.node_id)
                .await?
        {
            self.send_interrupt_best_effort(machine, thread.node_id, turn.id)
                .await;
        }
        Ok(ended.changed)
    }

    /// `lease.usage`: one model request of a lease, as the forwarder counted it. It becomes one row of the
    /// usage ledger with what the turn ran on, the person and place it is put down to, and the rate it is
    /// counted at, and raises what the turn is known to have used. A request that is reported again (the same
    /// delivery, or the same request named by the upstream) makes no second row.
    ///
    /// On the path `forwarded` the forwarder is the meter: its rows are the counted ones, and a harness's own
    /// totals for the turn are kept as the turn's figures and are not counted again.
    async fn on_lease_usage(
        &self,
        unit: &mut Unit<'_>,
        provider: &ProviderRow,
        ctx: &UsageContext<'_>,
        usage: &LeaseUsage,
    ) -> Result<(), EngineError> {
        let Some(lease) = threads::lease(&mut *unit.conn, usage.lease_id).await? else {
            tracing::warn!(lease = %usage.lease_id, "usage of a lease that the system does not know is not counted");
            return Ok(());
        };
        if lease.provider_id != provider.id {
            tracing::warn!(lease = %lease.id, "a machine told of the usage of a lease that is not its provider's");
            return Ok(());
        }
        // Closing totals and individual reports share the thread lock so a report cannot race
        // between the final sum and its reconciliation marker.
        if threads::lock(&mut *unit.conn, lease.thread_id)
            .await?
            .is_none()
        {
            return Ok(());
        }
        let Some(attribution) = ledger::attribution_of_lease(&mut *unit.conn, lease.id).await?
        else {
            tracing::warn!(lease = %lease.id, "usage of a lease with no turn is not counted");
            return Ok(());
        };
        // What the forwarder counted in all when the lease closed is in the ledger already: a request
        // reported after that was counted there, and is not counted again.
        if ledger::event_exists(&mut *unit.conn, ids::usage_close(lease.id)).await? {
            tracing::info!(lease = %lease.id, "usage reported after its lease was reconciled is not counted again");
            return Ok(());
        }
        let id = match usage
            .request_ref
            .as_deref()
            .filter(|named| !named.is_empty())
        {
            Some(named) => ids::usage_request(lease.id, named),
            None => ids::usage_event(ctx.machine, ctx.stream, ctx.seq),
        };
        self.count_forwarded(
            unit,
            provider,
            &attribution,
            Forwarded {
                timing: None,
                harness: false,
                counted: true,
                id,
                at: ctx.at,
                model: &usage.model,
                tokens: tokens_of(&serde_json::to_value(usage.usage)?),
                request_ref: usage.request_ref.as_deref(),
            },
        )
        .await
    }

    /// Writes one row the forwarder counted, at the rate its model is counted at and the service tier the
    /// turn ran with, and raises what the turn is known to have used where it was new.
    async fn count_forwarded(
        &self,
        unit: &mut Unit<'_>,
        provider: &ProviderRow,
        attribution: &Attribution,
        forwarded: Forwarded<'_>,
    ) -> Result<(), EngineError> {
        let Forwarded {
            timing,
            harness,
            counted,
            id,
            at,
            model,
            tokens,
            request_ref,
        } = forwarded;
        let entry = resources::model(&mut *unit.conn, provider.id, model).await?;
        let own = entry
            .as_ref()
            .and_then(|entry| {
                serde_json::from_value::<ModelParameters>(entry.parameters.clone()).ok()
            })
            .and_then(|parameters| parameters.rate);
        let agent = match attribution.agent_id {
            Some(agent) => resources::agent(&mut *unit.conn, agent).await?,
            None => None,
        };
        let (tier, program) = match agent
            .as_ref()
            .map(crate::harnesses::descriptor_of)
            .transpose()?
        {
            Some(Some(descriptor)) => (
                service_tier(&descriptor.parameters, &attribution.params),
                access_program(&descriptor.parameters, &attribution.params),
            ),
            _ => (None, None),
        };
        let context_tokens = Some(tokens.input);
        let book = RateBook::load(&mut *unit.conn, attribution.thread_id).await?;
        let priced = book
            .price(
                &mut *unit.conn,
                own.as_ref(),
                model,
                &tokens,
                context_tokens,
                tier.as_deref(),
            )
            .await?;
        let rate_json = priced
            .found
            .as_ref()
            .and_then(|found| serde_json::to_value(&found.rate).ok());
        let event = NewEvent {
            id,
            at,
            origin: if harness {
                ledger::origin::HARNESS
            } else {
                ledger::origin::FORWARDER
            },
            scope: ledger::scope::REQUEST,
            counted,
            attribution,
            model,
            tier: tier.as_deref(),
            tokens,
            context_tokens,
            request_ref,
            rate: rate_json.as_ref(),
            rate_source: priced.found.as_ref().map(|found| found.source.word()),
            credits: priced.credits.as_deref(),
        };
        let written =
            ledger::record_program(&mut *unit.conn, &event, timing, program.as_deref()).await?;
        if written {
            if let Some(timing) = timing {
                record_model_metrics(model, attribution.harness_kind.as_deref(), timing);
            }
            self.raise_turn_usage(unit, attribution).await?;
        }
        Ok(())
    }

    /// What a lease used in all, as its forwarder answered `lease.close`, against what the ledger counted
    /// of its turn: where the forwarder counted more (a `lease.usage` that never arrived), the difference
    /// becomes one row of its own, once for the lease however often it is closed, and a `lease.usage` of
    /// the lease that arrives after it is not counted again. A zero remainder still records that cut.
    pub(crate) async fn reconcile_lease_close(
        &self,
        lease: Id,
        provider: &ProviderRow,
        answer: &Json,
    ) -> Result<(), EngineError> {
        let Some(closed) = answer.get("usage").filter(|usage| !usage.is_null()) else {
            return Ok(());
        };
        let total = tokens_of(closed);
        let mut tx = self.inner.db.begin().await?;
        let Some(held) = threads::lease(&mut tx, lease).await? else {
            return Ok(());
        };
        if held.provider_id != provider.id
            || threads::lock(&mut tx, held.thread_id).await?.is_none()
        {
            return Ok(());
        }
        if ledger::event_exists(&mut tx, ids::usage_close(lease)).await? {
            return Ok(());
        }
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            if let Some(attribution) = ledger::attribution_of_lease(&mut *unit.conn, lease).await? {
                let counted =
                    ledger::counted_request_totals(&mut *unit.conn, attribution.turn_id).await?;
                let short = shortfall(&total, &counted);
                let at = unit.now();
                let model = attribution.turn_model.clone();
                self.count_forwarded(
                    &mut unit,
                    provider,
                    &attribution,
                    Forwarded {
                        timing: None,
                        harness: false,
                        counted: true,
                        id: ids::usage_close(lease),
                        at,
                        model: &model,
                        tokens: short,
                        request_ref: Some(CLOSE_REF),
                    },
                )
                .await?;
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// Raises the usage a turn is known to have to what the forwarder counted for it, where that is more.
    async fn raise_turn_usage(
        &self,
        unit: &mut Unit<'_>,
        attribution: &Attribution,
    ) -> Result<(), EngineError> {
        let Some(turn) = threads::turn(&mut *unit.conn, attribution.turn_id).await? else {
            return Ok(());
        };
        let counted = ledger::counted_request_totals(&mut *unit.conn, turn.id).await?;
        let known = turn.usage.as_ref().map(tokens_of).unwrap_or_default();
        let raised = larger(&known, &counted);
        if turn.usage.is_some() && raised == known {
            return Ok(());
        }
        let seq = unit.stamp(attribution.thread_id).await?;
        threads::set_turn_usage(&mut *unit.conn, turn.id, &usage_json(&raised), seq).await?;
        Ok(())
    }
}

impl Engine {
    /// Gives abandoned observations their turn's terminal outcome, preserving only observed times.
    pub(crate) async fn close_running_requests(
        &self,
        conn: &mut orch_db::Conn,
        turn: Id,
        state: &str,
        error: Option<&Json>,
    ) -> Result<(), EngineError> {
        let code = request_outcome(state, error);
        for row in ledger::finish_running(conn, turn, code, error).await? {
            record_model_metrics(&row.model, row.harness_kind.as_deref(), &row.timing());
        }
        Ok(())
    }

    /// Repairs rows left by an older worker or a loss between turn completion and request cleanup.
    pub(crate) async fn sweep_running_requests(&self, limit: i64) -> Result<(), EngineError> {
        let mut tx = self.db().begin().await?;
        for (turn, state, error) in ledger::stale_running_turns(&mut tx, limit).await? {
            self.close_running_requests(
                &mut tx,
                turn,
                state.as_deref().unwrap_or("failed"),
                error.as_ref(),
            )
            .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// Keeps adapter measurements for every provider kind; only non-forwarded requests add billed tokens.
    pub(crate) async fn count_adapter_requests(
        &self,
        unit: &mut Unit<'_>,
        turn_id: Id,
        requests: &[orch_agent_contract::requests::UsageRequest],
    ) -> Result<(), EngineError> {
        if requests.is_empty() {
            return Ok(());
        }
        let Some(turn) = threads::turn(&mut *unit.conn, turn_id).await? else {
            return Ok(());
        };
        let Some(attribution) =
            threads::binding::attribution_of_turn(&mut *unit.conn, turn_id).await?
        else {
            return Ok(());
        };
        let Some(provider) = resources::provider(&mut *unit.conn, attribution.provider_id).await?
        else {
            return Ok(());
        };
        for observed in requests {
            let mut request = observed.clone();
            if request.outcome.is_none() && !turn.is_active() {
                request.finish(
                    request_outcome(&turn.state, turn.error.as_ref()),
                    turn.error
                        .as_ref()
                        .and_then(|error| error.get("message"))
                        .and_then(Json::as_str),
                    turn.error
                        .as_ref()
                        .and_then(|error| error.get("said"))
                        .and_then(Json::as_str),
                );
            }
            // Durable reports may arrive after reconnect. Their own observation time determines
            // the daily and health windows, bounded by the owning turn, never by replay time.
            let lower = turn.started_at.unwrap_or(turn.created_at);
            let upper = turn.completed_at.unwrap_or_else(|| unit.now()).max(lower);
            let at = request
                .end
                .or(request.asked)
                .map(|at| at.get())
                .unwrap_or_else(|| unit.now())
                .clamp(lower, upper);
            let timing = request.measured();
            self.count_forwarded(
                unit,
                &provider,
                &attribution,
                Forwarded {
                    id: ids::usage_request(turn_id, &request.id),
                    at,
                    model: request.model.as_deref().unwrap_or(&attribution.turn_model),
                    tokens: tokens_of(&serde_json::to_value(request.tokens.usage())?),
                    request_ref: Some(&request.id),
                    timing: Some(&timing),
                    harness: true,
                    counted: turn.lease_id.is_none(),
                },
            )
            .await?;
        }
        Ok(())
    }

    /// A repaired profile declaration retries only its original turns' pending terminal work.
    /// Current thread bindings can already name a different profile; accounting still uses the segment.
    pub async fn wake_agent_terminal_work(&self, agent: Id) -> Result<usize, EngineError> {
        let mut after: Option<Id> = None;
        let mut count = 0_usize;
        loop {
            let threads: Vec<Id> = {
                let mut conn = self.db().acquire().await?;
                sqlx::query_scalar(
                    "SELECT t.thread_id FROM turns t JOIN segments s ON s.id=t.segment_id
                    WHERE s.agent_id=$1 AND t.end_work IS NOT NULL
                      AND ($2::uuid IS NULL OR t.thread_id>$2)
                    GROUP BY t.thread_id ORDER BY t.thread_id LIMIT $3",
                )
                .bind(agent)
                .bind(after)
                .bind(self.config().sweep_batch)
                .fetch_all(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?
            };
            let Some(last) = threads.last().copied() else {
                return Ok(count);
            };
            count = count.saturating_add(threads.len());
            // Do not hold a connection or transaction while scheduling the ordinary per-thread worker.
            for thread in threads {
                self.wake(thread);
            }
            after = Some(last);
        }
    }

    /// Counts a turn that went through no forwarder from what its harness reported. A harness that is
    /// signed in on its own machine (the path `native`) calls its model itself, so that no lease was
    /// opened and no request was counted on the way; the totals it reports are the only meter there is,
    /// and they are put down once, when the turn ends, with the model the turn ran. A turn that ran under
    /// a lease is counted by the forwarder, and its harness's totals stay the turn's figures only.
    pub(crate) async fn count_harness_usage(
        &self,
        unit: &mut Unit<'_>,
        turn: &threads::TurnRow,
    ) -> Result<(), EngineError> {
        let turn_id = turn.id;
        let has_requests = ledger::has_harness_requests(&mut *unit.conn, turn_id).await?;
        if turn.lease_id.is_some() {
            return Ok(());
        }
        // A missing report supplies no consumption to count. Measured request rows are
        // recorded separately; a terminal outcome alone must not invent a known zero.
        let Some(usage) = turn.usage.as_ref() else {
            return Ok(());
        };
        let mut tokens = tokens_of(usage);
        if has_requests {
            let counted = ledger::counted_request_totals(&mut *unit.conn, turn_id).await?;
            tokens = shortfall(&tokens, &counted);
            if tokens.total == 0 {
                return Ok(());
            }
        }
        let Some(attribution) =
            threads::binding::attribution_of_turn(&mut *unit.conn, turn_id).await?
        else {
            return Ok(());
        };
        let entry = resources::model(
            &mut *unit.conn,
            attribution.provider_id,
            &attribution.turn_model,
        )
        .await?;
        let own = entry
            .as_ref()
            .and_then(|entry| {
                serde_json::from_value::<ModelParameters>(entry.parameters.clone()).ok()
            })
            .and_then(|parameters| parameters.rate);
        let agent = match attribution.agent_id {
            Some(agent) => resources::agent(&mut *unit.conn, agent).await?,
            None => None,
        };
        let (tier, program) = match agent
            .as_ref()
            .map(crate::harnesses::descriptor_of)
            .transpose()?
        {
            Some(Some(descriptor)) => (
                service_tier(&descriptor.parameters, &attribution.params),
                access_program(&descriptor.parameters, &attribution.params),
            ),
            _ => (None, None),
        };
        let at = turn.completed_at.unwrap_or_else(|| unit.now());
        let context_tokens = Some(tokens.input);
        let book = RateBook::load(&mut *unit.conn, attribution.thread_id).await?;
        let priced = book
            .price(
                &mut *unit.conn,
                own.as_ref(),
                &attribution.turn_model,
                &tokens,
                context_tokens,
                tier.as_deref(),
            )
            .await?;
        let rate_json = priced
            .found
            .as_ref()
            .and_then(|found| serde_json::to_value(&found.rate).ok());
        let outcome = turn
            .error
            .as_ref()
            .and_then(|e| e.get("code"))
            .and_then(Json::as_str)
            .unwrap_or(if turn.state == "interrupted" {
                "forced"
            } else if turn.state == "failed" {
                "harness_error"
            } else {
                "done"
            });
        let timing = orch_api_contract::usage::RequestTiming {
            request_ms: turn
                .usage_detail
                .as_ref()
                .and_then(|d| d.get("api_time_ms"))
                .and_then(Json::as_i64)
                .map_or(
                    orch_api_contract::usage::UsageMeasurement::Unsupported,
                    |value| orch_api_contract::usage::UsageMeasurement::Told { value },
                ),
            outcome: if has_requests {
                orch_api_contract::usage::UsageMeasurement::Unsupported
            } else {
                orch_api_contract::usage::UsageMeasurement::Told {
                    value: outcome.to_owned(),
                }
            },
            error_message: turn
                .error
                .as_ref()
                .and_then(|e| e.get("message"))
                .and_then(Json::as_str)
                .map(str::to_owned),
            ..orch_api_contract::usage::RequestTiming::default()
        };
        let written = ledger::record_program(
            &mut *unit.conn,
            &NewEvent {
                id: ids::harness_usage(turn_id),
                at,
                origin: ledger::origin::HARNESS,
                scope: ledger::scope::TURN,
                counted: true,
                attribution: &attribution,
                model: &attribution.turn_model,
                tier: tier.as_deref(),
                tokens,
                context_tokens,
                request_ref: None,
                rate: rate_json.as_ref(),
                rate_source: priced.found.as_ref().map(|found| found.source.word()),
                credits: priced.credits.as_deref(),
            },
            Some(&timing),
            program.as_deref(),
        )
        .await?;
        if written {
            record_model_metrics(
                &attribution.turn_model,
                attribution.harness_kind.as_deref(),
                &timing,
            );
        }
        Ok(())
    }
}

fn request_outcome<'a>(state: &str, error: Option<&'a Json>) -> &'a str {
    if state == "completed" {
        "done"
    } else if state == "interrupted" {
        "forced"
    } else {
        error
            .and_then(|error| error.get("code"))
            .and_then(Json::as_str)
            .filter(|code| orch_agent_contract::requests::normalized_code(code))
            .unwrap_or("session_lost")
    }
}

/// Records only new terminal observations, with exactly the same sample eligibility as stored usage.
pub(crate) fn record_model_metrics(
    model: &str,
    harness: Option<&str>,
    timing: &orch_api_contract::usage::RequestTiming,
) {
    let Some(code) = timing.outcome.value() else {
        return;
    };
    let outcome = if code == "done" || orch_agent_contract::requests::normalized_code(code) {
        code.as_str()
    } else {
        "harness_error"
    };
    let model = model.to_owned();
    let harness = harness.unwrap_or("orch").to_owned();
    metrics::counter!("orch_model_requests_total","model"=>model.clone(),"harness"=>harness.clone(),"outcome"=>outcome.to_owned()).increment(1);
    if let Some(ms) = timing.first_token_ms.value() {
        metrics::histogram!("orch_model_first_token_seconds","model"=>model.clone(),"harness"=>harness.clone()).record(*ms as f64/1000.0);
    }
    if let Some(speed) = timing.speed_milli.value() {
        metrics::histogram!("orch_model_output_tokens_per_second","model"=>model,"harness"=>harness).record(*speed as f64/1000.0);
    }
}

/// Where an event of a provider's stream came from, which names a request that the upstream did not name.
struct UsageContext<'a> {
    machine: Id,
    stream: &'a str,
    seq: i64,
    at: chrono::DateTime<chrono::Utc>,
}

/// How a turn that runs under a lease is ended by something that happened to its provider.
struct Ending {
    state: &'static str,
    code: &'static str,
    message: String,
    input: InputAfter,
    item: bool,
}

// ---------------------------------------------------------------- keeping providers

impl Engine {
    /// What the supervisor does for providers at each look: moves the status of the ones whose machine came
    /// or went, stops the ones that were switched off, and closes the leases that no turn holds. Each part
    /// is its own, so that the failure of one does not hold the others back.
    pub(crate) async fn keep_providers(&self) {
        if let Err(error) = self.sync_provider_presence().await {
            tracing::warn!(%error, "the presence of providers could not be looked at; the next sweep looks again");
        }
        if let Err(error) = self.stop_switched_off_providers().await {
            tracing::warn!(%error, "providers that were switched off could not be stopped; the next sweep looks again");
        }
        if let Err(error) = self.close_stale_leases().await {
            tracing::warn!(%error, "leases that no turn holds could not be closed; the next sweep looks again");
        }
        if let Err(error) = self.release_ended_limits().await {
            tracing::warn!(%error, "providers whose limit has ended could not be looked at; the next sweep looks again");
        }
    }

    /// A provider closed by a limit of the plan whose time has passed is looked at again: it is `ready`, and the
    /// threads that were blocked for want of it start where the vendor decides, which a turn finds out.
    async fn release_ended_limits(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let ended: Vec<Id> = {
            let mut conn = self.inner.db.acquire().await?;
            keeper::exhausted_with_a_time(&mut conn, self.inner.config.sweep_batch).await?
        }
        .into_iter()
        .filter(|(_, facts)| limit_resets_at(facts).is_some_and(|ends| ends <= now))
        .map(|(id, _)| id)
        .collect();
        for provider in ended {
            let released = {
                let mut conn = self.inner.db.acquire().await?;
                match resources::provider(&mut conn, provider).await? {
                    Some(row) => release_if_ended(&mut conn, &row, now).await?.is_some(),
                    None => false,
                }
            };
            if released {
                self.reevaluate_provider(provider).await?;
            }
        }
        Ok(())
    }

    /// The provider a limit of the plan has closed, because the turn that ran on it ended with `failure`: it
    /// is `exhausted` with the sentence that says which limit and until when, and its facts take the limit
    /// with its time, which is what pre-flight, the sweep and the thread's limits read.
    pub(crate) async fn close_provider_by_limit(
        &self,
        unit: &mut Unit<'_>,
        provider: Id,
        failure: &TurnError,
    ) -> Result<(), EngineError> {
        let now = unit.now();
        let Some(row) = resources::provider(&mut *unit.conn, provider).await? else {
            return Ok(());
        };
        // What the machine reported stays as it is, with the members the contract requires: a record without
        // `windows` does not read, and the thread would then be told nothing of which limit it waits for.
        let mut facts = if row.facts.is_object() {
            row.facts.clone()
        } else {
            json!({})
        };
        if facts.get("windows").is_none_or(Json::is_null) {
            facts["windows"] = json!([]);
        }
        // A failure that names no moment does not take away one that is known and has not come: turns that
        // were running when the limit was reached do not all end with the same words.
        let still_standing = limit_resets_at(&row.facts).filter(|ends| *ends > now);
        let resets_at = failure.resets_at.or(still_standing.map(Time::new));
        facts["limit_reached"] = json!({
            "type": limit_name(failure),
            "resets_at": resets_at,
        });
        keeper::set_facts(&mut *unit.conn, provider, &facts, now).await?;
        keeper::move_status(
            &mut *unit.conn,
            provider,
            &["ready", "exhausted", "offline"],
            "exhausted",
            Some(&failure.message),
            now,
        )
        .await?;
        for thread in threads::blocked_threads_of_provider(&mut *unit.conn, provider, None).await? {
            unit.wake(thread);
        }
        Ok(())
    }

    /// An API provider whose keeping machine is away is `offline`, and is `ready` again when the machine is
    /// there; the threads that were blocked for want of it are looked at again.
    async fn sync_provider_presence(&self) -> Result<(), EngineError> {
        let moves = {
            let mut conn = self.inner.db.acquire().await?;
            keeper::sync_presence(&mut conn, self.inner.clock.now()).await?
        };
        for provider in moves.back {
            self.reevaluate_provider(provider).await?;
        }
        Ok(())
    }

    /// Stops the providers that were switched off or deleted, on the machine that keeps them, once for each
    /// act: the leases they have open are closed, a turn that was running under one is interrupted and its
    /// thread blocked, and the machine is told to stop.
    async fn stop_switched_off_providers(&self) -> Result<(), EngineError> {
        let since = crate::config::decisions_since(self.inner.clock.now());
        let switched = {
            let mut conn = self.inner.db.acquire().await?;
            keeper::switched_off_since(&mut conn, since, self.inner.config.sweep_batch).await?
        };
        for off in switched {
            let Some(machine) = off.machine_id else {
                continue;
            };
            let id = ids::provider_stop(off.id, machine, off.revision);
            if self.inner.commands.status(id).await?.is_some() {
                continue;
            }
            self.close_leases_of_switched_off(off.id).await?;
            let command = Outgoing::new(
                id,
                machine,
                SerialKey::Provider(off.id),
                &ProviderStop {
                    provider_id: off.id,

                    purge: None,
                },
            )?;
            self.inner.commands.issue(command).await?;
        }
        Ok(())
    }

    /// Closes the open leases of a provider that was switched off. A turn that runs under one is ended as
    /// interrupted by the system, its harness is asked to stop, and what its thread has waiting is blocked
    /// by the provider's state, as pre-flight finds it.
    async fn close_leases_of_switched_off(&self, provider: Id) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            for lease in keeper::open_leases_of(&mut *unit.conn, provider).await? {
                // The lease is closed for the reason first, so that the end of its turn does not close it
                // for another.
                let now = unit.now();
                threads::close_lease(&mut *unit.conn, lease.id, DISABLED, now).await?;
                self.end_leased_turn(
                    &mut unit,
                    &lease,
                    Ending {
                        state: turn_state::INTERRUPTED,
                        code: "provider_unavailable",
                        message: "The provider was switched off.".to_owned(),
                        input: InputAfter::Retry { retryable: true },
                        item: true,
                    },
                )
                .await?;
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------- the limit of a plan

/// How many characters of the vendor's words name a limit in the provider's facts.
const LIMIT_NAME_CHARS: usize = 200;

/// The name of the limit that closed a provider, as the vendor said it: the first line of its words, else the
/// adapter's own sentence.
fn limit_name(failure: &TurnError) -> String {
    failure
        .said
        .as_deref()
        .unwrap_or(&failure.message)
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(LIMIT_NAME_CHARS)
        .collect()
}

/// When the limit that a provider's facts name ends, if they name a time.
fn limit_resets_at(facts: &Json) -> Option<DateTime<Utc>> {
    facts
        .pointer("/limit_reached/resets_at")?
        .as_str()?
        .parse::<Time>()
        .ok()
        .map(Time::get)
}

/// The facts a machine reports while the provider stands closed by a limit that has not ended: where they do
/// not name a limit of their own, the one that closed it stays, so that a report that does not know of it does
/// not open the provider before the vendor's time.
fn keep_standing_limit(status: &str, stored: &Json, mut facts: Json, now: DateTime<Utc>) -> Json {
    if status != "exhausted"
        || facts.get("ordinary_usage_allowed").and_then(Json::as_bool) == Some(true)
    {
        return facts;
    }
    let reports_one = facts
        .get("limit_reached")
        .is_some_and(|limit| !limit.is_null());
    let standing = stored.get("limit_reached").filter(|limit| !limit.is_null());
    let still_stands = limit_resets_at(stored).is_none_or(|ends| ends > now);
    if !reports_one
        && still_stands
        && let (Some(standing), Some(members)) = (standing, facts.as_object_mut())
    {
        members.insert("limit_reached".to_owned(), standing.clone());
    }
    facts
}

/// Looks again at a provider that a limit with a time closed: when the time has passed it is `ready`, and its
/// facts no longer name the limit. Gives the provider as it is then, or nothing when it stays as it was.
pub(crate) async fn release_if_ended(
    conn: &mut orch_db::Conn,
    row: &ProviderRow,
    now: DateTime<Utc>,
) -> Result<Option<ProviderRow>, EngineError> {
    if row.status != "exhausted" || !limit_resets_at(&row.facts).is_some_and(|ends| ends <= now) {
        return Ok(None);
    }
    let observed: orch_agent_contract::facts::ProviderFacts =
        serde_json::from_value(row.facts.clone())?;
    if crate::provider_fact_reads::admission_exhausted(&observed, Time::new(now)) {
        return Ok(None);
    }
    let mut facts = row.facts.clone();
    facts["limit_reached"] = Json::Null;
    if facts.get("ordinary_usage_allowed").and_then(Json::as_bool) == Some(false) {
        facts["ordinary_usage_allowed"] = Json::Null;
    }
    keeper::set_facts(conn, row.id, &facts, now).await?;
    keeper::move_status(conn, row.id, &["exhausted"], "ready", None, now).await?;
    Ok(resources::provider(conn, row.id).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use orch_types::new_id;
    use serde_json::json;

    fn row(key_machine: Option<Id>, machine: Option<Id>) -> StartRow {
        StartRow {
            id: new_id(),
            name: "Team account".to_owned(),
            sources: json!({}),
            kind: "api".to_owned(),
            upstreams: json!([{
                "protocol": "openai-responses", "base_url": "https://api.example.com/v1",
                "options": { "auth": "bearer" }
            }]),
            protocols: vec!["openai-responses".to_owned()],
            placement: "connected".to_owned(),
            machine_id: machine,
            key_machine_id: key_machine,
            status: "ready".to_owned(),
            revision: 1,
            deleted_at: None,
        }
    }

    fn secret(name: &str) -> SecretRef {
        SecretRef {
            id: new_id(),
            name: name.to_owned(),
            version: Utc::now(),
        }
    }

    fn at(text: &str) -> DateTime<Utc> {
        text.parse::<Time>().unwrap().get()
    }

    fn weekly(ends: Option<&str>) -> TurnError {
        TurnError {
            reason: None,
            status: None,
            retry_after_secs: None,
            code: "provider_exhausted".to_owned(),
            message: "The weekly limit of the plan is reached.".to_owned(),
            retryable: false,
            said: Some(
                "You've hit your weekly limit · resets Oct 14, 5pm (Asia/Singapore)\n(request id 1)"
                    .to_owned(),
            ),
            resets_at: ends.map(|text| text.parse().unwrap()),
            window: None,
            incomplete: false,
            limit: None,
        }
    }

    #[test]
    fn a_limit_is_named_by_the_first_line_of_the_vendors_words_and_else_by_the_adapters_sentence() {
        assert_eq!(
            limit_name(&weekly(None)),
            "You've hit your weekly limit · resets Oct 14, 5pm (Asia/Singapore)"
        );
        let mut unsaid = weekly(None);
        unsaid.said = None;
        assert_eq!(
            limit_name(&unsaid),
            "The weekly limit of the plan is reached."
        );
        let mut long = weekly(None);
        long.said = Some("x".repeat(LIMIT_NAME_CHARS * 3));
        assert_eq!(limit_name(&long).chars().count(), LIMIT_NAME_CHARS);
    }

    #[test]
    fn the_time_of_a_limit_is_read_from_the_facts_only_when_they_name_one() {
        let named =
            json!({ "limit_reached": { "type": "x", "resets_at": "2026-10-14T09:00:00.000Z" } });
        assert_eq!(
            limit_resets_at(&named),
            Some(at("2026-10-14T09:00:00.000Z"))
        );
        assert_eq!(
            limit_resets_at(&json!({ "limit_reached": { "type": "x", "resets_at": null } })),
            None
        );
        assert_eq!(limit_resets_at(&json!({ "limit_reached": null })), None);
        assert_eq!(limit_resets_at(&json!({})), None);
        assert_eq!(
            limit_resets_at(&json!({ "limit_reached": { "resets_at": "tomorrow" } })),
            None
        );
    }

    #[test]
    fn a_report_that_does_not_name_the_limit_that_closed_the_provider_leaves_it_standing_until_its_time()
     {
        let stored = json!({ "plan": "max", "limit_reached": { "type": "weekly", "resets_at": "2026-10-14T09:00:00.000Z" } });
        let silent = json!({ "plan": "max", "windows": [], "limit_reached": null });
        let before = at("2026-10-13T00:00:00.000Z");
        let kept = keep_standing_limit("exhausted", &stored, silent.clone(), before);
        assert_eq!(kept["limit_reached"], stored["limit_reached"]);
        assert_eq!(
            kept["plan"], "max",
            "the rest of the report is taken as it came"
        );

        // The time has passed: the report is taken as it is.
        let after = at("2026-10-14T09:00:01.000Z");
        assert_eq!(
            keep_standing_limit("exhausted", &stored, silent.clone(), after),
            silent
        );
        // A provider that no limit closed takes every report as it is.
        assert_eq!(
            keep_standing_limit("ready", &stored, silent.clone(), before),
            silent
        );
        // A report that names a limit of its own is not overwritten.
        let own = json!({ "limit_reached": { "type": "seven_day", "resets_at": null } });
        assert_eq!(
            keep_standing_limit("exhausted", &stored, own.clone(), before),
            own
        );
        // A limit without a time stands for ever, until the machine says the provider is ready.
        let timeless = json!({ "limit_reached": { "type": "weekly", "resets_at": null } });
        let kept = keep_standing_limit("exhausted", &timeless, silent, after);
        assert_eq!(kept["limit_reached"], timeless["limit_reached"]);
    }

    #[test]
    fn a_key_the_server_holds_travels_as_a_reference_to_its_secret_and_never_as_text() {
        let machine = new_id();
        let key = secret("api_key");
        let header = secret("header:x-gateway-token");
        let plan = plan_start(
            &row(Some(machine), Some(machine)),
            &[key.clone(), header.clone()],
        )
        .unwrap();
        assert_eq!(plan.machine, machine);
        assert_eq!(plan.body.credential, Some(Secret::Ref(key.id)));
        assert_eq!(
            plan.body.secrets.get("x-gateway-token"),
            Some(&Secret::Ref(header.id))
        );
        let sent = serde_json::to_string(&plan.body).unwrap();
        assert!(sent.contains("$secret"), "{sent}");
    }

    #[test]
    fn a_key_kept_on_a_machine_is_not_sent_at_all_and_the_provider_starts_there() {
        let (keeper, runtime) = (new_id(), new_id());
        let plan = plan_start(&row(Some(keeper), Some(runtime)), &[]).unwrap();
        assert_eq!(plan.machine, keeper);
        assert_eq!(plan.body.credential, None);
    }

    #[test]
    fn what_a_start_carries_decides_whether_it_is_sent_again() {
        let machine = new_id();
        let provider = row(Some(machine), Some(machine));
        let key = secret("api_key");
        let first = plan_start(&provider, std::slice::from_ref(&key)).unwrap();
        let same = plan_start(&provider, std::slice::from_ref(&key)).unwrap();
        assert_eq!(first.digest, same.digest);
        let mut rotated = key.clone();
        rotated.version += chrono::Duration::seconds(1);
        assert_ne!(
            first.digest,
            plan_start(&provider, &[rotated]).unwrap().digest,
            "a key that was written again starts the provider anew"
        );
        let mut edited = provider.clone();
        edited.upstreams = json!([{ "protocol": "openai-responses", "base_url": "https://other.example.com/v1", "options": {} }]);
        assert_ne!(first.digest, plan_start(&edited, &[key]).unwrap().digest);
    }

    #[test]
    fn the_presenters_choice_and_secret_version_restart_the_provider_but_read_metadata_does_not() {
        let machine = new_id();
        let mut provider = row(Some(machine), Some(machine));
        let field = secret("source:presenter:field:exchange");
        provider.sources = json!({"presenter":{"by":"program","name":"exchange","config":{"region":"east"},"secrets":["exchange"]}});
        let first = plan_start(&provider, std::slice::from_ref(&field)).unwrap();
        let selected = first.body.presenter.as_ref().unwrap();
        assert_eq!(selected.name.as_deref(), Some("exchange"));
        assert_eq!(selected.provider.as_ref().unwrap().name, "Team account");
        assert_eq!(
            selected.secrets["exchange"],
            orch_provider_contract::SourceSecret::Ref(field.id)
        );
        provider.sources["presenter"]["last_error"] =
            json!({"code":"unavailable","message":"Retry later","at":"2026-10-13T00:00:00Z"});
        assert_eq!(
            plan_start(&provider, std::slice::from_ref(&field))
                .unwrap()
                .digest,
            first.digest
        );
        provider.sources["presenter"]["config"]["region"] = json!("west");
        assert_ne!(
            plan_start(&provider, std::slice::from_ref(&field))
                .unwrap()
                .digest,
            first.digest
        );
        provider.sources["presenter"]["config"]["region"] = json!("east");
        let mut rotated = field.clone();
        rotated.version += chrono::Duration::seconds(1);
        assert_ne!(
            plan_start(&provider, &[rotated]).unwrap().digest,
            first.digest
        );
    }

    #[test]
    fn a_provider_that_is_no_key_or_has_no_machine_has_nothing_to_start() {
        let mut subscription = row(None, Some(new_id()));
        subscription.kind = "subscription".to_owned();
        assert_eq!(plan_start(&subscription, &[]), Err(PlanError::NotKeyed));
        assert_eq!(plan_start(&row(None, None), &[]), Err(PlanError::NoMachine));
        assert_eq!(
            plan_start(&row(None, Some(new_id())), &[]),
            Err(PlanError::NoMachine),
            "a runtime is never guessed to be a credential keeper"
        );
        let machine = new_id();
        let mut broken = row(Some(machine), Some(machine));
        broken.upstreams = json!({ "not": "a list" });
        assert!(matches!(
            plan_start(&broken, &[]),
            Err(PlanError::Upstreams(_))
        ));
    }

    #[test]
    fn tokens_are_read_leniently_and_a_part_never_exceeds_its_whole() {
        let tokens = tokens_of(&json!({
            "input_tokens": 100, "cached_input_tokens": 400, "output_tokens": 20,
            "reasoning_tokens": 5, "total_tokens": 120
        }));
        assert_eq!(
            (
                tokens.input,
                tokens.cached_input,
                tokens.output,
                tokens.reasoning,
                tokens.total
            ),
            (100, 100, 20, 5, 120)
        );
        let bare = tokens_of(&json!({ "input_tokens": 3, "output_tokens": 4 }));
        assert_eq!(
            bare.total, 7,
            "a total that is not given is input plus output"
        );
        assert_eq!(tokens_of(&json!(null)), Tokens::default());
    }

    fn parameter(key: &str, options: Option<ParameterOptions>) -> ParameterDef {
        use orch_agent_contract::parameters::{
            ParameterApply, ParameterDefault, ParameterGroup, ParameterScope, ParameterType,
        };
        ParameterDef {
            key: key.to_owned(),
            title: key.to_owned(),
            description: String::new(),
            group: ParameterGroup::Model,
            kind: ParameterType::Enum,
            options,
            range: None,
            default: ParameterDefault::Value(Json::Null),
            scopes: vec![ParameterScope::Thread],
            apply: ParameterApply::NextTurn,
            eligible_when: None,
            feature: None,
        }
    }

    #[test]
    fn the_service_tier_of_a_turn_is_the_value_of_the_parameter_whose_options_are_the_models_tiers()
    {
        let parameters = vec![
            parameter("reasoning_summary", None),
            parameter(
                "service_tier",
                Some(ParameterOptions::FromModel {
                    from_model: ModelList::ServiceTiers,
                }),
            ),
        ];
        assert_eq!(
            service_tier(
                &parameters,
                &json!({ "service_tier": "fast", "reasoning_summary": "auto" })
            ),
            Some("fast".to_owned())
        );
        assert_eq!(
            service_tier(&parameters, &json!({ "reasoning_summary": "auto" })),
            None
        );
        assert_eq!(
            service_tier(&parameters[..1], &json!({ "service_tier": "fast" })),
            None
        );
    }

    #[test]
    fn access_programmes_use_an_external_adapters_declared_key_too() {
        let parameters = vec![parameter(
            "my_programme",
            Some(ParameterOptions::FromModel {
                from_model: ModelList::AccessPrograms,
            }),
        )];
        assert_eq!(
            access_program(
                &parameters,
                &json!({"my_programme":"research","cyber_access_program":"other"})
            ),
            Some("research".to_owned())
        );
        assert_eq!(
            access_program(&parameters, &json!({"cyber_access_program":"research"})),
            None
        );
    }

    #[test]
    fn the_larger_of_two_usages_is_taken_part_by_part() {
        let a = tokens_of(&json!({ "input_tokens": 10, "output_tokens": 50, "total_tokens": 60 }));
        let b = tokens_of(&json!({ "input_tokens": 30, "output_tokens": 20, "total_tokens": 50 }));
        let both = larger(&a, &b);
        assert_eq!((both.input, both.output, both.total), (30, 50, 60));
    }

    #[test]
    fn what_a_machine_that_cannot_serve_the_provider_says_blocks_the_binding_and_a_machine_that_is_away_only_waits()
     {
        use orch_link_contract::commands::CommandError;
        let cannot = Outcome::Failed(CommandError::new(
            ErrorCode::Unsupported,
            "no such protocol",
        ));
        let failure = classify_keeper("provider.start", &cannot);
        assert_eq!(failure.blocked.reason, "binding_invalid");
        assert!(!failure.retryable);
        let forbidden = Outcome::Failed(CommandError::new(
            ErrorCode::PolicyUpstreamNotAllowed,
            "model calls are not to be forwarded there",
        ));
        let by_policy = classify_keeper("provider.start", &forbidden);
        assert_eq!(by_policy.blocked.reason, "binding_invalid");
        assert!(by_policy.message.contains("not to be forwarded"));
        let away = Outcome::Failed(CommandError::new(ErrorCode::OfflinePeer, "away"));
        let waits = classify_keeper("lease.open", &away);
        assert_eq!(waits.blocked.reason, "provider_unavailable");
        assert!(waits.retryable);
        assert_eq!(
            classify_keeper("lease.open", &Outcome::Expired)
                .blocked
                .reason,
            "provider_unavailable"
        );
        assert_eq!(
            classify_plan(&PlanError::NoMachine).blocked.reason,
            "provider_unavailable"
        );
    }
}
