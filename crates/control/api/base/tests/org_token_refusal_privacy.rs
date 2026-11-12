//! A no-content refusal operation still bounds names in its ordinary error response.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod places;
mod support;

use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::{Permission, PermissionName};
use orch_api_contract::auth::OrgTokenRefuse;
use orch_api_contract::common::ErrorCode;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::queries::{access, identity};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

const NAME: &str = "Refusal source title sentinel";
const OPERATION: &str = "OrgTokenRefuse";

async fn key(world: &World, account: Id, org: Id, read: Option<&str>) -> String {
    let mut rules = vec![json!({
        "target":{"kind":"org","id":org,"reach":"self"},"resources":["key"],
        "allow":[{"operation":OPERATION,"actions":["manage"]}]
    })];
    if let Some(operation) = read {
        rules.push(json!({
            "target":{"kind":"org","id":org,"reach":"self"},"resources":["org"],
            "allow":[{"operation":operation,"actions":["read"]}]
        }));
    }
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: account,
            name: "Refusal error projection control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}
async fn refuse(world: &World, token: &str, org: Id, target: Id) -> RawReply {
    world
        .app
        .call(OrgTokenRefuse, &[&org, &target])
        .token(token)
        .send_bytes()
        .await
}
fn denied(reply: &RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let problem: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(problem["code"], json!(ErrorCode::AuthzDenied));
    assert_eq!(problem["required"], "members.manage", "{problem}");
    problem
}
fn names_org(problem: &Json, org: Id) -> bool {
    problem["missing"].as_array().is_some_and(|missing| {
        missing
            .iter()
            .any(|part| part["object"]["id"] == json!(org) && part["object"]["name"] == NAME)
    })
}

#[tokio::test]
async fn refusal_manage_without_own_read_keeps_the_ordinary_denial_but_not_its_unsupplied_org_name()
{
    let world = World::new().await;
    let owner = places::person(&world, "sourceowner").await;
    let viewer = places::person(&world, "sourceviewer").await;
    let org = places::make_org(&world, &owner, NAME, "refusal-source")
        .await
        .base
        .id;
    places::join(&world, org, org, &viewer, "viewer", &owner).await;
    let target = identity::testing::insert_api_token(
        &mut *world.conn().await,
        owner.id(),
        &token_hash(&new_token(API_TOKEN_PREFIX)),
    )
    .await
    .unwrap();
    let mut conn = world.conn().await;
    let ordinary = Principal::load(
        &mut conn,
        Caller::session(viewer.id()),
        Time::new(world.now()),
    )
    .await
    .unwrap();
    assert!(
        orch_authz::nodes::check_named(
            &mut conn,
            &ordinary,
            org,
            orch_api_core::middleware::authorization::VIEW,
            Act::Use,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    assert!(
        !orch_authz::nodes::check_named(
            &mut conn,
            &ordinary,
            org,
            PermissionName::Exact(Permission::MembersManage),
            Act::Use,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    drop(conn);
    // The request supplies only UUIDs. Its name comes from the actual ordinary
    // error producer, establishing a live source before testing key restrictions.
    let browser = refuse(&world, &viewer.token, org, target).await;
    let browser_problem = denied(&browser);
    assert!(names_org(&browser_problem, org), "{browser_problem}");
    assert!(String::from_utf8_lossy(&browser.raw).contains(NAME));

    for read in [None, Some("NodeGet.read")] {
        let caller = key(&world, viewer.id(), org, read).await;
        let response = refuse(&world, &caller, org, target).await;
        let problem = denied(&response);
        assert_eq!(problem["code"], browser_problem["code"]);
        assert!(!names_org(&problem, org), "{problem}");
        assert!(
            !String::from_utf8_lossy(&response.raw).contains(NAME),
            "the named source needs this operation's Read: {problem}"
        );
    }
    let complete = key(&world, viewer.id(), org, Some(OPERATION)).await;
    let complete_problem = denied(&refuse(&world, &complete, org, target).await);
    assert!(names_org(&complete_problem, org), "{complete_problem}");

    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM api_token_refusals WHERE token_id=$1 AND org_id=$2",
    )
    .bind(target)
    .bind(org)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(before, 0, "all ordinary denials precede the policy effect");
    access::testing::add_entry(
        &mut *world.conn().await,
        &access::PageKey::new("node", org, "live"),
        "any",
        "members.manage",
        ("user", Some(viewer.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    orch_authz::epoch::bump(&mut *world.conn().await)
        .await
        .unwrap();
    let manage_only = key(&world, viewer.id(), org, None).await;
    let accepted = refuse(&world, &manage_only, org, target).await;
    assert_eq!(
        accepted.status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&accepted.raw)
    );
    assert!(
        accepted.raw.is_empty(),
        "204 is the existing truthful receipt"
    );
    let after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM api_token_refusals WHERE token_id=$1 AND org_id=$2",
    )
    .bind(target)
    .bind(org)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(after, 1, "the optional error Read does not gate Manage");
}
