//! OpenRouter's key and account-credit reads through the common facts source interface.

use crate::{KnownVendor, known_vendor, read_details};
use async_trait::async_trait;
use orch_agent_contract::facts::normalize;
use orch_provider_contract::builtins::{BuiltinSource, VendorClient};
use orch_provider_contract::choice::builtin as names;
use orch_provider_contract::facts::{ProviderFacts, Spend, SpendPeriod};
use orch_provider_contract::source::{SourceCredential, SourceDescriptorFacts};
use orch_provider_contract::{
    FactMember, FactsReport, FactsSource, SourceAsk, SourceDescriptor, SourceError, SourceErrorCode,
};
use orch_types::{Clock, Json};
use std::sync::Arc;
use url::Url;

/// The ordinary-key reader with its transport supplied by the credential's keeping machine.
pub struct OpenRouter {
    client: Arc<dyn VendorClient>,
    clock: Arc<dyn Clock>,
}

impl OpenRouter {
    /// Uses the keeping machine's bounded HTTP client and observation clock.
    #[must_use]
    pub fn new(client: Arc<dyn VendorClient>, clock: Arc<dyn Clock>) -> Self {
        Self { client, clock }
    }
}

#[async_trait]
impl FactsSource for OpenRouter {
    fn describe(&self) -> SourceDescriptor {
        descriptor()
    }

    async fn read_facts(&self, ask: SourceAsk) -> Result<FactsReport, SourceError> {
        let key = ask.credential.ok_or_else(|| {
            SourceError::new(
                SourceErrorCode::Refused,
                "OpenRouter's facts need this provider's key",
            )
        })?;
        let base = ask
            .endpoints
            .iter()
            .filter_map(|endpoint| Url::parse(&endpoint.base_url).ok())
            .find(|base| known_vendor(base) == Some(KnownVendor::OpenRouter))
            .ok_or_else(|| {
                SourceError::new(
                    SourceErrorCode::Unsupported,
                    "the provider has no documented OpenRouter endpoint",
                )
            })?;
        let (mut facts, data) = read_details(
            self.client.as_ref(),
            &base,
            &key.key,
            KnownVendor::OpenRouter,
            self.clock.as_ref(),
        )
        .await
        .ok_or_else(|| {
            SourceError::new(
                SourceErrorCode::NotTold,
                "OpenRouter did not tell its key facts on this read",
            )
        })?;
        normalize(&mut facts);
        spend_details(&mut facts, &data);
        let serialized = serde_json::to_value(&facts).map_err(|_| {
            SourceError::new(
                SourceErrorCode::Malformed,
                "OpenRouter's facts could not be read",
            )
        })?;
        let mut report = serde_json::Map::new();
        for member in FactMember::ALL {
            if facts.was_told(member)
                && let Some(value) = serialized.get(member.as_str())
            {
                report.insert(member.as_str().to_owned(), value.clone());
            }
        }
        Ok(FactsReport(Json::Object(report)))
    }
}

fn spend_details(facts: &mut ProviderFacts, data: &Json) {
    // A vendor boolean is kept as vendor metadata, never guessed into a plan name.
    if let Some(free) = data.get("is_free_tier").and_then(Json::as_bool)
        && let Some(row) = facts.spend.iter_mut().find(|row| row.id == "key:total")
    {
        let _ = row
            .extensions
            .insert("x_openrouter_is_free_tier", Json::Bool(free));
    }
    for (suffix, period, field) in [
        ("total", SpendPeriod::Total, "byok_usage"),
        ("day", SpendPeriod::Day, "byok_usage_daily"),
        ("week", SpendPeriod::Week, "byok_usage_weekly"),
        ("month", SpendPeriod::Month, "byok_usage_monthly"),
    ] {
        let Some(used) = crate::amount(data.get(field)).and_then(crate::decimal) else {
            continue;
        };
        facts.spend.push(Spend {
            id: format!("byok:{suffix}"),
            label: Some("BYOK".into()),
            unit: crate::UNIT.into(),
            period,
            period_minutes: None,
            used: Some(used),
            limit: None,
            remaining: None,
            resets_at: None,
            extensions: Default::default(),
        });
    }
}

/// The built-in declaration, also offered by the server before a credential is read. The account's credits
/// are told only for a management key, so balances are not declared: an ordinary key shows them as
/// unsupported, and a balance that is told shows all the same.
#[must_use]
pub fn descriptor() -> SourceDescriptor {
    let mut descriptor = SourceDescriptor::new(
        "openrouter",
        "OpenRouter",
        "Reads the key's limits with their resets, its spending, the free models' requests of the day, the account's credits for a management key, and the key's lapse.",
    );
    descriptor.facts = Some(SourceDescriptorFacts {
        reports: vec![
            FactMember::Windows,
            FactMember::Spend,
            FactMember::Credential,
            FactMember::CredentialVerified,
        ],
        credential: SourceCredential::Key,
        ..SourceDescriptorFacts::default()
    });
    descriptor.docs_url =
        Some("https://openrouter.ai/docs/api/reference/api-keys/get-current-api-key".to_owned());
    descriptor
}

fn reads(base: &Url) -> bool {
    known_vendor(base).is_some()
}

fn made(client: Arc<dyn VendorClient>, clock: Arc<dyn Clock>) -> Arc<dyn FactsSource> {
    Arc::new(OpenRouter::new(client, clock))
}

/// The built-in as a program registers it in its table of built-in sources: the default source of a key at
/// OpenRouter's documented addresses.
#[must_use]
pub fn builtin() -> BuiltinSource {
    BuiltinSource {
        name: names::OPENROUTER,
        hosts: &["openrouter.ai"],
        descriptor,
        matches: reads,
        make: made,
    }
}
