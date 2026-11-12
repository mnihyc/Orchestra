//! Membership Manage exposes counts/IDs; full child metadata uses this operation's own Read.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod places;
mod support;

use http::StatusCode;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::ownership::{HoldingsResponse, OrgMemberHoldings};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{identity, resources};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;

fn rule(operation: &str, action: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],
        "allow":[{"operation":operation,"actions":[action]}]})
}

async fn key(world: &World, actor: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: actor,
            name: "Membership preview fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    secret
}

async fn legacy(world: &World, actor: Id, org: Id) -> String {
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: actor,
            name: "Legacy membership preview fixture",
            token_hash: &token_hash(&secret),
            scopes: &["members.manage".to_owned()],
            node_id: Some(org),
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        None,
    )
    .await
    .unwrap();
    secret
}

fn checked(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

struct Scene {
    world: World,
    owner: places::Who,
    member: places::Who,
    org: Id,
    domain: Id,
    other: Id,
    provider: Id,
    machine: Id,
    environment: Id,
}

impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let owner = places::person(&world, "hold-owner").await;
        let member = places::person(&world, "hold-member").await;
        let org = places::make_org(&world, &owner, "Holding root", "holding-root")
            .await
            .base
            .id;
        let domain =
            places::make_department(&world, &owner, org, "Actual member domain", "member-domain")
                .await
                .base
                .id;
        let other = places::make_department(&world, &owner, org, "Other domain", "other-domain")
            .await
            .base
            .id;
        places::join(&world, org, domain, &member, "viewer", &owner).await;
        let mut conn = world.conn().await;
        let provider = resources::access_fixtures::insert_resource(
            &mut conn,
            "provider",
            Some(member.id()),
            Some(domain),
            "Private provider sentinel",
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ResourceKind::Provider.into(),
                id: provider,
            },
            member.id(),
            NewAccess::OnlyMe,
            orch_types::Time::new(world.now()),
        )
        .await
        .unwrap();
        let machine =
            resources::testing::insert_machine(&mut conn, "Private machine sentinel", "connected")
                .await
                .unwrap();
        sqlx::query("UPDATE machines SET owner_id=$2 WHERE id=$1")
            .bind(machine)
            .bind(member.id())
            .execute(&mut *conn)
            .await
            .unwrap();
        let environment = resources::access_fixtures::insert_resource(
            &mut conn,
            "environment",
            Some(owner.id()),
            Some(domain),
            "Private registration sentinel",
        )
        .await
        .unwrap();
        sqlx::query("UPDATE environments SET placement='connected',machine_id=$2 WHERE id=$1")
            .bind(environment)
            .bind(machine)
            .execute(&mut *conn)
            .await
            .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ResourceKind::Environment.into(),
                id: environment,
            },
            owner.id(),
            NewAccess::OnlyMe,
            orch_types::Time::new(world.now()),
        )
        .await
        .unwrap();
        drop(conn);
        Self {
            world,
            owner,
            member,
            org,
            domain,
            other,
            provider,
            machine,
            environment,
        }
    }

    fn manage(&self) -> Json {
        rule("OrgMemberHoldings", "manage", "org", self.domain)
    }

    fn full(&self) -> Vec<Json> {
        vec![
            self.manage(),
            rule("OrgMemberHoldings", "read", "provider", self.provider),
            rule("OrgMemberHoldings", "read", "environment", self.environment),
            rule("OrgMemberHoldings", "read", "machine", self.machine),
        ]
    }

    async fn read(&self, secret: &str, path: Id) -> RawReply {
        self.world
            .app
            .call(OrgMemberHoldings, &[&path, &self.member.id()])
            .token(secret)
            .send_bytes()
            .await
    }
}

fn summary(body: &Json, scene: &Scene) {
    assert_eq!(body["projection"], "receipt");
    assert_eq!(body["org_id"], json!(scene.org));
    assert_eq!(body["domain_id"], json!(scene.domain));
    assert_eq!(body["user_id"], json!(scene.member.id()));
    assert_eq!(
        body["resources"][0]["object"],
        json!({"kind":"provider","id":scene.provider})
    );
    assert_eq!(
        body["machine_registrations"][0]["object"],
        json!({"kind":"environment","id":scene.environment})
    );
    assert_eq!(
        body["machine_registrations"][0]["machine_id"],
        json!(scene.machine)
    );
    assert!(body["resources"][0].get("personal_sign_in").is_none());
    assert!(body["machine_registrations"][0].get("machine").is_none());
    assert!(!body.to_string().contains("sentinel"));
    assert!(!body.to_string().contains("platform_os"));
}

#[tokio::test]
async fn holdings_manage_returns_counts_and_ids_and_only_own_read_discloses_metadata() {
    let scene = Scene::new().await;
    let manage = key(&scene.world, scene.owner.id(), vec![scene.manage()]).await;
    let bounded = checked(scene.read(&manage, scene.org).await);
    summary(&bounded, &scene);
    let borrowed = key(
        &scene.world,
        scene.owner.id(),
        vec![
            scene.manage(),
            rule("ProviderGet", "read", "provider", scene.provider),
            rule("EnvironmentGet", "read", "environment", scene.environment),
            rule("MachineGet", "read", "machine", scene.machine),
        ],
    )
    .await;
    summary(&checked(scene.read(&borrowed, scene.org).await), &scene);
    let no_machine = key(
        &scene.world,
        scene.owner.id(),
        vec![
            scene.manage(),
            rule("OrgMemberHoldings", "read", "provider", scene.provider),
            rule(
                "OrgMemberHoldings",
                "read",
                "environment",
                scene.environment,
            ),
        ],
    )
    .await;
    summary(&checked(scene.read(&no_machine, scene.org).await), &scene);
    let full = key(&scene.world, scene.owner.id(), scene.full()).await;
    let admitted = checked(scene.read(&full, scene.org).await);
    let ordinary = checked(scene.read(&scene.owner.token, scene.org).await);
    assert_eq!(admitted, ordinary);
    let legacy = legacy(&scene.world, scene.owner.id(), scene.org).await;
    assert_eq!(checked(scene.read(&legacy, scene.org).await), ordinary);
    assert_eq!(
        admitted["resources"][0]["object"]["name"],
        "Private provider sentinel"
    );
    assert_eq!(
        admitted["machine_registrations"][0]["machine"]["name"],
        "Private machine sentinel"
    );
    for field in ["nodes", "shared", "shared_by_kind", "entries", "pending"] {
        assert_eq!(bounded[field], ordinary[field]);
    }
    let own = key(&scene.world, scene.member.id(), vec![scene.manage()]).await;
    summary(&checked(scene.read(&own, scene.org).await), &scene);
}

#[tokio::test]
async fn holdings_scope_uses_the_stored_domain_and_never_supplies_ordinary_membership_authority() {
    let scene = Scene::new().await;
    let exact = key(&scene.world, scene.owner.id(), vec![scene.manage()]).await;
    let root_path = checked(scene.read(&exact, scene.org).await);
    let other_path = checked(scene.read(&exact, scene.other).await);
    assert_eq!(
        root_path, other_path,
        "the requested alias does not change the actual membership"
    );
    for rules in [
        vec![rule("OrgMemberHoldings", "manage", "org", scene.org)],
        vec![rule("OrgMemberHoldings", "manage", "org", scene.other)],
        vec![rule("OrgMemberHoldings", "write", "org", scene.domain)],
        vec![rule("OrgMemberMove", "manage", "org", scene.domain)],
    ] {
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        let reply = scene.read(&secret, scene.org).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert!(!String::from_utf8_lossy(&reply.raw).contains("sentinel"));
    }
    let outsider = places::person(&scene.world, "hold-outsider").await;
    places::join(
        &scene.world,
        scene.org,
        scene.org,
        &outsider,
        "viewer",
        &scene.owner,
    )
    .await;
    let no_acl = key(&scene.world, outsider.id(), vec![scene.manage()]).await;
    assert_eq!(
        scene.read(&no_acl, scene.org).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn empty_holdings_still_distinguish_an_aggregate_summary_from_the_full_native_body() {
    let world = World::new().await;
    let owner = places::person(&world, "hold-owner").await;
    let member = places::person(&world, "hold-member").await;
    let org = places::make_org(&world, &owner, "Empty holding root", "empty-holdings")
        .await
        .base
        .id;
    places::join(&world, org, org, &member, "viewer", &owner).await;
    let limited = key(
        &world,
        owner.id(),
        vec![rule("OrgMemberHoldings", "manage", "org", org)],
    )
    .await;
    let body = checked(
        world
            .app
            .call(OrgMemberHoldings, &[&org, &member.id()])
            .token(&limited)
            .send_bytes()
            .await,
    );
    assert_eq!(body["projection"], "receipt");
    assert_eq!(body["resources"], json!([]));
    assert_eq!(body["machine_registrations"], json!([]));
    assert!(matches!(
        serde_json::from_value::<HoldingsResponse>(body).unwrap(),
        HoldingsResponse::Receipt(_)
    ));
    let full = key(
        &world,
        owner.id(),
        vec![
            rule("OrgMemberHoldings", "manage", "org", org),
            json!({
                "target":{"kind":"org","id":org,"reach":"subtree"},"resources":["provider"],
                "allow":[{"operation":"OrgMemberHoldings","actions":["read"]}]
            }),
        ],
    )
    .await;
    let body = checked(
        world
            .app
            .call(OrgMemberHoldings, &[&org, &member.id()])
            .token(&full)
            .send_bytes()
            .await,
    );
    let ordinary = checked(
        world
            .app
            .call(OrgMemberHoldings, &[&org, &member.id()])
            .token(&owner.token)
            .send_bytes()
            .await,
    );
    assert_eq!(body, ordinary);
}

#[tokio::test]
async fn holdings_publication_keeps_each_original_registration_machine_association() {
    for change in [false, true] {
        let scene = Scene::new().await;
        let mut conn = scene.world.conn().await;
        let machine =
            resources::testing::insert_machine(&mut conn, "Second machine sentinel", "connected")
                .await
                .unwrap();
        sqlx::query("UPDATE machines SET owner_id=$2 WHERE id=$1")
            .bind(machine)
            .bind(scene.member.id())
            .execute(&mut *conn)
            .await
            .unwrap();
        let environment = resources::access_fixtures::insert_resource(
            &mut conn,
            "environment",
            Some(scene.owner.id()),
            Some(scene.domain),
            "Second registration sentinel",
        )
        .await
        .unwrap();
        sqlx::query("UPDATE environments SET placement='connected',machine_id=$2 WHERE id=$1")
            .bind(environment)
            .bind(machine)
            .execute(&mut *conn)
            .await
            .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ResourceKind::Environment.into(),
                id: environment,
            },
            scene.owner.id(),
            NewAccess::OnlyMe,
            orch_types::Time::new(scene.world.now()),
        )
        .await
        .unwrap();
        drop(conn);
        let mut rules = scene.full();
        rules.push(rule(
            "OrgMemberHoldings",
            "read",
            "environment",
            environment,
        ));
        rules.push(rule("OrgMemberHoldings", "read", "machine", machine));
        let secret = key(&scene.world, scene.owner.id(), rules).await;
        let original = checked(scene.read(&secret, scene.org).await);
        assert_eq!(
            original["machine_registrations"].as_array().unwrap().len(),
            2
        );
        let mut lock = scene.world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *lock)
            .await
            .unwrap();
        let app = scene.world.app.clone();
        let held_key = secret.clone();
        let org = scene.org;
        let user = scene.member.id();
        let request = tokio::spawn(async move {
            let call = app.call(OrgMemberHoldings, &[&org, &user]).token(&held_key);
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the native activity wait was not reached");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(blocker).fetch_one(&scene.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the full holdings body must wait at activity publication");
        if change {
            let mut conn = scene.world.conn().await;
            sqlx::query("UPDATE environments SET machine_id=CASE WHEN id=$1 THEN $3 ELSE $2 END WHERE id=ANY($4)")
                .bind(scene.environment).bind(scene.machine).bind(machine).bind(vec![scene.environment, environment])
                .execute(&mut *conn).await.unwrap();
            orch_authz::epoch::bump(&mut conn).await.unwrap();
        }
        lock.rollback().await.unwrap();
        let reply = request.await.unwrap();
        if change {
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
            assert!(!String::from_utf8_lossy(&reply.raw).contains("sentinel"));
            let current = checked(scene.read(&secret, scene.org).await);
            let row = current["machine_registrations"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["object"]["id"] == json!(scene.environment))
                .unwrap();
            assert_eq!(row["machine"]["id"], json!(machine));
        } else {
            assert_eq!(checked(reply), original);
        }
    }
}
