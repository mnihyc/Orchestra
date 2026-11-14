//! Interfaces, account offerings, capabilities and program support are data, with open identifiers.
use orch_types::{Id, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Unknown declaration members survive read/write but participate in no policy decision.
pub type Extensions = BTreeMap<String, Json>;

/// One extension's additive contribution; registering it cannot replace another declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Declarations {
    /// Version of the common declaration shape.
    pub contract: u32,
    /// Wire shapes, including generation, embedding and service interfaces.
    pub interfaces: BTreeMap<String, Interface>,
    /// Account instances with their actual endpoints and offerings.
    pub providers: BTreeMap<Id, Provider>,
    /// Program-independent capability semantics and their one setting/item/unit.
    pub capabilities: BTreeMap<String, Capability>,
    /// Support by program and access kind.
    pub programs: BTreeMap<String, Program>,
    /// Routes are registered declarations, not an exhaustive backend enum.
    pub routes: BTreeMap<String, Route>,
    /// Additive capability/mode/route links let an extension add a route without replacing common semantics.
    pub route_links: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    /// Ordered endpoint/model mappings owned by provider extensions; they never enable stored offerings.
    pub endpoint_rules: Vec<EndpointRule>,
    /// Future fields, preserved without granting support.
    #[serde(flatten)]
    pub extensions: Extensions,
}
impl Default for Declarations {
    fn default() -> Self {
        Self {
            contract: super::DECLARATION_CONTRACT,
            interfaces: BTreeMap::new(),
            providers: BTreeMap::new(),
            capabilities: BTreeMap::new(),
            programs: BTreeMap::new(),
            routes: BTreeMap::new(),
            route_links: BTreeMap::new(),
            endpoint_rules: Vec::new(),
            extensions: BTreeMap::new(),
        }
    }
}

/// A wire shape. Its open kind also admits a later service without a core enum change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Interface {
    /// Human name, from its extension.
    pub title: String,
    /// Generation, embedding, search, fetch, images, facts or a later declared kind.
    pub kind: String,
    /// Names the existing wire implementation; it is not a vendor selector.
    pub implementation: String,
    /// Native tools use the protocol option declaration shape unchanged.
    pub native_tools: BTreeMap<String, Binding>,
    /// Service request/result/counter/discovery mappings, interpreted by its implementation.
    pub shape: Binding,
    /// Recognized options; the existing constraint engine evaluates these schemas.
    pub options: Json,
    /// Future declaration fields.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// One uniform declaration of requests, common results, counters and prices.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Binding {
    /// Input schema for this capability; null retains the existing implementation's contract.
    pub input_schema: Json,
    /// A local schema over actual thread/offering facts; null imposes no extra predicate.
    pub eligible_when: Json,
    /// Existing native-tool declaration or service request shape.
    pub request: Json,
    /// Projection into the capability's existing common item.
    pub result: Json,
    /// Named counters with actual source pointers, never inferred item counts.
    pub counters: BTreeMap<String, Counter>,
    /// Declared quotes in their own currency/unit.
    pub prices: BTreeMap<String, Price>,
    /// Discovery mapping, which offers but never enables capabilities.
    pub discovery: Json,
    /// Future declaration fields.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// One measured quantity; absence is unknown unless the declaration explicitly says otherwise.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Counter {
    /// JSON pointer in a decoded response/frame.
    pub pointer: String,
    /// Explicit per-success quantity where the service bills one call rather than result count.
    pub constant: Option<i64>,
    /// Count actual completed rows only after their declared byte encoding decodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<CountItems>,
    /// The whole response must satisfy this evidence schema before absence can mean anything.
    pub valid_when: Json,
    /// Predicate over the decoded response; null imposes no extra condition.
    pub only_when: Json,
    /// Explicit quantity when that predicate is false, such as a documented free empty answer.
    pub otherwise: Option<i64>,
    /// Capture if reported; absence does not promise this otherwise unsupported quantity.
    pub optional: bool,
    /// The capability's ledger unit.
    pub unit: String,
    /// A new report replaces the prior cumulative snapshot rather than adding it.
    pub cumulative: bool,
    /// A lower cumulative snapshot explicitly starts again at zero; otherwise that interval is unknown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset_on_decrease: bool,
    /// Counts successful operations rather than attempts.
    pub successful_only: bool,
    /// Optional alternative vendor-reported cost, separate from an estimate.
    pub reported_cost: bool,
    /// Future declaration fields.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// Explicit actual-output cardinality; never a requested quantity or a citation count.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CountItems {
    pub pointer: String,
    /// Optional predicate selects completed outputs within a mixed native array.
    pub only_when: Json,
    /// Each selected row must contain nonempty standard base64 bytes at this pointer.
    pub base64: String,
}

/// A provider extension's endpoint-specific overrides, in declared overlay order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct EndpointRule {
    /// Provider-owned native configuration contributions, by program and its own setting name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub native_switches: BTreeMap<String, BTreeMap<String, NativeSwitch>>,
    /// Public model dialect facts owned by each program; never credentials or manager permissions.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub model_metadata: BTreeMap<String, Json>,
    /// Local schema over normalized protocol/scheme/host/port/path/model and URL trust facts.
    pub when: Json,
    /// Native overrides; null explicitly closes a capability instead of inheriting a compatible tool.
    pub native_tools: BTreeMap<String, Option<Binding>>,
    /// Additional endpoint declarations, such as the same account's service APIs.
    pub endpoints: BTreeMap<String, Endpoint>,
    /// Offered services; shipped declarations keep them disabled until a manager chooses them.
    pub offerings: BTreeMap<String, Offering>,
    /// Future members are retained without influencing eligibility or request generation.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// Exact decimal quote; conversion/rounding remains with the existing pricer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Price {
    /// Decimal text, never parsed through a float.
    pub amount: String,
    /// Original currency or the existing credits unit.
    pub currency: String,
    /// Charged quantity, such as token, search, fetch or image.
    pub unit: String,
    /// Future declaration fields.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// A provider is an account, never a hardcoded vendor kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Provider {
    /// Human account name; views require explicit readability facts before displaying it.
    pub title: String,
    /// Role names only. Secret material and admin/model keys are not declarations.
    pub credential_roles: Vec<String>,
    /// Actual addresses and interfaces under this account.
    pub endpoints: BTreeMap<String, Endpoint>,
    /// Models and services share one offering shape.
    pub offerings: BTreeMap<String, Offering>,
    /// Existing named option groups, kept as data.
    pub groups: BTreeMap<String, Json>,
    /// Existing facts-source reference, where one was chosen.
    pub facts_source: Option<String>,
    /// Future declaration fields.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// An address plus a declared interface; existing endpoint options keep their meaning.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Endpoint {
    /// Actual address, never displayed merely because a capability is usable.
    pub address: String,
    /// Interface id in the same registry.
    pub interface: String,
    /// Required existing credential role, where any.
    pub credential_role: Option<String>,
    /// Existing endpoint options.
    pub options: Json,
    /// Endpoint overrides; an explicit null declares no support and does not inherit a tool.
    pub native_tools: BTreeMap<String, Option<Binding>>,
    /// Future declaration fields.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// A stored model is an offering on its existing generation/embedding interface.
/// Its raw parameters are retained, not rewritten into a replacement stored format.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Offering {
    /// Endpoint/model-specific native feature declarations, never credentials or current authority.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub native_switches: BTreeMap<String, BTreeMap<String, NativeSwitch>>,
    /// Human model/service name.
    pub title: String,
    /// Existing endpoint id.
    pub endpoint: String,
    /// Model/service/later offering kind, supplied by its extension.
    pub kind: String,
    /// A manager enabled this offering; discovery alone never sets it.
    pub enabled: bool,
    /// Explicitly enabled capability/modes on this offering.
    pub capabilities: BTreeMap<String, Support>,
    /// Offering-specific native bindings; null explicitly closes support.
    pub native_tools: BTreeMap<String, Option<Binding>>,
    /// Existing group names.
    pub groups: Vec<String>,
    /// Existing model/service parameter JSON, including unknown additive members.
    pub parameters: Json,
    /// Existing offering constraints, validated by the ordinary request path.
    pub options: Json,
    /// Original per-unit quotes.
    pub prices: BTreeMap<String, Price>,
    /// Future declaration fields.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// Explicit support, with no inference from a model or program name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Support {
    /// An explicit support/enable fact.
    pub enabled: bool,
    /// Supported mode values; empty means no explicit mode can be selected.
    pub modes: Vec<String>,
    /// Known program default; absent remains unknown rather than off.
    pub default_mode: Option<String>,
    /// Native mode explicitly declared for a short delegated helper, independently of the unset default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delegated_mode: Option<String>,

    /// Input/result codecs and execution constraints verified by the actual program adapter.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub execution: Vec<ExecutionSupport>,

    /// Optional interface restriction.
    pub interfaces: Vec<String>,
    /// Additional actual-fact predicate.
    pub eligible_when: Json,
    /// Future declaration fields.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// An installed codec and its verified execution facilities, independent of provider names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ExecutionSupport {
    /// The exact input codec, including its version where its shape changes.
    pub input: String,
    /// The exact result codec; a text answer never proves another result family.
    pub result: String,
    /// Native incremental events and durable replay are implemented.
    pub streaming: bool,
    /// A source-bound read-tool bridge is implemented.
    pub read_tools: bool,
    /// Native tools are actually confined to the consumer's admitted catalogue.
    pub confined_tools: bool,
}

impl ExecutionSupport {
    /// A declared requirement needs both its exact codecs and every requested facility.
    #[must_use]
    pub fn satisfies(&self, required: &Self) -> bool {
        !required.input.is_empty()
            && !required.result.is_empty()
            && self.input == required.input
            && self.result == required.result
            && (!required.streaming || self.streaming)
            && (!required.read_tools || self.read_tools)
            && (!required.confined_tools || self.confined_tools)
    }
}

/// One capability declares its one setting, common item and ledger unit for every program.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Capability {
    /// Human name or a catalogue-resolved name supplied by the producer.
    pub title: String,
    /// The same setting key for every program.
    pub setting: String,
    /// Optional mode-valued setting; absent when the setting selects an offering rather than a mode.
    pub mode_setting: Option<String>,
    /// Existing call-rule purpose for this capability, where recovery applies.
    pub recovery_purpose: Option<String>,
    /// Organization's optional exact backend choice, where service routes are offered.
    pub backend_setting: Option<String>,
    /// Existing common thread item/presentation family.
    pub item: String,
    /// Existing ledger unit to which declared counters map.
    pub ledger_unit: String,
    /// Optional native-counter compatibility measurement and rate pointer; no vendor dispatch is implied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counter_accounting: Option<CounterAccounting>,
    /// Explicit modes and their ordered route preferences.
    pub modes: BTreeMap<String, Mode>,
    /// Capability default only where the program declares none.
    pub default_mode: Option<String>,
    /// Read-only projection of existing stored offering members, where support predates this contract.
    pub offering_support: Option<SupportProjection>,
    /// Future declaration fields.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// How an aggregate quantity joins the existing ledger's measurements and model rate book.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CounterAccounting {
    /// Existing numeric measurement name, where the capability predates generic counts.
    pub measurement: Option<String>,
    /// JSON pointer in the selected, copied model rate. An absent price stays unknown.
    pub price: Option<String>,
    /// Future members do not change a stored model or the meaning of its counters.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// Existing stored model JSON is interpreted on read, never rewritten when a declaration is added.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SupportProjection {
    /// Optional boolean pointer in the offering (including its untouched parameters).
    pub enabled: Option<String>,
    /// Optional enabled-mode list pointer. An empty/missing list enables no native mode.
    pub modes: Option<String>,
    /// Optional known default-mode pointer.
    pub default_mode: Option<String>,
    /// Future projection fields.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// A mode chooses routes, not vendors; an empty route list is the off mode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Mode {
    /// Human mode name.
    pub title: String,
    /// Route ids in declared preference order.
    pub routes: Vec<String>,
    /// Value for the program's native switch; null leaves its default untouched.
    pub native_value: Json,
    /// Future declaration fields.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// A program declares each access kind independently.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Program {
    /// Human program name.
    pub title: String,
    /// Subscription, forwarded or a later named access kind.
    pub access: BTreeMap<String, ProgramAccess>,
    /// Public native control names and their finite constant values; never credentials or account data.
    pub native_switches: BTreeMap<String, NativeSwitch>,
    /// Static program explanations, filtered against actual facts without changing support or authority.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<ProgramFact>,
    /// Future declaration fields.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// A program owns the spelling and values of each native switch, independently of a backend.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct NativeSwitch {
    /// The capability whose resolved native value controls this setting.
    pub capability: String,
    /// Native-value tokens mapped to public scalar constants actually accepted by the program.
    pub values: BTreeMap<String, Json>,
    /// Predicate over this resolution's actual program, access, mode, offering and endpoint facts.
    pub when: Json,
    /// An explicit public scalar feature default, including when no mode override was requested.
    pub value: Option<Json>,
    /// Public scalar used when a route is admitted and this switch's predicate matches.
    /// Opting into this also publishes `value` when no matching route is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_value: Option<Json>,
    /// The program explicitly permits provider declarations to refine this feature within its values.
    /// Mode switches stay provider-independent unless their program deliberately opens this.
    pub provider_overrides: bool,
    /// Future declaration fields do not enter a session's native configuration.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// Support belongs to the actual access kind, not a program-wide guess.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ProgramAccess {
    /// Generation/embedding interfaces this access actually speaks.
    pub interfaces: Vec<String>,
    /// Model-detail fields whose changes require a fresh native session on this access path.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub model_details: Vec<String>,
    /// Native support and known defaults by capability.
    pub native: BTreeMap<String, Support>,
    /// Whether this access can carry Orch's own tools.
    pub orch_tools: bool,
    /// Explanations specific to this credential path; they do not enable or suppress a route.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<ProgramFact>,
    /// Per-model native counters, decoded from the program's actual report by the shared counter module.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub counters: BTreeMap<String, Counter>,
    /// Future declaration fields.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// An extension's static, localized explanation about how its program carries a capability.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ProgramFact {
    /// Stable identity within this program's declaration.
    pub id: String,
    /// Capability rows this explains; empty applies to every capability.
    pub capabilities: Vec<String>,
    /// Optional predicate over actual thread facts, such as the selected arrangement. Missing facts never match.
    pub when: Json,
    /// Localized public text authored by the extension, without account metadata or credentials.
    pub text: BTreeMap<String, String>,
    /// Future members are retained but are not published or used as policy.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// Only authored static words enter an inspector fact, never its private extension members or predicates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramFactView {
    /// Declaration identity.
    pub id: String,
    /// Localized explanation.
    pub text: BTreeMap<String, String>,
}

/// An open route declaration. A program-session executor can be registered later without
/// changing a backend enum or any vendor/protocol dispatch in this core.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Route {
    /// Human name supplied by the extension, independent of its stable route id.
    pub title: String,
    /// Authored localized names; an absent language uses the supplied title.
    pub text: BTreeMap<String, String>,
    /// Native-model, native-program, service or a later route kind.
    pub kind: String,
    /// Capabilities this route can answer.
    pub capabilities: Vec<String>,
    /// Registered execution adapter; absence is permitted for an existing native program path.
    pub executor: Option<String>,
    /// Required codecs/facilities; only trusted installed support can satisfy these facts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionSupport>,
    /// Requires this program/access's explicit native support.
    pub requires_program_support: bool,
    /// The program and installed transport must carry the current generation interface.
    pub requires_generation_interface: bool,
    /// The route needs its selected offering's installed wire/service interface, independent of the harness.
    pub requires_interface: bool,
    /// An exact offering and its current authority facts must be present, including for delegated sessions.
    pub requires_offering: bool,
    /// Requires the offering's manager-enabled capability.
    pub requires_offering_support: bool,
    /// Native mode used by an auxiliary offering, where the thread mode is a service mode.
    pub offering_mode: Option<String>,
    /// Requires a declared native-tool binding.
    pub requires_native_tool: bool,
    /// Uses an organization's explicit backend reference rather than the current model.
    pub uses_backend: bool,
    /// Never selected by unset/default, even if it is the only possible route.
    pub requires_opt_in: bool,
    /// Requires the program's MCP/Orch tool support.
    pub requires_orch_tools: bool,
    /// Exactly this tool is offered on this route; no tool on native routes.
    pub orch_tool: Option<String>,
    /// Disable the native tool before offering the Orch service tool.
    pub disable_native: bool,
    /// Extra generic fact predicate; uses the existing schema evaluator.
    pub eligible_when: Json,
    /// Future declaration fields.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// Exact selected offering; no first-provider/cheapest-provider fallback is implied.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct OfferingRef {
    /// Account identity.
    pub provider: Id,
    /// Provider-local model/service identity.
    pub offering: String,
}

/// Organization's exact route/offering choice. Extra route data accommodates later delegation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct BackendRef {
    /// Registered route id, not a core-owned vendor/backend enum.
    pub route: String,
    /// Exact selected account offering, where the route uses one.
    pub offering: Option<OfferingRef>,
    /// Opaque adapter-owned references, never credential material or reusable admission.
    pub config: Json,
    /// Future declaration fields.
    #[serde(flatten)]
    pub extensions: Extensions,
}

impl BackendRef {
    /// A stable selection identity includes placement/options and ignores object member order.
    pub fn identity(&self) -> Result<String, serde_json::Error> {
        fn ordered(value: Json) -> Json {
            match value {
                Json::Object(members) => {
                    let sorted: BTreeMap<_, _> = members.into_iter().collect();
                    Json::Object(
                        sorted
                            .into_iter()
                            .map(|(key, value)| (key, ordered(value)))
                            .collect(),
                    )
                }
                Json::Array(values) => Json::Array(values.into_iter().map(ordered).collect()),
                value => value,
            }
        }
        serde_json::to_string(&ordered(serde_json::to_value(self)?))
    }

    /// Validate identities without making a route, credential or authority from them.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.route.trim().is_empty() {
            return Err("Choose a capability route.");
        }
        if self.offering.as_ref().is_some_and(|offering| {
            offering.provider.is_nil() || offering.offering.trim().is_empty()
        }) {
            return Err("Choose an existing provider and a nonempty model or service.");
        }
        Ok(())
    }
}

/// Current authorization/allowance facts supplied by the existing account permission path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Availability {
    /// Current ordinary Use, scope and allowance all permit this route.
    pub allowed: bool,
    /// Safe current refusal reason, without private source bodies.
    pub reason: Option<String>,
    /// Provider reference/name may be shown under the caller's existing Read rules.
    pub show_provider: bool,
    /// Offering reference/name may be shown.
    pub show_offering: bool,
    /// The actual quote may be shown.
    pub show_price: bool,
}

/// Actual per-capability backend facts, prepared by the existing source/Use suppliers.
/// The same account/model can use a different interface for auxiliary work without rewriting
/// its primary conversation offering. This private snapshot is never a serialized grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedBackend {
    pub reference: OfferingRef,
    /// Native program routes carry no invented wire endpoint.
    pub endpoint: Option<Endpoint>,
    pub offering: Offering,
    pub availability: Availability,
}

/// One actual thread snapshot. This carries facts, never authorizes or extends their lifetime.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ThreadContext {
    /// Actual program id.
    pub program: String,
    /// Actual subscription/forwarded/later access kind.
    pub access: String,
    /// Current generation offering, where known.
    pub current: Option<OfferingRef>,
    /// Explicit stored mode values; missing/null keep the program default.
    pub choices: BTreeMap<String, Option<String>>,
    /// Explicit organization backend choices by capability.
    pub backends: BTreeMap<String, BackendRef>,
    /// Actual installed backend endpoint/ceiling and current permission facts for this snapshot.
    /// Never carry these private endpoints or parameters through a session/API serialization.
    #[serde(skip)]
    pub backend_offerings: BTreeMap<String, SelectedBackend>,
    /// Current permitted/visible offering facts. Missing is denied, never an account session.
    pub availability: BTreeMap<Id, BTreeMap<String, Availability>>,
    /// Actual available credential roles by account, no secrets.
    pub credential_roles: BTreeMap<Id, Vec<String>>,
    /// Execution adapters actually installed in this process.
    pub executors: Vec<String>,
    /// Actual installed codecs, supplied by trusted execution code rather than serialized claims.
    #[serde(skip)]
    pub executor_support: BTreeMap<String, Vec<ExecutionSupport>>,
    /// Interface implementations actually installed, independently of their advertised declarations.
    pub interfaces: Vec<String>,
    /// Safe additional facts for declared eligibility schemas.
    pub facts: Json,
    /// The already resolved ordinary call rules and their safe scope label, for an inspector preview only.
    pub recovery: Option<RecoveryPolicy>,
}

/// Already resolved call-rule policy; this is a preview and never reserves or executes a fallback.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RecoveryPolicy {
    /// Rules from the existing inherited setting.
    pub rules: Vec<crate::calls::Rule>,
    /// Safe scope kind, without an unreadable enclosing place's identity or name.
    pub scope: String,
}

/// Internal resolved route; execution still uses the ordinary producer/guard on every call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedRoute {
    /// Registered route id.
    pub id: String,
    /// Declared route kind.
    pub kind: String,
    /// Registered executor, where any.
    pub executor: Option<String>,
    /// Exact consumer requirements retained from the resolved declaration until execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionSupport>,
    /// Exact selected offering, never an inferred provider.
    pub offering: Option<OfferingRef>,
    /// Opaque exact backend selection, where any.
    pub backend: Option<BackendRef>,
    /// The only Orch tool offered, where this is a service route.
    pub orch_tool: Option<String>,
    /// Native tool must be off before that Orch tool is offered.
    pub disable_native: bool,
    /// Null means leave the program default, otherwise the declared native switch value.
    pub native_value: Json,
}

/// The route inspector's bounded, permission-filtered provider reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderView {
    /// Readable account id.
    pub id: Id,
    /// Readable account name.
    pub title: String,
}

/// Same route result supplies dispatch planning, the inspector and the derived matrix.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Resolution {
    /// Capability id.
    pub capability: String,
    /// Human capability name.
    pub title: String,
    /// Its one shared setting.
    pub setting: String,
    /// Explicit mode or known native default; null is an unreported program default.
    pub mode: Option<String>,
    /// Available mode values derived from declarations and current facts.
    pub modes: Vec<String>,
    /// Whether the native default, rather than an explicit stored choice, applies.
    pub program_default: bool,
    /// Resolved route for the existing execution path; absent is unavailable/off/default unknown.
    #[serde(skip)]
    pub route: Option<ResolvedRoute>,
    /// Credential-free native controls from the same resolved route and the program's declaration.
    /// Empty leaves the program default untouched; this is no execution or authority proof.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub native_config: BTreeMap<String, Json>,
    /// Public route facts exclude private backend config, endpoint addresses and unreadable ids.
    pub route_view: Option<RouteView>,
    /// Safe unavailable/off/default reason code.
    pub reason: Option<String>,
    /// Safe supplier-owned reason detail, if any.
    pub detail: Option<String>,
    /// Permission-filtered inspector account.
    pub provider: Option<ProviderView>,
    /// Readable offering id; hidden when its names/content are not permitted.
    pub offering: Option<String>,
    /// Exact declared quotes only where readable.
    pub prices: BTreeMap<String, Price>,
    /// Capability's common item family.
    pub item: String,
    /// Capability's ledger unit.
    pub ledger_unit: String,
    /// Configured conditional candidates from the SAME calls.rules, never an inferred fallback.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<FallbackView>,
    /// Program/access explanations from the same declaration and snapshot; they never decide availability.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<ProgramFactView>,
}

/// Safe configured fallback facts, independently of whether a particular failure has occurred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FallbackView {
    /// Authored rule id.
    pub rule_id: String,
    /// Current setting's safe scope kind.
    pub scope: String,
    /// Failure kind, or any when unconstrained.
    pub when: String,
    /// Provider reference only where the SAME context says it may be read.
    pub provider: Option<ProviderView>,
    /// Offering only where readable; private backend configuration is never included.
    pub offering: Option<String>,
}

/// The inspector never receives the internal backend configuration or unreadable account references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RouteView {
    /// Declared route id.
    pub id: String,
    /// Native-model, native-program, service or a later declared kind.
    pub kind: String,
    /// Whether the native capability switch is disabled for this route.
    pub disable_native: bool,
    /// Exactly the one Orch tool chosen, where any.
    pub orch_tool: Option<String>,
}

/// A matrix cell is a projection of the same resolver, not a hand-authored support assertion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MatrixCell {
    /// Program row.
    pub program: String,
    /// Actual access-kind column.
    pub access: String,
    /// Exact current offering used for this cell, where any.
    pub current: Option<OfferingRef>,
    /// Same resolution shown by a thread inspector.
    pub resolution: Resolution,
}
