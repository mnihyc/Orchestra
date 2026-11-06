//! The native inventory and its counts share one operation boundary before their SQL window.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod chat_world;
mod places;
mod resources;
mod support;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use http::StatusCode;
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyScope, permissions::ResourceKind};
use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_contract::everything::{
    EverythingDirection, EverythingList, EverythingListQuery, EverythingRecent, EverythingScope,
    EverythingSort, ResourcesSummaryGet,
};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{hierarchy, identity, resources::access_fixtures};
use orch_types::{Id, Json, Time, new_id};
use places::{Who, admin, join, make_org, make_project, make_topic, person};
use serde_json::json;
use support::World;

struct Key {
    id: Id,
    secret: String,
}

fn policy(rules: Vec<Json>) -> Json {
    let scope: KeyScope = serde_json::from_value(json!({"rules": rules})).unwrap();
    assert!(scope.validate().is_empty());
    serde_json::to_value(scope).unwrap()
}

async fn issue(world: &World, user: Id, rules: Option<Vec<Json>>) -> Key {
    let scope = rules.map(policy);
    let scopes = if scope.is_none() {
        vec!["*".to_owned()]
    } else {
        vec![]
    };
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Native inventory scope control",
            token_hash: &token_hash(&secret),
            scopes: &scopes,
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        scope.as_ref(),
    )
    .await
    .unwrap();
    Key { id, secret }
}

fn rule(operation: &str, kind: &str, id: Id, reach: &str, business: &[&str], action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":business,
        "allow":[{"operation":operation,"actions":[action]}]})
}

async fn listing(world: &World, token: &str, query: &EverythingListQuery) -> RawReply {
    world
        .app
        .call(EverythingList, &[])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}

async fn summary(world: &World, token: &str) -> RawReply {
    world
        .app
        .call(ResourcesSummaryGet, &[])
        .token(token)
        .send_bytes()
        .await
}

fn shown(stage: &str, reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{stage}: {}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

fn ids(page: &Json) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect()
}

async fn resource(world: &World, owner: &Who, home: Option<Id>, kind: &str, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id = access_fixtures::insert_resource(&mut conn, kind, Some(owner.id()), home, name)
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: kind.parse::<ResourceKind>().unwrap().into(),
            id,
        },
        owner.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    id
}

fn resource_query(kind: &str) -> EverythingListQuery {
    EverythingListQuery {
        parent: Some("resources".into()),
        kinds: Some(kind.into()),
        sort: Some(EverythingSort::Name),
        direction: Some(EverythingDirection::Asc),
        ..Default::default()
    }
}

#[tokio::test]
async fn summary_counts_only_exact_current_resource_read_without_hydrating_or_counting_peers() {
    let world = World::new().await;
    let owner = person(&world, "count-owner").await;
    let root = make_org(&world, &owner, "Inventory counts", "inventory-counts")
        .await
        .base
        .id;
    let selected = resource(&world, &owner, Some(root), "template", "count-selected").await;
    resource(&world, &owner, Some(root), "template", "count-peer").await;
    for kind in [
        "provider",
        "agent",
        "environment",
        "host",
        "machine",
        "mcp_server",
        "mount",
        "skill",
    ] {
        resource(&world, &owner, Some(root), kind, &format!("count-{kind}")).await;
    }
    let browser = shown(
        "ordinary browser summary",
        summary(&world, &owner.token).await,
    );
    assert!(browser["templates"].as_i64().unwrap() >= 2);
    for field in [
        "providers",
        "agents",
        "environments",
        "hosts",
        "machines",
        "mcp",
        "mounts",
        "skills",
    ] {
        assert!(
            browser[field].as_i64().unwrap() > 0,
            "ordinary {field} fixture"
        );
    }
    let legacy = issue(&world, owner.id(), None).await;
    assert_eq!(
        shown(
            "ordinary legacy summary",
            summary(&world, &legacy.secret).await
        ),
        browser
    );
    let key = issue(
        &world,
        owner.id(),
        Some(vec![rule(
            "ResourcesSummaryGet",
            "template",
            selected,
            "self",
            &["template"],
            "read",
        )]),
    )
    .await;
    let bounded = shown(
        "modern exact Template summary",
        summary(&world, &key.secret).await,
    );
    assert_eq!(
        bounded,
        json!({"providers":0,"agents":0,"machines":0,"environments":0,"templates":1,"hosts":0,"mcp":0,"mounts":0,"skills":0})
    );
}

#[tokio::test]
async fn resource_self_read_filters_before_the_page_window_and_keeps_optional_columns_unknown() {
    let world = World::new().await;
    let owner = person(&world, "window-owner").await;
    let root = make_org(&world, &owner, "Inventory windows", "inventory-windows")
        .await
        .base
        .id;
    let first = resource(
        &world,
        &owner,
        Some(root),
        "template",
        "A excluded template",
    )
    .await;
    let selected = resource(
        &world,
        &owner,
        Some(root),
        "template",
        "Z selected template",
    )
    .await;
    let query = EverythingListQuery {
        limit: Some(1),
        ..resource_query("template")
    };
    let browser = shown(
        "ordinary browser first window",
        listing(&world, &owner.token, &query).await,
    );
    assert_eq!(ids(&browser), [first.to_string()]);
    assert!(browser["next_cursor"].is_string());
    let legacy = issue(&world, owner.id(), None).await;
    assert_eq!(
        shown(
            "ordinary legacy first window",
            listing(&world, &legacy.secret, &query).await
        ),
        browser
    );
    let key = issue(
        &world,
        owner.id(),
        Some(vec![rule(
            "EverythingList.mine",
            "template",
            selected,
            "self",
            &["template"],
            "read",
        )]),
    )
    .await;
    let page = shown(
        "modern key window after filtering",
        listing(&world, &key.secret, &query).await,
    );
    assert_eq!(ids(&page), [selected.to_string()]);
    assert!(
        page["next_cursor"].is_null(),
        "no false partial window remains"
    );
    assert_eq!(page["items"][0]["name"], "Z selected template");
    assert!(
        page["items"][0]["resource"].is_null(),
        "ordinary non-rich projection stays non-rich"
    );
    assert!(page["items"][0]["takes"].is_null());
    for step in page["items"][0]["path"].as_array().unwrap() {
        assert!(
            step["name"].is_null(),
            "ungranted ancestor names remain redacted"
        );
    }
}

#[tokio::test]
async fn native_resource_use_keeps_its_view_projection_and_scope_bounded_names() {
    let world = World::new().await;
    let owner = person(&world, "use-owner").await;
    let reader = person(&world, "use-reader").await;
    let root = make_org(&world, &owner, "Inventory use", "inventory-use")
        .await
        .base
        .id;
    join(&world, root, root, &reader, "viewer", &owner).await;
    let selected = resource(&world, &owner, Some(root), "template", "use-template").await;
    resources::name_person(&world, "template", selected, "template.use", reader.id()).await;
    let query = resource_query("template");
    let browser = shown(
        "ordinary resource Use includes View",
        listing(&world, &reader.token, &query).await,
    );
    assert_eq!(ids(&browser), [selected.to_string()]);
    assert_eq!(browser["items"][0]["my_access"], "take_part");
    assert_eq!(browser["items"][0]["name"], "use-template");
    assert_eq!(browser["items"][0]["owner"]["id"], json!(owner.id()));
    let key = issue(
        &world,
        reader.id(),
        Some(vec![
            rule(
                "EverythingList.mine",
                "template",
                selected,
                "self",
                &["template"],
                "read",
            ),
            rule(
                "ResourcesSummaryGet",
                "template",
                selected,
                "self",
                &["template"],
                "read",
            ),
        ]),
    )
    .await;
    let page = shown(
        "modern resource Use keeps its native View projection",
        listing(&world, &key.secret, &query).await,
    );
    assert_eq!(ids(&page), [selected.to_string()]);
    assert_eq!(page["items"][0]["my_access"], "take_part");
    for field in [
        "id",
        "kind",
        "name",
        "owner",
        "created_at",
        "created_by",
        "last_activity",
        "state",
        "size_bytes",
        "takes",
        "my_access",
        "tags",
        "href",
        "can_share",
        "settings_href",
        "resource",
    ] {
        assert_eq!(
            page["items"][0][field], browser["items"][0][field],
            "native resource Use intrinsic {field}"
        );
    }
    for step in page["items"][0]["path"].as_array().unwrap() {
        assert!(step["name"].is_null(), "unselected home names stay hidden");
    }
    assert_eq!(
        shown(
            "modern resource Use counted",
            summary(&world, &key.secret).await
        )["templates"],
        1
    );
}

struct Tree {
    world: World,
    owner: Who,
    topic: Id,
    threads: [Id; 2],
    documents: [Id; 2],
}

impl Tree {
    async fn new() -> Self {
        let world = World::new().await;
        let owner = person(&world, "tree-owner").await;
        let root = make_org(
            &world,
            &owner,
            "Inventory private root",
            "inventory-private-root",
        )
        .await
        .base
        .id;
        let project = make_project(&world, &owner, root, "Inventory private project")
            .await
            .base
            .id;
        let topic = make_topic(&world, &owner, project, "Inventory topic")
            .await
            .base
            .id;
        let threads = [new_id(), new_id()];
        for (index, thread) in threads.iter().enumerate() {
            hierarchy::testing::insert_node(
                &mut *world.conn().await,
                *thread,
                "thread",
                Some(topic),
                &format!("Scope fixture thread {index}"),
                owner.id(),
            )
            .await
            .unwrap();
        }
        let mut documents = vec![];
        for (index, thread) in threads.iter().enumerate() {
            let document = world
                .app
                .call(NodeDocumentCreate, &[thread])
                .token(&owner.token)
                .body(&NodeDocumentCreateBody {
                    title: format!("Scope fixture document {index}"),
                    blocks: None,
                    markdown: None,
                    access: None,
                })
                .send()
                .await
                .ok();
            documents.push(document.base.id);
        }
        Self {
            world,
            owner,
            topic,
            threads,
            documents: documents.try_into().unwrap(),
        }
    }
    fn query(&self) -> EverythingListQuery {
        EverythingListQuery {
            q: Some("Scope fixture".into()),
            kinds: Some("thread,document".into()),
            sort: Some(EverythingSort::Name),
            direction: Some(EverythingDirection::Asc),
            ..Default::default()
        }
    }
}

#[tokio::test]
async fn a_thread_self_selection_bounds_the_native_default_thread_profile_without_reading_its_peers()
 {
    let s = Tree::new().await;
    let query = EverythingListQuery {
        kinds: Some("thread".into()),
        ..s.query()
    };
    let browser = shown(
        "ordinary default threads",
        listing(&s.world, &s.owner.token, &query).await,
    );
    assert_eq!(ids(&browser).len(), 2);
    let custom: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_rows WHERE object_kind='node' AND object_id=ANY($1) AND page='live'")
        .bind(s.threads.to_vec()).fetch_one(&s.world.db.pool).await.unwrap();
    assert_eq!(
        custom, 0,
        "use actual inherited ordinary threads, not custom pages"
    );
    let key = issue(
        &s.world,
        s.owner.id(),
        Some(vec![rule(
            "EverythingList.mine",
            "thread",
            s.threads[1],
            "self",
            &["thread"],
            "read",
        )]),
    )
    .await;
    let page = shown(
        "modern Thread self with no custom page",
        listing(&s.world, &key.secret, &query).await,
    );
    assert_eq!(ids(&page), [s.threads[1].to_string()]);
    assert!(page["next_cursor"].is_null());
    for step in page["items"][0]["path"].as_array().unwrap() {
        if step["id"] != json!(s.threads[1]) {
            assert!(step["name"].is_null());
        }
    }
}

#[tokio::test]
async fn a_thread_subtree_selects_its_document_and_paginates_after_scope_filtering() {
    let s = Tree::new().await;
    let browser = shown(
        "ordinary threads and documents",
        listing(&s.world, &s.owner.token, &s.query()).await,
    );
    let expected_ids: Vec<String> = s
        .threads
        .iter()
        .chain(s.documents.iter())
        .map(ToString::to_string)
        .collect();
    let mut native_ids = ids(&browser);
    native_ids.sort();
    let mut expected_set = expected_ids;
    expected_set.sort();
    assert_eq!(native_ids, expected_set, "complete ordinary fixture set");
    // Search ranks relevance, visits and activity before the selected Name column.
    // Keep its native order, while proving the actual selected subtree has both rows.
    let native_selected: Vec<String> = browser["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| {
            row["path"]
                .as_array()
                .unwrap()
                .iter()
                .any(|step| step["id"] == json!(s.threads[1]))
        })
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect();
    let mut selected_set = native_selected.clone();
    selected_set.sort();
    let mut expected_selected = vec![s.threads[1].to_string(), s.documents[1].to_string()];
    expected_selected.sort();
    assert_eq!(
        selected_set, expected_selected,
        "actual ordinary subtree set"
    );
    let key = issue(
        &s.world,
        s.owner.id(),
        Some(vec![rule(
            "EverythingList.mine",
            "thread",
            s.threads[1],
            "subtree",
            &["thread", "document"],
            "read",
        )]),
    )
    .await;
    let first_query = EverythingListQuery {
        limit: Some(1),
        ..s.query()
    };
    let first = shown(
        "modern subtree first window",
        listing(&s.world, &key.secret, &first_query).await,
    );
    assert_eq!(
        ids(&first),
        [native_selected[0].clone()],
        "first subtree window: {first}; native selected order: {native_selected:?}"
    );
    assert!(first["next_cursor"].is_string());
    let second_query = EverythingListQuery {
        cursor: Some(first["next_cursor"].as_str().unwrap().into()),
        ..first_query
    };
    let second = shown(
        "modern subtree next window",
        listing(&s.world, &key.secret, &second_query).await,
    );
    assert_eq!(
        ids(&second),
        [native_selected[1].clone()],
        "second subtree window: {second}; native selected order: {native_selected:?}"
    );
    assert!(second["next_cursor"].is_null());
    let peer = issue(
        &s.world,
        s.owner.id(),
        Some(vec![rule(
            "EverythingList.mine",
            "topic",
            s.topic,
            "self",
            &["thread", "document"],
            "read",
        )]),
    )
    .await;
    assert!(
        ids(&shown(
            "Topic self is not its contents",
            listing(&s.world, &peer.secret, &s.query()).await
        ))
        .is_empty()
    );
}

#[tokio::test]
async fn physical_machine_counts_and_rows_use_the_host_subject_with_the_machine_business_family() {
    let world = World::new().await;
    let owner = person(&world, "facade-owner").await;
    let root = make_org(&world, &owner, "Inventory facade", "inventory-facade")
        .await
        .base
        .id;
    let host = resource(&world, &owner, Some(root), "host", "inventory-host").await;
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let query = resource_query("machine");
    assert!(
        ids(&shown(
            "ordinary physical machine facade",
            listing(&world, &owner.token, &query).await
        ))
        .contains(&machine.to_string())
    );
    let key = issue(
        &world,
        owner.id(),
        Some(vec![
            rule(
                "EverythingList.mine",
                "host",
                host,
                "self",
                &["machine"],
                "read",
            ),
            rule(
                "ResourcesSummaryGet",
                "host",
                host,
                "self",
                &["machine"],
                "read",
            ),
        ]),
    )
    .await;
    assert_eq!(
        ids(&shown(
            "modern Machine family actual Host subject",
            listing(&world, &key.secret, &query).await
        )),
        [machine.to_string()]
    );
    let count = shown(
        "modern Machine count actual Host subject",
        summary(&world, &key.secret).await,
    );
    assert_eq!(count["machines"], 1);
    assert_eq!(
        count["hosts"], 0,
        "Machine family must not implicitly grant Host family"
    );
    let literal = issue(
        &world,
        owner.id(),
        Some(vec![rule(
            "ResourcesSummaryGet",
            "machine",
            machine,
            "self",
            &["machine"],
            "read",
        )]),
    )
    .await;
    assert_eq!(
        shown(
            "literal Machine id cannot replace Host facade",
            summary(&world, &literal.secret).await
        )["machines"],
        0
    );
}

#[tokio::test]
async fn endpoint_actions_and_current_rules_never_borrow_each_others_inventory_grants() {
    let world = World::new().await;
    let owner = person(&world, "current-owner").await;
    let root = make_org(&world, &owner, "Inventory current", "inventory-current")
        .await
        .base
        .id;
    let first = resource(&world, &owner, Some(root), "template", "current-first").await;
    let second = resource(&world, &owner, Some(root), "template", "current-second").await;
    let query = resource_query("template");
    for (operation, action) in [("TemplateList", "read"), ("EverythingList.mine", "write")] {
        let key = issue(
            &world,
            owner.id(),
            Some(vec![rule(
                operation,
                "template",
                first,
                "self",
                &["template"],
                action,
            )]),
        )
        .await;
        assert_eq!(
            listing(&world, &key.secret, &query).await.status,
            StatusCode::FORBIDDEN,
            "wrong endpoint or action"
        );
    }
    let key = issue(
        &world,
        owner.id(),
        Some(vec![rule(
            "EverythingList.mine",
            "template",
            first,
            "self",
            &["template"],
            "read",
        )]),
    )
    .await;
    assert_eq!(
        summary(&world, &key.secret).await.status,
        StatusCode::FORBIDDEN,
        "list Read is not summary Read"
    );
    assert_eq!(
        ids(&shown(
            "same key first current scope",
            listing(&world, &key.secret, &query).await
        )),
        [first.to_string()]
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(key.id)
        .bind(policy(vec![rule(
            "EverythingList.mine",
            "template",
            second,
            "self",
            &["template"],
            "read",
        )]))
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        ids(&shown(
            "same key fresh current scope",
            listing(&world, &key.secret, &query).await
        )),
        [second.to_string()]
    );
}

#[tokio::test]
async fn a_scoped_read_does_not_add_ordinary_visibility_or_system_private_override() {
    let world = World::new().await;
    let owner = person(&world, "private-owner").await;
    let outsider = person(&world, "private-outsider").await;
    let root = make_org(&world, &owner, "Inventory private", "inventory-private")
        .await
        .base
        .id;
    let hidden = resource(&world, &owner, Some(root), "template", "private-template").await;
    let query = resource_query("template");
    assert!(
        ids(&shown(
            "ordinary hidden list",
            listing(&world, &outsider.token, &query).await
        ))
        .is_empty()
    );
    let key = issue(
        &world,
        outsider.id(),
        Some(vec![
            rule(
                "EverythingList.mine",
                "template",
                hidden,
                "self",
                &["template"],
                "read",
            ),
            rule(
                "ResourcesSummaryGet",
                "template",
                hidden,
                "self",
                &["template"],
                "read",
            ),
        ]),
    )
    .await;
    assert!(
        ids(&shown(
            "key cannot add ordinary View",
            listing(&world, &key.secret, &query).await
        ))
        .is_empty()
    );
    assert_eq!(
        shown(
            "key cannot add ordinary discoverability",
            summary(&world, &key.secret).await
        )["templates"],
        0
    );
    let native = admin(&world, "private-native").await;
    let system = EverythingListQuery {
        scope: Some(EverythingScope::System),
        ..query
    };
    assert!(
        ids(&shown(
            "ordinary explicit As admin system inventory",
            listing(&world, &native.token, &system).await
        ))
        .contains(&hidden.to_string())
    );
    let legacy = issue(&world, native.id(), None).await;
    assert_eq!(
        listing(&world, &legacy.secret, &system).await.status,
        StatusCode::FORBIDDEN,
        "legacy key cannot acquire As admin"
    );
    let scoped = issue(&world, native.id(), Some(vec![
        json!({"target":{"kind":"system","reach":"self"},"allow":[{"operation":"EverythingList.system","actions":["manage"]}]}),
        rule("EverythingList.system", "template", hidden, "self", &["template"], "manage"),
    ])).await;
    assert_eq!(
        listing(&world, &scoped.secret, &system).await.status,
        StatusCode::FORBIDDEN,
        "system operation scope is not private inspection"
    );
}

async fn independently_granted_placements(summary_only: bool) {
    let world = World::new().await;
    let owner = person(&world, "placement-owner").await;
    let root = make_org(&world, &owner, "Inventory placement", "inventory-placement")
        .await
        .base
        .id;
    let org = resource(&world, &owner, Some(root), "template", "org-template").await;
    let personal = resource(&world, &owner, None, "template", "personal-template").await;
    let system = access_fixtures::insert_resource(
        &mut *world.conn().await,
        "template",
        None,
        None,
        "system-template",
    )
    .await
    .unwrap();
    // A normal explicit page grant supplies View: neither system placement nor
    // first-owner metadata is being used as the browser's private override.
    for id in [personal, system] {
        resources::name_person(&world, "template", id, "template.view", owner.id()).await;
    }
    let operation = if summary_only {
        "ResourcesSummaryGet"
    } else {
        "EverythingList.mine"
    };
    let read = issue(
        &world,
        owner.id(),
        Some(vec![
            rule(
                operation,
                "template",
                personal,
                "self",
                &["template"],
                "read",
            ),
            rule(operation, "template", system, "self", &["template"], "read"),
        ]),
    )
    .await;
    let legacy = issue(&world, owner.id(), None).await;
    if summary_only {
        let browser = shown(
            "ordinary browser explicitly granted placements count",
            summary(&world, &owner.token).await,
        );
        assert!(browser["templates"].as_i64().unwrap() >= 3);
        assert_eq!(
            shown(
                "ordinary legacy explicitly granted placements count",
                summary(&world, &legacy.secret).await
            ),
            browser
        );
        assert_eq!(
            shown(
                "modern personal and system normal page counts",
                summary(&world, &read.secret).await
            )["templates"],
            2
        );
    } else {
        let query = resource_query("template");
        let browser = shown(
            "ordinary browser explicitly granted placements list",
            listing(&world, &owner.token, &query).await,
        );
        let ordinary = ids(&browser);
        for id in [org, personal, system] {
            assert!(
                ordinary.contains(&id.to_string()),
                "ordinary placement {id}"
            );
        }
        assert_eq!(
            shown(
                "ordinary legacy explicitly granted placements list",
                listing(&world, &legacy.secret, &query).await
            ),
            browser
        );
        let page = shown(
            "modern personal and system normal page list",
            listing(&world, &read.secret, &query).await,
        );
        let mut expected = vec![personal.to_string(), system.to_string()];
        let mut actual = ids(&page);
        expected.sort();
        actual.sort();
        assert_eq!(actual, expected);
        assert!(page["next_cursor"].is_null());
    }
}

#[tokio::test]
async fn summary_retains_normal_personal_and_system_page_grants_without_a_system_override() {
    independently_granted_placements(true).await;
}

#[tokio::test]
async fn everything_retains_normal_personal_and_system_page_grants_without_a_system_override() {
    independently_granted_placements(false).await;
}

#[tokio::test]
async fn a_rich_template_keeps_its_native_recipe_and_uses_own_operation_bounds_for_home_names() {
    let world = World::new().await;
    let owner = person(&world, "rich-owner").await;
    let root = make_org(&world, &owner, "Private recipe home", "private-recipe-home")
        .await
        .base
        .id;
    let template = resource(&world, &owner, Some(root), "template", "bounded-recipe").await;
    let spec = json!({"image":"fixture:stored","marker":"keep-native-recipe"});
    sqlx::query("UPDATE env_templates SET spec=$2 WHERE id=$1")
        .bind(template)
        .bind(&spec)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let query = EverythingListQuery {
        resource_details: Some(true),
        ..resource_query("template")
    };
    let browser = shown(
        "ordinary browser rich Template",
        listing(&world, &owner.token, &query).await,
    );
    assert_eq!(ids(&browser), [template.to_string()]);
    let ordinary = &browser["items"][0]["resource"]["value"];
    assert_eq!(ordinary["spec"], spec);
    assert_eq!(ordinary["home"]["name"], "Private recipe home");
    let legacy = issue(&world, owner.id(), None).await;
    assert_eq!(
        shown(
            "ordinary legacy rich Template",
            listing(&world, &legacy.secret, &query).await
        ),
        browser
    );
    let primary = rule(
        "EverythingList.mine",
        "template",
        template,
        "self",
        &["template"],
        "read",
    );
    let key = issue(&world, owner.id(), Some(vec![primary.clone()])).await;
    let page = shown(
        "modern rich Template with ungranted home name",
        listing(&world, &key.secret, &query).await,
    );
    assert_eq!(ids(&page), [template.to_string()]);
    let bounded = &page["items"][0]["resource"]["value"];
    for field in [
        "id",
        "name",
        "description",
        "driver",
        "kind",
        "spec",
        "units",
        "enabled",
        "revision",
        "owner",
    ] {
        assert_eq!(
            bounded[field], ordinary[field],
            "native Template {field} remains complete"
        );
    }
    assert_eq!(bounded["home"]["id"], json!(root));
    assert!(bounded["home"]["name"].is_null());
    assert_eq!(bounded["home"]["redacted"], true);
    let with_name = issue(
        &world,
        owner.id(),
        Some(vec![
            primary,
            rule("EverythingList.mine", "org", root, "self", &["org"], "read"),
        ]),
    )
    .await;
    let page = shown(
        "modern rich Template with actual home Read",
        listing(&world, &with_name.secret, &query).await,
    );
    assert_eq!(
        page["items"][0]["resource"]["value"]["home"],
        ordinary["home"]
    );
}

async fn without_document_seeds() -> Tree {
    let scene = Tree::new().await;
    sqlx::query("UPDATE documents SET deleted_at=$2 WHERE id=ANY($1)")
        .bind(scene.documents.to_vec())
        .bind(scene.world.now())
        .execute(&scene.world.db.pool)
        .await
        .unwrap();
    let seeds = orch_db::queries::everything::access_seeds(&mut *scene.world.conn().await, None)
        .await
        .unwrap();
    assert!(
        scene.threads.iter().all(|id| !seeds
            .iter()
            .any(|seed| seed.kind == "node" && seed.id == *id)),
        "ordinary main threads must not enter native seeds through documents or custom pages"
    );
    scene
}

#[tokio::test]
async fn an_exact_main_thread_with_no_document_or_custom_page_is_discovered_as_a_real_identity() {
    let scene = without_document_seeds().await;
    let query = EverythingListQuery {
        kinds: Some("thread".into()),
        ..scene.query()
    };
    assert_eq!(
        ids(&shown(
            "ordinary unexpanded main threads",
            listing(&scene.world, &scene.owner.token, &query).await
        ))
        .len(),
        2
    );
    let key = issue(
        &scene.world,
        scene.owner.id(),
        Some(vec![rule(
            "EverythingList.mine",
            "thread",
            scene.threads[1],
            "self",
            &["thread"],
            "read",
        )]),
    )
    .await;
    let page = shown(
        "modern exact unexpanded main Thread",
        listing(&scene.world, &key.secret, &query).await,
    );
    assert_eq!(ids(&page), [scene.threads[1].to_string()]);
    assert!(page["next_cursor"].is_null());
}

#[tokio::test]
async fn exact_chat_read_keeps_an_unnamed_row_without_loaning_the_hidden_keeper_title_to_search_or_cursor()
 {
    let scene = without_document_seeds().await;
    let chats: Vec<(Id, Id, Option<String>)> =
        sqlx::query_as("SELECT id,node_id,name FROM chats WHERE node_id=ANY($1) ORDER BY id")
            .bind(scene.threads.to_vec())
            .fetch_all(&scene.world.db.pool)
            .await
            .unwrap();
    assert_eq!(chats.len(), 2);
    assert!(
        chats.iter().all(|(_, _, name)| name.is_none()),
        "exercise the native keeping-node title fallback"
    );
    // The root branch does not flatten descendant Chats merely from kinds=chat.
    // Read the actual Chats and record real native visits for the flat opened view.
    for (chat, _, _) in &chats {
        scene
            .world
            .app
            .call(orch_api_contract::chats::ChatGet, &[chat])
            .token(&scene.owner.token)
            .send()
            .await
            .ok();
        scene
            .world
            .app
            .call(orch_api_contract::personal::MeVisitCreate, &[])
            .token(&scene.owner.token)
            .body(&orch_api_contract::personal::PersonalTarget {
                kind: orch_api_contract::search::NameKind::Chat,
                id: *chat,
            })
            .send()
            .await
            .ok();
    }
    let visited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recents WHERE user_id=$1 AND kind='chat' AND id=ANY($2)",
    )
    .bind(scene.owner.id())
    .bind(chats.iter().map(|(chat, _, _)| *chat).collect::<Vec<_>>())
    .fetch_one(&scene.world.db.pool)
    .await
    .unwrap();
    assert_eq!(visited, 2, "both readable Chats have actual native visits");
    let query = EverythingListQuery {
        kinds: Some("chat".into()),
        recent: Some(EverythingRecent::Opened),
        limit: Some(1),
        sort: Some(EverythingSort::Name),
        direction: Some(EverythingDirection::Asc),
        ..Default::default()
    };
    let ordinary = shown(
        "ordinary Chat names from readable keepers",
        listing(
            &scene.world,
            &scene.owner.token,
            &EverythingListQuery {
                limit: Some(200),
                ..query.clone()
            },
        )
        .await,
    );
    for (chat, _, _) in &chats {
        let row = ordinary["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == json!(chat))
            .unwrap();
        assert!(
            row["name"]
                .as_str()
                .unwrap()
                .starts_with("Scope fixture thread")
        );
    }
    let rules = chats
        .iter()
        .map(|(chat, _, _)| {
            rule(
                "EverythingList.mine",
                "chat",
                *chat,
                "self",
                &["chat"],
                "read",
            )
        })
        .collect();
    let key = issue(&scene.world, scene.owner.id(), Some(rules)).await;
    let first = shown(
        "modern actual Chats, no keeper main Read",
        listing(&scene.world, &key.secret, &query).await,
    );
    assert_eq!(
        ids(&first),
        [chats[0].0.to_string()],
        "redacted titles must not influence name ordering"
    );
    assert!(first["items"][0]["name"].is_null());
    assert!(
        first["items"][0]["href"].is_string(),
        "the native safe unnamed Chat row remains usable"
    );
    let cursor = first["next_cursor"].as_str().unwrap();
    let decoded = String::from_utf8(URL_SAFE_NO_PAD.decode(cursor).unwrap()).unwrap();
    assert!(
        !decoded.contains("Scope fixture thread"),
        "the hidden title must not enter its cursor"
    );
    let next = shown(
        "modern unnamed Chat next window",
        listing(
            &scene.world,
            &key.secret,
            &EverythingListQuery {
                cursor: Some(cursor.into()),
                ..query.clone()
            },
        )
        .await,
    );
    assert_eq!(ids(&next), [chats[1].0.to_string()]);
    assert!(next["items"][0]["name"].is_null());
    assert!(next["next_cursor"].is_null());
    let hidden_search = shown(
        "hidden keeper title is not a Chat name hit",
        listing(
            &scene.world,
            &key.secret,
            &EverythingListQuery {
                q: Some("Scope fixture thread".into()),
                recent: None,
                ..query
            },
        )
        .await,
    );
    assert!(ids(&hidden_search).is_empty());
}

#[tokio::test]
async fn library_source_names_intersect_the_same_native_privacy_projection_without_changing_counts()
{
    let world = World::new().await;
    let owner = person(&world, "library-owner").await;
    let root = make_org(&world, &owner, "Library source team", "library-source-team")
        .await
        .base
        .id;
    let template = resource(&world, &owner, Some(root), "template", "library-recipe").await;
    let environment = resource(
        &world,
        &owner,
        Some(root),
        "environment",
        "Private dependent environment",
    )
    .await;
    sqlx::query("UPDATE environments SET template_id=$2 WHERE id=$1")
        .bind(environment)
        .bind(template)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let query = EverythingListQuery {
        resource_details: Some(true),
        q: Some("library-recipe".into()),
        ..resource_query("template")
    };
    let ordinary = shown(
        "ordinary Template current library sources",
        listing(&world, &owner.token, &query).await,
    );
    assert_eq!(ids(&ordinary), [template.to_string()]);
    let group = |page: &Json| {
        page["items"][0]["library"]["dependents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|group| group["relation"] == "attachment")
            .unwrap()
            .clone()
    };
    let visible = group(&ordinary);
    assert_eq!(visible["count"], 1);
    assert_eq!(
        visible["visible"][0]["name"],
        "Private dependent environment"
    );
    let primary = rule(
        "EverythingList.mine",
        "template",
        template,
        "self",
        &["template"],
        "read",
    );
    let key = issue(&world, owner.id(), Some(vec![primary.clone()])).await;
    let hidden = shown(
        "modern library source withheld by own operation scope",
        listing(&world, &key.secret, &query).await,
    );
    let hidden = group(&hidden);
    assert_eq!(hidden["count"], visible["count"]);
    assert_eq!(hidden["hidden"], 1);
    assert!(hidden["visible"].as_array().unwrap().is_empty());
    let permitted = issue(
        &world,
        owner.id(),
        Some(vec![
            primary,
            rule(
                "EverythingList.mine",
                "environment",
                environment,
                "self",
                &["environment"],
                "read",
            ),
        ]),
    )
    .await;
    let restored = shown(
        "modern library source with actual own Read",
        listing(&world, &permitted.secret, &query).await,
    );
    assert_eq!(group(&restored), visible);
}

#[tokio::test]
async fn expired_ordinary_chat_membership_cannot_keep_inventory_or_finder_names_after_chatget_refuses()
 {
    use orch_api_contract::chats::{ChatGet, ChatMemberAdd};
    use orch_api_contract::search::{NameSearchList, NameSearchListQuery};
    let room = chat_world::Room::new().await;
    let chat = room
        .group(&room.cai, &[&room.bea], Some("Expiring guest room"))
        .await
        .base
        .id;
    room.world
        .app
        .call(ChatMemberAdd, &[&chat, &room.eve.id()])
        .token(&room.cai.token)
        .send()
        .await
        .ok();
    let expires = room.world.now() + chrono::Duration::seconds(1);
    sqlx::query("UPDATE chat_members SET expires_at=$3 WHERE chat_id=$1 AND user_id=$2")
        .bind(chat)
        .bind(room.eve.id())
        .bind(expires)
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    let query = EverythingListQuery {
        kinds: Some("chat".into()),
        q: Some("Expiring guest room".into()),
        ..Default::default()
    };
    let search = NameSearchListQuery {
        kinds: Some("chat".into()),
        q: Some("Expiring guest room".into()),
        ..Default::default()
    };
    assert_eq!(
        room.world
            .app
            .call(ChatGet, &[&chat])
            .token(&room.eve.token)
            .send_bytes()
            .await
            .status,
        StatusCode::OK,
        "ordinary active guest ChatGet premise"
    );
    assert!(
        ids(&shown(
            "ordinary active guest inventory premise",
            listing(&room.world, &room.eve.token, &query).await
        ))
        .contains(&chat.to_string())
    );
    let active_search = shown(
        "ordinary active guest finder premise",
        room.world
            .app
            .call(NameSearchList, &[])
            .token(&room.eve.token)
            .query(&search)
            .send_bytes()
            .await,
    );
    assert!(ids(&active_search).contains(&chat.to_string()));
    room.world.advance(chrono::Duration::seconds(2));
    let expired: bool = sqlx::query_scalar("SELECT guest AND left_at IS NULL AND expires_at<=$3 FROM chat_members WHERE chat_id=$1 AND user_id=$2")
        .bind(chat).bind(room.eve.id()).bind(room.world.now()).fetch_one(&room.world.db.pool).await.unwrap();
    assert!(
        expired,
        "actual timed guest membership expired without a leave or broad role mutation"
    );
    let refused = room
        .world
        .app
        .call(ChatGet, &[&chat])
        .token(&room.eve.token)
        .send_bytes()
        .await;
    assert_eq!(
        refused.status,
        StatusCode::NOT_FOUND,
        "canonical ordinary ChatGet no longer licenses this name"
    );
    let after = shown(
        "ordinary expired guest inventory",
        listing(&room.world, &room.eve.token, &query).await,
    );
    let found = shown(
        "ordinary expired guest finder",
        room.world
            .app
            .call(NameSearchList, &[])
            .token(&room.eve.token)
            .query(&search)
            .send_bytes()
            .await,
    );
    let inventory_visible = ids(&after).contains(&chat.to_string());
    let finder_visible = ids(&found).contains(&chat.to_string());
    assert!(
        !inventory_visible && !finder_visible,
        "expired guest still disclosed: inventory={inventory_visible}, finder={finder_visible}"
    );
    assert_eq!(
        room.world
            .app
            .call(ChatGet, &[&chat])
            .token(&room.bea.token)
            .send_bytes()
            .await
            .status,
        StatusCode::OK,
        "another still-valid member retains native access"
    );
    assert!(
        ids(&shown(
            "ordinary valid member inventory after peer expiry",
            listing(&room.world, &room.bea.token, &query).await
        ))
        .contains(&chat.to_string())
    );
}
