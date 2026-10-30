//! What a model can be run with at a provider, and a model as a list of models gives it.
//!
//! A model's entry at its provider says what it offers there, what its managers opened and what a thread on it
//! starts with ([`ModelParameters`]); a source of models lists them as suggestions ([`ModelSuggestion`]), which
//! become entries only when a manager accepts them. Written once here, the API's contract and the agent's
//! re-export them.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A paid-for speed a turn on a model may ask for beside the ordinary one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ServiceTier {
    /// What a request names.
    pub id: String,
    /// What it is called.
    pub label: String,
}

/// What a harness or an endpoint last said a model offers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelOffered {
    /// Speeds on offer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tiers: Option<Vec<ServiceTier>>,
    /// Access programs on offer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_programs: Option<Vec<String>>,
    /// The largest context, in tokens, that the harness or the endpoint says the model takes, where
    /// that is more than the window the model starts with. A manager opens it by writing it as the
    /// model's `context_max`; until then no thread can be given it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_max: Option<i64>,
}

/// A price that applies to requests whose prompt is larger than a size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RateAbove {
    /// The prompt size from which these prices apply.
    pub context_tokens: i64,
    /// Credits for a million input tokens.
    pub input_per_mtok: String,
    /// Credits for a million cached input tokens; as the ordinary rate when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_per_mtok: Option<String>,
    /// Credits for a million output tokens.
    pub output_per_mtok: String,
    /// Credits for a million tokens written to a prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_per_mtok: Option<String>,
}

/// What a model costs, in credits, written as decimal strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Rate {
    /// Credits for a million input tokens.
    pub input_per_mtok: String,
    /// Credits for a million cached input tokens.
    pub cached_input_per_mtok: String,
    /// Credits for a million output tokens.
    pub output_per_mtok: String,
    /// Credits for a million tokens written to a prompt cache; as input when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_per_mtok: Option<String>,
    /// Service tiers priced as a multiple of the rate, written as a decimal string: `"2"` for
    /// double, `"0.5"` for half.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiers: Option<BTreeMap<String, String>>,
    /// Prices that replace the ordinary ones for large prompts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub above: Option<Vec<RateAbove>>,
}

/// What one model can be run with at a provider. Models differ in all of it, so each member is the
/// model's own and has three parts: what the model offers there (`offered`), what its managers
/// opened (`efforts`, `service_tiers`, `access_programs`, up to `context_max`), and what a thread on
/// it starts with (`default_*`). What a model *may* be run with is written here; what a thread
/// *does* run with is the thread's own setting, chosen within what is open.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelParameters {
    /// The efforts it takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<String>>,
    /// The effort used when none is chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    /// The speeds a manager of the provider enabled; empty until then,
    /// whatever the harness offers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tiers: Option<Vec<ServiceTier>>,
    /// The speed a thread on this model starts with: one of `service_tiers`. Left out, the ordinary
    /// one. A manager's choice, never discovery's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_service_tier: Option<String>,
    /// The access programs a manager enabled, one by one, never by discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_programs: Option<Vec<String>>,
    /// The access program a thread on this model starts with: one of `access_programs`. Left out,
    /// none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_access_program: Option<String>,
    /// The model's own window, in tokens: what its harness works with when it is told nothing, as
    /// the harness's list gives it or a manager wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    /// The largest context a thread on this model may be given, where that is more than
    /// `context_window`: enabled by a manager of the provider, as a faster speed is, since a longer
    /// prompt may be counted at other prices or take more of a plan. Left out, there is no choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_max: Option<i64>,
    /// The context a thread on this model starts with, at most `context_max`. Left out, all that is
    /// open (`context_max`), and where nothing larger is open, the model's own window. Where the
    /// conversation is shortened below a context is worked out and is a member of nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_context: Option<i64>,
    /// What the harness or the endpoint most recently reported as on offer. Being listed here does
    /// not make anything usable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offered: Option<ModelOffered>,
    /// Whether it takes images.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_images: Option<bool>,
    /// What it costs, when a manager entered a price or discovery found one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<Rate>,
}

impl ModelParameters {
    /// The offered reasoning efforts, or none.
    #[must_use]
    pub fn efforts(&self) -> &[String] {
        self.efforts.as_deref().unwrap_or_default()
    }
    /// Service tiers explicitly enabled by a manager.
    #[must_use]
    pub fn service_tiers(&self) -> &[ServiceTier] {
        self.service_tiers.as_deref().unwrap_or_default()
    }
    /// Access programs explicitly enabled by a manager.
    #[must_use]
    pub fn access_programs(&self) -> &[String] {
        self.access_programs.as_deref().unwrap_or_default()
    }
    /// Whether this entry explicitly supports images.
    #[must_use]
    pub fn takes_images(&self) -> bool {
        self.input_images == Some(true)
    }
    /// The positive context a manager opened, if any.
    #[must_use]
    pub fn context_open(&self) -> Option<i64> {
        self.context_max.filter(|largest| *largest > 0)
    }
    /// The default tier only while it remains enabled.
    #[must_use]
    pub fn service_tier_start(&self) -> Option<&str> {
        self.default_service_tier
            .as_deref()
            .filter(|start| self.service_tiers().iter().any(|tier| tier.id == *start))
    }
    /// The default access program only while it remains enabled.
    #[must_use]
    pub fn access_program_start(&self) -> Option<&str> {
        self.default_access_program.as_deref().filter(|start| {
            self.access_programs()
                .iter()
                .any(|program| program == start)
        })
    }
    /// Start at the configured positive context within the opened limit; absent
    /// that, use all opened context or the model's ordinary window.
    #[must_use]
    pub fn context_start(&self) -> Option<i64> {
        match self.context_open() {
            Some(largest) => Some(
                self.default_context
                    .filter(|start| (1..=largest).contains(start))
                    .unwrap_or(largest),
            ),
            None => self.context_window.filter(|window| *window > 0),
        }
    }
    /// An enabled requested effort, otherwise the model's default.
    #[must_use]
    pub fn effort_for(&self, wanted: Option<&str>) -> Option<String> {
        wanted
            .filter(|effort| self.efforts().iter().any(|listed| listed == effort))
            .map(str::to_owned)
            .or_else(|| self.default_effort.clone())
    }
    /// Whether an explicit requested effort is no longer offered.
    #[must_use]
    pub fn effort_is_gone(&self, wanted: Option<&str>) -> bool {
        wanted.is_some_and(|effort| !self.efforts().iter().any(|listed| listed == effort))
    }
}

/// A model as a list of models gives it: a proposal and no more.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelSuggestion {
    /// The routing name: the exact name sent upstream, in whatever form that upstream takes.
    pub model_name: String,
    /// The proposed display name; the routing name where the list has none.
    pub display_name: String,
    /// What the list says of the model: its efforts and default, what it offers, its context window, whether
    /// it takes images, and a rate where the list carries prices.
    pub parameters: ModelParameters,
    /// Whether the list the suggestion comes from names it.
    pub listed: bool,
}

impl ModelSuggestion {
    /// A suggestion for a model that a list names as `id`, with `display_name` where the list has one.
    #[must_use]
    pub fn named(id: &str, display_name: Option<&str>) -> Self {
        Self {
            model_name: id.to_owned(),
            display_name: display_name
                .filter(|name| !name.is_empty())
                .unwrap_or(id)
                .to_owned(),
            parameters: ModelParameters::default(),
            listed: true,
        }
    }
}
