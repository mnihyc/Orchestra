//! DeepSeek's documented balance response through the common facts source interface.

use crate::{DOCUMENTATION, known_vendor, read};
use async_trait::async_trait;
use orch_provider_contract::builtins::{BuiltinSource, VendorClient};
use orch_provider_contract::choice::builtin as names;
use orch_provider_contract::source::{SourceCredential, SourceDescriptorFacts};
use orch_provider_contract::{
    FactMember, FactsReport, FactsSource, SourceAsk, SourceDescriptor, SourceError, SourceErrorCode,
};
use orch_types::Clock;
use std::sync::Arc;
use url::Url;

/// The documented balance reader; it has not been tried with a live DeepSeek key.
pub struct DeepSeek {
    client: Arc<dyn VendorClient>,
    clock: Arc<dyn Clock>,
}

impl DeepSeek {
    /// Uses the keeping machine's bounded HTTP client and observation clock.
    #[must_use]
    pub fn new(client: Arc<dyn VendorClient>, clock: Arc<dyn Clock>) -> Self {
        Self { client, clock }
    }
}

#[async_trait]
impl FactsSource for DeepSeek {
    fn describe(&self) -> SourceDescriptor {
        descriptor()
    }

    async fn read_facts(&self, ask: SourceAsk) -> Result<FactsReport, SourceError> {
        let key = ask.credential.ok_or_else(|| {
            SourceError::new(
                SourceErrorCode::Refused,
                "DeepSeek's balance read needs this provider's key",
            )
        })?;
        let base = ask
            .endpoints
            .iter()
            .filter_map(|endpoint| Url::parse(&endpoint.base_url).ok())
            .find(known_vendor)
            .ok_or_else(|| {
                SourceError::new(
                    SourceErrorCode::Unsupported,
                    "the provider has no documented DeepSeek endpoint",
                )
            })?;
        let mut report = read(self.client.as_ref(), &base, &key.key, self.clock.as_ref())
            .await
            .ok_or_else(|| {
                SourceError::new(
                    SourceErrorCode::NotTold,
                    "DeepSeek did not tell its balance on this read",
                )
            })?;
        if let Some(members) = report.as_object_mut() {
            members.remove("field_times");
        }
        Ok(FactsReport(report))
    }
}

/// The built-in declaration, also offered by the server before a credential is read.
#[must_use]
pub fn descriptor() -> SourceDescriptor {
    let mut descriptor = SourceDescriptor::new(
        "deepseek",
        "DeepSeek",
        "Reads the key's balance in each vendor currency and whether calls are available.",
    );
    descriptor.facts = Some(SourceDescriptorFacts {
        reports: vec![
            FactMember::Balances,
            FactMember::OrdinaryUsageAllowed,
            FactMember::CredentialVerified,
        ],
        credential: SourceCredential::Key,
        ..SourceDescriptorFacts::default()
    });
    descriptor.docs_url = Some(DOCUMENTATION.to_owned());
    descriptor
}

fn made(client: Arc<dyn VendorClient>, clock: Arc<dyn Clock>) -> Arc<dyn FactsSource> {
    Arc::new(DeepSeek::new(client, clock))
}

/// The built-in as a program registers it in its table of built-in sources: the default source of a key at
/// DeepSeek's documented bases.
#[must_use]
pub fn builtin() -> BuiltinSource {
    BuiltinSource {
        name: names::DEEPSEEK,
        hosts: &["api.deepseek.com"],
        descriptor,
        matches: known_vendor,
        make: made,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a test fails by panicking")]
    use super::*;
    use orch_plugin_contract::Secret;
    use orch_provider_contract::builtins::BuiltinSources;
    use orch_provider_contract::source::{
        SourceAskCredential, SourceAskEndpoint, SourceAskProvider,
    };
    use orch_types::{Json, ManualClock};
    use serde_json::json;
    use std::sync::Mutex;

    /// The vendor as a recorded answer, keeping what it was asked.
    struct Recorded {
        reply: Option<Json>,
        asked: Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl VendorClient for Recorded {
        async fn get(
            &self,
            base: &Url,
            path: &str,
            key: &Secret,
            _reply_bytes: usize,
        ) -> Option<Json> {
            assert_eq!(base.host_str(), Some("api.deepseek.com"));
            self.asked
                .lock()
                .unwrap()
                .push((path.to_owned(), key.expose().to_owned()));
            self.reply.clone()
        }
    }

    #[tokio::test]
    async fn the_registered_built_in_reads_the_balance_of_the_key_at_deepseeks_base() {
        let recorded = Arc::new(Recorded {
            reply: Some(
                json!({"is_available":true,"balance_infos":[{"currency":"USD","total_balance":"7.50","granted_balance":"0.00","topped_up_balance":"7.50"}]}),
            ),
            asked: Mutex::new(Vec::new()),
        });
        let entry = builtin();
        let table = BuiltinSources::new([entry]).unwrap();
        let base = Url::parse("https://api.deepseek.com").unwrap();
        assert_eq!(
            table
                .reading(std::slice::from_ref(&base))
                .map(|entry| entry.name),
            Some("deepseek")
        );
        assert_eq!((entry.descriptor)().name, entry.name);
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(read_at().get()));
        let source = (entry.make)(recorded.clone(), clock);
        let report = source
            .read_facts(SourceAsk {
                provider: SourceAskProvider {
                    id: orch_types::new_id(),
                    name: "DeepSeek".to_owned(),
                },
                endpoints: vec![SourceAskEndpoint {
                    protocol: "openai-responses".to_owned(),
                    base_url: base.to_string(),
                }],
                config: Default::default(),
                secrets: Default::default(),
                credential: Some(SourceAskCredential {
                    key: Secret::new("recorded-key"),
                }),
            })
            .await
            .unwrap();
        assert_eq!(report.0["balances"][0]["remaining"], "7.50");
        assert_eq!(
            recorded.asked.lock().unwrap().as_slice(),
            [("/user/balance".to_owned(), "recorded-key".to_owned())]
        );
    }

    fn read_at() -> orch_types::Time {
        "2026-10-15T00:00:00.000Z".parse().unwrap()
    }
}
