//! A native raw body supplies unknown operands, never a replacement destination or admission.
#![allow(clippy::unwrap_used, reason = "a control fails by panicking")]

use orch_access_contract::key_scope::{KeyObjectRef, KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_api_contract::routes::Endpoint;
use orch_authz::Caller;
use orch_engine::operation_scope::Prepared;
use orch_types::Id;

const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);
const UPLOAD: KeyTargetKind =
    KeyTargetKind::Identity(orch_access_contract::key_scope::KeyIdentityKind::Upload);

fn target(kind: KeyTargetKind, id: u128) -> KeyResolvedTarget {
    KeyResolvedTarget {
        object: KeyObjectRef {
            kind,
            id: Some(Id::from_u128(id)),
        },
        node_ancestors: Vec::new(),
        document_ancestors: Vec::new(),
        credential_account: None,
    }
}

fn prefix(destination: u128) -> Prepared {
    let mut plan = Prepared::new(
        orch_api_contract::threads::ThreadAttachmentCreate::ROUTE,
        "ThreadAttachmentCreate.from_upload",
    )
    .unwrap();
    let destination_slot = plan
        .variant()
        .facets
        .iter()
        .position(|facet| {
            facet.usage == FacetUsage::Required && facet.target == TargetResolver::PermissionObject
        })
        .unwrap();
    plan.bind(destination_slot, THREAD, &[target(THREAD, destination)])
        .unwrap();
    plan
}

fn complete(destination: u128, upload: u128) -> Prepared {
    let mut plan = prefix(destination);
    let source = plan
        .variant()
        .facets
        .iter()
        .position(|facet| {
            facet.usage == FacetUsage::Required
                && facet.target == TargetResolver::Resolved("attachment_source")
        })
        .unwrap();
    plan.bind(source, UPLOAD, &[target(UPLOAD, upload)])
        .unwrap();
    plan
}

#[test]
fn known_raw_destination_is_not_complete_admission_of_an_unknown_uploaded_source() {
    let early = prefix(1);
    let caller = Caller::session(Id::from_u128(9));
    assert!(early.covers_known_required_sources(&early));
    assert!(early.required_facets().is_err());
    assert!(early.admit(&caller, false).is_err());
    let full = complete(1, 2);
    assert!(full.covers_known_required_sources(&early));
    assert!(full.admit(&caller, false).is_ok());
    assert!(!full.covers_required_sources(&early));
    assert!(!early.covers_required_sources(&full));
    assert!(!prefix(3).covers_known_required_sources(&early));
    assert!(!complete(3, 2).covers_known_required_sources(&early));
}

#[test]
fn a_body_cannot_change_the_original_native_variant_or_borrow_another_operation() {
    let early = prefix(1);
    let mut direct = Prepared::new(
        orch_api_contract::threads::ThreadAttachmentCreate::ROUTE,
        "ThreadAttachmentCreate.direct",
    )
    .unwrap();
    direct.bind(0, THREAD, &[target(THREAD, 1)]).unwrap();
    assert!(!direct.covers_known_required_sources(&early));
    let mut document = Prepared::new(
        orch_api_contract::documents::DocumentFileCreate::ROUTE,
        "DocumentFileCreate.direct",
    )
    .unwrap();
    let document_kind = KeyTargetKind::Object(ObjectKind::Document);
    document
        .bind(0, document_kind, &[target(document_kind, 1)])
        .unwrap();
    assert!(!document.covers_known_required_sources(&early));
}

#[test]
fn missing_or_only_empty_known_roles_cannot_supply_a_raw_destination() {
    let missing = Prepared::new(
        orch_api_contract::threads::ThreadAttachmentCreate::ROUTE,
        "ThreadAttachmentCreate.from_upload",
    )
    .unwrap();
    assert!(!missing.covers_known_required_sources(&missing));
    assert!(!complete(1, 2).covers_known_required_sources(&missing));
    let mut empty = Prepared::new(
        orch_api_contract::notifications::NotificationChannelList::ROUTE,
        "NotificationChannelList.available",
    )
    .unwrap();
    for index in 0..empty.variant().facets.len() {
        empty.bind_empty(index).unwrap();
    }
    assert!(empty.required_facets().unwrap().is_empty());
    assert!(!empty.covers_known_required_sources(&empty));
}
