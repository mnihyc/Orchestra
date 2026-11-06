//! Operation policy narrows the same Caller; admission never substitutes for account authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a control fails by panicking"
)]

mod support;

use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{
    KeyAction, KeyEmptyFacet, KeyFacet, KeyObjectRef, KeyOperation, KeyReach, KeyResolvedTarget,
    KeyRule, KeyScope, KeyTarget, KeyTargetKind,
};
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{ObjectKind, Permission as P};
use orch_authz::pages::Target;
use orch_authz::{
    Caller, CapturedCaller, CapturedOperation, KeptOut, KeyScopeRefusal, Policy, Principal,
    TokenGrant, documents,
};
use orch_db::queries::{documents as rows, identity};
use orch_types::{Id, Time, new_id};
use serde_json::json;
use support::World;

fn object(kind: ObjectKind, id: Id) -> KeyObjectRef {
    KeyObjectRef {
        kind: KeyTargetKind::Object(kind),
        id: Some(id),
    }
}

fn rule(target: KeyObjectRef, reach: KeyReach, operation: &str, action: KeyAction) -> KeyRule {
    KeyRule {
        target: KeyTarget {
            kind: target.kind,
            id: target.id,
            reach,
        },
        resources: None,
        allow: vec![KeyOperation {
            operation: operation.into(),
            actions: vec![action],
        }],
    }
}

fn facet(target: KeyObjectRef, operation: &str, action: KeyAction) -> KeyFacet {
    KeyFacet {
        operation: operation.into(),
        action,
        resource_kind: target.kind,
        target: KeyResolvedTarget {
            object: target,
            node_ancestors: Vec::new(),
            document_ancestors: Vec::new(),
            credential_account: None,
        },
    }
}

fn caller(user: Id, scope: KeyScope, expiry: Option<Time>) -> Caller {
    Caller::token(
        user,
        TokenGrant::from_stored_scope(&[], None, Some(scope), expiry).unwrap(),
    )
}

#[test]
fn rule_mode_fails_closed_until_shared_admission_and_cannot_borrow_browser_admin() {
    let user = new_id();
    let document = object(ObjectKind::Document, new_id());
    let scope = KeyScope {
        rules: vec![rule(
            document,
            KeyReach::SelfOnly,
            "DocumentOpsCreate",
            KeyAction::Write,
        )],
        constraints: Vec::new(),
        excluded_places: Vec::new(),
    };
    let key = caller(user, scope, None);
    assert!(!key.permits(P::DocEdit));
    assert!(
        key.grant().unwrap().scopes().to_strings().is_empty(),
        "obsolete consumers see no permissive fallback"
    );
    assert!(key.admit_facets(&[]).is_err());
    assert!(
        key.admit_facets(&[facet(document, "DocumentGet", KeyAction::Read)])
            .is_err()
    );
    let admitted = key
        .admit_facets(&[facet(document, "DocumentOpsCreate", KeyAction::Write)])
        .unwrap()
        .into_caller();
    assert_eq!(admitted.user(), user);
    assert!(
        admitted.permits(P::DocEdit),
        "ordinary permission evaluation may now proceed"
    );
    assert!(!admitted.as_admin());
    assert!(!admitted.clone().with_admin_mode(true).as_admin());
    let browser = Caller::session(user).with_admin_mode(true);
    let narrowed = admitted.intersect_restrictions(&browser).unwrap();
    assert!(!narrowed.as_admin());
    assert!(
        !narrowed.permits(P::DocEdit),
        "intersection discards old admission"
    );
    assert!(narrowed.admitted_facets().is_none());
}

#[test]
fn new_place_restrictions_invalidate_only_admission_they_actually_block() {
    let user = new_id();
    let org = object(ObjectKind::Org, new_id());
    let document = object(ObjectKind::Document, new_id());
    let key = caller(
        user,
        KeyScope {
            rules: vec![rule(
                document,
                KeyReach::SelfOnly,
                "DocumentGet",
                KeyAction::Read,
            )],
            ..KeyScope::default()
        },
        None,
    );
    let mut read = facet(document, "DocumentGet", KeyAction::Read);
    read.target.node_ancestors.push(org);
    let admitted = key.admit_facets(&[read]).unwrap().into_caller();
    let unrelated = admitted
        .clone()
        .with_kept_out(vec![KeptOut::Refused { org: new_id() }]);
    assert!(unrelated.admitted_facets().is_some());
    assert!(unrelated.permits(P::DocView));
    let refused = admitted.clone().with_kept_out(vec![KeptOut::Refused {
        org: org.id.unwrap(),
    }]);
    assert!(refused.admitted_facets().is_none());
    assert!(!refused.permits(P::DocView));
    let expired_policy = admitted.with_kept_out(vec![KeptOut::Lifetime {
        place: None,
        max_days: 1,
    }]);
    assert!(expired_policy.admitted_facets().is_none());
    assert!(!expired_policy.permits(P::DocView));
}

#[test]
fn child_scope_snapshots_the_issuer_policy_and_cannot_outlive_or_escape_it() {
    let user = new_id();
    let project = object(ObjectKind::Project, new_id());
    let document = object(ObjectKind::Document, new_id());
    let end = Time::new(orch_testkit::clock::start() + chrono::Duration::hours(1));
    let scope = KeyScope {
        rules: vec![rule(
            project,
            KeyReach::Subtree,
            "DocumentGet",
            KeyAction::Read,
        )],
        constraints: Vec::new(),
        excluded_places: Vec::new(),
    };
    let issuer = caller(user, scope.clone(), Some(end));
    let child_rules = vec![rule(
        document,
        KeyReach::SelfOnly,
        "DocumentGet",
        KeyAction::Read,
    )];
    assert_eq!(
        issuer.attenuate_key_scope(child_rules.clone(), None),
        Err(KeyScopeRefusal::Lifetime)
    );
    let later = Time::new(end.get() + chrono::Duration::seconds(1));
    assert_eq!(
        issuer.attenuate_key_scope(child_rules.clone(), Some(later)),
        Err(KeyScopeRefusal::Lifetime)
    );
    let child = issuer
        .attenuate_key_scope(child_rules.clone(), Some(end))
        .unwrap();
    assert_eq!(child.constraints, vec![scope.rules]);
    let mut read = facet(document, "DocumentGet", KeyAction::Read);
    read.target.node_ancestors.push(project);
    assert!(child.allows(&[read.clone()]));
    read.target.node_ancestors.clear();
    assert!(!child.allows(&[read]));
    let legacy = Caller::token(user, TokenGrant::everything());
    assert_eq!(
        legacy.attenuate_key_scope(child_rules, Some(end)),
        Err(KeyScopeRefusal::LegacyIssuance)
    );
}

#[test]
fn child_policy_cannot_escape_issuer_refusal_or_lifetime_exclusions() {
    let user = new_id();
    let refused = object(ObjectKind::Org, new_id());
    let allowed = object(ObjectKind::Org, new_id());
    let a = object(ObjectKind::Document, new_id());
    let b = object(ObjectKind::Document, new_id());
    let policy = KeyScope {
        rules: vec![
            rule(refused, KeyReach::Subtree, "DocumentGet", KeyAction::Read),
            rule(allowed, KeyReach::Subtree, "DocumentGet", KeyAction::Read),
        ],
        ..KeyScope::default()
    };
    let mut read_a = facet(a, "DocumentGet", KeyAction::Read);
    read_a.target.node_ancestors.push(refused);
    let mut read_b = facet(b, "DocumentGet", KeyAction::Read);
    read_b.target.node_ancestors.push(allowed);
    for exclusion in [
        KeptOut::Refused {
            org: refused.id.unwrap(),
        },
        KeptOut::Lifetime {
            place: refused.id,
            max_days: 10,
        },
    ] {
        let issuer = Caller::token(
            user,
            TokenGrant::from_stored_scope(&[], None, Some(policy.clone()), None)
                .unwrap()
                .kept_out(vec![exclusion]),
        );
        assert!(!issuer.permits_facets(&[read_a.clone()]));
        assert!(issuer.permits_facets(&[read_b.clone()]));
        let child = issuer
            .attenuate_key_scope(
                vec![
                    rule(a, KeyReach::SelfOnly, "DocumentGet", KeyAction::Read),
                    rule(b, KeyReach::SelfOnly, "DocumentGet", KeyAction::Read),
                ],
                None,
            )
            .unwrap();
        assert!(
            !child.allows(&[read_a.clone()]),
            "a new token identity must not bypass the issuer's excluded place"
        );
        assert!(
            child.allows(&[read_b.clone()]),
            "an unrelated allowed place remains usable"
        );
        let encoded = serde_json::to_string(&child).unwrap();
        assert!(
            !encoded.contains("token_id"),
            "the retained ceiling is policy, not a live parent credential"
        );
    }
}

#[tokio::test]
async fn admitted_scope_still_requires_the_same_current_resource_permission() {
    let w = World::new().await;
    let reader = w.member("scoped-reader").await;
    let document = new_id();
    let mut conn = w.conn().await;
    rows::containers::insert_document(
        &mut conn,
        &rows::NewDocument {
            id: document,
            node_id: w.topic,
            org_id: w.root,
            owner_id: w.boss,
            created_by_thread: None,
            created_by_agent: None,
            title: "Scope is no permission",
            now: w.now.get(),
        },
        None,
        &format!("Scope~{}.md", document.simple()),
    )
    .await
    .unwrap();
    documents::start(&mut conn, document, w.boss, w.now)
        .await
        .unwrap();
    drop(conn);
    let target = Target::Resource(ObjectRef {
        kind: ObjectKind::Document,
        id: document,
    });
    w.change(w.boss, target, json!({"whole":"private"}))
        .await
        .unwrap();
    let object = object(ObjectKind::Document, document);
    let key = caller(
        reader,
        KeyScope {
            rules: vec![rule(
                object,
                KeyReach::SelfOnly,
                "DocumentGet",
                KeyAction::Read,
            )],
            constraints: Vec::new(),
            excluded_places: Vec::new(),
        },
        None,
    );
    let admitted = key
        .admit_facets(&[facet(object, "DocumentGet", KeyAction::Read)])
        .unwrap()
        .into_caller();
    let mut conn = w.conn().await;
    let principal = Principal::load(&mut conn, admitted.clone(), w.now)
        .await
        .unwrap();
    assert!(
        !documents::check(
            &mut conn,
            &principal,
            document,
            P::DocView,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    drop(conn);
    w.change(
        w.boss,
        target,
        json!({"rows":[{"permission":"doc.view","add":[{"holder":Holder::User{id:reader}}]}]}),
    )
    .await
    .unwrap();
    let mut conn = w.conn().await;
    let principal = Principal::load(&mut conn, admitted, w.now).await.unwrap();
    assert!(
        documents::check(
            &mut conn,
            &principal,
            document,
            P::DocView,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    assert!(!principal.is_system_admin());
}

async fn stored(w: &World, scope: &KeyScope, expiry: Time) -> Id {
    let id = new_id();
    let hash = id.as_bytes().repeat(2);
    let mut conn = w.conn().await;
    identity::create_api_token_scoped(
        &mut conn,
        &identity::NewApiToken {
            id,
            user_id: w.maker,
            name: "captured",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: w.now.get(),
            expires_at: Some(expiry.get()),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn restored_work_rechecks_current_key_policy_without_reusing_admission_or_paths() {
    let w = World::new().await;
    let thread = object(ObjectKind::Thread, w.thread);
    let read = rule(thread, KeyReach::SelfOnly, "ThreadSyncGet", KeyAction::Read);
    let write = rule(
        thread,
        KeyReach::SelfOnly,
        "ThreadInputCreate",
        KeyAction::Write,
    );
    let accepted = KeyScope {
        rules: vec![read.clone(), write.clone()],
        constraints: Vec::new(),
        excluded_places: Vec::new(),
    };
    let expiry = w.later(chrono::Duration::hours(1));
    let id = stored(&w, &accepted, expiry).await;
    let key = Caller::token(
        w.maker,
        TokenGrant::from_stored_scope(&[], None, Some(accepted.clone()), Some(expiry))
            .unwrap()
            .for_token(id),
    );
    let admitted = key
        .admit_facets(&[facet(thread, "ThreadInputCreate", KeyAction::Write)])
        .unwrap()
        .into_caller();
    let captured = CapturedCaller::capture(&admitted);
    let encoded = serde_json::to_string(&captured).unwrap();
    assert!(!encoded.contains("node_ancestors") && !encoded.contains("admitted"));
    let mut conn = w.conn().await;
    let current = KeyScope {
        rules: vec![read],
        constraints: Vec::new(),
        excluded_places: Vec::new(),
    };
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(id)
        .bind(serde_json::to_value(current).unwrap())
        .execute(&mut *conn)
        .await
        .unwrap();
    let restored = captured.restore(&mut conn, w.now).await.unwrap().unwrap();
    assert!(restored.admitted_facets().is_none());
    assert!(!restored.permits(P::ThreadView));
    assert!(
        restored
            .admit_facets(&[facet(thread, "ThreadSyncGet", KeyAction::Read)])
            .is_ok()
    );
    assert!(
        restored
            .admit_facets(&[facet(thread, "ThreadInputCreate", KeyAction::Write)])
            .is_err()
    );
    let expanded = KeyScope {
        rules: vec![
            write,
            rule(
                thread,
                KeyReach::SelfOnly,
                "ThreadUpdate",
                KeyAction::Manage,
            ),
        ],
        constraints: Vec::new(),
        excluded_places: Vec::new(),
    };
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(id)
        .bind(serde_json::to_value(expanded).unwrap())
        .execute(&mut *conn)
        .await
        .unwrap();
    let restored = captured.restore(&mut conn, w.now).await.unwrap().unwrap();
    assert!(
        restored
            .admit_facets(&[facet(thread, "ThreadInputCreate", KeyAction::Write)])
            .is_ok()
    );
    assert!(
        restored
            .admit_facets(&[facet(thread, "ThreadUpdate", KeyAction::Manage)])
            .is_err()
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(id)
        .bind(w.now.get())
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(captured.restore(&mut conn, w.now).await.unwrap().is_none());
}

#[test]
fn checked_empty_keeps_exact_evidence_and_loses_admission_on_restriction_or_capture() {
    use orch_access_contract::key_scope::KeyIdentityKind;
    let user = new_id();
    let account = KeyObjectRef {
        kind: KeyTargetKind::Identity(KeyIdentityKind::Account),
        id: Some(user),
    };
    let key = caller(
        user,
        KeyScope {
            rules: vec![rule(
                account,
                KeyReach::Subtree,
                "MeSessionOthersDelete",
                KeyAction::Manage,
            )],
            ..KeyScope::default()
        },
        None,
    );
    let required = [KeyEmptyFacet {
        action: KeyAction::Manage,
        resources: vec![KeyTargetKind::Identity(KeyIdentityKind::Session)],
    }];
    assert!(key.admit_facets(&[]).is_err());
    assert!(
        key.admit_empty_operation("MeSessionOthersDelete", &[])
            .is_err()
    );
    let admitted = key
        .admit_empty_operation("MeSessionOthersDelete", &required)
        .unwrap()
        .into_caller();
    assert_eq!(admitted.admitted_operation(), Some("MeSessionOthersDelete"));
    assert_eq!(admitted.admitted_empty_facets(), Some(required.as_slice()));
    assert_eq!(admitted.admitted_facets(), Some([].as_slice()));
    assert!(!admitted.as_admin());
    let narrowed = admitted.intersect_restrictions(&key).unwrap();
    assert!(narrowed.admitted_operation().is_none());
    let expired_policy = admitted.clone().with_kept_out(vec![KeptOut::Lifetime {
        place: None,
        max_days: 1,
    }]);
    assert!(expired_policy.admitted_operation().is_none());
    assert!(expired_policy.admitted_facets().is_none());
    assert!(!expired_policy.permits(P::ThreadInteract));
    assert!(
        expired_policy
            .admit_empty_operation("MeSessionOthersDelete", &required)
            .is_err()
    );
    let captured = serde_json::to_value(CapturedCaller::capture(&admitted)).unwrap();
    assert!(captured.get("admission").is_none());
    assert!(!captured.to_string().contains("required"));
    let browser = Caller::session(user)
        .admit_empty_operation("MeSessionOthersDelete", &required)
        .unwrap()
        .into_caller();
    assert_eq!(browser.admitted_operation(), Some("MeSessionOthersDelete"));
    assert!(
        !serde_json::to_string(&CapturedCaller::capture(&browser))
            .unwrap()
            .contains("MeSessionOthersDelete")
    );
}

#[test]
fn deferred_origin_requires_actual_selection_and_never_serializes_target_facts() {
    let user = new_id();
    let thread = object(ObjectKind::Thread, new_id());
    let browser = Caller::session(user);
    assert_eq!(
        CapturedOperation::capture(&browser),
        Err(KeyScopeRefusal::Unclassified)
    );
    let mut write = facet(thread, "ThreadInputCreate.submit", KeyAction::Write);
    write
        .target
        .node_ancestors
        .push(object(ObjectKind::Topic, new_id()));
    let admitted = browser
        .admit_facets(&[write.clone()])
        .unwrap()
        .into_caller();
    let origin = CapturedOperation::capture(&admitted).unwrap();
    assert_eq!(origin.operation(), "ThreadInputCreate.submit");
    assert_eq!(origin.user(), user);
    let encoded = serde_json::to_string(&origin).unwrap();
    assert!(!encoded.contains("node_ancestors"));
    assert!(!encoded.contains("admission"));
    assert!(!encoded.contains(&thread.id.unwrap().to_string()));
    let decoded: CapturedOperation = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, origin);
    let mut edit = write.clone();
    edit.operation = "ThreadInputUpdate.edit".into();
    let ambiguous = browser.admit_facets(&[write, edit]).unwrap().into_caller();
    assert_eq!(
        CapturedOperation::capture(&ambiguous),
        Err(KeyScopeRefusal::Unclassified)
    );
}

#[tokio::test]
async fn different_deferred_operations_keep_each_original_current_boundary() {
    let w = World::new().await;
    let thread = object(ObjectKind::Thread, w.thread);
    let expiry = w.later(chrono::Duration::hours(1));
    let mut origins = Vec::new();
    let mut ids = Vec::new();
    for selection in ["ThreadInputCreate.submit", "ThreadInputUpdate.edit"] {
        let scope = KeyScope {
            rules: vec![rule(
                thread,
                KeyReach::SelfOnly,
                selection,
                KeyAction::Write,
            )],
            ..KeyScope::default()
        };
        let id = stored(&w, &scope, expiry).await;
        let key = Caller::token(
            w.maker,
            TokenGrant::from_stored_scope(&[], None, Some(scope), Some(expiry))
                .unwrap()
                .for_token(id),
        );
        assert!(CapturedOperation::capture(&key).is_err());
        let admitted = key
            .admit_facets(&[facet(thread, selection, KeyAction::Write)])
            .unwrap()
            .into_caller();
        origins.push(CapturedOperation::capture(&admitted).unwrap());
        ids.push(id);
    }
    let mut conn = w.conn().await;
    for origin in &origins {
        let current = origin.restore(&mut conn, w.now).await.unwrap().unwrap();
        assert!(current.admitted_operation().is_none());
        assert!(
            current
                .admit_facets(&[facet(thread, origin.operation(), KeyAction::Write)])
                .is_ok()
        );
        assert!(
            current
                .admit_facets(&[facet(thread, origin.operation(), KeyAction::Manage)])
                .is_err()
        );
        assert!(
            current
                .admit_facets(&[facet(
                    object(ObjectKind::Thread, new_id()),
                    origin.operation(),
                    KeyAction::Write
                )])
                .is_err()
        );
        assert!(origin.restore(&mut conn, expiry).await.unwrap().is_none());
    }
    let create = origins[0].restore(&mut conn, w.now).await.unwrap().unwrap();
    let update = origins[1].restore(&mut conn, w.now).await.unwrap().unwrap();
    let wrongly_merged = create.intersect_restrictions(&update).unwrap();
    assert!(
        wrongly_merged
            .admit_facets(&[facet(thread, origins[0].operation(), KeyAction::Write)])
            .is_err()
    );
    assert!(
        wrongly_merged
            .admit_facets(&[facet(thread, origins[1].operation(), KeyAction::Write)])
            .is_err()
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(ids[1])
        .bind(serde_json::to_value(KeyScope::default()).unwrap())
        .execute(&mut *conn)
        .await
        .unwrap();
    let current = origins[1].restore(&mut conn, w.now).await.unwrap().unwrap();
    assert!(
        current
            .admit_facets(&[facet(thread, origins[1].operation(), KeyAction::Write)])
            .is_err()
    );
    assert!(
        origins[0]
            .restore(&mut conn, w.now)
            .await
            .unwrap()
            .unwrap()
            .admit_facets(&[facet(thread, origins[0].operation(), KeyAction::Write)])
            .is_ok()
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(ids[0])
        .bind(w.now.get())
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        origins[0]
            .restore(&mut conn, w.now)
            .await
            .unwrap()
            .is_none()
    );
}
