//! Catalogue decisions distinguish deliberate native/protocol behavior from missing creation coverage.
use orch_api_contract::{
    ports::{EnvironmentPortOpeningCreate, PortLinkUseCreate},
    routes::{self, Endpoint, Idempotency},
    threads::{ThreadInputCreate, ThreadTurnRetry},
};
use orch_api_core::registry::{CreationCoverage, Registry};
orch_api_contract::endpoints! {
    /// An unclassified test declaration stays visibly undecided.
    UndecidedCreation: POST "/test-undecided-creation", orch_api_contract::routes::Access::Public, none, none, none, 204,
        operation orch_api_contract::operations::OperationSpec::protocol();
}
#[test]
fn creation_coverage_reads_the_anonymous_exchange_decision_without_treating_missing_rows_as_exempt()
{
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let reason = match PortLinkUseCreate::ROUTE.idempotency {
        Idempotency::NotKeyed(reason) => reason,
        _ => panic!("the public exchange needs its explicit catalogue decision"),
    };
    assert!(
        reason.contains("anonymous")
            && reason.contains("no account")
            && reason.contains("transient handle")
    );
    assert_eq!(
        registry
            .creation_coverage(PortLinkUseCreate)
            .unwrap_or_else(|error| panic!("{error:?}")),
        CreationCoverage::NotKeyed(reason)
    );
    assert_eq!(
        registry
            .creation_coverage(EnvironmentPortOpeningCreate)
            .unwrap_or_else(|error| panic!("{error:?}")),
        CreationCoverage::SharedHeader
    );
    assert_eq!(
        registry
            .creation_coverage(ThreadInputCreate)
            .unwrap_or_else(|error| panic!("{error:?}")),
        CreationCoverage::NativeClientId
    );
    assert_eq!(
        registry
            .creation_coverage(ThreadTurnRetry)
            .unwrap_or_else(|error| panic!("{error:?}")),
        CreationCoverage::NativeClientId
    );
    assert_eq!(
        registry
            .creation_coverage(UndecidedCreation)
            .unwrap_or_else(|error| panic!("{error:?}")),
        CreationCoverage::Undecided
    );
}
#[test]
fn internal_contract_keeps_the_unkeyed_reason_next_to_the_public_exchange() {
    let document = orch_api_contract::openapi::internal_document(&routes::all(), "test", |_| {});
    let metadata = &document["paths"]["/port-links/use"]["post"]["x-orch-idempotency"];
    assert_eq!(metadata["mode"], "not_keyed");
    assert!(
        metadata["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("no account"))
    );
}

#[test]
fn catalogue_preserves_the_native_token_header_exception_and_the_noncreating_preview() {
    use orch_api_contract::{access::SharePreviewCreate, auth::MeTokenCreate};
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let token = match MeTokenCreate::ROUTE.idempotency {
        Idempotency::NativeHeader(reason) => reason,
        _ => panic!("token creation keeps its existing one-time-secret native fence"),
    };
    assert!(token.contains("one-time secret"));
    assert_eq!(
        registry
            .creation_coverage(MeTokenCreate)
            .unwrap_or_else(|error| panic!("{error:?}")),
        CreationCoverage::NativeHeader(token)
    );
    let preview = match SharePreviewCreate::ROUTE.idempotency {
        Idempotency::NotKeyed(reason) => reason,
        _ => panic!("trial advice creates no object"),
    };
    assert!(preview.contains("rolled back") && preview.contains("no object"));
    assert_eq!(
        registry
            .creation_coverage(SharePreviewCreate)
            .unwrap_or_else(|error| panic!("{error:?}")),
        CreationCoverage::NotKeyed(preview)
    );
    let document = orch_api_contract::openapi::internal_document(&routes::all(), "test", |_| {});
    assert_eq!(
        document["paths"]["/me/tokens"]["post"]["x-orch-idempotency"],
        serde_json::json!({"mode":"native_header","reason":token})
    );
    assert_eq!(
        document["paths"]["/{object}/{id}/share/preview"]["post"]["x-orch-idempotency"],
        serde_json::json!({"mode":"not_keyed","reason":preview})
    );
}
