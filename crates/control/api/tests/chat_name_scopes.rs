//! Full chat replies and new forwarded copies keep live names inside this operation's source scope.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod rig;
mod support;

use chat_world::Room;
use orch_api_contract::access::Role;
use orch_api_contract::chats::{
    ChatMentionPreview, ChatMentionPreviewBody, ChatMentionTarget, ChatMessage, ChatMessageCreate,
    ChatMessageCreateBody, ChatPart, ForwardCreate, ForwardCreateBody, ForwardDestination,
    ForwardDestinationChat, ForwardSource, ForwardSourceChat,
};
use orch_api_contract::directory::{GroupMemberAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_api_contract::threads::{ContentPart, ForwardedAuthor, ForwardedFrom};
use orch_types::{Id, new_id};
use serde_json::{Value, json};

fn text(words: &str) -> ChatPart {
    ChatPart::Text { text: words.into() }
}
fn mention(target: ChatMentionTarget) -> ChatPart {
    ChatPart::Mention { target }
}
fn post(parts: Vec<ChatPart>) -> ChatMessageCreateBody {
    ChatMessageCreateBody {
        parts,
        reply_to: None,
        agent_review: None,
        client_id: new_id().to_string(),
    }
}
fn rule(kind: &str, id: Id, actions: &[&str]) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":"ChatMessageCreate","actions":actions}]})
}
async fn key(world: &support::World, owner: Id, rules: Vec<Value>) -> String {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let scope = serde_json::to_value(scope).unwrap();
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let now = world.now();
    let mut conn = world.conn().await;
    create_api_token_scoped(
        &mut conn,
        &NewApiToken {
            id: new_id(),
            user_id: owner,
            name: "Chat live-name control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: now,
            // Fixed-clock metadata controls need only this ordinary short lifetime.
            expires_at: Some(now + chrono::Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    secret
}
async fn send(
    world: &support::World,
    token: &str,
    chat: Id,
    body: &ChatMessageCreateBody,
) -> ChatMessage {
    world
        .app
        .call(ChatMessageCreate, &[&chat])
        .token(token)
        .body(body)
        .send()
        .await
        .ok()
        .into_full()
        .expect("same-operation Chat Read allows the full reply with bounded live names")
        .message
}
fn label<'a>(message: &'a ChatMessage, target: &ChatMentionTarget) -> &'a str {
    &message
        .mention_names
        .iter()
        .find(|name| &name.target == target)
        .expect("the stable mention target remains")
        .name
}

#[tokio::test]
async fn full_chat_replies_bound_group_and_role_paths_without_removing_public_people_or_stored_text()
 {
    let room = Room::new().await;
    let group_name = "Protected crew label";
    let made = room
        .world
        .app
        .call(OrgGroupCreate, &[&room.root])
        .token(&room.ada.token)
        .body(&OrgGroupCreateBody {
            name: group_name.into(),
            description: None,
        })
        .send()
        .await
        .ok();
    room.world
        .app
        .call(GroupMemberAdd, &[&made.id, &room.cai.id()])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    let group = ChatMentionTarget::Group { id: made.id };
    let role = ChatMentionTarget::Role {
        node_id: room.topic,
        role: Role::Developer,
    };
    // The identical words supplied as ordinary text must stay: this is source projection, not a
    // text scrubber that deletes every occurrence of a currently protected resource's name.
    let literal = format!("Already shared literal: {group_name}. ");
    let body = post(vec![
        text(&literal),
        mention(group.clone()),
        text(" / "),
        mention(role.clone()),
    ]);
    let ordinary = send(&room.world, &room.ada.token, room.chat, &body).await;
    assert!(label(&ordinary, &group).contains(group_name));
    assert!(label(&ordinary, &role).contains("Login"));
    assert!(label(&ordinary, &role).contains("Web"));
    assert!(label(&ordinary, &role).contains("Acme"));
    let mut limited_body = body.clone();
    limited_body.client_id = new_id().to_string();
    let limited = key(
        &room.world,
        room.ada.id(),
        vec![rule("chat", room.chat, &["write", "read"])],
    )
    .await;
    let hidden = send(&room.world, &limited, room.chat, &limited_body).await;
    assert_eq!(label(&hidden, &group), "a group");
    assert_eq!(label(&hidden, &role), "Developers of a private place");
    assert!(Room::words(&hidden).contains(&literal));
    assert!(
        matches!(&hidden.author,Some(orch_api_contract::documents::Author::User(user)) if user.id==room.ada.id())
    );
    assert!(
        hidden
            .mentions
            .iter()
            .any(|user| user.id == room.cai.id() && user.username == "cai"),
        "public user attribution retains its existing model"
    );
    assert_eq!(
        hidden
            .mentions
            .iter()
            .map(|user| user.id)
            .collect::<Vec<_>>(),
        ordinary
            .mentions
            .iter()
            .map(|user| user.id)
            .collect::<Vec<_>>()
    );
    let stored: Value = sqlx::query_scalar("SELECT content FROM chat_messages WHERE id=$1")
        .bind(hidden.id)
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        stored,
        json!(limited_body.parts),
        "live-name projection changes no accepted message material"
    );
    let replayed = send(&room.world, &limited, room.chat, &limited_body).await;
    assert_eq!(replayed.id, hidden.id);
    assert_eq!(label(&replayed, &group), "a group");
    assert_eq!(label(&replayed, &role), "Developers of a private place");
    let leaf = key(
        &room.world,
        room.ada.id(),
        vec![
            rule("chat", room.chat, &["write", "read"]),
            rule("group", made.id, &["read"]),
            rule("topic", room.topic, &["read"]),
        ],
    )
    .await;
    let leaf = send(&room.world, &leaf, room.chat, &limited_body).await;
    assert_eq!(
        label(&leaf, &group),
        group_name,
        "a group grant does not name its org"
    );
    assert!(label(&leaf, &role).contains("Login"));
    assert!(!label(&leaf, &role).contains("Acme"));
    assert!(!label(&leaf, &role).contains("Web"));
    let full = key(
        &room.world,
        room.ada.id(),
        vec![
            rule("chat", room.chat, &["write", "read"]),
            rule("group", made.id, &["read"]),
            rule("topic", room.topic, &["read"]),
            rule("project", room.project, &["read"]),
            rule("org", room.root, &["read"]),
        ],
    )
    .await;
    let shown = send(&room.world, &full, room.chat, &limited_body).await;
    assert_eq!(shown.id, hidden.id);
    assert_eq!(label(&shown, &group), label(&ordinary, &group));
    assert_eq!(label(&shown, &role), label(&ordinary, &role));
    assert!(Room::words(&shown).contains(&literal));
}

#[tokio::test]
async fn opening_draft_live_labels_use_its_own_operation_and_current_private_snapshot() {
    use orch_api_contract::chats::{
        ChatDraftSet, ChatDraftSetBody, ChatOpening, ChatOpeningGet, ChatSyncGetQuery,
    };
    let room = Room::new().await;
    let group_name = "Private draft crew";
    let group = room
        .world
        .app
        .call(OrgGroupCreate, &[&room.root])
        .token(&room.ada.token)
        .body(&OrgGroupCreateBody {
            name: group_name.into(),
            description: None,
        })
        .send()
        .await
        .ok();
    let group_target = ChatMentionTarget::Group { id: group.id };
    let role_target = ChatMentionTarget::Role {
        node_id: room.topic,
        role: Role::Developer,
    };
    let parts = vec![
        text(group_name),
        mention(group_target.clone()),
        mention(role_target.clone()),
    ];
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
    async fn open(room: &Room, token: &str) -> ChatOpening {
        room.world
            .app
            .call(ChatOpeningGet, &[&room.chat])
            .token(token)
            .query(&ChatSyncGetQuery::default())
            .send()
            .await
            .ok()
    }
    let ordinary = open(&room, &room.ada.token).await.draft.unwrap();
    assert!(
        ordinary
            .mention_names
            .iter()
            .any(|name| name.target == group_target && name.name.contains(group_name))
    );
    assert!(
        ordinary
            .mention_names
            .iter()
            .any(|name| name.target == role_target && name.name.contains("Login"))
    );
    let opening_rule = |kind: &str, id: Id| {
        json!({
            "target":{"kind":kind,"id":id,"reach":"self"},
            "allow":[{"operation":"ChatOpeningGet","actions":["read"]}]
        })
    };
    let limited = key(
        &room.world,
        room.ada.id(),
        vec![opening_rule("chat", room.chat)],
    )
    .await;
    let hidden = open(&room, &limited).await.draft.unwrap();
    assert_eq!(
        hidden.parts, parts,
        "literal text and stable typed targets remain unchanged"
    );
    assert!(
        hidden
            .mention_names
            .iter()
            .any(|name| name.target == group_target && name.name == "a group")
    );
    assert!(
        hidden
            .mention_names
            .iter()
            .any(|name| name.target == role_target && name.name == "Developers of a private place")
    );
    let full = key(
        &room.world,
        room.ada.id(),
        vec![
            opening_rule("chat", room.chat),
            opening_rule("group", group.id),
            opening_rule("topic", room.topic),
            opening_rule("project", room.project),
            opening_rule("org", room.root),
        ],
    )
    .await;
    assert_eq!(open(&room, &full).await.draft.unwrap(), ordinary);
    assert!(
        open(&room, &room.bea.token).await.draft.is_none(),
        "another member has only their own draft"
    );
    let replacement = vec![text(
        "The caller changed this draft after its first opening",
    )];
    room.world
        .app
        .call(ChatDraftSet, &[&room.chat])
        .token(&room.ada.token)
        .body(&ChatDraftSetBody {
            parts: replacement.clone(),
            reply_to: None,
        })
        .send()
        .await
        .ok();
    let current = open(&room, &limited).await.draft.unwrap();
    assert_eq!(
        current.parts, replacement,
        "each opening prepares the current raw draft again"
    );
    assert!(current.mention_names.is_empty());
}

async fn forwarded_input(s: &rig::Scene, thread: Id, message: Id) -> Vec<ContentPart> {
    let content: Value = sqlx::query_scalar(
        "SELECT content FROM thread_inputs WHERE thread_id=$1 AND client_id=$2 AND author_id=$3",
    )
    .bind(thread)
    .bind(format!("chat-mention:{message}"))
    .bind(s.ada.id())
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    serde_json::from_value(content).unwrap()
}
fn from_node(copy: &orch_api_contract::threads::Forwarded) -> &orch_api_contract::nodes::NodeRef {
    let ForwardedFrom::Chat { chat } = &copy.from else {
        panic!("the copied source remains a chat")
    };
    chat.node
        .as_ref()
        .expect("these source chats belong to topics")
}
fn snapshot<'a>(parts: &'a [ContentPart], source: Id) -> &'a orch_api_contract::threads::Forwarded {
    parts
        .iter()
        .find_map(|part| match part {
            ContentPart::Forwarded(copy) if copy.source_id == source => Some(copy.as_ref()),
            _ => None,
        })
        .expect("the original snapshot was preserved")
}

#[tokio::test]
async fn new_agent_copies_bound_live_names_but_keep_existing_forwarded_snapshots_and_user_refs() {
    let s = rig::Scene::new().await;
    let target = s.thread_by(&s.ada, "Current protected receiver").await;
    let thread = target.base.id;
    let target_name = target.base.name.clone();
    let chat = s.tree.topic.base.chat_id;
    let topic_name = s.tree.topic.base.name.clone();
    sqlx::query("UPDATE threads SET queue_held=true WHERE node_id=$1")
        .bind(thread)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let original_topic = places::make_topic(
        &s.rig.world,
        &s.ada,
        s.tree.project.base.id,
        "Saved snapshot origin",
    )
    .await;
    let original_chat = original_topic.base.chat_id;
    let original = send(
        &s.rig.world,
        &s.eve.token,
        original_chat,
        &post(vec![text("Shared snapshot words")]),
    )
    .await;
    let copied = s
        .rig
        .world
        .app
        .call(ForwardCreate, &[])
        .token(&s.ada.token)
        .body(&ForwardCreateBody {
            from: ForwardSource::Chat(ForwardSourceChat {
                chat_id: original_chat,
                message_ids: vec![original.id],
            }),
            to: vec![ForwardDestination::Chat(ForwardDestinationChat {
                chat_id: chat,
            })],
            comment: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("the ordinary forwarding API returns its full snapshot");
    let saved = copied.chats[0].message.clone();
    let ChatPart::Forwarded(saved_copy) = &saved.parts[0] else {
        panic!("the ordinary forwarding API made a stored snapshot")
    };
    assert_eq!(
        from_node(saved_copy).name.as_deref(),
        Some("Saved snapshot origin")
    );
    let mut body = post(vec![
        text("Act on this "),
        mention(ChatMentionTarget::Agent { thread_id: thread }),
    ]);
    body.reply_to = Some(saved.id);
    let preview = s
        .rig
        .world
        .app
        .call(ChatMentionPreview, &[&chat])
        .token(&s.ada.token)
        .body(&ChatMentionPreviewBody {
            parts: body.parts.clone(),
            reply_to: body.reply_to,
        })
        .send()
        .await
        .ok();
    assert_eq!(preview.agent_asks.len(), 1);
    assert_eq!(
        preview.agent_asks[0].thread.name.as_deref(),
        Some(target_name.as_str())
    );
    assert!(preview.agent_asks[0].thread.thread_display.is_some());
    body.agent_review = preview.agent_review;
    let limited = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule("chat", chat, &["write", "read"]),
            rule("thread", thread, &["write"]),
        ],
    )
    .await;
    let hidden = send(&s.rig.world, &limited, chat, &body).await;
    assert_eq!(hidden.asked_agents.len(), 1);
    assert_eq!(hidden.asked_agents[0].thread.id, thread);
    assert!(hidden.asked_agents[0].thread.name.is_none());
    assert!(hidden.asked_agents[0].thread.thread_display.is_none());
    assert!(hidden.asked_agents[0].thread.redacted);
    assert_eq!(
        label(&hidden, &ChatMentionTarget::Agent { thread_id: thread }),
        "an agent"
    );
    let hidden_parts = forwarded_input(&s, thread, hidden.id).await;
    let preserved = snapshot(&hidden_parts, original.id);
    assert_eq!(
        preserved,
        saved_copy.as_ref(),
        "stored attribution and old copied names are already shared material"
    );
    assert!(matches!(&preserved.author,Some(ForwardedAuthor::User(user)) if user.id==s.eve.id()));
    let fresh = snapshot(&hidden_parts, hidden.id);
    assert!(fresh.text.contains("@an agent"));
    assert!(!fresh.text.contains(&target_name));
    assert!(from_node(fresh).name.is_none());
    assert!(from_node(fresh).redacted);
    assert!(matches!(&fresh.author,Some(ForwardedAuthor::User(user)) if user.id==s.ada.id()));
    let repeated = send(&s.rig.world, &limited, chat, &body).await;
    assert_eq!(repeated.id, hidden.id);
    assert!(repeated.asked_agents[0].thread.name.is_none());
    assert!(repeated.asked_agents[0].thread.thread_display.is_none());
    assert_eq!(forwarded_input(&s, thread, hidden.id).await, hidden_parts);
    // Same-route source Read restores live receiver/source-node names without granting the
    // unrelated original snapshot location or adding any Account Read requirement.
    let full = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule("chat", chat, &["write", "read"]),
            rule("thread", thread, &["write", "read"]),
            rule("topic", s.tree.topic.base.id, &["read"]),
        ],
    )
    .await;
    let shown = send(&s.rig.world, &full, chat, &body).await;
    assert_eq!(shown.id, hidden.id);
    assert_eq!(
        shown.asked_agents[0].thread.name.as_deref(),
        Some(target_name.as_str())
    );
    assert!(shown.asked_agents[0].thread.thread_display.is_some());
    body.client_id = new_id().to_string();
    let shown = send(&s.rig.world, &full, chat, &body).await;
    let shown_parts = forwarded_input(&s, thread, shown.id).await;
    assert_eq!(snapshot(&shown_parts, original.id), saved_copy.as_ref());
    let fresh = snapshot(&shown_parts, shown.id);
    assert!(fresh.text.contains(&format!("@{target_name}")));
    assert_eq!(from_node(fresh).name.as_deref(), Some(topic_name.as_str()));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 2, "replayed name projections schedule no new work");
    assert!(s.rig.turn_starts(thread).is_empty());
    s.rig.engine.stop();
}
