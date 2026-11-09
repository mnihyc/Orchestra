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
use std::collections::{BTreeSet, btree_map::Entry};
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

/// Catalogue mappings fill missing declarations, preserving actual supplied support and its source.
fn enrich_with_catalogue(
    suggestion: &mut ModelSuggestion,
    protocol: &str,
    projected: orch_provider_contract::capabilities::EndpointProjection,
) {
    if let Some(binding) = projected.native_tools.get("web_search")
        && suggestion
            .parameters
            .offered
            .as_ref()
            .and_then(|offered| offered.web_search.as_ref())
            .is_none()
    {
        let modes = binding
            .as_ref()
            .and_then(|binding| binding.request.get("modes"))
            .and_then(serde_json::Value::as_object)
            .map(|modes| modes.keys().cloned().collect())
            .unwrap_or_default();
        suggestion
            .parameters
            .offered
            .get_or_insert_with(Default::default)
            .web_search = Some(modes);
        suggestion
            .origins
            .insert("/parameters/offered/web_search".into(), "catalogue".into());
    }
    if !projected.native_tools.is_empty() {
        let native = suggestion
            .parameters
            .native_tools
            .entry(protocol.into())
            .or_default();
        for (tool, binding) in projected.native_tools {
            if let Entry::Vacant(slot) = native.entry(tool.clone()) {
                slot.insert(binding);
                let pointer = |part: &str| part.replace('~', "~0").replace('/', "~1");
                suggestion.origins.insert(
                    format!(
                        "/parameters/native_tools/{}/{}",
                        pointer(protocol),
                        pointer(&tool)
                    ),
                    "catalogue".into(),
                );
            }
        }
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
                    for mut suggestion in entry.protocol().parse_models(&body) {
                        let projected = forwarder.capability_sources().endpoint_capabilities(
                            endpoint.protocol(),
                            endpoint.base_url(),
                            &suggestion.model_name,
                        );
                        enrich_with_catalogue(&mut suggestion, endpoint.protocol(), projected);
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

#[cfg(test)]
mod tests {
    use super::*;
    use orch_provider_contract::{
        capabilities::{Binding, EndpointProjection},
        models::ModelOffered,
    };
    use serde_json::json;

    fn catalogue() -> EndpointProjection {
        EndpointProjection {
            native_tools: [
                (
                    "web_search".into(),
                    Some(Binding {
                        request: json!({"modes":{"live":{}}}),
                        ..Binding::default()
                    }),
                ),
                ("future/tool".into(), None),
            ]
            .into_iter()
            .collect(),
            ..EndpointProjection::default()
        }
    }

    #[test]
    fn catalogue_defaults_never_replace_actual_reported_modes_or_explicit_unsupported_bindings() {
        let mut model = ModelSuggestion::named("fixture-model", None);
        model.parameters.offered = Some(ModelOffered {
            web_search: Some(Vec::new()),
            ..ModelOffered::default()
        });
        model
            .parameters
            .native_tools
            .entry("future/wire".into())
            .or_default()
            .insert("web_search".into(), None);
        model
            .origins
            .insert("/parameters/offered/web_search".into(), "vendor".into());
        model.origins.insert(
            "/parameters/native_tools/future~1wire/web_search".into(),
            "vendor".into(),
        );
        enrich_with_catalogue(&mut model, "future/wire", catalogue());
        assert_eq!(
            model.parameters.offered.as_ref().unwrap().web_search,
            Some(Vec::new())
        );
        assert_eq!(
            model.parameters.native_tools["future/wire"]["web_search"],
            None
        );
        assert_eq!(model.origins["/parameters/offered/web_search"], "vendor");
        assert_eq!(
            model.origins["/parameters/native_tools/future~1wire/web_search"],
            "vendor"
        );
        assert_eq!(
            model.origins["/parameters/native_tools/future~1wire/future~1tool"],
            "catalogue"
        );
        assert!(model.parameters.web_search.is_none());
    }

    #[test]
    fn missing_values_are_catalogue_proposals_and_an_empty_extension_changes_nothing() {
        let mut model = ModelSuggestion::named("fixture-model", None);
        let unchanged = serde_json::to_vec(&model).unwrap();
        enrich_with_catalogue(&mut model, "future/wire", EndpointProjection::default());
        assert_eq!(serde_json::to_vec(&model).unwrap(), unchanged);
        enrich_with_catalogue(&mut model, "future/wire", catalogue());
        assert_eq!(
            model.parameters.offered.unwrap().web_search,
            Some(vec!["live".into()])
        );
        assert_eq!(model.origins["/parameters/offered/web_search"], "catalogue");
        assert!(model.parameters.web_search.is_none());
        assert!(model.parameters.default_web_search.is_none());
    }
}
