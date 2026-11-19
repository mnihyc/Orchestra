//! Exact native administrative selections keep current standing and their real source identities.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod admin_support;
mod places;
mod support;
use admin_support::cast;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::admin::{
    AdminHarnessUsageList, AdminHostList, AdminOrgList, AdminOrgListQuery, AdminRuntimeList,
    AdminRuntimeListQuery,
};
use orch_api_contract::common::ErrorCode;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;
async fn key(world: &World, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Native administrative selection",
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
fn rule(kind: &str, id: Id, operation: &str, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":[action]}]})
}
fn value(reply: orch_api_core::testing::RawReply) -> Json {
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.raw));
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn resource(world: &World, kind: &str, owner: Id, home: Id, name: &str) -> Id {
    fixtures::insert_resource(
        &mut *world.conn().await,
        kind,
        Some(owner),
        Some(home),
        name,
    )
    .await
    .unwrap()
}
async fn runtime(world: &World, kind: &str, host: Id, template: Id, unit: Id, name: &str) -> Id {
    let id = new_id();
    let (environment, provider) = if kind == "environment" {
        (Some(unit), None)
    } else {
        (None, Some(unit))
    };
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,environment_id,provider_id,driver,template_id,spec_hash,state,generation) VALUES($1,$2,$3,$4,$5,$6,'test',$7,'\\x00'::bytea,'running',2)")
        .bind(id).bind(kind).bind(host).bind(name).bind(environment).bind(provider).bind(template)
        .execute(&world.db.pool).await.unwrap();
    id
}
#[tokio::test]
async fn administrative_org_scope_selects_before_the_native_name_window_and_keeps_retained_rows() {
    let world = World::new().await;
    let c = cast(&world).await;
    for i in 0..8 {
        places::make_org(
            &world,
            &c.root,
            &format!("Scoped earlier {i}"),
            &format!("scoped-earlier-{i}"),
        )
        .await;
    }
    let org = places::make_org(&world, &c.root, "Scoped selected", "scoped-selected")
        .await
        .base
        .id;
    let query = AdminOrgListQuery {
        q: Some("Scoped".into()),
        limit: Some(2),
        ..Default::default()
    };
    let ordinary = value(
        world
            .app
            .call(AdminOrgList, &[])
            .token(&c.root.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary["items"].as_array().unwrap().len(), 2);
    assert!(ordinary["next_cursor"].is_string());
    let scoped = key(
        &world,
        c.root.id(),
        vec![rule("org", org, "AdminOrgList", "read")],
    )
    .await;
    let result = value(
        world
            .app
            .call(AdminOrgList, &[])
            .token(&scoped)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(result["items"].as_array().unwrap().len(), 1);
    assert_eq!(result["items"][0]["id"], json!(org));
    assert_eq!(result["items"][0]["name"], "Scoped selected");
    assert!(result["next_cursor"].is_null());
    sqlx::query("UPDATE nodes SET deleted_at=$2 WHERE id=$1")
        .bind(org)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let retained = AdminOrgListQuery {
        deleted: Some(orch_api_contract::documents::DeletedFilter::Only),
        ..query
    };
    let result = value(
        world
            .app
            .call(AdminOrgList, &[])
            .token(&scoped)
            .query(&retained)
            .send_bytes()
            .await,
    );
    assert_eq!(result["items"][0]["id"], json!(org));
    assert_eq!(result["items"][0]["name"], "Scoped selected");
    let outsider = key(
        &world,
        c.boss.id(),
        vec![rule("org", org, "AdminOrgList", "read")],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(AdminOrgList, &[])
            .token(&outsider)
            .query(&retained)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}
#[tokio::test]
async fn administrative_hosts_keep_the_fleet_shape_and_bound_its_real_home_and_publisher() {
    let world = World::new().await;
    let c = cast(&world).await;
    let host = resource(&world, "host", c.root.id(), c.acme, "Selected fleet").await;
    let other = resource(&world, "host", c.root.id(), c.acme, "Other fleet").await;
    let ordinary = value(
        world
            .app
            .call(AdminHostList, &[])
            .token(&c.root.token)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary.as_array().unwrap().len(), 2);
    let mut host_only = rule("host", host, "AdminHostList", "read");
    host_only["resources"] = json!(["host"]);
    let scoped = key(&world, c.root.id(), vec![host_only]).await;
    let result = value(
        world
            .app
            .call(AdminHostList, &[])
            .token(&scoped)
            .send_bytes()
            .await,
    );
    assert_eq!(result.as_array().unwrap().len(), 1);
    assert_eq!(result[0]["id"], json!(host));
    assert_eq!(result[0]["name"], "Selected fleet");
    assert_ne!(result[0]["machine"]["id"], json!(other));
    assert!(result[0]["full"].is_null() && result[0]["software_update"].is_null());
    assert!(result[0]["home"]["name"].is_null());
    assert!(!result[0]["address"].as_str().unwrap().contains("acme"));
    let with_names = key(
        &world,
        c.root.id(),
        vec![
            rule("host", host, "AdminHostList", "read"),
            rule("org", c.acme, "AdminHostList", "read"),
        ],
    )
    .await;
    let named = value(
        world
            .app
            .call(AdminHostList, &[])
            .token(&with_names)
            .send_bytes()
            .await,
    );
    assert_eq!(named[0]["machine"], result[0]["machine"]);
    assert!(named[0]["address"].as_str().unwrap().contains("acme"));
    sqlx::query("UPDATE users SET is_system_admin=false WHERE id=$1")
        .bind(c.root.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        world
            .app
            .call(AdminHostList, &[])
            .token(&scoped)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}
#[tokio::test]
async fn administrative_runtime_manage_resolves_the_stored_unit_and_its_native_references() {
    let world = World::new().await;
    let c = cast(&world).await;
    let host = resource(&world, "host", c.root.id(), c.acme, "Runtime host").await;
    let template = resource(&world, "template", c.root.id(), c.acme, "Runtime template").await;
    let env = resource(
        &world,
        "environment",
        c.root.id(),
        c.acme,
        "Runtime environment",
    )
    .await;
    let provider = resource(&world, "provider", c.root.id(), c.acme, "Runtime provider").await;
    let selected = runtime(&world, "environment", host, template, env, "selected-unit").await;
    runtime(&world, "provider", host, template, provider, "other-unit").await;
    let query = AdminRuntimeListQuery::default();
    let ordinary = value(
        world
            .app
            .call(AdminRuntimeList, &[])
            .token(&c.root.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary.as_array().unwrap().len(), 2);
    let scoped = key(
        &world,
        c.root.id(),
        vec![
            rule("environment", env, "AdminRuntimeList", "manage"),
            rule("host", host, "AdminRuntimeList", "manage"),
            rule("template", template, "AdminRuntimeList", "manage"),
        ],
    )
    .await;
    let result = value(
        world
            .app
            .call(AdminRuntimeList, &[])
            .token(&scoped)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(result.as_array().unwrap().len(), 1);
    assert_eq!(result[0]["id"], json!(selected));
    assert_eq!(result[0]["environment"]["id"], json!(env));
    assert_eq!(result[0]["template"]["id"], json!(template));
    let surrogate = key(
        &world,
        c.root.id(),
        vec![
            rule("environment", selected, "AdminRuntimeList", "manage"),
            rule("host", host, "AdminRuntimeList", "manage"),
            rule("template", template, "AdminRuntimeList", "manage"),
        ],
    )
    .await;
    let result = value(
        world
            .app
            .call(AdminRuntimeList, &[])
            .token(&surrogate)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(result, json!([]), "a runtime UUID is never a unit identity");
}
#[tokio::test]
async fn administrative_build_usage_selects_actual_units_before_aggregating_native_frozen_builds() {
    let world = World::new().await;
    let c = cast(&world).await;
    let host = resource(&world, "host", c.root.id(), c.acme, "Build host").await;
    let template = resource(&world, "template", c.root.id(), c.acme, "Build template").await;
    let first = resource(&world, "provider", c.root.id(), c.acme, "Selected provider").await;
    let second = resource(&world, "provider", c.root.id(), c.acme, "Other provider").await;
    for (unit, version) in [(first, "1.2.3"), (second, "9.9.9")] {
        let id = runtime(
            &world,
            "provider",
            host,
            template,
            unit,
            &format!("build-{version}"),
        )
        .await;
        let plan = json!({"runtime":id,"tools":[{"name":"fake","version":version,"url":"http://unused.test/tool","sha256":"00"}]});
        sqlx::query("INSERT INTO jobs(id,kind,provider_id,state,plan) VALUES($1,'provider_provision',$2,'succeeded',$3)")
            .bind(new_id()).bind(unit).bind(plan).execute(&world.db.pool).await.unwrap();
    }
    let ordinary = value(
        world
            .app
            .call(AdminHarnessUsageList, &[])
            .token(&c.root.token)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary.as_array().unwrap().len(), 2);
    let scoped = key(
        &world,
        c.root.id(),
        vec![rule("provider", first, "AdminHarnessUsageList", "read")],
    )
    .await;
    let result = value(
        world
            .app
            .call(AdminHarnessUsageList, &[])
            .token(&scoped)
            .send_bytes()
            .await,
    );
    assert_eq!(result.as_array().unwrap().len(), 1);
    assert_eq!(result[0]["version"], "1.2.3");
    assert_eq!(result[0]["providers"].as_array().unwrap().len(), 1);
    assert_eq!(result[0]["providers"][0]["id"], json!(first));
    assert!(
        !result[0]["providers"][0]["address"]
            .as_str()
            .unwrap()
            .contains("acme")
    );
    let empty = key(
        &world,
        c.root.id(),
        vec![rule("provider", new_id(), "AdminHarnessUsageList", "read")],
    )
    .await;
    assert_eq!(
        value(
            world
                .app
                .call(AdminHarnessUsageList, &[])
                .token(&empty)
                .send_bytes()
                .await
        ),
        json!([])
    );
}

#[tokio::test]
async fn administrative_org_limits_keep_manage_effects_with_a_typed_receipt_and_same_operation_readback()
 {
    use orch_api_contract::admin::{AdminOrgUpdate, OrgLimitsChange};
    let world = World::new().await;
    let c = cast(&world).await;
    let org = places::make_org(&world, &c.root, "Limits subject", "limits-subject")
        .await
        .base
        .id;
    let before: i64 = sqlx::query_scalar("SELECT revision FROM nodes WHERE id=$1")
        .bind(org)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let manage = key(
        &world,
        c.root.id(),
        vec![rule("org", org, "AdminOrgUpdate", "manage")],
    )
    .await;
    let receipt = value(
        world
            .app
            .call(AdminOrgUpdate, &[&org])
            .token(&manage)
            .body(&OrgLimitsChange {
                closed: Some(true),
                flat: None,
            })
            .send_bytes()
            .await,
    );
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["id"], json!(org));
    assert_eq!(receipt["revision"], before + 1);
    assert!(receipt.get("name").is_none() && receipt.get("closed").is_none());
    let stored: bool = sqlx::query_scalar("SELECT closed FROM orgs WHERE node_id=$1")
        .bind(org)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert!(stored);
    let noop = value(
        world
            .app
            .call(AdminOrgUpdate, &[&org])
            .token(&manage)
            .body(&OrgLimitsChange {
                closed: Some(true),
                flat: None,
            })
            .send_bytes()
            .await,
    );
    assert_eq!(noop["revision"], receipt["revision"]);
    let with_read = key(
        &world,
        c.root.id(),
        vec![
            rule("org", org, "AdminOrgUpdate", "manage"),
            rule("org", org, "AdminOrgUpdate", "read"),
        ],
    )
    .await;
    let full = value(
        world
            .app
            .call(AdminOrgUpdate, &[&org])
            .token(&with_read)
            .body(&OrgLimitsChange {
                closed: Some(false),
                flat: Some(true),
            })
            .send_bytes()
            .await,
    );
    assert_eq!(full["id"], json!(org));
    assert_eq!(full["name"], "Limits subject");
    assert_eq!(full["closed"], false);
    assert_eq!(full["flat"], true);
    let read_only = key(
        &world,
        c.root.id(),
        vec![rule("org", org, "AdminOrgUpdate", "read")],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(AdminOrgUpdate, &[&org])
            .token(&read_only)
            .body(&OrgLimitsChange {
                closed: Some(true),
                flat: None
            })
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let other = key(
        &world,
        c.root.id(),
        vec![rule("org", c.acme, "AdminOrgUpdate", "manage")],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(AdminOrgUpdate, &[&org])
            .token(&other)
            .body(&OrgLimitsChange {
                closed: Some(true),
                flat: None
            })
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[path = "admin_native_scopes/semantic.rs"]
mod semantic;
