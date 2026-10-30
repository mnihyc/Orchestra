//! An admin's existence authority in the tree: a private place they administer and may not view is its
//! minimal row (identity, structural parent, kind, name, first owner, Private) and nothing else, over HTTP.
//! Who administers its page here is whoever holds `access.manage` on it: an admin of the org reads it
//! outright with As admin on and holds nothing there with it off. A private child is placed under its
//! private parent; nobody else gets such a row; its content stays refused.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::access::{
    HolderInput, PageChange, PageChangeAdd, PageChangeRow, PermissionPageUpdate,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{
    NewNodeAccess, NodeGet, OrgTreeGet, OrgTreeGetQuery, OrgTreeGetReply, Project, ProjectCreate,
    ProjectCreateBody,
};
use orch_types::Id;
use places::{Who, join, make_org, make_topic, person};
use serde_json::json;
use support::World;

async fn tree(world: &World, who: &Who, org: Id) -> OrgTreeGetReply {
    world
        .app
        .call(OrgTreeGet, &[&org])
        .token(&who.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok()
}

async fn private_project(world: &World, by: &Who, org: Id, name: &str) -> Project {
    world
        .app
        .call(ProjectCreate, &[&org])
        .token(&by.token)
        .body(&ProjectCreateBody {
            name: name.to_owned(),
            description: None,
            access: Some(NewNodeAccess::Private),
        })
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn an_admin_reads_a_private_place_in_the_tree_as_its_minimal_row_and_nobody_else_does() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let eve = person(&world, "eve").await;
    let dan = person(&world, "dan").await;
    let rae = person(&world, "rae").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    join(&world, acme.base.id, acme.base.id, &eve, "admin", &boss).await;
    join(&world, acme.base.id, acme.base.id, &dan, "viewer", &boss).await;
    join(&world, acme.base.id, acme.base.id, &rae, "viewer", &boss).await;
    let secret = private_project(&world, &boss, acme.base.id, "Payroll").await;
    let inside = make_topic(&world, &boss, secret.base.id, "Salaries").await;
    // Dan may change who reaches both pages, and is named in nothing that lets him look in.
    for node in [secret.base.id, inside.base.id] {
        manages_the_page(&world, &boss, node, &dan).await;
    }

    let seen_by_dan = tree(&world, &dan, acme.base.id).await;
    let hidden = seen_by_dan.hidden();
    let project = hidden
        .iter()
        .find(|row| row.id == secret.base.id)
        .expect("whoever manages its page learns that the private project exists");
    assert_eq!(
        (
            project.kind.as_str(),
            project.name.as_str(),
            project.parent_id
        ),
        ("project", "Payroll", Some(acme.base.id))
    );
    assert_eq!(
        project.owner.as_ref().map(|owner| owner.id),
        Some(boss.id())
    );
    assert!(project.content_hidden && project.private);
    let topic = hidden
        .iter()
        .find(|row| row.id == inside.base.id)
        .expect("what lies in it is placed under it");
    assert_eq!(topic.parent_id, Some(secret.base.id));
    assert!(
        seen_by_dan
            .readable()
            .into_iter()
            .all(|node| node.id != secret.base.id && node.id != inside.base.id),
        "no readable row of either"
    );

    // The minimal row holds its six facts and the two flags, and nothing else, not even empty.
    let wire = serde_json::to_value(&seen_by_dan).unwrap();
    let row = wire["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(secret.base.id))
        .unwrap();
    let mut keys: Vec<&str> = row
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "content_hidden",
            "id",
            "kind",
            "name",
            "owner",
            "parent_id",
            "private"
        ]
    );

    // Somebody who manages nothing here gets no such row, and neither does the org's admin while As
    // admin is off; with it on, the admin reads the place itself.
    for nobody in [&rae, &eve] {
        let seen = tree(&world, nobody, acme.base.id).await;
        assert!(seen.hidden().is_empty(), "{}", nobody.person.username);
        assert!(
            seen.readable()
                .into_iter()
                .all(|node| node.id != secret.base.id),
            "{}",
            nobody.person.username
        );
    }
    places::admin_mode(&world, &eve, true).await;
    let administering = tree(&world, &eve, acme.base.id).await;
    assert!(administering.hidden().is_empty());
    assert!(
        administering
            .readable()
            .into_iter()
            .any(|node| node.id == secret.base.id)
    );

    // Its content stays refused to whoever has only the row: the row is existence, not a way in.
    let refused = world
        .app
        .call(NodeGet, &[&secret.base.id])
        .token(&dan.token)
        .send()
        .await
        .problem();
    assert!(
        matches!(refused.code, ErrorCode::AuthzDenied | ErrorCode::NotFound),
        "{refused:?}"
    );
}

/// Names `who` in the row that changes the page of `node`, and in no other.
async fn manages_the_page(world: &World, by: &Who, node: Id, who: &Who) {
    world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &node])
        .token(&by.token)
        .body(&PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "access.manage".to_owned(),
                profile: None,
                inherit: None,
                locked: None,
                add: Some(vec![PageChangeAdd {
                    holder: HolderInput::User { id: who.id() },
                    options: None,
                    expires_at: None,
                    note: None,
                }]),
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
}
