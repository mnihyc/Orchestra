//! The native message windows use one exact Chat read and current-name source footprint.
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
    ChatMentionTarget, ChatMessageCreate, ChatMessageCreateBody, ChatMessageList,
    ChatMessagePinSet, ChatPart, ForwardCreate, ForwardCreateBody, ForwardDestination,
    ForwardDestinationChat, ForwardSource, ForwardSourceChat,
};
use orch_api_contract::directory::{GroupMemberAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_types::{Id, new_id};
use serde_json::{Value, json};

const OP: &str = "ChatMessageList";
fn rule_for(operation: &str, kind: &str, id: Id) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}
fn rule(kind: &str, id: Id) -> Value {
    rule_for(OP, kind, id)
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
    let scope = serde_json::to_value(scope).unwrap();
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Message window scope control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            // Fixed-clock request controls fit the established short key-test lifetime.
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn list(world: &support::World, token: &str, chat: Id, query: &str) -> (u16, Value) {
    let r = world
        .app
        .call(ChatMessageList, &[&chat])
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
fn ids(body: &Value) -> Vec<Id> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| serde_json::from_value(m["id"].clone()).unwrap())
        .collect()
}

#[tokio::test]
async fn a_chat_read_key_keeps_each_native_before_after_around_and_pinned_window() {
    let room = Room::new().await;
    let one = room.say(&room.ada, room.chat, "Window one").await;
    let two = room.say(&room.ada, room.chat, "Window two").await;
    let three = room.say(&room.ada, room.chat, "Window three").await;
    room.world
        .app
        .call(ChatMessagePinSet, &[&room.chat, &two.id])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    let seq = room.sync(&room.ada, room.chat).await.seq;
    let queries = [
        format!("before={}&limit=2", seq + 1),
        "after=0&limit=2".to_owned(),
        format!("around={}&limit=2", two.id),
        "pinned=true&limit=2".to_owned(),
    ];
    // Prove all four native selections before the first modern positive can fail.
    let mut expected = Vec::new();
    for query in &queries {
        let native = ok(list(&room.world, &room.ada.token, room.chat, query).await);
        assert!(!ids(&native).is_empty(), "{query}");
        assert!(native["has_older"].is_boolean() && native["has_newer"].is_boolean());
        expected.push(native);
    }
    assert_eq!(ids(&expected[3]), [two.id]);
    assert!(ids(&expected[0]).contains(&three.id));
    assert!(ids(&expected[1]).contains(&one.id));
    let token = key(&room.world, room.ada.id(), vec![rule("chat", room.chat)]).await;
    for (query, native) in queries.iter().zip(expected) {
        assert_eq!(
            ok(list(&room.world, &token.secret, room.chat, query).await),
            native,
            "{query}"
        );
    }
}

#[tokio::test]
async fn windows_keep_native_errors_real_empty_results_and_actual_chat_identity() {
    let room = Room::new().await;
    let _local = room.say(&room.ada, room.chat, "Only the actual Chat").await;
    let other = room.direct(&room.ada, &room.bea).await.base.id;
    let foreign = room.say(&room.ada, other, "Never a foreign window").await;
    let wrong = format!("around={}", foreign.id);
    assert_eq!(
        list(&room.world, &room.ada.token, room.chat, &wrong)
            .await
            .0,
        404
    );
    for invalid in ["", "pinned=false", "before=1&after=0"] {
        assert_eq!(
            list(&room.world, &room.ada.token, room.chat, invalid)
                .await
                .0,
            400,
            "{invalid}"
        );
    }
    let highwater = room.sync(&room.ada, room.chat).await.seq;
    let empty_query = format!("after={highwater}");
    let empty = ok(list(&room.world, &room.ada.token, room.chat, &empty_query).await);
    assert!(ids(&empty).is_empty());
    let token = key(&room.world, room.ada.id(), vec![rule("chat", room.chat)]).await;
    assert_eq!(
        ok(list(&room.world, &token.secret, room.chat, &empty_query).await),
        empty
    );
    assert_eq!(
        list(&room.world, &token.secret, room.chat, &wrong).await.0,
        404
    );
    for invalid in ["", "pinned=false", "before=1&after=0"] {
        assert_eq!(
            list(&room.world, &token.secret, room.chat, invalid).await.0,
            400,
            "{invalid}"
        );
    }
    let unrelated = key(
        &room.world,
        room.ada.id(),
        vec![rule_for("ChatSyncGet", "chat", room.chat)],
    )
    .await;
    assert_eq!(
        list(&room.world, &unrelated.secret, room.chat, "after=0")
            .await
            .0,
        403
    );
    let denied = key(&room.world, room.eve.id(), vec![rule("chat", room.chat)]).await;
    assert_eq!(
        list(&room.world, &room.eve.token, room.chat, "after=0")
            .await
            .0,
        403
    );
    let denied = list(&room.world, &denied.secret, room.chat, "after=0").await;
    assert_eq!(denied.0, 403);
    assert!(!denied.1.to_string().contains("Only the actual Chat"));
}

fn label<'a>(message: &'a Value, target: &ChatMentionTarget) -> &'a str {
    message["mention_names"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["target"] == json!(target))
        .unwrap()["name"]
        .as_str()
        .unwrap()
}
#[tokio::test]
async fn message_list_names_need_this_operations_sources_and_leave_stored_parts_unchanged() {
    let room = Room::new().await;
    let name = "Protected window crew";
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
    let literal = format!("Previously shared literal: {name}. ");
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
    let native = ok(list(&room.world, &room.ada.token, room.chat, "after=0").await);
    let row = native["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == json!(said.id))
        .unwrap();
    assert!(label(row, &group_target).contains(name));
    assert!(label(row, &role_target).contains("Login"));
    let stored =
        orch_db::queries::chats::message(&mut *room.world.conn().await, room.chat, said.id)
            .await
            .unwrap()
            .unwrap()
            .content;
    let limited = key(
        &room.world,
        room.ada.id(),
        vec![
            rule("chat", room.chat),
            rule_for("ChatSyncGet", "group", group.id),
        ],
    )
    .await;
    let hidden = ok(list(&room.world, &limited.secret, room.chat, "after=0").await);
    let row = hidden["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == json!(said.id))
        .unwrap();
    assert_eq!(label(row, &group_target), "a group");
    assert_eq!(label(row, &role_target), "Developers of a private place");
    // Native messages render/coalesce mentions with the independently checked
    // current fallback labels; the authored typed parts stay in stored content.
    assert_eq!(
        row["parts"],
        json!([text(&format!(
            "{literal}@a group / @Developers of a private place"
        ))])
    );
    let full = key(
        &room.world,
        room.ada.id(),
        vec![
            rule("chat", room.chat),
            rule("group", group.id),
            rule("topic", room.topic),
            rule("project", room.project),
            rule("org", room.root),
        ],
    )
    .await;
    assert_eq!(
        ok(list(&room.world, &full.secret, room.chat, "after=0").await),
        native
    );
    let after = orch_db::queries::chats::message(&mut *room.world.conn().await, room.chat, said.id)
        .await
        .unwrap()
        .unwrap()
        .content;
    assert_eq!(after, stored);
}

#[tokio::test]
async fn a_new_message_list_rechecks_current_key_rights_without_reusing_earlier_rows() {
    let room = Room::new().await;
    room.say(&room.ada, room.chat, "A row behind the current key")
        .await;
    let native = ok(list(&room.world, &room.ada.token, room.chat, "after=0").await);
    assert!(!ids(&native).is_empty());
    let token = key(&room.world, room.ada.id(), vec![rule("chat", room.chat)]).await;
    assert_eq!(
        ok(list(&room.world, &token.secret, room.chat, "after=0").await),
        native
    );
    let unrelated = json!({"rules":[rule_for("ChatSyncGet","chat",room.chat)]});
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(token.id)
        .bind(unrelated)
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        list(&room.world, &token.secret, room.chat, "after=0")
            .await
            .0,
        403
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(token.id)
        .bind(room.world.now())
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        list(&room.world, &token.secret, room.chat, "after=0")
            .await
            .0,
        401
    );
}

#[tokio::test]
async fn an_item_window_reads_already_shared_chat_refs_without_lending_thread_view() {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await;
    scene
        .exchange(thread.base.id, 0, "A fixture exchange", "A fixture answer")
        .await;
    let item:Id=sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND kind='agent_message' ORDER BY seq_created LIMIT 1")
        .bind(thread.base.id).fetch_one(&scene.rig.world.db.pool).await.unwrap();
    let parts = vec![
        text("This item was shared in this Chat"),
        ChatPart::ItemRef {
            thread_id: thread.base.id,
            item_id: item,
        },
    ];
    let posted = scene
        .rig
        .world
        .app
        .call(ChatMessageCreate, &[&thread.base.chat_id])
        .token(&scene.ada.token)
        .body(&ChatMessageCreateBody {
            parts: parts.clone(),
            reply_to: None,
            agent_review: None,
            client_id: new_id().to_string(),
        })
        .send_bytes()
        .await;
    assert_eq!(posted.status, 201);
    let posted: Value = serde_json::from_slice(&posted.raw).unwrap();
    let message: Id = serde_json::from_value(posted["message"]["id"].clone()).unwrap();
    orch_db::queries::access::testing::add_entry(
        &mut *scene.rig.world.conn().await,
        &orch_db::queries::access::PageKey::new("node", thread.base.id, "live"),
        "any",
        "chat.view",
        ("user", Some(scene.bea.id()), None),
        None,
        scene.rig.world.now(),
    )
    .await
    .unwrap();
    let hidden_thread = scene
        .rig
        .world
        .app
        .call(orch_api_contract::nodes::NodeGet, &[&thread.base.id])
        .token(&scene.bea.token)
        .query(&orch_api_contract::nodes::DeletedInspectionQuery::default())
        .send_bytes()
        .await;
    assert_eq!(
        hidden_thread.status, 403,
        "ordinary ChatView is not ThreadView"
    );
    let query = format!("item_id={item}&limit=20");
    let native = ok(list(
        &scene.rig.world,
        &scene.bea.token,
        thread.base.chat_id,
        &query,
    )
    .await);
    assert_eq!(ids(&native), [message]);
    assert_eq!(native["messages"][0]["parts"], json!(parts));
    let token = key(
        &scene.rig.world,
        scene.bea.id(),
        vec![rule("chat", thread.base.chat_id)],
    )
    .await;
    let read = ok(list(&scene.rig.world, &token.secret, thread.base.chat_id, &query).await);
    assert_eq!(ids(&read), ids(&native));
    assert_eq!(read["messages"][0]["parts"], native["messages"][0]["parts"]);
    assert_eq!(read["has_older"], native["has_older"]);
    assert_eq!(read["has_newer"], native["has_newer"]);
    let empty = format!("item_id={}", new_id());
    assert!(
        ids(&ok(list(
            &scene.rig.world,
            &token.secret,
            thread.base.chat_id,
            &empty
        )
        .await))
        .is_empty()
    );
}

#[tokio::test]
async fn a_list_of_copied_material_needs_no_source_chat_read_or_snapshot_name_scrubbing() {
    let room = Room::new().await;
    let original = room
        .say(
            &room.ada,
            room.chat,
            "The original material was deliberately copied",
        )
        .await;
    let destination = room.direct(&room.ada, &room.bea).await.base.id;
    let forwarded = room
        .world
        .app
        .call(ForwardCreate, &[])
        .token(&room.ada.token)
        .body(&ForwardCreateBody {
            from: ForwardSource::Chat(ForwardSourceChat {
                chat_id: room.chat,
                message_ids: vec![original.id],
            }),
            to: vec![ForwardDestination::Chat(ForwardDestinationChat {
                chat_id: destination,
            })],
            comment: None,
            client_id: new_id().to_string(),
        })
        .send_bytes()
        .await;
    assert_eq!(forwarded.status, 201);
    let forwarded: Value = serde_json::from_slice(&forwarded.raw).unwrap();
    let copied: Id =
        serde_json::from_value(forwarded["chats"][0]["message"]["id"].clone()).unwrap();
    let before = ok(list(&room.world, &room.ada.token, destination, "after=0").await);
    let copied_parts = before["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == json!(copied))
        .unwrap()["parts"]
        .clone();
    assert!(
        copied_parts.to_string().contains("Login"),
        "the native stored source snapshot carries its shared name"
    );
    let source = room
        .world
        .app
        .call(orch_api_contract::nodes::NodeGet, &[&room.topic])
        .token(&room.ada.token)
        .send_bytes()
        .await;
    assert_eq!(source.status, 200);
    let source: Value = serde_json::from_slice(&source.raw).unwrap();
    let renamed = room
        .world
        .app
        .call(orch_api_contract::nodes::NodeUpdate, &[&room.topic])
        .token(&room.ada.token)
        .raw_body(
            &json!({"name":"Source renamed after copying","expected_revision":source["revision"]}),
        )
        .send_bytes()
        .await;
    assert_eq!(renamed.status, 200);
    let native = ok(list(&room.world, &room.ada.token, destination, "after=0").await);
    let native_parts = &native["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == json!(copied))
        .unwrap()["parts"];
    assert_eq!(
        native_parts, &copied_parts,
        "a stored shared snapshot is not rebound to a new source name"
    );
    let token = key(&room.world, room.ada.id(), vec![rule("chat", destination)]).await;
    let shown = ok(list(&room.world, &token.secret, destination, "after=0").await);
    let shown_parts = &shown["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == json!(copied))
        .unwrap()["parts"];
    assert_eq!(shown_parts, &copied_parts);
    assert_eq!(ids(&shown), ids(&native));
    assert_eq!(
        list(&room.world, &token.secret, room.chat, "after=0")
            .await
            .0,
        403,
        "destination reading never grants the original Chat"
    );
}
