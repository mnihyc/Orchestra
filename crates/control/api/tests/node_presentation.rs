//! Following document counts in large trees, node icon colours, child kind filters and personal sidebar state over HTTP.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use orch_api_contract::access::{PageChange, PageChangeWhole, PermissionPageUpdate};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{NodeDocumentList, NodeDocumentListQuery};
use orch_api_contract::nodes::{
    Node, NodeChildList, NodeChildListQuery, NodeKind, NodeUpdate, NodeUpdateBody, OrgTreeGet,
    OrgTreeGetQuery, OrgTreeGetReply, ProjectChildCreate, ProjectCreateBody,
};
use orch_api_contract::personal::{MeStateGet, MeStateSet};
use orch_settings::Target;
use orch_types::{Id, new_id};
use places::{Who, join, make_org, make_project, make_topic, person};
use serde_json::json;
use support::{Statements, World};

/// One thousand nodes exercise a whole tree; two hundred document-bearing nodes exercise aggregation.
const TREE_NODES: usize = 1_000;
const DOCUMENT_NODES: usize = 200;

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

#[tokio::test]
async fn a_thousand_nodes_with_documents_beside_two_hundred_cost_the_same_statements_as_without_documents()
 {
    let world = World::new().await;
    let maker = person(&world, "ada").await;
    let reader = person(&world, "bea").await;
    let org = make_org(&world, &maker, "Office", "office").await.base.id;
    join(&world, org, org, &reader, "viewer", &maker).await;
    // The root and its General project already exist; the remainder is made in one fixture statement.
    let ids: Vec<Id> = (0..TREE_NODES - 2).map(|_| new_id()).collect();
    sqlx::query("INSERT INTO nodes (id, kind, parent_id, org_id, path, name, owner_id, created_by, created_at, updated_at)
        SELECT id, 'project', $2, $2, ARRAY[$2, id], 'Project', $3, $3, $4, $4 FROM unnest($1::uuid[]) AS id")
        .bind(&ids).bind(org).bind(maker.id()).bind(world.now()).execute(&world.db.pool).await.unwrap();
    let call = world
        .app
        .call(OrgTreeGet, &[&org])
        .token(&reader.token)
        .query(&OrgTreeGetQuery::default());
    let before = Statements::now();
    let empty = call.send().await.ok();
    let empty_statements = Statements::now() - before;
    assert_eq!(empty.readable().len(), TREE_NODES);
    assert_eq!(
        empty
            .readable()
            .into_iter()
            .map(|row| row.docs)
            .sum::<i64>(),
        0
    );
    let documents: Vec<Id> = (0..DOCUMENT_NODES).map(|_| new_id()).collect();
    sqlx::query("INSERT INTO documents (id, node_id, org_id, owner_id, title, created_at, updated_at)
        SELECT id, node, $3, $4, 'Notes', $5, $5 FROM unnest($1::uuid[], $2::uuid[]) AS d(id, node)")
        .bind(&documents).bind(&ids[..DOCUMENT_NODES]).bind(org).bind(maker.id()).bind(world.now())
        .execute(&world.db.pool).await.unwrap();
    let call = world
        .app
        .call(OrgTreeGet, &[&org])
        .token(&reader.token)
        .query(&OrgTreeGetQuery::default());
    let before = Statements::now();
    let full = call.send().await.ok();
    let full_statements = Statements::now() - before;
    assert_eq!(full.readable().len(), TREE_NODES);
    assert_eq!(
        full.readable()
            .into_iter()
            .filter(|row| row.docs == 1)
            .count(),
        DOCUMENT_NODES
    );
    assert_eq!(empty_statements, full_statements);
    eprintln!(
        "tree: {TREE_NODES} nodes, 0/{} documents, {empty_statements}/{full_statements} statements",
        documents.len()
    );
}

#[tokio::test]
async fn a_private_document_is_not_counted_and_is_still_listed_for_its_reader() {
    let office = Desk::new().await;
    let reader = office.member("bea", "viewer").await;
    let doc = office.make(&office.ada, "Notes").await.base.id;
    assert_eq!(
        tree(&office.world, &reader, office.org)
            .await
            .readable()
            .into_iter()
            .find(|row| row.id == office.topic)
            .unwrap()
            .docs,
        1
    );
    office
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc])
        .token(&office.ada.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    for who in [&office.ada, &reader] {
        assert_eq!(
            tree(&office.world, who, office.org)
                .await
                .readable()
                .into_iter()
                .find(|row| row.id == office.topic)
                .unwrap()
                .docs,
            0
        );
    }
    let listed = |who: &Who| {
        office
            .world
            .app
            .call(NodeDocumentList, &[&office.topic])
            .token(&who.token)
            .query(&NodeDocumentListQuery::default())
            .send()
    };
    assert_eq!(
        listed(&office.ada).await.ok().items[0]
            .as_visible()
            .unwrap()
            .id,
        doc
    );
    assert!(listed(&reader).await.ok().items.is_empty());
}

#[tokio::test]
async fn a_tree_counts_only_document_rows_that_its_reader_holds() {
    let office = Desk::new().await;
    let reader = office.member("bea", "viewer").await;
    office.make(&office.ada, "Notes").await;
    sqlx::query(
        "INSERT INTO permission_rows (object_kind, object_id, page, profile, permission, inherit)
        VALUES ('node', $1, 'live', 'any', 'doc.view', false)",
    )
    .bind(office.topic)
    .execute(&office.world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        tree(&office.world, &office.ada, office.org)
            .await
            .readable()
            .into_iter()
            .find(|row| row.id == office.topic)
            .unwrap()
            .docs,
        1
    );
    assert_eq!(
        tree(&office.world, &reader, office.org)
            .await
            .readable()
            .into_iter()
            .find(|row| row.id == office.topic)
            .unwrap()
            .docs,
        0
    );
}

#[tokio::test]
async fn only_a_manager_changes_a_colour_and_a_stale_colour_change_changes_nothing() {
    let world = World::new().await;
    let maker = person(&world, "ada").await;
    let driver = person(&world, "bea").await;
    let org = make_org(&world, &maker, "Office", "office").await.base.id;
    let project = make_project(&world, &maker, org, "Work").await;
    join(&world, org, org, &driver, "interactor", &maker).await;
    let change = |who: &Who, color, revision, name| {
        world
            .app
            .call(NodeUpdate, &[&project.base.id])
            .token(&who.token)
            .body(&NodeUpdateBody {
                readme_document_id: None,
                color,
                custom_color: None,
                expected_revision: revision,
                name,
                description: None,
            })
            .send()
    };
    let denied = change(&driver, Some(Some(3)), project.base.revision, None)
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    assert_eq!(denied.extra["required"], "project.manage");
    for invalid in [0, 13] {
        let denied = change(&maker, Some(Some(invalid)), project.base.revision, None)
            .await
            .problem();
        assert_eq!(denied.code, ErrorCode::RequestInvalid);
        assert_eq!(denied.extra["errors"][0]["path"], "color");
    }
    let Node::Project(set) = change(&maker, Some(Some(12)), project.base.revision, None)
        .await
        .ok()
    else {
        panic!("a project")
    };
    assert_eq!(set.base.color, Some(12));
    assert_eq!(
        tree(&world, &driver, org)
            .await
            .readable()
            .into_iter()
            .find(|row| row.id == project.base.id)
            .unwrap()
            .color,
        Some(12)
    );
    let stale = change(
        &maker,
        Some(Some(2)),
        project.base.revision,
        Some("Changed".into()),
    )
    .await
    .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current"]["color"], 12);
    assert_eq!(stale.extra["current"]["name"], "Work");
    let Node::Project(clear) = change(&maker, Some(None), set.base.revision, None)
        .await
        .ok()
    else {
        panic!("a project")
    };
    assert_eq!(clear.base.color, None);
    let Node::Project(renamed) = change(&maker, None, clear.base.revision, Some("Renamed".into()))
        .await
        .ok()
    else {
        panic!("a project")
    };
    assert_eq!(renamed.base.color, None);
}

#[tokio::test]
async fn new_places_use_the_kind_colour_unless_the_parent_resolves_random() {
    let world = World::new().await;
    let maker = person(&world, "ada").await;
    let org = make_org(&world, &maker, "Office", "office").await.base.id;
    assert_eq!(
        make_project(&world, &maker, org, "As kind")
            .await
            .base
            .color,
        None
    );
    world
        .set_at(Target::Node(org), "nodes.new_color", json!("random"))
        .await;
    let project = make_project(&world, &maker, org, "Random").await;
    assert!((1..=12).contains(&project.base.color.unwrap()));
    let topic = make_topic(&world, &maker, project.base.id, "Inherited").await;
    assert!((1..=12).contains(&topic.base.color.unwrap()));
    world
        .set_at(
            Target::Node(project.base.id),
            "nodes.new_color",
            json!("kind"),
        )
        .await;
    assert_eq!(
        make_topic(&world, &maker, project.base.id, "Nearest")
            .await
            .base
            .color,
        None
    );
}

#[tokio::test]
async fn a_child_kind_filter_pages_folders_without_consuming_topics_and_refuses_impossible_kinds() {
    let office = Desk::new().await;
    let mut folders = Vec::new();
    for name in ["First", "Second", "Third"] {
        let made = office
            .world
            .app
            .call(ProjectChildCreate, &[&office.project])
            .token(&office.ada.token)
            .body(&ProjectCreateBody {
                name: name.into(),
                description: None,
                access: None,
            })
            .send()
            .await
            .ok();
        folders.push(made.base.id);
        make_topic(&office.world, &office.ada, office.project, "Between").await;
    }
    let mut cursor = None;
    let mut listed = Vec::new();
    loop {
        let page = office
            .world
            .app
            .call(NodeChildList, &[&office.project])
            .token(&office.ada.token)
            .query(&NodeChildListQuery {
                kind: Some(NodeKind::Project),
                limit: Some(1),
                cursor,
                archived: None,
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert!(
            page.items
                .iter()
                .all(|row| matches!(row.as_visible(), Some(Node::Project(_))))
        );
        listed.extend(page.items.iter().map(|row| {
            orch_api_core::routes::nodes::node_id(
                row.as_visible().expect("the maker can read each child"),
            )
        }));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(listed, folders);
    let refused = office
        .world
        .app
        .call(NodeChildList, &[&office.project])
        .token(&office.ada.token)
        .query(&NodeChildListQuery {
            kind: Some(NodeKind::Org),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn sidebar_state_is_personal_and_a_replacement_replaces_the_whole_key() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    for value in [
        json!({"folded": ["recent", "starred"], "documents": false}),
        json!({"folded": [], "documents": true}),
    ] {
        world
            .app
            .call(MeStateSet, &[&"sidebar"])
            .token(&ada.token)
            .body(&value)
            .send()
            .await
            .ok();
        let seen = world
            .app
            .call(MeStateGet, &[])
            .token(&ada.token)
            .send()
            .await
            .ok();
        assert_eq!(seen["sidebar"], value);
        assert!(
            !world
                .app
                .call(MeStateGet, &[])
                .token(&bea.token)
                .send()
                .await
                .ok()
                .contains_key("sidebar")
        );
    }
    assert_eq!(
        world
            .app
            .call(MeStateSet, &[&"not-a-key"])
            .token(&ada.token)
            .body(&json!({}))
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
}
