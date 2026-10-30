//! Permission-page inspection is scoped separately from reading private content.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use orch_api_contract::access::{
    EffectiveAccessGet, EffectiveAccessGetQuery, HistoryList, HistoryListQuery, PermissionPageGet,
    PermissionPageUpdate, PermissionPeopleList, PermissionPeopleListQuery,
};
use orch_api_contract::auth::{ApiTokenCreateBody, MeTokenCreate};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::DocumentGet;
use orch_api_contract::nodes::NodeGet;
use orch_types::Id;
use places::{Who, make_project};
use serde_json::json;

async fn credential(desk: &Desk, owner: &Who, node: Id, scopes: &[&str]) -> String {
    desk.world
        .app
        .call(MeTokenCreate, &[])
        .token(&owner.token)
        .body(&ApiTokenCreateBody {
            name: "Permission inspector".to_owned(),
            scopes: Some(scopes.iter().map(|scope| (*scope).to_owned()).collect()),
            node_id: Some(node),
            expires_at: None,
            proof: orch_api_contract::auth::SensitiveProof {
                password: Some(support::PASSWORD.to_owned()),
                ..Default::default()
            },
        })
        .send()
        .await
        .ok()
        .token
        .expect("the first answer carries the secret")
}

async fn inspect(desk: &Desk, object: &str, id: Id, user: Id, token: &str, view: &str) {
    let page = desk
        .world
        .app
        .call(PermissionPageGet, &[&object, &id])
        .token(token)
        .send()
        .await
        .ok();
    assert_eq!(page.object.id, id);
    assert_eq!(page.owner.unwrap().id, desk.ada.id());
    let effective = desk
        .world
        .app
        .call(EffectiveAccessGet, &[&object, &id])
        .token(token)
        .query(&EffectiveAccessGetQuery { user_id: user })
        .send()
        .await
        .ok();
    assert!(
        effective
            .permissions
            .iter()
            .any(|held| held.permission == "access.manage")
    );
    assert!(
        effective
            .permissions
            .iter()
            .all(|held| held.permission != view)
    );
    let people = desk
        .world
        .app
        .call(PermissionPeopleList, &[&object, &id])
        .token(token)
        .query(&PermissionPeopleListQuery {
            holder: "admins".to_owned(),
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .ok();
    assert!(people.items.iter().any(|person| person.id == desk.ada.id()));
    let history = desk
        .world
        .app
        .call(HistoryList, &[&object, &id])
        .token(token)
        .query(&HistoryListQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(history.code, ErrorCode::AuthzDenied);
    let denied = if object == "nodes" {
        desk.world
            .app
            .call(NodeGet, &[&id])
            .token(token)
            .send()
            .await
            .problem()
    } else {
        desk.world
            .app
            .call(DocumentGet, &[&id])
            .token(token)
            .send()
            .await
            .problem()
    };
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn an_explicit_page_manager_without_native_mode_reads_the_page_but_no_content_or_history() {
    let desk = Desk::new().await;
    let admin = desk.member("admin", "admin").await;
    let manager = desk.member("manager", "viewer").await;
    let viewer = desk.member("viewer", "viewer").await;
    let denied = desk.member("denied", "viewer").await;
    let document = desk.make(&desk.ada, "Private words").await.base.id;
    for (object, id, view) in [
        ("nodes", desk.project, "project.view"),
        ("documents", document, "doc.view"),
    ] {
        desk.world.app.call(PermissionPageUpdate, &[&object, &id])
            .token(&desk.ada.token).raw_body(&json!({
                "whole": "private", "rows": [
                    {"permission": "access.manage", "add": [{"holder": {"kind": "user", "id": manager.id()}}, {"holder": {"kind": "user", "id": admin.id()}}]},
                    {"permission": view, "add": [{"holder": {"kind": "user", "id": viewer.id()}}]}
                ]
            })).send().await.ok();
        for who in [&admin, &manager] {
            inspect(&desk, object, id, who.id(), &who.token, view).await;
        }
        desk.world
            .app
            .call(PermissionPageGet, &[&object, &id])
            .token(&viewer.token)
            .send()
            .await
            .ok();
        let refusal = desk
            .world
            .app
            .call(PermissionPageGet, &[&object, &id])
            .token(&denied.token)
            .send()
            .await
            .problem();
        assert_eq!(refusal.code, ErrorCode::AuthzDenied);
        if object == "nodes" {
            desk.world
                .app
                .call(NodeGet, &[&id])
                .token(&viewer.token)
                .send()
                .await
                .ok();
        } else {
            desk.world
                .app
                .call(DocumentGet, &[&id])
                .token(&viewer.token)
                .send()
                .await
                .ok();
        }
    }
}

#[tokio::test]
async fn private_page_inspection_through_administration_is_never_a_tokens_whatever_its_scopes_or_flags()
 {
    let desk = Desk::new().await;
    let admin = desk.member("admin", "admin").await;
    let other = make_project(&desk.world, &desk.ada, desk.org, "Elsewhere")
        .await
        .base
        .id;
    let system_admin = places::admin(&desk.world, "systemadmin").await;
    let document = desk.make(&desk.ada, "Private words").await.base.id;
    // An admin acts as one only in a browser session with As admin on: a token never does,
    // whatever its scopes, its subtree or its person's flags say.
    let scoped = credential(&desk, &admin, desk.project, &["access.manage"]).await;
    let view_only = credential(&desk, &admin, desk.project, &["project.view", "doc.view"]).await;
    let elsewhere = credential(&desk, &admin, other, &["access.manage"]).await;
    let flag_only = credential(&desk, &system_admin, desk.project, &["access.manage"]).await;
    for (object, id) in [("nodes", desk.project), ("documents", document)] {
        desk.world
            .app
            .call(PermissionPageUpdate, &[&object, &id])
            .token(&desk.ada.token)
            .raw_body(&json!({"whole": "private"}))
            .send()
            .await
            .ok();
        for token in [&scoped, &view_only, &elsewhere, &flag_only] {
            let problem = desk
                .world
                .app
                .call(PermissionPageGet, &[&object, &id])
                .token(token)
                .send()
                .await
                .problem();
            assert_eq!(problem.code, ErrorCode::AuthzDenied);
            let problem = desk
                .world
                .app
                .call(PermissionPeopleList, &[&object, &id])
                .token(token)
                .query(&PermissionPeopleListQuery {
                    holder: "admins".to_owned(),
                    limit: None,
                    cursor: None,
                })
                .send()
                .await
                .problem();
            assert_eq!(problem.code, ErrorCode::AuthzDenied);
        }
    }
}

#[tokio::test]
async fn effective_self_reports_only_the_actual_tokens_permissions() {
    let desk = Desk::new().await;
    let scoped = credential(&desk, &desk.ada, desk.project, &["project.view"]).await;
    let effective = desk
        .world
        .app
        .call(EffectiveAccessGet, &[&"nodes", &desk.project])
        .token(&scoped)
        .query(&EffectiveAccessGetQuery {
            user_id: desk.ada.id(),
        })
        .send()
        .await
        .ok();
    let permissions: Vec<&str> = effective
        .permissions
        .iter()
        .map(|held| held.permission.as_str())
        .collect();
    assert_eq!(permissions, ["project.view"]);
}
