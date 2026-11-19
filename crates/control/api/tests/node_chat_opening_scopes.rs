//! One Node opening binds its real root, independently readable chats and requested Defaults.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod rig;
mod support;

use chat_world::{Room, text};
use orch_api_contract::access::Role;
use orch_api_contract::chats::{
    ChatDraftSet, ChatDraftSetBody, ChatMentionTarget, ChatPart, NodeChatOpeningGet,
};
use orch_api_contract::directory::{GroupMemberAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_api_contract::resources::{CreateProvider, ProviderCreate, ProviderGet, Upstream};
use orch_api_contract::settings::{
    NodeSecretSet, NodeSecretSetBody, NodeSettingList, NodeSettingSet, NodeSettingSetBody,
    SettingRegistryGet,
};
use orch_types::{Id, new_id};
use serde_json::{Value, json};

const OP: &str = "NodeChatOpeningGet";
const SECRET: &str = "node-opening-secret-value-never-returned";
const PROVIDER: &str = "Selected private opening provider";

struct Key {
    id: Id,
    secret: String,
}
fn rule_for(operation: &str, kind: &str, id: Id) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},
        "allow":[{"operation":operation,"actions":["read"]}]})
}
fn rule(kind: &str, id: Id) -> Value {
    rule_for(OP, kind, id)
}
fn system_for(operation: &str) -> Value {
    json!({"target":{"kind":"system","reach":"self"},
        "allow":[{"operation":operation,"actions":["read"]}]})
}
async fn key(room: &Room, user: Id, rules: Vec<Value>) -> Key {
    key_in(&room.world, user, rules).await
}

async fn key_in(world: &support::World, user: Id, rules: Vec<Value>) -> Key {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let scope = serde_json::to_value(scope).unwrap();
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Node opening scope control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            // Fixed-clock HTTP sequences fit the established short key-test lifetime.
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn open(room: &Room, token: &str, node: Id, query: &str) -> (u16, Value) {
    open_in(&room.world, token, node, query).await
}

async fn open_in(world: &support::World, token: &str, node: Id, query: &str) -> (u16, Value) {
    let r = world
        .app
        .call(NodeChatOpeningGet, &[&node])
        .token(token)
        .raw_query(query)
        .send_bytes()
        .await;
    (r.status.as_u16(), serde_json::from_slice(&r.raw).unwrap())
}
fn ok(reply: (u16, Value)) -> Value {
    assert_eq!(reply.0, 200, "{}", reply.1);
    reply.1
}
fn scopes(body: &Value) -> Vec<Id> {
    body["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| serde_json::from_value(s["chat"]["id"].clone()).unwrap())
        .collect()
}
fn selected(body: &Value) -> Id {
    serde_json::from_value(body["opening"]["sync"]["chat"]["id"].clone()).unwrap()
}
fn message<'a>(body: &'a Value, id: Id) -> &'a Value {
    body["opening"]["sync"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == json!(id))
        .unwrap()
}

#[tokio::test]
async fn an_exact_root_and_chat_grant_opens_only_that_conversation_and_the_callers_draft() {
    let room = Room::new().await;
    let parent = chat_world::chat_of(&room.world, &room.ada, room.project).await;
    let said = room
        .say(&room.ada, room.chat, "The actual selected conversation")
        .await;
    room.say(&room.ada, parent, "Private unselected parent conversation")
        .await;
    room.world
        .app
        .call(ChatDraftSet, &[&room.chat])
        .token(&room.ada.token)
        .body(&ChatDraftSetBody {
            parts: vec![text("Only this account's composer")],
            reply_to: None,
        })
        .send()
        .await
        .ok();
    let browser = ok(open(&room, &room.ada.token, room.topic, "scope=project&limit=20").await);
    assert!(scopes(&browser).contains(&room.chat) && scopes(&browser).contains(&parent));
    assert_eq!(
        selected(&browser),
        parent,
        "ordinary preference is meaningful before key filtering"
    );
    let token = key(
        &room,
        room.ada.id(),
        vec![rule("topic", room.topic), rule("chat", room.chat)],
    )
    .await;
    let opened = ok(open(&room, &token.secret, room.topic, "scope=project&limit=20").await);
    assert_eq!(scopes(&opened), [room.chat]);
    assert_eq!(
        selected(&opened),
        room.chat,
        "preference falls back only among admitted scopes"
    );
    assert_eq!(message(&opened, said.id)["parts"], json!(said.parts));
    assert_eq!(
        opened["opening"]["draft"]["parts"][0]["text"],
        "Only this account's composer"
    );
    assert!(
        !opened
            .to_string()
            .contains("Private unselected parent conversation")
    );
    let other = key(
        &room,
        room.bea.id(),
        vec![rule("topic", room.topic), rule("chat", room.chat)],
    )
    .await;
    let other = ok(open(&room, &other.secret, room.topic, "limit=20").await);
    assert!(
        other["opening"]["draft"].is_null(),
        "one principal's draft is never lent to another"
    );
}

#[tokio::test]
async fn root_read_and_chat_read_are_independent_and_a_real_empty_chat_selection_is_valid() {
    let room = Room::new().await;
    let ordinary = ok(open(&room, &room.ada.token, room.topic, "limit=10").await);
    assert!(!scopes(&ordinary).is_empty());
    let no_root = key(
        &room,
        room.ada.id(),
        vec![
            rule("chat", room.chat),
            rule_for("NodeGet.read", "topic", room.topic),
        ],
    )
    .await;
    let refused = open(&room, &no_root.secret, room.topic, "limit=10").await;
    assert_eq!(
        refused.0, 403,
        "a child reader's operation cannot donate root authority: {}",
        refused.1
    );
    let root_only = key(
        &room,
        room.ada.id(),
        vec![
            rule("topic", room.topic),
            rule_for("ChatOpeningGet", "chat", room.chat),
        ],
    )
    .await;
    let empty = ok(open(&room, &root_only.secret, room.topic, "limit=10").await);
    assert!(scopes(&empty).is_empty());
    assert!(empty["opening"].is_null());
    let no_optional = ok(open(
        &room,
        &root_only.secret,
        room.topic,
        "defaults=false&secret_names=true&default_provider_name=true&embeddings_provider_name=true",
    )
    .await);
    assert!(
        no_optional.get("defaults").is_none(),
        "request bits do not enable an unrequested editor"
    );
}

#[tokio::test]
async fn a_topic_viewer_never_borrows_its_parent_chat_or_a_foreign_preferred_id() {
    let room = Room::new().await;
    {
        let mut conn = room.world.conn().await;
        for permission in ["topic.view", "chat.view"] {
            orch_db::queries::access::testing::add_entry(
                &mut conn,
                &orch_db::queries::access::PageKey::new("node", room.topic, "live"),
                "any",
                permission,
                ("user", Some(room.eve.id()), None),
                None,
                room.world.now(),
            )
            .await
            .unwrap();
        }
    }
    let parent = chat_world::chat_of(&room.world, &room.ada, room.project).await;
    let query = format!("scope=project&chat_id={parent}&limit=10");
    let ordinary = ok(open(&room, &room.eve.token, room.topic, &query).await);
    assert_eq!(scopes(&ordinary), [room.chat]);
    assert_eq!(selected(&ordinary), room.chat);
    let token = key(
        &room,
        room.eve.id(),
        vec![
            rule("topic", room.topic),
            rule("chat", room.chat),
            rule("chat", parent),
        ],
    )
    .await;
    let opened = ok(open(&room, &token.secret, room.topic, &query).await);
    assert_eq!(
        scopes(&opened),
        [room.chat],
        "key scope never adds missing ordinary parent authority"
    );
    assert_eq!(selected(&opened), room.chat);
    let denied = open(&room, &token.secret, room.project, "limit=10").await;
    assert_eq!(denied.0, 403);
}

#[tokio::test]
async fn a_registry_scope_refusal_is_one_defaults_part_and_never_vetoes_an_allowed_chat() {
    let room = Room::new().await;
    let registry = room
        .world
        .app
        .call(SettingRegistryGet, &[])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    let native_settings = room
        .world
        .app
        .call(NodeSettingList, &[&room.topic])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    let ordinary = ok(open(&room, &room.ada.token, room.topic, "defaults=true&limit=10").await);
    assert_eq!(ordinary["defaults"]["registry"], json!(registry));
    assert_eq!(ordinary["defaults"]["settings"], json!(native_settings));
    let no_registry = key(
        &room,
        room.ada.id(),
        vec![
            rule("topic", room.topic),
            rule("chat", room.chat),
            system_for("SettingRegistryGet"),
        ],
    )
    .await;
    let partial = ok(open(
        &room,
        &no_registry.secret,
        room.topic,
        "defaults=true&limit=10",
    )
    .await);
    assert_eq!(selected(&partial), room.chat);
    assert!(partial["defaults"]["settings"].is_array());
    assert!(partial["defaults"]["registry"].is_null());
    assert_eq!(partial["defaults"]["registry_problem"]["status"], 403);
    let permitted = key(
        &room,
        room.ada.id(),
        vec![
            rule("topic", room.topic),
            rule("chat", room.chat),
            system_for(OP),
        ],
    )
    .await;
    let full = ok(open(
        &room,
        &permitted.secret,
        room.topic,
        "defaults=true&limit=10",
    )
    .await);
    assert_eq!(full["defaults"]["registry"], json!(registry));
    assert!(full["defaults"]["registry_problem"].is_null());
    assert_eq!(selected(&full), room.chat);
}

async fn set_secret(room: &Room) {
    orch_api_core::routes::install_master_key(&orch_db::secrets::MasterKey::new([9; 32]));
    room.world
        .app
        .call(NodeSecretSet, &[&room.root, &"PRIVATE_PIPELINE"])
        .token(&room.ada.token)
        .body(&NodeSecretSetBody {
            value: SECRET.into(),
        })
        .send()
        .await
        .ok();
}
async fn selected_provider(room: &Room) -> Id {
    orch_api_core::routes::install_master_key(&orch_db::secrets::MasterKey::new([9; 32]));
    let home = places::make_org(
        &room.world,
        &room.ada,
        "Private provider place",
        "private-opening-models",
    )
    .await;
    let provider = room
        .world
        .app
        .call(ProviderCreate, &[])
        .token(&room.ada.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: PROVIDER.into(),
            home_id: home.base.id,
            access: None,
            arrangements: None,
            runtime_config: None,
            upstreams: vec![Upstream {
                protocol: "openai-responses".into(),
                base_url: "https://api.example.test/v1".into(),
                options: json!({}),
            }],
            api_key: Some("sk-opening-fixture".into()),
            secrets: None,
            key_machine_id: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .provider
        .base
        .id;
    room.world
        .app
        .call(NodeSettingSet, &[&room.root, &"agent.default_provider_id"])
        .token(&room.ada.token)
        .body(&NodeSettingSetBody {
            value: json!(provider),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    provider
}
fn setting<'a>(body: &'a Value, name: &str) -> &'a Value {
    body["defaults"]["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == name)
        .unwrap()
}

#[tokio::test]
async fn secret_names_never_include_values_and_provenance_needs_its_own_name_source() {
    let room = Room::new().await;
    set_secret(&room).await;
    let ordinary = ok(open(
        &room,
        &room.ada.token,
        room.topic,
        "defaults=true&secret_names=true",
    )
    .await);
    assert_eq!(
        ordinary["defaults"]["secrets"][0]["name"],
        "PRIVATE_PIPELINE"
    );
    assert_eq!(ordinary["defaults"]["secrets"][0]["node"]["name"], "Acme");
    assert!(!ordinary.to_string().contains(SECRET));
    let limited = key(
        &room,
        room.ada.id(),
        vec![rule("topic", room.topic), rule("chat", room.chat)],
    )
    .await;
    let hidden = ok(open(
        &room,
        &limited.secret,
        room.topic,
        "defaults=true&secret_names=true",
    )
    .await);
    assert_eq!(hidden["defaults"]["secrets"][0]["name"], "PRIVATE_PIPELINE");
    assert_eq!(
        hidden["defaults"]["secrets"][0]["node"]["id"],
        json!(room.root)
    );
    assert!(hidden["defaults"]["secrets"][0]["node"]["name"].is_null());
    assert_eq!(hidden["defaults"]["secrets"][0]["node"]["redacted"], true);
    assert!(!hidden.to_string().contains(SECRET));
    let no_names = ok(open(
        &room,
        &limited.secret,
        room.topic,
        "defaults=true&secret_names=false",
    )
    .await);
    assert!(no_names["defaults"]["secrets"].is_null());
    assert!(no_names["defaults"]["secrets_problem"].is_null());
    let permitted = key(
        &room,
        room.ada.id(),
        vec![
            rule("topic", room.topic),
            rule("chat", room.chat),
            rule("org", room.root),
        ],
    )
    .await;
    let shown = ok(open(
        &room,
        &permitted.secret,
        room.topic,
        "defaults=true&secret_names=true",
    )
    .await);
    assert_eq!(
        shown["defaults"]["secrets"],
        ordinary["defaults"]["secrets"]
    );
    assert!(!shown.to_string().contains(SECRET));
}

#[tokio::test]
async fn selected_provider_content_is_an_independent_same_operation_read_not_a_root_privilege() {
    let room = Room::new().await;
    let provider = selected_provider(&room).await;
    let native = room
        .world
        .app
        .call(ProviderGet, &[&provider])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    let ordinary = ok(open(
        &room,
        &room.ada.token,
        room.topic,
        "defaults=true&default_provider_name=true",
    )
    .await);
    assert_eq!(
        ordinary["defaults"]["providers"][0]["provider"],
        json!(native)
    );
    assert_eq!(
        ordinary["defaults"]["providers"][0]["provider"]["name"],
        PROVIDER
    );
    let unrelated = key(
        &room,
        room.ada.id(),
        vec![
            rule("topic", room.topic),
            rule("chat", room.chat),
            rule_for("ProviderGet", "provider", provider),
        ],
    )
    .await;
    let refused = ok(open(
        &room,
        &unrelated.secret,
        room.topic,
        "defaults=true&default_provider_name=true",
    )
    .await);
    assert_eq!(selected(&refused), room.chat);
    assert_eq!(
        setting(&refused, "agent.default_provider_id")["value"],
        json!(provider),
        "the stored setting keeps its supplied identity"
    );
    assert_eq!(refused["defaults"]["providers"][0]["id"], json!(provider));
    assert!(refused["defaults"]["providers"][0]["provider"].is_null());
    assert_eq!(
        refused["defaults"]["providers"][0]["problem"]["status"],
        403
    );
    assert!(!refused.to_string().contains(PROVIDER));
    let permitted = key(
        &room,
        room.ada.id(),
        vec![
            rule("topic", room.topic),
            rule("chat", room.chat),
            rule("provider", provider),
        ],
    )
    .await;
    let shown = ok(open(
        &room,
        &permitted.secret,
        room.topic,
        "defaults=true&default_provider_name=true",
    )
    .await);
    assert_eq!(
        shown["defaults"]["providers"][0]["provider"]["name"],
        PROVIDER
    );
    assert!(shown["defaults"]["providers"][0]["problem"].is_null());
    assert!(
        !shown.to_string().contains("Private provider place"),
        "a Provider read does not add a separate place-name grant"
    );
    let viewer = key(
        &room,
        room.bea.id(),
        vec![
            rule("topic", room.topic),
            rule("chat", room.chat),
            rule("provider", provider),
        ],
    )
    .await;
    let denied = ok(open(
        &room,
        &viewer.secret,
        room.topic,
        "defaults=true&default_provider_name=true",
    )
    .await);
    assert_eq!(selected(&denied), room.chat);
    assert!(denied["defaults"]["providers"][0]["provider"].is_null());
    assert_eq!(denied["defaults"]["providers"][0]["problem"]["status"], 403);
    assert!(!denied.to_string().contains(PROVIDER));
}

fn label<'a>(body: &'a Value, target: &ChatMentionTarget) -> &'a str {
    body["mention_names"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["target"] == json!(target))
        .unwrap()["name"]
        .as_str()
        .unwrap()
}

#[tokio::test]
async fn conversation_and_draft_use_the_same_operation_name_bounds_without_scrubbing_stored_text() {
    let room = Room::new().await;
    let private = "Protected opening crew";
    let group = room
        .world
        .app
        .call(OrgGroupCreate, &[&room.root])
        .token(&room.ada.token)
        .body(&OrgGroupCreateBody {
            name: private.into(),
            description: None,
        })
        .send()
        .await
        .ok();
    room.world
        .app
        .call(GroupMemberAdd, &[&group.id, &room.cai.id()])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    let group_target = ChatMentionTarget::Group { id: group.id };
    let role_target = ChatMentionTarget::Role {
        node_id: room.topic,
        role: Role::Developer,
    };
    let literal = format!("Previously supplied literal: {private}. ");
    let parts = vec![
        text(&literal),
        ChatPart::Mention {
            target: group_target.clone(),
        },
        text(" / "),
        ChatPart::Mention {
            target: role_target.clone(),
        },
    ];
    let said = room
        .send(&room.ada, room.chat, parts.clone(), &new_id().to_string())
        .await
        .ok()
        .message;
    room.world
        .app
        .call(ChatDraftSet, &[&room.chat])
        .token(&room.ada.token)
        .body(&ChatDraftSetBody {
            parts: parts.clone(),
            reply_to: None,
        })
        .send()
        .await
        .ok();
    let ordinary = ok(open(&room, &room.ada.token, room.topic, "limit=20").await);
    assert!(label(message(&ordinary, said.id), &group_target).contains(private));
    assert!(label(&ordinary["opening"]["draft"], &group_target).contains(private));
    let draft_role_name = "Developers of Login";
    assert_eq!(
        label(&ordinary["opening"]["draft"], &role_target),
        draft_role_name,
        "the native composer label uses the leaf, unlike a message's qualified role label"
    );
    let stored =
        orch_db::queries::chats::message(&mut *room.world.conn().await, room.chat, said.id)
            .await
            .unwrap()
            .unwrap()
            .content;
    let bounded = key(
        &room,
        room.ada.id(),
        vec![
            rule("topic", room.topic),
            rule("chat", room.chat),
            rule_for("ChatOpeningGet", "group", group.id),
        ],
    )
    .await;
    let hidden = ok(open(&room, &bounded.secret, room.topic, "limit=20").await);
    assert_eq!(label(message(&hidden, said.id), &group_target), "a group");
    assert_eq!(label(&hidden["opening"]["draft"], &group_target), "a group");
    // Messages render/coalesce typed mentions as text; draft/source parts retain
    // their typed editing form. These independent labels are the live lookups.
    let role_fallback = "Developers of a private place > a private place > Login";
    assert_eq!(
        label(message(&hidden, said.id), &role_target),
        role_fallback
    );
    assert_eq!(
        label(&hidden["opening"]["draft"], &role_target),
        draft_role_name,
        "the same Topic Read permits the native compact draft label"
    );
    assert_eq!(
        message(&hidden, said.id)["parts"],
        json!([text(&format!("{literal}@a group / @{role_fallback}"))])
    );
    assert_eq!(hidden["opening"]["draft"]["parts"], json!(parts));
    let permitted = key(
        &room,
        room.ada.id(),
        vec![
            rule("topic", room.topic),
            rule("chat", room.chat),
            rule("group", group.id),
            rule("project", room.project),
            rule("org", room.root),
        ],
    )
    .await;
    let shown = ok(open(&room, &permitted.secret, room.topic, "limit=20").await);
    assert_eq!(
        message(&shown, said.id)["mention_names"],
        message(&ordinary, said.id)["mention_names"]
    );
    assert_eq!(
        shown["opening"]["draft"]["mention_names"],
        ordinary["opening"]["draft"]["mention_names"]
    );
    assert_eq!(
        message(&shown, said.id)["parts"],
        message(&ordinary, said.id)["parts"],
        "full same-operation labels preserve the native rendered message"
    );
    let row = orch_db::queries::chats::message(&mut *room.world.conn().await, room.chat, said.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.content, stored,
        "name projection never rewrites stored material"
    );
}

#[tokio::test]
async fn a_fresh_opening_rechecks_current_key_narrowing_and_revocation() {
    let room = Room::new().await;
    let ordinary = ok(open(&room, &room.ada.token, room.topic, "limit=10").await);
    assert_eq!(selected(&ordinary), room.chat);
    let token = key(
        &room,
        room.ada.id(),
        vec![rule("topic", room.topic), rule("chat", room.chat)],
    )
    .await;
    let first = ok(open(&room, &token.secret, room.topic, "limit=10").await);
    assert_eq!(selected(&first), room.chat);
    let narrowed = json!({"rules":[rule("topic",room.topic)]});
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(token.id)
        .bind(narrowed)
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    let current = ok(open(&room, &token.secret, room.topic, "limit=10").await);
    assert!(scopes(&current).is_empty());
    assert!(
        current["opening"].is_null(),
        "previous prepared data cannot donate an old Chat grant"
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(token.id)
        .bind(room.world.now())
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    let revoked = open(&room, &token.secret, room.topic, "limit=10").await;
    assert_eq!(revoked.0, 401);
    assert!(revoked.1.get("opening").is_none());
}

#[tokio::test]
async fn a_defaults_readme_needs_its_actual_document_read_without_vetoing_the_node() {
    use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
    use orch_api_contract::nodes::{NodeGet, NodeUpdate};
    let room = Room::new().await;
    let made = room
        .world
        .app
        .call(NodeDocumentCreate, &[&room.topic])
        .token(&room.ada.token)
        .body(&NodeDocumentCreateBody {
            title: "A separately scoped live README".into(),
            blocks: None,
            markdown: None,
            access: None,
        })
        .send_bytes()
        .await;
    assert_eq!(
        made.status, 201,
        "ordinary document creation is the fixture premise"
    );
    let made: Value = serde_json::from_slice(&made.raw).unwrap();
    let document: Id = serde_json::from_value(made["id"].clone()).unwrap();
    let before = room
        .world
        .app
        .call(NodeGet, &[&room.topic])
        .token(&room.ada.token)
        .send_bytes()
        .await;
    assert_eq!(before.status, 200);
    let before: Value = serde_json::from_slice(&before.raw).unwrap();
    let updated = room
        .world
        .app
        .call(NodeUpdate, &[&room.topic])
        .token(&room.ada.token)
        .raw_body(&json!({"readme_document_id":document,"expected_revision":before["revision"]}))
        .send_bytes()
        .await;
    assert_eq!(
        updated.status, 200,
        "the node really names its own readable document"
    );
    let ordinary = ok(open(&room, &room.ada.token, room.topic, "defaults=true").await);
    assert_eq!(
        ordinary["defaults"]["node"]["readme"]["id"],
        json!(document)
    );
    assert_eq!(
        ordinary["defaults"]["node"]["readme"]["title"],
        "A separately scoped live README"
    );
    let unrelated = key(
        &room,
        room.ada.id(),
        vec![
            rule("topic", room.topic),
            rule("chat", room.chat),
            rule_for("DocumentGet.read", "document", document),
        ],
    )
    .await;
    let hidden = ok(open(&room, &unrelated.secret, room.topic, "defaults=true").await);
    assert_eq!(selected(&hidden), room.chat);
    assert!(hidden["defaults"]["node"]["readme"].is_null());
    assert!(
        !hidden
            .to_string()
            .contains("A separately scoped live README")
    );
    let permitted = key(
        &room,
        room.ada.id(),
        vec![
            rule("topic", room.topic),
            rule("chat", room.chat),
            rule("document", document),
        ],
    )
    .await;
    let shown = ok(open(&room, &permitted.secret, room.topic, "defaults=true").await);
    assert_eq!(shown["defaults"]["node"]["readme"]["id"], json!(document));
    assert_eq!(
        shown["defaults"]["node"]["readme"]["title"],
        "A separately scoped live README"
    );
    let unchanged = ok(open(&room, &room.ada.token, room.topic, "defaults=true").await);
    assert_eq!(
        unchanged["defaults"]["node"]["readme"],
        ordinary["defaults"]["node"]["readme"]
    );
}

#[tokio::test]
async fn a_defaults_environment_ref_needs_its_own_read_while_the_stored_default_id_stays() {
    let scene = rig::Scene::new().await;
    let world = &scene.rig.world;
    let topic = scene.tree.topic.base.id;
    let chat = scene.tree.topic.base.chat_id;
    let environment = scene.on.environment;
    // This established fixture inserts a current owned environment directly; no
    // provision or model command is submitted by this read-only control.
    world
        .set_at(
            orch_settings::Target::Node(scene.tree.project.base.id),
            "env.default_id",
            json!(environment),
        )
        .await;
    let ordinary = ok(open_in(world, &scene.ada.token, topic, "defaults=true").await);
    assert_eq!(
        ordinary["defaults"]["node"]["default_environment_id"],
        json!(environment)
    );
    assert!(
        ordinary["defaults"]["node"]["environments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == json!(environment)),
        "normal native summary must contain the actual environment"
    );
    let unrelated = key_in(
        world,
        scene.ada.id(),
        vec![
            rule("topic", topic),
            rule("chat", chat),
            rule_for("EnvironmentGet.read", "environment", environment),
        ],
    )
    .await;
    let hidden = ok(open_in(world, &unrelated.secret, topic, "defaults=true").await);
    assert_eq!(selected(&hidden), chat);
    assert_eq!(
        hidden["defaults"]["node"]["default_environment_id"],
        json!(environment)
    );
    assert!(
        hidden["defaults"]["node"]["environments"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        setting(&hidden, "env.default_id")["value"],
        json!(environment)
    );
    let permitted = key_in(
        world,
        scene.ada.id(),
        vec![
            rule("topic", topic),
            rule("chat", chat),
            rule("environment", environment),
        ],
    )
    .await;
    let shown = ok(open_in(world, &permitted.secret, topic, "defaults=true").await);
    assert_eq!(
        shown["defaults"]["node"]["default_environment_id"],
        json!(environment)
    );
    assert_eq!(
        shown["defaults"]["node"]["environments"],
        ordinary["defaults"]["node"]["environments"]
    );
    assert_eq!(
        setting(&shown, "env.default_id")["value"],
        json!(environment)
    );
}

#[tokio::test]
async fn an_ungranted_full_provider_group_declines_only_that_defaults_part() {
    let room = Room::new().await;
    let provider = selected_provider(&room).await;
    let home =
        orch_db::queries::resources::routes::providers(&mut *room.world.conn().await, &[provider])
            .await
            .unwrap()
            .pop()
            .unwrap()
            .home_id;
    // Provider access explains the ways that actually give this requester a
    // permission, not every page row. Bea must really receive View through this Group.
    places::join(&room.world, home, home, &room.bea, "viewer", &room.ada).await;
    let group = room
        .world
        .app
        .call(OrgGroupCreate, &[&home])
        .token(&room.ada.token)
        .body(&OrgGroupCreateBody {
            name: "Protected provider ACL crew".into(),
            description: None,
        })
        .send()
        .await
        .ok();
    room.world
        .app
        .call(GroupMemberAdd, &[&group.id, &room.bea.id()])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    orch_db::queries::access::testing::add_entry(
        &mut *room.world.conn().await,
        &orch_db::queries::access::PageKey::new("provider", provider, "live"),
        "any",
        "provider.view",
        ("group", Some(group.id), None),
        None,
        room.world.now(),
    )
    .await
    .unwrap();
    let native = room
        .world
        .app
        .call(ProviderGet, &[&provider])
        .token(&room.bea.token)
        .send_bytes()
        .await;
    assert_eq!(native.status, 200);
    let native_body: Value = serde_json::from_slice(&native.raw).unwrap();
    let holder = native_body["access"]["through"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|through| {
            (through["kind"] == "entry" && through["holder"]["kind"] == "group")
                .then_some(&through["holder"]["group"])
        })
        .expect("the actual requester gets ProviderView through this Group");
    assert_eq!(holder["id"], json!(group.id));
    assert_eq!(holder["name"], "Protected provider ACL crew");
    assert_eq!(holder["member_count"], 1);
    assert!(
        String::from_utf8_lossy(&native.raw).contains("Protected provider ACL crew"),
        "the actual native Provider page must really contain this full Group source"
    );
    let ordinary = ok(open(
        &room,
        &room.bea.token,
        room.topic,
        "defaults=true&default_provider_name=true",
    )
    .await);
    assert!(ordinary["defaults"]["providers"][0]["provider"].is_object());
    let limited = key(
        &room,
        room.bea.id(),
        vec![
            rule("topic", room.topic),
            rule("chat", room.chat),
            rule("provider", provider),
        ],
    )
    .await;
    let refused = ok(open(
        &room,
        &limited.secret,
        room.topic,
        "defaults=true&default_provider_name=true",
    )
    .await);
    assert_eq!(selected(&refused), room.chat);
    assert!(refused["defaults"]["settings"].is_array());
    assert!(refused["defaults"]["providers"][0]["provider"].is_null());
    assert_eq!(
        refused["defaults"]["providers"][0]["problem"]["status"],
        403
    );
    assert!(!refused.to_string().contains("Protected provider ACL crew"));
    let permitted = key(
        &room,
        room.bea.id(),
        vec![
            rule("topic", room.topic),
            rule("chat", room.chat),
            rule("provider", provider),
            rule("group", group.id),
        ],
    )
    .await;
    let shown = ok(open(
        &room,
        &permitted.secret,
        room.topic,
        "defaults=true&default_provider_name=true",
    )
    .await);
    assert_eq!(selected(&shown), room.chat);
    assert!(shown["defaults"]["providers"][0]["provider"].is_object());
    assert!(shown["defaults"]["providers"][0]["problem"].is_null());
    assert!(shown.to_string().contains("Protected provider ACL crew"));
}

#[tokio::test]
async fn both_requested_org_provider_names_share_the_actual_selected_provider_read() {
    let room = Room::new().await;
    let provider = selected_provider(&room).await;
    // Keep semantic execution explicitly disabled. This control reads the two
    // existing editor references; it submits no embedding/model request.
    for (name, value) in [
        ("search.semantic.enabled", json!(false)),
        (
            "search.semantic.embeddings",
            json!({"provider_id":provider,"model_name":"metadata-fixture"}),
        ),
    ] {
        room.world
            .app
            .call(NodeSettingSet, &[&room.root, &name])
            .token(&room.ada.token)
            .body(&NodeSettingSetBody {
                value,
                locked: None,
                expected_revision: None,
            })
            .send()
            .await
            .ok();
    }
    let query = "defaults=true&default_provider_name=true&embeddings_provider_name=true";
    let ordinary = ok(open(&room, &room.ada.token, room.root, query).await);
    let readers = ordinary["defaults"]["providers"].as_array().unwrap();
    assert_eq!(
        readers.len(),
        2,
        "both mounted native controls select the same real provider"
    );
    assert_eq!(readers[0]["id"], json!(provider));
    assert_eq!(readers[1]["id"], json!(provider));
    assert_eq!(readers[0]["provider"], readers[1]["provider"]);
    let org_chat = chat_world::chat_of(&room.world, &room.ada, room.root).await;
    let limited = key(
        &room,
        room.ada.id(),
        vec![rule("org", room.root), rule("chat", org_chat)],
    )
    .await;
    let partial = ok(open(&room, &limited.secret, room.root, query).await);
    assert_eq!(selected(&partial), org_chat);
    for reader in partial["defaults"]["providers"].as_array().unwrap() {
        assert_eq!(reader["id"], json!(provider));
        assert!(reader["provider"].is_null());
        assert_eq!(reader["problem"]["status"], 403);
    }
    let permitted = key(
        &room,
        room.ada.id(),
        vec![
            rule("org", room.root),
            rule("chat", org_chat),
            rule("provider", provider),
        ],
    )
    .await;
    let shown = ok(open(&room, &permitted.secret, room.root, query).await);
    let readers = shown["defaults"]["providers"].as_array().unwrap();
    assert_eq!(readers.len(), 2);
    assert_eq!(readers[0]["setting"], "agent.default_provider_id");
    assert_eq!(readers[1]["setting"], "search.semantic.embeddings");
    assert_eq!(readers[0]["provider"]["name"], PROVIDER);
    assert_eq!(readers[0]["provider"], readers[1]["provider"]);
    assert!(readers.iter().all(|reader| reader["problem"].is_null()));
}
