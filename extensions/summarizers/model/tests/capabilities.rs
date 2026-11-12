//! The shared resolver selects a real summary adapter; source/model mismatches issue no completion.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]
use async_trait::async_trait;
use orch_link_contract::commands::{ProviderComplete, ProviderCompleteResult};
use orch_provider_contract::SourceErrorCode;
use orch_provider_contract::capabilities::{
    self as cap, Availability, BackendRef, CallSource, CapabilityCall, Declarations, Endpoint,
    Executors, Interface, Offering, OfferingRef, Provider, Registry, Support, ThreadContext,
};
use orch_summarizer_contract::descriptor::SummaryField;
use orch_summarizer_contract::summarizer::{SummarizeError, SummaryLimits, SummaryRequest};
use orch_summarizer_model::{Complete, capabilities};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

struct Host {
    seen: Mutex<Vec<ProviderComplete>>,
}
#[async_trait]
impl Complete for Host {
    async fn complete(
        &self,
        _node: Id,
        request: ProviderComplete,
    ) -> Result<ProviderCompleteResult, SummarizeError> {
        self.seen.lock().unwrap().push(request);
        Ok(ProviderCompleteResult {
            text: "A completed step.\nThe next step remains.".into(),
            usage: Default::default(),
            request_ref: Some("synthetic-summary".into()),
            timing: Default::default(),
        })
    }
}

fn registry(provider: Id) -> Registry {
    let mut registry = Registry::default();
    registry
        .register("common", cap::catalogue().unwrap())
        .unwrap();
    // Discover the adapter through the descriptor boundary its host consumes, including
    // the validated extension wrapper's flattened wire representation.
    let descriptor = serde_json::to_value(orch_summarizer_model::descriptor::descriptor()).unwrap();
    let declarations =
        serde_json::from_value(descriptor["x_capability_declarations"].clone()).unwrap();
    registry.register("summarizer-model", declarations).unwrap();
    let mut declaration = Declarations::default();
    declaration.interfaces.insert(
        "test-generation".into(),
        Interface {
            kind: "generation".into(),
            ..Interface::default()
        },
    );
    declaration.providers.insert(
        provider,
        Provider {
            title: "Summary provider".into(),
            endpoints: BTreeMap::from([(
                "main".into(),
                Endpoint {
                    interface: "test-generation".into(),
                    ..Endpoint::default()
                },
            )]),
            offerings: BTreeMap::from([(
                "small".into(),
                Offering {
                    title: "Small".into(),
                    endpoint: "main".into(),
                    kind: "model".into(),
                    enabled: true,
                    capabilities: BTreeMap::from([(
                        "generation".into(),
                        Support {
                            enabled: true,
                            ..Support::default()
                        },
                    )]),
                    parameters: json!({"future_member":{"kept":true}}),
                    ..Offering::default()
                },
            )]),
            ..Provider::default()
        },
    );
    registry.register("admitted-account", declaration).unwrap();
    registry
}

fn context(provider: Id) -> ThreadContext {
    ThreadContext {
        choices: BTreeMap::from([("summary".into(), Some("service".into()))]),
        backends: BTreeMap::from([(
            "summary".into(),
            BackendRef {
                route: capabilities::ROUTE.into(),
                offering: Some(OfferingRef {
                    provider,
                    offering: "small".into(),
                }),
                ..BackendRef::default()
            },
        )]),
        availability: BTreeMap::from([(
            provider,
            BTreeMap::from([(
                "small".into(),
                Availability {
                    allowed: true,
                    ..Availability::default()
                },
            )]),
        )]),
        interfaces: vec!["test-generation".into()],
        ..ThreadContext::default()
    }
}

#[test]
fn an_explicit_model_summary_needs_an_installed_executor_and_current_authority() {
    let provider = new_id();
    let registry = registry(provider);
    let mut ctx = context(provider);
    let summary = |ctx: &ThreadContext| {
        registry
            .resolve(ctx)
            .into_iter()
            .find(|one| one.capability == "summary")
            .unwrap()
    };
    assert!(summary(&ctx).route.is_none());
    ctx.executors = vec![capabilities::EXECUTOR.into()];
    let resolved = summary(&ctx);
    assert_eq!(resolved.route.unwrap().id, capabilities::ROUTE);
    assert!(
        resolved.provider.is_none() && resolved.offering.is_none() && resolved.prices.is_empty(),
        "Use alone discloses no provider facts"
    );
    ctx.choices.clear();
    assert!(
        summary(&ctx).route.is_none(),
        "a configured backend does not spend by itself"
    );
    ctx.choices.insert("summary".into(), Some("service".into()));
    ctx.availability
        .get_mut(&provider)
        .unwrap()
        .get_mut("small")
        .unwrap()
        .allowed = false;
    assert!(summary(&ctx).route.is_none());
}

fn request(node: Id, provider: Id) -> SummaryRequest {
    SummaryRequest {
        field: SummaryField::Summary,
        node: serde_json::from_value(
            json!({"id":node,"kind":"topic","name":"Review","redacted":false}),
        )
        .unwrap(),
        previous: "Earlier context".into(),
        activity: vec![],
        tag_choices: vec![],
        limits: SummaryLimits {
            max_chars: 1200,
            max_tags: 0,
        },
        config: json!({"provider_id":provider,"model_name":"small"}),
    }
}

#[tokio::test]
async fn the_adapter_uses_one_host_completion_and_rejects_other_sources_models_and_zero_budgets() {
    let node = new_id();
    let provider = new_id();
    let source = CallSource {
        request_id: new_id(),
        node_id: Some(node),
        ..CallSource::default()
    };
    let host = Arc::new(Host {
        seen: Mutex::new(vec![]),
    });
    let mut executors = Executors::default();
    executors
        .register(
            capabilities::EXECUTOR,
            Arc::new(capabilities::Executor::new(host.clone(), source.clone())),
        )
        .unwrap();
    let registry = registry(provider);
    let mut ctx = context(provider);
    ctx.executors = executors.names();
    let resolved = registry
        .resolve(&ctx)
        .into_iter()
        .find(|one| one.capability == "summary")
        .unwrap();
    let call = CapabilityCall {
        source,
        capability: "summary".into(),
        route: resolved.route.unwrap(),
        item: "summary".into(),
        ledger_unit: "token".into(),
        input: serde_json::to_value(request(node, provider)).unwrap(),
    };
    for altered in ["source", "model", "budget"] {
        let mut refused = call.clone();
        match altered {
            "source" => refused.source.request_id = new_id(),
            "model" => refused.input["config"]["provider_id"] = json!(new_id()),
            _ => refused.input["limits"]["max_chars"] = json!(0),
        }
        assert_eq!(
            executors.execute(refused).await.unwrap_err().code,
            SourceErrorCode::Malformed
        );
    }
    assert!(host.seen.lock().unwrap().is_empty());
    let answer = executors.execute(call).await.unwrap();
    assert_eq!(host.seen.lock().unwrap().len(), 1);
    assert_eq!(answer.items[0].family, "summary");
    assert_eq!(
        answer.items[0].payload["text"],
        "A completed step. The next step remains."
    );
    assert_eq!(
        answer.items[0].native_ref.as_deref(),
        Some("synthetic-summary")
    );
    assert_eq!(
        answer.usage,
        Json::Null,
        "the host owns normalized usage; absent measurement states are not zero"
    );
}
