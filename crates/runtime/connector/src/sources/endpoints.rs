//! The endpoints' bounded model catalogues, preserving pagination, deduplication and refusal behavior.

use crate::forwarder::Forwarder;
use crate::roles::provider::{ProviderEntry, failure_of, refusal_of, unsupported};
use crate::upstream::UpstreamError;
use async_trait::async_trait;
use orch_agent_contract::discovery::ModelSuggestion;
use orch_link_contract::commands::CommandError;
use orch_provider_contract::source::SourceDescriptorModels;
use orch_provider_contract::{
    ModelsReport, ModelsSource, SourceAsk, SourceDescriptor, SourceError, SourceErrorCode,
};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The current endpoints and their protocols as one model source, using the forwarder's bounded transport.
pub struct Endpoints {
    forwarder: Arc<Forwarder>,
    provider: Arc<ProviderEntry>,
}

impl Endpoints {
    /// Keeps the same provider key and policy checks as forwarded model calls.
    #[must_use]
    pub fn new(forwarder: Arc<Forwarder>, provider: Arc<ProviderEntry>) -> Self {
        Self {
            forwarder,
            provider,
        }
    }
}

#[async_trait]
impl ModelsSource for Endpoints {
    fn describe(&self) -> SourceDescriptor {
        let mut descriptor = SourceDescriptor::new(
            "endpoints",
            "Endpoints",
            "Reads each endpoint's own list of models.",
        );
        descriptor.models = Some(SourceDescriptorModels::default());
        descriptor
    }

    async fn list_models(&self, ask: SourceAsk) -> Result<ModelsReport, SourceError> {
        if ask.provider.id != self.provider.id() {
            return Err(SourceError::new(
                SourceErrorCode::Refused,
                "the source belongs to another provider",
            ));
        }
        if !self.provider.endpoints().iter().any(|endpoint| {
            self.forwarder
                .protocols()
                .get(endpoint.protocol())
                .is_some_and(|entry| entry.protocol().models_request().is_some())
        }) {
            return Ok(ModelsReport::unlisted());
        }
        let models = list(&self.forwarder, &self.provider)
            .await
            .map_err(super::adapter::command_error)?;
        Ok(ModelsReport::listed(&models))
    }
}

/// Reads the lists in endpoint order without changing a provider's accepted models.
pub async fn list(
    forwarder: &Forwarder,
    provider: &ProviderEntry,
) -> Result<Vec<ModelSuggestion>, CommandError> {
    let mut found: Vec<ModelSuggestion> = Vec::new();
    let mut last_failure: Option<CommandError> = None;
    for endpoint in provider.endpoints() {
        let Some(entry) = forwarder.protocols().get(endpoint.protocol()) else {
            continue;
        };
        let Some((_, first)) = entry.protocol().models_request() else {
            continue;
        };
        let mut requested = first.to_owned();
        let mut visited = BTreeSet::new();
        for page in 0..orch_model_contract::protocol::MODEL_DISCOVERY_MAX_PAGES {
            if !visited.insert(requested.clone()) {
                return Err(unsupported(
                    "the provider's model pagination repeats a page",
                ));
            }
            match forwarder
                .ask_models_page(provider, endpoint, entry, &requested)
                .await
            {
                Ok((status, body)) if status.is_success() => {
                    for suggestion in entry.protocol().parse_models(&body) {
                        if !found
                            .iter()
                            .any(|known| known.model_name == suggestion.model_name)
                        {
                            if found.len()
                                == orch_model_contract::protocol::MODEL_DISCOVERY_MAX_MODELS
                            {
                                return Err(unsupported(
                                    "the provider's model list exceeds the discovery limit",
                                ));
                            }
                            found.push(suggestion);
                        }
                    }
                    let next = entry
                        .protocol()
                        .next_models_request(&requested, &body)
                        .map_err(|_| {
                            unsupported("the provider's model list has invalid pagination")
                        })?;
                    let Some(next) = next else { break };
                    if page + 1 == orch_model_contract::protocol::MODEL_DISCOVERY_MAX_PAGES {
                        return Err(unsupported(
                            "the provider's model list exceeds the page limit",
                        ));
                    }
                    requested = next;
                }
                Ok((status, _)) => {
                    last_failure = Some(refusal_of(status.as_u16()));
                    break;
                }
                Err(error @ UpstreamError::Address(_)) => return Err(failure_of(&error)),
                Err(error) => {
                    last_failure = Some(failure_of(&error));
                    break;
                }
            }
        }
    }
    match (found.is_empty(), last_failure) {
        (true, Some(failure)) => Err(failure),
        _ => Ok(found),
    }
}
