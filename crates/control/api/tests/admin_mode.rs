//! One explicit session mode, bounded administrative reach and consistent ordinary/elevated provenance over HTTP.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::access::{
    EffectiveAccessGet, EffectiveAccessGetQuery, PageChange, PermissionPageGet,
    PermissionPageUpdate, ThreadAccess,
};
use orch_api_contract::auth::{
    AdminModeSetBody, AdminOpeningBody, MeAdminModeGet, MeAdminModeSet, MeAdminOpening,
};
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::documents::{
    DocumentGet, NewDocumentAccess, NodeDocumentCreate, NodeDocumentCreateBody, NodeDocumentList,
    NodeDocumentListQuery,
};
use orch_api_contract::everything::{EverythingList, EverythingListQuery};
use orch_api_contract::nodes::{
    CreateThread, CreateThreadBinding, Node, NodeBase, NodeGet, NodeOrSystem, NodeUpdate,
    NodeUpdateBody, OrgTreeGet, OrgTreeGetQuery, ThreadCreate,
};
use orch_api_contract::search::{NameSearchList, NameSearchListQuery, SearchList, SearchListQuery};
use orch_api_contract::threads::ThreadKind;
use orch_types::{Id, new_id};
use places::{Who, join, make_department, make_project, make_topic, person, who};
use rig::{MODEL, Scene};
use serde_json::json;
use std::collections::HashSet;
use support::World;

async fn private_resource(world: &World, owner: &Who, home: Id, kind: &str) -> Id {
    let mut conn = world.conn().await;
    let id = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        kind,
        Some(owner.id()),
        Some(home),
        // A mount's and an MCP server's name is how agents and paths name it: lower case, no spaces.
        &format!("admin-proof-{kind}"),
    )
    .await
    .unwrap();
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(owner.id()),
        orch_types::Time::new(world.now()),
    )
    .await
    .unwrap();
    let object = orch_access_contract::decisions::ObjectRef {
        kind: kind.parse().unwrap(),
        id,
    };
    orch_authz::ownership::start_resource(
        &mut conn,
        object,
        owner.id(),
        orch_authz::ownership::NewAccess::OnlyMe,
        principal.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let plural = match kind {
        "agent" => "agents",
        "provider" => "providers",
        "environment" => "environments",
        "host" => "hosts",
        "machine" => "machines",
        "template" => "templates",
        "mount" => "mounts",
        "mcp_server" => "mcp-servers",
        _ => panic!("unsupported proof resource"),
    };
    change_page(
        world,
        owner,
        plural,
        id,
        serde_json::from_value(json!({"whole":"private"})).unwrap(),
    )
    .await;
    id
}

/// Uses the actual typed routes, retaining refusal statuses as well as full list rows.
async fn resource_views(
    world: &World,
    by: &Who,
    kind: &str,
    id: Id,
) -> (u16, serde_json::Value, serde_json::Value) {
    use orch_api_contract::{environment as env, mounts, resources as api};
    macro_rules! read {
        ($get:expr, $list:expr) => {{
            let one = world
                .app
                .call($get, &[&id])
                .token(&by.token)
                .send_bytes()
                .await;
            let list = world
                .app
                .call($list, &[])
                .token(&by.token)
                .send_bytes()
                .await;
            assert!(
                list.status.is_success(),
                "{kind} list: {}",
                String::from_utf8_lossy(&list.raw)
            );
            (
                one.status.as_u16(),
                serde_json::from_slice(&one.raw).unwrap(),
                serde_json::from_slice(&list.raw).unwrap(),
            )
        }};
    }
    match kind {
        "provider" => {
            let (status, one, page): (u16, serde_json::Value, serde_json::Value) =
                read!(api::ProviderGet, api::ProviderList);
            (status, one, page["items"].clone())
        }
        "agent" => read!(api::AgentGet, api::AgentList),
        "environment" => read!(env::EnvironmentGet, env::EnvironmentList),
        "machine" => read!(api::MachineGet, api::MachineList),
        "host" => read!(api::HostGet, api::HostList),
        "template" => read!(api::TemplateGet, api::TemplateList),
        "mount" => read!(mounts::MountGet, mounts::MountList),
        "mcp_server" => read!(api::McpServerGet, api::McpServerList),
        _ => panic!("unsupported proof resource"),
    }
}

async fn resource_marks(world: &World, by: &Who, resources: &[(&str, Id)], enabled: bool) {
    for (kind, id) in resources {
        let permission_area = if *kind == "mcp_server" { "mcp" } else { *kind };
        let (status, one, list) = resource_views(world, by, kind, *id).await;
        let listed = list
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == id.to_string());
        if enabled {
            assert_eq!(status, 200, "{kind}: {one}");
            let marker = &one["access"]["admin_access"];
            assert!(marker.is_object(), "{kind}: {one}");
            assert_eq!(marker["private"], true);
            assert_eq!(&listed.unwrap()["access"]["admin_access"], marker);
            let rights = one["access"]["admin_only"].as_array().unwrap();
            assert!(
                rights.contains(&json!(format!("{permission_area}.view"))),
                "{kind}: {one}"
            );
            assert!(
                rights.contains(&json!(format!("{permission_area}.manage"))),
                "{kind}: {one}"
            );
            assert_eq!(one["access"]["admin_place"], marker["place"]);
        } else {
            assert!(matches!(status, 403 | 404), "{kind}: {one}");
            assert!(
                listed.is_none(),
                "{kind} is absent while this session's mode is off"
            );
        }
    }
}

fn base(node: &Node) -> &NodeBase<ThreadKind> {
    match node {
        Node::Thread(row) => &row.base,
        _ => panic!("the route must return the requested thread"),
    }
}

async fn mode(world: &World, by: &Who, enabled: bool) {
    let state = world
        .app
        .call(MeAdminModeSet, &[])
        .token(&by.token)
        .body(&AdminModeSetBody { as_admin: enabled })
        .send()
        .await
        .ok();
    assert_eq!(state.as_admin, enabled);
    assert!(state.available);
}

async fn private_thread(
    scene: &Scene,
    topic: Id,
    name: &str,
) -> orch_api_contract::threads::Thread {
    scene
        .rig
        .world
        .app
        .call(ThreadCreate, &[&topic])
        .token(&scene.ada.token)
        .body(&CreateThread {
            name: Some(name.to_owned()),
            binding: Some(CreateThreadBinding {
                agent_id: scene.on.agent,
                provider_id: scene.on.provider,
                arrangement: None,
            }),
            model: Some(MODEL.to_owned()),
            environment: Some(EnvironmentChoice::Use(scene.on.environment)),
            directory: Some("web".to_owned()),
            access: Some(ThreadAccess::Change(
                serde_json::from_value(json!({ "whole": "private" })).unwrap(),
            )),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok()
}

async fn change_page(world: &World, by: &Who, object: &str, id: Id, mut change: PageChange) {
    let page = world
        .app
        .call(PermissionPageGet, &[&object, &id])
        .token(&by.token)
        .send()
        .await
        .ok();
    change.expected_revision = Some(page.revision);
    world
        .app
        .call(PermissionPageUpdate, &[&object, &id])
        .token(&by.token)
        .body(&change)
        .send()
        .await
        .ok();
}

#[derive(Debug, Clone, Copy)]
enum Level {
    System,
    Org,
    Department,
}

async fn check_level(level: Level) {
    let scene = Scene::new().await;
    let world = &scene.rig.world;
    let root = scene.tree.org.base.id;
    let east = make_department(world, &scene.ada, root, "East", "east")
        .await
        .base
        .id;
    let west = make_department(world, &scene.ada, root, "West", "west")
        .await
        .base
        .id;
    let east_project = make_project(world, &scene.ada, east, "East work")
        .await
        .base
        .id;
    let west_project = make_project(world, &scene.ada, west, "West work")
        .await
        .base
        .id;
    let east_topic = make_topic(world, &scene.ada, east_project, "East tasks")
        .await
        .base
        .id;
    let west_topic = make_topic(world, &scene.ada, west_project, "West tasks")
        .await
        .base
        .id;
    let by = if matches!(level, Level::System) {
        who(world, "inspector", true).await
    } else {
        person(world, "inspector").await
    };
    let (domain, role) = match level {
        Level::Department => (east, "admin"),
        Level::Org => (root, "admin"),
        Level::System => (root, "viewer"),
    };
    // The system case has an ordinary Viewer role, so its administration still comes
    // only from the system. Membership keeps an unrelated outsider-lifetime rule out of this matrix.
    join(world, root, domain, &by, role, &scene.ada).await;
    let private = private_thread(&scene, east_topic, "Admin proof private").await;
    let sibling = private_thread(&scene, west_topic, "Admin proof sibling").await;
    let shared = private_thread(&scene, east_topic, "Admin proof shared").await;
    change_page(
        world,
        &scene.ada,
        "nodes",
        shared.base.id,
        serde_json::from_value(json!({
            "name": [{ "choice": "full_view", "holders": [{ "kind": "user", "id": by.id() }] }]
        }))
        .unwrap(),
    )
    .await;
    let mut resources = Vec::new();
    for kind in [
        "provider",
        "agent",
        "environment",
        "machine",
        "host",
        "template",
        "mount",
        "mcp_server",
    ] {
        resources.push((kind, private_resource(world, &scene.ada, east, kind).await));
    }

    assert!(
        !world
            .app
            .call(MeAdminModeGet, &[])
            .token(&by.token)
            .send()
            .await
            .ok()
            .as_admin,
        "a newly opened browser session starts with its mode off"
    );
    let denied = world
        .app
        .call(NodeGet, &[&private.base.id])
        .token(&by.token)
        .send()
        .await;
    assert!(matches!(denied.status.as_u16(), 403 | 404));
    let ordinary = world
        .app
        .call(NodeGet, &[&shared.base.id])
        .token(&by.token)
        .send()
        .await
        .ok();
    assert!(base(&ordinary).access.admin_access.is_none());
    assert!(base(&ordinary).access.admin_only.is_empty());
    let ordinary_permissions: HashSet<_> =
        base(&ordinary).access.permissions.iter().cloned().collect();
    let ordinary_tree = world
        .app
        .call(OrgTreeGet, &[&root])
        .token(&by.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    let ordinary_ids: HashSet<_> = ordinary_tree
        .nodes
        .iter()
        .filter_map(|row| row.as_visible())
        .map(|row| row.id)
        .collect();
    resource_marks(world, &by, &resources, false).await;

    mode(world, &by, true).await;
    let elevated = world
        .app
        .call(NodeGet, &[&private.base.id])
        .token(&by.token)
        .send()
        .await
        .ok();
    let marker = base(&elevated).access.admin_access.as_ref().unwrap();
    assert!(marker.private);
    match (level, &marker.place) {
        (Level::System, NodeOrSystem::System(_)) => {}
        (Level::Org, NodeOrSystem::Node(place)) => assert_eq!(place.id, root),
        (Level::Department, NodeOrSystem::Node(place)) => assert_eq!(place.id, east),
        _ => panic!(
            "wrong administrative place for {level:?}: {:?}",
            marker.place
        ),
    }
    assert!(
        base(&elevated)
            .access
            .permissions
            .iter()
            .any(|permission| permission == "thread.manage")
    );
    let baseline = world
        .app
        .call(EffectiveAccessGet, &[&"nodes", &private.base.id])
        .token(&scene.ada.token)
        .query(&EffectiveAccessGetQuery { user_id: by.id() })
        .send()
        .await
        .ok();
    let baseline: HashSet<_> = baseline
        .permissions
        .into_iter()
        .map(|held| held.permission)
        .collect();
    let expected: HashSet<_> = base(&elevated)
        .access
        .permissions
        .iter()
        .filter(|permission| !baseline.contains(*permission))
        .cloned()
        .collect();
    assert_eq!(
        base(&elevated)
            .access
            .admin_only
            .iter()
            .cloned()
            .collect::<HashSet<_>>(),
        expected
    );

    let shared_on = world
        .app
        .call(NodeGet, &[&shared.base.id])
        .token(&by.token)
        .send()
        .await
        .ok();
    assert!(
        base(&shared_on).access.admin_access.is_none(),
        "an ordinary View grant is never marked, even on a private page"
    );
    let expected: HashSet<_> = base(&shared_on)
        .access
        .permissions
        .iter()
        .filter(|permission| !ordinary_permissions.contains(*permission))
        .cloned()
        .collect();
    assert_eq!(
        base(&shared_on)
            .access
            .admin_only
            .iter()
            .cloned()
            .collect::<HashSet<_>>(),
        expected
    );
    assert!(expected.contains("thread.manage"));
    assert_eq!(
        base(&shared_on).access.admin_place.as_ref(),
        Some(&marker.place),
        "an admin-only act names its actual source even when ordinary View leaves no marker"
    );
    resource_marks(world, &by, &resources, true).await;

    let tree = world
        .app
        .call(OrgTreeGet, &[&root])
        .token(&by.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    let marked = tree
        .nodes
        .iter()
        .filter_map(|row| row.as_visible())
        .find(|row| row.id == private.base.id)
        .unwrap();
    assert_eq!(marked.admin_access, base(&elevated).access.admin_access);
    let elevated_ids: HashSet<_> = tree
        .nodes
        .iter()
        .filter_map(|row| row.as_visible())
        .map(|row| row.id)
        .collect();
    let marked_ids: HashSet<_> = tree
        .nodes
        .iter()
        .filter_map(|row| row.as_visible())
        .filter(|row| row.admin_access.is_some())
        .map(|row| row.id)
        .collect();
    assert_eq!(
        marked_ids,
        elevated_ids.difference(&ordinary_ids).copied().collect(),
        "the marked set is exactly what vanishes without the native reach"
    );
    let names = world
        .app
        .call(NameSearchList, &[])
        .token(&by.token)
        .query(&NameSearchListQuery {
            q: Some("Admin proof".to_owned()),
            // A page of names holds at most this many; the proof's few things fit in one.
            limit: Some(orch_api_contract::search::NAME_SEARCH_LIMIT_MAX),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        names
            .iter()
            .find(|row| row.id == private.base.id)
            .unwrap()
            .admin_access,
        marked.admin_access
    );
    assert!(
        names
            .iter()
            .find(|row| row.id == shared.base.id)
            .unwrap()
            .admin_access
            .is_none()
    );
    for (_, id) in &resources {
        assert_eq!(
            names
                .iter()
                .find(|row| row.id == *id)
                .unwrap()
                .admin_access
                .as_ref()
                .unwrap()
                .place,
            marker.place
        );
    }
    let inventory = world
        .app
        .call(EverythingList, &[])
        .token(&by.token)
        .query(&EverythingListQuery {
            q: Some("Admin proof".to_owned()),
            limit: Some(200),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    for id in std::iter::once(private.base.id).chain(resources.iter().map(|(_, id)| *id)) {
        let row = inventory
            .items
            .iter()
            .find(|row| row.id == id.to_string())
            .unwrap();
        assert_eq!(row.admin_access.as_ref().unwrap().place, marker.place);
    }
    assert!(
        inventory
            .items
            .iter()
            .find(|row| row.id == shared.base.id.to_string())
            .unwrap()
            .admin_access
            .is_none()
    );
    let sibling_on = world
        .app
        .call(NodeGet, &[&sibling.base.id])
        .token(&by.token)
        .send()
        .await;
    assert_eq!(
        sibling_on.status.is_success(),
        !matches!(level, Level::Department),
        "department reach stops at that department"
    );

    let changed = world
        .app
        .call(NodeUpdate, &[&private.base.id])
        .token(&by.token)
        .body(
            &serde_json::from_value::<NodeUpdateBody>(json!({
                "name": "Admin proof renamed", "expected_revision": base(&elevated).revision
            }))
            .unwrap(),
        )
        .send()
        .await
        .ok();
    assert_eq!(base(&changed).name, "Admin proof renamed");
    let named: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='node' AND object_id=$1 AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL")
        .bind(private.base.id).bind(by.id()).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(
        named, 0,
        "native management never puts the admin on the page"
    );
    let written_as_admin: bool = sqlx::query_scalar("SELECT COALESCE(bool_and(as_admin),false) FROM audit_log WHERE actor_id=$1 AND object_id=$2")
        .bind(by.id()).bind(private.base.id).fetch_one(&world.db.pool).await.unwrap();
    assert!(written_as_admin);
    let follows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM subscriptions WHERE user_id=$1 AND node_id=$2")
            .bind(by.id())
            .bind(private.base.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        follows, 0,
        "native reading and management never subscribe their administrator"
    );
    let mut conn = world.conn().await;
    orch_db::queries::hierarchy::put_subscription(
        &mut conn,
        by.id(),
        private.base.id,
        "all",
        "manual",
        world.now(),
    )
    .await
    .unwrap();
    let recipients = orch_db::audit::scope_origin(
        orch_db::audit::Origin {
            as_admin: Some(by.id()),
            ..Default::default()
        },
        orch_engine::notifications::subscribers_of(
            &mut conn,
            private.base.id,
            orch_engine::notifications::Reach::AllOrImportant,
            None,
            world.now(),
        ),
    )
    .await
    .unwrap();
    assert!(
        !recipients.contains(&by.id()),
        "even a saved follow cannot turn native reach into a notification audience"
    );
    drop(conn);
    mode(world, &by, false).await;
    assert!(matches!(
        world
            .app
            .call(NodeGet, &[&private.base.id])
            .token(&by.token)
            .send()
            .await
            .status
            .as_u16(),
        403 | 404
    ));
    let still_shared = world
        .app
        .call(NodeGet, &[&shared.base.id])
        .token(&by.token)
        .send()
        .await
        .ok();
    assert!(base(&still_shared).access.admin_access.is_none());
    assert!(base(&still_shared).access.admin_only.is_empty());
}

#[tokio::test]
async fn a_system_admin_uses_only_their_ordinary_pages_until_this_session_is_enabled() {
    check_level(Level::System).await;
}

#[tokio::test]
async fn an_org_admin_reaches_private_work_without_being_named_on_it() {
    check_level(Level::Org).await;
}

#[tokio::test]
async fn a_department_admin_reaches_only_the_department_and_keeps_ordinary_grants_unmarked() {
    check_level(Level::Department).await;
}

#[tokio::test]
async fn opening_a_private_document_in_another_org_records_one_row_per_opening() {
    let scene = Scene::new().await;
    let world = &scene.rig.world;
    let by = who(world, "outsideadmin", true).await;
    let document = world
        .app
        .call(NodeDocumentCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&NodeDocumentCreateBody {
            title: "Private handbook".to_owned(),
            markdown: Some("Private words".to_owned()),
            blocks: None,
            access: Some(NewDocumentAccess::Private),
        })
        .send()
        .await
        .ok();
    assert!(matches!(
        world
            .app
            .call(DocumentGet, &[&document.base.id])
            .token(&by.token)
            .send()
            .await
            .status
            .as_u16(),
        403 | 404
    ));
    mode(world, &by, true).await;
    let opening = new_id();
    for _ in 0..2 {
        let view = world
            .app
            .call(DocumentGet, &[&document.base.id])
            .token(&by.token)
            .header("x-orch-opening", &opening.to_string())
            .send()
            .await
            .ok();
        assert!(matches!(
            view.access.admin_access.unwrap().place,
            NodeOrSystem::System(_)
        ));
    }
    world
        .app
        .call(MeAdminOpening, &[])
        .token(&by.token)
        .body(&AdminOpeningBody {
            kind: "document".to_owned(),
            id: document.base.id,
            opening_id: opening,
        })
        .send()
        .await
        .ok();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE actor_id=$1 AND object_id=$2 AND action='admin.opened' AND as_admin")
        .bind(by.id()).bind(document.base.id).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(count, 1, "HTTP and a cached/live opening share one receipt");
    let listed = world
        .app
        .call(NodeDocumentList, &[&scene.tree.topic.base.id])
        .token(&by.token)
        .query(&NodeDocumentListQuery::default())
        .send()
        .await
        .ok();
    assert!(
        listed
            .items
            .iter()
            .filter_map(|row| row.as_visible())
            .find(|row| row.id == document.base.id)
            .unwrap()
            .admin_access
            .is_some()
    );
    let search = |participating| SearchListQuery {
        regex: None,
        immediate: None,
        typing_age_ms: None,
        continuation_id: None,
        projection_revision: None,
        sort: None,
        participating,
        q: "Private handbook".to_owned(),
        kinds: Some("document".to_owned()),
        scope: None,
        author_id: None,
        from: None,
        to: None,
        limit: None,
        cursor: None,
        chat_id: None,
        document_id: None,
        with_file: None,
        mentions_me: None,
        ..SearchListQuery::default()
    };
    let found = world
        .app
        .call(SearchList, &[])
        .token(&by.token)
        .query(&search(None))
        .send()
        .await
        .ok();
    assert_eq!(found.items.len(), 1);
    assert!(found.items[0].admin_access.as_ref().unwrap().private);
    assert!(
        world
            .app
            .call(SearchList, &[])
            .token(&by.token)
            .query(&search(Some(true)))
            .send()
            .await
            .ok()
            .items
            .is_empty(),
        "ordinary participation filters native-only matches before paging"
    );
    world
        .app
        .call(DocumentGet, &[&document.base.id])
        .token(&by.token)
        .header("x-orch-opening", &new_id().to_string())
        .send()
        .await
        .ok();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE actor_id=$1 AND object_id=$2 AND action='admin.opened' AND as_admin")
        .bind(by.id()).bind(document.base.id).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(
        count, 2,
        "opening it again is recorded even on the same day"
    );
}

#[tokio::test]
async fn a_setting_that_closes_chat_to_everybody_also_closes_it_to_enabled_native_management() {
    use orch_api_contract::{
        chats::{ChatGet, ChatMessageCreate},
        settings::NodeSettingSet,
    };
    let scene = Scene::new().await;
    let world = &scene.rig.world;
    let by = person(world, "operator").await;
    join(
        world,
        scene.tree.org.base.id,
        scene.tree.org.base.id,
        &by,
        "admin",
        &scene.ada,
    )
    .await;
    let private = private_thread(&scene, scene.tree.topic.base.id, "Closed chat").await;
    world
        .app
        .call(NodeSettingSet, &[&scene.tree.org.base.id, &"chat.enabled"])
        .token(&scene.ada.token)
        .raw_body(&json!({"value":false,"locked":true}))
        .send()
        .await
        .ok();
    mode(world, &by, true).await;
    let chat = world
        .app
        .call(ChatGet, &[&private.base.chat_id])
        .token(&by.token)
        .send()
        .await
        .ok();
    assert!(!chat.can.send);
    assert!(chat.can.admin_access.is_some());
    let denied=world.app.call(ChatMessageCreate,&[&private.base.chat_id]).token(&by.token)
        .raw_body(&json!({"parts":[{"type":"text","text":"still closed"}],"client_id":new_id().to_string()})).send().await.problem();
    assert_eq!(
        denied.code,
        orch_api_contract::common::ErrorCode::ChatNotAllowed
    );
    assert_eq!(denied.extra["reason"], "disabled");
}

#[tokio::test]
async fn another_browser_session_and_an_all_scopes_token_keep_their_ordinary_access() {
    use orch_api_core::middleware::authentication::{
        ACCESS_TOKEN_PREFIX, API_TOKEN_PREFIX, new_token, token_hash,
    };
    let scene = Scene::new().await;
    let world = &scene.rig.world;
    let by = person(world, "operator").await;
    join(
        world,
        scene.tree.org.base.id,
        scene.tree.org.base.id,
        &by,
        "admin",
        &scene.ada,
    )
    .await;
    let other_token = new_token(ACCESS_TOKEN_PREFIX);
    let api_token = new_token(API_TOKEN_PREFIX);
    {
        let mut conn = world.conn().await;
        orch_db::queries::identity::testing::insert_session(
            &mut conn,
            new_id(),
            by.id(),
            &token_hash(&other_token),
            world.now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO api_tokens(id,user_id,name,token_hash,scopes) VALUES($1,$2,'ordinary automation',$3,ARRAY['*'])")
            .bind(new_id()).bind(by.id()).bind(token_hash(&api_token)).execute(&mut *conn).await.unwrap();
    }
    let other = Who {
        person: by.person.clone(),
        token: other_token,
    };
    let private = private_thread(&scene, scene.tree.topic.base.id, "Session private").await;
    mode(world, &by, true).await;
    world
        .app
        .call(NodeGet, &[&private.base.id])
        .token(&by.token)
        .send()
        .await
        .ok();
    assert!(
        !world
            .app
            .call(MeAdminModeGet, &[])
            .token(&other.token)
            .send()
            .await
            .ok()
            .as_admin
    );
    for token in [&other.token, &api_token] {
        assert!(matches!(
            world
                .app
                .call(NodeGet, &[&private.base.id])
                .token(token)
                .send()
                .await
                .status
                .as_u16(),
            403 | 404
        ));
    }
    assert_eq!(
        world
            .app
            .call(MeAdminModeGet, &[])
            .token(&api_token)
            .send()
            .await
            .status
            .as_u16(),
        403
    );
    assert_eq!(
        world
            .app
            .call(MeAdminModeSet, &[])
            .token(&api_token)
            .body(&AdminModeSetBody { as_admin: true })
            .send()
            .await
            .status
            .as_u16(),
        403
    );
    mode(world, &other, true).await;
    mode(world, &by, false).await;
    let still_enabled = world
        .app
        .call(NodeGet, &[&private.base.id])
        .token(&other.token)
        .send()
        .await
        .ok();
    assert!(base(&still_enabled).access.admin_access.is_some());
    assert!(matches!(
        world
            .app
            .call(NodeGet, &[&private.base.id])
            .token(&by.token)
            .send()
            .await
            .status
            .as_u16(),
        403 | 404
    ));
}
