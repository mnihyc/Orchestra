//! The native Host list keeps its selection, source and current caller boundaries.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use http::StatusCode;
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyScope, permissions::ResourceKind};
use orch_api_contract::resources::{HostList, HostListQuery};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{identity, resources::access_fixtures};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

struct Key {
    id: Id,
    secret: String,
}

async fn key(world: &World, user: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules": rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Host list source control",
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
    Key { id, secret }
}

fn rule(operation: &str, kind: &str, id: Id, business: &str) -> Json {
    json!({
        "target": {"kind": kind, "id": id, "reach": "self"},
        "resources": [business],
        "allow": [{"operation": operation, "actions": ["read"]}]
    })
}

fn all_families(operation: &str, kind: &str, id: Id) -> Json {
    json!({
        "target": {"kind": kind, "id": id, "reach": "self"},
        "allow": [{"operation": operation, "actions": ["read"]}]
    })
}

async fn host(world: &World, owner: Id, home: Option<Id>, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id = access_fixtures::insert_resource(&mut conn, "host", Some(owner), home, name)
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Host.into(),
            id,
        },
        owner,
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query(
        r#"UPDATE hosts SET labels=ARRAY['native-label'],
                capacity='{"cpus":4,"memory_mb":8192,"disk_gb":128}'::jsonb,
                storages='[{"name":"native-store","default":true,"private_views":false}]'::jsonb
         WHERE id=$1"#,
    )
    .bind(id)
    .execute(&mut *conn)
    .await
    .unwrap();
    id
}

async fn list(world: &World, token: &str, org: Option<Id>) -> RawReply {
    world
        .app
        .call(HostList, &[])
        .token(token)
        .query(&HostListQuery { org_id: org })
        .send_bytes()
        .await
}

fn rows(reply: RawReply) -> Vec<Json> {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn host_list_binds_exact_hosts_and_preserves_the_native_body() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let selected = host(
        &world,
        company.viewer.id(),
        Some(company.acme),
        "Selected Host",
    )
    .await;
    let peer = host(&world, company.viewer.id(), Some(company.acme), "Peer Host").await;
    let ordinary = rows(list(&world, &company.viewer.token, Some(company.acme)).await);
    let ordinary = ordinary
        .iter()
        .find(|row| row["id"] == json!(selected))
        .unwrap();
    let reader = key(
        &world,
        company.viewer.id(),
        vec![
            rule("HostList", "host", selected, "host"),
            rule("HostList", "org", company.acme, "org"),
        ],
    )
    .await;
    let scoped = rows(list(&world, &reader.secret, Some(company.acme)).await);
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0]["id"], json!(selected));
    assert!(!scoped.iter().any(|row| row["id"] == json!(peer)));
    for field in [
        "id",
        "name",
        "address",
        "machine",
        "owner",
        "labels",
        "drivers",
        "capacity",
        "declared",
        "schedulable",
        "when_full",
        "storages",
        "running",
        "revision",
    ] {
        assert_eq!(scoped[0][field], ordinary[field], "native Host {field}");
    }
    assert_eq!(scoped[0]["home"], ordinary["home"]);
}

#[tokio::test]
async fn host_list_keeps_its_own_operation_typed_kind_and_ordinary_visibility() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let selected = host(
        &world,
        company.viewer.id(),
        Some(company.acme),
        "Private Host",
    )
    .await;
    let other_operation = key(
        &world,
        company.viewer.id(),
        vec![rule("HostGet", "host", selected, "host")],
    )
    .await;
    assert_eq!(
        list(&world, &other_operation.secret, None).await.status,
        StatusCode::FORBIDDEN
    );
    let wrong_kind = key(
        &world,
        company.viewer.id(),
        vec![all_families("HostList", "machine", selected)],
    )
    .await;
    assert!(rows(list(&world, &wrong_kind.secret, None).await).is_empty());
    let wrong_family = key(
        &world,
        company.viewer.id(),
        vec![rule("HostList", "host", selected, "machine")],
    )
    .await;
    assert_eq!(
        list(&world, &wrong_family.secret, None).await.status,
        StatusCode::FORBIDDEN,
    );
    let no_ordinary_access = key(
        &world,
        company.stranger.id(),
        vec![all_families("HostList", "host", selected)],
    )
    .await;
    assert!(rows(list(&world, &no_ordinary_access.secret, None).await).is_empty());
}

#[tokio::test]
async fn host_list_names_use_same_operation_read_and_keep_the_original_org_filter() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let selected = host(
        &world,
        company.viewer.id(),
        Some(company.acme),
        "Homed Host",
    )
    .await;
    let system = host(&world, company.viewer.id(), None, "System Host").await;
    let elsewhere = places::make_org(&world, &company.boss, "Other home", "other-home")
        .await
        .base
        .id;
    let foreign = host(&world, company.viewer.id(), Some(elsewhere), "Other Host").await;
    let mut rules = vec![
        rule("HostList", "host", selected, "host"),
        rule("HostList", "host", system, "host"),
        rule("HostList", "host", foreign, "host"),
        rule("NodeGet", "org", company.acme, "org"),
    ];
    let without_names = key(&world, company.viewer.id(), rules.clone()).await;
    let selected_rows = rows(list(&world, &without_names.secret, Some(company.acme)).await);
    assert_eq!(
        selected_rows.len(),
        2,
        "the org filter still includes system Hosts"
    );
    assert!(!selected_rows.iter().any(|row| row["id"] == json!(foreign)));
    assert!(
        selected_rows
            .iter()
            .find(|row| row["id"] == json!(selected))
            .unwrap()["home"]["name"]
            .is_null()
    );
    rules.push(rule("HostList", "org", company.acme, "org"));
    let with_names = key(&world, company.viewer.id(), rules).await;
    let selected_rows = rows(list(&world, &with_names.secret, Some(company.acme)).await);
    assert_eq!(
        selected_rows
            .iter()
            .find(|row| row["id"] == json!(selected))
            .unwrap()["home"]["name"],
        "Acme"
    );
}

#[tokio::test]
async fn host_list_binds_only_existing_retained_environment_sources() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let selected = host(
        &world,
        company.viewer.id(),
        Some(company.acme),
        "Layer Host",
    )
    .await;
    let environment = new_id();
    sqlx::query("UPDATE hosts SET kept_images=$2 WHERE id=$1")
        .bind(selected)
        .bind(json!({"epoch": 7, "images": [{
            "system": "test", "environment": environment, "generation": 4,
            "capture": new_id(), "reference": format!("orch-keep-{environment}:4"),
            "bytes": 768, "total_bytes": 900, "layers": 2
        }]}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let reader = key(
        &world,
        company.viewer.id(),
        vec![rule("HostList", "host", selected, "host")],
    )
    .await;
    let page = rows(list(&world, &reader.secret, None).await);
    assert_eq!(
        page[0]["kept_layers"][0]["environment_id"],
        json!(environment)
    );
    assert_eq!(page[0]["kept_layers"][0]["bytes"], 768);
    assert!(page[0]["kept_layers"][0]["environment"].is_null());
    let retained_home = places::make_org(&world, &company.boss, "Retained home", "retained-home")
        .await
        .base
        .id;
    let mut conn = world.conn().await;
    let actual = access_fixtures::insert_resource(
        &mut conn,
        "environment",
        Some(company.viewer.id()),
        Some(retained_home),
        "Retained source",
    )
    .await
    .unwrap();
    drop(conn);
    sqlx::query("UPDATE hosts SET kept_images=jsonb_set(kept_images,'{images,0,environment}',to_jsonb($2::uuid)) WHERE id=$1")
        .bind(selected)
        .bind(actual)
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        list(&world, &reader.secret, None).await.status,
        StatusCode::FORBIDDEN
    );
    let complete = key(
        &world,
        company.viewer.id(),
        vec![
            rule("HostList", "host", selected, "host"),
            rule("HostList", "environment", actual, "environment"),
        ],
    )
    .await;
    let page = rows(list(&world, &complete.secret, None).await);
    assert_eq!(
        page[0]["kept_layers"][0]["environment"]["name"],
        "Retained source"
    );
    sqlx::query("UPDATE nodes SET deleted_at=$2 WHERE id=$1")
        .bind(retained_home)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let native = rows(list(&world, &company.viewer.token, None).await);
    let native = native
        .iter()
        .find(|row| row["id"] == json!(selected))
        .unwrap();
    let page = rows(list(&world, &complete.secret, None).await);
    assert_eq!(
        page[0]["kept_layers"], native["kept_layers"],
        "selected retained context keeps its factual identity after its home is deleted"
    );
}

#[tokio::test]
async fn host_list_rechecks_current_key_rules_instead_of_retaining_previous_admission() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let selected = host(
        &world,
        company.viewer.id(),
        Some(company.acme),
        "Current Host",
    )
    .await;
    let reader = key(
        &world,
        company.viewer.id(),
        vec![rule("HostList", "host", selected, "host")],
    )
    .await;
    assert_eq!(rows(list(&world, &reader.secret, None).await).len(), 1);
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(reader.id)
        .bind(serde_json::to_value(KeyScope::default()).unwrap())
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        list(&world, &reader.secret, None).await.status,
        StatusCode::FORBIDDEN
    );
}

async fn kept_image(world: &World, host: Id, environment: Id, generation: i64) {
    sqlx::query("UPDATE hosts SET kept_images=$2 WHERE id=$1")
        .bind(host)
        .bind(json!({"epoch": 7, "images": [{
            "system": "test", "environment": environment, "generation": generation,
            "capture": new_id(), "reference": format!("orch-keep-{environment}:{generation}"),
            "bytes": 768, "total_bytes": 900, "layers": 2
        }]}))
        .execute(&world.db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn host_list_retained_environment_addresses_use_its_own_name_read() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let selected = host(&world, company.viewer.id(), None, "Layer address Host").await;
    let environment = access_fixtures::insert_resource(
        &mut *world.conn().await,
        "environment",
        Some(company.viewer.id()),
        Some(company.acme),
        "Address source",
    )
    .await
    .unwrap();
    kept_image(&world, selected, environment, 4).await;
    let native = rows(list(&world, &company.viewer.token, None).await);
    let native = native
        .iter()
        .find(|row| row["id"] == json!(selected))
        .unwrap();
    let native_address = native["kept_layers"][0]["environment"]["address"]
        .as_str()
        .unwrap();
    assert!(
        native_address.contains("acme"),
        "the ordinary source has a real org component"
    );
    let mut rules = vec![
        rule("HostList", "host", selected, "host"),
        rule("HostList", "environment", environment, "environment"),
        rule("NodeGet", "org", company.acme, "org"),
    ];
    let without_name = key(&world, company.viewer.id(), rules.clone()).await;
    let page = rows(list(&world, &without_name.secret, None).await);
    let address = page[0]["kept_layers"][0]["environment"]["address"]
        .as_str()
        .unwrap();
    assert!(
        !address.contains("acme"),
        "another operation cannot lend this source's org name"
    );
    rules.push(rule("HostList", "org", company.acme, "org"));
    let with_name = key(&world, company.viewer.id(), rules).await;
    let page = rows(list(&world, &with_name.secret, None).await);
    assert_eq!(
        page[0]["kept_layers"][0]["environment"]["address"],
        native_address
    );
}

#[tokio::test]
async fn host_list_publication_retains_native_layer_generation_and_the_original_key_deadline() {
    // An unchanged source is the positive control for the actual publication rendezvous.
    for change in ["unchanged", "generation", "expiry"] {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let selected = host(&world, company.viewer.id(), None, "Original layer Host").await;
        let environment = new_id();
        kept_image(&world, selected, environment, 4).await;
        let reader = key(
            &world,
            company.viewer.id(),
            vec![rule("HostList", "host", selected, "host")],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let request = tokio::spawn(async move {
            let call = app
                .call(HostList, &[])
                .token(&reader.secret)
                .query(&HostListQuery { org_id: None });
            call.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait, async {
            loop {
                assert!(!request.is_finished(), "the Host body must reach its real activity wait");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the real activity write follows the native Host snapshot");
        if change == "generation" {
            kept_image(&world, selected, environment, 5).await;
        } else if change == "expiry" {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if change == "unchanged" {
            assert_eq!(rows(reply)[0]["kept_layers"][0]["generation"], 4);
        } else {
            assert_eq!(
                reply.status,
                if change == "expiry" {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                }
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("Original layer Host"));
        }
    }
}
