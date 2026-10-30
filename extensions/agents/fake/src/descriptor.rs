//! Configurable capabilities, arrangement types, executors and tool sets, credential forms, each kind of memory, and a sign-in method of each kind with every field type, one of which takes a file's text or its path.
//!
//! What the fake declares is everything: all five arrangement types, every capability, three
//! credential forms, one sign-in method of each kind. A profile's `config` takes parts of it away or
//! changes them ([`Profile`]), so that a test can stand for a harness that can do less: one that
//! cannot be separated from its tools (`separable: false`), one with another kind of memory, one with
//! no steering, a `separated` entry that works through another executor.

use std::collections::BTreeMap;

use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::credentials::{CredentialForm, CredentialSupport, Custody};
use orch_agent_contract::descriptor::{
    AgentDescriptor, AgentTerminal, ApprovalMode, ArrangementEntry, ArrangementType,
    AuxiliaryModels, Capabilities, EffortSupport, ExecutorBuild, HarnessInfo, McpSupport,
    MemoryKind, NativeProvider, OrchToolsSupport, ProviderFactKind, SandboxMode, SessionReload,
    SkillsSupport, UsageContext, classify_version,
};
use orch_agent_contract::discovery::{McpTransport, ModelSuggestion};
use orch_agent_contract::events::HarnessRequestKind;
use orch_agent_contract::login::{LoginKind, LoginMethodDef, LoginYields};
use orch_agent_contract::parameters::{
    Condition, DefaultFromModel, ModelFeature, ModelLimit, ModelList, ModelParameters, Offered,
    ParameterApply, ParameterDef, ParameterDefault, ParameterGroup, ParameterOptions,
    ParameterRange, ParameterScope, ParameterType, RangeMax, ServiceTier,
};
use orch_plugin_contract::Extensions;
use orch_types::{Choice, Field, FieldType, Json};
use serde::{Deserialize, Serialize};

use crate::events::Fault;

/// The kind the fake declares.
pub const KIND: &str = "fake";
/// The version of the fake's declarations.
pub const ADAPTER_VERSION: u32 = 1;
/// The version of the fake harness unless a profile names another: the first of a range it supports.
pub const HARNESS_VERSION: &str = "1.0.0";
/// The versions the fake supports.
pub const SUPPORTED_RANGE: &str = ">=1.0.0, <2.0.0";
/// The form of a copy that lapses.
pub const FORM_TOKEN: &str = "fake_token";
/// The form of a copy that lasts.
pub const FORM_KEY: &str = "api_key";
/// The form of a login that moves.
pub const FORM_LOGIN: &str = "fake_login";
/// How long a copy of the form `fake_token` is good for: two minutes, as the spec has it.
pub const TOKEN_LIFETIME_SECS: i64 = 120;
/// The sign-in by a code the person enters at an address.
pub const METHOD_CODE: &str = "fake_code";
/// The sign-in that shows an address and asks for a text back.
pub const METHOD_ASK: &str = "fake_ask";
/// The sign-in by a credential the person brings.
pub const METHOD_IMPORT: &str = "fake_import";
/// How long the token of the sign-in `fake_import` is good for: ten minutes, as the spec has it.
pub const IMPORT_TOKEN_LIFETIME_SECS: i64 = 600;
/// The models the fake lists.
pub const MODELS: [&str; 2] = ["fake-small", "fake-large"];

fn yes() -> bool {
    true
}

fn default_code_lifetime() -> u32 {
    // The usual life of a device code is a quarter of an hour; ten minutes is a round figure under it.
    600
}

/// What a new provider starts with when the profile says nothing: the two types in which the key stays
/// where it is kept, or, for a harness that cannot be separated from its tools, the least further type
/// (a copy of the credential).
fn default_new_provider_allows(separable: bool) -> Vec<ArrangementType> {
    if separable {
        vec![ArrangementType::Separated, ArrangementType::KeySeparated]
    } else {
        vec![ArrangementType::CredentialShared]
    }
}

fn default_memory() -> MemoryKind {
    MemoryKind::Directory
}

/// A `separated` entry of the declaration: an executor, the build it has to be, the tool sets it brings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeparatedEntry {
    /// The executor.
    pub executor: String,
    /// Which build it has to be: `same`, `*` or a semver range.
    #[serde(default = "any_build")]
    pub executor_version: ExecutorBuild,
    /// The tool sets of the executor the harness can work with.
    #[serde(default)]
    pub tool_sets: Option<Vec<String>>,
}

fn any_build() -> ExecutorBuild {
    ExecutorBuild::Any
}

/// What a profile's `config` can say. Every member is optional; a member that is not given is the
/// fake's own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Profile {
    /// Whether the harness can be separated from its tools and call an endpoint other than its
    /// vendor's. `false` removes `separated` and `key_separated`: a harness that can only run beside
    /// its tools on its own upstream.
    #[serde(default = "yes")]
    pub separable: bool,
    /// What a new provider of the fake starts with (`native_provider.arrangements`); `separated` and
    /// `key_separated` unless the harness cannot be separated, then a copy of the credential.
    pub new_provider_allows: Option<Vec<ArrangementType>>,
    /// What the harness keeps for itself from one session to the next: `directory`, `home` or `none`.
    #[serde(default = "default_memory")]
    pub memory: MemoryKind,
    /// A patch merged over the capabilities (a member given as `null` is taken away).
    pub capabilities: Json,
    /// The `separated` entries instead of the one through `orch-exec`.
    pub separated: Option<Vec<SeparatedEntry>>,
    /// The forms it issues and the names it runs on, instead of its three.
    pub credentials: Option<CredentialSupport>,
    /// The approval modes it has, instead of all three.
    pub approval_modes: Option<Vec<ApprovalMode>>,
    /// The sandbox modes it can enforce, instead of all three.
    pub sandbox_modes: Option<Vec<SandboxMode>>,
    /// How its asides read, instead of none.
    pub side_thread_modes: Option<Vec<orch_agent_contract::descriptor::SideReading>>,
    /// The protocols it speaks to a forwarder, instead of `openai-responses` and `anthropic-messages`.
    pub forwarded_protocols: Option<Vec<String>>,
    /// The version of the harness it reports, instead of [`HARNESS_VERSION`].
    pub harness_version: Option<String>,
    /// How many seconds the code of the sign-in `fake_code` is good for.
    #[serde(default = "default_code_lifetime")]
    pub code_lifetime_secs: u32,
    /// The tool servers the harness has configured on this machine: what `mcp.discover` finds.
    pub mcp_servers: Vec<ConfiguredServer>,
    /// Failures to inject, by the call that is to fail.
    pub faults: BTreeMap<String, Fault>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            separable: true,
            new_provider_allows: None,
            memory: default_memory(),
            capabilities: Json::Null,
            separated: None,
            credentials: None,
            approval_modes: None,
            sandbox_modes: None,
            side_thread_modes: None,
            forwarded_protocols: None,
            harness_version: None,
            code_lifetime_secs: default_code_lifetime(),
            mcp_servers: Vec::new(),
            faults: BTreeMap::new(),
        }
    }
}

/// A tool server the fake harness has configured, as its own configuration names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredServer {
    /// Its name.
    pub name: String,
    /// The program and its arguments, for a stdio server.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// The address, for an http server.
    #[serde(default)]
    pub url: Option<String>,
    /// Its variables.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// The names of the variables whose values are secrets, and the values.
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
}

impl ConfiguredServer {
    /// How it is reached.
    #[must_use]
    pub fn transport(&self) -> McpTransport {
        if self.url.is_some() {
            McpTransport::Http
        } else {
            McpTransport::Stdio
        }
    }
}

impl Profile {
    /// Reads the `config` of an agent profile; `null` is the fake as it comes.
    pub fn from_config(config: &Json) -> Result<Self, AdapterError> {
        if config.is_null() {
            return Ok(Self::default());
        }
        serde_json::from_value(config.clone()).map_err(|error| AdapterError::Other {
            code: "invalid_config".to_owned(),
            message: format!("the profile of the fake harness: {error}"),
            retryable: false,
        })
    }
}

/// The forms the fake issues from its login.
#[must_use]
pub fn credential_forms() -> Vec<CredentialForm> {
    vec![
        CredentialForm {
            form: FORM_TOKEN.to_owned(),
            custody: Custody::Shared,
            lifetime_secs: Some(TOKEN_LIFETIME_SECS),
        },
        CredentialForm {
            form: FORM_KEY.to_owned(),
            custody: Custody::Shared,
            lifetime_secs: None,
        },
        CredentialForm {
            form: FORM_LOGIN.to_owned(),
            custody: Custody::Transferred,
            lifetime_secs: None,
        },
    ]
}

/// Every capability, and the kind of memory the profile gives.
#[must_use]
pub fn full_capabilities(memory: MemoryKind) -> Capabilities {
    Capabilities {
        steer: true,
        interrupt: true,
        background_commands: true,
        approvals: vec![
            HarnessRequestKind::Command,
            HarnessRequestKind::FileChange,
            HarnessRequestKind::Permissions,
            HarnessRequestKind::UserInput,
            HarnessRequestKind::McpElicitation,
        ],
        subagents: true,
        subagent_limits: Vec::new(),
        plan: true,
        plan_mode: false,
        plan_mechanics: None,
        reasoning_summary: true,
        input_images: true,
        effort: EffortSupport::Turn,
        mcp_discovery: true,
        mcp: McpSupport {
            stdio: true,
            http: true,
            // This adapter takes a changed set when the same conversation is resumed.
            reload: false,
            strict: true,
        },
        orch_tools: OrchToolsSupport::Mcp,
        export_native: true,
        import_native: true,
        import_neutral: true,
        fork_native: true,
        native_input_prefix: None,
        resume: true,
        discover_sessions: true,
        usage_tokens: true,
        usage_context: UsageContext::Breakdown,
        phases: true,
        provider_facts: vec![
            ProviderFactKind::Plan,
            ProviderFactKind::Windows,
            ProviderFactKind::PlanCredits,
        ],
        model_discovery: true,
        title: true,
        auxiliary_models: AuxiliaryModels::Pinned,
        telemetry_off: true,
        memory,
        agent_terminal: AgentTerminal::None,
        session_reload: SessionReload::OnChange,
        instruction_root_marker: false,
        skills: SkillsSupport::Directory,
        approval_reviewer: None,
    }
}

/// The sign-in methods of the fake: one of each kind.
#[must_use]
pub fn login_methods() -> Vec<LoginMethodDef> {
    let mut every_field = vec![
        optional(Field::new("account", "Account name", FieldType::Text)),
        optional(Field::new("token", "Token", FieldType::Secret)),
        optional(Field::new(
            "credential_text",
            "Credential file (its text)",
            FieldType::SecretText,
        )),
        optional(Field::new(
            "credential_path",
            "Credential file (its path in the runtime)",
            FieldType::Text,
        )),
    ];
    let mut tier = Field::new("tier", "Tier", FieldType::Choice);
    tier.required = false;
    tier.choices = Some(
        ["free", "pro", "team"]
            .map(|value| Choice {
                value: value.to_owned(),
                label: value.to_uppercase(),
            })
            .to_vec(),
    );
    every_field.push(tier);
    every_field.push(optional(Field::new(
        "verified",
        "The account is verified",
        FieldType::Boolean,
    )));
    every_field.push(optional(Field::new(
        "expires_at",
        "The token ends on",
        FieldType::Date,
    )));
    every_field.push(optional(Field::new("labels", "Labels", FieldType::Map)));
    every_field.push(optional(Field::new(
        "headers",
        "Headers",
        FieldType::SecretMap,
    )));
    vec![
        LoginMethodDef {
            id: METHOD_CODE.to_owned(),
            title: "Sign in with a code".to_owned(),
            summary: "Shows an address and a code. It completes when the person answers, or ends when the code runs out."
                .to_owned(),
            kind: LoginKind::Interactive,
            fields: Vec::new(),
            yields: LoginYields::Login,
            lifetime_secs: None,
            requires: None,
            recommended: true,
        },
        LoginMethodDef {
            id: METHOD_ASK.to_owned(),
            title: "Sign in on a page".to_owned(),
            summary: "Shows an address, then asks for the text that the page shows."
                .to_owned(),
            kind: LoginKind::Interactive,
            fields: Vec::new(),
            yields: LoginYields::Login,
            lifetime_secs: None,
            requires: None,
            recommended: false,
        },
        LoginMethodDef {
            id: METHOD_IMPORT.to_owned(),
            title: "Bring a credential".to_owned(),
            summary: "Takes a credential the person already has: its file's text or its path, and the details beside it."
                .to_owned(),
            kind: LoginKind::Import,
            fields: every_field,
            yields: LoginYields::Token,
            lifetime_secs: Some(IMPORT_TOKEN_LIFETIME_SECS),
            requires: Some("A credential file, or its path inside the provider's runtime.".to_owned()),
            recommended: false,
        },
    ]
}

fn optional(mut field: Field) -> Field {
    field.required = false;
    field
}

/// The JSON Schema of what a provider of the fake may configure: a tenant.
#[must_use]
pub fn options_schema() -> Json {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "tenant": {
                "type": "string",
                "title": "Tenant",
                "description": "The tenant of the fake vendor that the account belongs to."
            }
        }
    })
}

/// The least context the fake takes, in tokens: the range of its `context` begins here.
pub const CONTEXT_LEAST: i64 = 1_000;
/// The largest context the fake lists as on offer for a model that offers a larger one: four times its window.
const CONTEXT_OFFERED_FACTOR: i64 = 4;

/// The parameters of the fake: one of each kind that reads the model's own entry, so that every path of a
/// speed, an access program and a context is taken without a real harness. Each starts with what the entry
/// says a thread on the model starts with; the context, with all that is open unless the entry names another
/// start.
#[must_use]
pub fn parameters() -> Vec<ParameterDef> {
    let scopes = vec![
        ParameterScope::Org,
        ParameterScope::Project,
        ParameterScope::Topic,
        ParameterScope::Thread,
    ];
    vec![
        ParameterDef {
            key: "service_tier".to_owned(),
            title: "Speed".to_owned(),
            description: "A faster way of running a turn, where the model's managers enabled one.".to_owned(),
            group: ParameterGroup::Model,
            kind: ParameterType::Enum,
            options: Some(ParameterOptions::FromModel {
                from_model: ModelList::ServiceTiers,
            }),
            range: None,
            default: ParameterDefault::FromModel {
                from_model: DefaultFromModel::DefaultServiceTier,
            },
            scopes: scopes.clone(),
            apply: ParameterApply::NextTurn,
            eligible_when: Some(Condition::ModelHas(ModelFeature::ServiceTiers)),
            feature: None,
        },
        ParameterDef {
            key: "access_program".to_owned(),
            title: "Access program".to_owned(),
            description: "A program that widens what the model may be asked to do, where the model's managers enabled one.".to_owned(),
            group: ParameterGroup::Safety,
            kind: ParameterType::Enum,
            options: Some(ParameterOptions::FromModel {
                from_model: ModelList::AccessPrograms,
            }),
            range: None,
            default: ParameterDefault::FromModel {
                from_model: DefaultFromModel::DefaultAccessProgram,
            },
            scopes: scopes.clone(),
            apply: ParameterApply::NextTurn,
            eligible_when: Some(Condition::ModelHas(ModelFeature::AccessPrograms)),
            feature: None,
        },
        ParameterDef {
            key: "context".to_owned(),
            title: "Context".to_owned(),
            description: "How much of a conversation the fake keeps, in tokens, up to the largest its model's managers opened. It never shortens a conversation: this only says how large it could be.".to_owned(),
            group: ParameterGroup::Model,
            kind: ParameterType::Integer,
            options: None,
            range: Some(ParameterRange {
                min: CONTEXT_LEAST,
                max: RangeMax::FromModel {
                    from_model: ModelLimit::ContextMax,
                },
            }),
            default: ParameterDefault::FromModel {
                from_model: DefaultFromModel::DefaultContext,
            },
            scopes,
            apply: ParameterApply::NextTurn,
            eligible_when: Some(Condition::ModelHas(ModelFeature::ContextMax)),
            feature: None,
        },
    ]
}

/// The models the fake lists.
#[must_use]
pub fn models() -> Vec<ModelSuggestion> {
    MODELS
        .iter()
        .map(|name| {
            let mut suggestion = ModelSuggestion::named(name, Some(&display_name(name)));
            suggestion.parameters = ModelParameters {
                efforts: Some(vec![
                    "low".to_owned(),
                    "medium".to_owned(),
                    "high".to_owned(),
                ]),
                default_effort: Some("medium".to_owned()),
                context_window: Some(crate::events::CONTEXT_WINDOW),
                input_images: Some(true),
                // The large model has a faster speed, an access program and a larger context to offer, none
                // of which is open until a manager of the provider opens it.
                offered: (*name == "fake-large").then(|| Offered {
                    service_tiers: Some(vec![ServiceTier {
                        id: "fast".to_owned(),
                        label: "Fast".to_owned(),
                    }]),
                    access_programs: Some(vec!["early".to_owned()]),
                    context_max: Some(crate::events::CONTEXT_WINDOW * CONTEXT_OFFERED_FACTOR),
                }),
                ..ModelParameters::default()
            };
            suggestion
        })
        .collect()
}

fn display_name(model: &str) -> String {
    let mut words = model.split('-').map(|word| {
        let mut letters = word.chars();
        letters
            .next()
            .map(|first| first.to_uppercase().chain(letters).collect::<String>())
            .unwrap_or_default()
    });
    words.by_ref().collect::<Vec<_>>().join(" ")
}

/// What the program of one fake adapter is: its kind and its words. The adapter is the scripted one for every
/// program whose manifest names it as its protocol, so that a program with no code of its own can be had for a test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// The kind it is declared as.
    pub kind: &'static str,
    /// What it is called in English.
    pub name: String,
    /// What it is called in the other languages, by language.
    pub names: BTreeMap<String, String>,
}

impl Program {
    /// The fake program Orch ships.
    #[must_use]
    pub fn shipped() -> Self {
        Self::of(orch_agent_programs::shipped(KIND))
    }

    /// The program a manifest describes.
    #[must_use]
    pub fn of(manifest: &orch_agent_programs::ProgramManifest) -> Self {
        Self {
            kind: orch_agent_programs::intern(&manifest.kind),
            name: manifest.name().to_owned(),
            names: manifest.names.others(),
        }
    }
}

/// What the fake declares for `profile`, running as `harness_version`.
pub fn descriptor(
    profile: &Profile,
    harness_version: &str,
) -> Result<AgentDescriptor, AdapterError> {
    descriptor_of(&Program::shipped(), profile, harness_version)
}

/// What the fake declares for `profile` as `program`, running as `harness_version`.
pub fn descriptor_of(
    program: &Program,
    profile: &Profile,
    harness_version: &str,
) -> Result<AgentDescriptor, AdapterError> {
    let separated: Vec<SeparatedEntry> = profile.separated.clone().unwrap_or_else(|| {
        vec![SeparatedEntry {
            executor: "orch-exec".to_owned(),
            executor_version: ExecutorBuild::Any,
            tool_sets: Some(
                ["claude-code", "plain", "custom"]
                    .map(str::to_owned)
                    .to_vec(),
            ),
        }]
    });
    let mut arrangements = Vec::new();
    if profile.separable {
        for entry in &separated {
            arrangements.push(ArrangementEntry {
                kind: ArrangementType::Separated,
                executor: Some(entry.executor.clone()),
                executor_version: Some(entry.executor_version.clone()),
                tool_sets: entry.tool_sets.clone(),
            });
        }
        arrangements.push(plain(ArrangementType::KeySeparated));
    }
    arrangements.push(plain(ArrangementType::CredentialShared));
    arrangements.push(plain(ArrangementType::HandedOver));
    arrangements.push(plain(ArrangementType::Together));

    let credentials = profile
        .credentials
        .clone()
        .unwrap_or_else(|| CredentialSupport {
            issues: credential_forms(),
            accepts: [FORM_TOKEN, FORM_KEY, FORM_LOGIN]
                .map(str::to_owned)
                .to_vec(),
        });
    let mut capabilities = serde_json::to_value(full_capabilities(profile.memory))
        .map_err(|e| AdapterError::Internal(e.to_string()))?;
    if !profile.capabilities.is_null() {
        merge(&mut capabilities, &profile.capabilities);
    }
    let capabilities: Capabilities =
        serde_json::from_value(capabilities).map_err(|error| AdapterError::Other {
            code: "invalid_config".to_owned(),
            message: format!("the capabilities of the fake harness: {error}"),
            retryable: false,
        })?;
    if capabilities.mcp.reload {
        return Err(AdapterError::Other {
            code: "invalid_config".to_owned(),
            message: "The fake adapter refreshes MCP servers by resuming a session; mcp.reload must be false."
                .to_owned(),
            retryable: false,
        });
    }
    let native_provider = NativeProvider {
        harness_kind: program.kind.to_owned(),
        id: None,
        title: "Scripted subscription".to_owned(),
        plans: BTreeMap::from([
            ("free".to_owned(), "Free".to_owned()),
            ("pro".to_owned(), "Pro".to_owned()),
        ]),
        login_methods: login_methods(),
        options_schema: options_schema(),
        arrangements: profile
            .new_provider_allows
            .clone()
            .unwrap_or_else(|| default_new_provider_allows(profile.separable)),
    };
    let mut descriptor = AgentDescriptor {
        kind: program.kind.to_owned(),
        adapter_version: ADAPTER_VERSION,
        harness: HarnessInfo {
            name: program.name.clone(),
            version: harness_version.to_owned(),
            supported_range: SUPPORTED_RANGE.to_owned(),
            status: classify_version(harness_version, SUPPORTED_RANGE, true),
        },
        arrangements,
        native_provider: Some(native_provider),
        native_providers: Vec::new(),
        forwarded_protocols: profile.forwarded_protocols.clone().unwrap_or_else(|| {
            ["openai-responses", "anthropic-messages"]
                .map(str::to_owned)
                .to_vec()
        }),
        direct_protocols: Vec::new(),
        credentials,
        runtime_template: None,
        approval_modes: profile.approval_modes.clone().unwrap_or_else(|| {
            vec![
                ApprovalMode::Ask,
                ApprovalMode::OnRequest,
                ApprovalMode::Bypass,
            ]
        }),
        sandbox_modes: profile.sandbox_modes.clone().unwrap_or_else(|| {
            vec![
                SandboxMode::ReadOnly,
                SandboxMode::Workspace,
                SandboxMode::Open,
            ]
        }),
        // The fake declares what its profile says; it is no evidence of a native boundary.
        side_thread_modes: profile.side_thread_modes.clone().unwrap_or_default(),
        capabilities,
        parameters: parameters(),
        text: program
            .names
            .iter()
            .map(|(language, words)| {
                (
                    language.clone(),
                    BTreeMap::from([("harness.name".to_owned(), words.clone())]),
                )
            })
            .collect(),
        extensions: Extensions::default(),
    };
    // Auto exists only for a program whose manifest has the case that proves it, and the fake's has none.
    if !orch_agent_programs::shipped(KIND).proves("approval_modes.auto") {
        descriptor
            .approval_modes
            .retain(|mode| *mode != ApprovalMode::Auto);
    }
    descriptor.check().map_err(|faults| AdapterError::Other {
        code: "invalid_config".to_owned(),
        message: format!(
            "the profile makes a descriptor that does not hold together: {}",
            faults
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        ),
        retryable: false,
    })?;
    Ok(descriptor)
}

fn plain(kind: ArrangementType) -> ArrangementEntry {
    ArrangementEntry {
        kind,
        executor: None,
        executor_version: None,
        tool_sets: None,
    }
}

/// Merges `patch` into `target` as a JSON merge patch does: `null` removes a member.
fn merge(target: &mut Json, patch: &Json) {
    if let Json::Object(members) = patch {
        if !target.is_object() {
            *target = Json::Object(serde_json::Map::new());
        }
        if let Json::Object(into) = target {
            for (name, value) in members {
                if value.is_null() {
                    into.remove(name);
                } else {
                    merge(into.entry(name.clone()).or_insert(Json::Null), value);
                }
            }
        }
    } else {
        *target = patch.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::descriptor::HarnessSupport;
    use serde_json::json;

    #[test]
    fn the_fake_publishes_one_parameter_of_each_kind_that_reads_a_models_entry() {
        use orch_agent_contract::parameters::{
            Eligibility, ParameterType, ThreadKind, check_parameters, sendable,
        };
        let defs = parameters();
        assert_eq!(check_parameters(&defs), Ok(()));
        let keys: Vec<_> = defs.iter().map(|def| def.key.as_str()).collect();
        assert_eq!(keys, ["service_tier", "access_program", "context"]);
        assert_eq!(defs[0].kind_of_thread(), Some(ThreadKind::ServiceTier));
        assert_eq!(defs[2].kind_of_thread(), Some(ThreadKind::Context));
        assert_eq!(defs[2].kind, ParameterType::Integer);
        assert_eq!(defs[2].range.map(|range| range.min), Some(CONTEXT_LEAST));

        // Each starts with what the model's entry says a thread starts with.
        let model: ModelParameters = serde_json::from_value(json!({
            "service_tiers": [{ "id": "fast", "label": "Fast" }], "default_service_tier": "fast",
            "access_programs": ["early"],
            "context_window": 200_000, "context_max": 800_000
        }))
        .unwrap();
        let context = Eligibility {
            credential_path: orch_agent_contract::credentials::CredentialPath::Native,
            provider_kind: orch_agent_contract::descriptor::ProviderKind::Subscription,
            environment_placement: orch_agent_contract::descriptor::Placement::Managed,
            model,
            harness_version: None,
        };
        let sent = sendable(&defs, &context, &Default::default(), &json!({}));
        assert_eq!(
            Json::Object(sent.values.into_iter().collect()),
            json!({ "service_tier": "fast", "context": 800_000, "context_window": 200_000 }),
            "the speed it started with, all the context that is open, and no program: it names none"
        );
    }

    #[test]
    fn the_large_model_offers_a_speed_a_program_and_a_larger_context_that_nothing_enables() {
        let models = models();
        let large = models
            .iter()
            .find(|model| model.model_name == "fake-large")
            .unwrap();
        let offered = large.parameters.offered.clone().unwrap();
        assert_eq!(offered.service_tiers.map(|tiers| tiers.len()), Some(1));
        assert_eq!(offered.access_programs, Some(vec!["early".to_owned()]));
        assert_eq!(offered.context_max, Some(800_000));
        assert!(
            large.parameters.service_tiers.is_none()
                && large.parameters.access_programs.is_none()
                && large.parameters.context_max.is_none(),
            "being on offer opens nothing"
        );
        let small = models
            .iter()
            .find(|model| model.model_name == "fake-small")
            .unwrap();
        assert_eq!(small.parameters.offered, None);
    }

    #[test]
    fn the_fake_as_it_comes_declares_every_arrangement_and_its_implemented_capabilities() {
        let descriptor = descriptor(&Profile::default(), HARNESS_VERSION).unwrap();
        for kind in ArrangementType::ALL {
            assert!(descriptor.supports(kind), "{kind}");
        }
        let separated: Vec<_> = descriptor.separated().collect();
        assert_eq!(separated.len(), 1);
        assert_eq!(separated[0].executor.as_deref(), Some("orch-exec"));
        assert_eq!(
            separated[0].tool_sets.as_deref().unwrap(),
            ["claude-code", "plain", "custom"]
        );
        let capabilities = &descriptor.capabilities;
        assert!(capabilities.steer && capabilities.interrupt && capabilities.phases);
        assert!(!capabilities.mcp.reload);
        assert_eq!(capabilities.memory, MemoryKind::Directory);
        assert_eq!(descriptor.harness.status, HarnessSupport::Supported);
        assert_eq!(descriptor.credentials.issues.len(), 3);
        assert_eq!(descriptor.check(), Ok(()));
    }

    #[test]
    fn a_profile_that_cannot_be_separated_loses_the_two_arrangements_that_need_it() {
        let profile = Profile::from_config(&json!({ "separable": false })).unwrap();
        let descriptor = descriptor(&profile, HARNESS_VERSION).unwrap();
        assert!(!descriptor.supports(ArrangementType::Separated));
        assert!(!descriptor.supports(ArrangementType::KeySeparated));
        assert!(descriptor.supports(ArrangementType::Together));
    }

    #[test]
    fn what_a_new_provider_starts_with_and_the_kind_of_memory_come_from_the_profile() {
        let profile = Profile::from_config(&json!({
            "new_provider_allows": ["separated", "credential_shared"], "memory": "home"
        }))
        .unwrap();
        let descriptor = descriptor(&profile, HARNESS_VERSION).unwrap();
        assert_eq!(
            descriptor.native_provider.unwrap().arrangements,
            [
                ArrangementType::Separated,
                ArrangementType::CredentialShared
            ]
        );
        assert_eq!(descriptor.capabilities.memory, MemoryKind::Home);
    }

    #[test]
    fn a_capability_is_taken_away_or_changed_by_a_patch() {
        let profile = Profile::from_config(&json!({
            "capabilities": { "steer": false, "effort": "none", "mcp": { "strict": false } }
        }))
        .unwrap();
        let capabilities = descriptor(&profile, HARNESS_VERSION).unwrap().capabilities;
        assert!(!capabilities.steer);
        assert_eq!(capabilities.effort, EffortSupport::None);
        assert!(!capabilities.mcp.strict && capabilities.mcp.stdio);
        assert!(capabilities.interrupt);
    }

    #[test]
    fn a_profile_cannot_advertise_the_unsupported_live_mcp_reload() {
        let profile = Profile::from_config(&json!({
            "capabilities": { "mcp": { "reload": true } }
        }))
        .unwrap();
        let error = descriptor(&profile, HARNESS_VERSION).unwrap_err();
        assert_eq!(error.code(), "invalid_config");
        assert!(error.detail().contains("resuming a session"));
        assert!(error.detail().contains("mcp.reload must be false"));
    }

    #[test]
    fn a_profile_that_asks_for_something_that_does_not_hold_is_refused_with_why() {
        let error = Profile::from_config(&json!({ "seperable": false })).unwrap_err();
        assert_eq!(error.code(), "invalid_config");
        let profile = Profile::from_config(&json!({
            "credentials": { "issues": [], "accepts": [] }
        }))
        .unwrap();
        let error = descriptor(&profile, HARNESS_VERSION).unwrap_err();
        assert!(error.detail().contains("credential form"), "{error}");
    }

    #[test]
    fn a_harness_older_than_the_range_is_unsupported_and_a_newer_one_untested() {
        let old = descriptor(&Profile::default(), "0.9.0").unwrap();
        assert_eq!(old.harness.status, HarnessSupport::Unsupported);
        let new = descriptor(&Profile::default(), "2.1.0").unwrap();
        assert_eq!(new.harness.status, HarnessSupport::Untested);
    }

    #[test]
    fn there_is_one_sign_in_method_of_each_kind_and_the_import_has_every_type_of_field() {
        let methods = login_methods();
        let kinds: Vec<_> = methods.iter().map(|m| (m.id.as_str(), m.kind)).collect();
        assert_eq!(
            kinds,
            [
                (METHOD_CODE, LoginKind::Interactive),
                (METHOD_ASK, LoginKind::Interactive),
                (METHOD_IMPORT, LoginKind::Import)
            ]
        );
        let import = &methods[2];
        assert_eq!(import.lifetime_secs, Some(600));
        for kind in [
            FieldType::Text,
            FieldType::Secret,
            FieldType::SecretText,
            FieldType::Choice,
            FieldType::Boolean,
            FieldType::Date,
            FieldType::Map,
            FieldType::SecretMap,
        ] {
            assert!(
                import.fields.iter().any(|field| field.kind == kind),
                "{kind:?}"
            );
        }
        assert!(
            orch_agent_contract::login::check_methods(&methods).is_ok(),
            "the declaration holds together"
        );
    }
}
