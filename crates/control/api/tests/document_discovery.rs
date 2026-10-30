//! Folder-only grants are discoverable without knowing or viewing their hierarchy parents.

mod desk;
mod places;
mod reader_diagnostics;
mod support;

use desk::Desk;
use orch_api_contract::access::{
    AccessChoice, HolderInput, PageChange, PageChangeName, PageChangeWhole, PermissionPageUpdate,
};
use orch_api_contract::auth::{AdminModeSetBody, MeAdminModeSet};
use orch_api_contract::common::PageQuery;
use orch_api_contract::directory::{GroupHolderAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_api_contract::document_tree::{
    DocumentContainerCreate, DocumentContainerCreateBody, DocumentContainerDocumentCreate,
    DocumentContainerGet, DocumentTreeItem, DocumentTreeList, DocumentTreeListQuery,
};
use orch_api_contract::documents::DocumentGet;
use orch_api_contract::nodes::{NodeGet, SharedList, SharedListItem, SharedListReply};
use orch_api_contract::search::{NameHit, NameKind, NameSearchList, NameSearchListQuery};
use orch_types::{Id, Time, new_id};
use places::Who;
use serde_json::json;
use std::collections::HashSet;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn folder(desk: &Desk, parent: Option<Id>, name: &str) -> TestResult<Id> {
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([42; 32]),
    );
    let made = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&DocumentContainerCreateBody {
            parent_container_id: parent,
            name: name.into(),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    match made.entries.first().map(|entry| entry.item) {
        Some(DocumentTreeItem::Container(id)) => Ok(id),
        _ => Err("a new folder must have a typed container identity".into()),
    }
}

async fn private_parent(desk: &Desk) -> TestResult<Id> {
    let parent = folder(desk, None, "Never reveal parent").await?;
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &parent])
        .token(&desk.ada.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    folder(desk, Some(parent), "Never reveal sibling").await?;
    Ok(parent)
}

async fn give(desk: &Desk, id: Id, holder: HolderInput, expires_at: Option<Time>) {
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &id])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![holder],
                choice: AccessChoice::FullView,
                export: Some(false),
                options: None,
                expires_at,
                note: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
}

async fn shared_page(
    desk: &Desk,
    reader: &Who,
    cursor: Option<String>,
    limit: u32,
) -> SharedListReply {
    desk.world
        .app
        .call(SharedList, &[])
        .token(&reader.token)
        .query(&PageQuery {
            limit: Some(limit),
            cursor,
        })
        .send()
        .await
        .ok()
}

async fn shared(desk: &Desk, reader: &Who) -> Vec<SharedListItem> {
    let mut cursor = None;
    let mut rows = Vec::new();
    let mut visited = HashSet::new();
    loop {
        let page = shared_page(desk, reader, cursor, 2).await;
        assert_eq!(page.partial, page.next_cursor.is_some());
        rows.extend(page.items);
        let Some(next) = page.next_cursor else {
            return rows;
        };
        assert!(visited.insert(next.clone()), "every scanned page advances");
        cursor = Some(next);
    }
}

async fn names(desk: &Desk, reader: &Who, kinds: Option<&str>) -> Vec<NameHit> {
    desk.world
        .app
        .call(NameSearchList, &[])
        .token(&reader.token)
        .query(&NameSearchListQuery {
            q: Some("Shared research".into()),
            kinds: kinds.map(str::to_owned),
            ..Default::default()
        })
        .send()
        .await
        .ok()
        .items
}

fn no_private_names(value: &impl serde::Serialize) -> TestResult {
    let text = serde_json::to_string(value)?;
    for hidden in [
        "Never reveal parent",
        "Never reveal sibling",
        "Acme",
        "Web",
        "Plans",
    ] {
        assert!(
            !text.contains(hidden),
            "an independently unreadable parent/sibling name is absent"
        );
    }
    Ok(())
}

async fn open_discovered(desk: &Desk, reader: &Who, discovered: Id) -> TestResult {
    let opened = desk
        .world
        .app
        .call(DocumentContainerGet, &[&discovered])
        .token(&reader.token)
        .send()
        .await
        .ok();
    let directory = opened
        .directory
        .ok_or("a readable folder supplies its actual directory context")?;
    assert!(directory.node.redacted && directory.node.name.is_none());
    no_private_names(&directory)?;
    let page = desk
        .world
        .app
        .call(DocumentTreeList, &[&directory.node.id])
        .token(&reader.token)
        .query(&DocumentTreeListQuery {
            parent_container_id: Some(discovered),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 1);
    let DocumentTreeItem::Document(child) = page.items[0].entry.item else {
        return Err("the readable child is text".into());
    };
    assert!(page.items[0].standing.read && !page.items[0].standing.manage);
    let document = desk
        .world
        .app
        .call(DocumentGet, &[&child])
        .token(&reader.token)
        .send()
        .await
        .ok();
    assert_eq!(document.base.title, "Readable child");
    assert_eq!(
        desk.world
            .app
            .call(NodeGet, &[&directory.node.id])
            .token(&reader.token)
            .send()
            .await
            .status,
        403,
        "discovering and opening a folder grants no node View"
    );
    no_private_names(&page)?;
    no_private_names(&document)?;
    Ok(())
}

async fn add_child(desk: &Desk, parent: Id) {
    let mut body = desk::body("Readable child");
    body.markdown = Some("Only this folder's content is shared.".into());
    desk.world
        .app
        .call(DocumentContainerDocumentCreate, &[&parent])
        .token(&desk.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn an_outsider_discovers_a_folder_and_opens_its_child_without_knowing_the_parent()
-> TestResult {
    let desk = Desk::new().await;
    let reader = desk.stranger("folder-reader").await;
    let parent = private_parent(&desk).await?;
    let id = folder(&desk, Some(parent), "Shared research").await?;
    add_child(&desk, id).await;
    give(&desk, id, HolderInput::User { id: reader.id() }, None).await;
    let rows = shared(&desk, &reader).await;
    assert_eq!(rows.len(), 1);
    let SharedListItem::DocumentContainer { folder, path, .. } = &rows[0] else {
        return Err("folder grants have a distinct shared-row kind".into());
    };
    assert_eq!(folder.id, id);
    assert_eq!(folder.name.as_deref(), Some("Shared research"));
    assert!(!folder.redacted);
    assert!(!path.is_empty() && path.iter().all(|step| step.redacted && step.name.is_none()));
    no_private_names(&rows)?;
    open_discovered(&desk, &reader, folder.id).await?;
    for kinds in [None, Some("document_container")] {
        let found = names(&desk, &reader, kinds).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, NameKind::DocumentContainer);
        assert_eq!(found[0].id, folder.id);
        assert_eq!(
            found[0].href,
            Some(format!("/document-folder/{}", folder.id))
        );
        assert!(
            !found[0].path.is_empty()
                && found[0]
                    .path
                    .iter()
                    .all(|step| step.redacted && step.name.is_none())
        );
        assert!(found[0].admin_access.is_none());
        no_private_names(&found)?;
        open_discovered(&desk, &reader, found[0].id).await?;
    }
    Ok(())
}

#[tokio::test]
async fn a_real_group_grant_discovers_the_folder_without_a_surrounding_place_grant() -> TestResult {
    let desk = Desk::new().await;
    let reader = desk.stranger("group-reader").await;
    let parent = private_parent(&desk).await?;
    let id = folder(&desk, Some(parent), "Shared research").await?;
    add_child(&desk, id).await;
    let group = desk
        .world
        .app
        .call(OrgGroupCreate, &[&desk.org])
        .token(&desk.ada.token)
        .body(&OrgGroupCreateBody {
            name: "Research readers".into(),
            description: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        desk.world
            .app
            .call(GroupHolderAdd, &[&group.id])
            .token(&desk.ada.token)
            .body(&HolderInput::User { id: reader.id() })
            .send()
            .await
            .status,
        204
    );
    give(&desk, id, HolderInput::Group { id: group.id }, None).await;
    let rows = shared(&desk, &reader).await;
    assert!(
        rows.iter().any(
            |row| matches!(row,SharedListItem::DocumentContainer {folder,..} if folder.id==id)
        )
    );
    let found = names(&desk, &reader, Some("document_container")).await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, id);
    no_private_names(&rows)?;
    no_private_names(&found)?;
    open_discovered(&desk, &reader, found[0].id).await?;
    Ok(())
}

#[tokio::test]
async fn expired_and_revoked_folder_grants_disappear_from_both_discovery_routes() -> TestResult {
    let desk = Desk::new().await;
    let reader = desk.stranger("temporary-reader").await;
    let parent = private_parent(&desk).await?;
    let id = folder(&desk, Some(parent), "Shared research").await?;
    let until = desk.world.now() + chrono::Duration::seconds(1);
    give(
        &desk,
        id,
        HolderInput::User { id: reader.id() },
        Some(Time::new(until)),
    )
    .await;
    assert_eq!(shared(&desk, &reader).await.len(), 1);
    assert_eq!(
        names(&desk, &reader, Some("document_container"))
            .await
            .len(),
        1
    );
    desk.world.app.clock.advance(chrono::Duration::seconds(2));
    assert!(shared(&desk, &reader).await.is_empty());
    assert!(
        names(&desk, &reader, Some("document_container"))
            .await
            .is_empty()
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentContainerGet, &[&id])
            .token(&reader.token)
            .send()
            .await
            .status,
        404
    );
    give(&desk, id, HolderInput::User { id: reader.id() }, None).await;
    assert_eq!(shared(&desk, &reader).await.len(), 1);
    let entry: Id = sqlx::query_scalar(
        "SELECT id FROM permission_entries WHERE object_kind='document_container' AND object_id=$1
        AND permission='doc.view' AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL",
    )
    .bind(id)
    .bind(reader.id())
    .fetch_one(&desk.world.db.pool)
    .await?;
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &id])
        .token(&desk.ada.token)
        .raw_body(&json!({"rows":[{"permission":"doc.view","remove":[entry]}]}))
        .send()
        .await
        .ok();
    assert!(shared(&desk, &reader).await.is_empty());
    assert!(
        names(&desk, &reader, Some("document_container"))
            .await
            .is_empty()
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentContainerGet, &[&id])
            .token(&reader.token)
            .send()
            .await
            .status,
        404
    );
    Ok(())
}

#[tokio::test]
async fn a_hidden_candidate_produces_an_empty_continuable_page_not_the_end_of_shared_folders()
-> TestResult {
    let desk = Desk::new().await;
    let reader = desk.stranger("page-reader").await;
    let parent = private_parent(&desk).await?;
    let visible = folder(&desk, Some(parent), "Shared research").await?;
    let hidden = folder(&desk, Some(parent), "Shared research Never reveal sibling").await?;
    give(&desk, visible, HolderInput::User { id: reader.id() }, None).await;
    desk.world.app.call(PermissionPageUpdate,&[&"document-containers",&hidden]).token(&desk.ada.token)
        .raw_body(&json!({"rows":[{"permission":"access.manage","add":[{"holder":{"kind":"user","id":reader.id()}}]}]}))
        .send().await.ok();
    // Creation order is fixture metadata, chosen explicitly rather than relying on random UUID ties.
    sqlx::query("UPDATE document_containers SET created_at=$2 WHERE id=$1")
        .bind(visible)
        .bind(desk.world.now() - chrono::Duration::seconds(2))
        .execute(&desk.world.db.pool)
        .await?;
    sqlx::query("UPDATE document_containers SET created_at=$2,updated_at=$3 WHERE id=$1")
        .bind(hidden)
        .bind(desk.world.now() - chrono::Duration::seconds(1))
        .bind(desk.world.now() + chrono::Duration::seconds(1))
        .execute(&desk.world.db.pool)
        .await?;
    let first = shared_page(&desk, &reader, None, 1).await;
    assert!(first.items.is_empty() && first.partial && first.next_cursor.is_some());
    no_private_names(&first)?;
    let last = shared_page(&desk, &reader, first.next_cursor, 1).await;
    assert_eq!(last.items.len(), 1);
    assert!(
        matches!(&last.items[0],SharedListItem::DocumentContainer {folder,..} if folder.id==visible)
    );
    assert!(!last.partial && last.next_cursor.is_none());
    let found = names(&desk, &reader, Some("document_container")).await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, visible);
    let first_name = desk
        .world
        .app
        .call(NameSearchList, &[])
        .token(&reader.token)
        .query(&NameSearchListQuery {
            q: Some("research".into()),
            kinds: Some("document_container".into()),
            limit: Some(1),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        first_name.items.len(),
        1,
        "bulk native filtering skips the newer matching inaccessible candidate"
    );
    assert_eq!(first_name.items[0].id, visible);
    no_private_names(&first_name)?;
    Ok(())
}

#[tokio::test]
async fn admin_only_name_visibility_never_invents_participation_or_an_ordinary_shared_grant()
-> TestResult {
    let desk = Desk::new().await;
    let reader = desk.stranger("system-admin").await;
    desk.world.make_admin(&reader.person).await;
    let parent = private_parent(&desk).await?;
    let id = folder(&desk, Some(parent), "Shared research").await?;
    assert!(
        names(&desk, &reader, Some("document_container"))
            .await
            .is_empty()
    );
    desk.world
        .app
        .call(MeAdminModeSet, &[])
        .token(&reader.token)
        .body(&AdminModeSetBody { as_admin: true })
        .send()
        .await
        .ok();
    let found = names(&desk, &reader, Some("document_container")).await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, id);
    assert!(found[0].admin_access.is_some());
    assert!(!found[0].participating);
    assert!(
        shared(&desk, &reader).await.is_empty(),
        "As admin alone is no actual named/owned folder share"
    );
    desk.world.app.call(PermissionPageUpdate,&[&"document-containers",&id]).token(&desk.ada.token)
        .raw_body(&json!({"rows":[{"permission":"access.manage","add":[{"holder":{"kind":"user","id":reader.id()}}]}]}))
        .send().await.ok();
    let rows = shared(&desk, &reader).await;
    assert!(
        matches!(rows.as_slice(),[SharedListItem::DocumentContainer {folder,admin_access:Some(_),..}] if folder.id==id),
        "a candidate opened only by As admin preserves the current native marker"
    );
    Ok(())
}

#[tokio::test]
async fn merged_pages_preserve_node_json_and_accept_the_existing_node_cursor() -> TestResult {
    reader_diagnostics::trace("shared_mixed_node_cursor", async {
    let desk = Desk::new().await;
    let reader = desk.stranger("mixed-reader").await;
    let parent = private_parent(&desk).await?;
    let id = folder(&desk,Some(parent),"Shared research").await?;
    give(&desk,id,HolderInput::User {id:reader.id()},None).await;
    let topic = desk.another_topic("A shared topic").await;
    desk.world.app.call(PermissionPageUpdate,&[&"nodes",&topic]).token(&desk.ada.token)
        .raw_body(&json!({"name":[{"holders":[{"kind":"user","id":reader.id()}],"choice":"full_view"}]}))
        .send().await.ok();
    sqlx::query("UPDATE document_containers SET created_at=$2 WHERE id=$1").bind(id)
        .bind(desk.world.now()-chrono::Duration::seconds(1)).execute(&desk.world.db.pool).await?;
    let reply = desk.world.app.call(SharedList,&[]).token(&reader.token)
        .query(&PageQuery {limit:Some(1),cursor:None}).send().await;
    let raw:serde_json::Value = serde_json::from_slice(&reply.raw)?;
    let first = reply.ok();
    assert_eq!(first.items.len(),1);
    let SharedListItem::Node(node) = &first.items[0] else { return Err("the newer shared node stays first".into()); };
    assert_eq!(raw["items"][0],serde_json::to_value(node.as_ref())?);
    assert_eq!(raw["items"][0]["kind"],"topic");
    assert_eq!(raw["items"][0]["id"],topic.to_string());
    let cursor = first.next_cursor.ok_or("the folder is still beyond the scanned node")?;
    let parts:Vec<_> = cursor.split('|').collect();
    assert_eq!(parts.len(),4);
    assert_eq!(parts[2],"node");
    let legacy = format!("{}|{}|{}",parts[0],parts[1],parts[3]);
    let next = shared_page(&desk,&reader,Some(cursor),1).await;
    let compatible = shared_page(&desk,&reader,Some(legacy),1).await;
    assert_eq!(compatible,next);
    assert!(matches!(next.items.as_slice(),[SharedListItem::DocumentContainer {folder,..}] if folder.id==id));
    assert!(!next.partial && next.next_cursor.is_none());
    Ok(())
    }).await.0
}

#[tokio::test]
async fn soft_deleted_folders_leave_shared_and_names_while_owners_still_use_native_read()
-> TestResult {
    use orch_api_contract::document_tree::{
        DocumentTreeCommand, DocumentTreeDeletePreview, DocumentTreeOperate,
        DocumentTreeOperationBody,
    };
    let desk = Desk::new().await;
    let reader = desk.stranger("deleted-reader").await;
    let parent = private_parent(&desk).await?;
    let id = folder(&desk, Some(parent), "Shared research").await?;
    give(&desk, id, HolderInput::User { id: reader.id() }, None).await;
    assert!(
        shared(&desk, &desk.ada).await.iter().any(
            |item| matches!(item,SharedListItem::DocumentContainer {folder,..} if folder.id==id)
        ),
        "ordinary accountable ownership is a candidate, subject to current native Read"
    );
    assert_eq!(shared(&desk, &reader).await.len(), 1);
    let mut body = DocumentTreeOperationBody {
        command: DocumentTreeCommand::Delete,
        items: vec![DocumentTreeItem::Container(id)],
        to: None,
        name: None,
        client_id: new_id().to_string(),
        version: None,
        deletion_plan: None,
    };
    let preview = desk
        .world
        .app
        .call(DocumentTreeDeletePreview, &[])
        .token(&desk.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    body.deletion_plan = Some(preview.stamp);
    desk.world
        .app
        .call(DocumentTreeOperate, &[])
        .token(&desk.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert!(shared(&desk, &reader).await.is_empty());
    assert!(
        names(&desk, &reader, Some("document_container"))
            .await
            .is_empty()
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentContainerGet, &[&id])
            .token(&reader.token)
            .send()
            .await
            .status,
        404
    );
    Ok(())
}

#[tokio::test]
async fn an_absent_supplied_template_does_not_rollback_a_real_place_share_or_relax_core_dependencies()
-> TestResult {
    use orch_api_contract::access::PermissionPageGet;
    use orch_api_contract::resources::TemplateGet;
    use orch_authz::depends::{self, Starting};
    use orch_authz::usable::Policies;
    use orch_authz::{Caller, Principal};
    use orch_db::queries::resources;
    use orch_settings::{Place, get};
    use orch_settings_contract::registry::keys;

    let desk = Desk::new().await;
    let reader = desk.stranger("page-reader").await;
    let template = orch_db::seeds::DEFAULT_TEMPLATE_ID;
    let mut conn = desk.world.conn().await;
    let selected: Option<Id> =
        get(&mut conn, Place::Node(desk.topic), keys::ENV_TEMPLATE_ID).await?;
    assert_eq!(
        selected,
        Some(template),
        "the setting supplier still names the installation's default"
    );
    assert!(
        resources::resource_facts(&mut conn, &[template])
            .await?
            .is_empty(),
        "this office deliberately has no native installation seeds"
    );
    let principal = Principal::load(
        &mut conn,
        Caller::session(desk.ada.id()),
        Time::new(desk.world.now()),
    )
    .await?;
    let strict = depends::of_place(
        &mut conn,
        &principal,
        desk.topic,
        &Starting {
            template: Some(template),
            ..Starting::default()
        },
        &Policies::default(),
    )
    .await;
    assert!(
        matches!(strict, Err(orch_authz::Error::NotFound(id)) if id == template),
        "core work/copy/offer dependency resolution remains strict about a missing configured resource"
    );
    drop(conn);

    let before = desk
        .world
        .app
        .call(PermissionPageGet, &[&"nodes", &desk.topic])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert!(
        before.depends.is_empty(),
        "an absent resource is not an invented live permission dependency"
    );
    assert_eq!(
        desk.world
            .app
            .call(NodeGet, &[&desk.topic])
            .token(&reader.token)
            .send()
            .await
            .status,
        403
    );
    let changed = desk
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &desk.topic])
        .token(&desk.ada.token)
        .raw_body(
            &json!({"name":[{"holders":[{"kind":"user","id":reader.id()}],"choice":"full_view"}]}),
        )
        .send()
        .await
        .ok();
    assert!(changed.revision > before.revision);
    assert!(changed.depends.is_empty());
    let current = desk
        .world
        .app
        .call(PermissionPageGet, &[&"nodes", &desk.topic])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        current.revision, changed.revision,
        "the native page mutation actually committed"
    );
    assert_eq!(
        desk.world
            .app
            .call(NodeGet, &[&desk.topic])
            .token(&reader.token)
            .send()
            .await
            .status,
        200
    );
    assert_eq!(
        desk.world
            .app
            .call(NodeGet, &[&desk.project])
            .token(&reader.token)
            .send()
            .await
            .status,
        403
    );
    assert_eq!(
        desk.world
            .app
            .call(TemplateGet, &[&template])
            .token(&reader.token)
            .send()
            .await
            .status,
        404,
        "editing a place's page does not create a template or grant access to one"
    );
    let phantom_grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM permission_entries
        WHERE object_kind='template' AND object_id=$1",
    )
    .bind(template)
    .fetch_one(&desk.world.db.pool)
    .await?;
    assert_eq!(
        phantom_grants, 0,
        "the missing configured resource gains no synthetic permission entries"
    );
    Ok(())
}

#[tokio::test]
async fn deleting_an_unused_builtin_template_removes_only_its_live_dependency_from_a_places_page()
-> TestResult {
    use orch_api_contract::access::PermissionPageGet;
    use orch_api_contract::resources::{TemplateDelete, TemplateGet};
    use orch_db::queries::resources;
    use orch_db::seeds::{self, Release};
    use orch_settings::{Place, get};
    use orch_settings_contract::registry::keys;

    let desk = Desk::new().await;
    let reader = desk.stranger("unchanged-reader").await;
    let template = seeds::DEFAULT_TEMPLATE_ID;
    {
        let mut tx = desk.world.db.pool.begin().await?;
        seeds::seed(
            &mut tx,
            &Release {
                environment_image: "registry.example/orch-test-env:1",
                environment_image_digest: None,
                adapters: &[],
            },
            desk.world.now(),
        )
        .await?;
        tx.commit().await?;
    }
    let before = desk
        .world
        .app
        .call(PermissionPageGet, &[&"nodes", &desk.topic])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(before.depends.len(), 1);
    let dependency = &before.depends[0];
    assert_eq!(dependency.object.id, template);
    assert_eq!(dependency.permission, "template.use");
    assert_eq!(dependency.for_permission, "topic.interact");
    assert!(
        dependency.object.owner.is_none(),
        "a shipped system template has no invented person owner"
    );
    let template_page = desk
        .world
        .app
        .call(PermissionPageGet, &[&"templates", &template])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        dependency.can_give,
        template_page
            .gives
            .choices
            .contains(&AccessChoice::FullInteract),
        "giving follows the template's actual holder-sharing policy, not ownership of the place"
    );
    let changed = desk
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &desk.topic])
        .token(&desk.ada.token)
        .raw_body(
            &json!({"name":[{"holders":[{"kind":"user","id":reader.id()}],"choice":"full_view"}]}),
        )
        .send()
        .await
        .ok();
    assert_eq!(changed.depends[0].object.id, template);

    let admin = places::admin(&desk.world, "template-admin").await;
    assert_eq!(
        desk.world
            .app
            .call(TemplateDelete, &[&template])
            .token(&admin.token)
            .send()
            .await
            .status,
        204,
        "the actual native route allows an administrator to remove an unused built-in template"
    );
    {
        let mut conn = desk.world.conn().await;
        let facts = resources::resource_facts(&mut conn, &[template]).await?;
        assert_eq!(facts.len(), 1);
        assert!(
            facts[0].deleted,
            "removal retains the actual template identity instead of inventing a missing row"
        );
        let selected: Option<Id> =
            get(&mut conn, Place::Node(desk.topic), keys::ENV_TEMPLATE_ID).await?;
        assert_eq!(
            selected,
            Some(template),
            "the dependency projection never rewrites the configured choice"
        );
    }
    let after = desk
        .world
        .app
        .call(PermissionPageGet, &[&"nodes", &desk.topic])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert!(
        after.depends.is_empty(),
        "a removed template has no live dependency offer"
    );
    assert_eq!(
        after.revision, changed.revision,
        "only the unrelated template was removed"
    );
    assert_eq!(
        desk.world
            .app
            .call(NodeGet, &[&desk.topic])
            .token(&reader.token)
            .send()
            .await
            .status,
        200,
        "the committed place share survives removal of its unavailable default"
    );
    assert_eq!(
        desk.world
            .app
            .call(TemplateGet, &[&template])
            .token(&reader.token)
            .send()
            .await
            .status,
        404,
        "the dependency projection does not restore or grant a removed template"
    );
    let entry: Id = sqlx::query_scalar(
        "SELECT id FROM permission_entries WHERE object_kind='node' AND object_id=$1
        AND permission='topic.view' AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL",
    )
    .bind(desk.topic)
    .bind(reader.id())
    .fetch_one(&desk.world.db.pool)
    .await?;
    let committed = desk
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &desk.topic])
        .token(&desk.ada.token)
        .raw_body(&json!({"rows":[{"permission":"topic.view","remove":[entry]}]}))
        .send()
        .await
        .ok();
    assert!(committed.revision > after.revision && committed.depends.is_empty());
    assert_eq!(
        desk.world
            .app
            .call(NodeGet, &[&desk.topic])
            .token(&reader.token)
            .send()
            .await
            .status,
        403,
        "the current native page still controls reach after the unavailable default is omitted"
    );
    Ok(())
}

#[tokio::test]
async fn shared_documents_page_with_places_and_folders_then_expire_without_exposing_neighbors()
-> TestResult {
    let desk = Desk::new().await;
    let reader = desk.stranger("document-reader").await;
    let document = desk.make(&desk.ada, "Shared document").await;
    let sibling = desk.make(&desk.ada, "Never reveal sibling").await;
    let until = desk.world.now() + chrono::Duration::seconds(1);
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &document.base.id])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: reader.id() }],
                choice: AccessChoice::FullView,
                export: Some(false),
                options: None,
                expires_at: Some(Time::new(until)),
                note: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let folder_id = folder(&desk, None, "Shared folder").await?;
    give(
        &desk,
        folder_id,
        HolderInput::User { id: reader.id() },
        None,
    )
    .await;
    let topic = desk.another_topic("Shared place").await;
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &topic])
        .token(&desk.ada.token)
        .raw_body(
            &json!({"name":[{"holders":[{"kind":"user","id":reader.id()}],"choice":"full_view"}]}),
        )
        .send()
        .await
        .ok();
    sqlx::query("UPDATE documents SET created_at=$2 WHERE id=$1")
        .bind(document.base.id)
        .bind(desk.world.now() - chrono::Duration::seconds(2))
        .execute(&desk.world.db.pool)
        .await?;
    sqlx::query("UPDATE document_containers SET created_at=$2 WHERE id=$1")
        .bind(folder_id)
        .bind(desk.world.now() - chrono::Duration::seconds(1))
        .execute(&desk.world.db.pool)
        .await?;
    let first = shared_page(&desk, &reader, None, 1).await;
    assert!(
        matches!(first.items.as_slice(),[SharedListItem::Node(node)] if matches!(node.as_ref(), orch_api_contract::nodes::Node::Topic(value) if value.base.id == topic))
    );
    let second = shared_page(
        &desk,
        &reader,
        Some(first.next_cursor.ok_or("folder follows")?),
        1,
    )
    .await;
    assert!(
        matches!(second.items.as_slice(),[SharedListItem::DocumentContainer {folder,..}] if folder.id==folder_id)
    );
    let third = shared_page(
        &desk,
        &reader,
        Some(second.next_cursor.ok_or("document follows")?),
        1,
    )
    .await;
    let [
        SharedListItem::Document {
            document: reference,
            path,
            participating,
            ..
        },
    ] = third.items.as_slice()
    else {
        return Err("the last page must contain the independently shared document".into());
    };
    assert_eq!(reference.id, document.base.id);
    assert_eq!(reference.title, "Shared document");
    assert!(*participating);
    assert!(reference.node.redacted && reference.node.name.is_none());
    assert!(!path.is_empty() && path.iter().all(|step| step.redacted && step.name.is_none()));
    assert!(!third.partial && third.next_cursor.is_none());
    no_private_names(&third)?;
    assert_eq!(
        desk.world
            .app
            .call(DocumentGet, &[&reference.id])
            .token(&reader.token)
            .send()
            .await
            .ok()
            .base
            .id,
        reference.id
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentGet, &[&sibling.base.id])
            .token(&reader.token)
            .send()
            .await
            .status,
        403
    );
    desk.world.advance(chrono::Duration::seconds(2));
    assert!(!shared(&desk,&reader).await.iter().any(|item| matches!(item,SharedListItem::Document {document:reference,..} if reference.id==document.base.id)));
    assert_eq!(
        desk.world
            .app
            .call(DocumentGet, &[&document.base.id])
            .token(&reader.token)
            .send()
            .await
            .status,
        403
    );
    Ok(())
}
