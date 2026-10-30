//! Parameter definitions, eligibility and model-driven options; what a harness offers for a model, apart from what a provider has enabled.
//!
//! An adapter publishes the parameters its harness accepts ([`ParameterDef`]). Whether one exists for a
//! thread is decided from the declaration and from facts about the thread's place and model, by this crate
//! and not by the engine: a parameter is **shown and sent** only when it is *eligible* (its condition holds
//! or it has none) and *enabled* (it names no feature, or the resolved `agent.features` has that feature
//! on). A parameter that is not shown is not sent, whatever value is stored; a stored value that is not among
//! the current options is not sent either, and is flagged.
//!
//! What a model offers lives in the provider's entry for it ([`ModelParameters`]). What costs more or
//! reaches further is never filled in by discovery: the service tiers beside the ordinary one, the
//! access programs and a context larger than the model starts with are listed as *offered*, and become the
//! model's own only when a manager of the provider enables them on that model. The entry has the same three
//! parts for each of its effort, service tier, access program and context: what the model offers there,
//! what its managers opened, and what a thread on it starts with. What a thread *does* run with is its own
//! setting, chosen within what is open; a choice that is no longer open is not sent and is flagged, and the
//! thread runs with what the model starts with.

use std::collections::BTreeMap;

use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::credentials::CredentialPath;
use crate::descriptor::{Placement, ProviderKind};

/// Where on a screen a parameter is put.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ParameterGroup {
    /// About the model.
    Model,
    /// About how the agent behaves.
    Behaviour,
    /// About what the agent may do.
    Safety,
    /// Everything else.
    Advanced,
}

/// What kind of value a parameter takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ParameterType {
    /// One of a list of values.
    Enum,
    /// Yes or no.
    Boolean,
    /// Text.
    String,
    /// A whole number.
    Integer,
}

/// One choice of an enumerated parameter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ParameterOption {
    /// What is stored and sent.
    pub value: String,
    /// What is shown.
    pub label: String,
    /// A sentence on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A list of the model's own that can be the options of a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelList {
    /// The reasoning efforts the model lists.
    Efforts,
    /// The service tiers a manager enabled on the model.
    ServiceTiers,
    /// The access programs a manager enabled on the model.
    AccessPrograms,
}

/// Where the options of a parameter come from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ParameterOptions {
    /// The model's own list.
    FromModel {
        /// Which list.
        from_model: ModelList,
    },
    /// A fixed list.
    Listed(Vec<ParameterOption>),
}

/// What a model gives as the default of a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DefaultFromModel {
    /// The model's default reasoning effort.
    DefaultEffort,
    /// The service tier a thread on the model starts with; none where the entry names none.
    DefaultServiceTier,
    /// The access program a thread on the model starts with; none where the entry names none.
    DefaultAccessProgram,
    /// The context a thread on the model starts with: the entry's `default_context`; left out, all
    /// that is open (`context_max`).
    DefaultContext,
}

/// What a model's entry limits a number by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelLimit {
    /// The largest context the managers of the provider opened on the model.
    ContextMax,
}

/// The most a ranged parameter may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RangeMax {
    /// This number.
    Fixed(i64),
    /// What the thread's model allows.
    FromModel {
        /// Which of the model's limits.
        from_model: ModelLimit,
    },
}

/// The least and the most an `integer` parameter may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ParameterRange {
    /// The least it may be: what the adapter's harness takes.
    pub min: i64,
    /// The most it may be.
    pub max: RangeMax,
}

/// The value a parameter has when nobody set one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ParameterDefault {
    /// What the model says.
    FromModel {
        /// Which of the model's values.
        from_model: DefaultFromModel,
    },
    /// This value; `null` sends nothing and leaves the harness to decide.
    Value(Json),
}

impl Default for ParameterDefault {
    fn default() -> Self {
        Self::Value(Json::Null)
    }
}

/// The scopes at which a parameter can be set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ParameterScope {
    /// An org, at any level.
    Org,
    /// A project.
    Project,
    /// A topic.
    Topic,
    /// A thread.
    Thread,
}

/// When a changed value takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ParameterApply {
    /// With the next turn.
    NextTurn,
    /// When the next session opens.
    NewSession,
    /// When the environment is recreated.
    RecreateEnvironment,
}

/// A list of a model's that a condition can ask about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelFeature {
    /// The reasoning efforts it lists.
    Efforts,
    /// The service tiers enabled on it.
    ServiceTiers,
    /// The access programs enabled on it.
    AccessPrograms,
    /// Whether it takes images.
    InputImages,
    /// Whether a context larger than the model's own window is open on it.
    ContextMax,
}

/// The lists of a model that a condition can ask whether a value is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelField {
    /// The reasoning efforts.
    Efforts,
    /// The service tiers, by id.
    ServiceTiers,
    /// The access programs.
    AccessPrograms,
}

/// A value of a model's list.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ModelIncludes {
    /// Which list.
    pub field: ModelField,
    /// The value looked for in it.
    pub value: String,
}

/// A condition on the place and the model of a thread, written as data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    /// Every one of them holds.
    All(Vec<Condition>),
    /// At least one of them holds.
    Any(Vec<Condition>),
    /// It does not hold.
    Not(Box<Condition>),
    /// The harness reaches the model by this path.
    CredentialPath(CredentialPath),
    /// The provider is of this kind.
    ProviderKind(ProviderKind),
    /// The environment is of this placement.
    EnvironmentPlacement(Placement),
    /// The model has something of this kind.
    ModelHas(ModelFeature),
    /// The model's list has this value.
    ModelIncludes(ModelIncludes),
    /// The harness that runs is in this semver range.
    HarnessVersion(String),
}

/// Model entries and rates are the API's types, including their eligibility helpers.
pub use orch_api_contract::resources::{
    ModelOffered as Offered, ModelParameters, Rate, RateAbove as RateStep, ServiceTier,
};

/// What is known when it is asked whether a parameter exists for a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Eligibility {
    /// How the harness reaches the model in the thread's arrangement.
    pub credential_path: CredentialPath,
    /// Whether the thread's provider is a subscription or an API upstream.
    pub provider_kind: ProviderKind,
    /// Whether the thread's environment is managed or a person's machine.
    pub environment_placement: Placement,
    /// What the provider's entry for the thread's model says.
    pub model: ModelParameters,
    /// The version of the harness that runs, when it is known.
    pub harness_version: Option<String>,
}

impl Condition {
    /// Whether the condition holds for `ctx`. An empty `all` holds, an empty `any` does not; a range that
    /// is no semver range, or a harness whose version is not known, does not hold.
    #[must_use]
    pub fn holds(&self, ctx: &Eligibility) -> bool {
        match self {
            Self::All(parts) => parts.iter().all(|part| part.holds(ctx)),
            Self::Any(parts) => parts.iter().any(|part| part.holds(ctx)),
            Self::Not(part) => !part.holds(ctx),
            Self::CredentialPath(path) => ctx.credential_path == *path,
            Self::ProviderKind(kind) => ctx.provider_kind == *kind,
            Self::EnvironmentPlacement(placement) => ctx.environment_placement == *placement,
            Self::ModelHas(feature) => match feature {
                ModelFeature::Efforts => !ctx.model.efforts().is_empty(),
                ModelFeature::ServiceTiers => !ctx.model.service_tiers().is_empty(),
                ModelFeature::AccessPrograms => !ctx.model.access_programs().is_empty(),
                ModelFeature::InputImages => ctx.model.takes_images(),
                ModelFeature::ContextMax => ctx.model.context_open().is_some(),
            },
            Self::ModelIncludes(ModelIncludes { field, value }) => match field {
                ModelField::Efforts => ctx.model.efforts().contains(value),
                ModelField::ServiceTiers => ctx
                    .model
                    .service_tiers()
                    .iter()
                    .any(|tier| &tier.id == value),
                ModelField::AccessPrograms => ctx.model.access_programs().contains(value),
            },
            Self::HarnessVersion(range) => ctx
                .harness_version
                .as_deref()
                .is_some_and(|version| crate::descriptor::version_in_range(version, range)),
        }
    }
}

/// One parameter that an adapter accepts for its harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ParameterDef {
    /// Its key, unique within the adapter.
    pub key: String,
    /// What it is called.
    pub title: String,
    /// What it does.
    pub description: String,
    /// Where it is put.
    pub group: ParameterGroup,
    /// What kind of value it takes.
    #[serde(rename = "type")]
    pub kind: ParameterType,
    /// The choices of an enumerated parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<ParameterOptions>,
    /// For an `integer`: the least and the most it may be. The most may come from the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<ParameterRange>,
    /// The value it has when nobody set one; `null` sends nothing and the harness decides.
    #[serde(default)]
    pub default: ParameterDefault,
    /// The scopes it can be set at.
    pub scopes: Vec<ParameterScope>,
    /// When a change takes effect.
    pub apply: ParameterApply,
    /// The condition under which it exists for a thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eligible_when: Option<Condition>,
    /// A feature that has to be on in the resolved `agent.features`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<String>,
}

/// What a parameter comes to for a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// It does not exist for the thread: not shown, not sent, whatever is stored.
    Hidden,
    /// This value is sent.
    Send(Json),
    /// Nothing is sent and the harness decides.
    SendNothing,
    /// The stored value is not among the current options, or not in the range the model allows now: not
    /// sent, and flagged on the screen.
    Stale(Json),
    /// The stored value is not of the parameter's type: not sent, and flagged.
    WrongType(Json),
}

/// A parameter that stands for something a thread has one of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadKind {
    /// The turn's service tier: its options come from the model's service tiers.
    ServiceTier,
    /// The thread's context: its range's most comes from the model's `context_max`.
    Context,
}

impl ParameterDef {
    /// Which of the things a thread has one of this parameter is, if it is one of them: the service
    /// tier (its options come from the model's `service_tiers`) or the context (its range's most
    /// comes from the model's `context_max`).
    #[must_use]
    pub fn kind_of_thread(&self) -> Option<ThreadKind> {
        if matches!(
            self.options,
            Some(ParameterOptions::FromModel {
                from_model: ModelList::ServiceTiers
            })
        ) {
            return Some(ThreadKind::ServiceTier);
        }
        matches!(
            self.range,
            Some(ParameterRange {
                max: RangeMax::FromModel {
                    from_model: ModelLimit::ContextMax
                },
                ..
            })
        )
        .then_some(ThreadKind::Context)
    }

    /// Whether none of it is a choice of its own: the ordinary tier among a model's service tiers,
    /// or no program among its access programs. A thread stores that choice as `null`, which is
    /// apart from storing nothing (and so running with what the model starts with): a model that
    /// starts in a faster tier or under a program leaves a thread a way to run without it.
    #[must_use]
    pub fn chooses_none(&self) -> bool {
        matches!(
            self.options,
            Some(ParameterOptions::FromModel {
                from_model: ModelList::ServiceTiers | ModelList::AccessPrograms
            })
        )
    }

    /// Whether its condition holds, or it has none.
    #[must_use]
    pub fn is_eligible(&self, ctx: &Eligibility) -> bool {
        self.eligible_when
            .as_ref()
            .is_none_or(|condition| condition.holds(ctx))
    }

    /// Whether it names no feature, or the resolved features have that one on.
    #[must_use]
    pub fn is_enabled(&self, features: &BTreeMap<String, bool>) -> bool {
        self.feature
            .as_ref()
            .is_none_or(|feature| features.get(feature) == Some(&true))
    }

    /// Whether it is shown and sent: eligible and enabled.
    #[must_use]
    pub fn is_shown(&self, ctx: &Eligibility, features: &BTreeMap<String, bool>) -> bool {
        self.is_eligible(ctx) && self.is_enabled(features)
    }

    /// The choices it has for `model`: its fixed list, or the model's own.
    #[must_use]
    pub fn options_for(&self, model: &ModelParameters) -> Vec<ParameterOption> {
        let plain = |value: &str, label: &str| ParameterOption {
            value: value.to_owned(),
            label: label.to_owned(),
            description: None,
        };
        match &self.options {
            None => Vec::new(),
            Some(ParameterOptions::Listed(listed)) => listed.clone(),
            Some(ParameterOptions::FromModel { from_model }) => match from_model {
                ModelList::Efforts => model
                    .efforts()
                    .iter()
                    .map(|effort| plain(effort, effort))
                    .collect(),
                ModelList::ServiceTiers => model
                    .service_tiers()
                    .iter()
                    .map(|tier| plain(&tier.id, &tier.label))
                    .collect(),
                ModelList::AccessPrograms => model
                    .access_programs()
                    .iter()
                    .map(|program| plain(program, program))
                    .collect(),
            },
        }
    }

    /// The value it has for `model` when nobody set one; `None` sends nothing.
    #[must_use]
    pub fn default_for(&self, model: &ModelParameters) -> Option<Json> {
        match &self.default {
            ParameterDefault::Value(Json::Null) => None,
            ParameterDefault::Value(value) => Some(value.clone()),
            ParameterDefault::FromModel { from_model } => match from_model {
                DefaultFromModel::DefaultEffort => model.default_effort.clone().map(Json::String),
                DefaultFromModel::DefaultServiceTier => model.service_tier_start().map(Json::from),
                DefaultFromModel::DefaultAccessProgram => {
                    model.access_program_start().map(Json::from)
                }
                DefaultFromModel::DefaultContext => model.context_start().map(Json::from),
            },
        }
    }

    /// The most the parameter may be for `model`: its range's, or what the model's entry allows.
    /// None where it has no range, or the range depends on a limit the model's entry does not set
    /// (nothing larger than its own window is open).
    #[must_use]
    pub fn range_max(&self, model: &ModelParameters) -> Option<i64> {
        match self.range?.max {
            RangeMax::Fixed(largest) => Some(largest),
            RangeMax::FromModel {
                from_model: ModelLimit::ContextMax,
            } => model.context_open(),
        }
    }

    /// Whether a number lies in the parameter's range for `model`. A parameter without a range takes
    /// any; one whose most the model does not set takes none.
    #[must_use]
    pub fn in_range(&self, number: i64, model: &ModelParameters) -> bool {
        match self.range {
            None => true,
            Some(range) => {
                number >= range.min
                    && self
                        .range_max(model)
                        .is_some_and(|largest| number <= largest)
            }
        }
    }

    /// What a thread starts with on `model` where it chose nothing, or chose what is not open: the
    /// default, so long as it lies in the range. None sends nothing.
    #[must_use]
    pub fn start_for(&self, model: &ModelParameters) -> Option<Json> {
        self.default_for(model).filter(|start| {
            start
                .as_i64()
                .is_none_or(|number| self.in_range(number, model))
        })
    }

    /// What this parameter comes to for a thread whose stored value is `stored`.
    #[must_use]
    pub fn resolve(
        &self,
        ctx: &Eligibility,
        features: &BTreeMap<String, bool>,
        stored: Option<&Json>,
    ) -> Resolution {
        if !self.is_shown(ctx, features) {
            return Resolution::Hidden;
        }
        // A thread that chose none of a speed or a program chose the ordinary tier, or no program:
        // that is sent as nothing, whatever the model starts with.
        if stored.is_some_and(Json::is_null) && self.chooses_none() {
            return Resolution::SendNothing;
        }
        let Some(value) = stored.filter(|value| !value.is_null()) else {
            return self
                .start_for(&ctx.model)
                .map_or(Resolution::SendNothing, Resolution::Send);
        };
        let right_type = match self.kind {
            ParameterType::Enum | ParameterType::String => value.is_string(),
            ParameterType::Boolean => value.is_boolean(),
            ParameterType::Integer => value.is_i64() || value.is_u64(),
        };
        if !right_type {
            return Resolution::WrongType(value.clone());
        }
        if self.kind == ParameterType::Integer
            && value
                .as_i64()
                .is_none_or(|number| !self.in_range(number, &ctx.model))
        {
            return Resolution::Stale(value.clone());
        }
        let listed = self.kind == ParameterType::Enum || self.options.is_some();
        if listed
            && !self
                .options_for(&ctx.model)
                .iter()
                .any(|option| value.as_str() == Some(option.value.as_str()))
        {
            return Resolution::Stale(value.clone());
        }
        Resolution::Send(value.clone())
    }
}

/// What is to be sent of an adapter's parameters, and what is to be flagged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sendable {
    /// The parameters to pass to the harness, by key.
    pub values: BTreeMap<String, Json>,
    /// The keys whose stored value is not among the current options, not in the range the model
    /// allows now, or not of the right type. Each is sent with what the model starts with, or not at
    /// all where it starts with nothing.
    pub flagged: Vec<String>,
}

/// The key under which the model's own window, in tokens, goes with the values of an adapter's
/// parameters in `session.open` and `turn.start`, where the adapter publishes the context. It is no
/// parameter and nobody chooses it: it is what the model's entry says the harness works with when it is
/// told nothing, and a harness that does not know a model has to be told it. The adapter reads the
/// context in force under the key of its own parameter, and this beside it.
pub const MODEL_WINDOW_KEY: &str = "context_window";

/// Works out what is sent of `defs` for a thread whose stored values are `stored` (an object by key).
///
/// A parameter that is not shown is left out. One whose stored value is not open is flagged and sent
/// with what the model starts with. Where the adapter publishes the context, the model's own window is
/// sent beside it, whether or not a larger context is open on the model.
#[must_use]
pub fn sendable(
    defs: &[ParameterDef],
    ctx: &Eligibility,
    features: &BTreeMap<String, bool>,
    stored: &Json,
) -> Sendable {
    let mut result = Sendable::default();
    let tells_context = defs
        .iter()
        .any(|def| def.kind_of_thread() == Some(ThreadKind::Context));
    if let Some(window) = ctx.model.context_window.filter(|window| *window > 0)
        && tells_context
    {
        result
            .values
            .insert(MODEL_WINDOW_KEY.to_owned(), Json::from(window));
    }
    for def in defs {
        match def.resolve(ctx, features, stored.get(&def.key)) {
            Resolution::Send(value) => {
                result.values.insert(def.key.clone(), value);
            }
            Resolution::Stale(_) | Resolution::WrongType(_) => {
                // The choice opens nothing: it is flagged and not sent, and the thread runs with what
                // its model starts with.
                result.flagged.push(def.key.clone());
                if let Some(start) = def.start_for(&ctx.model) {
                    result.values.insert(def.key.clone(), start);
                }
            }
            Resolution::Hidden | Resolution::SendNothing => {}
        }
    }
    result
}

/// What is wrong with a list of parameters.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParameterFault {
    /// Two parameters have one key.
    #[error("parameter {0:?} is declared twice")]
    DuplicateKey(String),
    /// An enumerated parameter has no options.
    #[error("parameter {0:?} is an enum and has no options")]
    EnumWithoutOptions(String),
    /// A parameter that is not enumerated has options.
    #[error("parameter {0:?} has options and is not an enum")]
    OptionsWithoutEnum(String),
    /// A parameter can be set nowhere.
    #[error("parameter {0:?} can be set at no scope")]
    NoScope(String),
    /// A condition names a range that is no semver range.
    #[error("parameter {0:?} has a condition with a harness version range that is no range")]
    BadRange(String),
    /// A parameter that is not an integer has a range.
    #[error("parameter {0:?} has a range and is not an integer")]
    RangeWithoutInteger(String),
    /// A range that no number is in.
    #[error("parameter {0:?} has a range that no number is in")]
    EmptyRange(String),
    /// An adapter publishes a second parameter of a kind that the thread has one of: the service tier
    /// and the context.
    #[error("parameter {0:?} is a second one of its kind: an adapter publishes at most one")]
    SecondOfItsKind(String),
    /// A parameter has the key that the model's own window is sent under.
    #[error("parameter {0:?} has a key that is the model's window's")]
    ReservedKey(String),
}

fn range_faults(condition: &Condition) -> bool {
    match condition {
        Condition::All(parts) | Condition::Any(parts) => parts.iter().any(range_faults),
        Condition::Not(part) => range_faults(part),
        Condition::HarnessVersion(range) => semver::VersionReq::parse(range).is_err(),
        _ => false,
    }
}

/// Checks an adapter's list of parameters: keys are unique, an enum has options and nothing else does,
/// a parameter can be set at some scope, and the ranges of conditions are ranges. Every fault is returned.
pub fn check_parameters(defs: &[ParameterDef]) -> Result<(), Vec<ParameterFault>> {
    let mut faults = Vec::new();
    for (position, def) in defs.iter().enumerate() {
        if defs[..position].iter().any(|other| other.key == def.key) {
            faults.push(ParameterFault::DuplicateKey(def.key.clone()));
        }
        match (def.kind, &def.options) {
            (ParameterType::Enum, None) => {
                faults.push(ParameterFault::EnumWithoutOptions(def.key.clone()))
            }
            (ParameterType::Boolean | ParameterType::String | ParameterType::Integer, Some(_)) => {
                faults.push(ParameterFault::OptionsWithoutEnum(def.key.clone()));
            }
            _ => {}
        }
        if def.scopes.is_empty() {
            faults.push(ParameterFault::NoScope(def.key.clone()));
        }
        if def.eligible_when.as_ref().is_some_and(range_faults) {
            faults.push(ParameterFault::BadRange(def.key.clone()));
        }
        if def.key == MODEL_WINDOW_KEY {
            faults.push(ParameterFault::ReservedKey(def.key.clone()));
        }
        if let Some(range) = def.range {
            if def.kind != ParameterType::Integer {
                faults.push(ParameterFault::RangeWithoutInteger(def.key.clone()));
            }
            if matches!(range.max, RangeMax::Fixed(largest) if largest < range.min) {
                faults.push(ParameterFault::EmptyRange(def.key.clone()));
            }
        }
        if defs[..position].iter().any(|other| {
            other.kind_of_thread() == def.kind_of_thread() && def.kind_of_thread().is_some()
        }) {
            faults.push(ParameterFault::SecondOfItsKind(def.key.clone()));
        }
    }
    if faults.is_empty() {
        Ok(())
    } else {
        Err(faults)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_models_enabled_defaults_and_rate_are_the_same_entry_the_api_shows() {
        let wire = json!({
            "efforts": ["low", "high"], "default_effort": "low",
            "service_tiers": [{ "id": "fast", "label": "Fast" }], "default_service_tier": "fast",
            "access_programs": ["blue"], "default_access_program": "blue",
            "context_window": 1000, "context_max": 2000, "default_context": 1500, "input_images": true,
            "offered": { "service_tiers": [{ "id": "batch", "label": "Batch" }], "context_max": 4000 },
            "rate": { "input_per_mtok": "1.5", "cached_input_per_mtok": "0.15", "output_per_mtok": "6",
                "cache_write_per_mtok": "2", "tiers": { "fast": "2" },
                "above": [{ "context_tokens": 1000, "input_per_mtok": "3", "output_per_mtok": "12" }] }
        });
        let offered: ModelParameters = serde_json::from_value(wire.clone()).unwrap();
        let visible: orch_api_contract::resources::ModelParameters = offered.clone();
        assert_eq!(serde_json::to_value(&visible).unwrap(), wire);
        assert_eq!(visible.service_tier_start(), Some("fast"));
        assert_eq!(visible.access_program_start(), Some("blue"));
        assert_eq!(visible.context_start(), Some(1500));
        assert!(visible.takes_images());
        let rate: orch_api_contract::resources::Rate = offered.rate.unwrap();
        assert_eq!(rate.cache_write_per_mtok.as_deref(), Some("2"));
        assert_eq!(
            schemars::schema_for!(ModelParameters),
            schemars::schema_for!(orch_api_contract::resources::ModelParameters)
        );
        assert_eq!(
            schemars::schema_for!(Rate),
            schemars::schema_for!(orch_api_contract::resources::Rate)
        );
    }

    fn def(value: Json) -> ParameterDef {
        serde_json::from_value(value).unwrap()
    }

    fn context(model: Json) -> Eligibility {
        Eligibility {
            credential_path: CredentialPath::Native,
            provider_kind: ProviderKind::Subscription,
            environment_placement: Placement::Managed,
            model: serde_json::from_value(model).unwrap(),
            harness_version: Some("0.160.1".into()),
        }
    }

    fn service_tier() -> ParameterDef {
        def(json!({
            "key": "service_tier", "title": "Speed", "description": "A faster mode.", "group": "model",
            "type": "enum", "options": { "from_model": "service_tiers" }, "default": null,
            "scopes": ["org", "project", "topic", "thread"], "apply": "next_turn",
            "eligible_when": { "model_has": "service_tiers" }
        }))
    }

    fn cyber() -> ParameterDef {
        def(json!({
            "key": "cyber_access_program", "title": "Access program", "description": "Cyber programs.",
            "group": "safety", "type": "enum", "options": { "from_model": "access_programs" }, "default": null,
            "scopes": ["thread"], "apply": "next_turn",
            "eligible_when": { "all": [ { "credential_path": "native" }, { "model_has": "access_programs" } ] },
            "feature": "codex.cyber_access"
        }))
    }

    #[test]
    fn a_parameter_reads_as_the_spec_writes_it() {
        let def = def(json!({
            "key": "reasoning_summary", "title": "Reasoning summary", "description": "How much of its reasoning is summarised.",
            "group": "behaviour", "type": "enum",
            "options": [ { "value": "auto", "label": "Auto" }, { "value": "none", "label": "None", "description": "No summary." } ],
            "default": "auto", "scopes": ["org", "thread"], "apply": "next_turn"
        }));
        assert_eq!(def.default, ParameterDefault::Value(json!("auto")));
        assert!(
            matches!(def.options, Some(ParameterOptions::Listed(ref listed)) if listed.len() == 2)
        );
        assert_eq!(check_parameters(&[def]), Ok(()));
    }

    #[test]
    fn a_parameter_exists_only_when_it_is_eligible_and_enabled() {
        let features = BTreeMap::from([("codex.cyber_access".to_owned(), true)]);
        let with_programs = context(json!({ "access_programs": ["daybreakBlue"] }));
        assert!(cyber().is_shown(&with_programs, &features));
        assert!(
            !cyber().is_shown(&with_programs, &BTreeMap::new()),
            "the feature is off"
        );
        assert!(
            !cyber().is_shown(
                &with_programs,
                &BTreeMap::from([("codex.cyber_access".into(), false)])
            ),
            "the feature is switched off, not just absent"
        );
        let mut forwarded = with_programs.clone();
        forwarded.credential_path = CredentialPath::Forwarded;
        assert!(
            !cyber().is_shown(&forwarded, &features),
            "the path is not native"
        );
        let without = context(json!({ "offered": { "access_programs": ["daybreakBlue"] } }));
        assert!(
            !cyber().is_shown(&without, &features),
            "an access program that is only offered is not usable until a manager enables it"
        );
    }

    #[test]
    fn a_parameter_that_is_not_shown_is_not_sent_whatever_is_stored() {
        let ctx = context(json!({}));
        assert_eq!(
            service_tier().resolve(&ctx, &BTreeMap::new(), Some(&json!("fast"))),
            Resolution::Hidden
        );
        let outcome = sendable(
            &[service_tier()],
            &ctx,
            &BTreeMap::new(),
            &json!({ "service_tier": "fast" }),
        );
        assert!(outcome.values.is_empty() && outcome.flagged.is_empty());
    }

    #[test]
    fn options_that_come_from_the_model_are_the_ones_a_manager_enabled() {
        let ctx = context(json!({
            "service_tiers": [ { "id": "fast", "label": "Fast" } ],
            "offered": { "service_tiers": [ { "id": "fast", "label": "Fast" }, { "id": "flex", "label": "Flex" } ] }
        }));
        let tier = service_tier();
        let options: Vec<_> = tier
            .options_for(&ctx.model)
            .into_iter()
            .map(|o| o.value)
            .collect();
        assert_eq!(options, ["fast"]);
        assert_eq!(
            tier.resolve(&ctx, &BTreeMap::new(), Some(&json!("fast"))),
            Resolution::Send(json!("fast"))
        );
        assert_eq!(
            tier.resolve(&ctx, &BTreeMap::new(), Some(&json!("flex"))),
            Resolution::Stale(json!("flex")),
            "offered is not enabled: the stored value is flagged and not sent"
        );
        assert_eq!(
            tier.resolve(&ctx, &BTreeMap::new(), None),
            Resolution::SendNothing
        );
        let outcome = sendable(
            &[tier],
            &ctx,
            &BTreeMap::new(),
            &json!({ "service_tier": "flex" }),
        );
        assert_eq!(outcome.flagged, ["service_tier"]);
    }

    #[test]
    fn the_default_may_come_from_the_model() {
        let effort = def(json!({
            "key": "effort", "title": "Effort", "description": "Reasoning effort.", "group": "model",
            "type": "enum", "options": { "from_model": "efforts" }, "default": { "from_model": "default_effort" },
            "scopes": ["thread"], "apply": "next_turn"
        }));
        let ctx = context(json!({ "efforts": ["low", "high"], "default_effort": "high" }));
        assert_eq!(
            effort.resolve(&ctx, &BTreeMap::new(), None),
            Resolution::Send(json!("high"))
        );
        assert_eq!(
            effort.resolve(&ctx, &BTreeMap::new(), Some(&json!("low"))),
            Resolution::Send(json!("low"))
        );
        assert_eq!(
            effort.resolve(&ctx, &BTreeMap::new(), Some(&json!(3))),
            Resolution::WrongType(json!(3))
        );
    }

    #[test]
    fn a_condition_combines_what_is_known_of_the_place_and_the_model() {
        let ctx = context(json!({ "efforts": ["low", "high"], "input_images": true }));
        let holds = |condition: Json| {
            serde_json::from_value::<Condition>(condition)
                .unwrap()
                .holds(&ctx)
        };
        assert!(holds(
            json!({ "model_includes": { "field": "efforts", "value": "high" } })
        ));
        assert!(!holds(
            json!({ "model_includes": { "field": "efforts", "value": "max" } })
        ));
        assert!(holds(json!({ "model_has": "input_images" })));
        assert!(holds(json!({ "not": { "provider_kind": "api" } })));
        assert!(holds(
            json!({ "any": [ { "environment_placement": "connected" }, { "provider_kind": "subscription" } ] })
        ));
        assert!(!holds(json!({ "any": [] })));
        assert!(holds(json!({ "all": [] })));
        assert!(holds(json!({ "harness_version": ">=0.160.0, <0.163.0" })));
        assert!(!holds(json!({ "harness_version": ">=0.163.0" })));
        assert!(!holds(json!({ "harness_version": "not a range" })));
    }

    #[test]
    fn an_effort_is_the_threads_when_the_model_lists_it_and_the_models_default_otherwise() {
        let model: ModelParameters =
            serde_json::from_value(json!({ "efforts": ["low", "high"], "default_effort": "low" }))
                .unwrap();
        assert_eq!(model.effort_for(Some("high")).as_deref(), Some("high"));
        assert_eq!(model.effort_for(Some("max")).as_deref(), Some("low"));
        assert_eq!(model.effort_for(None).as_deref(), Some("low"));
        assert!(model.effort_is_gone(Some("max")));
        assert!(!model.effort_is_gone(None));
        let plain = ModelParameters::default();
        assert_eq!(
            plain.effort_for(Some("high")),
            None,
            "a model that lists none takes none"
        );
    }

    #[test]
    fn a_list_of_parameters_is_refused_for_each_thing_that_is_wrong_with_it() {
        let mut broken = vec![service_tier(), service_tier()];
        broken[1].options = None;
        broken[1].scopes.clear();
        broken[1].eligible_when = Some(Condition::HarnessVersion("soon".into()));
        let faults = check_parameters(&broken).unwrap_err();
        assert!(faults.contains(&ParameterFault::DuplicateKey("service_tier".into())));
        assert!(faults.contains(&ParameterFault::EnumWithoutOptions("service_tier".into())));
        assert!(faults.contains(&ParameterFault::NoScope("service_tier".into())));
        assert!(faults.contains(&ParameterFault::BadRange("service_tier".into())));
    }

    fn context_def() -> ParameterDef {
        def(json!({
            "key": "context", "title": "Context", "description": "How much of a conversation is kept.",
            "group": "model", "type": "integer",
            "range": { "min": 32000, "max": { "from_model": "context_max" } },
            "default": { "from_model": "default_context" },
            "scopes": ["org", "project", "topic", "thread"], "apply": "new_session",
            "eligible_when": { "model_has": "context_max" }
        }))
    }

    fn access() -> ParameterDef {
        def(json!({
            "key": "access_program", "title": "Access", "description": "A program.", "group": "safety",
            "type": "enum", "options": { "from_model": "access_programs" },
            "default": { "from_model": "default_access_program" },
            "scopes": ["thread"], "apply": "next_turn",
            "eligible_when": { "model_has": "access_programs" }
        }))
    }

    fn speed() -> ParameterDef {
        let mut speed = service_tier();
        speed.default = ParameterDefault::FromModel {
            from_model: DefaultFromModel::DefaultServiceTier,
        };
        speed
    }

    fn context_of(def: &ParameterDef, model: Json, stored: Option<Json>) -> Resolution {
        def.resolve(&context(model), &BTreeMap::new(), stored.as_ref())
    }

    #[test]
    fn a_context_is_an_integer_whose_most_is_what_the_models_managers_opened() {
        let def = context_def();
        assert_eq!(
            def.range,
            Some(ParameterRange {
                min: 32_000,
                max: RangeMax::FromModel {
                    from_model: ModelLimit::ContextMax
                }
            })
        );
        assert_eq!(def.kind_of_thread(), Some(ThreadKind::Context));
        assert_eq!(speed().kind_of_thread(), Some(ThreadKind::ServiceTier));
        assert_eq!(check_parameters(&[def.clone(), speed()]), Ok(()));
        let written = serde_json::to_value(&def).unwrap();
        assert_eq!(
            written["range"]["max"],
            json!({ "from_model": "context_max" })
        );
        let fixed: ParameterRange =
            serde_json::from_value(json!({ "min": 1000, "max": 8000 })).unwrap();
        assert_eq!(fixed.max, RangeMax::Fixed(8000));
        let open = json!({ "context_window": 272000, "context_max": 1000000 });
        assert!(def.is_shown(&context(open), &BTreeMap::new()));
        let closed = json!({ "context_window": 272000, "offered": { "context_max": 1000000 } });
        assert!(
            !def.is_shown(&context(closed), &BTreeMap::new()),
            "a larger context that is only on offer is not open: nothing is shown"
        );
    }

    #[test]
    fn a_thread_starts_with_all_that_is_open_unless_the_entry_names_another_start() {
        let all = json!({ "context_window": 272000, "context_max": 1000000 });
        assert_eq!(
            context_of(&context_def(), all, None),
            Resolution::Send(json!(1_000_000))
        );
        let named =
            json!({ "context_window": 272000, "context_max": 1000000, "default_context": 272000 });
        assert_eq!(
            context_of(&context_def(), named, None),
            Resolution::Send(json!(272_000)),
            "the entry names its own window as the start"
        );
        let dangling =
            json!({ "context_window": 272000, "context_max": 400000, "default_context": 900000 });
        assert_eq!(
            context_of(&context_def(), dangling, None),
            Resolution::Send(json!(400_000)),
            "a start above what is open is not one"
        );
        let low = json!({ "context_max": 20000 });
        assert_eq!(
            context_of(&context_def(), low, None),
            Resolution::SendNothing,
            "a start below what the adapter's harness takes is not sent"
        );
    }

    #[test]
    fn a_choice_between_the_least_and_the_most_is_sent_as_it_is() {
        let model = json!({ "context_window": 272000, "context_max": 1000000 });
        for number in [32_000, 200_000, 1_000_000] {
            assert_eq!(
                context_of(&context_def(), model.clone(), Some(json!(number))),
                Resolution::Send(json!(number))
            );
        }
        for number in [31_999, 1_000_001, 0, -5] {
            assert_eq!(
                context_of(&context_def(), model.clone(), Some(json!(number))),
                Resolution::Stale(json!(number)),
                "{number} is not open"
            );
        }
        assert_eq!(
            context_of(&context_def(), model, Some(json!("big"))),
            Resolution::WrongType(json!("big"))
        );
    }

    #[test]
    fn a_choice_above_a_context_that_was_lowered_is_flagged_and_the_thread_runs_on_the_start() {
        let lowered = context(json!({ "context_window": 272000, "context_max": 500000 }));
        let outcome = sendable(
            &[context_def()],
            &lowered,
            &BTreeMap::new(),
            &json!({ "context": 1000000 }),
        );
        assert_eq!(outcome.flagged, ["context"]);
        assert_eq!(outcome.values["context"], json!(500_000));
        let shrunk = context(json!({
            "context_window": 272000, "context_max": 500000, "default_context": 300000
        }));
        let outcome = sendable(
            &[context_def()],
            &shrunk,
            &BTreeMap::new(),
            &json!({ "context": 1000000 }),
        );
        assert_eq!(
            outcome.values["context"],
            json!(300_000),
            "the entry's own start"
        );
    }

    #[test]
    fn nothing_is_chosen_or_shown_for_a_model_with_no_larger_context_open() {
        let plain = context(json!({ "context_window": 272000 }));
        assert_eq!(
            context_def().resolve(&plain, &BTreeMap::new(), Some(&json!(100_000))),
            Resolution::Hidden
        );
        let outcome = sendable(
            &[context_def()],
            &plain,
            &BTreeMap::new(),
            &json!({ "context": 100000 }),
        );
        assert_eq!(
            outcome.values.keys().collect::<Vec<_>>(),
            [MODEL_WINDOW_KEY],
            "only the model's own window goes to the adapter"
        );
        assert!(outcome.flagged.is_empty());
        assert_eq!(plain.model.context_start(), Some(272_000));
        assert_eq!(ModelParameters::default().context_start(), None);
    }

    #[test]
    fn a_speed_and_a_program_start_where_the_entry_says_and_a_choice_comes_before_the_start() {
        let model = json!({
            "service_tiers": [ { "id": "fast", "label": "Fast" }, { "id": "flex", "label": "Flex" } ],
            "default_service_tier": "fast",
            "access_programs": ["blue", "red"], "default_access_program": "blue"
        });
        let ctx = context(model);
        let features = BTreeMap::new();
        assert_eq!(
            speed().resolve(&ctx, &features, None),
            Resolution::Send(json!("fast"))
        );
        assert_eq!(
            speed().resolve(&ctx, &features, Some(&json!("flex"))),
            Resolution::Send(json!("flex"))
        );
        assert_eq!(
            access().resolve(&ctx, &features, None),
            Resolution::Send(json!("blue"))
        );
        assert_eq!(
            access().resolve(&ctx, &features, Some(&json!("red"))),
            Resolution::Send(json!("red"))
        );
        let none = context(
            json!({ "service_tiers": [ { "id": "fast", "label": "Fast" } ],
                                   "access_programs": ["blue"] }),
        );
        assert_eq!(
            speed().resolve(&none, &features, None),
            Resolution::SendNothing,
            "left to itself a thread starts at the ordinary tier"
        );
        assert_eq!(
            access().resolve(&none, &features, None),
            Resolution::SendNothing,
            "and with no program"
        );
    }

    #[test]
    fn a_thread_chooses_the_ordinary_tier_and_no_program_where_the_model_starts_in_another() {
        let ctx = context(json!({
            "service_tiers": [ { "id": "fast", "label": "Fast" } ], "default_service_tier": "fast",
            "access_programs": ["blue"], "default_access_program": "blue"
        }));
        let features = BTreeMap::new();
        assert_eq!(
            speed().resolve(&ctx, &features, Some(&Json::Null)),
            Resolution::SendNothing,
            "the ordinary tier, chosen"
        );
        assert_eq!(
            access().resolve(&ctx, &features, Some(&Json::Null)),
            Resolution::SendNothing,
            "no program, chosen"
        );
        let outcome = sendable(
            &[speed(), access()],
            &ctx,
            &features,
            &json!({ "service_tier": null }),
        );
        assert!(!outcome.values.contains_key("service_tier"));
        assert_eq!(
            outcome.values["access_program"],
            json!("blue"),
            "what was not chosen runs with what the model starts with"
        );
        assert!(outcome.flagged.is_empty());
    }

    #[test]
    fn a_start_that_is_no_longer_enabled_is_not_sent_and_a_stale_choice_runs_on_the_start() {
        let dangling = context(json!({
            "service_tiers": [ { "id": "flex", "label": "Flex" } ], "default_service_tier": "fast",
            "access_programs": ["red"], "default_access_program": "blue"
        }));
        let features = BTreeMap::new();
        assert_eq!(
            speed().resolve(&dangling, &features, None),
            Resolution::SendNothing
        );
        assert_eq!(
            access().resolve(&dangling, &features, None),
            Resolution::SendNothing
        );
        let model = context(json!({
            "service_tiers": [ { "id": "flex", "label": "Flex" } ], "default_service_tier": "flex"
        }));
        let outcome = sendable(
            &[speed()],
            &model,
            &features,
            &json!({ "service_tier": "fast" }),
        );
        assert_eq!(outcome.flagged, ["service_tier"]);
        assert_eq!(
            outcome.values["service_tier"],
            json!("flex"),
            "the choice 'fast' opens nothing; the thread runs on what the model starts with"
        );
    }

    #[test]
    fn the_models_own_window_goes_beside_the_context_whether_or_not_a_larger_one_is_open() {
        let features = BTreeMap::new();
        let plain = context(json!({ "context_window": 272000 }));
        let outcome = sendable(&[context_def()], &plain, &features, &json!({}));
        assert_eq!(outcome.values.len(), 1);
        assert_eq!(outcome.values[MODEL_WINDOW_KEY], json!(272_000));
        let open = context(json!({ "context_window": 272000, "context_max": 1000000 }));
        let outcome = sendable(&[context_def()], &open, &features, &json!({}));
        assert_eq!(outcome.values["context"], json!(1_000_000));
        assert_eq!(outcome.values[MODEL_WINDOW_KEY], json!(272_000));
        let unknown = context(json!({}));
        assert!(
            sendable(&[context_def()], &unknown, &features, &json!({}))
                .values
                .is_empty(),
            "a model whose entry has no window: none is sent"
        );
        assert!(
            sendable(&[speed()], &plain, &features, &json!({}))
                .values
                .is_empty(),
            "an adapter that publishes no context is not told one"
        );
        let mut reserved = speed();
        reserved.key = MODEL_WINDOW_KEY.into();
        reserved.kind = ParameterType::Integer;
        reserved.options = None;
        assert!(
            check_parameters(&[reserved])
                .unwrap_err()
                .contains(&ParameterFault::ReservedKey(MODEL_WINDOW_KEY.into()))
        );
    }

    #[test]
    fn a_list_of_parameters_is_refused_for_a_range_without_an_integer_and_a_second_of_a_kind() {
        let mut wrong = context_def();
        wrong.kind = ParameterType::String;
        wrong.range = Some(ParameterRange {
            min: 10,
            max: RangeMax::Fixed(5),
        });
        let faults = check_parameters(&[wrong]).unwrap_err();
        assert!(faults.contains(&ParameterFault::RangeWithoutInteger("context".into())));
        assert!(faults.contains(&ParameterFault::EmptyRange("context".into())));
        let mut other = context_def();
        other.key = "window".into();
        let faults = check_parameters(&[context_def(), other, speed()]).unwrap_err();
        assert_eq!(faults, [ParameterFault::SecondOfItsKind("window".into())]);
    }

    #[test]
    fn what_a_model_says_of_itself_reads_as_the_spec_writes_it() {
        let model: ModelParameters = serde_json::from_value(json!({
            "efforts": ["low", "medium", "high"], "default_effort": "medium",
            "service_tiers": [ { "id": "fast", "label": "Fast" } ], "default_service_tier": "fast",
            "offered": { "access_programs": ["daybreakBlue"], "context_max": 1000000 },
            "context_window": 272000, "context_max": 1000000, "default_context": 272000,
            "input_images": true,
            "rate": { "input_per_mtok": "2.50", "cached_input_per_mtok": "0.25", "output_per_mtok": "15",
                      "tiers": { "fast": "2" },
                      "above": [ { "context_tokens": 272000, "input_per_mtok": "5", "output_per_mtok": "22.5" } ] }
        }))
        .unwrap();
        assert_eq!(
            model.rate.as_ref().unwrap().tiers.as_ref().unwrap()["fast"],
            "2"
        );
        assert_eq!(model.service_tier_start(), Some("fast"));
        assert_eq!(model.context_start(), Some(272_000));
        assert_eq!(
            model.offered.as_ref().and_then(|o| o.context_max),
            Some(1_000_000)
        );
        let written = serde_json::to_value(&model).unwrap();
        assert!(
            written.get("access_programs").is_none(),
            "left out, not null"
        );
        assert_eq!(
            written["rate"]["above"][0]["context_tokens"],
            json!(272_000)
        );
    }
}
