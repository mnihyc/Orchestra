//! Which arrangement a thread gets: what is available from the declarations and the facts, in what order, and what is permitted.
//!
//! A thread's agent is three parts: the harness, what carries out its tool calls (an executor), and the
//! credential. An **arrangement** is one way to place them, of five types ([`ArrangementType`]). Nothing
//! else ranks, limits or names the ways an agent can be placed, and nothing in this module names a
//! harness, a vendor or an executor: it reads the adapter's descriptor, what the provider's side can issue
//! and serve, and what the environment's facts list.
//!
//! The choice is made in these steps:
//!
//! 1. **Available** ([`candidates`]): each type the adapter declares, in the adapter's order, is available
//!    or has the reason it is not ([`Incompatible`]). A `separated` entry is available when the
//!    environment offers something the entry can work through; the thread takes the first offer of the
//!    environment, in the environment's order, that an entry can work through, and nobody names it.
//! 2. **Order**: the types the resolved `binding.prefer` lists come first, in its order, and the others
//!    follow in the adapter's. Within `credential_shared` a form that lapses by itself comes before one
//!    that lasts.
//! 3. **Permitted** ([`Permission`]): the provider's owner allows the type, and the entry the thread is
//!    bound through, a scope's `binding.allowed`, and the machine that keeps the credential, each where it
//!    says anything. `together` is asked only of `binding.allowed`.
//! 4. A thread takes the first available and permitted arrangement ([`choose`]). Where a request names a
//!    type, an arrangement of that type must be available and permitted. When there is none, the answer is
//!    the reason the first candidate failed for.

use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::credentials::{CredentialForm, CredentialPath, Custody, usable_forms};
use crate::descriptor::{AgentDescriptor, ArrangementEntry, ArrangementType, ExecutorBuild};
use crate::facts::ExecutorFact;

/// Where a provider's credential is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialLocation {
    /// With the provider, in its runtime.
    AtHome,
    /// Handed over to this environment.
    HeldBy(Id),
}

/// One model access the provider serves, independently of how it keeps its credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Serves {
    /// Its harness's own sign-in, named by the native provider declaration.
    Native {
        /// The native provider's harness kind.
        harness: String,
    },
    /// Model requests in this protocol, including protocols a translator supplies.
    Forwarded {
        /// The protocol the provider or its translator serves.
        protocol: String,
    },
}

/// What the provider's side is, for the choice of an arrangement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSide {
    /// Every native harness or forwarded protocol its access element serves.
    pub serves: Vec<Serves>,
    /// The forms its side can issue: the adapter's `credentials.issues` for a subscription, the wire
    /// protocol's forms for an API key.
    pub issues: Vec<CredentialForm>,
    /// Where its credential is.
    pub credential: CredentialLocation,
}

/// What the thread's environment is, for the choice of an arrangement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceSide {
    /// The environment.
    pub environment_id: Id,
    /// What its exec node offers, in the order to try them (`exec.offers`): executors and tool sets of
    /// Orch's own, by name.
    pub offers: Vec<String>,
    /// The executors its facts list.
    pub executors: Vec<ExecutorFact>,
    /// Whether the environment can run the harness: it is the machine the provider is connected on, or a
    /// managed environment whose template lists the harness kind, or a connected one whose machine lists it
    /// in local policy.
    pub can_run_harness: bool,
    /// Whether the provider's runtime and the environment are one machine.
    pub same_machine: bool,
    /// The build of the harness that would run there, when it differs from the descriptor's.
    pub harness_version: Option<String>,
}

/// Why no arrangement is available and permitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Incompatible {
    /// The provider serves another native harness (or the adapter declares no native access).
    HarnessMismatch,
    /// The harness and the provider speak no protocol in common.
    NoCommonProtocol,
    /// No credential form is on both sides' lists.
    NoCredentialForm,
    /// The type is not allowed, by the party named.
    ArrangementNotAllowed {
        /// Who does not allow it.
        by: NotAllowedBy,
    },
    /// The provider's credential is with another environment.
    HandedOverElsewhere,
    /// The environment cannot run the harness: the profile belongs to another machine.
    AgentNotOnThatMachine,
    /// The environment offers nothing this harness can work through.
    ExecutorMissing {
        /// What the environment offers, in its order.
        offers: Vec<String>,
        /// What the harness could use: the executors and tool sets its entries name.
        usable: Vec<String>,
    },
    /// The provider's runtime and the environment are not one machine.
    ColocationImpossible,
}

impl Incompatible {
    /// The code, as `binding.incompatible` states it.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::HarnessMismatch => "harness_mismatch",
            Self::NoCommonProtocol => "no_common_protocol",
            Self::NoCredentialForm => "no_credential_form",
            Self::ArrangementNotAllowed { .. } => "arrangement_not_allowed",
            Self::HandedOverElsewhere => "handed_over_elsewhere",
            Self::AgentNotOnThatMachine => "agent_not_on_that_machine",
            Self::ExecutorMissing { .. } => "executor_missing",
            Self::ColocationImpossible => "colocation_impossible",
        }
    }
}

/// Who does not allow an arrangement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotAllowedBy {
    /// The provider's owner.
    Provider,
    /// The entry the thread is bound through.
    Entry,
    /// A policy (`binding.allowed`).
    Policy,
    /// The machine that keeps the credential.
    Machine,
}

/// One arrangement as a thread would have it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Arrangement {
    /// The type.
    #[serde(rename = "type")]
    pub kind: ArrangementType,
    /// For `separated`: the executor that carries out the tool calls.
    pub executor: Option<String>,
    /// For an executor with several tool sets: the one the thread has.
    pub tool_set: Option<String>,
    /// For `credential_shared` and `handed_over`: the form of the credential that leaves.
    pub form: Option<CredentialForm>,
    /// The chosen forwarded protocol; native access has none.
    pub protocol: Option<String>,
}

impl Arrangement {
    fn of(kind: ArrangementType) -> Self {
        Self {
            kind,
            executor: None,
            tool_set: None,
            form: None,
            protocol: None,
        }
    }
}

/// How the model is held to the lease's allowlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelEnforcement {
    /// The forwarder refuses a model outside the lease.
    Wire,
    /// The model is checked at every turn start and passed explicitly; the harness talks to its upstream
    /// itself.
    Adapter,
}

/// Who reports what a thread uses, which decides how firmly a usage cap can hold it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Metering {
    /// Every request is counted where the key is.
    Wire,
    /// The harness reports its own totals while it works.
    Harness,
    /// Nothing is counted.
    None,
}

/// How the model is enforced on `path`: on the wire when the harness calls the forwarder, otherwise by the
/// adapter.
#[must_use]
pub const fn model_enforcement(path: CredentialPath) -> ModelEnforcement {
    match path {
        CredentialPath::Forwarded => ModelEnforcement::Wire,
        _ => ModelEnforcement::Adapter,
    }
}

/// Who meters use on `path`: the forwarder where the harness calls it, otherwise the harness when its
/// adapter declares `usage_tokens`, otherwise nobody.
#[must_use]
pub const fn metering(path: CredentialPath, usage_tokens: bool) -> Metering {
    match (path, usage_tokens) {
        (CredentialPath::Forwarded, _) => Metering::Wire,
        (_, true) => Metering::Harness,
        (_, false) => Metering::None,
    }
}

/// The model access of an arrangement: `native` means its native harness matched; `holds_login`
/// identifies that harness's own profile. Another profile on native access needs a shared credential.
#[must_use]
pub const fn credential_path(
    kind: ArrangementType,
    native: bool,
    holds_login: bool,
) -> CredentialPath {
    match kind {
        ArrangementType::CredentialShared => CredentialPath::Shared,
        ArrangementType::HandedOver => CredentialPath::Handover,
        ArrangementType::KeySeparated => CredentialPath::Forwarded,
        ArrangementType::Separated | ArrangementType::Together if native => {
            if holds_login {
                CredentialPath::Native
            } else {
                CredentialPath::Shared
            }
        }
        ArrangementType::Separated | ArrangementType::Together => CredentialPath::Forwarded,
    }
}

/// Whether the provider serves the native harness the adapter declares.
#[must_use]
pub fn native_matches(descriptor: &AgentDescriptor, serves: &[Serves]) -> bool {
    descriptor.native_provider.as_ref().is_some_and(|native| {
        serves.iter().any(|served| matches!(served, Serves::Native { harness } if harness == &native.harness_kind))
    })
}

/// The first protocol of the adapter's list (its order) that the provider serves.
fn common_protocol<'a>(protocols: &'a [String], serves: &[Serves]) -> Option<&'a String> {
    protocols
        .iter()
        .find(|protocol| serves.iter().any(|served| matches!(served, Serves::Forwarded { protocol: served } if served == *protocol)))
}

/// Whether a profile can pair with the served native harness or a forwarded/direct protocol it declares.
#[must_use]
pub fn pairs(descriptor: &AgentDescriptor, serves: &[Serves]) -> bool {
    native_matches(descriptor, serves)
        || common_protocol(&descriptor.forwarded_protocols, serves).is_some()
        || common_protocol(&descriptor.direct_protocols, serves).is_some()
}

fn no_reach(serves: &[Serves]) -> Incompatible {
    if serves
        .iter()
        .any(|served| matches!(served, Serves::Native { .. }))
        && !serves
            .iter()
            .any(|served| matches!(served, Serves::Forwarded { .. }))
    {
        Incompatible::HarnessMismatch
    } else {
        Incompatible::NoCommonProtocol
    }
}

/// The matching native harness is used directly; otherwise the first common forwarded protocol is used.
fn forwarded_reach(
    descriptor: &AgentDescriptor,
    provider: &ProviderSide,
) -> Result<Option<String>, Incompatible> {
    if native_matches(descriptor, &provider.serves) {
        Ok(None)
    } else {
        common_protocol(&descriptor.forwarded_protocols, &provider.serves)
            .cloned()
            .map(Some)
            .ok_or_else(|| no_reach(&provider.serves))
    }
}

/// What the entry can work through at the offer `offer`: the executor, and the tool set it comes to.
fn works_through(
    entry: &ArrangementEntry,
    offer: &str,
    place: &PlaceSide,
    harness_version: Option<&str>,
) -> Option<(String, Option<String>)> {
    let executor = entry.executor.as_deref()?;
    // An offer that names its executor (`orch-exec:plain`) is for that executor's tool set; one that does not is
    // an executor, or a tool set written alone as offers were before they named their executor.
    let offer = match offer.split_once(':') {
        Some((offered, set)) if offered == executor => set,
        Some(_) => return None,
        None => offer,
    };
    let fact = place
        .executors
        .iter()
        .find(|fact| fact.kind == executor && fact.available)?;
    let build = entry.executor_version.clone().unwrap_or(ExecutorBuild::Any);
    if !build.fits(&fact.version, harness_version) {
        return None;
    }
    let wanted = entry.tool_sets.as_deref();
    let offered = fact.tool_sets.as_deref();
    if offer == executor {
        let tool_set = match (offered, wanted) {
            (None | Some([]), _) => None,
            (Some(offered), Some(wanted)) => {
                Some(wanted.iter().find(|set| offered.contains(set))?.clone())
            }
            (Some(offered), None) => offered.first().cloned(),
        };
        return Some((executor.to_owned(), tool_set));
    }
    let is_a_set_of_the_executor = offered.is_some_and(|sets| sets.iter().any(|set| set == offer));
    let harness_can = wanted.is_none_or(|sets| sets.iter().any(|set| set == offer));
    (is_a_set_of_the_executor && harness_can).then(|| (executor.to_owned(), Some(offer.to_owned())))
}

/// Everything the harness's `separated` entries could use, by name.
fn usable_names(descriptor: &AgentDescriptor) -> Vec<String> {
    let mut names = Vec::new();
    for entry in descriptor.separated() {
        match (&entry.tool_sets, &entry.executor) {
            (Some(sets), _) => names.extend(sets.iter().cloned()),
            (None, Some(executor)) => names.push(executor.clone()),
            (None, None) => {}
        }
    }
    names.dedup();
    names
}

fn separated(
    descriptor: &AgentDescriptor,
    provider: &ProviderSide,
    place: &PlaceSide,
) -> Result<Arrangement, Incompatible> {
    let protocol = forwarded_reach(descriptor, provider)?;
    let harness_version = place
        .harness_version
        .as_deref()
        .or(Some(descriptor.harness.version.as_str()));
    for offer in &place.offers {
        for entry in descriptor.separated() {
            if let Some((executor, tool_set)) = works_through(entry, offer, place, harness_version)
            {
                return Ok(Arrangement {
                    executor: Some(executor),
                    tool_set,
                    protocol,
                    ..Arrangement::of(ArrangementType::Separated)
                });
            }
        }
    }
    Err(Incompatible::ExecutorMissing {
        offers: place.offers.clone(),
        usable: usable_names(descriptor),
    })
}

fn sharing_forms(
    descriptor: &AgentDescriptor,
    provider: &ProviderSide,
    custody: Custody,
) -> Result<Vec<CredentialForm>, Incompatible> {
    if !native_matches(descriptor, &provider.serves)
        && common_protocol(&descriptor.direct_protocols, &provider.serves).is_none()
    {
        return Err(no_reach(&provider.serves));
    }
    let forms = usable_forms(
        &provider.issues,
        &descriptor.credentials.accepts,
        Some(custody),
    );
    if forms.is_empty() {
        Err(Incompatible::NoCredentialForm)
    } else {
        Ok(forms)
    }
}

/// The candidates for one declared type, in the order they would be taken. Each is an arrangement or the
/// reason there is none of that type.
fn candidates_of(
    kind: ArrangementType,
    descriptor: &AgentDescriptor,
    provider: &ProviderSide,
    place: &PlaceSide,
) -> Vec<Result<Arrangement, Incompatible>> {
    let one = |result: Result<Arrangement, Incompatible>| vec![result];
    match kind {
        ArrangementType::Separated => one(separated(descriptor, provider, place)),
        ArrangementType::KeySeparated => one(common_protocol(
            &descriptor.forwarded_protocols,
            &provider.serves,
        )
        .cloned()
        .ok_or_else(|| no_reach(&provider.serves))
        .and_then(|protocol| {
            if place.can_run_harness {
                Ok(Arrangement {
                    protocol: Some(protocol),
                    ..Arrangement::of(kind)
                })
            } else {
                Err(Incompatible::AgentNotOnThatMachine)
            }
        })),
        ArrangementType::CredentialShared => {
            match sharing_forms(descriptor, provider, Custody::Shared) {
                Err(reason) => one(Err(reason)),
                Ok(_) if !place.can_run_harness => one(Err(Incompatible::AgentNotOnThatMachine)),
                Ok(forms) => forms
                    .into_iter()
                    .map(|form| {
                        Ok(Arrangement {
                            form: Some(form),
                            ..Arrangement::of(kind)
                        })
                    })
                    .collect(),
            }
        }
        ArrangementType::HandedOver => {
            one(
                sharing_forms(descriptor, provider, Custody::Transferred).and_then(|forms| {
                    if !place.can_run_harness {
                        return Err(Incompatible::AgentNotOnThatMachine);
                    }
                    match provider.credential {
                        CredentialLocation::HeldBy(holder) if holder != place.environment_id => {
                            Err(Incompatible::HandedOverElsewhere)
                        }
                        _ => Ok(Arrangement {
                            form: forms.into_iter().next(),
                            ..Arrangement::of(kind)
                        }),
                    }
                }),
            )
        }
        ArrangementType::Together => {
            one(forwarded_reach(descriptor, provider).and_then(|protocol| {
                if place.same_machine {
                    Ok(Arrangement {
                        protocol,
                        ..Arrangement::of(kind)
                    })
                } else {
                    Err(Incompatible::ColocationImpossible)
                }
            }))
        }
    }
}

/// Every arrangement the adapter declares, in the adapter's order, each as it would be or with the reason
/// it cannot be. A `separated` entry appears once, where the first of them stands; a type appears once
/// however many entries name it, except `credential_shared`, which appears once for each usable form.
#[must_use]
pub fn candidates(
    descriptor: &AgentDescriptor,
    provider: &ProviderSide,
    place: &PlaceSide,
) -> Vec<(ArrangementType, Result<Arrangement, Incompatible>)> {
    let mut seen: Vec<ArrangementType> = Vec::new();
    let mut all = Vec::new();
    for entry in &descriptor.arrangements {
        if seen.contains(&entry.kind) {
            continue;
        }
        seen.push(entry.kind);
        all.extend(
            candidates_of(entry.kind, descriptor, provider, place)
                .into_iter()
                .map(|result| (entry.kind, result)),
        );
    }
    all
}

/// The types that each party allows, where it says anything. `None` is a party that does not limit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Permission {
    /// What the provider's owner allows (`providers.arrangements`).
    pub provider: Vec<ArrangementType>,
    /// What the entry the thread is bound through allows, when it names any.
    pub entry: Option<Vec<ArrangementType>>,
    /// What the resolved `binding.allowed` lists, where a scope set it.
    pub binding_allowed: Option<Vec<ArrangementType>>,
    /// Where the credential is kept on a person's machine, that machine's own list.
    pub machine: Option<Vec<ArrangementType>>,
}

impl Permission {
    /// Whether an arrangement of `kind` is permitted, or who does not allow it. `together` is asked only of
    /// `binding.allowed`, since nothing leaves a machine that already has everything.
    pub fn allows(&self, kind: ArrangementType) -> Result<(), NotAllowedBy> {
        let has = |list: &[ArrangementType]| list.contains(&kind);
        if kind.needs_consent() {
            if !has(&self.provider) {
                return Err(NotAllowedBy::Provider);
            }
            if self.entry.as_deref().is_some_and(|list| !has(list)) {
                return Err(NotAllowedBy::Entry);
            }
        }
        if self
            .binding_allowed
            .as_deref()
            .is_some_and(|list| !has(list))
        {
            return Err(NotAllowedBy::Policy);
        }
        if kind.needs_consent() && self.machine.as_deref().is_some_and(|list| !has(list)) {
            return Err(NotAllowedBy::Machine);
        }
        Ok(())
    }
}

/// How a thread's arrangement is chosen, beyond what is available.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    /// The types the resolved `binding.prefer` lists, in its order; they come first.
    pub prefer: Vec<ArrangementType>,
    /// What each party allows.
    pub permission: Permission,
}

/// The arrangement a thread takes, or why there is none.
///
/// The candidates are taken in the adapter's order with the preferred types first. A thread takes the first
/// that is available and permitted; when `requested` names a type, only candidates of that type count.
/// Where nothing is available and permitted the answer is the reason the first candidate failed for: that
/// it was unavailable, or that it is not allowed and by whom.
pub fn choose(
    descriptor: &AgentDescriptor,
    provider: &ProviderSide,
    place: &PlaceSide,
    policy: &Policy,
    requested: Option<ArrangementType>,
) -> Result<Arrangement, Incompatible> {
    let mut all = candidates(descriptor, provider, place);
    all.retain(|(kind, _)| requested.is_none_or(|wanted| wanted == *kind));
    // The preferred types first, in their order; the others in the adapter's. The sort is stable.
    all.sort_by_key(|(kind, _)| {
        policy
            .prefer
            .iter()
            .position(|preferred| preferred == kind)
            .unwrap_or(policy.prefer.len())
    });
    let mut first_failure: Option<Incompatible> = None;
    for (kind, candidate) in all {
        let failure = match candidate {
            Ok(arrangement) => match policy.permission.allows(kind) {
                Ok(()) => return Ok(arrangement),
                Err(by) => Incompatible::ArrangementNotAllowed { by },
            },
            Err(reason) => reason,
        };
        first_failure.get_or_insert(failure);
    }
    Err(first_failure.unwrap_or(Incompatible::HarnessMismatch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::AgentDescriptor;
    use serde_json::{Value, json};

    fn id(n: u8) -> Id {
        format!("0199abcd-1234-7000-8000-0000000000{n:02}")
            .parse()
            .unwrap()
    }

    fn descriptor(arrangements: Value, extra: Value) -> AgentDescriptor {
        let mut base = json!({
            "kind": "h", "adapter_version": 1,
            "harness": { "name": "H", "version": "0.160.1", "supported_range": ">=0.160.0", "status": "supported" },
            "arrangements": arrangements,
            "native_provider": { "harness_kind": "h", "title": "H subscription", "arrangements": ["separated"] },
            "forwarded_protocols": ["openai-responses"],
            "approval_modes": ["auto"], "sandbox_modes": ["open"]
        });
        for (key, value) in extra.as_object().unwrap() {
            base[key] = value.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    /// The first built-in adapter's arrangements: through its own exec server, then the key, then together.
    fn codex_like() -> AgentDescriptor {
        descriptor(
            json!([
                { "type": "separated", "executor": "codex-exec-server", "executor_version": "same" },
                { "type": "key_separated" }, { "type": "together" }
            ]),
            json!({}),
        )
    }

    /// The second: the key, a copy (two forms), through Orch's executor in three tool sets, the login, together.
    fn claude_like() -> AgentDescriptor {
        descriptor(
            json!([
                { "type": "key_separated" },
                { "type": "credential_shared" },
                { "type": "separated", "executor": "orch-exec", "executor_version": "*",
                  "tool_sets": ["claude-code", "plain", "custom"] },
                { "type": "handed_over" }, { "type": "together" }
            ]),
            json!({
                "forwarded_protocols": ["anthropic-messages"],
                "direct_protocols": ["anthropic-messages"],
                "credentials": { "issues": [
                    { "form": "oauth_token", "custody": "shared", "lifetime_secs": null },
                    { "form": "access_token", "custody": "shared", "lifetime_secs": 3600 },
                    { "form": "login", "custody": "transferred", "lifetime_secs": null } ],
                  "accepts": ["access_token", "oauth_token", "login", "api_key"] }
            }),
        )
    }

    fn api(protocols: &[&str]) -> ProviderSide {
        ProviderSide {
            serves: protocols
                .iter()
                .map(|protocol| Serves::Forwarded {
                    protocol: (*protocol).to_owned(),
                })
                .collect(),
            issues: vec![CredentialForm::api_key()],
            credential: CredentialLocation::AtHome,
        }
    }

    fn subscription(descriptor: &AgentDescriptor) -> ProviderSide {
        ProviderSide {
            serves: vec![Serves::Native {
                harness: "h".into(),
            }],
            issues: descriptor.credentials.issues.clone(),
            credential: CredentialLocation::AtHome,
        }
    }

    fn fact(kind: &str, version: &str, tool_sets: Option<&[&str]>) -> ExecutorFact {
        ExecutorFact {
            kind: kind.into(),
            version: version.into(),
            available: true,
            reason: None,
            sandbox_modes: vec![],
            tool_sets: tool_sets.map(|sets| sets.iter().map(ToString::to_string).collect()),
        }
    }

    fn place(offers: &[&str], executors: Vec<ExecutorFact>) -> PlaceSide {
        PlaceSide {
            environment_id: id(9),
            offers: offers.iter().map(ToString::to_string).collect(),
            executors,
            can_run_harness: false,
            same_machine: false,
            harness_version: None,
        }
    }

    fn everything() -> Vec<ArrangementType> {
        ArrangementType::ALL.to_vec()
    }

    fn policy() -> Policy {
        Policy {
            prefer: vec![],
            permission: Permission {
                provider: everything(),
                ..Permission::default()
            },
        }
    }

    #[test]
    fn a_harness_with_its_own_exec_server_is_placed_through_it_when_the_environment_offers_the_same_build()
     {
        let d = codex_like();
        let p = subscription(&d);
        let e = place(
            &["codex-exec-server", "claude-code", "plain"],
            vec![
                fact("codex-exec-server", "0.160.1", None),
                fact(
                    "orch-exec",
                    "0.1.0",
                    Some(&["claude-code", "plain", "custom"]),
                ),
            ],
        );
        let got = choose(&d, &p, &e, &policy(), None).unwrap();
        assert_eq!(got.kind, ArrangementType::Separated);
        assert_eq!(got.executor.as_deref(), Some("codex-exec-server"));
        assert_eq!(got.tool_set, None);
        // The environment's executor is another build: not offered "for it" until it is updated.
        let older = place(
            &["codex-exec-server"],
            vec![fact("codex-exec-server", "0.159.0", None)],
        );
        assert_eq!(
            choose(&d, &p, &older, &policy(), None),
            Err(Incompatible::ExecutorMissing {
                offers: vec!["codex-exec-server".into()],
                usable: vec!["codex-exec-server".into()],
            })
        );
    }

    #[test]
    fn a_harness_with_no_exec_server_takes_the_first_tool_set_the_environment_lists_that_it_can_work_with()
     {
        let d = claude_like();
        let p = api(&["anthropic-messages"]);
        let mut e = place(
            &["codex-exec-server", "plain", "claude-code"],
            vec![fact("orch-exec", "0.1.0", Some(&["claude-code", "plain"]))],
        );
        let only_separated = Policy {
            permission: Permission {
                provider: vec![ArrangementType::Separated],
                ..Permission::default()
            },
            ..Policy::default()
        };
        let got = choose(&d, &p, &e, &only_separated, None).unwrap();
        assert_eq!(
            (got.executor.as_deref(), got.tool_set.as_deref()),
            (Some("orch-exec"), Some("plain"))
        );
        assert_eq!(got.protocol.as_deref(), Some("anthropic-messages"));
        // `custom` is offered by a template whose agents act through MCP servers alone, and is passed over
        // while no MCP server is enabled: the facts do not list it then.
        e.offers = vec!["custom".into(), "claude-code".into()];
        let got = choose(&d, &p, &e, &only_separated, None).unwrap();
        assert_eq!(got.tool_set.as_deref(), Some("claude-code"));
        e.executors = vec![fact(
            "orch-exec",
            "0.1.0",
            Some(&["claude-code", "plain", "custom"]),
        )];
        e.offers = vec!["custom".into(), "claude-code".into()];
        assert_eq!(
            choose(&d, &p, &e, &only_separated, None)
                .unwrap()
                .tool_set
                .as_deref(),
            Some("custom")
        );
    }

    #[test]
    fn an_environment_that_offers_nothing_the_harness_can_use_refuses_separated_naming_both_lists()
    {
        let d = claude_like();
        let p = api(&["anthropic-messages"]);
        let e = place(
            &["codex-exec-server"],
            vec![fact("orch-exec", "0.1.0", Some(&["plain"]))],
        );
        let only_separated = Policy {
            permission: Permission {
                provider: vec![ArrangementType::Separated],
                ..Permission::default()
            },
            ..Policy::default()
        };
        let Err(Incompatible::ExecutorMissing { offers, usable }) = choose(
            &d,
            &p,
            &e,
            &only_separated,
            Some(ArrangementType::Separated),
        ) else {
            panic!("the environment offers nothing it can use");
        };
        assert_eq!(offers, ["codex-exec-server"]);
        assert_eq!(usable, ["claude-code", "plain", "custom"]);
    }

    #[test]
    fn an_executor_that_is_not_available_here_is_passed_over() {
        let d = codex_like();
        let p = subscription(&d);
        let mut broken = fact("codex-exec-server", "0.160.1", None);
        broken.available = false;
        broken.reason = Some("the build is missing".into());
        let e = place(&["codex-exec-server"], vec![broken]);
        assert!(matches!(
            choose(&d, &p, &e, &policy(), Some(ArrangementType::Separated)),
            Err(Incompatible::ExecutorMissing { .. })
        ));
    }

    #[test]
    fn the_order_is_the_adapters_and_the_preferred_types_come_first() {
        let d = codex_like();
        let p = api(&["openai-responses"]);
        let mut e = place(
            &["codex-exec-server"],
            vec![fact("codex-exec-server", "0.160.1", None)],
        );
        e.can_run_harness = true;
        assert_eq!(
            choose(&d, &p, &e, &policy(), None).unwrap().kind,
            ArrangementType::Separated
        );
        let prefer = Policy {
            prefer: vec![ArrangementType::KeySeparated],
            ..policy()
        };
        let got = choose(&d, &p, &e, &prefer, None).unwrap();
        assert_eq!(got.kind, ArrangementType::KeySeparated);
        assert_eq!(got.protocol.as_deref(), Some("openai-responses"));
    }

    #[test]
    fn a_copy_that_lapses_comes_before_one_that_lasts() {
        let d = claude_like();
        let p = subscription(&d);
        let mut e = place(&[], vec![]);
        e.can_run_harness = true;
        let shared = Policy {
            permission: Permission {
                provider: vec![ArrangementType::CredentialShared],
                ..Permission::default()
            },
            ..Policy::default()
        };
        let forms: Vec<_> = candidates(&d, &p, &e)
            .into_iter()
            .filter(|(kind, _)| *kind == ArrangementType::CredentialShared)
            .map(|(_, result)| result.unwrap().form.unwrap().form)
            .collect();
        assert_eq!(forms, ["access_token", "oauth_token"]);
        let got = choose(&d, &p, &e, &shared, None).unwrap();
        assert_eq!(got.form.unwrap().form, "access_token");
    }

    #[test]
    fn what_the_owner_allows_is_a_consent_and_a_preference_does_not_override_it() {
        let d = claude_like();
        let p = subscription(&d);
        let mut e = place(&[], vec![]);
        e.can_run_harness = true;
        // The owner allows `separated` alone, which the environment cannot carry out: nothing is shared.
        let alone = Policy {
            prefer: vec![ArrangementType::CredentialShared],
            permission: Permission {
                provider: vec![ArrangementType::Separated],
                ..Permission::default()
            },
        };
        assert_eq!(
            choose(&d, &p, &e, &alone, None),
            Err(Incompatible::ArrangementNotAllowed {
                by: NotAllowedBy::Provider
            })
        );
        // The same by the entry, by a policy and by the machine.
        for (permission, by) in [
            (
                Permission {
                    provider: everything(),
                    entry: Some(vec![ArrangementType::Together]),
                    ..Permission::default()
                },
                NotAllowedBy::Entry,
            ),
            (
                Permission {
                    provider: everything(),
                    binding_allowed: Some(vec![ArrangementType::Together]),
                    ..Permission::default()
                },
                NotAllowedBy::Policy,
            ),
            (
                Permission {
                    provider: everything(),
                    machine: Some(vec![ArrangementType::Together]),
                    ..Permission::default()
                },
                NotAllowedBy::Machine,
            ),
        ] {
            let policy = Policy {
                permission,
                ..Policy::default()
            };
            assert_eq!(
                choose(&d, &p, &e, &policy, Some(ArrangementType::CredentialShared)),
                Err(Incompatible::ArrangementNotAllowed { by })
            );
        }
    }

    #[test]
    fn together_is_asked_only_of_the_policy_that_allows_it() {
        let d = codex_like();
        let p = subscription(&d);
        let mut e = place(&[], vec![]);
        e.same_machine = true;
        let nobody_consented = Policy {
            permission: Permission::default(),
            ..Policy::default()
        };
        let got = choose(&d, &p, &e, &nobody_consented, None).unwrap();
        assert_eq!(got.kind, ArrangementType::Together);
        let forbidden = Policy {
            permission: Permission {
                binding_allowed: Some(vec![ArrangementType::Separated]),
                ..Permission::default()
            },
            ..Policy::default()
        };
        assert_eq!(
            choose(&d, &p, &e, &forbidden, Some(ArrangementType::Together)),
            Err(Incompatible::ArrangementNotAllowed {
                by: NotAllowedBy::Policy
            })
        );
        e.same_machine = false;
        assert_eq!(
            choose(
                &d,
                &p,
                &e,
                &nobody_consented,
                Some(ArrangementType::Together)
            ),
            Err(Incompatible::ColocationImpossible)
        );
    }

    #[test]
    fn the_reason_for_nothing_is_the_reason_the_first_candidate_failed_for() {
        let d = codex_like();
        let e = place(&[], vec![]);
        // A subscription of another harness: every type fails for it, and the first says so.
        let other = ProviderSide {
            serves: vec![Serves::Native {
                harness: "other".into(),
            }],
            ..subscription(&d)
        };
        let got = choose(&d, &other, &e, &policy(), None);
        assert_eq!(got, Err(Incompatible::HarnessMismatch));
        assert_eq!(got.unwrap_err().code(), "harness_mismatch");
        // An API provider with no protocol in common.
        assert_eq!(
            choose(&d, &api(&["anthropic-messages"]), &e, &policy(), None),
            Err(Incompatible::NoCommonProtocol)
        );
        // Asked for a type the adapter does not declare.
        assert_eq!(
            choose(
                &d,
                &subscription(&d),
                &e,
                &policy(),
                Some(ArrangementType::HandedOver)
            ),
            Err(Incompatible::HarnessMismatch)
        );
    }

    #[test]
    fn a_credential_that_is_with_another_environment_cannot_be_handed_over_again() {
        let d = claude_like();
        let mut p = subscription(&d);
        let mut e = place(&[], vec![]);
        e.can_run_harness = true;
        let handed = Policy {
            permission: Permission {
                provider: vec![ArrangementType::HandedOver],
                ..Permission::default()
            },
            ..Policy::default()
        };
        let got = choose(&d, &p, &e, &handed, None).unwrap();
        assert_eq!(
            (got.kind, got.form.unwrap().form.as_str()),
            (ArrangementType::HandedOver, "login")
        );
        p.credential = CredentialLocation::HeldBy(id(9));
        assert_eq!(
            choose(&d, &p, &e, &handed, None).unwrap().kind,
            ArrangementType::HandedOver,
            "held by this environment"
        );
        p.credential = CredentialLocation::HeldBy(id(7));
        let asked = Some(ArrangementType::HandedOver);
        assert_eq!(
            choose(&d, &p, &e, &handed, asked),
            Err(Incompatible::HandedOverElsewhere)
        );
        e.can_run_harness = false;
        assert_eq!(
            choose(&d, &p, &e, &handed, asked),
            Err(Incompatible::AgentNotOnThatMachine)
        );
    }

    #[test]
    fn a_form_issued_from_a_key_needs_a_harness_that_speaks_the_upstream_protocol_itself() {
        let d = claude_like();
        let mut e = place(&[], vec![]);
        e.can_run_harness = true;
        let shared = Policy {
            permission: Permission {
                provider: vec![ArrangementType::CredentialShared],
                ..Permission::default()
            },
            ..Policy::default()
        };
        assert_eq!(
            choose(&d, &api(&["anthropic-messages"]), &e, &shared, None)
                .unwrap()
                .form
                .unwrap()
                .form,
            "api_key"
        );
        assert_eq!(
            choose(&d, &api(&["openai-responses"]), &e, &shared, None),
            Err(Incompatible::NoCommonProtocol)
        );
    }

    #[test]
    fn a_forwarded_service_pairs_even_when_a_different_native_harness_is_served_too() {
        let d = codex_like();
        let mut p = api(&["openai-responses"]);
        p.serves.insert(
            0,
            Serves::Native {
                harness: "another-harness".into(),
            },
        );
        let e = place(
            &["codex-exec-server"],
            vec![fact("codex-exec-server", "0.160.1", None)],
        );
        assert!(pairs(&d, &p.serves));
        let selected = choose(&d, &p, &e, &policy(), Some(ArrangementType::Separated)).unwrap();
        assert_eq!(selected.protocol.as_deref(), Some("openai-responses"));
        let path = credential_path(selected.kind, selected.protocol.is_none(), true);
        assert_eq!(
            path,
            CredentialPath::Forwarded,
            "owning the keeper does not turn a forwarded request into native access"
        );
        assert_eq!(model_enforcement(path), ModelEnforcement::Wire);
        assert_eq!(metering(path, true), Metering::Wire);
    }

    #[test]
    fn a_provider_serving_both_accesses_uses_the_one_its_selected_arrangement_requires() {
        let d = codex_like();
        let mut p = subscription(&d);
        p.serves.push(Serves::Forwarded {
            protocol: "openai-responses".into(),
        });
        let mut e = place(
            &["codex-exec-server"],
            vec![fact("codex-exec-server", "0.160.1", None)],
        );
        e.can_run_harness = true;
        assert!(pairs(&d, &p.serves));
        let native = choose(&d, &p, &e, &policy(), None).unwrap();
        assert_eq!(native.kind, ArrangementType::Separated);
        assert_eq!(native.protocol, None);
        assert_eq!(
            credential_path(native.kind, native.protocol.is_none(), true),
            CredentialPath::Native
        );
        assert_eq!(
            credential_path(native.kind, native.protocol.is_none(), false),
            CredentialPath::Shared
        );
        let forwarded = choose(&d, &p, &e, &policy(), Some(ArrangementType::KeySeparated)).unwrap();
        assert_eq!(forwarded.protocol.as_deref(), Some("openai-responses"));
        assert_eq!(
            credential_path(forwarded.kind, forwarded.protocol.is_none(), true),
            CredentialPath::Forwarded
        );
    }

    #[test]
    fn served_translator_protocols_follow_the_harness_order_and_sharing_requires_its_direct_protocol()
     {
        let mut d = claude_like();
        d.forwarded_protocols = vec!["translated-format".into(), "anthropic-messages".into()];
        d.direct_protocols = vec!["translated-format".into()];
        let mut p = api(&["anthropic-messages", "translated-format"]);
        p.serves.insert(
            0,
            Serves::Native {
                harness: "unrelated".into(),
            },
        );
        let mut e = place(&[], vec![]);
        e.can_run_harness = true;
        let forwarded = choose(&d, &p, &e, &policy(), Some(ArrangementType::KeySeparated)).unwrap();
        assert_eq!(
            forwarded.protocol.as_deref(),
            Some("translated-format"),
            "the harness's protocol order is preserved"
        );
        let shared = choose(
            &d,
            &p,
            &e,
            &policy(),
            Some(ArrangementType::CredentialShared),
        )
        .unwrap();
        assert_eq!(shared.form.unwrap().form, "api_key");
        d.direct_protocols.clear();
        assert_eq!(
            choose(
                &d,
                &p,
                &e,
                &policy(),
                Some(ArrangementType::CredentialShared)
            ),
            Err(Incompatible::NoCommonProtocol)
        );
    }

    #[test]
    fn colocation_and_accepted_forms_do_not_claim_a_native_service_the_harness_does_not_declare() {
        let mut d = codex_like();
        d.native_provider = None;
        d.credentials.accepts = vec!["api_key".into()];
        let mut p = subscription(&d);
        p.issues = vec![CredentialForm::api_key()];
        let mut e = place(&[], vec![]);
        e.same_machine = true;
        e.can_run_harness = true;
        assert!(!pairs(&d, &p.serves));
        assert_eq!(
            choose(&d, &p, &e, &policy(), Some(ArrangementType::Together)),
            Err(Incompatible::HarnessMismatch)
        );
        p.serves.clear();
        assert_eq!(
            choose(&d, &p, &e, &policy(), Some(ArrangementType::KeySeparated)),
            Err(Incompatible::NoCommonProtocol)
        );
    }

    #[test]
    fn how_the_model_is_enforced_and_use_is_metered_follow_from_the_path() {
        assert_eq!(
            model_enforcement(CredentialPath::Forwarded),
            ModelEnforcement::Wire
        );
        assert_eq!(
            model_enforcement(CredentialPath::Shared),
            ModelEnforcement::Adapter
        );
        assert_eq!(metering(CredentialPath::Forwarded, false), Metering::Wire);
        assert_eq!(metering(CredentialPath::Native, true), Metering::Harness);
        assert_eq!(metering(CredentialPath::Handover, false), Metering::None);
    }

    #[test]
    fn the_reasons_are_written_as_binding_incompatible_states_them() {
        let reasons = [
            (Incompatible::HarnessMismatch, "harness_mismatch"),
            (Incompatible::NoCommonProtocol, "no_common_protocol"),
            (Incompatible::NoCredentialForm, "no_credential_form"),
            (
                Incompatible::ArrangementNotAllowed {
                    by: NotAllowedBy::Provider,
                },
                "arrangement_not_allowed",
            ),
            (Incompatible::HandedOverElsewhere, "handed_over_elsewhere"),
            (
                Incompatible::AgentNotOnThatMachine,
                "agent_not_on_that_machine",
            ),
            (
                Incompatible::ExecutorMissing {
                    offers: vec![],
                    usable: vec![],
                },
                "executor_missing",
            ),
            (Incompatible::ColocationImpossible, "colocation_impossible"),
        ];
        for (reason, code) in reasons {
            assert_eq!(reason.code(), code);
            assert_eq!(serde_json::to_value(&reason).unwrap()["reason"], code);
        }
    }
}
