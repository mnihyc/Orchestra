//! What a provider's managers chose for each of its elements, as it is kept and as the server tells the machine that keeps the credential for one call.
//!
//! Each element of a provider that can come from outside (its facts with their resets, its models, the
//! presentation of its key) is implemented by one source: the default that follows from the provider's
//! credential and endpoints, a built-in one by name, a program registered beside the credential, an address,
//! or nothing ([`SourceBy`]). The choice is kept in `providers.sources` as a [`SourceChoice`] per element
//! ([`ProviderSources`]; `{}` is the default for every element, which is how every provider reads as it did),
//! with references to secrets instead of values, the descriptor learned from the source, and its last error.
//! For one call the server tells the keeping machine a [`SourceSpec`]: the same choice, with the source's
//! secret values and the address's authorization sealed to that machine. An absent spec is the default.

use std::collections::BTreeMap;

use orch_types::{Id, Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::address::{AddressAuth, SourceAddress};
use crate::source::{SourceDescriptor, SourceErrorCode};

/// The names of the built-in sources.
pub mod builtin {
    /// The harness that keeps a sign-in, through its adapter: facts and models.
    pub const ADAPTER: &str = "adapter";
    /// OpenRouter's key and account-credit reads: facts.
    pub const OPENROUTER: &str = "openrouter";
    /// DeepSeek's balance read: facts.
    pub const DEEPSEEK: &str = "deepseek";
    /// Each endpoint's own list of models: models.
    pub const ENDPOINTS: &str = "endpoints";
    /// Every built-in name.
    pub const ALL: [&str; 4] = [ADAPTER, OPENROUTER, DEEPSEEK, ENDPOINTS];
    /// The built-ins that read a key's facts from its vendor, which a program registers with their readers
    /// ([`crate::builtins`]).
    pub const KEY_FACTS: [&str; 2] = [OPENROUTER, DEEPSEEK];
}

/// What keeps a provider's credential, as far as the default of its sources goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keeper {
    /// A harness keeps a sign-in.
    SignIn,
    /// A key is presented to its endpoints.
    Key,
    /// None of its endpoints takes a key.
    NoKey,
}

/// The source that implements `element` of a provider whose managers left it at its default, by name; none for
/// nothing. A sign-in's facts and models come from the harness that keeps it. A key's facts come from the first
/// built-in of `table` that reads one of its `endpoints`, and nothing else; every built-in reads with the key, so
/// a provider whose endpoints take none has no default facts. A key's models, or a provider's that takes none,
/// are each endpoint's own list. Nothing presents a key by default. The server names this source on the
/// provider's page and the machine that keeps the credential reads it, so both ask this.
#[must_use]
pub fn default_source(
    element: SourceElement,
    keeper: Keeper,
    endpoints: &[url::Url],
    table: &crate::builtins::BuiltinSources,
) -> Option<&'static str> {
    match (element, keeper) {
        (SourceElement::Presenter, _) | (SourceElement::Facts, Keeper::NoKey) => None,
        (SourceElement::Facts | SourceElement::Models, Keeper::SignIn) => Some(builtin::ADAPTER),
        (SourceElement::Models, Keeper::Key | Keeper::NoKey) => Some(builtin::ENDPOINTS),
        (SourceElement::Facts, Keeper::Key) => table.reading(endpoints).map(|entry| entry.name),
    }
}

/// An element of a provider whose source its managers choose.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SourceElement {
    /// What the provider is now, with the resets its facts list.
    Facts,
    /// Which models it offers.
    Models,
    /// What is presented for its key on a model request.
    Presenter,
}

impl SourceElement {
    /// Every element.
    pub const ALL: [Self; 3] = [Self::Facts, Self::Models, Self::Presenter];

    /// The element as it is written.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Facts => "facts",
            Self::Models => "models",
            Self::Presenter => "presenter",
        }
    }

    /// The name of the provider secret that holds the value of the source's secret field `field`.
    #[must_use]
    pub fn field_secret(self, field: &str) -> String {
        format!("source:{}:field:{field}", self.as_str())
    }

    /// The name of the provider secret that holds the authorization of the source's address.
    #[must_use]
    pub fn authorization_secret(self) -> String {
        format!("source:{}:authorization", self.as_str())
    }
}

impl std::fmt::Display for SourceElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What implements an element.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceBy {
    /// What follows from the provider's credential and endpoints ([`default_source`]): its harness for a
    /// sign-in, a vendor's reads of a key at its hosts, each endpoint's list of models, else nothing.
    #[default]
    Default,
    /// A built-in source, by `name` ([`builtin`]).
    Builtin,
    /// A program registered on the machine that keeps the credential, by `name`.
    Program,
    /// An address, reached from the machine that keeps the credential.
    Address,
    /// Nothing: no facts are read beside what the keeper reports, no models are listed (they are added by
    /// name), no key is presented but by its own rule.
    None,
}

/// A secret of a source on its way to the machine that keeps the credential: as the server keeps the
/// command, the identifier of the provider secret; as the command is sent, its value sealed to that machine.
/// It is written as the link writes every secret of a command (`{"$secret": id}`, `{"$sealed": "…"}`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum SourceSecret {
    /// As the command is kept: the identifier of the secret.
    #[serde(rename = "$secret")]
    Ref(Id),
    /// As the command is sent: the value, sealed to the machine it goes to.
    #[serde(rename = "$sealed")]
    Sealed(String),
}

/// What the server tells the machine that keeps the credential of the source of one element, for one call.
/// Where a command carries none, the default source is used, as before sources could be chosen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceSpec {
    /// The server's current providers.allow_http_upstreams permission for this address call; absent, false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_http: bool,
    /// The provider's current name for the source's call context; absent on older commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<crate::source::SourceAskProvider>,
    /// What implements the element.
    pub by: SourceBy,
    /// The built-in source or the program, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The address, with how it is authorized and whether the provider's key may be sent to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<SourceAddress>,
    /// What the provider's managers gave for the source's fields that are no secrets.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, Json>,
    /// The values of the source's secret fields, by field.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, SourceSecret>,
    /// The address's authorization: the bearer token, or the value of the named header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<SourceSecret>,
}

impl SourceSpec {
    /// Whether this is the default source, which a command need not carry.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.by == SourceBy::Default
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The last error of a source, kept beside its choice and shown where the source is: its facts keep their
/// values and times, and the provider's status is untouched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceChoiceLastError {
    /// What went wrong.
    pub code: SourceErrorCode,
    /// The source's sentence: at most 300 characters, every secret the machine holds replaced by `[secret]`.
    pub message: String,
    /// When.
    pub at: Time,
}

/// What a provider's managers chose for one element, as `providers.sources` keeps it: the choice, with the
/// names of the secret fields that have a value instead of their values, the descriptor learned from the
/// source, and its last error.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceChoice {
    /// What implements the element.
    pub by: SourceBy,
    /// The built-in source or the program, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The address, with how it is authorized and whether the provider's key may be sent to it. The
    /// authorization itself is the provider secret `source:<element>:authorization`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<SourceAddress>,
    /// What the provider's managers gave for the source's fields that are no secrets.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, Json>,
    /// The source's secret fields that have a value, by name; each value is the provider secret
    /// `source:<element>:field:<name>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
    /// What the source said it is, the last time it was asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<SourceDescriptor>,
    /// When it was last asked what it is: an address is asked again a day later, and after an answer in
    /// another shape or one that it does not implement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub described_at: Option<Time>,
    /// The last error of the source, until a read succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<SourceChoiceLastError>,
}

/// Why a choice cannot be kept.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChoiceFault {
    /// A built-in source or a program is chosen without its name.
    #[error("a built-in source or a program is chosen by name")]
    NoName,
    /// No built-in source has the name.
    #[error("{0:?} is no built-in source")]
    UnknownBuiltin(String),
    /// An address is chosen without one.
    #[error("an address is chosen with its url")]
    NoAddress,
    /// A name or an address is given where the choice takes none.
    #[error("only a built-in source or a program has a name, and only an address an address")]
    Stray,
}

impl SourceChoice {
    /// The default source.
    #[must_use]
    pub fn default_source() -> Self {
        Self::default()
    }

    /// Checks that the choice names what it chooses: a built-in source or a program by name, an address by its
    /// address, and nothing else beside. The address itself is checked against the policy where it is set
    /// ([`SourceAddress::check`]).
    pub fn check(&self) -> Result<(), ChoiceFault> {
        match self.by {
            SourceBy::Builtin | SourceBy::Program => {
                let name = self
                    .name
                    .as_deref()
                    .filter(|name| !name.is_empty())
                    .ok_or(ChoiceFault::NoName)?;
                if self.by == SourceBy::Builtin && !builtin::ALL.contains(&name) {
                    return Err(ChoiceFault::UnknownBuiltin(name.to_owned()));
                }
                if self.address.is_some() {
                    return Err(ChoiceFault::Stray);
                }
            }
            SourceBy::Address => {
                if self.address.is_none() {
                    return Err(ChoiceFault::NoAddress);
                }
                if self.name.is_some() {
                    return Err(ChoiceFault::Stray);
                }
            }
            SourceBy::Default | SourceBy::None => {
                if self.name.is_some() || self.address.is_some() {
                    return Err(ChoiceFault::Stray);
                }
            }
        }
        Ok(())
    }

    /// What the server tells the keeping machine of this choice of `element`, with each secret as a reference
    /// to the provider secret that `secret_id` finds by name (`source:<element>:…`). A secret without a row is
    /// left out.
    #[must_use]
    pub fn spec(
        &self,
        element: SourceElement,
        secret_id: &dyn Fn(&str) -> Option<Id>,
    ) -> SourceSpec {
        let secrets = self
            .secrets
            .iter()
            .filter_map(|field| {
                secret_id(&element.field_secret(field))
                    .map(|id| (field.clone(), SourceSecret::Ref(id)))
            })
            .collect();
        let authorization = self
            .address
            .as_ref()
            .filter(|address| address.auth != AddressAuth::None)
            .and_then(|_| secret_id(&element.authorization_secret()))
            .map(SourceSecret::Ref);
        SourceSpec {
            allow_http: false,
            provider: None,
            by: self.by,
            name: self.name.clone(),
            address: self.address.clone(),
            config: self.config.clone(),
            secrets,
            authorization,
        }
    }
}

/// What a provider's managers chose for each element, as the column `providers.sources` keeps it. An element
/// left out has its default source: `{}` is what every provider had before sources could be chosen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSources {
    /// The source of its facts and of the resets they list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<SourceChoice>,
    /// The source of its models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<SourceChoice>,
    /// What presents its key, for a provider whose credential is a key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presenter: Option<SourceChoice>,
}

impl ProviderSources {
    /// The choice for `element`, where one was made.
    #[must_use]
    pub fn get(&self, element: SourceElement) -> Option<&SourceChoice> {
        match element {
            SourceElement::Facts => self.facts.as_ref(),
            SourceElement::Models => self.models.as_ref(),
            SourceElement::Presenter => self.presenter.as_ref(),
        }
    }

    /// The choice for `element`, to be changed.
    pub fn get_mut(&mut self, element: SourceElement) -> &mut Option<SourceChoice> {
        match element {
            SourceElement::Facts => &mut self.facts,
            SourceElement::Models => &mut self.models,
            SourceElement::Presenter => &mut self.presenter,
        }
    }

    /// What implements `element`: the default where nothing was chosen.
    #[must_use]
    pub fn by(&self, element: SourceElement) -> SourceBy {
        self.get(element)
            .map_or(SourceBy::Default, |choice| choice.by)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SECRET: &str = "0199c1a0-0000-7000-8000-0000000000aa";
    const TOKEN: &str = "0199c1a0-0000-7000-8000-0000000000ab";

    #[test]
    fn an_address_call_needs_the_servers_current_http_permission() {
        let mut spec: SourceSpec = serde_json::from_value(
            json!({"by":"address","address":{"url":"http://internal.example/report"}}),
        )
        .unwrap();
        assert!(!spec.allow_http);
        assert!(
            spec.address
                .as_ref()
                .unwrap()
                .check(spec.allow_http)
                .is_err()
        );
        spec.allow_http = true;
        let sent = serde_json::to_value(spec).unwrap();
        assert_eq!(sent["allow_http"], true);
    }

    fn address_choice() -> SourceChoice {
        serde_json::from_value(json!({
            "by": "address",
            "address": { "url": "https://plan.acme.example/orch", "auth": "bearer", "send_credential": false },
            "config": { "account": "team-4" },
            "secrets": ["token"],
            "described_at": "2026-10-15T10:00:00.000Z",
            "last_error": { "code": "timeout", "message": "plan.acme.example did not answer in time",
                            "at": "2026-10-15T10:05:00.000Z" }
        }))
        .unwrap()
    }

    #[test]
    fn every_provider_reads_its_sources_as_the_defaults_from_an_empty_column() {
        let sources: ProviderSources = serde_json::from_value(json!({})).unwrap();
        assert_eq!(sources, ProviderSources::default());
        assert_eq!(serde_json::to_value(&sources).unwrap(), json!({}));
        for element in SourceElement::ALL {
            assert_eq!(sources.by(element), SourceBy::Default);
        }
    }

    #[test]
    fn a_kept_choice_names_its_secrets_and_never_holds_their_values() {
        let choice = address_choice();
        assert_eq!(choice.check(), Ok(()));
        let mut sources = ProviderSources::default();
        *sources.get_mut(SourceElement::Facts) = Some(choice.clone());
        let kept = serde_json::to_value(&sources).unwrap();
        assert_eq!(kept["facts"]["secrets"], json!(["token"]));
        assert_eq!(kept["facts"]["last_error"]["code"], "timeout");
        assert!(kept["facts"].get("authorization").is_none());
        assert_eq!(
            serde_json::from_value::<ProviderSources>(kept).unwrap(),
            sources
        );
        let ids = |name: &str| -> Option<Id> {
            match name {
                "source:facts:field:token" => SECRET.parse().ok(),
                "source:facts:authorization" => TOKEN.parse().ok(),
                _ => None,
            }
        };
        let spec = choice.spec(SourceElement::Facts, &ids);
        assert_eq!(
            serde_json::to_value(&spec).unwrap(),
            json!({
                "by": "address",
                "address": { "url": "https://plan.acme.example/orch", "auth": "bearer", "send_credential": false },
                "config": { "account": "team-4" },
                "secrets": { "token": { "$secret": SECRET } },
                "authorization": { "$secret": TOKEN }
            })
        );
    }

    #[test]
    fn a_spec_on_the_link_carries_its_secrets_as_the_link_seals_them() {
        let sent: SourceSpec = serde_json::from_value(json!({
            "by": "program", "name": "acme-plan",
            "secrets": { "token": { "$sealed": "c2VhbGVk" } }
        }))
        .unwrap();
        assert_eq!(
            sent.secrets["token"],
            SourceSecret::Sealed("c2VhbGVk".to_owned())
        );
        assert!(!sent.is_default());
        let default = SourceSpec::default();
        assert!(default.is_default());
        assert_eq!(
            serde_json::to_value(&default).unwrap(),
            json!({ "by": "default" })
        );
    }

    #[test]
    fn a_choice_names_what_it_chooses_and_nothing_beside() {
        let choose = |value: Json| {
            serde_json::from_value::<SourceChoice>(value)
                .unwrap()
                .check()
        };
        assert_eq!(
            choose(json!({ "by": "builtin", "name": "openrouter" })),
            Ok(())
        );
        assert_eq!(
            choose(json!({ "by": "builtin", "name": "acme" })),
            Err(ChoiceFault::UnknownBuiltin("acme".into()))
        );
        assert_eq!(choose(json!({ "by": "program" })), Err(ChoiceFault::NoName));
        assert_eq!(
            choose(json!({ "by": "address" })),
            Err(ChoiceFault::NoAddress)
        );
        assert_eq!(
            choose(json!({ "by": "none", "name": "acme-plan" })),
            Err(ChoiceFault::Stray)
        );
        assert_eq!(choose(json!({ "by": "default" })), Ok(()));
        assert_eq!(
            SourceElement::Models.field_secret("token"),
            "source:models:field:token"
        );
    }

    #[test]
    fn the_default_source_follows_the_credential_and_the_endpoints_by_one_rule() {
        let table = crate::builtins::tests::table();
        let at = |text: &str| url::Url::parse(text).unwrap();
        let openrouter = [at("https://openrouter.ai/api/v1")];
        let deepseek = [at("https://api.deepseek.com/anthropic")];
        let elsewhere = [at("https://api.openai.com/v1")];
        let default = |element, keeper, endpoints: &[url::Url]| {
            default_source(element, keeper, endpoints, &table)
        };
        // A sign-in: its harness, whatever the endpoints.
        assert_eq!(
            default(SourceElement::Facts, Keeper::SignIn, &openrouter),
            Some(builtin::ADAPTER)
        );
        assert_eq!(
            default(SourceElement::Models, Keeper::SignIn, &[]),
            Some(builtin::ADAPTER)
        );
        // A key: the vendor's read at its host, else nothing; models from the endpoints.
        assert_eq!(
            default(SourceElement::Facts, Keeper::Key, &openrouter),
            Some(builtin::OPENROUTER)
        );
        assert_eq!(
            default(SourceElement::Facts, Keeper::Key, &deepseek),
            Some(builtin::DEEPSEEK)
        );
        assert_eq!(default(SourceElement::Facts, Keeper::Key, &elsewhere), None);
        assert_eq!(
            default(SourceElement::Models, Keeper::Key, &openrouter),
            Some(builtin::ENDPOINTS)
        );
        // No key at a vendor's host: its read needs the key, so nothing is read by default.
        assert_eq!(
            default(SourceElement::Facts, Keeper::NoKey, &openrouter),
            None
        );
        assert_eq!(
            default(SourceElement::Models, Keeper::NoKey, &openrouter),
            Some(builtin::ENDPOINTS)
        );
        // Nothing presents a key by default.
        for keeper in [Keeper::SignIn, Keeper::Key, Keeper::NoKey] {
            assert_eq!(default(SourceElement::Presenter, keeper, &openrouter), None);
        }
        // A program that registered no vendor reads names no default for a key.
        let none = crate::builtins::BuiltinSources::default();
        assert_eq!(
            default_source(SourceElement::Facts, Keeper::Key, &openrouter, &none),
            None
        );
    }
}
