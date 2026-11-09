//! Service-key creation binds the real Account parent and reuses issuer attenuation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use chrono::{DateTime, Duration, Utc};
use http::StatusCode;
use orch_access_contract::key_scope::{KeyRule, KeyScope};
use orch_api_contract::auth::{ApiTokenCreateBody, SensitiveProof, TokenCreateResponse};
use orch_api_contract::common::{ErrorCode, IDEMPOTENCY_HEADER};
use orch_api_contract::directory::{
    OrgServiceAccountCreate, OrgServiceAccountCreateBody, ServiceAccountTokenCreate,
};
use orch_api_contract::users::UserGet;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity;
use orch_types::{Id, Time, new_id};
use places::Who;
use serde_json::json;
use support::World;

struct Setup {
    world: World,
    boss: Who,
    viewer: Who,
    service: Id,
    other: Id,
}

impl Setup {
    async fn new() -> Self {
        let world = World::new().await;
        let boss = places::person(&world, "boss").await;
        let viewer = places::person(&world, "viewer").await;
        let org = places::make_org(&world, &boss, "Private service org", "service-org")
            .await
            .base
            .id;
        places::join(&world, org, org, &viewer, "viewer", &boss).await;
        let service = make_service(&world, &boss, org, "build-a").await;
        let other = make_service(&world, &boss, org, "build-b").await;
        Self {
            world,
            boss,
            viewer,
            service,
            other,
        }
    }
}

async fn make_service(world: &World, by: &Who, org: Id, name: &str) -> Id {
    world
        .app
        .call(OrgServiceAccountCreate, &[&org])
        .token(&by.token)
        .body(&OrgServiceAccountCreateBody {
            username: name.into(),
            display_name: name.into(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response")
        .user
        .id
}

fn rule(account: Id, family: &str, operation: &str, actions: &[&str]) -> KeyRule {
    serde_json::from_value(json!({
        "target":{"kind":"account","id":account,"reach":"self"},
        "resources":[family],"allow":[{"operation":operation,"actions":actions}]
    }))
    .unwrap()
}

struct Issuer {
    id: Id,
    secret: String,
    scope: KeyScope,
    expires: DateTime<Utc>,
}

async fn issuer(world: &World, user: Id, rules: Vec<KeyRule>) -> Issuer {
    let scope = KeyScope {
        rules,
        ..Default::default()
    };
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let raw = serde_json::to_value(&scope).unwrap();
    let id = new_id();
    // Bound the fixture to one hour; requested child lifetimes stay strictly inside it.
    let expires = world.now() + Duration::hours(1);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Service issuer",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(expires),
        },
        Some(&raw),
    )
    .await
    .unwrap();
    Issuer {
        id,
        secret,
        scope,
        expires,
    }
}

fn asked(world: &World, rules: Vec<KeyRule>) -> ApiTokenCreateBody {
    ApiTokenCreateBody {
        name: "Service child".into(),
        rules: Some(rules),
        scopes: None,
        node_id: None,
        expires_at: Some(Time::new(world.now() + Duration::minutes(30))),
        proof: None,
    }
}

#[test]
fn service_issuance_declares_key_actions_at_the_account_parent() {
    use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
    use orch_api_contract::operations::{FacetUsage, TargetResolver, for_route};
    use orch_api_contract::routes::Endpoint;
    let op = for_route(&ServiceAccountTokenCreate::ROUTE).unwrap();
    assert!(op.receipt);
    assert_eq!(op.variants.len(), 1);
    let variant = &op.variants[0];
    assert!(!variant.legacy_tokens);
    assert_eq!(variant.facets.len(), 2);
    assert_eq!(variant.facets[0].action, KeyAction::Manage);
    assert_eq!(variant.facets[1].action, KeyAction::Read);
    assert_eq!(variant.facets[1].usage, FacetUsage::ResponseContent);
    for facet in &variant.facets {
        assert_eq!(
            facet.resources,
            [KeyTargetKind::Identity(KeyIdentityKind::Key)]
        );
        assert_eq!(
            facet.target,
            TargetResolver::TypedPath {
                parameter: "id",
                kind: KeyTargetKind::Identity(KeyIdentityKind::Account),
            }
        );
    }
}

#[tokio::test]
async fn an_admitted_key_issues_for_the_service_with_a_receipt_and_an_inherited_ceiling() {
    let s = Setup::new().await;
    let parent_rules = vec![
        rule(s.service, "key", "ServiceAccountTokenCreate", &["manage"]),
        rule(s.service, "account", "UserGet", &["read"]),
    ];
    let parent = issuer(&s.world, s.boss.id(), parent_rules.clone()).await;
    let requested = vec![
        rule(s.service, "account", "UserGet", &["read"]),
        rule(s.other, "account", "UserGet", &["read"]),
    ];
    let body = asked(&s.world, requested.clone());
    let response = s
        .world
        .app
        .call(ServiceAccountTokenCreate, &[&s.service])
        .token(&parent.secret)
        .header(IDEMPOTENCY_HEADER, "service-create-once")
        .body(&body)
        .send()
        .await;
    assert_eq!(response.status, StatusCode::OK);
    let raw: serde_json::Value = serde_json::from_slice(&response.raw).unwrap();
    assert_eq!(raw["projection"], "receipt");
    assert!(raw.get("info").is_none());
    let TokenCreateResponse::Receipt(created) = response.ok() else {
        panic!("Manage-only issuance returns its receipt")
    };
    let secret = created.token.unwrap();
    assert!(!created.already_created);
    let row = identity::api_token_by_hash(
        &mut *s.world.conn().await,
        &token_hash(&secret),
        s.world.now(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        row.user_id, s.service,
        "the created key authenticates the service, not its manager"
    );
    let scope: KeyScope = serde_json::from_value(row.operation_scope.unwrap()).unwrap();
    assert_eq!(scope.rules, requested);
    assert!(
        scope
            .constraints
            .iter()
            .any(|layer| layer == &parent.scope.rules)
    );
    assert_eq!(
        s.world
            .app
            .call(UserGet, &[&s.service])
            .token(&secret)
            .send()
            .await
            .ok()
            .id,
        s.service
    );
    assert_eq!(
        s.world
            .app
            .call(UserGet, &[&s.other])
            .token(&secret)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN,
        "requested rules cannot escape the issuer's recorded ceiling"
    );
    let replay = s
        .world
        .app
        .call(ServiceAccountTokenCreate, &[&s.service])
        .token(&parent.secret)
        .header(IDEMPOTENCY_HEADER, "service-create-once")
        .body(&body)
        .send()
        .await
        .ok();
    let TokenCreateResponse::Receipt(replay) = replay else {
        panic!("replay remains a receipt")
    };
    assert_eq!(replay.token_id, created.token_id);
    assert!(replay.already_created && replay.token.is_none());
    assert_eq!(
        identity::api_tokens_of(&mut *s.world.conn().await, s.service)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        s.world
            .app
            .call(ServiceAccountTokenCreate, &[&s.other])
            .token(&parent.secret)
            .body(&body)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );

    let mut broad_rules = parent_rules;
    broad_rules.push(rule(
        s.other,
        "key",
        "ServiceAccountTokenCreate",
        &["manage"],
    ));
    let broader = issuer(&s.world, s.boss.id(), broad_rules).await;
    let mismatch = s
        .world
        .app
        .call(ServiceAccountTokenCreate, &[&s.other])
        .token(&broader.secret)
        .header(IDEMPOTENCY_HEADER, "service-create-once")
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(mismatch.code, ErrorCode::ConflictIdempotencyMismatch);
    assert!(
        identity::api_tokens_of(&mut *s.world.conn().await, s.other)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        identity::revoke_api_token(
            &mut *s.world.conn().await,
            parent.id,
            s.boss.id(),
            s.world.now()
        )
        .await
        .unwrap()
    );
    assert_eq!(
        s.world
            .app
            .call(UserGet, &[&s.service])
            .token(&secret)
            .send()
            .await
            .ok()
            .id,
        s.service,
        "issuer ceilings are snapshots, not a live parent-key revocation chain"
    );
}

#[tokio::test]
async fn service_creation_keeps_member_subject_lifecycle_and_issuance_limits() {
    let s = Setup::new().await;
    let manage = rule(s.service, "key", "ServiceAccountTokenCreate", &["manage"]);
    let body = asked(
        &s.world,
        vec![rule(s.service, "account", "UserGet", &["read"])],
    );
    let powerless = issuer(&s.world, s.viewer.id(), vec![manage.clone()]).await;
    let denied = s
        .world
        .app
        .call(ServiceAccountTokenCreate, &[&s.service])
        .token(&powerless.secret)
        .body(&body)
        .send()
        .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&denied.raw).contains("Private service org"));
    let problem = denied.problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
    assert!(
        problem.extra.is_empty(),
        "without Read no named member-policy payload is returned"
    );
    let browser = s
        .world
        .app
        .call(ServiceAccountTokenCreate, &[&s.service])
        .token(&s.viewer.token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(browser.code, problem.code);
    assert!(browser.extra.contains_key("missing"));

    let wrong_family = issuer(
        &s.world,
        s.boss.id(),
        vec![rule(
            s.service,
            "account",
            "ServiceAccountTokenCreate",
            &["manage"],
        )],
    )
    .await;
    assert_eq!(
        s.world
            .app
            .call(ServiceAccountTokenCreate, &[&s.service])
            .token(&wrong_family.secret)
            .body(&body)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN,
        "the Account parent is not the operation's Key business family"
    );
    let read_only = issuer(
        &s.world,
        s.boss.id(),
        vec![rule(
            s.service,
            "key",
            "ServiceAccountTokenCreate",
            &["read"],
        )],
    )
    .await;
    assert_eq!(
        s.world
            .app
            .call(ServiceAccountTokenCreate, &[&s.service])
            .token(&read_only.secret)
            .body(&body)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let parent = issuer(&s.world, s.boss.id(), vec![manage]).await;
    let mut longer = body.clone();
    longer.expires_at = Some(Time::new(parent.expires + Duration::seconds(1)));
    let denied = s
        .world
        .app
        .call(ServiceAccountTokenCreate, &[&s.service])
        .token(&parent.secret)
        .body(&longer)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    assert!(denied.extra.is_empty());
    let mut legacy = body.clone();
    legacy.rules = None;
    legacy.scopes = Some(vec!["*".into()]);
    assert_eq!(
        s.world
            .app
            .call(ServiceAccountTokenCreate, &[&s.service])
            .token(&parent.secret)
            .body(&legacy)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let human = issuer(
        &s.world,
        s.boss.id(),
        vec![rule(
            s.boss.id(),
            "key",
            "ServiceAccountTokenCreate",
            &["manage"],
        )],
    )
    .await;
    assert_eq!(
        s.world
            .app
            .call(ServiceAccountTokenCreate, &[&s.boss.id()])
            .token(&human.secret)
            .body(&body)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND,
        "the ordinary service producer still rejects human subjects"
    );
    let disabled = issuer(
        &s.world,
        s.boss.id(),
        vec![rule(
            s.other,
            "key",
            "ServiceAccountTokenCreate",
            &["manage"],
        )],
    )
    .await;
    identity::disable_service_account(
        &mut *s.world.conn().await,
        s.other,
        s.boss.id(),
        s.world.now(),
    )
    .await
    .unwrap();
    assert_eq!(
        s.world
            .app
            .call(ServiceAccountTokenCreate, &[&s.other])
            .token(&disabled.secret)
            .body(&body)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert!(
        identity::api_tokens_of(&mut *s.world.conn().await, s.service)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        identity::api_tokens_of(&mut *s.world.conn().await, s.other)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn browser_service_creation_keeps_its_fresh_proof_and_full_modern_reply() {
    let s = Setup::new().await;
    let mut body = asked(
        &s.world,
        vec![rule(s.service, "account", "UserGet", &["read"])],
    );
    assert_eq!(
        s.world
            .app
            .call(ServiceAccountTokenCreate, &[&s.service])
            .token(&s.boss.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthReauthenticationRequired
    );
    body.proof = Some(SensitiveProof {
        password: Some(support::PASSWORD.into()),
        ..Default::default()
    });
    let reply = s
        .world
        .app
        .call(ServiceAccountTokenCreate, &[&s.service])
        .token(&s.boss.token)
        .body(&body)
        .send()
        .await
        .ok();
    let TokenCreateResponse::Full(full) = reply else {
        panic!("the browser keeps its full modern response")
    };
    assert!(full.token.is_some() && !full.already_created);
    assert_eq!(full.info.rules, body.rules);
}
