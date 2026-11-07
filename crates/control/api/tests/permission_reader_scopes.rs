//! Real operation-scoped credentials retain the existing permission-page inspection authority.
#![recursion_limit = "256"]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod rig;
mod support;

use crate::support::FullPermissionPageReply;
use desk::Desk;
use orch_api_contract::access::{
    EffectiveAccess, EffectiveAccessGet, EffectiveAccessGetQuery, Holder, HolderInput,
    NodeDefaultsGet, PermissionCatalogueGet, PermissionPage, PermissionPageGet,
    PermissionPageUpdate, PermissionPeopleList, PermissionPeopleListQuery,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{GroupHolderAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_api_contract::documents::DocumentGet;
use orch_api_contract::nodes::{NodeGet, NodeOrSystem};
use orch_api_core::testing::Reply;
use orch_types::{Id, new_id};
use places::Who;
use serde_json::{Value, json};
use support::World;

fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}

fn system_rule(operation: &str) -> Value {
    json!({"target":{"kind":"system","reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}

async fn key(world: &World, owner: Id, rules: Vec<Value>) -> String {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let scope = serde_json::to_value(scope).unwrap();
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let now = world.now();
    create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: owner,
            name: "Permission reader control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: now,
            expires_at: Some(now + chrono::Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    secret
}

async fn page(world: &World, token: &str, object: &str, id: Id) -> Reply<PermissionPage> {
    world
        .app
        .call(PermissionPageGet, &[&object, &id])
        .token(token)
        .send()
        .await
}

async fn effective(
    world: &World,
    token: &str,
    object: &str,
    id: Id,
    user: Id,
) -> Reply<EffectiveAccess> {
    world
        .app
        .call(EffectiveAccessGet, &[&object, &id])
        .token(token)
        .query(&EffectiveAccessGetQuery { user_id: user })
        .send()
        .await
}

async fn private_manager(desk: &Desk, object: &str, id: Id, manager: &Who) {
    desk.world
        .app
        .call(PermissionPageUpdate, &[&object, &id])
        .token(&desk.ada.token)
        .raw_body(&json!({
            "whole":"private",
            "rows":[{"permission":"access.manage","add":[{"holder":{"kind":"user","id":manager.id()}}]}]
        }))
        .send()
        .await.full_permission_page()
        .ok();
}

#[tokio::test]
async fn scoped_manage_inspects_private_node_and_document_without_granting_content_view() {
    let desk = Desk::new().await;
    let manager = desk.member("page-manager", "viewer").await;
    let document = desk.make(&desk.ada, "Private document body").await.base.id;
    for (object, kind, id, content_operation, required_view) in [
        (
            "nodes",
            "project",
            desk.project,
            "NodeGet.read",
            "project.view",
        ),
        (
            "documents",
            "document",
            document,
            "DocumentGet.read",
            "doc.view",
        ),
    ] {
        private_manager(&desk, object, id, &manager).await;
        let ordinary = page(&desk.world, &manager.token, object, id).await.ok();
        assert_eq!(ordinary.object.id, id);
        let token = key(
            &desk.world,
            manager.id(),
            vec![
                rule("PermissionPageGet.inspect", kind, id, &["manage"]),
                rule("PermissionPeopleList.inspect", kind, id, &["manage"]),
                // The content refusal below must come from ordinary authority, not an absent
                // endpoint selection. There is deliberately no Account Read rule.
                rule(content_operation, kind, id, &["read"]),
            ],
        )
        .await;
        let inspected = page(&desk.world, &token, object, id).await.ok();
        assert_eq!(inspected.object, ordinary.object);
        assert_eq!(inspected.owner.as_ref().unwrap().id, desk.ada.id());
        assert_eq!(inspected.rows.len(), ordinary.rows.len());
        assert_eq!(inspected.admins.count, ordinary.admins.count);
        assert_eq!(inspected.covers, ordinary.covers);
        let people = desk
            .world
            .app
            .call(PermissionPeopleList, &[&object, &id])
            .token(&token)
            .query(&PermissionPeopleListQuery {
                holder: "admins".into(),
                limit: None,
                cursor: None,
            })
            .send()
            .await
            .ok();
        assert!(people.items.iter().any(|person| person.id == desk.ada.id()));
        for (browser, token) in [(true, &manager.token), (false, &token)] {
            let refused = if object == "nodes" {
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
            assert_eq!(refused.code, ErrorCode::AuthzDenied);
            // DocumentGet has no scoped operation binding yet. Its key refusal may be
            // an additional scope denial, which does not qualify its ordinary View barrier.
            // The browser and the already-bound NodeGet key still prove missing View.
            if browser || object == "nodes" {
                assert_eq!(
                    refused.extra.get("required"),
                    Some(&json!(required_view)),
                    "the browser and bound node key reach the ordinary missing-View barrier"
                );
            }
        }
        let view_only = key(
            &desk.world,
            manager.id(),
            vec![rule("PermissionPageGet.view", kind, id, &["read"])],
        )
        .await;
        assert_eq!(
            page(&desk.world, &view_only, object, id)
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied,
            "the key cannot relabel ordinary page inspection as content viewing"
        );
    }
}

#[tokio::test]
async fn effective_self_and_other_inspection_keep_their_exact_selected_operation() {
    let desk = Desk::new().await;
    let manager = desk.member("effective-manager", "viewer").await;
    let document = desk
        .make(&desk.ada, "Only its authority is visible")
        .await
        .base
        .id;
    private_manager(&desk, "documents", document, &manager).await;
    let ordinary = effective(
        &desk.world,
        &manager.token,
        "documents",
        document,
        manager.id(),
    )
    .await
    .ok();
    assert!(
        ordinary
            .permissions
            .iter()
            .any(|p| p.permission == "access.manage")
    );
    assert!(
        ordinary
            .permissions
            .iter()
            .all(|p| p.permission != "doc.view")
    );
    let own = key(
        &desk.world,
        manager.id(),
        vec![rule(
            "EffectiveAccessGet.self",
            "document",
            document,
            &["read"],
        )],
    )
    .await;
    let inspected = effective(&desk.world, &own, "documents", document, manager.id())
        .await
        .ok();
    assert!(
        inspected
            .permissions
            .iter()
            .any(|p| p.permission == "access.manage")
    );
    assert!(
        inspected
            .permissions
            .iter()
            .all(|p| p.permission != "doc.view")
    );
    assert_eq!(
        effective(&desk.world, &own, "documents", document, desk.ada.id())
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied,
        "a self selection never borrows the requested other person's authority"
    );
    let other = key(
        &desk.world,
        manager.id(),
        vec![rule(
            "EffectiveAccessGet.inspect",
            "document",
            document,
            &["manage"],
        )],
    )
    .await;
    let expected = effective(
        &desk.world,
        &manager.token,
        "documents",
        document,
        desk.ada.id(),
    )
    .await
    .ok();
    let owner = effective(&desk.world, &other, "documents", document, desk.ada.id())
        .await
        .ok();
    assert_eq!(owner.user.id, desk.ada.id());
    assert_eq!(
        owner
            .permissions
            .iter()
            .map(|p| &p.permission)
            .collect::<Vec<_>>(),
        expected
            .permissions
            .iter()
            .map(|p| &p.permission)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        effective(&desk.world, &other, "documents", document, manager.id())
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied,
        "self remains the Read variant even when the account has only ordinary page management"
    );
}

#[tokio::test]
async fn full_group_holder_payload_needs_its_actual_same_operation_source_read() {
    let desk = Desk::new().await;
    let member = desk.member("group-member", "viewer").await;
    let group = desk
        .world
        .app
        .call(OrgGroupCreate, &[&desk.org])
        .token(&desk.ada.token)
        .body(&OrgGroupCreateBody {
            name: "Named permission group".into(),
            description: Some("A real group description, not an invented placeholder".into()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response");
    desk.world
        .app
        .call(GroupHolderAdd, &[&group.id])
        .token(&desk.ada.token)
        .body(&orch_api_contract::directory::GroupHolderAddBody {
            holder: HolderInput::User { id: member.id() },
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let document = desk.make(&desk.ada, "Group authority source").await.base.id;
    desk.world.app.call(PermissionPageUpdate, &[&"documents", &document]).token(&desk.ada.token)
        .raw_body(&json!({"rows":[{"permission":"doc.view","add":[{"holder":{"kind":"group","id":group.id},"note":"Retain this ACL entry"}]}]}))
        .send().await.full_permission_page().ok();
    let ordinary = page(&desk.world, &desk.ada.token, "documents", document)
        .await
        .ok();
    let expected_entry = ordinary
        .rows
        .iter()
        .flat_map(|row| &row.entries)
        .find(
            |entry| matches!(&entry.holder, Holder::Group { group: shown } if shown.id == group.id),
        )
        .unwrap();
    let Holder::Group { group: expected } = &expected_entry.holder else {
        unreachable!()
    };
    assert!(expected.member_count > 0);
    let source = rule("PermissionPageGet.view", "document", document, &["read"]);
    for extra in [
        None,
        Some(rule(
            "EffectiveAccessGet.self",
            "group",
            group.id,
            &["read"],
        )),
        Some(rule(
            "PermissionPageGet.inspect",
            "group",
            group.id,
            &["read"],
        )),
    ] {
        let mut rules = vec![source.clone()];
        rules.extend(extra);
        let narrow = key(&desk.world, desk.ada.id(), rules).await;
        assert_eq!(
            page(&desk.world, &narrow, "documents", document)
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied,
            "missing Group Read cannot silently remove the holder or borrow another operation"
        );
    }
    let full = key(
        &desk.world,
        desk.ada.id(),
        vec![
            source,
            rule("PermissionPageGet.view", "group", group.id, &["read"]),
        ],
    )
    .await;
    let shown = page(&desk.world, &full, "documents", document).await.ok();
    let actual_entry = shown
        .rows
        .iter()
        .flat_map(|row| &row.entries)
        .find(|entry| entry.id == expected_entry.id)
        .expect("the same ACL entry remains");
    let Holder::Group { group: actual } = &actual_entry.holder else {
        panic!("the holder remains a full Group")
    };
    assert_eq!(
        (&actual.name, &actual.description, actual.member_count),
        (&expected.name, &expected.description, expected.member_count)
    );
    assert_eq!(actual_entry.note, expected_entry.note);
    assert!(
        actual.place.is_none(),
        "the ungranted group-home path is independently hidden"
    );
    // The primary View page grant alone is incomplete, but this same key has a complete
    // authorized Manage-inspection path. Do not lock selection to the incomplete View path.
    let alternative = key(
        &desk.world,
        desk.ada.id(),
        vec![
            rule("PermissionPageGet.view", "document", document, &["read"]),
            rule(
                "PermissionPageGet.inspect",
                "document",
                document,
                &["manage"],
            ),
            rule("PermissionPageGet.inspect", "group", group.id, &["read"]),
        ],
    )
    .await;
    let inspected = page(&desk.world, &alternative, "documents", document)
        .await
        .ok();
    let entry = inspected
        .rows
        .iter()
        .flat_map(|row| &row.entries)
        .find(|entry| entry.id == expected_entry.id)
        .expect("inspection preserves the real group entry");
    let Holder::Group { group: actual } = &entry.holder else {
        unreachable!()
    };
    assert_eq!(actual.name, expected.name);
    assert_eq!(actual.description, expected.description);
    assert_eq!(actual.member_count, expected.member_count);
}

#[tokio::test]
async fn defaults_and_constant_catalogue_keep_exact_operation_and_target_bounds() {
    let desk = Desk::new().await;
    let expected = desk
        .world
        .app
        .call(NodeDefaultsGet, &[&desk.project])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let token = key(
        &desk.world,
        desk.ada.id(),
        vec![
            rule("NodeDefaultsGet", "project", desk.project, &["read"]),
            system_rule("PermissionCatalogueGet"),
        ],
    )
    .await;
    let shown = desk
        .world
        .app
        .call(NodeDefaultsGet, &[&desk.project])
        .token(&token)
        .send()
        .await
        .ok();
    assert_eq!(shown.object, expected.object);
    assert_eq!(shown.followers, expected.followers);
    assert_eq!(
        shown
            .rows
            .iter()
            .map(|row| &row.permission)
            .collect::<Vec<_>>(),
        expected
            .rows
            .iter()
            .map(|row| &row.permission)
            .collect::<Vec<_>>()
    );
    let catalogue = desk
        .world
        .app
        .call(PermissionCatalogueGet, &[])
        .token(&token)
        .send()
        .await
        .ok();
    let ordinary = desk
        .world
        .app
        .call(PermissionCatalogueGet, &[])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(catalogue, ordinary);
    assert!(!catalogue.permissions.is_empty());
    for rules in [
        vec![rule("NodeDefaultsGet", "topic", desk.topic, &["read"])],
        vec![rule(
            "PermissionPageGet.view",
            "project",
            desk.project,
            &["read"],
        )],
    ] {
        let narrow = key(&desk.world, desk.ada.id(), rules).await;
        assert_eq!(
            desk.world
                .app
                .call(NodeDefaultsGet, &[&desk.project])
                .token(&narrow)
                .send()
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied
        );
    }
    for rules in [
        vec![rule("PermissionCatalogueGet", "org", desk.org, &["read"])],
        vec![rule("NodeDefaultsGet", "project", desk.project, &["read"])],
    ] {
        let narrow = key(&desk.world, desk.ada.id(), rules).await;
        assert_eq!(
            desk.world
                .app
                .call(PermissionCatalogueGet, &[])
                .token(&narrow)
                .send()
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied
        );
    }
}

#[tokio::test]
async fn inherited_node_names_are_optional_without_losing_the_authorized_page_rows_or_counts() {
    let desk = Desk::new().await;
    // An explicit inherited entry makes the provenance premise independent of shipped default rows.
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &desk.topic])
        .token(&desk.ada.token)
        .raw_body(&json!({"rows":[{"permission":"doc.view","inherit":false,
            "add":[{"holder":{"kind":"user","id":desk.ada.id()}}]}]}))
        .send()
        .await
        .full_permission_page()
        .ok();
    let document = desk.make(&desk.ada, "Provenance document").await.base.id;
    let ordinary = page(&desk.world, &desk.ada.token, "documents", document)
        .await
        .ok();
    let inherited = ordinary
        .rows
        .iter()
        .find(|row| {
            matches!(
                row.inherited.as_ref().and_then(|inherited| inherited.from.as_ref()),
                Some(NodeOrSystem::Node(node)) if node.id == desk.topic && node.name.is_some()
            )
        })
        .expect("the normal producer actually names this inherited topic");
    let source = rule("PermissionPageGet.view", "document", document, &["read"]);
    let narrow = key(&desk.world, desk.ada.id(), vec![source.clone()]).await;
    let hidden = page(&desk.world, &narrow, "documents", document).await.ok();
    let hidden_row = hidden
        .rows
        .iter()
        .find(|row| row.permission == inherited.permission)
        .unwrap();
    let Some(NodeOrSystem::Node(origin)) = hidden_row.inherited.as_ref().unwrap().from.as_ref()
    else {
        panic!("the provenance identity remains")
    };
    assert_eq!(origin.id, desk.topic);
    assert!(origin.redacted && origin.name.is_none() && origin.thread_display.is_none());
    assert_eq!(hidden.rows.len(), ordinary.rows.len());
    assert_eq!(hidden.admins.count, ordinary.admins.count);
    let full = key(
        &desk.world,
        desk.ada.id(),
        vec![
            source,
            rule("PermissionPageGet.view", "topic", desk.topic, &["read"]),
            rule("PermissionPageGet.view", "project", desk.project, &["read"]),
            rule("PermissionPageGet.view", "org", desk.org, &["read"]),
        ],
    )
    .await;
    let shown = page(&desk.world, &full, "documents", document).await.ok();
    let shown_row = shown
        .rows
        .iter()
        .find(|row| row.permission == inherited.permission)
        .unwrap();
    assert_eq!(shown_row.inherited, inherited.inherited);
}

#[tokio::test]
async fn thread_page_requires_actual_provider_and_environment_sources_before_full_dependency_payloads()
 {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    let ordinary = page(&scene.rig.world, &scene.ada.token, "nodes", thread)
        .await
        .ok();
    assert!(
        ordinary
            .depends
            .iter()
            .any(|dependency| dependency.object.id == scene.on.provider)
    );
    assert_eq!(
        ordinary.reaches.as_ref().unwrap().environment.id,
        scene.on.environment
    );
    let primary = rule("PermissionPageGet.view", "thread", thread, &["read"]);
    for sources in [
        vec![],
        vec![rule(
            "PermissionPageGet.view",
            "provider",
            scene.on.provider,
            &["read"],
        )],
        vec![rule(
            "PermissionPageGet.view",
            "environment",
            scene.on.environment,
            &["read"],
        )],
        vec![
            rule(
                "PermissionPageGet.view",
                "provider",
                scene.on.provider,
                &["read"],
            ),
            rule("PermissionPageGet.view", "environment", new_id(), &["read"]),
        ],
    ] {
        let token = key(
            &scene.rig.world,
            scene.ada.id(),
            std::iter::once(primary.clone()).chain(sources).collect(),
        )
        .await;
        assert_eq!(
            page(&scene.rig.world, &token, "nodes", thread)
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied,
            "both actual sources are required; missing data is not silently omitted"
        );
    }
    let full = key(
        &scene.rig.world,
        scene.ada.id(),
        vec![
            primary,
            rule(
                "PermissionPageGet.view",
                "provider",
                scene.on.provider,
                &["read"],
            ),
            rule(
                "PermissionPageGet.view",
                "environment",
                scene.on.environment,
                &["read"],
            ),
        ],
    )
    .await;
    let shown = page(&scene.rig.world, &full, "nodes", thread).await.ok();
    assert_eq!(shown.reaches, ordinary.reaches);
    assert_eq!(shown.depends.len(), ordinary.depends.len());
    for expected in &ordinary.depends {
        let actual = shown
            .depends
            .iter()
            .find(|actual| {
                actual.object.id == expected.object.id
                    && actual.for_permission == expected.for_permission
                    && actual.permission == expected.permission
            })
            .unwrap();
        assert_eq!(actual.object, expected.object);
        assert_eq!(actual.options, expected.options);
        assert_eq!(actual.open, expected.open);
        assert_eq!(actual.can_give, expected.can_give);
    }
    assert!(
        scene.rig.turn_starts(thread).is_empty(),
        "inspection starts no native work"
    );
    scene.rig.engine.stop();
}

#[tokio::test]
async fn inherited_container_names_keep_the_typed_hidden_identity_until_the_same_operation_reads_it()
 {
    use orch_api_contract::document_tree::{
        DocumentContainerCreate, DocumentContainerCreateBody, DocumentDirectory,
        DocumentTreeCommand, DocumentTreeItem, DocumentTreeOperate, DocumentTreeOperationBody,
    };
    let desk = Desk::new().await;
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([42; 32]),
    );
    let made = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&DocumentContainerCreateBody {
            parent_container_id: None,
            name: "Protected folder provenance".into(),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    let Some(DocumentTreeItem::Container(container)) = made.entries.first().map(|entry| entry.item)
    else {
        panic!("the normal folder producer returns an actual container")
    };
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &container])
        .token(&desk.ada.token)
        .raw_body(&json!({"rows":[{"permission":"doc.view","inherit":false,
            "add":[{"holder":{"kind":"user","id":desk.ada.id()}}]}]}))
        .send()
        .await
        .full_permission_page()
        .ok();
    let document = desk
        .make(&desk.ada, "Child with inherited folder authority")
        .await
        .base
        .id;
    desk.world
        .app
        .call(DocumentTreeOperate, &[])
        .token(&desk.ada.token)
        .body(&DocumentTreeOperationBody {
            command: DocumentTreeCommand::Move,
            items: vec![DocumentTreeItem::Document(document)],
            to: Some(DocumentDirectory::Container(container)),
            name: None,
            client_id: new_id().to_string(),
            version: None,
            deletion_plan: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this native tree control requires its full response");
    let ordinary = page(&desk.world, &desk.ada.token, "documents", document)
        .await
        .ok();
    let expected = ordinary
        .rows
        .iter()
        .find(|row| row.permission == "doc.view")
        .and_then(|row| row.inherited.as_ref())
        .expect("the child follows the real folder entry");
    let Some(NodeOrSystem::DocumentContainer(folder)) = expected.from.as_ref() else {
        panic!("ordinary provenance is the document container")
    };
    assert_eq!(folder.id, container);
    assert_eq!(folder.name.as_deref(), Some("Protected folder provenance"));
    assert!(!folder.redacted);
    let source = rule("PermissionPageGet.view", "document", document, &["read"]);
    let narrow = key(&desk.world, desk.ada.id(), vec![source.clone()]).await;
    let hidden = page(&desk.world, &narrow, "documents", document).await.ok();
    let hidden_row = hidden
        .rows
        .iter()
        .find(|row| row.permission == "doc.view")
        .and_then(|row| row.inherited.as_ref())
        .unwrap();
    let Some(NodeOrSystem::DocumentContainer(hidden_folder)) = hidden_row.from.as_ref() else {
        panic!("the hidden folder keeps its typed identity")
    };
    assert_eq!(hidden_folder.id, container);
    assert!(hidden_folder.redacted && hidden_folder.name.is_none());
    assert_eq!(
        hidden_row
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        expected
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>()
    );
    let full = key(
        &desk.world,
        desk.ada.id(),
        vec![
            source,
            rule(
                "PermissionPageGet.view",
                "document_container",
                container,
                &["read"],
            ),
        ],
    )
    .await;
    let shown = page(&desk.world, &full, "documents", document).await.ok();
    assert_eq!(
        shown
            .rows
            .iter()
            .find(|row| row.permission == "doc.view")
            .and_then(|row| row.inherited.as_ref())
            .and_then(|row| row.from.as_ref()),
        expected.from.as_ref()
    );
}
