//! What a model can be run with at a provider, and a model as a list of models gives it.
//!
//! A model's entry at its provider says what it offers there, what its managers opened and what a thread on it
//! starts with ([`ModelParameters`]); a source of models lists them as suggestions ([`ModelSuggestion`]), which
//! become entries only when a manager accepts them. Written once here, the API's contract and the agent's
//! re-export them.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod option_layers;

/// The same per-interface option declarations on an endpoint, model group or model.
pub type RequestOptions = BTreeMap<String, orch_types::Json>;

/// Options attached to an existing named group of a provider's model offerings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelGroupOptions {
    /// Option forms and constraints keyed by interface; `native` names program parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_options: Option<RequestOptions>,
    /// Future declaration members survive a manager's edit without enabling an unknown option.
    #[serde(flatten)]
    pub extra: BTreeMap<String, orch_types::Json>,
}

/// Current option declarations of a model's actual provider, without credentials or addresses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelOptionLayers {
    /// Endpoint declarations indexed by their existing interface name.
    #[serde(default)]
    pub endpoints: BTreeMap<String, RequestOptions>,
    /// Declarations on the provider's existing named model groups.
    #[serde(default)]
    pub groups: BTreeMap<String, ModelGroupOptions>,
}

/// Read the actual stored model and its source declarations through the one option composer.
/// A selected interface comes from the existing arrangement/call; this function selects no route.
pub fn composed_model_parameters(
    parameters: &orch_types::Json,
    groups: &[String],
    layers: &orch_types::Json,
    protocol: Option<&str>,
) -> Result<ModelParameters, serde_json::Error> {
    let model: ModelParameters = serde_json::from_value(parameters.clone())?;
    let layers: ModelOptionLayers = if layers.is_null() {
        ModelOptionLayers::default()
    } else {
        serde_json::from_value(layers.clone())?
    };
    // An unbound catalogue can show each interface's own wire schema. Native program options
    // of an endpoint apply only when the existing arrangement selected that endpoint.
    let mut endpoint = layers
        .endpoints
        .iter()
        .filter_map(|(protocol, options)| {
            options
                .get(protocol)
                .map(|schema| (protocol.clone(), schema.clone()))
        })
        .collect::<RequestOptions>();
    // A translator can select another outgoing interface at the forwarder. Keep every actual
    // endpoint's wire rule frozen; only a directly selected endpoint contributes native controls.
    if let Some(native) = protocol
        .and_then(|protocol| layers.endpoints.get(protocol))
        .and_then(|options| options.get("native"))
    {
        endpoint.insert("native".to_owned(), native.clone());
    }
    let selected = groups
        .iter()
        .filter_map(|group| layers.groups.get(group))
        .filter_map(|group| group.request_options.as_ref());
    Ok(compose_request_options(&model, Some(&endpoint), selected))
}

/// Compose option declarations without changing the model's offered or enabled capabilities.
/// Endpoint is least specific, followed by groups in their saved order, then the model.
/// A later property supplies its form; every original layer still constrains the final value.
#[must_use]
pub fn compose_request_options<'a>(
    model: &'a ModelParameters,
    endpoint: Option<&'a RequestOptions>,
    groups: impl IntoIterator<Item = &'a RequestOptions>,
) -> ModelParameters {
    let mut layers = endpoint.into_iter().chain(groups).collect::<Vec<_>>();
    if layers.iter().all(|layer| layer.is_empty()) {
        return model.clone();
    }
    layers.extend(model.request_options.as_ref());
    let protocols = layers
        .iter()
        .flat_map(|layer| layer.keys())
        .collect::<std::collections::BTreeSet<_>>();
    let mut composed = model.clone();
    composed.request_options = Some(
        protocols
            .into_iter()
            .map(|protocol| {
                let schemas = layers
                    .iter()
                    .filter_map(|layer| layer.get(protocol))
                    .collect::<Vec<_>>();
                (protocol.clone(), option_layers::compose(&schemas))
            })
            .collect(),
    );
    composed
}

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
    /// Additional prompt-cache lifetimes observed without enabling them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_lifetimes: Option<Vec<String>>,
    /// Additional reasoning modes observed at this offering, without enabling them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_modes: Option<Vec<String>>,
    /// Optional discovery details. These facts neither enable a capability nor constrain an old model entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<ModelReport>,
    /// Native search modes the source offers, without enabling them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search: Option<Vec<String>>,
    /// Speeds on offer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tiers: Option<Vec<ServiceTier>>,
    /// Capacity classes observed independently of inference speed, without enabling them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_tiers: Option<Vec<ServiceTier>>,
    /// Access programs on offer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_programs: Option<Vec<String>>,
    /// The largest context, in tokens, that the harness or the endpoint says the model takes, where
    /// that is more than the window the model starts with. A manager opens it by writing it as the
    /// model's `context_max`; until then no thread can be given it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_max: Option<i64>,
}

/// What an endpoint actually reported, retained independently of a manager's enabled model options.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ModelReport {
    /// Capability support objects in the interface's declared vocabulary; unrecognized members grant nothing.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub capabilities: BTreeMap<String, orch_types::Json>,
    /// Reported maximum output tokens, without changing the thread's output limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_max: Option<i64>,
    /// Lifecycle stage as reported. Future stages remain data, not a new refusal rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<String>,
    /// Reported deprecation time; absent means it was not reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated_at: Option<String>,
    /// Reported retirement schedule. A past schedule does not declare that retirement happened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retires_at: Option<String>,
    /// Model line proposed by the endpoint, never inferred from a routing name or enabled as a group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// Future discovery members survive reads and edits without affecting dispatch.
    #[serde(flatten)]
    pub extensions: crate::capabilities::Extensions,
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
    /// Credits for a million tokens written to a one-hour prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h_per_mtok: Option<String>,
    /// Credits for a million twelve-hour cache writes; inherits the root quote when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_12h_per_mtok: Option<String>,
}

/// What a model costs, in credits, written as decimal strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Rate {
    /// Credits for one successful native web search, additional to token prices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search_per_call: Option<String>,
    /// Credits for one successful native page fetch, separate from searches and tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_fetch_per_call: Option<String>,
    /// Credits for a million input tokens.
    pub input_per_mtok: String,
    /// Credits for a million cached input tokens.
    pub cached_input_per_mtok: String,
    /// Credits for a million output tokens.
    pub output_per_mtok: String,
    /// Credits for a million tokens written to a prompt cache; as input when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_per_mtok: Option<String>,
    /// Credits for a million one-hour cache writes; absence leaves that price unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h_per_mtok: Option<String>,
    /// Credits for a million twelve-hour cache writes; absence leaves that price unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_12h_per_mtok: Option<String>,
    /// Service tiers priced as a multiple of the rate, written as a decimal string: `"2"` for
    /// double, `"0.5"` for half.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiers: Option<BTreeMap<String, String>>,
    /// Actual reported capacity classes priced independently of inference-speed multipliers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_tiers: Option<BTreeMap<String, String>>,
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
    /// Additional cache lifetimes enabled by a manager. Ordinary five-minute caching stays available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_lifetimes: Option<Vec<String>>,
    /// Optional interface/capability bindings, projected from installed declarations without rewriting older entries.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub native_tools: BTreeMap<String, BTreeMap<String, Option<crate::capabilities::Binding>>>,
    /// Native search modes enabled by a provider manager; absence leaves search disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search: Option<Vec<String>>,
    /// The enabled search mode used when a use makes no choice; absent means disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_web_search: Option<String>,
    /// The efforts it takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<String>>,
    /// The effort used when none is chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    /// Additional reasoning modes explicitly enabled by a provider manager.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_modes: Option<Vec<String>>,
    /// An enabled reasoning mode used when a use makes no choice; never set by discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_mode: Option<String>,
    /// The speeds a manager of the provider enabled; empty until then,
    /// whatever the harness offers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tiers: Option<Vec<ServiceTier>>,
    /// The speed a thread on this model starts with: one of `service_tiers`. Left out, the ordinary
    /// one. A manager's choice, never discovery's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_service_tier: Option<String>,
    /// Enabled capacity selections. Absence keeps undeclared/older endpoint defaults untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_tiers: Option<Vec<ServiceTier>>,
    /// An enabled capacity selection used when none is chosen, never set by discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_capacity_tier: Option<String>,
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
    /// Provider constraints on additional request options, keyed by wire protocol; `native` uses
    /// declared adapter parameter keys. JSON Schema enum/const/default/required retain their distinct
    /// meanings. An omitted schema preserves the protocol's existing extension surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_options: Option<RequestOptions>,
    /// What it costs, when a manager entered a price or discovery found one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<Rate>,
}

impl ModelParameters {
    /// Cache lifetimes are explicit; a discovered offer never enables one-hour writes.
    #[must_use]
    pub fn cache_lifetime_enabled(&self, lifetime: &str) -> bool {
        lifetime == "5m"
            || self
                .cache_lifetimes
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|enabled| enabled == lifetime)
    }
    /// The provider's option schema for this wire format, or for declared native parameters.
    #[must_use]
    pub fn option_schema(&self, protocol: &str) -> Option<&orch_types::Json> {
        self.request_options.as_ref()?.get(protocol)
    }

    /// Original schemas retained by the composer, before protocol-owned option normalization.
    #[must_use]
    pub fn original_option_layers(&self, protocol: &str) -> Option<&[orch_types::Json]> {
        let schema = self.option_schema(protocol)?;
        let checks = schema.get("allOf")?.as_array()?;
        let layers = schema.get("x-orch-option-layers")?.as_array()?;
        if layers.len() < 2 || checks.len() != layers.len() {
            return None;
        }
        for (index, check) in checks.iter().enumerate() {
            let name = format!("option_layer_{index}");
            let pointer = format!("#/$defs/{name}");
            if check.get("$ref").and_then(orch_types::Json::as_str) != Some(pointer.as_str())
                || schema.get("$defs")?.get(name.as_str()).is_none()
            {
                return None;
            }
        }
        Some(layers.as_slice())
    }

    /// One declared option's rule. Absence leaves the existing protocol/adapter declaration in force.
    #[must_use]
    pub fn option_rule(&self, protocol: &str, key: &str) -> Option<&orch_types::Json> {
        self.option_rule_at(protocol, &[key])
    }

    /// A nested option's rule, keeping local references rooted in the original schema.
    #[must_use]
    pub fn option_rule_at(&self, protocol: &str, path: &[&str]) -> Option<&orch_types::Json> {
        let schema = self.option_schema(protocol)?;
        let mut rule = schema;
        for (depth, key) in path
            .iter()
            .map(Some)
            .chain(std::iter::once(None))
            .enumerate()
        {
            let mut visited = std::collections::BTreeSet::new();
            while let Some(reference) = rule.get("$ref").and_then(orch_types::Json::as_str) {
                if rule.get("const").is_some() || !visited.insert(reference) {
                    break;
                }
                let Some(next) = reference
                    .strip_prefix('#')
                    .and_then(|pointer| schema.pointer(pointer))
                else {
                    break;
                };
                rule = next;
            }
            if let Some(key) = key {
                // The first step reads the composed group and model layers; deeper steps read inside that member.
                rule = if depth == 0 {
                    option_layers::option_rule(schema, key)?
                } else {
                    rule.get("properties")?.get(*key)?
                };
            }
        }
        Some(rule)
    }

    /// Checks manager-authored schemas with the same local validator used by extension forms.
    pub fn check_option_schemas(&self) -> Result<(), &'static str> {
        for schema in self
            .request_options
            .iter()
            .flat_map(|schemas| schemas.values())
        {
            if !schema.is_object() {
                return Err("request option constraints must be an object schema");
            }
            if !option_form_supported(schema) {
                return Err(
                    "request options use property, item, enum, const, default, range and local-reference schemas; conditional schemas have no supported option form",
                );
            }
            orch_types::schema::check(schema)
                .map_err(|_| "request option constraints contain an invalid schema")?;
        }
        Ok(())
    }

    /// Search modes explicitly enabled on this model. Disabled remains an explicit ordinary choice.
    #[must_use]
    pub fn web_search_modes(&self) -> &[String] {
        self.web_search.as_deref().unwrap_or_default()
    }

    /// A default never enables a mode that the manager has not enabled.
    #[must_use]
    pub fn web_search_start(&self) -> &str {
        self.default_web_search
            .as_deref()
            .filter(|mode| {
                self.web_search_modes()
                    .iter()
                    .any(|enabled| enabled == mode)
            })
            .unwrap_or("disabled")
    }

    /// The offered reasoning efforts, or none.
    #[must_use]
    pub fn efforts(&self) -> &[String] {
        self.efforts.as_deref().unwrap_or_default()
    }
    /// Reasoning modes explicitly enabled by a manager; an offer or default grants nothing.
    #[must_use]
    pub fn reasoning_modes(&self) -> &[String] {
        self.reasoning_modes.as_deref().unwrap_or_default()
    }
    /// The reasoning start only while it remains explicitly enabled.
    #[must_use]
    pub fn reasoning_mode_start(&self) -> Option<&str> {
        self.default_reasoning_mode
            .as_deref()
            .filter(|start| self.reasoning_modes().iter().any(|mode| mode == *start))
    }
    /// Whether the model reasons before it answers, as its parameters say: it takes efforts, or a
    /// reasoning mode starts on. What reasons spends part of the output allowance before any text.
    #[must_use]
    pub fn reasons(&self) -> bool {
        !self.efforts().is_empty() || self.reasoning_mode_start().is_some()
    }
    /// Service tiers explicitly enabled by a manager.
    #[must_use]
    pub fn service_tiers(&self) -> &[ServiceTier] {
        self.service_tiers.as_deref().unwrap_or_default()
    }
    /// Capacity selections explicitly enabled by the provider manager.
    #[must_use]
    pub fn capacity_tiers(&self) -> &[ServiceTier] {
        self.capacity_tiers.as_deref().unwrap_or_default()
    }
    /// A default does not enable a capacity selection.
    #[must_use]
    pub fn capacity_tier_start(&self) -> Option<&str> {
        self.default_capacity_tier
            .as_deref()
            .filter(|start| self.capacity_tiers().iter().any(|tier| tier.id == *start))
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

/// Keep newly configured option declarations within the existing form renderer's vocabulary.
/// Values still use the shared JSON Schema validator; this only checks what can be explained by a form.
fn option_form_supported(schema: &orch_types::Json) -> bool {
    if schema.is_boolean() {
        return true;
    }
    let Some(object) = schema.as_object() else {
        return false;
    };
    object.iter().all(|(key, value)| match key.as_str() {
        "properties" | "$defs" | "definitions" => value
            .as_object()
            .is_some_and(|fields| fields.values().all(option_form_supported)),
        "items" | "additionalProperties" => option_form_supported(value),
        "$ref" => value
            .as_str()
            .is_some_and(|reference| reference == "#" || reference.starts_with("#/")),
        "x-orch-replace" => value
            .as_bool()
            .is_some_and(|replace| !replace || object.contains_key("const")),
        "$schema" | "type" | "title" | "description" | "enum" | "const" | "default"
        | "required" | "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum"
        | "multipleOf" | "minLength" | "maxLength" | "pattern" | "format" | "minItems"
        | "maxItems" | "uniqueItems" | "minProperties" | "maxProperties" | "examples" => true,
        _ => key.starts_with("x-"),
    })
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
    /// Provenance of explicitly supplied values, keyed by JSON pointer in this suggestion.
    /// An unknown or absent origin never becomes vendor confirmation or enabled support.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub origins: BTreeMap<String, String>,
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
            origins: BTreeMap::new(),
        }
    }
}
