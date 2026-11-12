//! Organization reads use the selected operation's real objects and optional reference sources.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use crate::support::FullPermissionPageReply;
use chrono::Duration;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::PermissionPageUpdate;
use orch_api_contract::documents::NodeDocumentCreate;
use orch_api_contract::nodes::{NodeUpdate, OrgBySlugGet, OrgList};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity;
use orch_types::{Id, Json, new_id};
use places::{Who, admin, join, make_department, make_org, person};
use serde_json::json;
use support::World;

fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}

async fn key(world: &World, owner: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: owner,
            name: "Org reader control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}

fn success(reply: RawReply) -> Json {
    success_status(reply, StatusCode::OK)
}

fn success_status(reply: RawReply, expected: StatusCode) -> Json {
    assert_eq!(
        reply.status,
        expected,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn list(world: &World, token: &str) -> RawReply {
    world.app.call(OrgList, &[]).token(token).send_bytes().await
}

async fn by_slug(world: &World, token: &str, address: &str) -> RawReply {
    world
        .app
        .call(OrgBySlugGet, &[&address])
        .token(token)
        .send_bytes()
        .await
}

async fn org_read(world: &World, token: &str, operation: &str, address: &str) -> Json {
    if operation == "OrgList" {
        success(list(world, token).await)[0].clone()
    } else {
        success(by_slug(world, token, address).await)
    }
}

async fn office() -> (World, Who, orch_api_contract::nodes::Org) {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    (world, owner, org)
}

#[tokio::test]
async fn org_list_filters_actual_objects_before_projection_and_preserves_the_ordinary_order() {
    let (world, owner, org) = office().await;
    let other = make_org(&world, &owner, "Other organization", "other").await;
    let ordinary = success(list(&world, &owner.token).await);
    let token = key(
        &world,
        owner.id(),
        vec![rule("OrgList", "org", org.base.id)],
    )
    .await;
    let scoped = success(list(&world, &token).await);
    assert_eq!(scoped.as_array().unwrap().len(), 1);
    assert_eq!(
        scoped[0],
        ordinary
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == json!(org.base.id))
            .unwrap()
            .clone()
    );
    let token = key(
        &world,
        owner.id(),
        vec![
            rule("OrgList", "org", owner.person.personal_org),
            rule("OrgList", "org", other.base.id),
        ],
    )
    .await;
    let scoped = success(list(&world, &token).await);
    let expected: Vec<_> = ordinary
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| {
            row["id"] == json!(owner.person.personal_org) || row["id"] == json!(other.base.id)
        })
        .cloned()
        .collect();
    assert_eq!(scoped, json!(expected));
}

#[tokio::test]
async fn org_by_slug_binds_the_actual_root_or_department_and_keeps_exact_operation_identity() {
    let (world, owner, org) = office().await;
    let department = make_department(&world, &owner, org.base.id, "Department", "department").await;
    for (target, address) in [
        (org.base.id, org.slug.as_str()),
        (department.base.id, department.address.as_str()),
    ] {
        let token = key(
            &world,
            owner.id(),
            vec![rule("OrgBySlugGet", "org", target)],
        )
        .await;
        let value = success(by_slug(&world, &token, address).await);
        assert_eq!(value["id"], json!(target));
        assert_eq!(value["owner"]["id"], json!(owner.id()));
        let wrong = key(&world, owner.id(), vec![rule("OrgList", "org", target)]).await;
        assert!(!by_slug(&world, &wrong, address).await.status.is_success());
    }
    let root_only = key(
        &world,
        owner.id(),
        vec![rule("OrgBySlugGet", "org", org.base.id)],
    )
    .await;
    assert!(
        !by_slug(&world, &root_only, &department.address)
            .await
            .status
            .is_success()
    );
    assert_eq!(
        by_slug(&world, &root_only, "missing-org").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn org_readme_needs_its_actual_same_operation_document_read_without_losing_the_org() {
    let (world, owner, org) = office().await;
    let document = success_status(
        world
            .app
            .call(NodeDocumentCreate, &[&org.base.id])
            .token(&owner.token)
            .raw_body(&json!({"title":"README private title","markdown":"Stored README paragraph"}))
            .send_bytes()
            .await,
        StatusCode::CREATED,
    );
    let document_id: Id = serde_json::from_value(document["id"].clone()).unwrap();
    let revision =
        orch_db::queries::hierarchy::with_ancestors(&mut *world.conn().await, &[org.base.id])
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.id == org.base.id)
            .unwrap()
            .revision;
    success(
        world
            .app
            .call(NodeUpdate, &[&org.base.id])
            .token(&owner.token)
            .raw_body(&json!({"readme_document_id":document_id,"expected_revision":revision}))
            .send_bytes()
            .await,
    );
    for operation in ["OrgList", "OrgBySlugGet"] {
        let excluded = key(
            &world,
            owner.id(),
            vec![rule(operation, "org", org.base.id)],
        )
        .await;
        let value = org_read(&world, &excluded, operation, &org.slug).await;
        assert_eq!(value["id"], json!(org.base.id));
        assert!(value.get("readme").is_none_or(Json::is_null));
        let unrelated = key(
            &world,
            owner.id(),
            vec![
                rule(operation, "org", org.base.id),
                rule("DocumentGet", "document", document_id),
            ],
        )
        .await;
        assert!(
            org_read(&world, &unrelated, operation, &org.slug)
                .await
                .get("readme")
                .is_none_or(Json::is_null)
        );
        let included = key(
            &world,
            owner.id(),
            vec![
                rule(operation, "org", org.base.id),
                rule(operation, "document", document_id),
            ],
        )
        .await;
        let value = org_read(&world, &included, operation, &org.slug).await;
        assert_eq!(value["readme"]["id"], json!(document_id));
        assert_eq!(value["readme"]["title"], "README private title");
        assert_eq!(value["readme"]["owner"]["id"], json!(owner.id()));
        assert_eq!(
            value["readme"]["description_line"],
            "Stored README paragraph"
        );
    }
}

#[tokio::test]
async fn department_path_names_keep_typed_hidden_refs_until_this_operation_reads_the_ancestor() {
    let (world, owner, org) = office().await;
    let department = make_department(&world, &owner, org.base.id, "Department", "department").await;
    let token = key(
        &world,
        owner.id(),
        vec![rule("OrgBySlugGet", "org", department.base.id)],
    )
    .await;
    let value = success(by_slug(&world, &token, &department.address).await);
    let root = value["path"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(org.base.id))
        .unwrap();
    assert_eq!(root["redacted"], true);
    assert!(root.get("name").is_none_or(Json::is_null));
    assert_eq!(value["owner"]["id"], json!(owner.id()));
    let token = key(
        &world,
        owner.id(),
        vec![
            rule("OrgBySlugGet", "org", department.base.id),
            rule("OrgBySlugGet", "org", org.base.id),
        ],
    )
    .await;
    let value = success(by_slug(&world, &token, &department.address).await);
    let root = value["path"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(org.base.id))
        .unwrap();
    assert_eq!(root["name"], "Office");
    assert_eq!(root["redacted"], false);
}

#[tokio::test]
async fn org_readers_apply_current_ordinary_view_loss_without_naming_a_hidden_member_org() {
    let (world, owner, org) = office().await;
    let viewer = person(&world, "viewer").await;
    join(&world, org.base.id, org.base.id, &viewer, "viewer", &owner).await;
    let token = key(
        &world,
        viewer.id(),
        vec![
            rule("OrgList", "org", org.base.id),
            rule("OrgBySlugGet", "org", org.base.id),
        ],
    )
    .await;
    assert_eq!(
        success(list(&world, &token).await)[0]["id"],
        json!(org.base.id)
    );
    assert_eq!(
        success(by_slug(&world, &token, &org.slug).await)["id"],
        json!(org.base.id)
    );
    world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &org.base.id])
        .token(&owner.token)
        .raw_body(&json!({"rows":[{"permission":"org.view","inherit":false}]}))
        .send()
        .await
        .full_permission_page()
        .ok();
    assert_eq!(success(list(&world, &token).await), json!([]));
    assert!(!by_slug(&world, &token, &org.slug).await.status.is_success());
}

#[tokio::test]
async fn org_reader_keys_do_not_borrow_the_accounts_browser_as_admin_private_reach() {
    let (world, owner, org) = office().await;
    world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &org.base.id])
        .token(&owner.token)
        .raw_body(&json!({"whole":"private"}))
        .send()
        .await
        .full_permission_page()
        .ok();
    let operator = admin(&world, "operator").await;
    assert_eq!(
        success(by_slug(&world, &operator.token, &org.slug).await)["id"],
        json!(org.base.id)
    );
    let token = key(
        &world,
        operator.id(),
        vec![
            rule("OrgList", "org", org.base.id),
            rule("OrgBySlugGet", "org", org.base.id),
        ],
    )
    .await;
    assert_eq!(success(list(&world, &token).await), json!([]));
    assert!(!by_slug(&world, &token, &org.slug).await.status.is_success());
}
