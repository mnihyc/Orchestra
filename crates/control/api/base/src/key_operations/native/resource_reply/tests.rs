//! Actual optional collection Read never borrows a completed writer's authority.
#![allow(clippy::unwrap_used, reason = "test assertions fail by panicking")]

use super::*;

use orch_access_contract::key_scope::KeyAction;

use orch_access_contract::key_scope::KeyOperation;

use orch_access_contract::key_scope::KeyReach;

use orch_access_contract::key_scope::KeyRule;

use orch_access_contract::key_scope::KeyScope;

use orch_access_contract::key_scope::KeyTarget;

use orch_api_contract::directory::TagUpdate;

use orch_api_contract::operations::FacetSpec;

use orch_api_contract::operations::OperationSpec;

use orch_api_contract::operations::TargetResolver;

use orch_api_contract::routes::Endpoint;
#[derive(PartialEq, Eq)]
enum Source {
    Root,
    Content,
}

const ORG: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Org);

const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);

const SPEC: OperationSpec = OperationSpec::receipt(&OperationSpec::facets(&[
    FacetSpec::required(KeyAction::Manage, &[ORG], TargetResolver::Resolved("root")),
    FacetSpec::response_content(&[THREAD], TargetResolver::ResolvedCollection("contents")),
]));

const ROW: Declaration<Source> = Declaration {
    operation: "TagUpdate",
    bindings: &[
        Binding {
            source: Source::Root,
            target: TargetResolver::Resolved("root"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::Content,
            target: TargetResolver::ResolvedCollection("contents"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};

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

fn facts(ids: &[u128]) -> Vec<Facts<Source>> {
    vec![
        Facts::one(Source::Root, ORG, target(ORG, 10)),
        Facts::collection(
            Source::Content,
            if ids.is_empty() {
                Vec::new()
            } else {
                vec![(THREAD, ids.iter().map(|id| target(THREAD, *id)).collect())]
            },
        ),
    ]
}

fn plan(facts: &[Facts<Source>]) -> OperationPlan {
    let mut route = TagUpdate::ROUTE;
    route.operation = Some(SPEC);
    bind_required_route(route, &ROW, facts).unwrap()
}

fn caller(reads: &[(u128, &str)], manage: bool) -> Caller {
    let mut rules = reads
        .iter()
        .map(|(id, operation)| KeyRule {
            target: KeyTarget {
                kind: THREAD,
                id: Some(Id::from_u128(*id)),
                reach: KeyReach::SelfOnly,
            },
            resources: Some(vec![THREAD]),
            allow: vec![KeyOperation {
                operation: (*operation).to_owned(),
                actions: vec![KeyAction::Read],
            }],
        })
        .collect::<Vec<_>>();
    if manage {
        rules.push(KeyRule {
            target: KeyTarget {
                kind: ORG,
                id: Some(Id::from_u128(10)),
                reach: KeyReach::SelfOnly,
            },
            resources: Some(vec![ORG]),
            allow: vec![KeyOperation {
                operation: "TagUpdate".to_owned(),
                actions: vec![KeyAction::Manage],
            }],
        });
    }
    let scope = KeyScope {
        rules,
        ..KeyScope::default()
    };
    Caller::token(
        Id::from_u128(9),
        orch_authz::TokenGrant::from_stored_scope(&[], None, Some(scope), None).unwrap(),
    )
}

#[cfg(any(test, feature = "test-support"))]
fn collection_read_needs_every_actual_subject_and_never_readmits_completed_manage() {
    let facts = facts(&[1, 2]);
    let plan = plan(&facts);
    for reads in [
        vec![(1, "TagUpdate")],
        vec![(1, "TagUpdate"), (2, "TagDelete")],
    ] {
        assert!(
            read_caller(&caller(&reads, true), &plan, &ROW, &Source::Content, &facts)
                .unwrap()
                .is_none()
        );
    }
    let reader = read_caller(
        &caller(&[(1, "TagUpdate"), (2, "TagUpdate")], false),
        &plan,
        &ROW,
        &Source::Content,
        &facts,
    )
    .unwrap()
    .unwrap();
    let admitted = reader.admitted_facets().unwrap();
    assert_eq!(admitted.len(), 2);
    assert!(admitted.iter().all(|facet| facet.action == KeyAction::Read));
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_collection_read_needs_every_actual_subject_and_never_readmits_completed_manage() {
    self::collection_read_needs_every_actual_subject_and_never_readmits_completed_manage()
}

#[cfg(any(test, feature = "test-support"))]
fn empty_content_is_not_a_missing_role_a_singleton_or_a_new_permission() {
    let facts = facts(&[]);
    let plan = plan(&facts);
    let writer = caller(&[], true);
    let reader = read_caller(&writer, &plan, &ROW, &Source::Content, &facts)
        .unwrap()
        .unwrap();
    assert_eq!(reader, writer);
    assert!(reader.admitted_facets().is_none());
    assert!(read_caller(&writer, &plan, &ROW, &Source::Content, &facts[..1]).is_err());
    let mut wrong = facts;
    wrong[1].cardinality = Cardinality::One;
    assert!(read_caller(&writer, &plan, &ROW, &Source::Content, &wrong).is_err());
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_empty_content_is_not_a_missing_role_a_singleton_or_a_new_permission() {
    self::empty_content_is_not_a_missing_role_a_singleton_or_a_new_permission()
}
