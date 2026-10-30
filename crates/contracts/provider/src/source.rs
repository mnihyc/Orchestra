//! The interfaces a provider's facts, models and key presentation are read through, what a source declares, what it is given and what it answers.
//!
//! One implementation may implement any of the three interfaces, and says which in its [`SourceDescriptor`]:
//! `facts` (with `spends_resets` for the resets its facts list), `models`, `presents`. Every call is given a
//! [`SourceAsk`]: the provider, its endpoints, the configuration its managers gave the source, the values of
//! the source's secret fields, and the provider's key only where the source declared that it wants it.
//!
//! A facts report is a JSON object ([`FactsReport`]): a member present in it, even `null` or `[]`, is told by
//! this read; a member absent is not told and keeps what was told before. The machine that keeps the
//! credential reads it member by member and stamps each told member, and each told list item, with the time
//! of the read. A source's failure ([`SourceError`]) never changes the provider's status: what was told before
//! stays, and the error is kept beside the choice of the source.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use async_trait::async_trait;
use orch_plugin_contract::{Extensions, Secret};
use orch_types::{Field, Id, Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::facts::{
    AvailableResets, Balance, CredentialFacts, ExtraUsage, LimitReached, Spend, UsageWindow,
};
use crate::limits;
use crate::members::FactMember;
use crate::models::ModelSuggestion;
use crate::wire;

pub use crate::facts::ResetRequest;

// ---------------------------------------------------------------- the interfaces

/// Reads what a provider is now, and spends the resets its facts list (`facts.read`, `resets.spend`).
#[async_trait]
pub trait FactsSource: Send + Sync {
    /// What the source is and what it tells.
    fn describe(&self) -> SourceDescriptor;

    /// Reads the provider's facts now.
    async fn read_facts(&self, ask: SourceAsk) -> Result<FactsReport, SourceError>;

    /// Spends one of the resets this source listed in `available_resets`, by its own id. The same idempotency
    /// key given twice spends once. A source that spends no resets answers `unsupported`.
    async fn spend_reset(
        &self,
        _ask: SourceAsk,
        _reset: ResetRequest,
    ) -> Result<ResetReport, SourceError> {
        Err(SourceError::unsupported(wire::RESETS_SPEND))
    }
}

/// Lists the models a provider offers (`models.list`).
#[async_trait]
pub trait ModelsSource: Send + Sync {
    /// What the source is and what it tells.
    fn describe(&self) -> SourceDescriptor;

    /// Lists the provider's models now.
    async fn list_models(&self, ask: SourceAsk) -> Result<ModelsReport, SourceError>;
}

/// Says what to present for a provider's key on a model request (`credential.present`): keys only.
#[async_trait]
pub trait CredentialPresenter: Send + Sync {
    /// What the source is and what it tells.
    fn describe(&self) -> SourceDescriptor;

    /// The headers to present for the key now, and until when they are good.
    async fn present(&self, ask: PresentAsk) -> Result<Presented, SourceError>;
}

// ---------------------------------------------------------------- what a source declares

/// Whether a source wants the provider's key to read its facts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceCredential {
    /// It reads without the key.
    #[default]
    None,
    /// It wants the provider's key; it is given only where the provider's credential is a key (and, for an
    /// address, only where the provider's managers turned that on for it).
    Key,
}

/// What a source tells of a provider's facts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceDescriptorFacts {
    /// The members it can tell. A member it does not list is unsupported until it tells it; a name this
    /// contract does not know is left out.
    #[serde(default, deserialize_with = "known_members")]
    pub reports: Vec<FactMember>,
    /// Whether it spends the resets it lists in `available_resets` (`resets.spend`).
    #[serde(default)]
    pub spends_resets: bool,
    /// Whether it wants the provider's key to read.
    #[serde(default)]
    pub credential: SourceCredential,
    /// The least time between two of its reads, in seconds, at least 60; `null` leaves it to Orch.
    #[serde(default)]
    pub min_interval_secs: Option<u32>,
    /// The plans it may name in `plan`, by id, with the name to show for each.
    #[serde(default)]
    pub plans: BTreeMap<String, String>,
}

/// What a source declares of the models it lists. It has no members yet: that it is there says that the
/// source implements `models.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceDescriptorModels {}

/// What a source declares of the key presentation it makes. It has no members yet: that it is there says
/// that the source implements `credential.present`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceDescriptorPresents {}

/// What a source declares: what it is, which of the interfaces it implements, and what it asks of a
/// provider's managers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceDescriptor {
    /// Its name: one of a kind among the programs of a machine, and among the built-in sources.
    pub name: String,
    /// What it is called.
    pub title: String,
    /// What it is, in one sentence.
    pub summary: String,
    /// The version of this contract it speaks.
    pub contract: u32,
    /// What it tells of the facts; `null` when it implements no facts.
    #[serde(default)]
    pub facts: Option<SourceDescriptorFacts>,
    /// `null` when it lists no models.
    #[serde(default)]
    pub models: Option<SourceDescriptorModels>,
    /// `null` when it presents no credential.
    #[serde(default)]
    pub presents: Option<SourceDescriptorPresents>,
    /// What it asks of a provider's managers. A secret field is kept as the provider secret
    /// `source:<element>:field:<name>`, written once and never shown again.
    #[serde(default)]
    pub fields: Vec<Field>,
    /// Where its own documentation is.
    #[serde(default)]
    pub docs_url: Option<String>,
    /// Members of its own (`x_…`), kept and never interpreted.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// Reads the members a source lists, leaving out the names this contract does not know: a source written
/// for a later contract still describes itself.
fn known_members<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<FactMember>, D::Error> {
    let names = Option::<Vec<Json>>::deserialize(deserializer)?.unwrap_or_default();
    Ok(names
        .into_iter()
        .filter_map(|name| serde_json::from_value(name).ok())
        .collect())
}

/// What is wrong with a source's declaration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DescriptorFault {
    /// It has no name, or one longer than a name may be.
    #[error("a source has a name of 1 to {} characters", limits::TEXT_MAX_CHARS)]
    Name,
    /// It has no title.
    #[error("a source has a title")]
    NoTitle,
    /// It speaks no version of the contract.
    #[error("a source speaks a version of the contract, 1 or later")]
    Contract,
    /// It implements none of facts, models and presentation.
    #[error("a source implements facts, models or the presentation of a key")]
    NothingImplemented,
    /// It would be read more often than once a minute.
    #[error("min_interval_secs is at least {}", limits::MIN_INTERVAL_LEAST_SECS)]
    MinInterval,
    /// It lists a member twice.
    #[error("the member {0} is listed twice")]
    ReportedTwice(FactMember),
    /// It spends resets and does not tell them.
    #[error("a source that spends resets tells available_resets")]
    ResetsUntold,
    /// A field has no name, or a name another field has.
    #[error("the field {0:?} has no name or is named twice")]
    Field(String),
    /// Its documentation is not at an address of the web.
    #[error("docs_url is an http or https address")]
    DocsUrl,
}

impl SourceDescriptor {
    /// A descriptor that speaks this contract and implements nothing yet.
    #[must_use]
    pub fn new(name: &str, title: &str, summary: &str) -> Self {
        Self {
            name: name.to_owned(),
            title: title.to_owned(),
            summary: summary.to_owned(),
            contract: wire::CONTRACT,
            facts: None,
            models: None,
            presents: None,
            fields: Vec::new(),
            docs_url: None,
            extensions: Extensions::new(),
        }
    }

    /// The members it can tell; none where it implements no facts.
    #[must_use]
    pub fn reports(&self) -> &[FactMember] {
        self.facts
            .as_ref()
            .map_or(&[], |facts| facts.reports.as_slice())
    }

    /// Whether it spends the resets it lists.
    #[must_use]
    pub fn spends_resets(&self) -> bool {
        self.facts.as_ref().is_some_and(|facts| facts.spends_resets)
    }

    /// Whether it lists models.
    #[must_use]
    pub fn lists_models(&self) -> bool {
        self.models.is_some()
    }

    /// Whether it presents a key.
    #[must_use]
    pub fn presents_credential(&self) -> bool {
        self.presents.is_some()
    }

    /// Whether it wants the provider's key: to read facts with it, or to present it.
    #[must_use]
    pub fn wants_key(&self) -> bool {
        self.presents.is_some()
            || self
                .facts
                .as_ref()
                .is_some_and(|facts| facts.credential == SourceCredential::Key)
    }

    /// Whether it implements `method` ([`wire`]): `describe` always, the others as it declares.
    #[must_use]
    pub fn implements(&self, method: &str) -> bool {
        match method {
            wire::DESCRIBE => true,
            wire::FACTS_READ => self.facts.is_some(),
            wire::RESETS_SPEND => self.spends_resets(),
            wire::MODELS_LIST => self.lists_models(),
            wire::CREDENTIAL_PRESENT => self.presents_credential(),
            _ => false,
        }
    }

    /// The fields whose values are kept as provider secrets.
    pub fn secret_fields(&self) -> impl Iterator<Item = &Field> {
        self.fields.iter().filter(|field| field.kind.is_secret())
    }

    /// Checks the declaration for consistency with itself: a name and a title, a version of the contract,
    /// at least one interface, no member listed twice, resets spent only where they are told, reads no more
    /// often than once a minute, fields named once each, documentation on the web.
    pub fn check(&self) -> Result<(), DescriptorFault> {
        if self.name.trim().is_empty() || self.name.chars().count() > limits::TEXT_MAX_CHARS {
            return Err(DescriptorFault::Name);
        }
        if self.title.trim().is_empty() {
            return Err(DescriptorFault::NoTitle);
        }
        if self.contract < 1 {
            return Err(DescriptorFault::Contract);
        }
        if self.facts.is_none() && self.models.is_none() && self.presents.is_none() {
            return Err(DescriptorFault::NothingImplemented);
        }
        if let Some(facts) = &self.facts {
            if facts
                .min_interval_secs
                .is_some_and(|secs| secs < limits::MIN_INTERVAL_LEAST_SECS)
            {
                return Err(DescriptorFault::MinInterval);
            }
            for (index, member) in facts.reports.iter().enumerate() {
                if facts.reports[..index].contains(member) {
                    return Err(DescriptorFault::ReportedTwice(*member));
                }
            }
            if facts.spends_resets && !facts.reports.contains(&FactMember::AvailableResets) {
                return Err(DescriptorFault::ResetsUntold);
            }
        }
        for (index, field) in self.fields.iter().enumerate() {
            if field.name.is_empty()
                || self.fields[..index]
                    .iter()
                    .any(|other| other.name == field.name)
            {
                return Err(DescriptorFault::Field(field.name.clone()));
            }
        }
        if let Some(docs) = &self.docs_url
            && !(docs.starts_with("https://") || docs.starts_with("http://"))
        {
            return Err(DescriptorFault::DocsUrl);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- what a source is given

/// The provider a call is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceAskProvider {
    /// Its identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
}

/// One endpoint of the provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceAskEndpoint {
    /// The protocol it speaks, such as `openai-responses` or `anthropic-messages`.
    pub protocol: String,
    /// Its address.
    pub base_url: String,
}

/// The provider's credential, as a source that wants it is given it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceAskCredential {
    /// The provider's key.
    #[schemars(with = "String")]
    pub key: Secret,
}

/// What every call to a source is given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceAsk {
    /// The provider.
    pub provider: SourceAskProvider,
    /// Its endpoints; none for a provider signed in through a harness.
    #[serde(default)]
    pub endpoints: Vec<SourceAskEndpoint>,
    /// What the provider's managers gave for the source's fields that are no secrets, by field.
    #[serde(default)]
    pub config: BTreeMap<String, Json>,
    /// The values of the source's secret fields, by field: those it declared, and no other.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, String>")]
    pub secrets: BTreeMap<String, Secret>,
    /// The provider's key: only where the source wants it, the credential is a key, and, for an address,
    /// the provider's managers turned on sending it there. Never a sign-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<SourceAskCredential>,
}

/// What `credential.present` is given: the key to present, the endpoint it is presented to, and what every
/// call is given of the provider and the source's fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresentAsk {
    /// The provider.
    pub provider: SourceAskProvider,
    /// The endpoint the model request goes to.
    pub endpoint: SourceAskEndpoint,
    /// What the provider's managers gave for the source's fields that are no secrets.
    #[serde(default)]
    pub config: BTreeMap<String, Json>,
    /// The values of the source's secret fields.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, String>")]
    pub secrets: BTreeMap<String, Secret>,
    /// The key to present.
    pub credential: SourceAskCredential,
}

// ---------------------------------------------------------------- what a source answers

/// What a source answers to `facts.read`: a JSON object whose members are the members of a provider's facts
/// ([`FactMember`]) that this read tells.
///
/// A member present, even `null` or `[]`, is told by this read: `null` and `[]` say that there is none. A
/// member absent is not told and keeps what was told before. Each list item is read on its own: one without
/// its required members, or past its list's cap ([`limits`]), is left out and named in the read's note, and
/// the others are kept. Amounts are decimal text (at most 40 characters), times RFC 3339 in UTC, other text at
/// most 200 characters. Members this contract does not know are ignored; members of an item whose names start
/// with `x_` are kept on it.
///
/// It is kept as the source wrote it, so that a member told as `null` stays apart from one not told, until the
/// machine that keeps the credential reads it member by member. Its JSON Schema lists every member with its
/// shape ([`FactsReport::member_schema`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FactsReport(pub Json);

impl JsonSchema for FactsReport {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "FactsReport".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let mut properties = serde_json::Map::new();
        for member in FactMember::ALL {
            let mut schema = Self::member_schema(member, generator);
            schema.insert(
                "description".to_owned(),
                Json::String(Self::member_meaning(member).to_owned()),
            );
            properties.insert(member.as_str().to_owned(), schema.to_value());
        }
        schemars::json_schema!({
            "type": "object",
            "description": "The members of a provider's facts this read tells. A member present, even null or [], is told by this read; a member absent is not told and keeps what was told before. Members of a later contract are ignored.",
            "properties": properties,
            "additionalProperties": true
        })
    }
}

impl FactsReport {
    /// The shape of `member` in a report: its type, or `null` for none.
    pub fn member_schema(
        member: FactMember,
        generator: &mut schemars::SchemaGenerator,
    ) -> schemars::Schema {
        match member {
            FactMember::Plan | FactMember::Identity => generator.subschema_for::<Option<String>>(),
            FactMember::Windows => generator.subschema_for::<Option<Vec<UsageWindow>>>(),
            FactMember::LimitReached => generator.subschema_for::<Option<LimitReached>>(),
            FactMember::OrdinaryUsageAllowed | FactMember::CredentialVerified => {
                generator.subschema_for::<Option<bool>>()
            }
            FactMember::Balances => generator.subschema_for::<Option<Vec<Balance>>>(),
            FactMember::Spend => generator.subschema_for::<Option<Vec<Spend>>>(),
            FactMember::ExtraUsage => generator.subschema_for::<Option<ExtraUsage>>(),
            FactMember::AvailableResets => generator.subschema_for::<Option<AvailableResets>>(),
            FactMember::Credential => generator.subschema_for::<Option<CredentialFacts>>(),
        }
    }

    /// What `member` says, as the help pages describe it.
    #[must_use]
    pub const fn member_meaning(member: FactMember) -> &'static str {
        match member {
            FactMember::Plan => {
                "The plan's id, at most 200 characters; the name shown for it is the one the source's `plans` give."
            }
            FactMember::Windows => {
                "The usage windows of the plan, at most 64, each by its `id`: how much is used in percent, and when it starts over."
            }
            FactMember::LimitReached => {
                "A limit of the plan that has been reached, with when it lifts: new turns wait until then, or until a read admits ordinary use."
            }
            FactMember::OrdinaryUsageAllowed => {
                "Whether the vendor admits ordinary use now: false holds new turns, true lifts that."
            }
            FactMember::Balances => {
                "What is left to be spent, at most 32, each by its `id` and in its own unit; amounts are decimal text."
            }
            FactMember::Spend => {
                "What has been spent over periods, at most 64, each by its `id`, with the period's limit where it has one; amounts are decimal text."
            }
            FactMember::ExtraUsage => "Spending beyond the plan, in the vendor's unit.",
            FactMember::AvailableResets => {
                "The resets that can be spent, with their credits, at most 64; a source that spends them declares `spends_resets`."
            }
            FactMember::Identity => "Who the credential belongs to, at most 200 characters.",
            FactMember::Credential => {
                "How the credential was given, what it is, and when it lapses."
            }
            FactMember::CredentialVerified => {
                "True only where this read authenticated with the provider's credential; it is kept only from a read that was given the key."
            }
        }
    }

    /// The report's members, where it is an object at all.
    #[must_use]
    pub fn members(&self) -> Option<&serde_json::Map<String, Json>> {
        self.0.as_object()
    }

    /// Whether `member` is told by this report: present, even as `null` or `[]`.
    #[must_use]
    pub fn tells(&self, member: FactMember) -> bool {
        self.members()
            .is_some_and(|members| members.contains_key(member.as_str()))
    }
}

/// What a source answers to `models.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelsReport {
    /// The models it lists (at most [`limits::MODELS_MAX`]): `null` when it does not list models, `[]` when it
    /// lists none. Each needs `model_name`; `display_name`, `parameters` and `listed` may be left out. Each is
    /// read on its own, so that one in another shape is left out and the others kept.
    #[serde(default)]
    #[schemars(with = "Option<Vec<ModelSuggestion>>")]
    pub models: Option<Vec<Json>>,
}

impl ModelsReport {
    /// The report of a source that lists `models`.
    #[must_use]
    pub fn listed(models: &[ModelSuggestion]) -> Self {
        Self {
            models: Some(
                models
                    .iter()
                    .filter_map(|model| serde_json::to_value(model).ok())
                    .collect(),
            ),
        }
    }

    /// The report of a source that does not list models.
    #[must_use]
    pub fn unlisted() -> Self {
        Self { models: None }
    }
}

/// What a source answers to `resets.spend`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResetReport {
    /// The outcome, in the source's own words.
    pub outcome: String,
    /// The facts it read after spending, where it read them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<FactsReport>,
}

/// What a source answers to `credential.present`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Presented {
    /// The headers a model request carries in place of the key's own presentation, by name.
    #[schemars(with = "BTreeMap<String, String>")]
    pub headers: BTreeMap<String, Secret>,
    /// When they stop being good; `null` where the source does not say.
    #[serde(default)]
    pub expires_at: Option<Time>,
}

impl Presented {
    /// Until when headers presented at `now` are used before the source is asked again: a minute before they
    /// lapse, or five minutes where the source does not say when.
    #[must_use]
    pub fn reuse_until(&self, now: Time) -> Time {
        let span = |duration: std::time::Duration| {
            chrono::TimeDelta::from_std(duration).unwrap_or_default()
        };
        let at = match self.expires_at {
            Some(expires) => expires
                .get()
                .checked_sub_signed(span(limits::PRESENTED_RENEW_BEFORE)),
            None => now
                .get()
                .checked_add_signed(span(limits::PRESENTED_REUSE_WITHOUT_EXPIRY)),
        };
        at.map_or(now, Time::new).max(now)
    }
}

// ---------------------------------------------------------------- errors

/// Why a source did not answer as asked. The codes are the same for every binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum SourceErrorCode {
    /// The method is not implemented.
    #[serde(rename = "unsupported")]
    Unsupported,
    /// The source could not tell now: its vendor was silent.
    #[serde(rename = "not_told")]
    NotTold,
    /// The vendor refused this read.
    #[serde(rename = "refused")]
    Refused,
    /// No answer within the method's deadline.
    #[serde(rename = "timeout")]
    Timeout,
    /// The program is down or restarting, the address cannot be reached, or it answered HTTP 5xx or 429.
    #[serde(rename = "unavailable")]
    Unavailable,
    /// The answer is not in its shape: the message names the first wrong member.
    #[serde(rename = "malformed")]
    Malformed,
    /// The answer is over its size, or a list over its cap.
    #[serde(rename = "too_large")]
    TooLarge,
    /// An address answered with a redirection, which is never followed.
    #[serde(rename = "redirected")]
    Redirected,
    /// An address refused the authorization Orch holds for it (HTTP 401 or 403).
    #[serde(rename = "unauthorized")]
    Unauthorized,
    /// The address is not among the upstreams a person's machine allows.
    #[serde(rename = "policy.upstream_not_allowed")]
    UpstreamNotAllowed,
    /// A key could not be presented for a model request.
    #[serde(rename = "credential_unavailable")]
    CredentialUnavailable,
}

impl SourceErrorCode {
    /// Every code.
    pub const ALL: [Self; 11] = [
        Self::Unsupported,
        Self::NotTold,
        Self::Refused,
        Self::Timeout,
        Self::Unavailable,
        Self::Malformed,
        Self::TooLarge,
        Self::Redirected,
        Self::Unauthorized,
        Self::UpstreamNotAllowed,
        Self::CredentialUnavailable,
    ];

    /// The code as it is written.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::NotTold => "not_told",
            Self::Refused => "refused",
            Self::Timeout => "timeout",
            Self::Unavailable => "unavailable",
            Self::Malformed => "malformed",
            Self::TooLarge => "too_large",
            Self::Redirected => "redirected",
            Self::Unauthorized => "unauthorized",
            Self::UpstreamNotAllowed => "policy.upstream_not_allowed",
            Self::CredentialUnavailable => "credential_unavailable",
        }
    }

    /// Whether asking again may succeed: after a timeout, an unavailable source, or a key that could not be
    /// presented.
    #[must_use]
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::Unavailable | Self::CredentialUnavailable
        )
    }
}

impl fmt::Display for SourceErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A code that is none of [`SourceErrorCode`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is no code of a source's error")]
pub struct UnknownCode(pub String);

impl FromStr for SourceErrorCode {
    type Err = UnknownCode;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|code| code.as_str() == text)
            .ok_or_else(|| UnknownCode(text.to_owned()))
    }
}

/// A source's failure, as a program or an address writes it (`{"code", "message", "retryable"}`) and as the
/// machine that keeps the credential reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[error("{code}: {message}")]
pub struct SourceError {
    /// What went wrong.
    pub code: SourceErrorCode,
    /// A sentence on it, in the source's words; every secret replaced by `[secret]` before it is kept.
    pub message: String,
    /// Whether asking again may succeed.
    #[serde(default)]
    pub retryable: bool,
}

impl SourceError {
    /// An error of `code`, retryable where the code is.
    #[must_use]
    pub fn new(code: SourceErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: code.retryable(),
        }
    }

    /// The answer of a source that does not implement `method`.
    #[must_use]
    pub fn unsupported(method: &str) -> Self {
        Self::new(
            SourceErrorCode::Unsupported,
            format!("the source does not implement {method}"),
        )
    }

    /// The same error with every one of `secrets` in its message replaced by `[secret]`, and the message cut
    /// to what is kept ([`limits::ERROR_MESSAGE_MAX_CHARS`]).
    #[must_use]
    pub fn scrubbed(mut self, secrets: &[&str]) -> Self {
        self.message = limits::scrub(&self.message, secrets);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan_descriptor() -> Json {
        json!({
            "name": "acme-plan", "title": "Acme plan", "summary": "Reads the Acme plan's windows and balance.",
            "contract": 1,
            "facts": { "reports": ["plan", "windows", "limit_reached", "balances", "spend", "available_resets",
                                   "x_later_member"],
                       "spends_resets": true, "credential": "key", "min_interval_secs": 300,
                       "plans": { "acme-pro": "Acme Pro" } },
            "models": {},
            "presents": null,
            "fields": [ { "name": "account", "label": "Account", "type": "text", "required": true,
                          "help": null, "choices": null, "default": null, "max_bytes": null },
                        { "name": "token", "label": "Token", "type": "secret", "required": true,
                          "help": null, "choices": null, "default": null, "max_bytes": null } ],
            "docs_url": "https://acme.example/orch",
            "x_build": "2026.10"
        })
    }

    #[test]
    fn a_descriptor_reads_as_a_program_writes_it_and_keeps_its_own_members() {
        let descriptor: SourceDescriptor = serde_json::from_value(plan_descriptor()).unwrap();
        assert_eq!(
            descriptor.reports(),
            [
                FactMember::Plan,
                FactMember::Windows,
                FactMember::LimitReached,
                FactMember::Balances,
                FactMember::Spend,
                FactMember::AvailableResets
            ],
            "a member of a later contract is left out"
        );
        assert!(descriptor.spends_resets() && descriptor.lists_models());
        assert!(descriptor.wants_key() && !descriptor.presents_credential());
        assert_eq!(
            descriptor.extensions.get("x_build"),
            Some(&json!("2026.10"))
        );
        assert_eq!(
            descriptor
                .secret_fields()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            ["token"]
        );
        assert_eq!(descriptor.check(), Ok(()));
        assert!(descriptor.implements(wire::RESETS_SPEND));
        assert!(!descriptor.implements(wire::CREDENTIAL_PRESENT));
        assert!(descriptor.implements(wire::DESCRIBE));
        let written = serde_json::to_value(&descriptor).unwrap();
        assert_eq!(written["x_build"], "2026.10");
        assert_eq!(
            serde_json::from_value::<SourceDescriptor>(written).unwrap(),
            descriptor
        );
    }

    #[test]
    fn a_descriptor_that_contradicts_itself_is_refused() {
        let read = |change: &dyn Fn(&mut Json)| {
            let mut value = plan_descriptor();
            change(&mut value);
            serde_json::from_value::<SourceDescriptor>(value)
                .unwrap()
                .check()
        };
        assert_eq!(read(&|d| d["name"] = json!("")), Err(DescriptorFault::Name));
        assert_eq!(
            read(&|d| d["title"] = json!(" ")),
            Err(DescriptorFault::NoTitle)
        );
        assert_eq!(
            read(&|d| d["contract"] = json!(0)),
            Err(DescriptorFault::Contract)
        );
        assert_eq!(
            read(&|d| {
                d["facts"] = Json::Null;
                d["models"] = Json::Null;
            }),
            Err(DescriptorFault::NothingImplemented)
        );
        assert_eq!(
            read(&|d| d["facts"]["min_interval_secs"] = json!(59)),
            Err(DescriptorFault::MinInterval)
        );
        assert_eq!(
            read(&|d| d["facts"]["reports"] = json!(["plan", "plan"])),
            Err(DescriptorFault::ReportedTwice(FactMember::Plan))
        );
        assert_eq!(
            read(&|d| d["facts"]["reports"] = json!(["plan"])),
            Err(DescriptorFault::ResetsUntold)
        );
        assert_eq!(
            read(&|d| d["fields"][1]["name"] = json!("account")),
            Err(DescriptorFault::Field("account".into()))
        );
        assert_eq!(
            read(&|d| d["docs_url"] = json!("file:///etc/passwd")),
            Err(DescriptorFault::DocsUrl)
        );
        let built_in = SourceDescriptor::new("none", "Nothing", "Tells nothing of the plan.");
        assert_eq!(built_in.contract, wire::CONTRACT);
        assert_eq!(built_in.check(), Err(DescriptorFault::NothingImplemented));
    }

    #[test]
    fn a_source_is_given_the_key_only_as_a_member_of_its_own_and_never_prints_it() {
        let ask: SourceAsk = serde_json::from_value(json!({
            "provider": { "id": "0199c1a0-0000-7000-8000-00000000000a", "name": "Acme" },
            "endpoints": [ { "protocol": "openai-responses", "base_url": "https://api.acme.example/v1" } ],
            "config": { "account": "team-4" },
            "secrets": { "token": "s3cr3t" },
            "credential": { "key": "sk-acme-123" }
        }))
        .unwrap();
        let shown = format!("{ask:?}");
        assert!(
            !shown.contains("s3cr3t") && !shown.contains("sk-acme-123"),
            "{shown}"
        );
        let written = serde_json::to_value(&ask).unwrap();
        assert_eq!(written["credential"]["key"], "sk-acme-123");
        assert_eq!(written["secrets"]["token"], "s3cr3t");
        let mut without = ask;
        without.credential = None;
        assert!(
            serde_json::to_value(&without)
                .unwrap()
                .get("credential")
                .is_none()
        );
    }

    #[test]
    fn a_report_keeps_a_member_told_empty_apart_from_one_not_told() {
        let report: FactsReport = serde_json::from_value(
            json!({ "windows": [], "plan": null, "balances": [ { "id": "USD" } ] }),
        )
        .unwrap();
        assert!(report.tells(FactMember::Windows) && report.tells(FactMember::Plan));
        assert!(!report.tells(FactMember::Spend));
        let reset: ResetReport =
            serde_json::from_value(json!({ "outcome": "reset", "facts": { "windows": [] } }))
                .unwrap();
        assert!(reset.facts.unwrap().tells(FactMember::Windows));
        let unlisted: ModelsReport = serde_json::from_value(json!({ "models": null })).unwrap();
        assert_eq!(unlisted, ModelsReport::unlisted());
        let none: ModelsReport = serde_json::from_value(json!({ "models": [] })).unwrap();
        assert_eq!(none.models, Some(Vec::new()));
        let listed =
            ModelsReport::listed(&[ModelSuggestion::named("acme-large", Some("Acme Large"))]);
        assert_eq!(listed.models.unwrap()[0]["model_name"], "acme-large");
    }

    #[test]
    fn the_schema_of_a_report_lists_every_member_a_source_may_tell_each_of_which_may_be_null() {
        let schema = schemars::SchemaGenerator::default()
            .into_root_schema_for::<FactsReport>()
            .to_value();
        let properties = schema["properties"].as_object().unwrap();
        assert_eq!(properties.len(), FactMember::ALL.len());
        for member in FactMember::ALL {
            let property = &properties[member.as_str()];
            assert!(
                property["description"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "{member}"
            );
            assert!(
                property.to_string().contains("null"),
                "{member} may be told as null"
            );
        }
        assert_eq!(
            schema["additionalProperties"], true,
            "members of its own are allowed"
        );
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn errors_are_written_with_the_codes_every_binding_shares() {
        let error: SourceError = serde_json::from_value(
            json!({ "code": "policy.upstream_not_allowed", "message": "not in provider.upstreams" }),
        )
        .unwrap();
        assert_eq!(error.code, SourceErrorCode::UpstreamNotAllowed);
        assert!(!error.retryable);
        for code in SourceErrorCode::ALL {
            assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
            assert_eq!(code.as_str().parse::<SourceErrorCode>(), Ok(code));
        }
        assert!(SourceError::new(SourceErrorCode::Timeout, "slow").retryable);
        assert!(!SourceError::new(SourceErrorCode::Malformed, "windows").retryable);
        assert_eq!(
            SourceError::unsupported(wire::MODELS_LIST).code,
            SourceErrorCode::Unsupported
        );
        assert!("teapot".parse::<SourceErrorCode>().is_err());
        let scrubbed =
            SourceError::new(SourceErrorCode::Refused, "the key sk-acme-123 was refused")
                .scrubbed(&["sk-acme-123"]);
        assert_eq!(scrubbed.message, "the key [secret] was refused");
    }

    #[test]
    fn presented_headers_are_used_until_a_minute_before_they_lapse_or_five_minutes() {
        let now: Time = "2026-10-15T10:00:00.000Z".parse().unwrap();
        let lapsing = Presented {
            headers: BTreeMap::from([("Authorization".to_owned(), Secret::new("Bearer t"))]),
            expires_at: Some("2026-10-15T11:00:00.000Z".parse().unwrap()),
        };
        assert_eq!(
            lapsing.reuse_until(now),
            "2026-10-15T10:59:00.000Z".parse::<Time>().unwrap()
        );
        let silent = Presented {
            headers: BTreeMap::new(),
            expires_at: None,
        };
        assert_eq!(
            silent.reuse_until(now),
            "2026-10-15T10:05:00.000Z".parse::<Time>().unwrap()
        );
        let lapsed = Presented {
            headers: BTreeMap::new(),
            expires_at: Some("2026-10-15T10:00:30.000Z".parse().unwrap()),
        };
        assert_eq!(lapsed.reuse_until(now), now, "never earlier than now");
        assert!(!format!("{lapsing:?}").contains("Bearer t"));
    }
}
