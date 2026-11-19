//! Mention discovery and real rollback previews keep one actual operation and current source cuts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod chat_world;
mod places;
mod reader_diagnostics;
mod rig;
mod support;

use chat_world::{Room, text};
use orch_api_contract::access::Role;
use orch_api_contract::chats::{
    ChatMentionList, ChatMentionListQuery, ChatMentionPreview, ChatMentionPreviewBody,
    ChatMentionTarget, ChatPart, ChatStateSet, ChatStateSetBody,
};
use orch_api_contract::directory::{GroupMemberAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
use orch_types::{Hash, Id, new_id};
use serde_json::{Value, json};

fn rule(op: &str, kind: &str, id: Id) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":op,"actions":["read"]}]})
}
struct Key {
    id: Id,
    secret: String,
}
async fn key(world: &support::World, user: Id, rules: Vec<Value>) -> Key {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let stored = serde_json::to_value(scope).unwrap();
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Mention operation scope control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            // The fixed fixture clock and finite HTTP sequence fit this existing short test-key life.
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&stored),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn legacy(world: &support::World, user: Id, scopes: &[&str]) -> String {
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let secret = new_token(API_TOKEN_PREFIX);
    let scopes: Vec<String> = scopes.iter().map(|scope| (*scope).to_owned()).collect();
    create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Native mention legacy control",
            token_hash: &token_hash(&secret),
            scopes: &scopes,
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        None,
    )
    .await
    .unwrap();
    secret
}
fn result(reply: orch_api_core::testing::RawReply) -> (u16, Value) {
    (
        reply.status.as_u16(),
        serde_json::from_slice(&reply.raw).unwrap(),
    )
}
fn ok(reply: (u16, Value)) -> Value {
    assert_eq!(reply.0, 200, "{}", reply.1);
    reply.1
}
async fn list(
    world: &support::World,
    token: &str,
    chat: Id,
    q: Option<&str>,
    limit: u32,
    cursor: Option<String>,
) -> (u16, Value) {
    result(
        world
            .app
            .call(ChatMentionList, &[&chat])
            .token(token)
            .query(&ChatMentionListQuery {
                q: q.map(str::to_owned),
                limit: Some(limit),
                cursor,
            })
            .send_bytes()
            .await,
    )
}
async fn preview(
    world: &support::World,
    token: &str,
    chat: Id,
    parts: Vec<ChatPart>,
    reply: Option<Id>,
) -> (u16, Value) {
    result(
        world
            .app
            .call(ChatMentionPreview, &[&chat])
            .token(token)
            .body(&ChatMentionPreviewBody {
                parts,
                reply_to: reply,
            })
            .send_bytes()
            .await,
    )
}
fn human(items: &Value) -> Vec<Value> {
    items
        .as_array()
        .unwrap()
        .iter()
        .filter(|choice| choice["target"]["kind"] == "user")
        .cloned()
        .collect()
}
fn target(kind: &str, id: Id) -> Value {
    json!({"kind":kind,"id":id})
}
fn named(target: ChatMentionTarget) -> ChatPart {
    ChatPart::Mention { target }
}
async fn group(room: &Room) -> Id {
    let group = room
        .world
        .app
        .call(OrgGroupCreate, &[&room.root])
        .token(&room.ada.token)
        .body(&OrgGroupCreateBody {
            name: "Private mention witness crew".into(),
            description: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response");
    for who in [&room.bea, &room.cai] {
        room.world
            .app
            .call(GroupMemberAdd, &[&group.id, &who.id()])
            .token(&room.ada.token)
            .send()
            .await
            .ok();
    }
    group.id
}
fn roots(room: &Room, op: &str) -> Vec<Value> {
    vec![
        rule(op, "chat", room.chat),
        rule(op, "org", room.root),
        rule(op, "project", room.project),
        rule(op, "topic", room.topic),
    ]
}
async fn counts(world: &support::World) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM chat_messages),(SELECT count(*) FROM thread_inputs),(SELECT count(*) FROM jobs),(SELECT count(*) FROM commands)")
        .fetch_one(&world.db.pool).await.unwrap()
}

#[tokio::test]
async fn current_public_human_choices_need_only_this_chats_read_not_account_or_other_operation_grants()
 {
    let room = Room::new().await;
    let native = ok(list(&room.world, &room.bea.token, room.chat, None, 100, None).await);
    let users = human(&native["items"]);
    assert!(users.len() > 1);
    assert!(
        users
            .iter()
            .all(|choice| choice["user"]["id"] == choice["target"]["id"])
    );
    let reader = key(
        &room.world,
        room.bea.id(),
        vec![rule("ChatMentionList", "chat", room.chat)],
    )
    .await;
    let limited = ok(list(&room.world, &reader.secret, room.chat, None, 100, None).await);
    assert_eq!(human(&limited["items"]), users);
    assert!(
        limited["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|choice| choice["target"]["kind"] == "user")
    );
    let unrelated = key(
        &room.world,
        room.bea.id(),
        vec![rule("ChatDraftGet", "chat", room.chat)],
    )
    .await;
    assert_eq!(
        list(&room.world, &unrelated.secret, room.chat, None, 100, None)
            .await
            .0,
        403
    );
    assert_eq!(
        preview(
            &room.world,
            &reader.secret,
            room.chat,
            vec![text("Hi @cai")],
            None
        )
        .await
        .0,
        403
    );
}

#[tokio::test]
async fn protected_choices_and_private_search_are_cut_before_pagination_under_the_same_operation() {
    let room = Room::new().await;
    let group = group(&room).await;
    let native = ok(list(
        &room.world,
        &room.ada.token,
        room.chat,
        Some("Private mention witness"),
        100,
        None,
    )
    .await);
    assert!(
        native["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|choice| choice["target"] == target("group", group))
    );
    let roles = ok(list(
        &room.world,
        &room.ada.token,
        room.chat,
        Some("Login"),
        100,
        None,
    )
    .await);
    assert!(
        roles["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|choice| choice["target"]["kind"] == "role")
    );
    let mut all = roots(&room, "ChatMentionList");
    all.push(rule("ChatMentionList", "group", group));
    let full = key(&room.world, room.ada.id(), all).await;
    assert_eq!(
        ok(list(
            &room.world,
            &full.secret,
            room.chat,
            Some("Private mention witness"),
            100,
            None
        )
        .await),
        native
    );
    let root = key(
        &room.world,
        room.ada.id(),
        vec![
            rule("ChatMentionList", "chat", room.chat),
            rule("ChatDraftGet", "group", group),
        ],
    )
    .await;
    for q in ["Private mention witness", "Login"] {
        let hidden = ok(list(&room.world, &root.secret, room.chat, Some(q), 1, None).await);
        assert_eq!(hidden["items"], json!([]));
        assert!(hidden["next_cursor"].is_null());
    }
    let ordinary = ok(list(&room.world, &room.ada.token, room.chat, None, 100, None).await);
    assert_eq!(
        ok(list(&room.world, &full.secret, room.chat, None, 100, None).await),
        ordinary
    );
    let wanted = human(&ordinary["items"]);
    let mut seen = Vec::new();
    let mut cursor = None;
    for _ in 0..wanted.len() + 1 {
        let page = ok(list(&room.world, &root.secret, room.chat, None, 1, cursor).await);
        seen.extend(page["items"].as_array().unwrap().iter().cloned());
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert!(cursor.is_none());
    assert_eq!(seen, wanted);
}

#[tokio::test]
async fn all_lists_only_current_readable_followers_and_keeps_its_native_aggregate() {
    let room = Room::new().await;
    for who in [&room.bea, &room.cai] {
        room.world
            .app
            .call(ChatStateSet, &[&room.chat])
            .token(&who.token)
            .body(&ChatStateSetBody {
                level: Some(Some(orch_api_contract::chats::ChatLevel::All)),
                hidden: None,
            })
            .send()
            .await
            .ok()
            .into_full()
            .unwrap();
    }
    let native = ok(list(
        &room.world,
        &room.ada.token,
        room.chat,
        Some("@all"),
        100,
        None,
    )
    .await);
    assert_eq!(native["followers"], 2);
    assert_eq!(native["items"].as_array().unwrap().len(), 2);
    let reader = key(
        &room.world,
        room.ada.id(),
        vec![rule("ChatMentionList", "chat", room.chat)],
    )
    .await;
    assert_eq!(
        ok(list(
            &room.world,
            &reader.secret,
            room.chat,
            Some("@all"),
            100,
            None
        )
        .await),
        native
    );
}

#[tokio::test]
async fn human_preview_counts_the_current_typed_union_without_publishing_or_borrowing_a_source_read()
 {
    let room = Room::new().await;
    let group = group(&room).await;
    let parts = vec![
        named(ChatMentionTarget::Group { id: group }),
        named(ChatMentionTarget::Role {
            node_id: room.topic,
            role: Role::Developer,
        }),
    ];
    let before = counts(&room.world).await;
    let native = ok(preview(&room.world, &room.ada.token, room.chat, parts.clone(), None).await);
    assert!(native["count"].as_i64().unwrap() > 0);
    assert!(native.get("agent_asks").is_none());
    assert_eq!(counts(&room.world).await, before);
    let mut rules = roots(&room, "ChatMentionPreview");
    rules.push(rule("ChatMentionPreview", "group", group));
    let full = key(&room.world, room.ada.id(), rules).await;
    assert_eq!(
        ok(preview(&room.world, &full.secret, room.chat, parts.clone(), None).await),
        native
    );
    let mut rules = roots(&room, "ChatMentionPreview");
    rules.push(rule("ChatMentionList", "group", group));
    let unrelated = key(&room.world, room.ada.id(), rules).await;
    assert_eq!(
        preview(&room.world, &unrelated.secret, room.chat, parts, None)
            .await
            .0,
        403
    );
    let plain = key(
        &room.world,
        room.ada.id(),
        vec![rule("ChatMentionPreview", "chat", room.chat)],
    )
    .await;
    let user = vec![named(ChatMentionTarget::User { id: room.bea.id() })];
    assert_eq!(
        ok(preview(&room.world, &plain.secret, room.chat, user.clone(), None).await),
        ok(preview(&room.world, &room.ada.token, room.chat, user, None).await)
    );
    assert_eq!(counts(&room.world).await, before);
}

#[tokio::test]
async fn human_only_preview_does_not_consume_an_authored_attachment_or_item_reference() {
    let room = Room::new().await;
    let parts = vec![
        text("Hi @cai"),
        ChatPart::Attachment {
            blob: Hash::of(b"an authored reference, never stored or opened"),
            name: "authored-reference.txt".into(),
            size: 3,
            media_type: "text/plain".into(),
        },
        ChatPart::ItemRef {
            thread_id: new_id(),
            item_id: new_id(),
        },
    ];
    let before = counts(&room.world).await;
    let native = ok(preview(&room.world, &room.ada.token, room.chat, parts.clone(), None).await);
    assert_eq!(native["count"], 1);
    let reader = key(
        &room.world,
        room.ada.id(),
        vec![rule("ChatMentionPreview", "chat", room.chat)],
    )
    .await;
    assert_eq!(
        ok(preview(&room.world, &reader.secret, room.chat, parts, None).await),
        native
    );
    assert_eq!(counts(&room.world).await, before);
}

#[tokio::test]
async fn legacy_send_ceilings_and_disabled_sending_keep_their_original_list_and_preview_gate() {
    let room = Room::new().await;
    let writable = legacy(&room.world, room.bea.id(), &["chat.view", "chat.interact"]).await;
    let parts = vec![text("Hi @cai")];
    assert!(
        !ok(list(&room.world, &writable, room.chat, Some("cai"), 100, None).await)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        ok(preview(&room.world, &writable, room.chat, parts.clone(), None).await)["count"],
        1
    );
    let read_only = legacy(&room.world, room.bea.id(), &["chat.view"]).await;
    assert_eq!(
        list(&room.world, &read_only, room.chat, None, 100, None)
            .await
            .0,
        403
    );
    assert_eq!(
        preview(&room.world, &read_only, room.chat, parts.clone(), None)
            .await
            .0,
        403
    );
    room.world
        .app
        .call(NodeSettingSet, &[&room.topic, &"chat.enabled"])
        .token(&room.ada.token)
        .body(&NodeSettingSetBody {
            value: json!(false),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    for token in [&room.bea.token, &writable] {
        assert_eq!(
            list(&room.world, token, room.chat, None, 100, None).await.0,
            403
        );
        assert_eq!(
            preview(&room.world, token, room.chat, parts.clone(), None)
                .await
                .0,
            403
        );
    }
}

#[tokio::test]
async fn an_agent_preview_returns_real_trial_input_with_read_scopes_and_never_dispatches_or_keeps_rows()
 {
    let scene = rig::Scene::new().await;
    let thread = scene
        .thread_by(&scene.ada, "Private preview receiver")
        .await;
    let chat = scene.tree.topic.base.chat_id;
    let original = scene
        .rig
        .world
        .app
        .call(orch_api_contract::chats::ChatMessageCreate, &[&chat])
        .token(&scene.eve.token)
        .body(&orch_api_contract::chats::ChatMessageCreateBody {
            parts: vec![text("An ordinary prior reply")],
            reply_to: None,
            agent_review: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .message;
    let parts = vec![
        text("Please inspect "),
        named(ChatMentionTarget::Agent {
            thread_id: thread.base.id,
        }),
    ];
    let before = counts(&scene.rig.world).await;
    let commands = scene.rig.commands.issued_total();
    let native = ok(preview(
        &scene.rig.world,
        &scene.ada.token,
        chat,
        parts.clone(),
        Some(original.id),
    )
    .await);
    let native_ask = &native["agent_asks"][0];
    assert_eq!(native_ask["thread"]["id"], json!(thread.base.id));
    assert_eq!(native_ask["model"], rig::MODEL);
    assert_eq!(native_ask["input"]["author"]["id"], json!(scene.ada.id()));
    assert_eq!(
        native_ask["input"]["content"][0]["text"],
        "An ordinary prior reply"
    );
    assert_eq!(counts(&scene.rig.world).await, before);
    assert_eq!(scene.rig.commands.issued_total(), commands);
    let rules = vec![
        rule("ChatMentionPreview", "chat", chat),
        rule("ChatMentionPreview", "thread", thread.base.id),
        rule("ChatMentionPreview", "org", scene.tree.org.base.id),
        rule("ChatMentionPreview", "project", scene.tree.project.base.id),
        rule("ChatMentionPreview", "topic", scene.tree.topic.base.id),
    ];
    let reader = key(&scene.rig.world, scene.ada.id(), rules).await;
    let shown = ok(preview(
        &scene.rig.world,
        &reader.secret,
        chat,
        parts.clone(),
        Some(original.id),
    )
    .await);
    let ask = &shown["agent_asks"][0];
    assert_eq!(ask["thread"], native_ask["thread"]);
    assert_eq!(ask["agent"], native_ask["agent"]);
    assert_eq!(ask["model"], native_ask["model"]);
    assert_eq!(ask["effort"], native_ask["effort"]);
    assert_eq!(ask["input"]["author"], native_ask["input"]["author"]);
    assert_eq!(ask["input"]["thread_id"], json!(thread.base.id));
    assert_eq!(ask["input"]["delivery"], "queue");
    assert_eq!(
        ask["input"]["content"][0]["text"],
        "An ordinary prior reply"
    );
    assert_eq!(
        ask["input"]["content"][1]["text"],
        "Please inspect @Private preview receiver"
    );
    assert!(shown["agent_review"].as_str().is_some());
    let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE id=$1")
        .bind(ask["input"]["id"].as_str().unwrap().parse::<Id>().unwrap())
        .fetch_one(&scene.rig.world.db.pool)
        .await
        .unwrap();
    assert_eq!(retained, 0);
    assert_eq!(counts(&scene.rig.world).await, before);
    assert_eq!(scene.rig.commands.issued_total(), commands);
    let missing = key(
        &scene.rig.world,
        scene.ada.id(),
        vec![
            rule("ChatMentionPreview", "chat", chat),
            rule("ChatMentionList", "thread", thread.base.id),
        ],
    )
    .await;
    let denied = preview(
        &scene.rig.world,
        &missing.secret,
        chat,
        parts.clone(),
        Some(original.id),
    )
    .await;
    assert_eq!(denied.0, 403);
    assert!(!denied.1.to_string().contains("Private preview receiver"));
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(reader.id)
        .bind(json!({"rules":[rule("ChatMentionList","chat",chat)]}))
        .execute(&scene.rig.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        preview(
            &scene.rig.world,
            &reader.secret,
            chat,
            parts,
            Some(original.id)
        )
        .await
        .0,
        403
    );
    assert_eq!(counts(&scene.rig.world).await, before);
    assert_eq!(scene.rig.commands.issued_total(), commands);
}

#[tokio::test]
async fn agent_choices_need_current_thread_read_but_keep_the_native_disabled_interaction_choice() {
    let scene = rig::Scene::new().await;
    let thread = scene
        .thread_by(&scene.ada, "Private disabled mention receiver")
        .await;
    let chat = scene.tree.topic.base.chat_id;
    let native = ok(list(
        &scene.rig.world,
        &scene.cai.token,
        chat,
        Some("Private disabled mention"),
        100,
        None,
    )
    .await);
    let chosen = native["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|choice| choice["target"] == json!({"kind":"agent","thread_id":thread.base.id}))
        .unwrap();
    assert_eq!(chosen["available"], false);
    assert_eq!(chosen["reason"], "thread.interact");
    assert!(chosen["started_at"].is_string());
    let full = key(
        &scene.rig.world,
        scene.cai.id(),
        vec![
            rule("ChatMentionList", "chat", chat),
            rule("ChatMentionList", "thread", thread.base.id),
        ],
    )
    .await;
    assert_eq!(
        ok(list(
            &scene.rig.world,
            &full.secret,
            chat,
            Some("Private disabled mention"),
            100,
            None
        )
        .await),
        native
    );
    let root = key(
        &scene.rig.world,
        scene.cai.id(),
        vec![
            rule("ChatMentionList", "chat", chat),
            rule("ChatMentionPreview", "thread", thread.base.id),
        ],
    )
    .await;
    let hidden = ok(list(
        &scene.rig.world,
        &root.secret,
        chat,
        Some("Private disabled mention"),
        1,
        None,
    )
    .await);
    assert_eq!(hidden["items"], json!([]));
    assert!(hidden["next_cursor"].is_null());
}

async fn trial_effects(world: &support::World) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM audit_log),(SELECT count(*) FROM outbox),(SELECT count(*) FROM notifications)")
        .fetch_one(&world.db.pool).await.unwrap()
}

async fn owns_blob(world: &support::World, user: Id, hash: Hash) -> bool {
    orch_db::queries::uploads::owns(&mut *world.conn().await, user, &hash)
        .await
        .unwrap()
}

#[tokio::test]
async fn an_external_saved_attachment_is_not_reselected_from_the_previews_own_trial_custody() {
    use orch_api_contract::chats::{
        ChatAttachmentCreate, ChatMessageCreate, ChatMessageCreateBody, OrgChatCreate,
        OrgChatCreateBody,
    };
    let scene = rig::Scene::new().await;
    let receiver = scene.thread_by(&scene.ada, "External byte review").await;
    let chat = scene.tree.topic.base.chat_id;
    let source = scene
        .rig
        .world
        .app
        .call(OrgChatCreate, &[&scene.tree.org.base.id])
        .token(&scene.eve.token)
        .body(&OrgChatCreateBody {
            user_ids: vec![scene.ada.id()],
            name: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("the ordinary chat fixture is full")
        .base
        .id;
    let uploaded = scene
        .rig
        .world
        .app
        .call(ChatAttachmentCreate, &[&source])
        .token(&scene.eve.token)
        .header(
            orch_api_contract::threads::FILENAME_HEADER,
            "external-preview.txt",
        )
        .bytes(b"bytes read from a separately authorized saved chat")
        .send()
        .await
        .ok();
    let attachment = ChatPart::Attachment {
        blob: uploaded.blob,
        name: uploaded.name,
        size: uploaded.size,
        media_type: uploaded.media_type,
    };
    scene
        .rig
        .world
        .app
        .call(ChatMessageCreate, &[&source])
        .token(&scene.eve.token)
        .body(&ChatMessageCreateBody {
            parts: vec![attachment.clone()],
            reply_to: None,
            agent_review: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert!(
        !owns_blob(&scene.rig.world, scene.ada.id(), uploaded.blob).await,
        "the reader has a real saved source, not upload custody"
    );
    let parts = vec![
        text("Please read "),
        attachment,
        named(ChatMentionTarget::Agent {
            thread_id: receiver.base.id,
        }),
    ];
    let before = counts(&scene.rig.world).await;
    let effects = trial_effects(&scene.rig.world).await;
    let commands = scene.rig.commands.issued_total();
    let pins: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(uploaded.blob.as_bytes().as_slice())
        .fetch_one(&scene.rig.world.db.pool)
        .await
        .unwrap();
    let native = ok(preview(
        &scene.rig.world,
        &scene.ada.token,
        chat,
        parts.clone(),
        None,
    )
    .await);
    assert_eq!(native["agent_asks"].as_array().unwrap().len(), 1);
    assert!(
        native["agent_asks"][0]["input"]["content"][0]["attachments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["blob"] == json!(uploaded.blob))
    );
    assert!(!owns_blob(&scene.rig.world, scene.ada.id(), uploaded.blob).await);
    assert_eq!(counts(&scene.rig.world).await, before);
    assert_eq!(trial_effects(&scene.rig.world).await, effects);
    let roots = vec![
        rule("ChatMentionPreview", "chat", chat),
        rule("ChatMentionPreview", "thread", receiver.base.id),
        rule("ChatMentionPreview", "org", scene.tree.org.base.id),
        rule("ChatMentionPreview", "project", scene.tree.project.base.id),
        rule("ChatMentionPreview", "topic", scene.tree.topic.base.id),
    ];
    let mut complete = roots.clone();
    complete.push(rule("ChatMentionPreview", "chat", source));
    let reader = key(&scene.rig.world, scene.ada.id(), complete).await;
    let shown = ok(preview(&scene.rig.world, &reader.secret, chat, parts.clone(), None).await);
    assert_eq!(
        shown["agent_asks"][0]["thread"]["id"],
        json!(receiver.base.id)
    );
    assert!(
        shown["agent_asks"][0]["input"]["content"][0]["attachments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["blob"] == json!(uploaded.blob))
    );
    let missing = key(&scene.rig.world, scene.ada.id(), roots).await;
    assert_eq!(
        preview(&scene.rig.world, &missing.secret, chat, parts, None)
            .await
            .0,
        404
    );
    assert!(
        !owns_blob(&scene.rig.world, scene.ada.id(), uploaded.blob).await,
        "no temporary remember can escape the preview"
    );
    assert_eq!(counts(&scene.rig.world).await, before);
    assert_eq!(trial_effects(&scene.rig.world).await, effects);
    assert_eq!(scene.rig.commands.issued_total(), commands);
    let after: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(uploaded.blob.as_bytes().as_slice())
        .fetch_one(&scene.rig.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        after, pins,
        "no trial pin, audit, outbox, notification, post, input, job or command survives"
    );
}

#[tokio::test]
async fn picker_scope_lookup_adds_no_per_group_queries_beyond_the_existing_native_audience_work() {
    let room = Room::new().await;
    let reader = key(
        &room.world,
        room.ada.id(),
        vec![json!({
            "target":{"kind":"org","id":room.root,"reach":"subtree"},
            "resources":["org","project","topic","thread","chat","group"],
            "allow":[{"operation":"ChatMentionList","actions":["read"]}],
        })],
    )
    .await;
    // Two distinct nonempty groups expose the old per-candidate Group-by-id
    // walk. Calibrate its real SQL fingerprints through the existing supplier,
    // rather than copying SQL or confusing native per-audience work with scope
    // target lookups. Full payload equality is checked independently as well.
    let mut lookup_counts = Vec::new();
    for name in ["First bounded picker crew", "Second bounded picker crew"] {
        let group = room
            .world
            .app
            .call(OrgGroupCreate, &[&room.root])
            .token(&room.ada.token)
            .body(&OrgGroupCreateBody {
                name: name.into(),
                description: None,
            })
            .send()
            .await
            .ok()
            .into_full()
            .expect("this ordinary control requires the full native creation response");
        room.world
            .app
            .call(GroupMemberAdd, &[&group.id, &room.bea.id()])
            .token(&room.ada.token)
            .send()
            .await
            .ok();
        support::Statements::now();
        let mut conn = room.world.conn().await;
        orch_db::queries::access::groups_full(&mut conn, &[group.id])
            .await
            .unwrap();
        let (calibrated, calibration) = reader_diagnostics::trace(
            "mention-group-lookup-calibration",
            orch_db::queries::access::groups_full(&mut conn, &[group.id]),
        )
        .await;
        assert_eq!(calibrated.unwrap().len(), 1);
        drop(conn);
        let signatures: Vec<_> = calibration["signatures"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert!(
            !signatures.is_empty(),
            "the calibrated Group supplier must issue real SQL"
        );
        let native = ok(list(&room.world, &room.ada.token, room.chat, None, 100, None).await);
        ok(list(&room.world, &reader.secret, room.chat, None, 100, None).await);
        let (scoped, observed) = reader_diagnostics::trace(
            "mention-scoped-group-lookup-count",
            list(&room.world, &reader.secret, room.chat, None, 100, None),
        )
        .await;
        assert_eq!(ok(scoped), native);
        let lookups: u64 = signatures
            .iter()
            .filter_map(|signature| observed["signatures"][signature].as_u64())
            .sum();
        assert!(
            lookups > 0,
            "the complete picker really resolves its protected Group sources"
        );
        lookup_counts.push(lookups);
    }
    assert_eq!(
        lookup_counts[0], lookup_counts[1],
        "protected-name preparation must not repeat Group-by-id reads per candidate"
    );
}
