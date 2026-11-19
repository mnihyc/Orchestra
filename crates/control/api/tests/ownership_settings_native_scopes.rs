//! Native settings provenance and retained ownership rows stay bound to their own operation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod places;
mod support;

use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::ownership::OrgDeletedList;
use orch_api_contract::settings::EnvironmentSettingList;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::{hierarchy, identity, resources, settings};
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
            name: "Native settings and ownership fixture",
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

fn checked(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn environment_settings_keep_native_values_with_independently_bound_provenance_names() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let org = places::make_org(&world, &owner, "Setting root", "setting-root")
        .await
        .base
        .id;
    let project = places::make_project(&world, &owner, org, "Private provenance")
        .await
        .base
        .id;
    let env = resources::access_fixtures::insert_environment(
        &mut *world.conn().await,
        owner.id(),
        Some(project),
        "Settings subject",
        "managed",
    )
    .await
    .unwrap();
    settings::put(
        &mut *world.conn().await,
        Some(project),
        "env.idle_stop_minutes",
        &settings::Stored {
            value: &json!(37),
            locked: false,
            by: Some(owner.id()),
            at: world.now(),
        },
        None,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET config=$2 WHERE id=$1")
        .bind(env)
        .bind(json!({"exec.orch_tools":false}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let limited = key(
        &world,
        owner.id(),
        vec![
            rule("EnvironmentSettingList", "read", "environment", env),
            rule("NodeTagList", "read", "project", project),
        ],
    )
    .await;
    let bounded = checked(
        world
            .app
            .call(EnvironmentSettingList, &[&env])
            .token(&limited)
            .send_bytes()
            .await,
    );
    let settings = bounded.as_array().unwrap();
    let idle = settings
        .iter()
        .find(|setting| setting["key"] == "env.idle_stop_minutes")
        .unwrap();
    assert_eq!(idle["value"], 37);
    assert_eq!(idle["source"]["node"]["id"], json!(project));
    assert!(idle["source"]["node"]["name"].is_null());
    assert_eq!(idle["source"]["node"]["redacted"], true);
    assert!(!bounded.to_string().contains("Private provenance"));
    assert_eq!(
        settings
            .iter()
            .find(|setting| setting["key"] == "exec.orch_tools")
            .unwrap()["value"],
        false
    );

    let full = key(
        &world,
        owner.id(),
        vec![
            rule("EnvironmentSettingList", "read", "environment", env),
            rule("EnvironmentSettingList", "read", "project", project),
            rule("EnvironmentSettingList", "read", "org", org),
        ],
    )
    .await;
    let full_reply = checked(
        world
            .app
            .call(EnvironmentSettingList, &[&env])
            .token(&full)
            .send_bytes()
            .await,
    );
    let ordinary = checked(
        world
            .app
            .call(EnvironmentSettingList, &[&env])
            .token(&owner.token)
            .send_bytes()
            .await,
    );
    assert_eq!(full_reply, ordinary);
    assert!(full_reply.to_string().contains("Private provenance"));
}

#[tokio::test]
async fn environment_settings_scope_does_not_supply_native_environment_view() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let outsider = places::person(&world, "outsider").await;
    let org = places::make_org(&world, &owner, "Hidden root", "hidden-settings")
        .await
        .base
        .id;
    let env = resources::access_fixtures::insert_environment(
        &mut *world.conn().await,
        owner.id(),
        Some(org),
        "Hidden settings subject",
        "managed",
    )
    .await
    .unwrap();
    let secret = key(
        &world,
        outsider.id(),
        vec![rule("EnvironmentSettingList", "read", "environment", env)],
    )
    .await;
    let denied = world
        .app
        .call(EnvironmentSettingList, &[&env])
        .token(&secret)
        .send_bytes()
        .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&denied.raw).contains("Hidden settings subject"));
}

#[tokio::test]
async fn deleted_org_lists_use_retained_identity_native_ownership_and_retention_before_output() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let outsider = places::person(&world, "outsider").await;
    let mut roots = Vec::new();
    let expired = world.now()
        - chrono::Duration::days(
            i64::try_from(world.app.app.config.retention.soft_delete_days).unwrap() + 1,
        );
    for (name, slug) in [
        ("Selected deletion", "selected-deletion"),
        ("Other deletion", "other-deletion"),
        ("Expired deletion", "expired-deletion"),
    ] {
        let id = places::make_org(&world, &owner, name, slug).await.base.id;
        hierarchy::testing::soft_delete(
            &mut *world.conn().await,
            id,
            owner.id(),
            if slug == "expired-deletion" {
                expired
            } else {
                world.now()
            },
        )
        .await
        .unwrap();
        roots.push(id);
    }
    let secret = key(
        &world,
        owner.id(),
        vec![rule("OrgDeletedList", "manage", "org", roots[0])],
    )
    .await;
    let bounded = checked(
        world
            .app
            .call(OrgDeletedList, &[])
            .token(&secret)
            .send_bytes()
            .await,
    );
    assert_eq!(bounded.as_array().unwrap().len(), 1);
    assert_eq!(bounded[0]["org"]["id"], json!(roots[0]));
    assert_eq!(bounded[0]["org"]["name"], "Selected deletion");
    assert!(!bounded.to_string().contains("Other deletion"));
    let ordinary = checked(
        world
            .app
            .call(OrgDeletedList, &[])
            .token(&owner.token)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary.as_array().unwrap().len(), 2);
    assert!(ordinary.as_array().unwrap().contains(&bounded[0]));
    let excluded = key(
        &world,
        outsider.id(),
        vec![rule("OrgDeletedList", "manage", "org", roots[0])],
    )
    .await;
    let empty = checked(
        world
            .app
            .call(OrgDeletedList, &[])
            .token(&excluded)
            .send_bytes()
            .await,
    );
    assert_eq!(empty, json!([]));
}
