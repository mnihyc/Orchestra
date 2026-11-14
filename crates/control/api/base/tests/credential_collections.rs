//! Account collections and exact credentials keep separate scope and ownership boundaries.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use chrono::Duration;
use http::StatusCode;
use orch_access_contract::key_scope::{KeyRule, KeyScope};
use orch_api_contract::auth::{
    ApiTokenCreateBody, MeSessionList, MeTokenCreate, MeTokenDelete, MeTokenList,
    TokenCreateResponse,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{
    OrgServiceAccountCreate, OrgServiceAccountCreateBody, ServiceAccountTokenDelete,
    ServiceAccountTokenList,
};
use orch_api_contract::nodes::NodeKind;
use orch_api_contract::users::UserGet;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::{identity, key_activity};
use orch_types::{Id, Time, new_id};
use places::Who;
use serde_json::json;
use support::World;

#[tokio::test]
async fn account_credentials_are_searched_before_pages_and_every_continuation_is_available() {
    use orch_api_contract::common::CollectionQuery;
    let world = World::new().await;
    let owner = places::person(&world, "page-owner").await;
    for index in 0..13 {
        legacy(
            &world,
            owner.id(),
            &format!("Page key {index:02}"),
            None,
            false,
        )
        .await;
        session(
            &world,
            owner.id(),
            &format!("Page browser {index:02}"),
            false,
        )
        .await;
    }
    let last = world
        .app
        .call(MeTokenList, &[])
        .token(&owner.token)
        .query(&CollectionQuery {
            q: Some("Page key 12".to_owned()),
            limit: Some(3),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(last.items.len(), 1);
    assert_eq!(last.items[0].name, "Page key 12");
    let browser = world
        .app
        .call(MeSessionList, &[])
        .token(&owner.token)
        .query(&CollectionQuery {
            q: Some("Page browser 12".to_owned()),
            limit: Some(3),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(browser.items.len(), 1);
    let mut cursor = None;
    let mut names = std::collections::HashSet::new();
    loop {
        let page = world
            .app
            .call(MeTokenList, &[])
            .token(&owner.token)
            .query(&CollectionQuery {
                q: Some("Page key".to_owned()),
                limit: Some(3),
                cursor,
            })
            .send()
            .await
            .ok();
        for row in page.items {
            assert!(names.insert(row.name));
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(names.len(), 13);
}

struct Key {
    id: Id,
    secret: String,
}

fn rule(
    kind: &str,
    id: Id,
    reach: &str,
    family: &str,
    operation: &str,
    actions: &[&str],
) -> KeyRule {
    serde_json::from_value(json!({
        "target":{"kind":kind,"id":id,"reach":reach},
        "resources":[family],"allow":[{"operation":operation,"actions":actions}]
    }))
    .unwrap()
}

fn scope(rules: Vec<KeyRule>) -> KeyScope {
    let result = KeyScope {
        rules,
        ..Default::default()
    };
    assert!(result.validate().is_empty());
    result
}

async fn stored_key(
    world: &World,
    owner: Id,
    name: &str,
    policy: Option<KeyScope>,
    node: Option<Id>,
    expired: bool,
) -> Key {
    assert!(
        policy.is_none() || node.is_none(),
        "fixture modes do not mix"
    );
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let raw = policy.map(|value| serde_json::to_value(value).unwrap());
    let scopes = if raw.is_some() {
        Vec::new()
    } else {
        vec!["*".to_owned()]
    };
    // Active fixtures fit existing place ceilings. Expired subjects remain stored and revocable.
    let (at, end) = if expired {
        (
            world.now() - Duration::hours(2),
            world.now() - Duration::minutes(1),
        )
    } else {
        (world.now(), world.now() + Duration::hours(1))
    };
    let mut conn = world.conn().await;
    identity::create_api_token_scoped(
        &mut conn,
        &identity::NewApiToken {
            id,
            user_id: owner,
            name,
            token_hash: &hash,
            scopes: &scopes,
            node_id: node,
            at,
            expires_at: Some(end),
        },
        raw.as_ref(),
    )
    .await
    .unwrap();
    Key { id, secret }
}

async fn scoped(world: &World, owner: Id, rules: Vec<KeyRule>) -> Key {
    stored_key(
        world,
        owner,
        "Scoped caller",
        Some(scope(rules)),
        None,
        false,
    )
    .await
}

async fn legacy(world: &World, owner: Id, name: &str, node: Option<Id>, expired: bool) -> Key {
    stored_key(world, owner, name, None, node, expired).await
}

async fn session(world: &World, owner: Id, name: &str, expired: bool) -> Id {
    let id = new_id();
    let refresh = token_hash(&new_id().to_string());
    let (at, expires) = if expired {
        (
            world.now() - Duration::hours(2),
            world.now() - Duration::minutes(1),
        )
    } else {
        (world.now(), world.now() + Duration::hours(1))
    };
    let mut conn = world.conn().await;
    identity::create_session(
        &mut conn,
        &identity::NewSession {
            id,
            user_id: owner,
            refresh_hash: &refresh,
            user_agent: name,
            ip: Some("192.0.2.12".parse().unwrap()),
            at,
            expires_at: expires,
        },
    )
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn personal_key_collection_composes_root_rows_and_protected_names_without_merging_rules() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let foreign = places::person(&world, "foreign").await;
    let org = places::make_org(&world, &owner, "Office", "office")
        .await
        .base
        .id;
    let project = places::make_project(&world, &owner, org, "Private project label")
        .await
        .base
        .id;
    let a = legacy(&world, owner.id(), "Credential A", Some(project), false).await;
    let b = legacy(&world, owner.id(), "Expired credential", None, true).await;
    let removed = legacy(&world, owner.id(), "Revoked credential", None, false).await;
    let foreign_key = legacy(&world, foreign.id(), "Foreign credential", None, false).await;
    {
        let mut conn = world.conn().await;
        assert!(
            identity::revoke_api_token(&mut conn, removed.id, owner.id(), world.now())
                .await
                .unwrap()
        );
    }
    let root = rule(
        "account",
        owner.id(),
        "self",
        "key",
        "MeTokenList",
        &["read"],
    );
    let root_only = scoped(&world, owner.id(), vec![root.clone()]).await;
    assert!(
        world
            .app
            .call(MeTokenList, &[])
            .token(&root_only.secret)
            .send()
            .await
            .ok()
            .items
            .is_empty()
    );
    let exact_only = scoped(
        &world,
        owner.id(),
        vec![rule("key", a.id, "self", "key", "MeTokenList", &["read"])],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(MeTokenList, &[])
            .token(&exact_only.secret)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN,
        "an exact row alone does not open its account collection"
    );
    let composed_rules = vec![
        root.clone(),
        rule("key", a.id, "self", "key", "MeTokenList", &["read"]),
        rule("key", a.id, "self", "key", "MeTokenDelete", &["read"]),
        rule("key", b.id, "self", "key", "MeTokenDelete", &["manage"]),
    ];
    let composed = scoped(&world, owner.id(), composed_rules.clone()).await;
    let rows = world
        .app
        .call(MeTokenList, &[])
        .token(&composed.secret)
        .send()
        .await
        .ok()
        .items;
    assert_eq!(
        rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![a.id]
    );
    let node = rows[0].node.as_ref().unwrap();
    assert_eq!(node.id, project);
    assert_eq!(node.kind, NodeKind::Project);
    assert!(
        node.name.is_none() && node.redacted,
        "the key row does not grant a protected node label"
    );
    assert_eq!(
        world
            .app
            .call(MeTokenDelete, &[&a.id])
            .token(&composed.secret)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN,
        "Read on A and Manage on B never become Manage on A"
    );

    let mut named_rules = composed_rules;
    named_rules.push(rule(
        "project",
        project,
        "self",
        "project",
        "MeTokenList",
        &["read"],
    ));
    let named = scoped(&world, owner.id(), named_rules).await;
    let rows = world
        .app
        .call(MeTokenList, &[])
        .token(&named.secret)
        .send()
        .await
        .ok()
        .items;
    assert_eq!(
        rows[0].node.as_ref().unwrap().name.as_deref(),
        Some("Private project label")
    );
    assert!(!rows[0].node.as_ref().unwrap().redacted);
    let not_ancestry = scoped(
        &world,
        owner.id(),
        vec![
            root,
            rule(
                "project",
                project,
                "subtree",
                "key",
                "MeTokenList",
                &["read"],
            ),
        ],
    )
    .await;
    assert!(
        world
            .app
            .call(MeTokenList, &[])
            .token(&not_ancestry.secret)
            .send()
            .await
            .ok()
            .items
            .is_empty(),
        "a legacy key restriction is not placement ancestry of that credential"
    );

    let all = scoped(
        &world,
        owner.id(),
        vec![rule(
            "account",
            owner.id(),
            "subtree",
            "key",
            "MeTokenList",
            &["read"],
        )],
    )
    .await;
    let rows = world
        .app
        .call(MeTokenList, &[])
        .token(&all.secret)
        .send()
        .await
        .ok()
        .items;
    let expected = {
        let mut conn = world.conn().await;
        identity::api_tokens_of(&mut conn, owner.id())
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        expected,
        "normal ordering is preserved"
    );
    assert!(
        rows.iter().any(|row| row.id == b.id),
        "expired, unrevoked keys stay listed"
    );
    assert!(
        !rows
            .iter()
            .any(|row| row.id == removed.id || row.id == foreign_key.id)
    );
    let browser = world
        .app
        .call(MeTokenList, &[])
        .token(&owner.token)
        .send()
        .await
        .ok()
        .items;
    assert_eq!(
        browser
            .iter()
            .find(|row| row.id == a.id)
            .unwrap()
            .node
            .as_ref()
            .unwrap()
            .name
            .as_deref(),
        Some("Private project label")
    );
    let old = legacy(&world, owner.id(), "Legacy caller", None, false).await;
    let legacy_rows = world
        .app
        .call(MeTokenList, &[])
        .token(&old.secret)
        .send()
        .await
        .ok()
        .items;
    assert_eq!(
        legacy_rows
            .iter()
            .find(|row| row.id == a.id)
            .unwrap()
            .node
            .as_ref()
            .unwrap()
            .name
            .as_deref(),
        Some("Private project label")
    );
}

#[tokio::test]
async fn session_collection_uses_actual_owners_and_live_rows_without_inventing_a_current_session() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let foreign = places::person(&world, "foreign").await;
    let a = session(&world, owner.id(), "Browser A", false).await;
    let b = session(&world, owner.id(), "Browser B", false).await;
    let expired = session(&world, owner.id(), "Expired browser", true).await;
    let ended = session(&world, owner.id(), "Ended browser", false).await;
    let outside = session(&world, foreign.id(), "Foreign browser", false).await;
    {
        let mut conn = world.conn().await;
        assert!(
            identity::revoke_session(&mut conn, ended, owner.id(), world.now())
                .await
                .unwrap()
        );
    }
    let root = rule(
        "account",
        owner.id(),
        "self",
        "session",
        "MeSessionList",
        &["read"],
    );
    let root_only = scoped(&world, owner.id(), vec![root.clone()]).await;
    assert!(
        world
            .app
            .call(MeSessionList, &[])
            .token(&root_only.secret)
            .send()
            .await
            .ok()
            .items
            .is_empty()
    );
    let exact_only = scoped(
        &world,
        owner.id(),
        vec![rule(
            "session",
            a,
            "self",
            "session",
            "MeSessionList",
            &["read"],
        )],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(MeSessionList, &[])
            .token(&exact_only.secret)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let one = scoped(
        &world,
        owner.id(),
        vec![
            root.clone(),
            rule("session", a, "self", "session", "MeSessionList", &["read"]),
        ],
    )
    .await;
    let rows = world
        .app
        .call(MeSessionList, &[])
        .token(&one.secret)
        .send()
        .await
        .ok()
        .items;
    assert_eq!(rows.iter().map(|row| row.id).collect::<Vec<_>>(), vec![a]);
    assert_eq!(rows[0].user_agent, "Browser A");
    assert_eq!(rows[0].ip.as_deref(), Some("192.0.2.12"));
    assert!(!rows[0].current, "an API key is not any browser session");
    let wrong_owner = scoped(
        &world,
        owner.id(),
        vec![
            root,
            rule(
                "account",
                foreign.id(),
                "subtree",
                "session",
                "MeSessionList",
                &["read"],
            ),
        ],
    )
    .await;
    assert!(
        world
            .app
            .call(MeSessionList, &[])
            .token(&wrong_owner.secret)
            .send()
            .await
            .ok()
            .items
            .is_empty()
    );
    let all = scoped(
        &world,
        owner.id(),
        vec![rule(
            "account",
            owner.id(),
            "subtree",
            "session",
            "MeSessionList",
            &["read"],
        )],
    )
    .await;
    let rows = world
        .app
        .call(MeSessionList, &[])
        .token(&all.secret)
        .send()
        .await
        .ok()
        .items;
    let expected = {
        let mut conn = world.conn().await;
        identity::live_sessions(&mut conn, owner.id(), world.now())
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(rows.iter().map(|row| row.id).collect::<Vec<_>>(), expected);
    assert!(rows.iter().any(|row| row.id == a) && rows.iter().any(|row| row.id == b));
    assert!(rows.iter().all(|row| !row.current));
    assert!(
        !rows
            .iter()
            .any(|row| [expired, ended, outside].contains(&row.id))
    );
    let browser = world
        .app
        .call(MeSessionList, &[])
        .token(&owner.token)
        .send()
        .await
        .ok()
        .items;
    assert_eq!(browser.iter().filter(|row| row.current).count(), 1);
    let old = legacy(&world, owner.id(), "Legacy session reader", None, false).await;
    let legacy_rows = world
        .app
        .call(MeSessionList, &[])
        .token(&old.secret)
        .send()
        .await
        .ok()
        .items;
    assert_eq!(
        legacy_rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        expected
    );
    assert!(legacy_rows.iter().all(|row| !row.current));
}

#[tokio::test]
async fn personal_revoke_is_exact_keeps_expired_subjects_and_never_revokes_child_keys() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let foreign = places::person(&world, "foreign").await;
    let a = legacy(&world, owner.id(), "A", None, false).await;
    let expired = legacy(&world, owner.id(), "Expired", None, true).await;
    let outside = legacy(&world, foreign.id(), "Foreign", None, false).await;
    let exact = scoped(
        &world,
        owner.id(),
        vec![
            rule("key", a.id, "self", "key", "MeTokenDelete", &["read"]),
            rule(
                "key",
                expired.id,
                "self",
                "key",
                "MeTokenDelete",
                &["manage"],
            ),
            rule(
                "key",
                outside.id,
                "self",
                "key",
                "MeTokenDelete",
                &["manage"],
            ),
        ],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(MeTokenDelete, &[&a.id])
            .token(&exact.secret)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN,
        "a different target's Manage cannot be borrowed"
    );
    let gone = world
        .app
        .call(MeTokenDelete, &[&expired.id])
        .token(&exact.secret)
        .send()
        .await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT);
    assert!(gone.raw.is_empty());
    assert_eq!(
        world
            .app
            .call(MeTokenDelete, &[&expired.id])
            .token(&exact.secret)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND,
        "an already-revoked credential stays absent"
    );
    assert_eq!(
        world
            .app
            .call(MeTokenDelete, &[&outside.id])
            .token(&exact.secret)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND,
        "scope never substitutes for actual ownership"
    );
    let wrong_account = scoped(
        &world,
        owner.id(),
        vec![rule(
            "account",
            foreign.id(),
            "subtree",
            "key",
            "MeTokenDelete",
            &["manage"],
        )],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(MeTokenDelete, &[&a.id])
            .token(&wrong_account.secret)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );

    let parent_scope = scope(vec![
        rule(
            "account",
            owner.id(),
            "subtree",
            "key",
            "MeTokenDelete",
            &["manage"],
        ),
        rule(
            "account",
            owner.id(),
            "self",
            "key",
            "MeTokenCreate",
            &["manage"],
        ),
        rule(
            "account",
            owner.id(),
            "self",
            "account",
            "UserGet",
            &["read"],
        ),
    ]);
    let parent = stored_key(
        &world,
        owner.id(),
        "Parent",
        Some(parent_scope),
        None,
        false,
    )
    .await;
    // Use real issuance so the audit records the issuer; revocation must not turn
    // that provenance into a live parent-child credential relationship.
    let created = world
        .app
        .call(MeTokenCreate, &[])
        .token(&parent.secret)
        .body(&ApiTokenCreateBody {
            name: "Child".into(),
            rules: Some(vec![rule(
                "account",
                owner.id(),
                "self",
                "account",
                "UserGet",
                &["read"],
            )]),
            scopes: None,
            node_id: None,
            expires_at: Some(Time::new(world.now() + Duration::minutes(30))),
            proof: None,
        })
        .send()
        .await
        .ok();
    let TokenCreateResponse::Receipt(created) = created else {
        panic!("the Manage-only issuer receives the first-secret receipt")
    };
    let child = Key {
        id: created.token_id,
        secret: created
            .token
            .expect("first issuance contains its new secret"),
    };
    let revoked = world
        .app
        .call(MeTokenDelete, &[&parent.id])
        .token(&parent.secret)
        .send()
        .await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    assert!(revoked.raw.is_empty());
    assert_eq!(
        world
            .app
            .call(UserGet, &[&owner.id()])
            .token(&parent.secret)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthInvalidToken
    );
    assert_eq!(
        world
            .app
            .call(UserGet, &[&owner.id()])
            .token(&child.secret)
            .send()
            .await
            .ok()
            .id,
        owner.id()
    );
    let mut conn = world.conn().await;
    assert!(
        identity::api_token_listed(&mut conn, a.id, owner.id())
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        identity::api_token_listed(&mut conn, outside.id, foreign.id())
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        identity::api_token_listed(&mut conn, child.id, owner.id())
            .await
            .unwrap()
            .is_some()
    );
}

async fn service(world: &World, owner: &Who, org: Id, name: &str) -> Id {
    world
        .app
        .call(OrgServiceAccountCreate, &[&org])
        .token(&owner.token)
        .body(&OrgServiceAccountCreateBody {
            username: name.to_owned(),
            display_name: name.to_owned(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response")
        .user
        .id
}

#[tokio::test]
async fn service_collections_and_revokes_keep_member_checks_and_real_credential_owners() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let viewer = places::person(&world, "viewer").await;
    let org = places::make_org(&world, &owner, "Private service org", "service-org")
        .await
        .base
        .id;
    places::join(&world, org, org, &viewer, "viewer", &owner).await;
    let project = places::make_project(&world, &owner, org, "Protected service scope")
        .await
        .base
        .id;
    let account = service(&world, &owner, org, "service-a").await;
    let other = service(&world, &owner, org, "service-b").await;
    let a = legacy(&world, account, "Service A", Some(project), false).await;
    let expired = legacy(&world, account, "Service expired", None, true).await;
    let outside = legacy(&world, other, "Other service", None, false).await;
    let root = rule(
        "account",
        account,
        "self",
        "key",
        "ServiceAccountTokenList",
        &["read"],
    );
    let root_only = scoped(&world, owner.id(), vec![root.clone()]).await;
    assert!(
        world
            .app
            .call(ServiceAccountTokenList, &[&account])
            .token(&root_only.secret)
            .send()
            .await
            .ok()
            .items
            .is_empty()
    );
    let one = scoped(
        &world,
        owner.id(),
        vec![
            root,
            rule(
                "key",
                a.id,
                "self",
                "key",
                "ServiceAccountTokenList",
                &["read"],
            ),
            rule(
                "key",
                outside.id,
                "self",
                "key",
                "ServiceAccountTokenList",
                &["read"],
            ),
        ],
    )
    .await;
    let rows = world
        .app
        .call(ServiceAccountTokenList, &[&account])
        .token(&one.secret)
        .send()
        .await
        .ok()
        .items;
    assert_eq!(
        rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![a.id]
    );
    assert!(
        rows[0].node.as_ref().unwrap().name.is_none() && rows[0].node.as_ref().unwrap().redacted
    );
    assert_eq!(
        world
            .app
            .call(ServiceAccountTokenList, &[&other])
            .token(&one.secret)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN,
        "an exact row cannot replace the other account's collection grant"
    );
    let all_rules = vec![
        rule(
            "account",
            account,
            "subtree",
            "key",
            "ServiceAccountTokenList",
            &["read"],
        ),
        rule(
            "account",
            account,
            "subtree",
            "key",
            "ServiceAccountTokenDelete",
            &["manage"],
        ),
    ];
    let all = scoped(&world, owner.id(), all_rules.clone()).await;
    let rows = world
        .app
        .call(ServiceAccountTokenList, &[&account])
        .token(&all.secret)
        .send()
        .await
        .ok()
        .items;
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row.id == a.id) && rows.iter().any(|row| row.id == expired.id));
    let old = legacy(&world, owner.id(), "Legacy service manager", None, false).await;
    let legacy_rows = world
        .app
        .call(ServiceAccountTokenList, &[&account])
        .token(&old.secret)
        .send()
        .await
        .ok()
        .items;
    assert_eq!(legacy_rows.len(), 2);
    assert_eq!(
        legacy_rows
            .iter()
            .find(|row| row.id == a.id)
            .unwrap()
            .node
            .as_ref()
            .unwrap()
            .name
            .as_deref(),
        Some("Protected service scope")
    );
    let powerless = scoped(&world, viewer.id(), all_rules).await;
    let denied = world
        .app
        .call(ServiceAccountTokenList, &[&account])
        .token(&powerless.secret)
        .send()
        .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&denied.raw).contains("Private service org"));
    assert!(!String::from_utf8_lossy(&denied.raw).contains("Service expired"));
    {
        let mut conn = world.conn().await;
        let watermark = key_activity::snapshot(&mut conn, powerless.id)
            .await
            .unwrap();
        let calls = key_activity::page(
            &mut conn,
            powerless.id,
            None,
            world.now(),
            watermark,
            None,
            key_activity::KEPT_CALLS,
        )
        .await
        .unwrap();
        let refusal = calls
            .iter()
            .find(|call| call.operation == "ServiceAccountTokenList" && call.status == 403)
            .expect("the actual denial is recorded");
        let logged = refusal.facets.to_string();
        assert!(
            !logged.contains(&a.id.to_string()) && !logged.contains(&expired.id.to_string()),
            "a failed member check must not record undisclosed private child IDs"
        );
    }
    assert_eq!(
        world
            .app
            .call(ServiceAccountTokenDelete, &[&account, &a.id])
            .token(&powerless.secret)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let wrong_owner = scoped(
        &world,
        owner.id(),
        vec![rule(
            "account",
            other,
            "subtree",
            "key",
            "ServiceAccountTokenDelete",
            &["manage"],
        )],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(ServiceAccountTokenDelete, &[&account, &a.id])
            .token(&wrong_owner.secret)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let exact = scoped(
        &world,
        owner.id(),
        vec![rule(
            "key",
            a.id,
            "self",
            "key",
            "ServiceAccountTokenDelete",
            &["manage"],
        )],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(ServiceAccountTokenDelete, &[&other, &a.id])
            .token(&exact.secret)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND,
        "the stored owner must match the path account"
    );
    let gone = world
        .app
        .call(ServiceAccountTokenDelete, &[&account, &a.id])
        .token(&exact.secret)
        .send()
        .await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT);
    assert!(gone.raw.is_empty());
    let gone = world
        .app
        .call(ServiceAccountTokenDelete, &[&account, &expired.id])
        .token(&all.secret)
        .send()
        .await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT);
    assert!(gone.raw.is_empty());
    let legacy_target = legacy(&world, account, "Legacy revoke target", None, false).await;
    let legacy_gone = world
        .app
        .call(ServiceAccountTokenDelete, &[&account, &legacy_target.id])
        .token(&old.secret)
        .send()
        .await;
    assert_eq!(legacy_gone.status, StatusCode::NO_CONTENT);
    assert!(legacy_gone.raw.is_empty());
    {
        let mut conn = world.conn().await;
        assert!(
            identity::api_token_listed(&mut conn, outside.id, other)
                .await
                .unwrap()
                .is_some()
        );
        identity::disable_service_account(&mut conn, account, owner.id(), world.now())
            .await
            .unwrap();
    }
    assert_eq!(
        world
            .app
            .call(ServiceAccountTokenList, &[&account])
            .token(&all.secret)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND,
        "disabled service subjects retain their ordinary lifecycle refusal"
    );
}
