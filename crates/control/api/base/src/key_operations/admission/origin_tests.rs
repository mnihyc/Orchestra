//! A later key cannot widen an already restored native operation's scope boundary.
#![allow(clippy::unwrap_used, reason = "test assertions fail by panicking")]

use super::*;

use orch_access_contract::key_scope::KeyObjectRef;

use orch_access_contract::key_scope::KeyScope;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::operations::OperationSpec;

use orch_api_contract::routes::Access;

use orch_api_contract::routes::Endpoint;

use orch_types::Id;

use serde_json::json;
const OPERATION: &str = "RetainedReadControl";

const TOPIC: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Topic);

fn key(token: u128, topics: &[u128]) -> Caller {
    let rules = topics
        .iter()
        .map(|id| {
            json!({"target":{"kind":"topic","id":Id::from_u128(*id),"reach":"self"},
                "allow":[{"operation":OPERATION,"actions":["read"]}]})
        })
        .collect::<Vec<_>>();
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    Caller::token(
        Id::from_u128(9),
        orch_authz::TokenGrant::from_stored_scope(&[], None, Some(scope), None)
            .unwrap()
            .for_token(Id::from_u128(token)),
    )
}

fn target(id: u128) -> KeyResolvedTarget {
    KeyResolvedTarget {
        object: KeyObjectRef {
            kind: TOPIC,
            id: Some(Id::from_u128(id)),
        },
        node_ancestors: Vec::new(),
        document_ancestors: Vec::new(),
        credential_account: None,
    }
}

fn plan(id: u128) -> Prepared {
    let mut route = orch_api_contract::goals::ThreadGoalGet::ROUTE;
    route.name = OPERATION;
    route.access = Access::User;
    route.operation = Some(OperationSpec::single(
        KeyAction::Read,
        &[TOPIC],
        TargetResolver::Resolved("subject"),
    ));
    let mut plan = Prepared::new(route, OPERATION).unwrap();
    plan.bind(0, TOPIC, &[target(id)]).unwrap();
    plan
}

#[cfg(any(test, feature = "test-support"))]
fn a_wider_incoming_key_cannot_replace_the_restored_native_source_boundary() {
    let original = key(21, &[1]);
    let current = key(22, &[1, 2]);
    let mut selected = plan(2);
    assert!(selected.clone().admit(&current).is_ok());
    // Production installs only a live-restored CapturedOperation through with_origin.
    // Here the pure admission half receives that already restored boundary.
    selected.restriction = Some(original);
    assert!(
        !selected
            .permits_binding(&current, 0, TOPIC, &[target(2)])
            .unwrap()
    );
    assert!(selected.admit(&current).is_err());
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_wider_incoming_key_cannot_replace_the_restored_native_source_boundary() {
    self::a_wider_incoming_key_cannot_replace_the_restored_native_source_boundary()
}

#[cfg(any(test, feature = "test-support"))]
fn narrowed_native_admission_retains_both_keys_and_never_adds_private_mode() {
    let original = key(21, &[1]);
    let current = key(22, &[1, 2]);
    let mut selected = plan(1);
    selected.restriction = Some(original);
    let admitted = selected.admit(&current).unwrap().caller;
    let mut ids = admitted.grant().unwrap().token_ids().collect::<Vec<_>>();
    ids.sort_unstable();
    assert_eq!(ids, [Id::from_u128(21), Id::from_u128(22)]);
    assert_eq!(admitted.admitted_operation(), Some(OPERATION));
    assert!(!admitted.as_admin());
    assert!(admitted.permits_facets(admitted.admitted_facets().unwrap()));
    assert!(
        !plan(2)
            .permits_binding(&admitted, 0, TOPIC, &[target(2)])
            .unwrap()
    );
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_narrowed_native_admission_retains_both_keys_and_never_adds_private_mode() {
    self::narrowed_native_admission_retains_both_keys_and_never_adds_private_mode()
}

#[cfg(any(test, feature = "test-support"))]
fn a_restored_different_account_is_not_a_valid_native_operation_boundary() {
    let original = key(21, &[1]);
    let other = Caller::token(Id::from_u128(10), original.grant().unwrap().clone());
    let mut selected = plan(1);
    selected.restriction = Some(other);
    assert!(selected.admit(&key(22, &[1])).is_err());
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_restored_different_account_is_not_a_valid_native_operation_boundary() {
    self::a_restored_different_account_is_not_a_valid_native_operation_boundary()
}

#[cfg(any(test, feature = "test-support"))]
fn an_adoption_identity_prefix_retains_the_initial_narrower_credential_bounds() {
    let make =
        |token: u128, topics: &[u128]| {
            let rules = topics.iter().map(|id| json!({
            "target":{"kind":"topic","id":Id::from_u128(*id),"reach":"self"},
            "resources":["thread"],"allow":[{"operation":"ThreadAdopt","actions":["write"]}]
        })).collect::<Vec<_>>();
            let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
            assert!(scope.validate().is_empty());
            Caller::token(
                Id::from_u128(9),
                orch_authz::TokenGrant::from_stored_scope(&[], None, Some(scope), None)
                    .unwrap()
                    .for_token(Id::from_u128(token)),
            )
        };
    let original = make(21, &[1]);
    let current = make(22, &[1, 2]);
    let mut known = Prepared::new(orch_api_contract::nodes::ThreadAdopt::ROUTE, "ThreadAdopt")
        .unwrap()
        .adoption_creation_prefix()
        .unwrap();
    // The pure half receives the same already-restored restriction installed by with_origin.
    known.restriction = Some(original);
    let bound = known.creation_prefix_restrictions(&current).unwrap();
    let thread = KeyTargetKind::Object(ObjectKind::Thread);
    assert!(
        known
            .permits_binding(&current, 0, thread, &[target(1)])
            .unwrap()
    );
    let unrestricted =
        Prepared::new(orch_api_contract::nodes::ThreadAdopt::ROUTE, "ThreadAdopt").unwrap();
    assert!(
        unrestricted
            .permits_binding(&current, 0, thread, &[target(2)])
            .unwrap()
    );
    assert!(
        !unrestricted
            .permits_binding(&bound, 0, thread, &[target(2)])
            .unwrap(),
        "the private root-only checkpoint cannot discard the initial narrower source"
    );
    let mut ids = bound.grant().unwrap().token_ids().collect::<Vec<_>>();
    ids.sort_unstable();
    assert_eq!(ids, [Id::from_u128(21), Id::from_u128(22)]);
    assert!(!bound.as_admin());
    assert!(
        known.shared().required_facets().is_err(),
        "future creator roles remain unresolved"
    );
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_an_adoption_identity_prefix_retains_the_initial_narrower_credential_bounds() {
    self::an_adoption_identity_prefix_retains_the_initial_narrower_credential_bounds()
}
