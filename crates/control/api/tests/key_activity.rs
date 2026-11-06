//! Key history keeps credential attribution separate from current reader and resource authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use chrono::Duration;
use orch_access_contract::key_scope::{
    KeyAction, KeyIdentityKind, KeyObjectRef, KeyOperation, KeyReach, KeyRule, KeyScope, KeyTarget,
    KeyTargetKind,
};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::access::{Holder, PermissionPageUpdate};
use orch_api_contract::auth::{
    ApiTokenActivityQuery, MeGet, MeTokenActivityList, MeTokenCreate, OrgTokenActivityList,
};
use orch_api_contract::common::{ErrorCode, IDEMPOTENCY_HEADER, Page};
use orch_api_contract::nodes::NodeGet;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::{identity, key_activity};
use orch_types::{Id, new_id};
use serde_json::json;
use support::{FullTokenReply as _, World};

async fn credential(world: &World, owner: Id, id: Id, scope: Option<&KeyScope>) -> String {
    let secret = new_token(API_TOKEN_PREFIX);
    let scopes = if scope.is_some() {
        Vec::new()
    } else {
        vec!["*".to_owned()]
    };
    let scope = scope.map(|scope| serde_json::to_value(scope).unwrap());
    let mut conn = world.conn().await;
    identity::create_api_token_scoped(
        &mut conn,
        &identity::NewApiToken {
            id,
            user_id: owner,
            name: "Automation",
            token_hash: &token_hash(&secret),
            scopes: &scopes,
            node_id: None,
            at: world.now(),
            expires_at: None,
        },
        scope.as_ref(),
    )
    .await
    .unwrap();
    secret
}

fn query(limit: Option<u32>, cursor: Option<String>) -> ApiTokenActivityQuery {
    ApiTokenActivityQuery { limit, cursor }
}

fn read_scope(target: Id) -> KeyScope {
    KeyScope {
        rules: vec![KeyRule {
            target: KeyTarget {
                kind: KeyTargetKind::Identity(KeyIdentityKind::Key),
                id: Some(target),
                reach: KeyReach::SelfOnly,
            },
            resources: Some(vec![KeyTargetKind::Identity(KeyIdentityKind::Key)]),
            allow: vec![KeyOperation {
                operation: "MeTokenActivityList".to_owned(),
                actions: vec![KeyAction::Read],
            }],
        }],
        ..KeyScope::default()
    }
}

#[tokio::test]
async fn a_real_credential_creation_replay_is_marked_without_retaining_either_secret_or_body() {
    let world = World::new().await;
    let ada = places::person(&world, "ada").await;
    let key = new_id();
    let issuer = credential(
        &world,
        ada.id(),
        key,
        Some(&KeyScope {
            rules: vec![KeyRule {
                target: KeyTarget {
                    kind: KeyTargetKind::Identity(KeyIdentityKind::Account),
                    id: Some(ada.id()),
                    reach: KeyReach::Subtree,
                },
                resources: Some(vec![KeyTargetKind::Identity(KeyIdentityKind::Key)]),
                allow: vec![KeyOperation {
                    operation: "MeTokenCreate".to_owned(),
                    actions: vec![KeyAction::Manage, KeyAction::Read],
                }],
            }],
            ..KeyScope::default()
        }),
    )
    .await;
    let body = json!({"name":"Do not retain this child name","rules":[]});
    let first = world
        .app
        .call(MeTokenCreate, &[])
        .token(&issuer)
        .header(IDEMPOTENCY_HEADER, "create-child-once")
        .raw_body(&body)
        .send()
        .await
        .full_token()
        .ok();
    assert!(!first.already_created);
    let child_secret = first
        .token
        .as_deref()
        .expect("the first creation shows its new secret");
    let repeated = world
        .app
        .call(MeTokenCreate, &[])
        .token(&issuer)
        .header(IDEMPOTENCY_HEADER, "create-child-once")
        .raw_body(&body)
        .send()
        .await
        .full_token()
        .ok();
    assert!(repeated.already_created);
    assert!(repeated.token.is_none());
    assert_eq!(repeated.info.id, first.info.id);
    let page = world
        .app
        .call(MeTokenActivityList, &[&key])
        .token(&ada.token)
        .query(&query(None, None))
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items.iter().filter(|call| call.replayed).count(), 1);
    assert!(
        page.items
            .iter()
            .all(|call| call.status == 200 && call.operation == "MeTokenCreate")
    );
    let wire = serde_json::to_string(&page).unwrap();
    for hidden in [
        &issuer,
        child_secret,
        "Do not retain this child name",
        "create-child-once",
        "operation_scope",
        "proof",
    ] {
        assert!(
            !wire.contains(hidden),
            "secrets, bodies and idempotency headers are not call metadata"
        );
    }
}

#[tokio::test]
async fn calls_and_refusals_have_safe_metadata_and_self_reads_do_not_extend_equal_clock_pages() {
    let world = World::new().await;
    let ada = places::person(&world, "ada").await;
    let other = places::person(&world, "other").await;
    let key = new_id();
    let secret = credential(&world, ada.id(), key, None).await;
    world.app.call(MeGet, &[]).token(&secret).send().await.ok();
    world
        .app
        .call(NodeGet, &[&ada.person.personal_org])
        .token(&secret)
        .send()
        .await
        .ok();
    let invalid = world
        .app
        .call(NodeGet, &[&ada.person.personal_org])
        .token(&secret)
        .peer("198.51.100.7:41001".parse().unwrap())
        .raw_query("inspect_deleted=invalid&private_text=never-retain-this")
        .send()
        .await
        .problem();
    assert_eq!(invalid.code, ErrorCode::RequestInvalid);
    let denied = world
        .app
        .call(NodeGet, &[&other.person.personal_org])
        .token(&secret)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    let initial = world
        .app
        .call(MeTokenActivityList, &[&key])
        .token(&ada.token)
        .query(&query(None, None))
        .send()
        .await
        .ok();
    assert_eq!(initial.items.len(), 4);
    let ordinary = initial
        .items
        .iter()
        .find(|call| call.operation == "NodeGet" && call.status == 200)
        .unwrap();
    assert!(
        ordinary
            .facets
            .iter()
            .any(|facet| facet.action == KeyAction::Read
                && facet.target
                    == KeyObjectRef {
                        kind: KeyTargetKind::Object(ObjectKind::Org),
                        id: Some(ada.person.personal_org)
                    }),
        "legacy keys also retain the shared operation's actual resolved action/target"
    );
    assert!(initial.items.iter().any(|call|call.status==400 && call.error_code.as_deref()==Some("request.invalid")));
    assert!(
        initial
            .items
            .iter()
            .any(|call| call.status == 403 && call.error_code.as_deref() == Some("authz.denied"))
    );
    assert!(
        initial
            .items
            .iter()
            .any(|call| call.ip.as_deref() == Some("198.51.100.7"))
    );
    assert!(
        initial
            .items
            .iter()
            .all(|call| !call.request_id.is_empty() && !call.replayed)
    );
    let wire = serde_json::to_string(&initial).unwrap();
    for hidden in [
        &secret,
        "never-retain-this",
        "inspect_deleted=invalid",
        "private_text",
    ] {
        assert!(
            !wire.contains(hidden),
            "neither credential nor query material is call metadata"
        );
    }
    let mut cursor = None;
    let mut ids = Vec::new();
    // Every read below appends its own event at the identical ManualClock time.
    // A watermark makes the original four-record traversal finite and exact.
    for _ in 0..initial.items.len() {
        let page = world
            .app
            .call(MeTokenActivityList, &[&key])
            .token(&secret)
            .query(&query(Some(1), cursor))
            .send()
            .await
            .ok();
        assert_eq!(page.items.len(), 1);
        assert_ne!(page.items[0].operation, "MeTokenActivityList");
        ids.push(page.items[0].id);
        cursor = page.next_cursor;
    }
    assert!(cursor.is_none());
    assert_eq!(
        ids,
        initial.items.iter().map(|call| call.id).collect::<Vec<_>>()
    );
    let now = world
        .app
        .call(MeTokenActivityList, &[&key])
        .token(&ada.token)
        .query(&query(None, None))
        .send()
        .await
        .ok();
    assert_eq!(now.items.len(), 8);
    assert_eq!(
        now.items
            .iter()
            .filter(|call| call.operation == "MeTokenActivityList")
            .count(),
        4
    );
    let unknown = world
        .app
        .call(MeGet, &[])
        .token("opat_not-a-known-key")
        .send()
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::AuthInvalidToken);
    let after = world
        .app
        .call(MeTokenActivityList, &[&key])
        .token(&ada.token)
        .query(&query(None, None))
        .send()
        .await
        .ok();
    assert_eq!(
        after.items.len(),
        8,
        "unknown credentials are never attributed to a valid key"
    );
}

#[tokio::test]
async fn reading_key_history_requires_both_current_ownership_and_its_shared_operation_scope() {
    let world = World::new().await;
    let ada = places::person(&world, "ada").await;
    let other = places::person(&world, "other").await;
    let key = new_id();
    let secret = credential(&world, ada.id(), key, Some(&read_scope(key))).await;
    world
        .app
        .call(MeTokenActivityList, &[&key])
        .token(&secret)
        .query(&query(None, None))
        .send()
        .await
        .ok();
    let second = new_id();
    credential(&world, ada.id(), second, None).await;
    let wrong_target = world
        .app
        .call(MeTokenActivityList, &[&second])
        .token(&secret)
        .query(&query(None, None))
        .send()
        .await
        .problem();
    assert_eq!(wrong_target.code, ErrorCode::AuthzDenied);
    let wrong_owner = world
        .app
        .call(MeTokenActivityList, &[&key])
        .token(&other.token)
        .query(&query(None, None))
        .send()
        .await
        .problem();
    assert_eq!(wrong_owner.code, ErrorCode::NotFound);
    let mut conn = world.conn().await;
    identity::revoke_api_token(&mut conn, key, ada.id(), world.now())
        .await
        .unwrap();
    drop(conn);
    let retained = world
        .app
        .call(MeTokenActivityList, &[&key])
        .token(&ada.token)
        .query(&query(None, None))
        .send()
        .await
        .ok();
    assert_eq!(
        retained.items.len(),
        2,
        "revocation retains only the actual completed calls"
    );
    let refused = retained
        .items
        .iter()
        .find(|call| call.status == 403)
        .unwrap();
    assert_eq!(refused.selection.as_deref(), Some("MeTokenActivityList"));
    assert_eq!(
        refused.facets[0].target.id,
        Some(second),
        "the attempted resolved target is retained on scope denial"
    );
    let invalid = world
        .app
        .call(MeTokenActivityList, &[&key])
        .token(&secret)
        .query(&query(None, None))
        .send()
        .await
        .problem();
    assert_eq!(invalid.code, ErrorCode::AuthInvalidToken);
    world.advance(Duration::days(8));
    let expired = world
        .app
        .call(MeTokenActivityList, &[&key])
        .token(&ada.token)
        .query(&query(None, None))
        .send()
        .await
        .ok();
    assert!(expired.items.is_empty());
}

#[tokio::test]
async fn org_history_projects_only_its_facets_with_current_names_and_separate_ip_authority() {
    let desk = desk::Desk::new().await;
    let manager = desk.member("manager", "viewer").await;
    let outsider = desk.stranger("outsider").await;
    desk.world.app.call(PermissionPageUpdate,&[&"nodes",&desk.org]).token(&desk.ada.token)
        .raw_body(&json!({"rows":[{"permission":"members.manage","add":[{"holder":{"kind":"user","id":manager.id()}}]}]}))
        .send().await.ok();
    let document = desk.make(&desk.ada, "Private local title").await.base.id;
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &document])
        .token(&desk.ada.token)
        .raw_body(&json!({"whole":"private"}))
        .send()
        .await
        .ok();
    let key = new_id();
    credential(&desk.world, desk.ada.id(), key, None).await;
    let foreign = outsider.person.personal_org;
    let facets = json!([
        {"operation":"DocumentTreeOperate.copy","action":"write","resource_kind":"document","target":{"kind":"document","id":document},"organizations":[desk.org]},
        {"operation":"DocumentTreeOperate.copy","action":"read","resource_kind":"org","target":{"kind":"org","id":foreign},"organizations":[foreign]}
    ]);
    let id = new_id();
    let mut conn = desk.world.conn().await;
    assert!(
        key_activity::append(
            &mut conn,
            &key_activity::NewCall {
                id,
                token: key,
                account: desk.ada.id(),
                at: desk.world.now(),
                operation: "DocumentTreeOperate",
                selection: Some("DocumentTreeOperate.copy"),
                facets: &facets,
                orgs: &[desk.org, foreign],
                method: "POST",
                route: "/document-tree/operations",
                status: 200,
                error: None,
                replayed: false,
                duration_ms: 7,
                request_id: "compound-call",
                ip: Some("198.51.100.7"),
            },
            desk.world.now()
        )
        .await
        .unwrap()
    );
    drop(conn);
    let page = desk
        .world
        .app
        .call(OrgTokenActivityList, &[&desk.org, &key])
        .token(&manager.token)
        .query(&query(None, None))
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].facets.len(), 1);
    assert_eq!(
        page.items[0].facets[0].target,
        KeyObjectRef {
            kind: KeyTargetKind::Object(ObjectKind::Document),
            id: Some(document)
        }
    );
    assert_eq!(
        page.items[0].facets[0].target_name, None,
        "managing members does not disclose private document names"
    );
    assert_eq!(
        page.items[0].ip, None,
        "members management alone is not Audit View"
    );
    let wire = serde_json::to_string(&page).unwrap();
    assert!(!wire.contains(&foreign.to_string()));
    assert!(!wire.contains("Private local title"));
    let scoped_id = new_id();
    let scoped = credential(
        &desk.world,
        manager.id(),
        scoped_id,
        Some(&KeyScope {
            rules: vec![KeyRule {
                target: KeyTarget {
                    kind: KeyTargetKind::Object(ObjectKind::Org),
                    id: Some(desk.org),
                    reach: KeyReach::SelfOnly,
                },
                resources: Some(vec![KeyTargetKind::Identity(KeyIdentityKind::Key)]),
                allow: vec![KeyOperation {
                    operation: "OrgTokenActivityList".to_owned(),
                    actions: vec![KeyAction::Read],
                }],
            }],
            ..KeyScope::default()
        }),
    )
    .await;
    let scoped_page = desk
        .world
        .app
        .call(OrgTokenActivityList, &[&desk.org, &key])
        .token(&scoped)
        .query(&query(None, None))
        .send()
        .await
        .ok();
    assert_eq!(
        scoped_page.items, page.items,
        "the same org/key read uses the shared operation target and projection"
    );
    let wrong_scope = desk
        .world
        .app
        .call(OrgTokenActivityList, &[&manager.person.personal_org, &key])
        .token(&scoped)
        .query(&query(None, None))
        .send()
        .await
        .problem();
    assert_eq!(
        wrong_scope.code,
        ErrorCode::AuthzDenied,
        "ordinary authority in another org does not widen the selected key scope"
    );
    let denied = desk
        .world
        .app
        .call(OrgTokenActivityList, &[&desk.org, &key])
        .token(&outsider.token)
        .query(&query(None, None))
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    let changed=desk.world.app.call(PermissionPageUpdate,&[&"documents",&document]).token(&desk.ada.token)
        .raw_body(&json!({"rows":[{"permission":"doc.view","add":[{"holder":{"kind":"user","id":manager.id()}}]}]}))
        .send().await.ok();
    let entry = changed
        .rows
        .iter()
        .find(|row| row.permission == "doc.view")
        .unwrap()
        .entries
        .iter()
        .find(|entry| matches!(&entry.holder,Holder::User{user} if user.id==manager.id()))
        .unwrap()
        .id;
    let named = desk
        .world
        .app
        .call(OrgTokenActivityList, &[&desk.org, &key])
        .token(&manager.token)
        .query(&query(None, None))
        .send()
        .await
        .ok();
    assert_eq!(
        named.items[0].facets[0].target_name.as_deref(),
        Some("Private local title")
    );
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &document])
        .token(&desk.ada.token)
        .raw_body(&json!({"rows":[{"permission":"doc.view","remove":[entry]}]}))
        .send()
        .await
        .ok();
    let hidden = desk
        .world
        .app
        .call(OrgTokenActivityList, &[&desk.org, &key])
        .token(&manager.token)
        .query(&query(None, None))
        .send()
        .await
        .ok();
    assert_eq!(
        hidden.items[0].facets[0].target_name, None,
        "historical names do not survive current access removal"
    );
    let owner: Page<_> = desk
        .world
        .app
        .call(OrgTokenActivityList, &[&desk.org, &key])
        .token(&desk.ada.token)
        .query(&query(None, None))
        .send()
        .await
        .ok();
    assert_eq!(owner.items[0].ip.as_deref(), Some("198.51.100.7"));
    assert_eq!(owner.items[0].facets.len(), 1);
}
