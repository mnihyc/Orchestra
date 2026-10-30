//! Arrangements by type: what an adapter, a provider and an environment make possible; what the provider's owner, the entry, policy and the machine allow; the executor, and the tool set that the environment's offers come to for the harness, taken at each session; default binding and fallbacks.
//!
//! The rule that chooses an arrangement is the agent contract's
//! ([`orch_agent_contract::arrangement::choose`]): what is available from
//! the adapter's declaration and the facts, in the adapter's order with the
//! preferred types first, and what is permitted. This module reads the facts
//! out of the database for one thread and hands them to that rule, so that
//! creating a thread, and every pre-flight after it, ask the same question
//! the same way.
//!
//! The machine that keeps a credential on a person's machine has a list of
//! its own that narrows the types (`provider.arrangements` of its local
//! policy, as it last reported it); [`choose_with_machine`] and [`options`]
//! ask it as well, and the machine enforces it again whatever the server
//! asks. Harness placement is checked against the reported roles, local
//! policy, installed programs and successful probes on each actual machine.
//! A `separated` entry is also judged on the executors the environment reports.
//!
//! What may be chosen is told by [`options`]: every arrangement the adapter
//! declares for one profile on one provider in one environment, with whether
//! it can be done, whether it is allowed and whose decision it is when not.
//! [`Engine::bind_thread`] and [`Engine::place_thread`] put a thread that has
//! no history yet on another profile, provider or environment, asking the
//! caller then and never again.

use crate::access::{Answer, ProviderUse};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::harnesses;
use crate::leases;
use crate::provider_setup::Learning;
use crate::resolved::{self, Resolved};
use crate::unit::Unit;
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::{Decision, MissingAccess, ObjectRef, Refusal};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_agent_contract::arrangement::{
    self, Arrangement, CredentialLocation, Incompatible, Metering, ModelEnforcement, PlaceSide,
    Policy, ProviderSide, Serves,
};
use orch_agent_contract::credentials::{CredentialForm, CredentialPath, Custody, usable_forms};
use orch_agent_contract::descriptor::{
    AgentDescriptor, ApprovalMode, ArrangementType, ProviderKind,
};
use orch_agent_contract::facts::EnvironmentFacts;
use orch_authz::{Act, Caller, Principal, nodes};
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::environments::thread_engine::{self as environments, EnvironmentRow};
use orch_db::queries::resources::thread_engine::{
    self as resources, AgentRow, MachinePlace, ModelRow, ProviderRow,
};
use orch_db::queries::threads::binding::{EnvironmentUse, SegmentBinding};
use orch_db::queries::threads::{self, NewSegment, thread_state};
use orch_settings::{Place, Snapshot, Target, Wanted};
use orch_settings_contract::registry::{self, keys};
use orch_types::{Id, Json, Seq, Time, new_id};
use serde_json::json;

/// Keeps an offered preference, otherwise uses the program's first ordinary mode.
pub(crate) fn offered_approval_mode(
    descriptor: &AgentDescriptor,
    preferred: ApprovalMode,
) -> Result<ApprovalMode, &'static str> {
    if descriptor.approval_modes.contains(&preferred) {
        return Ok(preferred);
    }
    descriptor
        .approval_modes
        .iter()
        .copied()
        .find(|mode| *mode != ApprovalMode::Auto)
        .ok_or(crate::sessions::APPROVAL_UNSUPPORTED)
}

/// The longest name a thread may be given: what a node's name allows.
pub const NAME_MAX_CHARS: usize = 120;

/// How long a harness a machine could not run waits before it is tried again: ten minutes, so that a machine
/// that reports itself every few seconds is not asked to start it at each report (and its commands do not pile
/// up), while a harness installed meanwhile is found within ten minutes.
pub const DISCOVERY_RETRY_AFTER: chrono::Duration = chrono::Duration::minutes(10);

/// What a discovery that did not come up says on the harness's row: the machine's own words where it
/// answered (`this machine lists no program for codex`), else what became of the command.
fn discovery_detail(outcome: &crate::commands::Outcome) -> String {
    match outcome {
        crate::commands::Outcome::Failed(error) if !error.detail.is_empty() => error.detail.clone(),
        crate::commands::Outcome::Failed(error) => error.code.clone(),
        crate::commands::Outcome::Expired => {
            "the machine did not take the start in time".to_owned()
        }
        crate::commands::Outcome::Cancelled => "the start was cancelled".to_owned(),
        crate::commands::Outcome::Succeeded(_) => {
            "the harness did not report what it offers".to_owned()
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(word: &str) -> Option<T> {
    serde_json::from_value(Json::String(word.to_owned())).ok()
}

/// The arrangement types a provider's owner allows.
#[must_use]
pub fn allowed_by_provider(provider: &ProviderRow) -> Vec<ArrangementType> {
    provider
        .arrangements
        .iter()
        .filter_map(|word| parse(word))
        .collect()
}

/// The kind of a provider, as the agent contract has it.
#[must_use]
pub fn provider_kind(provider: &ProviderRow) -> ProviderKind {
    if provider.kind == "subscription" {
        ProviderKind::Subscription
    } else {
        ProviderKind::Api
    }
}

/// The access of an arrangement type, where only its type and the current declarations are held.
#[must_use]
pub fn credential_path(
    arrangement: ArrangementType,
    provider: &ProviderRow,
    agent: &AgentRow,
) -> CredentialPath {
    let native = harnesses::descriptor_of(agent)
        .ok()
        .flatten()
        .is_some_and(|descriptor| arrangement::native_matches(&descriptor, &serves(provider)));
    arrangement::credential_path(
        arrangement,
        native,
        native_login_here(arrangement, provider, Some(agent.id)),
    )
}

/// The access actually selected, including fresh native declarations not yet present on an older row snapshot.
#[must_use]
pub fn selected_path(
    selected: &Arrangement,
    provider: &ProviderRow,
    agent: &AgentRow,
) -> CredentialPath {
    arrangement::credential_path(
        selected.kind,
        selected.protocol.is_none(),
        native_login_here(selected.kind, provider, Some(agent.id)),
    )
}

/// Native access needs the actual keeper's home; another runtime uses an explicitly issued copy.
pub fn native_login_here(kind: ArrangementType, provider: &ProviderRow, agent: Option<Id>) -> bool {
    provider.agent_id.is_some()
        && provider.agent_id == agent
        && (kind != ArrangementType::Separated || provider.key_machine_id == provider.machine_id)
}

/// The facts an arrangement is chosen from, for one thread.
#[derive(Debug, Clone)]
pub struct Facts<'a> {
    /// What the profile's adapter declares.
    pub descriptor: &'a AgentDescriptor,
    /// The provider.
    pub provider: &'a ProviderRow,
    /// The environment.
    pub environment: &'a EnvironmentRow,
    /// What the environment's exec node offers, in the order to try.
    pub offers: Vec<String>,
    /// The settings of the thread's place.
    pub settings: &'a Resolved,
    /// The types the entry the thread is bound through names; none while it
    /// names none.
    pub entry: Option<Vec<ArrangementType>>,
    /// The template's list of harness kinds that may run beside its tools.
    pub colocated: Vec<String>,
    /// The actual machines and the profile's required machine, checked before offering a placement.
    pub runtime: RuntimeFacts,
}

/// Whether each possible harness location can actually host this profile.
#[derive(Debug, Clone, Default)]
pub struct RuntimeFacts {
    /// The profile actually being placed, distinct from the profile keeping a native account.
    pub agent_id: Option<Id>,
    /// Credential forms declared by that account's own source harness, never inferred from its client.
    pub issuer_forms: Option<Vec<CredentialForm>>,
    /// Why the machine's declared forwarding service cannot run or issue a key, when it cannot.
    pub forwarder_problem: Option<String>,
    /// Why the provider's machine cannot host the harness, when it cannot.
    pub provider_problem: Option<String>,
    /// Why the environment cannot host the harness, when it cannot.
    pub environment_problem: Option<String>,
    /// A connected profile stays on the machine it was registered from.
    pub agent_machine: Option<Id>,
}

/// Whether a machine's report contains the harness it is allowed to run.
#[must_use]
pub fn machine_has_harness(machine: &MachinePlace, kind: &str) -> bool {
    let listed = |member: &str| {
        machine
            .facts
            .get(member)
            .and_then(Json::as_array)
            .is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.get("kind").and_then(Json::as_str) == Some(kind))
            })
    };
    if !machine.roles.iter().any(|role| role == "agent") || !listed("adapters") {
        return false;
    }
    !needs_harness_program(&machine.facts, kind)
        || (harness_permitted(&machine.facts, kind) && listed("harnesses"))
}

fn needs_harness_program(facts: &Json, kind: &str) -> bool {
    facts
        .get("adapters")
        .and_then(Json::as_array)
        .and_then(|adapters| {
            adapters
                .iter()
                .find(|adapter| adapter.get("kind").and_then(Json::as_str) == Some(kind))
        })
        .and_then(|adapter| adapter.get("needs_binary"))
        .and_then(Json::as_bool)
        != Some(false)
}

fn harness_permitted(facts: &Json, kind: &str) -> bool {
    let Some(policy) = orch_db::queries::environment_facts::policy_in(facts) else {
        return true;
    };
    policy
        .get("agent.harnesses")
        .and_then(Json::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("kind").and_then(Json::as_str) == Some(kind))
        })
}

fn harness_problem(machine: Option<&MachinePlace>, kind: &str) -> Option<String> {
    let Some(machine) = machine else {
        return Some("The place where the harness would run has no machine.".to_owned());
    };
    if !machine.roles.iter().any(|role| role == "agent") {
        return Some(
            "The machine where the harness would run does not take the agent role.".to_owned(),
        );
    }
    if machine
        .facts
        .get("adapters")
        .and_then(Json::as_array)
        .is_none()
    {
        return Some(
            "The machine is connected and is waiting to report its initial facts.".to_owned(),
        );
    }
    (!machine_has_harness(machine, kind))
        .then(|| format!("The machine does not report an allowed, installed {kind} harness."))
}

async fn reported_harness_problem(
    conn: &mut orch_db::Conn,
    machine: Option<&MachinePlace>,
    agent: &AgentRow,
) -> Result<Option<String>, EngineError> {
    let problem = harness_problem(machine, &agent.harness_kind);
    let Some(machine) = machine else {
        return Ok(problem);
    };
    if !machine.roles.iter().any(|role| role == "agent")
        || machine
            .facts
            .get("adapters")
            .and_then(Json::as_array)
            .is_none()
    {
        return Ok(problem);
    }
    if let Some((status, detail, descriptor, pin_resolved)) =
        orch_db::queries::binding_start::harness_report(
            conn,
            machine.id,
            agent.id,
            agent.profile.get("harness_version").and_then(Json::as_str),
        )
        .await?
    {
        if status == "error" {
            // Only this recorded refusal may be retried after the pin stops excluding the build.
            if pin_resolved {
                return Ok(problem);
            }
            return Ok(Some(detail.unwrap_or_else(|| {
                "The installed harness could not be started.".to_owned()
            })));
        }
        let locally_allowed = harness_permitted(&machine.facts, &agent.harness_kind);
        if status == harnesses::ONLINE
            && locally_allowed
            && descriptor
                .as_object()
                .is_some_and(|descriptor| !descriptor.is_empty())
        {
            // A compiled harness may need no separate program; a reported machine policy still
            // has to permit its kind, even after a successful old probe.
            return Ok(None);
        }
    }
    Ok(problem)
}

/// Reads the actual harness sides while retaining template promises for a managed runtime not made yet.
pub async fn runtime_facts(
    conn: &mut orch_db::Conn,
    agent: &AgentRow,
    provider: &ProviderRow,
    environment: &EnvironmentRow,
) -> Result<RuntimeFacts, EngineError> {
    let provider_machine = match leases::machine_of(provider) {
        Some(id) => resources::machine_place(conn, id).await?,
        None => None,
    };
    let runtime_machine = if provider.machine_id == provider.key_machine_id {
        provider_machine.clone()
    } else {
        match provider.machine_id {
            Some(id) => resources::machine_place(conn, id).await?,
            None => None,
        }
    };
    let environment_machine = match environment.machine_id {
        Some(id) => resources::machine_place(conn, id).await?,
        None => None,
    };
    let forwarder_problem = if !serves(provider)
        .iter()
        .any(|access| matches!(access, Serves::Forwarded { .. }))
        || (provider.machine_id.is_none() && provider_machine.is_none())
    {
        None
    } else {
        match provider_machine.as_ref() {
            None => Some("The provider has no machine to forward model calls.".to_owned()),
            Some(machine) if !machine.roles.iter().any(|role| role == "provider") => {
                Some("The machine that keeps the key does not take the provider role.".to_owned())
            }
            Some(machine)
                if machine
                    .facts
                    .get("adapters")
                    .and_then(Json::as_array)
                    .is_none() =>
            {
                Some(
                    "The provider machine is connected and is waiting to report its initial facts."
                        .to_owned(),
                )
            }
            Some(_) => None,
        }
    };
    let provider_problem = if provider.machine_id.is_none() {
        None
    } else {
        reported_harness_problem(conn, runtime_machine.as_ref(), agent).await?
    };
    let unit_lifecycle = orch_db::queries::environment_facts::of(conn, environment.id)
        .await?
        .is_some_and(|facts| facts.makes_units());
    let environment_problem = if unit_lifecycle && environment_machine.is_none() {
        None
    } else {
        reported_harness_problem(conn, environment_machine.as_ref(), agent).await?
    };
    let issuer = match provider.agent_id {
        Some(id) if id == agent.id => Some(agent.clone()),
        Some(id) => resources::agent(conn, id).await?,
        None => None,
    };
    let issuer_forms = issuer
        .as_ref()
        .map(harnesses::descriptor_of)
        .transpose()?
        .flatten()
        .map(|descriptor| descriptor.credentials.issues);
    Ok(RuntimeFacts {
        agent_id: Some(agent.id),
        issuer_forms,
        forwarder_problem,
        provider_problem,
        environment_problem,
        agent_machine: agent.machine_id,
    })
}

/// What the environment's facts list, where they were reported.
fn environment_facts(environment: &EnvironmentRow) -> Option<EnvironmentFacts> {
    serde_json::from_value(environment.facts.clone()).ok()
}

/// Whether the provider's runtime and the environment are one machine.
fn same_machine(provider: &ProviderRow, environment: &EnvironmentRow) -> bool {
    leases::machine_of(provider).is_some() && leases::machine_of(provider) == environment.machine_id
}

/// The access the provider declares: its native harness and each protocol its endpoints or translators serve.
#[must_use]
pub fn serves(provider: &ProviderRow) -> Vec<Serves> {
    provider
        .harness_kind
        .iter()
        .map(|harness| Serves::Native {
            harness: harness.clone(),
        })
        .chain(provider.protocols.iter().map(|protocol| Serves::Forwarded {
            protocol: protocol.clone(),
        }))
        .collect()
}

/// The two sides an arrangement is chosen between.
#[must_use]
pub fn sides(facts: &Facts<'_>) -> (ProviderSide, PlaceSide) {
    let provider = facts.provider;
    let issues: Vec<CredentialForm> = match provider_kind(provider) {
        ProviderKind::Subscription => facts.runtime.issuer_forms.clone().unwrap_or_default(),
        ProviderKind::Api => vec![CredentialForm::api_key()],
    };
    let credential = match provider.held_by_environment_id {
        Some(holder) => CredentialLocation::HeldBy(holder),
        None => CredentialLocation::AtHome,
    };
    let kind = &facts.descriptor.kind;
    let can_run_harness = facts.runtime.environment_problem.is_none()
        && (same_machine(provider, facts.environment)
            || facts.colocated.iter().any(|listed| listed == kind));
    (
        ProviderSide {
            serves: serves(provider),
            issues,
            credential,
        },
        PlaceSide {
            environment_id: facts.environment.id,
            offers: facts.offers.clone(),
            executors: environment_facts(facts.environment)
                .map(|facts| facts.executors)
                .unwrap_or_default(),
            can_run_harness,
            same_machine: same_machine(provider, facts.environment),
            harness_version: None,
        },
    )
}

/// The permissions that apply to the thread.
#[must_use]
pub fn policy(facts: &Facts<'_>) -> Policy {
    Policy {
        prefer: facts.settings.binding_prefer.clone(),
        permission: arrangement::Permission {
            provider: allowed_by_provider(facts.provider),
            entry: facts.entry.clone(),
            binding_allowed: facts.settings.binding_allowed.clone(),
            machine: None,
        },
    }
}

/// The arrangement a thread takes, or why there is none. `requested` names a
/// type that must be available and permitted.
pub fn choose(
    facts: &Facts<'_>,
    requested: Option<ArrangementType>,
) -> Result<Arrangement, Incompatible> {
    choose_with_machine(facts, None, requested)
}

/// The sentence that tells why no arrangement is available.
#[must_use]
pub fn describe(why: &Incompatible) -> String {
    match why {
        Incompatible::ArrangementNotAllowed { by } => {
            format!("{}: not allowed by {by:?}", why.code())
        }
        Incompatible::ExecutorMissing { offers, usable } => format!(
            "{}: the environment offers {offers:?} and the harness can work through {usable:?}",
            why.code()
        ),
        other => other.code().to_owned(),
    }
}

/// The sentence a person is told when an arrangement is not offered: what it is that stands in the
/// way, in plain words, beside the reason the code names.
#[must_use]
pub fn sentence(why: &Incompatible) -> String {
    use orch_agent_contract::arrangement::NotAllowedBy;
    match why {
        Incompatible::HarnessMismatch => {
            "This provider is a subscription of another harness than this agent profile runs."
                .to_owned()
        }
        Incompatible::NoCommonProtocol => {
            "The harness and the provider speak no protocol in common.".to_owned()
        }
        Incompatible::NoCredentialForm => {
            "The provider has no credential that this harness can run on away from the provider's own runtime."
                .to_owned()
        }
        Incompatible::ArrangementNotAllowed { by } => match by {
            NotAllowedBy::Provider => {
                "The provider's owner does not allow the provider to be used this way.".to_owned()
            }
            NotAllowedBy::Entry => {
                "The entry through which you use the provider does not allow it to be used this way."
                    .to_owned()
            }
            NotAllowedBy::Policy => {
                "A policy of the place where the thread lies leaves this arrangement out.".to_owned()
            }
            NotAllowedBy::Machine => {
                "The machine that keeps the credential does not allow it to be used this way."
                    .to_owned()
            }
        },
        Incompatible::HandedOverElsewhere => {
            "The provider's credential is with another environment.".to_owned()
        }
        Incompatible::AgentNotOnThatMachine => {
            "The machine where this arrangement puts the harness does not offer the required agent role and harness, or this profile belongs to another machine."
                .to_owned()
        }
        Incompatible::ExecutorMissing { offers, usable } => format!(
            "The environment offers {} and the harness can work through {}.",
            list_of(offers),
            list_of(usable)
        ),
        Incompatible::ColocationImpossible => {
            "The provider's runtime and the environment are not one machine.".to_owned()
        }
    }
}

fn list_of(names: &[String]) -> String {
    if names.is_empty() {
        "nothing".to_owned()
    } else {
        names.join(", ")
    }
}

/// What the environment's exec node offers, in the order to try.
pub async fn offers_of(
    conn: &mut orch_db::Conn,
    environment: Id,
) -> Result<Vec<String>, EngineError> {
    Ok(
        orch_settings::get::<Vec<String>>(conn, Place::Environment(environment), keys::EXEC_OFFERS)
            .await?
            .unwrap_or_default(),
    )
}

/// The harness kinds a template promises, or that a connected machine reports and locally allows.
pub async fn colocated_of(
    conn: &mut orch_db::Conn,
    environment: &EnvironmentRow,
) -> Result<Vec<String>, EngineError> {
    if orch_db::queries::environment_facts::of(conn, environment.id)
        .await?
        .is_some_and(|facts| facts.attaches())
    {
        let Some(id) = environment.machine_id else {
            return Ok(Vec::new());
        };
        let Some(machine) = resources::machine_place(conn, id).await? else {
            return Ok(Vec::new());
        };
        let kinds = machine
            .facts
            .get("harnesses")
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("kind").and_then(Json::as_str))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut allowed: Vec<String> = kinds
            .into_iter()
            .filter(|kind| machine_has_harness(&machine, kind))
            .collect();
        if machine.roles.iter().any(|role| role == "agent") {
            allowed.extend(orch_db::queries::binding_start::reported_kinds(conn, id).await?);
        }
        allowed.sort();
        allowed.dedup();
        return Ok(allowed);
    }
    let spec = environments::template_spec(conn, environment.id).await?;
    Ok(spec
        .map(|spec| orch_agent_programs::Programs::colocated_for(&spec))
        .unwrap_or_default())
}

// ---------------------------------------------------------------- creating a thread

/// A thread to create.
#[derive(Debug, Clone)]
pub struct NewThread {
    /// The topic it is made in.
    pub topic: Id,
    /// Its name.
    pub name: String,
    /// The agent profile it runs.
    pub agent: Id,
    /// The provider it runs on.
    pub provider: Id,
    /// The environment its tools run in.
    pub environment: Id,
    /// Its directory there, relative to the workspace root, without `..`.
    pub directory: String,
    /// The type of arrangement the creator asks for; the first that is
    /// available and permitted when none.
    pub arrangement: Option<ArrangementType>,
    /// The model; the settings of the place decide when none.
    pub model: Option<String>,
    /// The reasoning effort.
    pub effort: Option<String>,
}

/// How a new thread is born besides what it is made of.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Birth {
    /// The optional first authored message, accepted in the same transaction as the thread.
    /// Refusal rolls back the thread, its page, source and settings too.
    pub first_input: Option<orch_api_contract::nodes::FirstInput>,
    /// Missing directories default to source preparation; an explicit choice overrides that on any machine.
    pub prepare_source: Option<bool>,
    /// The owned attempt to prepare before a first session.
    pub worktree: Option<orch_api_contract::source::WorktreeChoice>,
    /// Whether the title is left to automation: the creator gave no name, and
    /// the thread is called by one until a person names it. A thread whose
    /// creator named it keeps that name from automation.
    pub titled_by_automation: bool,
    /// Settings the creator gives the thread at once, by key, besides its
    /// model and effort.
    pub settings: Vec<(String, Json)>,
    /// The page the thread starts with when it does not follow the defaults:
    /// the rows its creator changes in the same request.
    pub page: Option<orch_access_contract::pages::PageChange>,
}

/// A thread that was created.
#[derive(Debug, Clone, PartialEq)]
pub struct CreatedThread {
    /// Its node.
    pub thread: Id,
    /// The first segment of it.
    pub segment: Id,
    /// The arrangement it took.
    pub arrangement: Arrangement,
    /// The highest number of its sequence so far.
    pub seq: Seq,
}

/// Why a thread could not be created.
#[derive(Debug, thiserror::Error)]
pub enum CreateThreadError {
    /// There is no such topic, or it is deleted.
    #[error("there is no such topic")]
    NoSuchTopic,
    /// The caller lacks something, and this is all of it.
    #[error("the caller lacks access to what the thread is made of")]
    Denied(Box<Refusal>),
    /// The name is empty or too long.
    #[error("a thread's name is between 1 and {NAME_MAX_CHARS} characters")]
    InvalidName,
    /// The directory leaves the workspace.
    #[error("a directory is relative to the workspace and does not go up")]
    InvalidDirectory,
    /// The agent profile does not exist or is switched off.
    #[error("the agent profile is not there or is switched off")]
    AgentUnavailable,
    /// The provider does not exist or is switched off.
    #[error("the provider is not there or is switched off")]
    ProviderUnavailable,
    /// The provider's current address is outside a contributing policy at the destination.
    #[error("{0}")]
    ProviderNotAllowed(crate::provider_policy::Refused),
    /// The environment does not exist, or its removal is scheduled.
    #[error("the environment is not there or is being removed")]
    EnvironmentUnavailable,
    /// The profile's harness never reported what it is.
    #[error("the agent profile has not reported what its harness is")]
    AgentNotReported,
    /// The model is not one the provider offers, or not one the place allows.
    #[error("the model {0:?} is not available here")]
    ModelNotAllowed(String),
    /// The effort is not one the model lists.
    #[error("the model does not take the effort {0:?}")]
    EffortNotListed(String),
    /// No arrangement is available and permitted.
    #[error("no arrangement is available: {}", describe(.0))]
    Incompatible(Incompatible),
    /// What the thread's places say to it as standing instructions comes, with its headings, to more characters than a
    /// thread may be told ([`crate::sessions::STANDING_LIMIT`]): the thread is not made, and its first turn never meets it.
    #[error(
        "the standing instructions here come to {0} characters with their headings, more than a thread may be told"
    )]
    InstructionsTooLarge(usize),
    /// A setting could not be written.
    #[error("a setting of the thread could not be written: {0}")]
    Setting(String),
    /// The page the creator asked the thread to start with could not be set.
    #[error("the page of the thread could not be set: {0}")]
    Page(String),
    /// The first message was refused; the whole creation transaction is rolled back.
    #[error("the first message could not be accepted: {error}")]
    FirstInput {
        /// The uncommitted thread, used only to attribute the ordinary input refusal.
        thread: Id,
        /// The same refusal returned when sending to an existing thread.
        error: Box<crate::inputs::InputError>,
    },
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<orch_db::DbError> for CreateThreadError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_authz::Error> for CreateThreadError {
    fn from(error: orch_authz::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// Whether a directory is a path under the workspace root: relative, with no
/// step up and no empty step.
#[must_use]
pub fn directory_is_valid(directory: &str) -> bool {
    directory.is_empty()
        || (!directory.starts_with('/')
            && directory
                .split('/')
                .all(|step| !step.is_empty() && step != ".." && step != "."))
}

impl Engine {
    /// Learns installed, locally allowed harnesses after facts arrive, without a thread or model call.
    pub async fn discover_descriptors(&self, machine: Id) -> Result<(), EngineError> {
        self.discover_descriptors_after_facts(machine, true).await
    }

    /// Repeats a failed discovery only after fresh facts, which can announce a repaired installation.
    pub async fn discover_descriptors_after_facts(
        &self,
        machine: Id,
        retry: bool,
    ) -> Result<(), EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let Some(place) = resources::machine_place(&mut conn, machine).await? else {
            return Ok(());
        };
        if !place.online || place.deleted_at.is_some() || place.status != "active" {
            return Ok(());
        }
        // A harness the machine could not run is asked again after a pause, not at every report of its
        // machine (several a minute): a repaired installation is noticed within it.
        let retry_before = self.inner.clock.now() - DISCOVERY_RETRY_AFTER;
        let pairs =
            orch_db::queries::binding_start::undiscovered(&mut conn, machine, retry, retry_before)
                .await?;
        drop(conn);
        for (agent, provider) in pairs {
            let profile = {
                let mut conn = self.inner.db.acquire().await?;
                resources::agent(&mut conn, agent).await?
            };
            let Some(profile) = profile else { continue };
            let drives = place
                .facts
                .get("adapters")
                .and_then(Json::as_array)
                .is_some_and(|adapters| {
                    adapters.iter().any(|adapter| {
                        adapter.get("kind").and_then(Json::as_str) == Some(&profile.harness_kind)
                    })
                });
            if !place.roles.iter().any(|role| role == "agent") || !drives {
                continue;
            }
            let key = harnesses::HarnessKey {
                provider,
                agent,
                machine,
            };
            {
                let mut conn = self.inner.db.acquire().await?;
                let known = resources::harness(&mut conn, provider, agent, machine).await?;
                if known.as_ref().is_some_and(|known| {
                    known.status == harnesses::ONLINE
                        && known
                            .descriptor
                            .as_object()
                            .is_none_or(|descriptor| descriptor.is_empty())
                }) {
                    resources::set_harness_status(
                        &mut conn,
                        provider,
                        agent,
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
                }
            }
            let command = new_id();
            let outcome = tokio::time::timeout(
                self.inner.config.start_timeout,
                self.ensure_harness(key, &profile, command),
            )
            .await;
            match outcome {
                Ok(Ok(harnesses::Started::Up)) => {}
                Ok(Ok(harnesses::Started::Failed(outcome))) => {
                    tracing::info!(%machine, %agent, ?outcome, "an installed harness could not report its descriptor");
                    self.discovery_failed(key, &discovery_detail(&outcome))
                        .await?;
                }
                Ok(Err(error)) => {
                    tracing::warn!(%machine, %agent, %error, "harness discovery failed")
                }
                Err(_) => {
                    self.inner.commands.cancel(command).await?;
                    self.discovery_failed(key, "the harness did not start in time")
                        .await?;
                }
            }
        }
        Ok(())
    }

    /// Says once, as the harness's own state on that machine, why it could not come up there: its row is
    /// `error` with the reason, which the provider shows, and the row's time holds the next try back for
    /// [`DISCOVERY_RETRY_AFTER`].
    async fn discovery_failed(
        &self,
        key: harnesses::HarnessKey,
        detail: &str,
    ) -> Result<(), EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        resources::set_harness_status(
            &mut conn,
            key.provider,
            key.agent,
            key.machine,
            &resources::HarnessReport {
                status: "error",
                detail: Some(detail),
                version: None,
                sessions_open: 0,
            },
            self.inner.clock.now(),
        )
        .await?;
        Ok(())
    }

    /// Creates a thread in a topic, bound to an agent profile, a provider and
    /// an environment. The caller needs `topic.interact` on the topic, the
    /// use of the profile and of the provider with the model and effort, and
    /// the management of the environment; each is asked, and what is lacking
    /// is told whole. The arrangement is the one the contract's rule chooses.
    /// The thread begins idle, with its first segment and a first line in its
    /// timeline.
    pub async fn create_thread(
        &self,
        caller: &Caller,
        new: NewThread,
    ) -> Result<CreatedThread, CreateThreadError> {
        self.create_thread_with(caller, new, Birth::default()).await
    }

    /// As [`Engine::create_thread`], with how the thread is born.
    pub async fn create_thread_with(
        &self,
        caller: &Caller,
        new: NewThread,
        birth: Birth,
    ) -> Result<CreatedThread, CreateThreadError> {
        self.create_thread_as(caller, new, birth, None).await
    }

    /// As [`Engine::create_thread_with`], under an identifier chosen beforehand: a fork names the thread it
    /// is making in its plan before the thread exists, so that whatever restarts it finds the same thread.
    pub async fn create_thread_as(
        &self,
        caller: &Caller,
        new: NewThread,
        birth: Birth,
        id: Option<Id>,
    ) -> Result<CreatedThread, CreateThreadError> {
        self.create_thread_taking(caller, new, birth, id, None)
            .await
    }

    /// As [`Engine::create_thread_as`], for a thread that takes in a session that exists already
    /// (`adopted`, the harness's own name for it): its first segment holds the session, and the first
    /// turn opens it as one that was adopted.
    pub async fn create_thread_taking(
        &self,
        caller: &Caller,
        new: NewThread,
        birth: Birth,
        id: Option<Id>,
        adopted: Option<&str>,
    ) -> Result<CreatedThread, CreateThreadError> {
        self.create_thread_from(None, caller, new, birth, id, adopted)
            .await
    }

    /// As [`Engine::create_thread_taking`], in the transaction of the API's action guard: `principal` was
    /// loaded in `tx` by that check once the request had come. An unreported harness is returned to the
    /// caller, which may discover it outside the transaction and retry with a fresh credential guard.
    pub async fn create_thread_guarded(
        &self,
        tx: orch_db::Tx,
        principal: Principal,
        new: NewThread,
        birth: Birth,
        adopted: Option<&str>,
    ) -> Result<CreatedThread, CreateThreadError> {
        let caller = principal.caller().clone();
        self.create_thread_from(Some((tx, principal)), &caller, new, birth, None, adopted)
            .await
    }

    /// Makes an ordinary thread inside its caller's guarded transaction, without committing it.
    /// Due schedules use this so their run, new thread and first authored input succeed together.
    /// A cold unreported harness refuses; no native effect is performed while the transaction is held.
    pub async fn create_thread_in_transaction(
        &self,
        tx: &mut orch_db::Conn,
        caller: &Caller,
        new: &NewThread,
        birth: &Birth,
        id: Id,
        now: DateTime<Utc>,
    ) -> Result<CreatedThread, CreateThreadError> {
        let name = new.name.trim();
        if name.is_empty() || name.chars().count() > NAME_MAX_CHARS {
            return Err(CreateThreadError::InvalidName);
        }
        if !directory_is_valid(&new.directory) {
            return Err(CreateThreadError::InvalidDirectory);
        }
        let characters = crate::sessions::standing_characters(tx, new.topic).await?;
        if characters > crate::sessions::STANDING_LIMIT {
            return Err(CreateThreadError::InstructionsTooLarge(characters));
        }
        let principal = Principal::load(tx, caller.clone(), Time::new(now)).await?;
        self.create_in(tx, &principal, new, name, birth, Some(id), None, now)
            .await
    }

    async fn create_thread_from(
        &self,
        mut guarded: Option<(orch_db::Tx, Principal)>,
        caller: &Caller,
        new: NewThread,
        birth: Birth,
        id: Option<Id>,
        adopted: Option<&str>,
    ) -> Result<CreatedThread, CreateThreadError> {
        let name = new.name.trim();
        if name.is_empty() || name.chars().count() > NAME_MAX_CHARS {
            return Err(CreateThreadError::InvalidName);
        }
        if !directory_is_valid(&new.directory) {
            return Err(CreateThreadError::InvalidDirectory);
        }
        let may_discover = guarded.is_none();
        let mut learned = false;
        loop {
            let (mut tx, principal) = match guarded.take() {
                Some(given) => given,
                None => {
                    // A birth, as in the action guard's transaction: its commit checks the epochs its
                    // decisions read and moves only the cache epoch, so no write elsewhere is refused for it.
                    let mut tx = orch_db::queries::transaction_guard::request_scope(
                        orch_db::queries::transaction_guard::Scope::Creation,
                        self.inner.db.begin(),
                    )
                    .await?;
                    let principal =
                        Principal::load(&mut tx, caller.clone(), Time::new(self.inner.clock.now()))
                            .await?;
                    (tx, principal)
                }
            };
            let now = principal.now().get();
            // A thread made here starts with what the topic's places say to it: where that is more than a thread may be
            // told, it is not made.
            let characters = crate::sessions::standing_characters(&mut tx, new.topic).await?;
            if characters > crate::sessions::STANDING_LIMIT {
                return Err(CreateThreadError::InstructionsTooLarge(characters));
            }
            match self
                .create_in(&mut tx, &principal, &new, name, &birth, id, adopted, now)
                .await
            {
                Ok(created) => {
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                    if birth.first_input.is_some() {
                        self.wake(created.thread);
                    }
                    return Ok(created);
                }
                // The caller may use what they chose, and the profile's harness never said what it
                // is: it is started once to say so, outside the transaction, and the thread is made
                // from what it declared.
                Err(CreateThreadError::AgentNotReported) if may_discover && !learned => {
                    drop(tx);
                    learned = true;
                    match self
                        .learn_descriptor(new.agent, new.provider, Some(new.environment))
                        .await?
                    {
                        Learning::Known => {}
                        Learning::Unknown(_) => return Err(CreateThreadError::AgentNotReported),
                    }
                }
                Err(other) => return Err(other),
            }
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "a thread is made from who makes it, what it is made of, how it is born and the identifier it is born under"
    )]
    async fn create_in(
        &self,
        tx: &mut orch_db::Conn,
        principal: &Principal,
        new: &NewThread,
        name: &str,
        birth: &Birth,
        id: Option<Id>,
        adopted: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<CreatedThread, CreateThreadError> {
        let conn: &mut orch_db::Conn = &mut *tx;
        let caller = principal.caller();
        let person = principal.user();
        let decision = nodes::check(
            &mut *conn,
            principal,
            new.topic,
            Permission::TopicStartThreads,
            Act::Use,
            orch_authz::Policy::default(),
        )
        .await
        .map_err(|error| match error {
            orch_authz::Error::NotFound(_) => CreateThreadError::NoSuchTopic,
            other => other.into(),
        })?;
        if let Decision::Refused(refusal) = decision {
            return Err(CreateThreadError::Denied(Box::new(refusal)));
        }
        if threads::node_name(&mut *conn, new.topic).await?.is_none() {
            return Err(CreateThreadError::NoSuchTopic);
        }
        let agent = resources::agent(&mut *conn, new.agent)
            .await?
            .filter(|agent| agent.deleted_at.is_none() && !agent.disabled)
            .ok_or(CreateThreadError::AgentUnavailable)?;
        let provider = resources::provider(&mut *conn, new.provider)
            .await?
            .filter(|provider| provider.deleted_at.is_none() && provider.status != "disabled")
            .ok_or(CreateThreadError::ProviderUnavailable)?;
        if !orch_db::queries::loss::accepts_threads(conn, new.environment).await? {
            return Err(CreateThreadError::EnvironmentUnavailable);
        }
        let environment = environments::environment(&mut *conn, new.environment)
            .await?
            .filter(|environment| {
                environment.deleted_at.is_none()
                    && environment.remove_at.is_none()
                    && !matches!(environment.status.as_str(), "lost" | "removed")
            })
            .ok_or(CreateThreadError::EnvironmentUnavailable)?;

        // What the thread is put on is asked of the access engine for the
        // caller, all of it at once: a refusal tells everything that is lacking,
        // not only the first thing.
        let mut lacking: Vec<(Permission, ObjectKind, Id, Vec<MissingAccess>)> = Vec::new();
        let agent_answer = self
            .inner
            .access
            .agent_use(&mut *conn, person, new.topic, agent.id)
            .await
            .map_err(EngineError::from)?;
        if let Answer::Refused(missing) = agent_answer {
            lacking.push((Permission::AgentUse, ObjectKind::Agent, agent.id, missing));
        }
        let provider_answer = self
            .inner
            .access
            .provider_use(
                &mut *conn,
                &ProviderUse {
                    person,
                    node: new.topic,
                    provider: provider.id,
                    model: new.model.as_deref(),
                    effort: new.effort.as_deref(),
                },
            )
            .await
            .map_err(EngineError::from)?;
        let entry = match &provider_answer {
            Answer::Allowed { arrangements } => arrangements.clone(),
            Answer::Refused(_) => None,
        };
        if let Answer::Refused(missing) = provider_answer {
            lacking.push((
                Permission::ProviderUse,
                ObjectKind::Provider,
                provider.id,
                missing,
            ));
        }
        let environment_answer = self
            .inner
            .access
            .environment_place(&mut *conn, person, environment.id)
            .await
            .map_err(EngineError::from)?;
        if let Answer::Refused(missing) = environment_answer {
            lacking.push((
                Permission::EnvironmentUse,
                ObjectKind::Environment,
                environment.id,
                missing,
            ));
        }
        if let Some((permission, kind, id, _)) = lacking.first() {
            let object = ObjectRef {
                kind: *kind,
                id: *id,
            };
            let missing = lacking
                .iter()
                .flat_map(|(_, _, _, missing)| missing.iter().cloned())
                .collect();
            return Err(CreateThreadError::Denied(Box::new(Refusal::of(
                *permission,
                object,
                missing,
            ))));
        }

        // The model and the effort the creator asks for must be ones the
        // provider offers and the place allows, as for any setting of them.
        let settings = resolved::load(&mut *conn, new.topic).await?;
        if let Some(why) =
            crate::provider_policy::forbidden(&mut *conn, new.topic, provider.id).await?
        {
            return Err(CreateThreadError::ProviderNotAllowed(why));
        }
        if let Some(model) = new.model.as_deref() {
            let row = resources::model(&mut *conn, provider.id, model).await?;
            crate::models::check_allowed(row.as_ref(), settings.models_allowed.as_deref(), model)
                .map_err(|_| CreateThreadError::ModelNotAllowed(model.to_owned()))?;
            let row = row.ok_or_else(|| CreateThreadError::ModelNotAllowed(model.to_owned()))?;
            if let Some(effort) = new.effort.as_deref() {
                let parameters = model_parameters(&row)?;
                if !parameters.efforts().iter().any(|listed| listed == effort) {
                    return Err(CreateThreadError::EffortNotListed(effort.to_owned()));
                }
            }
        }

        // Only for a caller who may use all of it: a profile whose harness never said what it is has it
        // say so, and the thread is made then (see `create_thread_with`).
        let descriptor =
            harnesses::descriptor_of(&agent)?.ok_or(CreateThreadError::AgentNotReported)?;
        let offers = offers_of(&mut *conn, environment.id).await?;
        let colocated = colocated_of(&mut *conn, &environment).await?;
        let machine = machine_arrangements_of(&mut *conn, &provider).await?;
        let facts = Facts {
            descriptor: &descriptor,
            provider: &provider,
            environment: &environment,
            offers,
            settings: &settings,
            entry,
            colocated,
            runtime: runtime_facts(conn, &agent, &provider, &environment).await?,
        };
        let place = initial_place(conn, &facts).await?;
        let arrangement = choose_placed(&facts, &place, machine, new.arrangement)
            .map_err(CreateThreadError::Incompatible)?;

        // An inherited preference may belong to a different program. Only the initial default is
        // narrowed here: explicit choices are checked, and ordinary setting writes retain locks.
        let initial_approval = if let Some(value) = birth
            .settings
            .iter()
            .rev()
            .find(|(key, _)| key == keys::APPROVAL_MODE)
            .map(|(_, value)| value)
        {
            let mode: ApprovalMode = serde_json::from_value(value.clone())
                .map_err(|error| CreateThreadError::Setting(error.to_string()))?;
            if !descriptor.approval_modes.contains(&mode) {
                return Err(CreateThreadError::Setting(
                    crate::sessions::APPROVAL_UNSUPPORTED.to_owned(),
                ));
            }
            None
        } else if descriptor.approval_modes.contains(&settings.approval_mode) {
            None
        } else {
            Some(
                offered_approval_mode(&descriptor, settings.approval_mode)
                    .map_err(|why| CreateThreadError::Setting(why.to_owned()))?,
            )
        };

        let thread = id.unwrap_or_else(new_id);
        if !threads::insert_thread(&mut *conn, thread, new.topic, name, person, now).await? {
            return Err(CreateThreadError::NoSuchTopic);
        }
        // A node is born with its maker as its first owner, with its chat, and
        // with a line in the feeds of its place, as any node is.
        if birth.titled_by_automation {
            threads::leave_title_to_automation(&mut *conn, thread).await?;
        }
        orch_authz::pages::start(
            &mut *conn,
            orch_access_contract::pages::StoredKind::Node,
            thread,
            person,
            Time::new(now),
        )
        .await?;
        threads::make_node_chat(&mut *conn, thread, person, now).await?;
        crate::notifications::follow_thread(&mut *conn, person, thread, now).await?;
        orch_db::queries::hierarchy::stamp(&mut *conn, thread).await?;
        orch_db::queries::hierarchy::stamp(&mut *conn, new.topic).await?;
        let mut unit = Unit::new(&mut *conn, now);
        let seq = unit.stamp(thread).await?;
        threads::set_binding(
            &mut *unit.conn,
            thread,
            &threads::Binding {
                agent: agent.id,
                provider: provider.id,
                arrangement: arrangement.kind.as_str(),
                executor: arrangement.executor.as_deref(),
                tool_set: arrangement.tool_set.as_deref(),
            },
            seq,
        )
        .await?;
        threads::set_environment(&mut *unit.conn, thread, environment.id, &new.directory, seq)
            .await?;
        crate::temporary_environments::bind_thread(
            &mut *unit.conn,
            environment.id,
            thread,
            person,
            now,
        )
        .await?;
        let path = selected_path(&arrangement, &provider, &agent);
        let segment = new_id();
        let segment_row = threads::insert_segment(
            &mut *unit.conn,
            &NewSegment {
                id: segment,
                thread_id: thread,
                agent_id: agent.id,
                provider_id: provider.id,
                environment_id: environment.id,
                env_generation: environment.generation,
                arrangement: arrangement.kind.as_str(),
                executor: arrangement.executor.as_deref(),
                tool_set: arrangement.tool_set.as_deref(),
                credential_path: credential_path_word(path),
                native_session_ref: adopted,
                started_by: Some(person),
                started_at: now,
            },
            seq,
        )
        .await?;
        if adopted.is_some() {
            orch_db::queries::adoption::mark_adopted(&mut *unit.conn, segment).await?;
        }
        threads::set_current(&mut *unit.conn, thread, Some(segment), None, seq).await?;
        let reference_time = orch_types::Time::new(unit.now());
        let created_payload = json!({
            "marker": "created",
            "data": {
                "by": crate::views::user_ref_json(&mut *unit.conn, person).await?,
                "agent": crate::views::agent_ref_json(&mut *unit.conn, &agent).await?,
                "provider": crate::views::provider_ref_json(&mut *unit.conn, &provider).await?,
                "model": new.model,
                "effort": new.effort,
                "environment": crate::views::environment_ref_json(&mut *unit.conn, &environment, reference_time).await?,
            }
        });
        crate::ingest::add_marker(
            &mut unit,
            thread,
            segment_row.id,
            None,
            &created_payload,
            "created",
        )
        .await?;
        let mut to_write: Vec<(&str, Json)> = Vec::new();
        if let Some(mode) = initial_approval {
            to_write.push((keys::APPROVAL_MODE, json!(mode)));
        }
        for (key, value) in [
            (keys::AGENT_MODEL, &new.model),
            (keys::AGENT_EFFORT, &new.effort),
        ] {
            if let Some(value) = value {
                to_write.push((key, Json::String(value.clone())));
            }
        }
        to_write.extend(
            birth
                .settings
                .iter()
                .map(|(key, value)| (key.as_str(), value.clone())),
        );
        for (key, value) in to_write {
            orch_settings::write::write(
                &mut *unit.conn,
                &orch_settings::write::Write {
                    actor: Actor::User(person),
                    target: Target::Node(thread),
                    key,
                    value,
                    locked: None,
                    expected_revision: None,
                    may_lock: false,
                    origin: &Origin::default(),
                },
                now,
            )
            .await
            .map_err(|error| CreateThreadError::Setting(error.to_string()))?;
        }
        if let Some(change) = &birth.page {
            orch_authz::pages::change(
                &mut *unit.conn,
                principal,
                orch_authz::pages::Target::Live(thread),
                change,
                orch_authz::Policy::default(),
                &Origin::default(),
            )
            .await
            .map_err(|error| CreateThreadError::Page(error.to_string()))?;
        }
        crate::jobs::source::freeze(
            &mut unit,
            caller,
            thread,
            environment.id,
            &new.directory,
            birth.prepare_source,
            birth.worktree.as_ref(),
        )
        .await?;
        audit::append(
            &mut *unit.conn,
            &{
                let mut entry = Entry::new(
                    Actor::User(person),
                    orch_db::audit::action!(AuditAction::ThreadCreated).as_str(),
                    "thread",
                    Some(thread),
                    "created a thread",
                );
                entry.history = AuditAction::ThreadCreated.is_history_line();
                entry.node_id = Some(thread);
                entry.environment_id = Some(environment.id);
                entry
            },
            &Origin::default(),
            now,
        )
        .await?;
        if let Some(first) = &birth.first_input {
            self.submit_input_in(
                &mut unit,
                caller,
                thread,
                &crate::inputs::InputRequest {
                    plan: false,
                    content: first.content.clone(),
                    delivery: orch_api_contract::threads::Delivery::Queue,
                    client_id: first.client_id.clone(),
                    bypass: None,
                },
            )
            .await
            .map_err(|error| CreateThreadError::FirstInput {
                thread,
                error: Box::new(error),
            })?;
        }
        let finished = unit.finish().await?;
        let seq = finished.touched.get(&thread).copied().unwrap_or(seq);
        Ok(CreatedThread {
            thread,
            segment,
            arrangement,
            seq,
        })
    }
}

/// The word the segment table has for a credential path.
#[must_use]
pub const fn credential_path_word(path: CredentialPath) -> &'static str {
    match path {
        CredentialPath::Native => "native",
        CredentialPath::Forwarded => "forwarded",
        CredentialPath::Shared => "shared",
        CredentialPath::Handover => "handover",
    }
}

/// What a provider's entry for a model says of it.
pub fn model_parameters(
    row: &ModelRow,
) -> Result<orch_agent_contract::parameters::ModelParameters, EngineError> {
    Ok(serde_json::from_value(row.parameters.clone())?)
}

/// The state a new thread begins in.
pub const BEGINS: &str = thread_state::IDLE;

// ---------------------------------------------------------------- what may be chosen

/// One way of placing a thread's harness, its tools and its credential, as the dialog that binds a
/// thread lists it: whether it can be done with this provider and this environment, whether the
/// parties whose consent it takes allow it, and why not.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrangementOption {
    /// The type.
    pub kind: ArrangementType,
    /// As the thread would have it, where it can be done.
    pub arrangement: Option<Arrangement>,
    /// Whether it can be done with this provider and this environment.
    pub available: bool,
    /// Whether the provider's owner, the entry the caller uses it through, a policy and the machine
    /// that keeps the credential all allow it.
    pub permitted: bool,
    /// Why it is not offered: that it cannot be done, or whose decision it is that it is not
    /// allowed. What cannot be done is told first, as the choice does.
    pub why_not: Option<Incompatible>,
    /// 1 is what a binding that names no type takes: the adapter's order, after the types a scope
    /// prefers. The ones that can be taken come first, the others follow in the same order.
    pub rank: u32,
    /// The actual missing machine capability, when that is the obstacle.
    pub detail: Option<String>,
}

impl ArrangementOption {
    /// Whether a thread can be bound in it.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.available && self.permitted
    }
}

/// The arrangement types in which a credential kept on the person's machine may be used at the
/// server's request, as that machine reports its local policy; none where it reports none (a
/// machine the system made has no policy of its own).
#[must_use]
pub fn arrangements_of_machine(machine: &MachinePlace) -> Option<Vec<ArrangementType>> {
    arrangements_of_policy(orch_db::queries::environment_facts::policy_in(
        &machine.facts,
    ))
}

/// The machine's reported credential policy, shared with provider setup and its current view.
/// Generic over the arrangement type's crate (the agent contract's here, the API contract's in the API's views): the
/// words are the same, and each caller names the type it holds.
pub fn arrangements_of_policy<T: serde::de::DeserializeOwned>(
    policy: Option<&Json>,
) -> Option<Vec<T>> {
    let policy = policy.filter(|policy| !policy.is_null())?;
    let listed = policy.get("provider.arrangements").and_then(Json::as_array);
    Some(
        listed
            .into_iter()
            .flatten()
            .filter_map(|word| word.as_str().and_then(parse))
            .collect(),
    )
}

/// The arrangement types the machine that keeps the provider's credential allows, when that is a
/// person's machine and it reported its policy.
pub async fn machine_arrangements_of(
    conn: &mut orch_db::Conn,
    provider: &ProviderRow,
) -> Result<Option<Vec<ArrangementType>>, EngineError> {
    let policy = orch_db::queries::environment_facts::credential_policy(conn, provider.id).await?;
    Ok(arrangements_of_policy(policy.as_ref()))
}

/// The permissions that apply to the thread, with the list of the machine that keeps the
/// credential (`provider.arrangements` of its local policy) added where there is one.
#[must_use]
pub fn policy_with_machine(facts: &Facts<'_>, machine: Option<Vec<ArrangementType>>) -> Policy {
    let mut policy = policy(facts);
    policy.permission.machine = machine;
    policy
}

/// As [`choose`], with the machine that keeps the credential asked as well.
pub fn choose_with_machine(
    facts: &Facts<'_>,
    machine: Option<Vec<ArrangementType>>,
    requested: Option<ArrangementType>,
) -> Result<Arrangement, Incompatible> {
    let (_, place) = sides(facts);
    choose_placed(facts, &place, machine, requested)
}

/// As [`choose_with_machine`], on a place that is handed over instead of read from the facts. The part that
/// forks and switches judges an environment that is not made yet by what its template promises, and the
/// environment in the facts is the one the thread leaves.
pub fn choose_placed(
    facts: &Facts<'_>,
    place: &PlaceSide,
    machine: Option<Vec<ArrangementType>>,
    requested: Option<ArrangementType>,
) -> Result<Arrangement, Incompatible> {
    let choices = options_placed(facts, place, machine);
    let mut first = None;
    for choice in choices
        .into_iter()
        .filter(|choice| requested.is_none_or(|kind| choice.kind == kind))
    {
        if choice.is_open()
            && let Some(arrangement) = choice.arrangement
        {
            return Ok(arrangement);
        }
        first = first.or(choice.why_not);
    }
    Err(first.unwrap_or(Incompatible::HarnessMismatch))
}

/// Every arrangement the adapter declares with this provider and this environment, each as it
/// would be or with the reason it is not offered, and whether it is allowed, in the order a thread
/// without a request would take them. The same rule that chooses one for a thread reads the same
/// facts, so that what the dialog shows is what binding does.
#[must_use]
pub fn options(facts: &Facts<'_>, machine: Option<Vec<ArrangementType>>) -> Vec<ArrangementOption> {
    let (_, place) = sides(facts);
    options_placed(facts, &place, machine)
}

/// As [`options`], on a place that is handed over instead of read from the facts (see
/// [`choose_placed`]).
#[must_use]
pub fn options_placed(
    facts: &Facts<'_>,
    place: &PlaceSide,
    machine: Option<Vec<ArrangementType>>,
) -> Vec<ArrangementOption> {
    let (provider, _) = sides(facts);
    let policy = policy_with_machine(facts, machine);
    let mut all = arrangement::candidates(facts.descriptor, &provider, place);
    all.sort_by_key(|(kind, _)| {
        policy
            .prefer
            .iter()
            .position(|preferred| preferred == kind)
            .unwrap_or(policy.prefer.len())
    });
    let mut listed: Vec<ArrangementOption> = all
        .into_iter()
        .map(|(kind, candidate)| {
            let needs_copy=kind==ArrangementType::Separated && candidate.as_ref().is_ok_and(|chosen|chosen.protocol.is_none())
                && !native_login_here(kind,facts.provider,facts.runtime.agent_id);
            let candidate=if needs_copy {candidate.and_then(|mut chosen| {
                chosen.form=Some(usable_forms(&provider.issues,&facts.descriptor.credentials.accepts,Some(Custody::Shared))
                    .into_iter().next().ok_or(Incompatible::NoCredentialForm)?);
                Ok(chosen)
            })} else {candidate};
            let problem = runtime_problem(facts, place, kind, candidate.as_ref().ok());
            let (candidate, detail) = match (candidate, problem) {
                (Ok(_), Some(detail)) => (Err(Incompatible::AgentNotOnThatMachine), Some(detail)),
                (Err(Incompatible::AgentNotOnThatMachine), detail) => {
                    (Err(Incompatible::AgentNotOnThatMachine), detail)
                }
                (candidate, _) => (candidate, None),
            };
            let allowed = policy.permission.allows(kind).and_then(|()|if needs_copy {policy.permission.allows(ArrangementType::CredentialShared)} else {Ok(())});
            let permitted = allowed.is_ok();
            let (arrangement, cannot) = match candidate {
                Ok(found) => (Some(found), None),
                Err(why) => (None, Some(why)),
            };
            ArrangementOption {
                kind,
                available: arrangement.is_some(),
                arrangement,
                permitted,
                why_not: cannot.or_else(|| {
                    allowed
                        .err()
                        .map(|by| Incompatible::ArrangementNotAllowed { by })
                }),
                rank: 0,
                detail:detail.or_else(||needs_copy.then(||"This native harness needs a credential copy at its selected runtime; a shared form and credential_shared permission are required.".into())),
            }
        })
        .collect();
    let mut rank: u32 = 0;
    for open in [true, false] {
        for option in listed.iter_mut().filter(|option| option.is_open() == open) {
            rank += 1;
            option.rank = rank;
        }
    }
    listed
}

fn runtime_problem(
    facts: &Facts<'_>,
    place: &PlaceSide,
    kind: ArrangementType,
    selected: Option<&Arrangement>,
) -> Option<String> {
    if let Some(problem) = &facts.runtime.forwarder_problem {
        let native = selected.map_or_else(
            || arrangement::native_matches(facts.descriptor, &serves(facts.provider)),
            |chosen| chosen.protocol.is_none(),
        );
        // Ownership only distinguishes native from shared here; either avoids the forwarder.
        let path = arrangement::credential_path(kind, native, false);
        // Directly shared keys still need their issuer. A native harness can remain available
        // while a different, forwarded service on the same machine is unavailable.
        let issues_key = matches!(
            kind,
            ArrangementType::CredentialShared | ArrangementType::HandedOver
        ) && provider_kind(facts.provider) == ProviderKind::Api;
        if path == CredentialPath::Forwarded || issues_key {
            return Some(problem.clone());
        }
    }
    let machine = if kind == ArrangementType::Separated {
        facts.provider.machine_id
    } else if place.environment_id.is_nil() {
        None
    } else {
        facts.environment.machine_id
    };
    if facts
        .runtime
        .agent_machine
        .is_some_and(|required| Some(required) != machine)
    {
        return Some("This profile's harness is registered on another machine.".to_owned());
    }
    if kind == ArrangementType::Separated {
        facts.runtime.provider_problem.clone()
    } else if place.environment_id.is_nil() {
        None
    } else {
        facts.runtime.environment_problem.clone()
    }
}

/// What the thread's place scope puts into `binding.allowed`: the node whose value decides it, or
/// none while no scope set it.
pub async fn allowed_set_at(
    conn: &mut orch_db::Conn,
    thread: Id,
) -> Result<Option<Id>, EngineError> {
    let def = registry::get(keys::BINDING_ALLOWED).ok_or_else(|| {
        EngineError::unreadable("the setting binding.allowed is not in the registry")
    })?;
    let snapshot = Snapshot::load(conn, Place::Node(thread), Wanted::Keys(&[def])).await?;
    Ok(snapshot.resolve(def).source.node)
}

/// What an environment offers a harness, read once for any number of profiles and providers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvironmentOffers {
    /// What its exec node offers, in the order to try.
    pub offers: Vec<String>,
    /// The harness kinds its template lets run beside its tools.
    pub colocated: Vec<String>,
}

/// What a new environment promises for this harness, before it has reported live facts.
#[must_use]
pub fn promised_place(descriptor: &AgentDescriptor, offers: &EnvironmentOffers) -> PlaceSide {
    let mut executors = Vec::new();
    for entry in descriptor.separated() {
        let Some(kind) = &entry.executor else {
            continue;
        };
        if executors
            .iter()
            .any(|fact: &orch_agent_contract::facts::ExecutorFact| &fact.kind == kind)
        {
            continue;
        }
        executors.push(orch_agent_contract::facts::ExecutorFact {
            kind: kind.clone(),
            version: descriptor.harness.version.clone(),
            available: true,
            reason: None,
            sandbox_modes: Vec::new(),
            tool_sets: entry.tool_sets.clone(),
        });
    }
    PlaceSide {
        environment_id: Id::nil(),
        offers: offers.offers.clone(),
        executors,
        can_run_harness: offers.colocated.contains(&descriptor.kind),
        same_machine: false,
        harness_version: None,
    }
}

/// What an environment offers a harness.
/// A new template-backed runtime has no executor report yet. Use its declared promises for admission,
/// exactly as the chooser does, and retain live facts (including an explicit empty executor list) once reported.
pub async fn initial_place(
    conn: &mut orch_db::Conn,
    facts: &Facts<'_>,
) -> Result<PlaceSide, EngineError> {
    let environment = facts.environment;
    let promised = environment.id.is_nil()
        || (matches!(environment.status.as_str(), "none" | "provisioning")
            && environment.facts.get("executors").is_none()
            && environments::template_spec(conn, environment.id)
                .await?
                .is_some());
    Ok(if promised {
        promised_place(
            facts.descriptor,
            &EnvironmentOffers {
                offers: facts.offers.clone(),
                colocated: facts.colocated.clone(),
            },
        )
    } else {
        sides(facts).1
    })
}

/// What an environment offers a harness.
pub async fn environment_offers(
    conn: &mut orch_db::Conn,
    environment: &EnvironmentRow,
) -> Result<EnvironmentOffers, EngineError> {
    Ok(EnvironmentOffers {
        offers: offers_of(conn, environment.id).await?,
        colocated: colocated_of(conn, environment).await?,
    })
}

/// What an arrangement comes to for a thread: where the harness runs, what carries out its tools,
/// how it reaches the model, what leaves the provider, and who counts what it uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Isolation {
    /// The arrangement type.
    pub kind: ArrangementType,
    /// Whether the harness runs where the provider's runtime is, and not in the environment.
    pub harness_with_provider: bool,
    /// The executor that carries out its tool calls; none is the harness itself.
    pub executor: Option<String>,
    /// Which of that executor's tool sets the agent works with.
    pub tool_set: Option<String>,
    /// How the harness reaches the model.
    pub path: CredentialPath,
    /// Whether the credential itself is with the environment.
    pub handed_over: bool,
    /// For a copy or the credential itself: the adapter's name for what leaves.
    pub form: Option<String>,
    /// How long a copy lasts; none until it is rotated or revoked.
    pub lifetime_secs: Option<i64>,
    /// Where the list of models is enforced.
    pub model_enforcement: ModelEnforcement,
    /// Who reports what is used.
    pub metering: Metering,
}

/// What an option comes to with `provider`, for a profile that holds the provider's login or not,
/// whose adapter reports its own usage or not.
#[must_use]
pub fn isolation(
    option: &ArrangementOption,
    provider: &ProviderRow,
    holds_login: bool,
    usage_tokens: bool,
) -> Isolation {
    let found = option.arrangement.as_ref();
    let native = found.map_or_else(
        || {
            serves(provider)
                .iter()
                .any(|served| matches!(served, Serves::Native { .. }))
        },
        |chosen| chosen.protocol.is_none(),
    );
    let holds_login = holds_login
        && (option.kind != ArrangementType::Separated
            || provider.key_machine_id == provider.machine_id);
    let path = arrangement::credential_path(option.kind, native, holds_login);
    let form = found.and_then(|found| found.form.as_ref());
    Isolation {
        kind: option.kind,
        harness_with_provider: option.kind == ArrangementType::Separated,
        executor: found.and_then(|found| found.executor.clone()),
        tool_set: found.and_then(|found| found.tool_set.clone()),
        path,
        handed_over: path == CredentialPath::Handover,
        form: form.map(|form| form.form.clone()),
        lifetime_secs: form.and_then(|form| form.lifetime_secs),
        model_enforcement: arrangement::model_enforcement(path),
        metering: arrangement::metering(path, usage_tokens),
    }
}

/// A directory name for a topic's threads in an environment: its name in lower case letters,
/// digits, `-` and `_`, the rest becoming one `-`.
#[must_use]
pub fn directory_named_after(topic: &str) -> String {
    let mut name = String::new();
    for letter in topic.chars().flat_map(char::to_lowercase) {
        if letter.is_ascii_alphanumeric() || letter == '_' {
            name.push(letter);
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
    }
    let name = name.trim_end_matches('-');
    if name.is_empty() {
        FALLBACK_DIRECTORY.to_owned()
    } else {
        name.to_owned()
    }
}

/// The directory of a topic's threads when its name makes none.
const FALLBACK_DIRECTORY: &str = "topic";

/// The directory a thread of `topic` gets in an environment when the request names none: that of
/// the thread it joins, else, in an environment that has served only other topics, one named
/// after the topic, else the workspace root itself.
#[must_use]
pub fn default_directory(topic_name: &str, uses: Option<&EnvironmentUse>) -> String {
    match uses {
        Some(uses) => uses
            .topic_directory
            .clone()
            .unwrap_or_else(|| directory_named_after(topic_name)),
        None => String::new(),
    }
}

// ---------------------------------------------------------------- putting a thread without history on something else

/// The agent profile and the provider a thread without history is put on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rebind {
    /// The agent profile.
    pub agent: Id,
    /// The provider.
    pub provider: Id,
    /// The type of arrangement the caller asks for; the first that is available and permitted when
    /// none.
    pub arrangement: Option<ArrangementType>,
    /// The model, by its routing name.
    pub model: Option<String>,
    /// The reasoning effort.
    pub effort: Option<String>,
}

/// The environment a thread without history is put into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replace {
    /// The environment.
    pub environment: Id,
    /// Its directory there, relative to the workspace root; what joining gives when none.
    pub directory: Option<String>,
}

/// What a thread without history was put on.
#[derive(Debug, Clone, PartialEq)]
pub struct Rebound {
    /// The thread.
    pub thread: Id,
    /// The arrangement it has now; none for a thread that is bound to nothing yet.
    pub arrangement: Option<Arrangement>,
    /// The directory it has in its environment.
    pub directory: String,
    /// The highest number of its sequence so far.
    pub seq: Seq,
}

/// Why a thread could not be put on something else.
#[derive(Debug, thiserror::Error)]
pub enum RebindError {
    /// There is no such thread.
    #[error("there is no such thread")]
    NoSuchThread,
    /// The thread is archived, and so read-only.
    #[error("the thread is archived")]
    Archived,
    /// A side thread is set up as its parent is.
    #[error("a side thread is set up as its parent is")]
    SideThread,
    /// The thread has a history: a switch changes what it runs on.
    #[error("a thread with history is changed by a switch")]
    UseSwitch,
    /// The caller lacks something, and this is all of it.
    #[error("the caller lacks access to what the thread is put on")]
    Denied(Box<Refusal>),
    /// The agent profile does not exist or is switched off.
    #[error("the agent profile is not there or is switched off")]
    AgentUnavailable,
    /// The provider does not exist or is switched off.
    #[error("the provider is not there or is switched off")]
    ProviderUnavailable,
    /// The provider's current address is outside a contributing policy at this thread.
    #[error("{0}")]
    ProviderNotAllowed(crate::provider_policy::Refused),
    /// The environment does not exist, or its removal is scheduled.
    #[error("the environment is not there or is being removed")]
    EnvironmentUnavailable,
    /// The thread is in no environment, and a binding is worked out in one.
    #[error("the thread is in no environment yet")]
    NoEnvironment,
    /// The profile's harness never reported what it is.
    #[error("the agent profile has not reported what its harness is")]
    AgentNotReported,
    /// The model is not one the provider offers, or not one the place allows.
    #[error("the model {0:?} is not available here")]
    ModelNotAllowed(String),
    /// The effort is not one the model lists.
    #[error("the model does not take the effort {0:?}")]
    EffortNotListed(String),
    /// The directory leaves the workspace.
    #[error("a directory is relative to the workspace and does not go up")]
    InvalidDirectory,
    /// No arrangement is available and permitted.
    #[error("no arrangement is available: {}", describe(.0))]
    Incompatible(Incompatible),
    /// A setting of the thread could not be written.
    #[error("a setting of the thread could not be written: {0}")]
    Setting(String),
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<orch_db::DbError> for RebindError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_authz::Error> for RebindError {
    fn from(error: orch_authz::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// Checks a model and an effort the caller chose against what the provider lists and the place
/// allows, as setting them does.
async fn check_model_choice(
    conn: &mut orch_db::Conn,
    provider: &ProviderRow,
    settings: &Resolved,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<(), RebindError> {
    let Some(model) = model else {
        return Ok(());
    };
    let row = resources::model(conn, provider.id, model).await?;
    crate::models::check_allowed(row.as_ref(), settings.models_allowed.as_deref(), model)
        .map_err(|_| RebindError::ModelNotAllowed(model.to_owned()))?;
    let row = row.ok_or_else(|| RebindError::ModelNotAllowed(model.to_owned()))?;
    if let Some(effort) = effort {
        let parameters = model_parameters(&row)?;
        if !parameters.efforts().iter().any(|listed| listed == effort) {
            return Err(RebindError::EffortNotListed(effort.to_owned()));
        }
    }
    Ok(())
}

impl Engine {
    /// The thread, locked, when it is one that may be put on something else by whoever manages it:
    /// not gone, not archived, not a side thread, and with no history yet. Whoever puts a thread
    /// on a provider, a profile or an environment is checked for it then and never again, so that
    /// is all the thread itself is asked: that `thread.manage` is held.
    async fn lock_for_binding(
        &self,
        conn: &mut orch_db::Conn,
        principal: &Principal,
        thread: Id,
    ) -> Result<threads::ThreadRow, RebindError> {
        let row = threads::lock(&mut *conn, thread)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or(RebindError::NoSuchThread)?;
        let decision = nodes::check(
            &mut *conn,
            principal,
            thread,
            Permission::ThreadManage,
            Act::Use,
            orch_authz::Policy::default(),
        )
        .await;
        match decision {
            Ok(Decision::Allowed { .. }) => {}
            Ok(Decision::Refused(refusal)) => return Err(RebindError::Denied(Box::new(refusal))),
            Err(orch_authz::Error::NotFound(_)) => return Err(RebindError::NoSuchThread),
            Err(orch_authz::Error::Archived { .. }) => return Err(RebindError::Archived),
            Err(orch_authz::Error::Suspended { .. }) => {
                return Err(RebindError::Denied(Box::new(Refusal::of(
                    Permission::ThreadManage,
                    ObjectRef {
                        kind: ObjectKind::Thread,
                        id: thread,
                    },
                    Vec::new(),
                ))));
            }
            Err(error) => return Err(error.into()),
        }
        if row.archived_at.is_some() {
            return Err(RebindError::Archived);
        }
        if row.side_of.is_some() {
            return Err(RebindError::SideThread);
        }
        if threads::binding::has_history(&mut *conn, thread).await? {
            return Err(RebindError::UseSwitch);
        }
        Ok(row)
    }

    /// Puts a thread that has no history on another agent profile and provider (and so on another
    /// arrangement), with a model and an effort if the caller names them. The caller needs
    /// `thread.manage` on the thread and their own use of the profile and of the provider with the
    /// model and effort; each is asked, and what is lacking is told whole. The thread's first
    /// segment, which no session was opened in, is put on the same. A thread with history is
    /// changed by a switch, and is refused.
    pub async fn bind_thread(
        &self,
        caller: &Caller,
        thread: Id,
        new: Rebind,
    ) -> Result<Rebound, RebindError> {
        self.bind_thread_from(None, caller, thread, new).await
    }

    /// As [`Engine::bind_thread`], in the transaction of the API's action guard. An unreported harness
    /// is returned to the caller so discovery and a fresh credential guard remain its responsibility.
    pub async fn bind_thread_guarded(
        &self,
        tx: orch_db::Tx,
        principal: Principal,
        thread: Id,
        new: Rebind,
    ) -> Result<Rebound, RebindError> {
        let caller = principal.caller().clone();
        self.bind_thread_from(Some((tx, principal)), &caller, thread, new)
            .await
    }

    async fn bind_thread_from(
        &self,
        mut guarded: Option<(orch_db::Tx, Principal)>,
        caller: &Caller,
        thread: Id,
        new: Rebind,
    ) -> Result<Rebound, RebindError> {
        let may_discover = guarded.is_none();
        let mut learned = false;
        loop {
            let (mut tx, principal) = match guarded.take() {
                Some(given) => given,
                None => {
                    let mut tx = self.inner.db.begin().await?;
                    orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
                    let principal =
                        Principal::load(&mut tx, caller.clone(), Time::new(self.inner.clock.now()))
                            .await?;
                    (tx, principal)
                }
            };
            match self.bind_in(&mut tx, &principal, thread, &new).await {
                Ok(bound) => {
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                    self.wake(thread);
                    return Ok(bound);
                }
                // As when a thread is made: the caller may use what they chose, and the harness is
                // started once to say what it is.
                Err(RebindError::AgentNotReported) if may_discover && !learned => {
                    drop(tx);
                    learned = true;
                    let environment = {
                        let mut conn = self.inner.db.acquire().await?;
                        threads::thread(&mut conn, thread)
                            .await?
                            .and_then(|row| row.environment_id)
                    };
                    match self
                        .learn_descriptor(new.agent, new.provider, environment)
                        .await?
                    {
                        Learning::Known => {}
                        Learning::Unknown(_) => return Err(RebindError::AgentNotReported),
                    }
                }
                Err(other) => return Err(other),
            }
        }
    }

    async fn bind_in(
        &self,
        tx: &mut orch_db::Tx,
        principal: &Principal,
        thread_id: Id,
        new: &Rebind,
    ) -> Result<Rebound, RebindError> {
        let conn: &mut orch_db::Conn = &mut *tx;
        let person = principal.user();
        let now = principal.now().get();
        let thread = self
            .lock_for_binding(&mut *conn, principal, thread_id)
            .await?;
        let agent = resources::agent(&mut *conn, new.agent)
            .await?
            .filter(|agent| agent.deleted_at.is_none() && !agent.disabled)
            .ok_or(RebindError::AgentUnavailable)?;
        let provider = resources::provider(&mut *conn, new.provider)
            .await?
            .filter(|provider| provider.deleted_at.is_none() && provider.status != "disabled")
            .ok_or(RebindError::ProviderUnavailable)?;
        let environment_id = thread.environment_id.ok_or(RebindError::NoEnvironment)?;
        let environment = environments::environment(&mut *conn, environment_id)
            .await?
            .filter(|environment| environment.deleted_at.is_none())
            .ok_or(RebindError::EnvironmentUnavailable)?;

        // The caller's own use of what the thread is put on, all of it at once: a refusal tells
        // everything that is lacking, not only the first thing.
        let mut lacking: Vec<(Permission, ObjectKind, Id, Vec<MissingAccess>)> = Vec::new();
        let agent_answer = self
            .inner
            .access
            .agent_use(&mut *conn, person, thread_id, agent.id)
            .await
            .map_err(EngineError::from)?;
        if let Answer::Refused(missing) = agent_answer {
            lacking.push((Permission::AgentUse, ObjectKind::Agent, agent.id, missing));
        }
        let provider_answer = self
            .inner
            .access
            .provider_use(
                &mut *conn,
                &ProviderUse {
                    person,
                    node: thread_id,
                    provider: provider.id,
                    model: new.model.as_deref(),
                    effort: new.effort.as_deref(),
                },
            )
            .await
            .map_err(EngineError::from)?;
        let entry = match &provider_answer {
            Answer::Allowed { arrangements } => arrangements.clone(),
            Answer::Refused(_) => None,
        };
        if let Answer::Refused(missing) = provider_answer {
            lacking.push((
                Permission::ProviderUse,
                ObjectKind::Provider,
                provider.id,
                missing,
            ));
        }
        if let Some((permission, kind, id, _)) = lacking.first() {
            let missing = lacking
                .iter()
                .flat_map(|(_, _, _, missing)| missing.iter().cloned())
                .collect();
            return Err(RebindError::Denied(Box::new(Refusal::of(
                *permission,
                ObjectRef {
                    kind: *kind,
                    id: *id,
                },
                missing,
            ))));
        }

        let descriptor = harnesses::descriptor_of(&agent)?.ok_or(RebindError::AgentNotReported)?;
        let settings = resolved::load(&mut *conn, thread_id).await?;
        if let Some(why) =
            crate::provider_policy::forbidden(&mut *conn, thread_id, provider.id).await?
        {
            return Err(RebindError::ProviderNotAllowed(why));
        }
        check_model_choice(
            &mut *conn,
            &provider,
            &settings,
            new.model.as_deref(),
            new.effort.as_deref(),
        )
        .await?;
        let place = environment_offers(&mut *conn, &environment).await?;
        let machine = machine_arrangements_of(&mut *conn, &provider).await?;
        let facts = Facts {
            descriptor: &descriptor,
            provider: &provider,
            environment: &environment,
            offers: place.offers,
            settings: &settings,
            entry,
            colocated: place.colocated,
            runtime: runtime_facts(conn, &agent, &provider, &environment).await?,
        };
        let arrangement = choose_with_machine(&facts, machine, new.arrangement)
            .map_err(RebindError::Incompatible)?;

        let mut unit = Unit::new(&mut *conn, now);
        let seq = unit.stamp(thread_id).await?;
        write_binding(
            &mut unit,
            &thread,
            &agent,
            &provider,
            &environment,
            &arrangement,
            person,
            seq,
        )
        .await?;
        for (key, value) in [
            (keys::AGENT_MODEL, &new.model),
            (keys::AGENT_EFFORT, &new.effort),
        ] {
            if let Some(value) = value {
                orch_settings::write::write(
                    &mut *unit.conn,
                    &orch_settings::write::Write {
                        actor: Actor::User(person),
                        target: Target::Node(thread_id),
                        key,
                        value: Json::String(value.clone()),
                        locked: None,
                        expected_revision: None,
                        may_lock: false,
                        origin: &Origin::default(),
                    },
                    now,
                )
                .await
                .map_err(|error| RebindError::Setting(error.to_string()))?;
            }
        }
        record_bound(
            &mut unit,
            person,
            &thread,
            &agent,
            &provider,
            &environment,
            &arrangement,
            (new.model.as_deref(), new.effort.as_deref()),
        )
        .await?;
        let finished = unit.finish().await?;
        Ok(Rebound {
            thread: thread_id,
            arrangement: Some(arrangement),
            directory: thread.workspace_dir.clone(),
            seq: finished.touched.get(&thread_id).copied().unwrap_or(seq),
        })
    }

    /// Puts a thread that has no history into another environment, in a directory of it. The
    /// caller needs `thread.manage` on the thread and `environment.manage` on the environment it
    /// leaves and on the one it enters, and the thread's agent must still be able to work there:
    /// the arrangement is worked out again for the new environment, keeping the type if it can be
    /// kept. A thread with history is changed by a switch, and is refused.
    pub async fn place_thread(
        &self,
        caller: &Caller,
        thread: Id,
        to: Replace,
    ) -> Result<Rebound, RebindError> {
        let mut tx = self.inner.db.begin().await?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        let now = self.inner.clock.now();
        let principal = Principal::load(&mut tx, caller.clone(), Time::new(now)).await?;
        let placed = self.place_in(&mut tx, &principal, thread, &to).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.wake(thread);
        Ok(placed)
    }

    /// As [`Engine::place_thread`], in the transaction of the API's action guard, with its caller and its time.
    pub async fn place_thread_guarded(
        &self,
        mut tx: orch_db::Tx,
        principal: Principal,
        thread: Id,
        to: Replace,
    ) -> Result<Rebound, RebindError> {
        let placed = self.place_in(&mut tx, &principal, thread, &to).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.wake(thread);
        Ok(placed)
    }

    async fn place_in(
        &self,
        tx: &mut orch_db::Tx,
        principal: &Principal,
        thread_id: Id,
        to: &Replace,
    ) -> Result<Rebound, RebindError> {
        let conn: &mut orch_db::Conn = &mut *tx;
        let person = principal.user();
        let now = principal.now().get();
        let thread = self
            .lock_for_binding(&mut *conn, principal, thread_id)
            .await?;
        if let Some(directory) = to.directory.as_deref()
            && !directory_is_valid(directory)
        {
            return Err(RebindError::InvalidDirectory);
        }
        if !orch_db::queries::loss::accepts_threads(conn, to.environment).await? {
            return Err(RebindError::EnvironmentUnavailable);
        }
        let environment = environments::environment(&mut *conn, to.environment)
            .await?
            .filter(|environment| {
                environment.deleted_at.is_none()
                    && environment.remove_at.is_none()
                    && !matches!(environment.status.as_str(), "lost" | "removed")
            })
            .ok_or(RebindError::EnvironmentUnavailable)?;
        let decision = orch_authz::environments::check_move(
            &mut *conn,
            principal,
            &orch_authz::environments::Moving {
                thread: Some(thread_id),
                from: thread.environment_id,
                to: Some(environment.id),
                all_of_the_environment: false,
            },
            orch_authz::Policy::default(),
        )
        .await?;
        if let Decision::Refused(refusal) = decision {
            return Err(RebindError::Denied(Box::new(refusal)));
        }
        let directory = match to.directory.clone() {
            Some(directory) => directory,
            None => {
                let topic_name = threads::node_name(&mut *conn, thread.topic_id)
                    .await?
                    .unwrap_or_default();
                let uses = threads::binding::environment_uses(
                    &mut *conn,
                    &[environment.id],
                    thread.topic_id,
                )
                .await?;
                default_directory(&topic_name, uses.first())
            }
        };
        let (Some(agent_id), Some(provider_id)) = (thread.agent_id, thread.provider_id) else {
            // A thread that is bound to nothing is put in the environment, and waits for a binding.
            let mut unit = Unit::new(&mut *conn, now);
            let seq = unit.stamp(thread_id).await?;
            threads::set_environment(&mut *unit.conn, thread_id, environment.id, &directory, seq)
                .await?;
            unit.finish().await?;
            return Ok(Rebound {
                thread: thread_id,
                arrangement: None,
                directory,
                seq,
            });
        };
        let agent = resources::agent(&mut *conn, agent_id)
            .await?
            .filter(|agent| agent.deleted_at.is_none())
            .ok_or(RebindError::AgentUnavailable)?;
        let provider = resources::provider(&mut *conn, provider_id)
            .await?
            .filter(|provider| provider.deleted_at.is_none())
            .ok_or(RebindError::ProviderUnavailable)?;
        let descriptor = harnesses::descriptor_of(&agent)?.ok_or(RebindError::AgentNotReported)?;
        let settings = resolved::load(&mut *conn, thread_id).await?;
        let place = environment_offers(&mut *conn, &environment).await?;
        let machine = machine_arrangements_of(&mut *conn, &provider).await?;
        // Whoever asked of the provider was checked when the thread was put on it; what the
        // entry they used then narrowed to is kept in what the provider's owner allows now.
        let facts = Facts {
            descriptor: &descriptor,
            provider: &provider,
            environment: &environment,
            offers: place.offers,
            settings: &settings,
            entry: None,
            colocated: place.colocated,
            runtime: runtime_facts(conn, &agent, &provider, &environment).await?,
        };
        let kept = parse(thread.arrangement.as_deref().unwrap_or_default());
        let arrangement = match choose_with_machine(&facts, machine.clone(), kept) {
            Ok(found) => found,
            Err(_) => {
                choose_with_machine(&facts, machine, None).map_err(RebindError::Incompatible)?
            }
        };
        let mut unit = Unit::new(&mut *conn, now);
        let seq = unit.stamp(thread_id).await?;
        threads::set_environment(&mut *unit.conn, thread_id, environment.id, &directory, seq)
            .await?;
        write_binding(
            &mut unit,
            &thread,
            &agent,
            &provider,
            &environment,
            &arrangement,
            person,
            seq,
        )
        .await?;
        record_bound(
            &mut unit,
            person,
            &thread,
            &agent,
            &provider,
            &environment,
            &arrangement,
            (None, None),
        )
        .await?;
        let finished = unit.finish().await?;
        Ok(Rebound {
            thread: thread_id,
            arrangement: Some(arrangement),
            directory,
            seq: finished.touched.get(&thread_id).copied().unwrap_or(seq),
        })
    }
}

/// Writes what a thread is put on: its row, and its first segment, which no session was opened in.
#[allow(
    clippy::too_many_arguments,
    reason = "the first segment records the chosen resources and the person who set them up"
)]
async fn write_binding(
    unit: &mut Unit<'_>,
    thread: &threads::ThreadRow,
    agent: &AgentRow,
    provider: &ProviderRow,
    environment: &EnvironmentRow,
    arrangement: &Arrangement,
    person: Id,
    seq: Seq,
) -> Result<(), EngineError> {
    threads::set_binding(
        &mut *unit.conn,
        thread.node_id,
        &threads::Binding {
            agent: agent.id,
            provider: provider.id,
            arrangement: arrangement.kind.as_str(),
            executor: arrangement.executor.as_deref(),
            tool_set: arrangement.tool_set.as_deref(),
        },
        seq,
    )
    .await?;
    if let Some(segment) = thread.current_segment_id {
        let path = selected_path(arrangement, provider, agent);
        threads::binding::rebind_segment(
            &mut *unit.conn,
            segment,
            &SegmentBinding {
                agent: agent.id,
                provider: provider.id,
                environment: environment.id,
                env_generation: environment.generation,
                arrangement: arrangement.kind.as_str(),
                executor: arrangement.executor.as_deref(),
                tool_set: arrangement.tool_set.as_deref(),
                credential_path: credential_path_word(path),
            },
            seq,
        )
        .await?;
    } else {
        let path = selected_path(arrangement, provider, agent);
        let started_at = unit.now();
        let segment = threads::insert_segment(
            unit.conn,
            &NewSegment {
                id: new_id(),
                thread_id: thread.node_id,
                agent_id: agent.id,
                provider_id: provider.id,
                environment_id: environment.id,
                env_generation: environment.generation,
                arrangement: arrangement.kind.as_str(),
                executor: arrangement.executor.as_deref(),
                tool_set: arrangement.tool_set.as_deref(),
                credential_path: credential_path_word(path),
                native_session_ref: None,
                started_by: Some(person),
                started_at,
            },
            seq,
        )
        .await?;
        threads::set_current(unit.conn, thread.node_id, Some(segment.id), None, seq).await?;
        if thread.blocked_reason.as_deref() == Some("unbound") {
            threads::set_state(
                unit.conn,
                thread.node_id,
                &[threads::thread_state::BLOCKED],
                &threads::StateChange::plain(threads::thread_state::IDLE),
                seq,
            )
            .await?;
        }
    }
    Ok(())
}

/// The line a thread's history gets when it is put on a provider, a profile and an environment.
#[allow(
    clippy::too_many_arguments,
    reason = "everything the line says of what the thread is put on"
)]
async fn record_bound(
    unit: &mut Unit<'_>,
    person: Id,
    thread: &threads::ThreadRow,
    agent: &AgentRow,
    provider: &ProviderRow,
    environment: &EnvironmentRow,
    arrangement: &Arrangement,
    (model, effort): (Option<&str>, Option<&str>),
) -> Result<(), EngineError> {
    let mut entry = Entry::new(
        Actor::User(person),
        orch_db::audit::action!(AuditAction::ThreadBound).as_str(),
        "thread",
        Some(thread.node_id),
        "put a thread on a provider, a profile and an environment",
    );
    entry.history = AuditAction::ThreadBound.is_history_line();
    entry.node_id = Some(thread.node_id);
    entry.environment_id = Some(environment.id);
    entry.details = json!({
        "agent": agent.id,
        "provider": provider.id,
        "environment": environment.id,
        "arrangement": arrangement.kind.as_str(),
        "model": model,
        "effort": effort,
    });
    let now = unit.now();
    audit::append(&mut *unit.conn, &entry, &Origin::default(), now).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_supervised_adapter_supplies_its_own_harness_but_an_older_report_still_needs_an_allowed_binary()
     {
        let mut machine = MachinePlace {
            id: Id::nil(),
            roles: vec!["agent".into()],
            status: "active".into(),
            online: true,
            placement: "connected".into(),
            owner_id: None,
            deleted_at: None,
            facts: json!({"adapters":[{"kind":"custom","needs_binary":false}],"harnesses":[],"policy":{"agent.harnesses":[]}}),
        };
        assert!(machine_has_harness(&machine, "custom"));
        assert!(!machine_has_harness(&machine, "unregistered"));
        machine.facts["adapters"][0]
            .as_object_mut()
            .unwrap()
            .remove("needs_binary");
        assert!(!machine_has_harness(&machine, "custom"));
    }

    #[test]
    fn harness_and_credential_policy_answers_are_identical_for_both_display_placements() {
        for placement in ["connected", "managed"] {
            let mut machine = MachinePlace {
                id: Id::nil(),
                roles: vec!["agent".into(), "provider".into()],
                status: "active".into(),
                online: true,
                placement: placement.into(),
                owner_id: None,
                deleted_at: None,
                facts: json!({"adapters":[{"kind":"fake"}],"harnesses":[{"kind":"fake"}],"policy":{"agent.harnesses":[],"provider.arrangements":["key_separated"]}}),
            };
            assert!(!machine_has_harness(&machine, "fake"));
            assert_eq!(
                arrangements_of_machine(&machine),
                Some(vec![ArrangementType::KeySeparated])
            );
            machine.facts["policy"]["agent.harnesses"] =
                json!([{"kind":"fake","path":"/bin/fake"}]);
            assert!(machine_has_harness(&machine, "fake"));
            machine.facts.as_object_mut().unwrap().remove("policy");
            assert!(machine_has_harness(&machine, "fake"));
            assert_eq!(arrangements_of_machine(&machine), None);
        }
    }

    #[test]
    fn a_directory_stays_under_the_workspace_root() {
        assert!(directory_is_valid(""));
        assert!(directory_is_valid("web/api"));
        assert!(!directory_is_valid("/etc"));
        assert!(!directory_is_valid("web/../.."));
        assert!(!directory_is_valid("web//api"));
        assert!(!directory_is_valid("./web"));
    }

    #[test]
    fn a_topic_names_its_directory_in_lower_case_letters_and_dashes() {
        assert_eq!(directory_named_after("Fix the Login!"), "fix-the-login");
        assert_eq!(directory_named_after("  API v2 / Beta "), "api-v2-beta");
        assert_eq!(directory_named_after("!!!"), "topic");
        assert!(directory_is_valid(&directory_named_after("Ünïcode name")));
    }
}
