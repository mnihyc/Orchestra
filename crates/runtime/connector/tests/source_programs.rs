//! A single registered provider program supplies its declared interfaces, with no replacement of built-in or earlier names.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use async_trait::async_trait;
use orch_connector_core::sources::Programs;
use orch_provider_contract::source::{
    SourceAskProvider, SourceDescriptorFacts, SourceDescriptorModels,
};
use orch_provider_contract::{
    CredentialPresenter, FactMember, FactsReport, FactsSource, ModelsReport, ModelsSource,
    PresentAsk, Presented, SourceAsk, SourceDescriptor, SourceError,
};
use orch_types::new_id;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Example {
    calls: Arc<AtomicUsize>,
    name: String,
}
impl Example {
    fn descriptor(&self) -> SourceDescriptor {
        let mut descriptor =
            SourceDescriptor::new(&self.name, "Example", "A registered provider source.");
        descriptor.facts = Some(SourceDescriptorFacts {
            reports: vec![FactMember::Plan],
            ..SourceDescriptorFacts::default()
        });
        descriptor.models = Some(SourceDescriptorModels::default());
        descriptor
    }
}

#[async_trait]
impl FactsSource for Example {
    fn describe(&self) -> SourceDescriptor {
        self.descriptor()
    }
    async fn read_facts(&self, _ask: SourceAsk) -> Result<FactsReport, SourceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(FactsReport(json!({"plan":"pro"})))
    }
}
#[async_trait]
impl ModelsSource for Example {
    fn describe(&self) -> SourceDescriptor {
        self.descriptor()
    }
    async fn list_models(&self, _ask: SourceAsk) -> Result<ModelsReport, SourceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ModelsReport {
            models: Some(Vec::new()),
        })
    }
}
#[async_trait]
impl CredentialPresenter for Example {
    fn describe(&self) -> SourceDescriptor {
        self.descriptor()
    }
    async fn present(&self, _ask: PresentAsk) -> Result<Presented, SourceError> {
        Err(SourceError::unsupported("credential.present"))
    }
}

fn ask() -> SourceAsk {
    SourceAsk {
        provider: SourceAskProvider {
            id: new_id(),
            name: "Team".to_owned(),
        },
        endpoints: Vec::new(),
        config: Default::default(),
        secrets: Default::default(),
        credential: None,
    }
}

#[tokio::test]
async fn one_negotiated_program_supplies_both_interfaces_and_a_duplicate_never_replaces_it() {
    let calls = Arc::new(AtomicUsize::new(0));
    let program = Arc::new(Example {
        calls: Arc::clone(&calls),
        name: "example".to_owned(),
    });
    let mut programs = Programs::default();
    programs.register("example", Arc::clone(&program)).unwrap();
    assert!(programs.register("example", Arc::clone(&program)).is_err());
    assert!(
        programs
            .register("openrouter", Arc::clone(&program))
            .is_err()
    );
    assert!(
        programs
            .register(
                "alias",
                Arc::new(Example {
                    calls: Arc::clone(&calls),
                    name: "deepseek".to_owned()
                })
            )
            .is_err()
    );
    assert!(programs.get("missing").is_none());
    let source = programs.get("example").unwrap();
    assert_eq!(
        FactsSource::read_facts(source.as_ref(), ask())
            .await
            .unwrap()
            .0,
        json!({"plan":"pro"})
    );
    assert_eq!(
        ModelsSource::list_models(source.as_ref(), ask())
            .await
            .unwrap()
            .models,
        Some(Vec::new())
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "both calls reach the same registered instance"
    );
}
