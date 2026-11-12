//! Failed native work binds actual error sources without inventing a successful creation.
#![allow(clippy::unwrap_used, reason = "a control fails by panicking")]

use orch_access_contract::key_scope::{
    KeyAction, KeyObjectRef, KeyOperation, KeyReach, KeyResolvedTarget, KeyRule, KeyScope,
    KeyTarget, KeyTargetKind,
};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::directory::OrgGroupCreate;
use orch_api_contract::directory_sources::{self as directory, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::routes::Endpoint;
use orch_authz::{Caller, TokenGrant};
use orch_engine::operation_scope::{Prepared, sources};
use orch_types::Id;

const ORG: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Org);
const ERROR_ORGS: Declaration<Source> = Declaration {
    operation: "OrgGroupCreate",
    bindings: &[directory::GROUP_CREATE.bindings[2]],
};
const CREATED_ONLY: Declaration<Source> = Declaration {
    operation: "OrgGroupCreate",
    bindings: &[directory::GROUP_CREATE.bindings[1]],
};

fn org(id: u128) -> KeyResolvedTarget {
    KeyResolvedTarget {
        object: KeyObjectRef {
            kind: ORG,
            id: Some(Id::from_u128(id)),
        },
        node_ancestors: Vec::new(),
        document_ancestors: Vec::new(),
        credential_account: None,
    }
}

fn plan(targets: Vec<KeyResolvedTarget>) -> Prepared {
    sources::bind_declared(
        OrgGroupCreate::ROUTE,
        &ERROR_ORGS,
        &[sources::Facts::collection(
            Source::GroupOrgNames,
            if targets.is_empty() {
                Vec::new()
            } else {
                vec![(ORG, targets)]
            },
        )],
    )
    .unwrap()
}

fn read_slot(plan: &Prepared) -> usize {
    sources::indices(plan, &ERROR_ORGS.bindings[0]).unwrap()[0]
}

fn caller(target: &KeyResolvedTarget, operation: &str, action: KeyAction) -> Caller {
    Caller::token(
        Id::from_u128(9),
        TokenGrant::from_stored_scope(
            &[],
            None,
            Some(KeyScope {
                rules: vec![KeyRule {
                    target: KeyTarget {
                        kind: target.object.kind,
                        id: target.object.id,
                        reach: KeyReach::Subtree,
                    },
                    resources: None,
                    allow: vec![KeyOperation {
                        operation: operation.to_owned(),
                        actions: vec![action],
                    }],
                }],
                ..KeyScope::default()
            }),
            None,
        )
        .unwrap(),
    )
}

#[test]
fn an_error_shape_reads_actual_sources_without_a_created_subject_or_effect_admission() {
    let target = org(1);
    let prepared = plan(vec![target.clone()]);
    let actual = prepared
        .response_facets_for(&[read_slot(&prepared)])
        .unwrap();
    assert_eq!(actual.len(), 1);
    assert_eq!(actual[0].operation, "OrgGroupCreate");
    assert_eq!(actual[0].action, KeyAction::Read);
    assert_eq!(actual[0].resource_kind, ORG);
    assert_eq!(actual[0].target, target);
    assert!(prepared.required_facets().is_err());
    assert!(prepared.response_facets().is_err());
    assert!(
        prepared
            .admit(&Caller::session(Id::from_u128(9)), false)
            .is_err()
    );
    assert!(
        prepared
            .admit(&Caller::session(Id::from_u128(9)), true)
            .is_err()
    );
    let reader = caller(&target, "OrgGroupCreate", KeyAction::Read);
    assert!(reader.admit_facets(&actual).is_ok());
    assert!(prepared.admit(&reader, false).is_err());
    assert!(
        caller(&target, "OrgGroupCreate", KeyAction::Manage)
            .admit_facets(&actual)
            .is_err()
    );
    assert!(
        caller(&target, "OrgGet.read", KeyAction::Read)
            .admit_facets(&actual)
            .is_err()
    );
    assert!(
        caller(&org(2), "OrgGroupCreate", KeyAction::Read)
            .admit_facets(&actual)
            .is_err()
    );
}

#[test]
fn a_failure_shape_keeps_missing_duplicate_scalar_and_response_only_rules() {
    let prepared = plan(vec![org(1)]);
    let index = read_slot(&prepared);
    for indices in [
        vec![],
        vec![index, index],
        vec![0],
        vec![usize::MAX],
        vec![1],
    ] {
        assert!(prepared.response_facets_for(&indices).is_err());
    }
    assert!(sources::bind_declared(OrgGroupCreate::ROUTE, &ERROR_ORGS, &[]).is_err());
    let fact = sources::Facts::collection(Source::GroupOrgNames, vec![(ORG, vec![org(1)])]);
    assert!(
        sources::bind_declared(
            OrgGroupCreate::ROUTE,
            &ERROR_ORGS,
            &[fact.clone(), fact.clone()],
        )
        .is_err()
    );
    assert!(
        sources::bind_declared(
            OrgGroupCreate::ROUTE,
            &ERROR_ORGS,
            &[
                fact,
                sources::Facts::collection(Source::CreatedGroup, Vec::new())
            ],
        )
        .is_err()
    );
    assert!(
        sources::bind_declared(
            OrgGroupCreate::ROUTE,
            &CREATED_ONLY,
            &[sources::Facts::collection(Source::CreatedGroup, Vec::new())],
        )
        .is_err(),
        "an absent creation cannot be a successful empty scalar"
    );
    let empty = plan(Vec::new());
    assert!(
        empty
            .response_facets_for(&[read_slot(&empty)])
            .unwrap()
            .is_empty()
    );
    assert!(empty.required_facets().is_err());
}

#[test]
fn fresh_error_sources_keep_original_identities_and_never_reuse_old_ancestry() {
    let old_home = org(10);
    let new_home = org(11);
    let mut original = org(1);
    original.node_ancestors.push(old_home.object);
    let mut moved = original.clone();
    moved.node_ancestors = vec![new_home.object];
    let opened = plan(vec![original.clone()]);
    let fresh = plan(vec![moved]);
    let indices = [read_slot(&opened)];
    assert!(fresh.covers_response_sources(&opened, &indices));
    let reader = caller(&old_home, "OrgGroupCreate", KeyAction::Read);
    assert!(
        reader
            .admit_facets(&opened.response_facets_for(&indices).unwrap())
            .is_ok()
    );
    assert!(
        reader
            .admit_facets(&fresh.response_facets_for(&indices).unwrap())
            .is_err()
    );
    assert!(!plan(vec![org(2)]).covers_response_sources(&opened, &indices));
    assert!(!plan(Vec::new()).covers_response_sources(&opened, &indices));
    assert!(plan(vec![original, org(2)]).covers_response_sources(&opened, &indices));
    assert!(!fresh.covers_response_sources(&opened, &[]));
}

#[test]
fn two_error_roles_cannot_exchange_the_same_readable_native_identities() {
    use orch_api_contract::ownership::NodeDeletePreview;
    use orch_api_contract::ownership_sources::{self as ownership, Source};
    const SHAPE: Declaration<Source> = Declaration {
        operation: "NodeDeletePreview",
        bindings: &[
            ownership::DELETE_PREVIEW.bindings[1],
            ownership::DELETE_PREVIEW.bindings[2],
        ],
    };
    let prepare = |root, place| {
        sources::bind_declared(
            NodeDeletePreview::ROUTE,
            &SHAPE,
            &[
                sources::Facts::one(Source::DeleteReviewNode, ORG, org(root)),
                sources::Facts::collection(
                    Source::DeleteReviewPlaces,
                    vec![(ORG, vec![org(place)])],
                ),
            ],
        )
        .unwrap()
    };
    let original = prepare(1, 2);
    let indices = SHAPE
        .bindings
        .iter()
        .flat_map(|binding| sources::indices(&original, binding).unwrap())
        .collect::<Vec<_>>();
    assert!(prepare(1, 2).covers_response_sources(&original, &indices));
    assert!(!prepare(2, 1).covers_response_sources(&original, &indices));
    assert!(!original.covers_response_sources(&prepare(2, 1), &indices));
}
