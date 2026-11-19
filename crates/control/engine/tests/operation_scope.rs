//! The transport-neutral binder checks complete effects without granting account authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a control fails by panicking"
)]

use orch_access_contract::decisions::Scopes;
use orch_access_contract::key_scope::{
    KeyAction, KeyIdentityKind, KeyObjectRef, KeyOperation, KeyReach, KeyResolvedTarget, KeyRule,
    KeyScope, KeyTarget, KeyTargetKind,
};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::operations::{self, FacetSpec, FacetUsage, OperationSpec, TargetResolver};
use orch_api_contract::routes::{Access, Endpoint};
use orch_authz::{Act, Caller, CapturedOperation, Policy, Principal, TokenGrant};
use orch_engine::operation_scope::{BindingError, Prepared};
use orch_testkit::{clock, database::TestDb};
use orch_types::{Id, Time, new_id};

const DOCUMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Document);
const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);
const COPY: &str = "DocumentTreeOperate.copy";

fn target(kind: KeyTargetKind) -> KeyResolvedTarget {
    KeyResolvedTarget {
        object: KeyObjectRef {
            kind,
            id: Some(new_id()),
        },
        node_ancestors: Vec::new(),
        document_ancestors: Vec::new(),
        credential_account: None,
    }
}

fn rule(target: &KeyResolvedTarget, operation: &str, actions: &[KeyAction]) -> KeyRule {
    KeyRule {
        target: KeyTarget {
            kind: target.object.kind,
            id: target.object.id,
            reach: KeyReach::SelfOnly,
        },
        resources: None,
        allow: vec![KeyOperation {
            operation: operation.to_owned(),
            actions: actions.to_vec(),
        }],
    }
}

fn scoped(user: Id, rules: Vec<KeyRule>) -> Caller {
    Caller::token(
        user,
        TokenGrant::from_stored_scope(
            &[],
            None,
            Some(KeyScope {
                rules,
                ..KeyScope::default()
            }),
            None,
        )
        .unwrap(),
    )
}

fn copy() -> Prepared {
    Prepared::new(
        orch_api_contract::document_tree::DocumentTreeOperate::ROUTE,
        COPY,
    )
    .unwrap()
}

#[test]
fn compound_copy_requires_every_source_and_destination_after_partial_preparation() {
    let user = new_id();
    let source = target(DOCUMENT);
    let other_source = target(DOCUMENT);
    let destination = target(KeyTargetKind::Object(ObjectKind::Topic));
    let key = scoped(
        user,
        vec![
            rule(&source, COPY, &[KeyAction::Read]),
            rule(&destination, COPY, &[KeyAction::Write]),
        ],
    );
    let mut prepared = copy();
    assert_eq!(prepared.variant().operation, COPY);
    assert!(
        Prepared::new(
            orch_api_contract::document_tree::DocumentTreeOperate::ROUTE,
            "DocumentTreeOperate",
        )
        .is_err()
    );
    assert!(prepared.preparation_caller(&key).is_err());
    prepared.bind(1, DOCUMENT, &[destination.clone()]).unwrap();
    let preparation = prepared.preparation_caller(&key).unwrap();
    assert_eq!(preparation.user(), user);
    assert_eq!(preparation.grant(), key.grant());
    assert!(prepared.required_facets().is_err());
    assert!(prepared.admit(&preparation, false).is_err());
    assert!(
        prepared
            .permits_binding(&key, 0, DOCUMENT, &[source.clone()])
            .unwrap()
    );
    assert!(
        !prepared
            .permits_binding(&key, 0, DOCUMENT, &[other_source.clone()])
            .unwrap()
    );

    // A permitted destination and one permitted source cannot hide another actual source.
    let mut expanded = prepared.clone();
    expanded
        .bind(0, DOCUMENT, &[source.clone(), other_source])
        .unwrap();
    assert!(expanded.admit(&key, false).is_err());
    prepared.bind(0, DOCUMENT, &[source.clone()]).unwrap();
    let admitted = prepared.admit(&key, false).unwrap();
    assert_eq!(admitted.admitted_facets().unwrap().len(), 2);
    assert_eq!(admitted.admitted_operation(), Some(COPY));

    // Read on the destination and Write on the source cannot be recombined across rules.
    let reversed = scoped(
        user,
        vec![
            rule(&source, COPY, &[KeyAction::Write]),
            rule(&destination, COPY, &[KeyAction::Read]),
        ],
    );
    assert!(prepared.admit(&reversed, false).is_err());
}

#[test]
fn invalid_or_duplicate_source_bindings_cannot_erase_a_required_effect() {
    let source = target(DOCUMENT);
    let destination = target(KeyTargetKind::Object(ObjectKind::Topic));
    let mut prepared = copy();
    assert!(matches!(
        prepared.bind(0, THREAD, &[source.clone()]),
        Err(BindingError::Refused)
    ));
    assert!(prepared.bind(0, DOCUMENT, &[]).is_err());
    assert!(
        prepared.bind_empty(0).is_err(),
        "sources is not a collection declaration"
    );
    assert!(prepared.bind(99, DOCUMENT, &[source.clone()]).is_err());
    assert!(prepared.required_facets().is_err());
    prepared.bind(0, DOCUMENT, &[source.clone()]).unwrap();
    assert!(prepared.bind(0, DOCUMENT, &[target(DOCUMENT)]).is_err());
    assert!(prepared.bind_empty(0).is_err());
    prepared.bind(1, DOCUMENT, &[destination.clone()]).unwrap();
    let facts = prepared.required_facets().unwrap();
    assert_eq!(facts.len(), 2);
    assert_eq!(facts[0].target, source);
    assert_eq!(facts[1].target, destination);
    assert_eq!(
        facts[1].resource_kind, DOCUMENT,
        "creation family is not the parent's identity kind"
    );

    let deny_all = scoped(new_id(), Vec::new());
    assert!(prepared.admit(&deny_all, false).is_err());
    assert!(prepared.preparation_caller(&deny_all).is_err());
}

#[test]
fn legacy_admission_and_permission_bounds_survive_the_common_binder() {
    let user = new_id();
    let root = new_id();
    let legacy = Caller::token(
        user,
        TokenGrant::new(Scopes::Only([Permission::DocView].into()), Some(root)),
    );
    let mut document = target(DOCUMENT);
    document.node_ancestors.push(KeyObjectRef {
        kind: KeyTargetKind::Object(ObjectKind::Topic),
        id: Some(root),
    });
    let mut read = Prepared::new(
        orch_api_contract::documents::DocumentGet::ROUTE,
        "DocumentGet.read",
    )
    .unwrap();
    read.bind(0, DOCUMENT, &[document]).unwrap();
    // This projection has no ancillary names; their declared collections are explicitly empty.
    for index in 1..read.variant().facets.len() {
        read.bind_empty(index).unwrap();
    }
    let admitted = read.admit(&legacy, false).unwrap();
    assert_eq!(admitted.grant(), legacy.grant());
    assert!(admitted.permits(Permission::DocView));
    assert!(!admitted.permits(Permission::DocEdit));
    assert!(admitted.reaches(Some(&[root])));
    assert!(!admitted.reaches(Some(&[new_id()])));

    let account = KeyResolvedTarget {
        object: KeyObjectRef {
            kind: KeyTargetKind::Identity(KeyIdentityKind::Account),
            id: Some(user),
        },
        node_ancestors: Vec::new(),
        document_ancestors: Vec::new(),
        credential_account: None,
    };
    let mut issue = Prepared::new(
        orch_api_contract::auth::MeTokenCreate::ROUTE,
        "MeTokenCreate",
    )
    .unwrap();
    let key_kind = KeyTargetKind::Identity(KeyIdentityKind::Key);
    issue.bind(0, key_kind, &[account.clone()]).unwrap();
    let explicit = scoped(
        user,
        vec![rule(&account, "MeTokenCreate", &[KeyAction::Manage])],
    );
    assert!(issue.admit(&explicit, false).is_ok());
    assert!(issue.admit(&Caller::session(user), false).is_ok());
    let old_everything = Caller::token(user, TokenGrant::everything());
    assert!(issue.admit(&old_everything, false).is_err());
    assert!(issue.preparation_caller(&old_everything).is_err());
    assert!(
        issue
            .permits_binding(&old_everything, 0, key_kind, &[account])
            .is_err()
    );
    let retained_legacy = explicit.intersect_restrictions(&old_everything).unwrap();
    assert!(issue.admit(&retained_legacy, false).is_err());
}

#[test]
fn scoped_response_admission_requires_every_declared_response_target() {
    const SPEC: OperationSpec = OperationSpec::facets(&[
        FacetSpec::required(
            KeyAction::Write,
            &[DOCUMENT],
            TargetResolver::Resolved("destination"),
        ),
        FacetSpec::response_content(&[DOCUMENT], TargetResolver::Resolved("changed_document")),
        FacetSpec::response_content(&[DOCUMENT], TargetResolver::Resolved("related_document")),
    ]);
    let mut route = orch_api_contract::documents::DocumentOpsCreate::ROUTE;
    route.name = "BoundResponseControl";
    route.access = Access::User;
    route.operation = Some(OperationSpec::receipt(&SPEC));
    let destination = target(DOCUMENT);
    let related = target(DOCUMENT);
    let key = scoped(
        new_id(),
        vec![
            rule(
                &destination,
                route.name,
                &[KeyAction::Read, KeyAction::Write],
            ),
            rule(&related, route.name, &[KeyAction::Read]),
        ],
    );
    let mut prepared = Prepared::new(route, route.name).unwrap();
    prepared.bind(0, DOCUMENT, &[destination.clone()]).unwrap();
    prepared.bind(1, DOCUMENT, &[destination]).unwrap();
    assert_eq!(prepared.response_coverage(&key), None);
    assert!(prepared.response_facets().is_err());
    assert!(prepared.admit(&key, false).is_ok());
    assert!(
        prepared.admit(&key, true).is_err(),
        "one known read cannot complete an unresolved response"
    );

    let mut denied = prepared.clone();
    denied.bind(2, DOCUMENT, &[target(DOCUMENT)]).unwrap();
    assert_eq!(denied.response_coverage(&key), Some(false));
    assert!(denied.admit(&key, true).is_err());
    assert!(denied.admit(&key, false).is_ok());
    prepared.bind(2, DOCUMENT, &[related]).unwrap();
    assert_eq!(prepared.response_coverage(&key), Some(true));
    assert_eq!(prepared.response_facets().unwrap().len(), 2);
    assert_eq!(
        prepared
            .admit(&key, true)
            .unwrap()
            .admitted_facets()
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn captured_create_and_edit_admit_independently_but_still_need_current_thread_authority() {
    use orch_db::queries::access::{
        PageKey,
        testing::{add_entry, insert_member},
    };
    use orch_db::queries::hierarchy::testing::insert_node;
    use orch_db::queries::identity::{self, testing::insert_user};

    let db = TestDb::fresh().await;
    let now = Time::new(clock::start());
    let (owner, user, org, project, topic, thread) =
        (new_id(), new_id(), new_id(), new_id(), new_id(), new_id());
    let mut conn = db.pool.acquire().await.unwrap();
    insert_user(&mut conn, owner, "owner").await.unwrap();
    insert_user(&mut conn, user, "writer").await.unwrap();
    for (id, kind, parent) in [
        (org, "org", None),
        (project, "project", Some(org)),
        (topic, "topic", Some(project)),
        (thread, "thread", Some(topic)),
    ] {
        insert_node(&mut conn, id, kind, parent, kind, owner)
            .await
            .unwrap();
    }
    insert_member(&mut conn, org, user, org).await.unwrap();
    let mut materialized = target(THREAD);
    materialized.object.id = Some(thread);
    materialized.node_ancestors = [
        (ObjectKind::Org, org),
        (ObjectKind::Project, project),
        (ObjectKind::Topic, topic),
    ]
    .map(|(kind, id)| KeyObjectRef {
        kind: KeyTargetKind::Object(kind),
        id: Some(id),
    })
    .to_vec();
    let create_route = orch_api_contract::threads::ThreadInputCreate::ROUTE;
    let create_operation = operations::for_route(&create_route).unwrap();
    let create_selection = create_operation
        .variants
        .iter()
        .find(|variant| variant.name.is_empty())
        .or_else(|| {
            create_operation
                .variants
                .iter()
                .find(|variant| variant.name == "submit")
        })
        .expect("the current input contract declares a plain submission")
        .operation
        .as_str();
    let update_route = orch_api_contract::threads::ThreadInputUpdate::ROUTE;
    let update_operation = operations::for_route(&update_route).unwrap();
    let update_selection = update_operation
        .variants
        .iter()
        .find(|variant| variant.name.is_empty())
        .or_else(|| {
            update_operation
                .variants
                .iter()
                .find(|variant| variant.name == "edit")
        })
        .expect("the current input contract declares an ordinary edit")
        .operation
        .as_str();
    let selections = [
        (create_route, create_selection),
        (update_route, update_selection),
    ];
    let mut origins = Vec::new();
    for (route, selection) in selections {
        let token = new_id();
        let hash = token.as_bytes().repeat(2);
        let scope = KeyScope {
            rules: vec![rule(&materialized, selection, &[KeyAction::Write])],
            ..KeyScope::default()
        };
        identity::create_api_token_scoped(
            &mut conn,
            &identity::NewApiToken {
                id: token,
                user_id: user,
                name: selection,
                token_hash: &hash,
                scopes: &[],
                node_id: None,
                at: now.get(),
                expires_at: None,
            },
            Some(&serde_json::to_value(&scope).unwrap()),
        )
        .await
        .unwrap();
        let key = Caller::token(
            user,
            TokenGrant::from_stored_scope(&[], None, Some(scope), None)
                .unwrap()
                .for_token(token),
        );
        let mut prepared = Prepared::new(route, selection).unwrap();
        bind_plain_input(&mut prepared, &materialized);
        let accepted = prepared.admit(&key, false).unwrap();
        let encoded =
            serde_json::to_string(&CapturedOperation::capture(&accepted).unwrap()).unwrap();
        assert!(!encoded.contains("node_ancestors"));
        let origin: CapturedOperation = serde_json::from_str(&encoded).unwrap();
        assert_eq!(origin.operation(), selection);
        let restored = origin.restore(&mut conn, now).await.unwrap().unwrap();
        assert!(restored.admitted_facets().is_none());
        assert!(!restored.permits(Permission::ThreadInteract));
        origins.push((route, origin, restored));
    }
    let flattened = origins[0].2.intersect_restrictions(&origins[1].2).unwrap();
    let mut admitted = Vec::new();
    for (route, origin, restored) in &origins {
        let mut prepared = Prepared::new(*route, origin.operation()).unwrap();
        bind_plain_input(&mut prepared, &materialized);
        assert!(
            prepared.admit(&flattened, false).is_err(),
            "distinct accepted operation names are not interchangeable ceilings"
        );
        let caller = prepared.admit(restored, false).unwrap();
        let principal = Principal::load(&mut conn, caller.clone(), now)
            .await
            .unwrap();
        assert!(
            !orch_authz::nodes::check(
                &mut conn,
                &principal,
                thread,
                Permission::ThreadInteract,
                Act::Use,
                Policy::default(),
            )
            .await
            .unwrap()
            .is_allowed(),
            "key admission grants no ThreadInteract to its account"
        );
        admitted.push(caller);
    }
    add_entry(
        &mut conn,
        &PageKey::new("node", thread, "live"),
        "any",
        Permission::ThreadInteract.as_str(),
        ("user", Some(user), None),
        None,
        now.get(),
    )
    .await
    .unwrap();
    for caller in admitted {
        let principal = Principal::load(&mut conn, caller, now).await.unwrap();
        assert!(
            orch_authz::nodes::check(
                &mut conn,
                &principal,
                thread,
                Permission::ThreadInteract,
                Act::Use,
                Policy::default(),
            )
            .await
            .unwrap()
            .is_allowed()
        );
        assert!(!principal.is_system_admin());
    }
}

/// This control submits literal words: it has no source reads, native selection or bypass.
/// Only declared collections may be empty; any new required singleton needs real facts.
fn bind_plain_input(prepared: &mut Prepared, thread: &KeyResolvedTarget) {
    let required: Vec<_> = prepared
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter(|(_, facet)| facet.usage == FacetUsage::Required)
        .map(|(index, facet)| (index, facet.target, facet.resources.clone()))
        .collect();
    let mut has_thread = false;
    for (index, target, resources) in required {
        match target {
            TargetResolver::Path("id") => {
                assert!(resources.contains(&THREAD));
                prepared
                    .bind(index, THREAD, std::slice::from_ref(thread))
                    .unwrap();
                has_thread = true;
            }
            TargetResolver::ResolvedCollection(_) => prepared.bind_empty(index).unwrap(),
            other => panic!("plain input control needs facts for required target {other:?}"),
        }
    }
    assert!(
        has_thread,
        "the plain operation must bind its actual thread"
    );
}
