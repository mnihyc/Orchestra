//! Chat readers share their ordinary content while bounding live names and current credential views.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod realtime_support;
mod rig;
mod support;
#[path = "rig_tool_source/mod.rs"]
mod tool_source;

use crate::support::FullPermissionPageReply;
use chat_world::Room;
use orch_api_contract::access::{Holder, PermissionPageUpdate, Role};
use orch_api_contract::chats::{
    Chat, ChatGet, ChatMentionTarget, ChatMessage, ChatMessageCreate, ChatMessageCreateBody,
    ChatMessageDelete, ChatMessageUpdate, ChatMessageUpdateBody, ChatPart, ChatSync, ChatSyncGet,
    ChatSyncGetQuery, ChatUpdate, ChatUpdateBody,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{GroupHolderAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_api_contract::documents::Author;
use orch_api_core::testing::Reply;
use orch_config::ServerConfig;
use orch_db::Db;
use orch_types::{Clock, Id, new_id};
use realtime_support::{Client, Server, quick};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

struct Key {
    id: Id,
    secret: String,
}

fn rule(operation: &str, kind: &str, id: Id) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},
        "allow":[{"operation":operation,"actions":["read"]}]})
}

async fn key(world: &support::World, user: Id, rules: Vec<Value>) -> Key {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let now = world.now();
    create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Chat reader scope control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: now,
            expires_at: Some(now + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    Key { id, secret }
}

async fn narrow_key(world: &support::World, key: &Key, rules: Vec<Value>) {
    let scope: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    // This changes the actual persisted policy of the already-authenticated credential. The
    // following publication must restore it, without another key lending its richer caller.
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(key.id)
        .bind(serde_json::to_value(scope).unwrap())
        .execute(&world.db.pool)
        .await
        .unwrap();
}

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
        .expect("the ordinary session post has its full reply")
        .message
}

async fn get(world: &support::World, token: &str, chat: Id) -> Reply<Chat> {
    world.app.call(ChatGet, &[&chat]).token(token).send().await
}

async fn sync(
    world: &support::World,
    token: &str,
    chat: Id,
    query: ChatSyncGetQuery,
) -> Reply<ChatSync> {
    world
        .app
        .call(ChatSyncGet, &[&chat])
        .token(token)
        .query(&query)
        .send()
        .await
}

fn label<'a>(message: &'a ChatMessage, target: &ChatMentionTarget) -> &'a str {
    &message
        .mention_names
        .iter()
        .find(|entry| &entry.target == target)
        .expect("the stable mention target remains")
        .name
}

async fn group(room: &Room, name: &str) -> Id {
    let made = room
        .world
        .app
        .call(OrgGroupCreate, &[&room.root])
        .token(&room.ada.token)
        .body(&OrgGroupCreateBody {
            name: name.into(),
            description: Some("A full inspection group description".into()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response");
    room.world
        .app
        .call(GroupHolderAdd, &[&made.id])
        .token(&room.ada.token)
        .body(&orch_api_contract::directory::GroupHolderAddBody {
            holder: orch_api_contract::access::HolderInput::User { id: room.cai.id() },
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    made.id
}

fn root_names(room: &Room, operation: &str) -> Vec<Value> {
    vec![
        rule(operation, "org", room.root),
        rule(operation, "project", room.project),
        rule(operation, "topic", room.topic),
    ]
}

#[tokio::test]
async fn each_chat_reader_requires_its_exact_operation_and_chat_target_and_ordinary_read() {
    let room = Room::new().await;
    room.say(&room.ada, room.chat, "Existing shared words")
        .await;
    let other_chat = room.chat_in_another_topic().await;
    for operation in ["ChatGet", "ChatSyncGet"] {
        let proper = key(
            &room.world,
            room.bea.id(),
            vec![rule(operation, "chat", room.chat)],
        )
        .await;
        let wrong_target = key(
            &room.world,
            room.bea.id(),
            vec![rule(operation, "chat", other_chat)],
        )
        .await;
        let wrong_operation = key(
            &room.world,
            room.bea.id(),
            vec![rule(
                if operation == "ChatGet" {
                    "ChatSyncGet"
                } else {
                    "ChatGet"
                },
                "chat",
                room.chat,
            )],
        )
        .await;
        let outside = key(
            &room.world,
            room.eve.id(),
            vec![rule(operation, "chat", room.chat)],
        )
        .await;
        if operation == "ChatGet" {
            assert_eq!(
                get(&room.world, &proper.secret, room.chat)
                    .await
                    .ok()
                    .base
                    .id,
                room.chat
            );
            for token in [
                &wrong_target.secret,
                &wrong_operation.secret,
                &outside.secret,
            ] {
                assert_eq!(
                    get(&room.world, token, room.chat).await.problem().code,
                    ErrorCode::AuthzDenied
                );
            }
        } else {
            let shown = sync(&room.world, &proper.secret, room.chat, Default::default())
                .await
                .ok();
            assert_eq!(Room::words(&shown.messages[0]), "Existing shared words");
            for token in [
                &wrong_target.secret,
                &wrong_operation.secret,
                &outside.secret,
            ] {
                assert_eq!(
                    sync(&room.world, token, room.chat, Default::default())
                        .await
                        .problem()
                        .code,
                    ErrorCode::AuthzDenied
                );
            }
        }
    }
    // This is the same underlying ordinary refusal, without credential restrictions.
    assert_eq!(
        get(&room.world, &room.eve.token, room.chat)
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn live_group_role_and_root_names_are_bounded_without_scrubbing_chat_words_or_public_people()
{
    let room = Room::new().await;
    let group_id = group(&room, "Protected reader crew").await;
    let group = ChatMentionTarget::Group { id: group_id };
    let role = ChatMentionTarget::Role {
        node_id: room.topic,
        role: Role::Developer,
    };
    let literal = "Stored literal: Protected reader crew and Acme Web Login.";
    let original = send(
        &room.world,
        &room.ada.token,
        room.chat,
        &post(vec![
            text(literal),
            mention(group.clone()),
            mention(role.clone()),
        ]),
    )
    .await;
    let mut reply = post(vec![text("Reply retains the accepted source words")]);
    reply.reply_to = Some(original.id);
    let reply = send(&room.world, &room.bea.token, room.chat, &reply).await;
    let ordinary = room.sync(&room.ada, room.chat).await;
    let normal = ordinary
        .messages
        .iter()
        .find(|message| message.id == original.id)
        .unwrap();
    assert!(label(normal, &group).contains("Protected reader crew"));
    assert!(label(normal, &role).contains("Login"));
    for operation in ["ChatGet", "ChatSyncGet"] {
        let other_operation = if operation == "ChatGet" {
            "ChatSyncGet"
        } else {
            "ChatGet"
        };
        let mut unrelated_reads = vec![
            rule(operation, "chat", room.chat),
            rule(other_operation, "group", group_id),
        ];
        unrelated_reads.extend(root_names(&room, other_operation));
        let narrow = key(&room.world, room.ada.id(), unrelated_reads).await;
        let mut full_rules = vec![
            rule(operation, "chat", room.chat),
            rule(operation, "group", group_id),
        ];
        full_rules.extend(root_names(&room, operation));
        let full = key(&room.world, room.ada.id(), full_rules).await;
        if operation == "ChatGet" {
            let hidden = get(&room.world, &narrow.secret, room.chat).await.ok();
            let node = hidden.base.node.as_ref().unwrap();
            assert_eq!(node.id, room.topic);
            assert!(node.redacted && node.name.is_none() && node.thread_display.is_none());
            assert_eq!(hidden.preview.as_ref().unwrap().id, reply.id);
            assert!(
                matches!(&hidden.preview.as_ref().unwrap().author,Some(Author::User(user)) if user.id==room.bea.id())
            );
            assert_eq!(
                get(&room.world, &full.secret, room.chat).await.ok(),
                ordinary.chat
            );
        } else {
            let hidden = sync(&room.world, &narrow.secret, room.chat, Default::default())
                .await
                .ok();
            let message = hidden
                .messages
                .iter()
                .find(|message| message.id == original.id)
                .unwrap();
            assert_eq!(label(message, &group), "a group");
            assert_eq!(label(message, &role), "Developers of a private place");
            assert!(Room::words(message).contains(literal));
            assert!(
                matches!(&message.author,Some(Author::User(user)) if user.id==room.ada.id() && user.username=="ada")
            );
            assert_eq!(message.mentions, normal.mentions);
            let reply = hidden
                .messages
                .iter()
                .find(|message| message.id == reply.id)
                .unwrap();
            assert_eq!(reply.reply_to.as_ref().unwrap().id, original.id);
            assert!(reply.reply_to.as_ref().unwrap().excerpt.contains(literal));
            assert_eq!(
                sync(&room.world, &full.secret, room.chat, Default::default())
                    .await
                    .ok(),
                ordinary
            );
        }
    }
}

#[tokio::test]
async fn sync_keeps_original_limit_since_and_deleted_row_semantics_under_the_same_reader_scope() {
    let room = Room::new().await;
    let mut sent = Vec::new();
    for word in ["one", "two", "three", "four"] {
        sent.push(room.say(&room.ada, room.chat, word).await);
    }
    let mut rules = vec![rule("ChatSyncGet", "chat", room.chat)];
    rules.extend(root_names(&room, "ChatSyncGet"));
    let token = key(&room.world, room.bea.id(), rules).await;
    let first = sync(
        &room.world,
        &token.secret,
        room.chat,
        ChatSyncGetQuery {
            since: None,
            limit: Some(2),
        },
    )
    .await
    .ok();
    let ordinary = sync(
        &room.world,
        &room.bea.token,
        room.chat,
        ChatSyncGetQuery {
            since: None,
            limit: Some(2),
        },
    )
    .await
    .ok();
    assert_eq!(first, ordinary);
    assert_eq!(
        first
            .messages
            .iter()
            .map(|message| message.id)
            .collect::<Vec<_>>(),
        vec![sent[2].id, sent[3].id]
    );
    assert!(first.complete && first.has_older);
    room.world
        .app
        .call(ChatMessageUpdate, &[&room.chat, &sent[1].id])
        .token(&room.ada.token)
        .body(&ChatMessageUpdateBody {
            parts: vec![text("two edited")],
        })
        .send()
        .await
        .ok();
    room.world
        .app
        .call(ChatMessageDelete, &[&room.chat, &sent[3].id])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    let query = ChatSyncGetQuery {
        since: Some(first.seq),
        limit: None,
    };
    let changed = sync(&room.world, &token.secret, room.chat, query.clone())
        .await
        .ok();
    assert_eq!(
        changed,
        sync(&room.world, &room.bea.token, room.chat, query)
            .await
            .ok()
    );
    assert_eq!(changed.messages.len(), 2);
    assert_eq!(
        Room::words(
            changed
                .messages
                .iter()
                .find(|message| message.id == sent[1].id)
                .unwrap()
        ),
        "two edited"
    );
    let deleted = changed
        .messages
        .iter()
        .find(|message| message.id == sent[3].id)
        .unwrap();
    assert!(deleted.deleted && deleted.parts.is_empty());
    let preview = changed.chat.preview.as_ref().unwrap();
    assert_eq!(preview.id, sent[3].id);
    assert!(
        preview.deleted && preview.text.is_empty(),
        "deletion does not substitute older preview text"
    );
    assert!(changed.complete && changed.since == Some(first.seq));
    assert!(
        sync(
            &room.world,
            &token.secret,
            room.chat,
            ChatSyncGetQuery {
                since: Some(changed.seq),
                limit: None
            }
        )
        .await
        .ok()
        .messages
        .is_empty()
    );
}

#[tokio::test]
async fn direct_chat_inspection_holder_requires_group_source_read_but_never_account_read() {
    let room = Room::new().await;
    let group_id = group(&room, "Current chat inspectors").await;
    room.world.app.call(PermissionPageUpdate,&[&"nodes",&room.root]).token(&room.ada.token)
        .raw_body(&json!({"rows":[{"permission":"directs.view","add":[{"holder":{"kind":"group","id":group_id}}]}]}))
        .send().await.full_permission_page().ok();
    let direct = room.direct(&room.bea, &room.cai).await;
    room.say(&room.bea, direct.base.id, "Direct shared content")
        .await;
    let ordinary = get(&room.world, &room.bea.token, direct.base.id).await.ok();
    let expected = ordinary
        .inspection_holders
        .as_ref()
        .unwrap()
        .iter()
        .find_map(|holder| match holder {
            Holder::Group { group } if group.id == group_id => Some(group),
            _ => None,
        })
        .expect("the normal direct chat actually includes this full inspection holder");
    assert!(expected.member_count > 0);
    for operation in ["ChatGet", "ChatSyncGet"] {
        let primary = rule(operation, "chat", direct.base.id);
        let narrow = key(&room.world, room.bea.id(), vec![primary.clone()]).await;
        let mut rules = vec![primary, rule(operation, "group", group_id)];
        rules.extend(root_names(&room, operation));
        let full = key(&room.world, room.bea.id(), rules).await;
        let shown = if operation == "ChatGet" {
            assert_eq!(
                get(&room.world, &narrow.secret, direct.base.id)
                    .await
                    .problem()
                    .code,
                ErrorCode::AuthzDenied
            );
            get(&room.world, &full.secret, direct.base.id).await.ok()
        } else {
            assert_eq!(
                sync(
                    &room.world,
                    &narrow.secret,
                    direct.base.id,
                    Default::default()
                )
                .await
                .problem()
                .code,
                ErrorCode::AuthzDenied
            );
            let shown = sync(
                &room.world,
                &full.secret,
                direct.base.id,
                Default::default(),
            )
            .await
            .ok();
            assert_eq!(
                Room::words(shown.messages.last().unwrap()),
                "Direct shared content"
            );
            shown.chat
        };
        let actual = shown
            .inspection_holders
            .as_ref()
            .unwrap()
            .iter()
            .find_map(|holder| match holder {
                Holder::Group { group } if group.id == group_id => Some(group),
                _ => None,
            })
            .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(
            shown.members, ordinary.members,
            "public account identities need no Account Read selection"
        );
    }
}

async fn share_chat_with_agent(scene: &rig::Scene, chat: Id, thread: Id) {
    use orch_api_contract::share::{
        ShareAgentCreate, ShareLevel, ShareParts, ShareTo, ShareToThread, ShareWhat, ShareWhatChat,
        ShareWithAgents,
    };
    scene
        .rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&scene.ada.token)
        .body(&ShareWithAgents {
            link_origin: None,
            viewers_read: None,
            read_first: None,
            what: ShareWhat::Chat(ShareWhatChat { chat_id: chat }),
            inside: None,
            download: Some(false),
            parts: Some(ShareParts {
                chat: Some(ShareLevel::Write),
                ..Default::default()
            }),
            mode: orch_api_contract::threads::ShareMode::Live,
            to: ShareTo::Threads(vec![ShareToThread { thread_id: thread }]),
            comment: None,
            expires_at: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
}

async fn agent_post(scene: &rig::Scene, chat: Id, thread: Id, words: &str) -> Id {
    use orch_agent_contract::events::OrchTool;
    use orch_engine::tools::CallContext;
    use orch_link_contract::commands::{Command, CommandKind};
    let turn = tool_source::active(scene, thread)
        .await
        .expect("an actual authored and admitted source turn");
    let call_ref = new_id().to_string();
    scene
        .rig
        .engine
        .answer_tool_call(&CallContext {
            machine: scene.rig.machine_id,
            thread,
            turn,
            call_ref: call_ref.clone(),
            attempt: None,
            tool: OrchTool::SendMessage,
            arguments: json!({"target":{"kind":"chat","path":format!("id:{chat}")},"text":words}),
        })
        .await
        .unwrap();
    rig::eventually("the real chat posting tool result", || async {
        scene
            .rig
            .seen
            .of_kind(CommandKind::OrchToolResult)
            .iter()
            .any(|command| {
                matches!(command,
            Command::OrchToolResult(result) if result.call_ref==call_ref)
            })
    })
    .await;
    let result = scene
        .rig
        .seen
        .of_kind(CommandKind::OrchToolResult)
        .into_iter()
        .find_map(|command| match command {
            Command::OrchToolResult(result) if result.call_ref == call_ref => Some(result),
            _ => None,
        })
        .unwrap();
    assert!(
        result.error.is_none(),
        "the ordinary producer must accept this authored chat post: {:?}",
        result.error
    );
    sqlx::query_scalar("SELECT id FROM chat_messages WHERE chat_id=$1 AND author_thread_id=$2 ORDER BY seq_created DESC LIMIT 1")
        .bind(chat).bind(thread).fetch_one(scene.rig.db.pool()).await.unwrap()
}

fn agent_thread(author: &Option<Author>) -> &orch_api_contract::nodes::NodeRef {
    let Some(Author::Agent(author)) = author else {
        panic!("the real stored author remains an agent")
    };
    &author.thread
}

#[tokio::test]
async fn agent_preview_reply_mentions_and_roster_use_same_operation_thread_read_without_agent_scope()
 {
    use orch_api_contract::chats::{ChatMentionPreview, ChatMentionPreviewBody};
    let scene = rig::Scene::new().await;
    let name = "Protected live chat origin 915";
    let origin = scene.thread_by(&scene.ada, name).await.base.id;
    let chat = scene.tree.topic.base.chat_id;
    share_chat_with_agent(&scene, chat, origin).await;
    let original = agent_post(&scene, chat, origin, "A real agent-authored chat message").await;
    let ordinary = get(&scene.rig.world, &scene.ada.token, chat).await.ok();
    assert_eq!(ordinary.preview.as_ref().unwrap().id, original);
    assert_eq!(
        agent_thread(&ordinary.preview.as_ref().unwrap().author)
            .name
            .as_deref(),
        Some(name)
    );
    assert!(
        ordinary
            .agents
            .as_ref()
            .unwrap()
            .iter()
            .any(|agent| agent.thread.id == origin)
    );
    let narrow = key(
        &scene.rig.world,
        scene.ada.id(),
        vec![rule("ChatGet", "chat", chat)],
    )
    .await;
    let hidden = get(&scene.rig.world, &narrow.secret, chat).await.ok();
    let hidden_thread = agent_thread(&hidden.preview.as_ref().unwrap().author);
    assert_eq!(hidden_thread.id, origin);
    assert!(
        hidden_thread.name.is_none()
            && hidden_thread.thread_display.is_none()
            && hidden_thread.redacted
    );
    let (Some(Author::Agent(hidden_author)), Some(Author::Agent(normal_author))) = (
        &hidden.preview.as_ref().unwrap().author,
        &ordinary.preview.as_ref().unwrap().author,
    ) else {
        unreachable!()
    };
    assert!(!normal_author.agent.is_empty());
    assert_eq!(
        hidden_author.agent, normal_author.agent,
        "the stored author snapshot is not a new protected Agent resource lookup"
    );
    assert!(
        hidden
            .agents
            .as_ref()
            .unwrap()
            .iter()
            .all(|agent| agent.thread.id != origin)
    );
    let full = key(
        &scene.rig.world,
        scene.ada.id(),
        vec![
            rule("ChatGet", "chat", chat),
            rule("ChatGet", "thread", origin),
        ],
    )
    .await;
    let shown = get(&scene.rig.world, &full.secret, chat).await.ok();
    assert_eq!(
        agent_thread(&shown.preview.as_ref().unwrap().author)
            .name
            .as_deref(),
        Some(name)
    );
    assert!(
        agent_thread(&shown.preview.as_ref().unwrap().author)
            .thread_display
            .is_some()
    );
    let roster = shown
        .agents
        .as_ref()
        .unwrap()
        .iter()
        .find(|agent| agent.thread.id == origin)
        .unwrap();
    assert!(roster.readable && roster.writable);
    assert!(
        !roster.agent.is_empty(),
        "existing agent display does not require a new Agent scope"
    );

    // Hold later authored inputs; creating a mention must not silently start another native turn.
    sqlx::query("UPDATE threads SET queue_held=true WHERE node_id=$1")
        .bind(origin)
        .execute(scene.rig.db.pool())
        .await
        .unwrap();
    let literal = format!("Stored literal: {name}. ");
    let target = ChatMentionTarget::Agent { thread_id: origin };
    let mut body = post(vec![text(&literal), mention(target.clone())]);
    body.reply_to = Some(original);
    let preview = scene
        .rig
        .world
        .app
        .call(ChatMentionPreview, &[&chat])
        .token(&scene.ada.token)
        .body(&ChatMentionPreviewBody {
            parts: body.parts.clone(),
            reply_to: body.reply_to,
        })
        .send()
        .await
        .ok();
    assert_eq!(preview.agent_asks.len(), 1);
    body.agent_review = preview.agent_review;
    let reply = send(&scene.rig.world, &scene.ada.token, chat, &body).await;
    let normal = sync(&scene.rig.world, &scene.ada.token, chat, Default::default())
        .await
        .ok();
    let narrow = key(
        &scene.rig.world,
        scene.ada.id(),
        vec![rule("ChatSyncGet", "chat", chat)],
    )
    .await;
    let hidden = sync(&scene.rig.world, &narrow.secret, chat, Default::default())
        .await
        .ok();
    let stored = hidden
        .messages
        .iter()
        .find(|message| message.id == original)
        .unwrap();
    assert!(agent_thread(&stored.author).redacted);
    let repeated = hidden
        .messages
        .iter()
        .find(|message| message.id == reply.id)
        .unwrap();
    assert_eq!(label(repeated, &target), "an agent");
    assert!(Room::words(repeated).contains(&literal));
    assert!(agent_thread(&repeated.reply_to.as_ref().unwrap().author).redacted);
    assert_eq!(
        repeated.reply_to.as_ref().unwrap().excerpt,
        "A real agent-authored chat message"
    );
    assert!(
        repeated
            .asked_agents
            .iter()
            .all(|asked| asked.thread.id == origin && asked.thread.redacted)
    );
    let full = key(
        &scene.rig.world,
        scene.ada.id(),
        vec![
            rule("ChatSyncGet", "chat", chat),
            rule("ChatSyncGet", "thread", origin),
        ],
    )
    .await;
    let shown = sync(&scene.rig.world, &full.secret, chat, Default::default())
        .await
        .ok();
    let shown = shown
        .messages
        .iter()
        .find(|message| message.id == reply.id)
        .unwrap();
    let expected = normal
        .messages
        .iter()
        .find(|message| message.id == reply.id)
        .unwrap();
    assert_eq!(label(shown, &target), label(expected, &target));
    assert_eq!(
        agent_thread(&shown.reply_to.as_ref().unwrap().author)
            .name
            .as_deref(),
        Some(name)
    );
    assert!(
        shown
            .asked_agents
            .iter()
            .any(|asked| asked.thread.id == origin && !asked.thread.redacted)
    );

    scene
        .rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &origin])
        .token(&scene.ada.token)
        .raw_body(&json!({"whole":"private"}))
        .send()
        .await
        .full_permission_page()
        .ok();
    let reader = key(
        &scene.rig.world,
        scene.cai.id(),
        vec![
            rule("ChatSyncGet", "chat", chat),
            rule("ChatSyncGet", "thread", origin),
        ],
    )
    .await;
    let reader = sync(&scene.rig.world, &reader.secret, chat, Default::default())
        .await
        .ok();
    assert!(
        reader
            .chat
            .agents
            .as_ref()
            .unwrap()
            .iter()
            .all(|agent| agent.thread.id != origin),
        "scope does not manufacture ordinary ThreadView"
    );
    assert!(
        agent_thread(
            &reader
                .messages
                .iter()
                .find(|message| message.id == original)
                .unwrap()
                .author
        )
        .redacted
    );
    scene.rig.engine.stop();
}

#[tokio::test]
async fn stored_forward_attachment_and_item_reference_content_does_not_dereference_ungranted_sources()
 {
    use orch_api_contract::chats::{
        ChatAttachmentCreate, ForwardCreate, ForwardCreateBody, ForwardDestination,
        ForwardDestinationChat, ForwardSource, ForwardSourceChat,
    };
    use orch_api_contract::threads::FILENAME_HEADER;
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await;
    let origin = thread.base.id;
    let chat = thread.base.chat_id;
    scene
        .exchange(
            origin,
            0,
            "Create an actual quoted item",
            "Already shared item words",
        )
        .await;
    let item:Id=sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND kind='agent_message' ORDER BY seq_created LIMIT 1")
        .bind(origin).fetch_one(scene.rig.db.pool()).await.unwrap();
    let source_topic = places::make_topic(
        &scene.rig.world,
        &scene.ada,
        scene.tree.project.base.id,
        "Saved forward origin",
    )
    .await;
    let source = send(
        &scene.rig.world,
        &scene.ada.token,
        source_topic.base.chat_id,
        &post(vec![text("Stored forward words")]),
    )
    .await;
    let forwarded = scene
        .rig
        .world
        .app
        .call(ForwardCreate, &[])
        .token(&scene.ada.token)
        .body(&ForwardCreateBody {
            from: ForwardSource::Chat(ForwardSourceChat {
                chat_id: source_topic.base.chat_id,
                message_ids: vec![source.id],
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
        .unwrap()
        .chats[0]
        .message
        .clone();
    assert!(matches!(&forwarded.parts[0], ChatPart::Forwarded(_)));
    let upload = scene
        .rig
        .world
        .app
        .call(ChatAttachmentCreate, &[&chat])
        .token(&scene.ada.token)
        .header(FILENAME_HEADER, "Already-shared-attachment.bin")
        .bytes(b"Real accepted attachment bytes")
        .send()
        .await
        .ok();
    let accepted = send(
        &scene.rig.world,
        &scene.ada.token,
        chat,
        &post(vec![
            text("Stored attachment and quote identities"),
            ChatPart::Attachment {
                blob: upload.blob,
                name: upload.name.clone(),
                size: upload.size,
                media_type: upload.media_type.clone(),
            },
            ChatPart::ItemRef {
                thread_id: origin,
                item_id: item,
            },
        ]),
    )
    .await;
    let token = key(
        &scene.rig.world,
        scene.ada.id(),
        vec![rule("ChatSyncGet", "chat", chat)],
    )
    .await;
    let shown = sync(&scene.rig.world, &token.secret, chat, Default::default())
        .await
        .ok();
    assert_eq!(
        shown
            .messages
            .iter()
            .find(|message| message.id == forwarded.id)
            .unwrap()
            .parts,
        forwarded.parts,
        "stored copied text and attribution need no fresh source Chat Read"
    );
    assert_eq!(
        shown
            .messages
            .iter()
            .find(|message| message.id == accepted.id)
            .unwrap()
            .parts,
        accepted.parts,
        "existing attachment metadata and ItemRef IDs are shared chat content, not a source byte/item read"
    );
    assert!(
        shown.chat.base.node.as_ref().unwrap().redacted,
        "the live parent Thread name is still bounded"
    );
    let original = synced_parts(&scene.rig.world, &scene.ada.token, chat, accepted.id).await;
    assert_eq!(original, accepted.parts);
    scene.rig.engine.stop();
}

async fn synced_parts(world: &support::World, token: &str, chat: Id, message: Id) -> Vec<ChatPart> {
    sync(world, token, chat, Default::default())
        .await
        .ok()
        .messages
        .into_iter()
        .find(|row| row.id == message)
        .unwrap()
        .parts
}

async fn serve(room: &Room) -> Server {
    let clock: Arc<dyn Clock> = Arc::new(room.world.app.clock.clone());
    let db = Db::from_pool(room.world.db.pool.clone());
    let mut tuning = quick(&ServerConfig::default());
    // Match the existing chat fixture: publication, not a polling sweep, drives these updates.
    tuning.sweep_every = Duration::from_secs(60);
    Server::over(&db, &clock, tuning).await
}

async fn follow(server: &Server, token: &str, id: i64, chat: Id, since: i64) -> Client {
    let mut client = Client::signed_in(server.addr, token).await;
    client.sub(id, &format!("chat:{chat}"), Some(since)).await;
    let ack = client.expect("subscribed").await;
    assert_eq!(ack["mode"], "delta");
    client.expect("presence").await;
    client
}

fn frame_message(frame: &Value, id: Id) -> ChatMessage {
    serde_json::from_value(
        frame["data"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == json!(id))
            .unwrap()
            .clone(),
    )
    .unwrap()
}

async fn next_sync(client: &mut Client) -> Value {
    client.until("sync").await.pop().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_socket_requires_sync_operation_and_never_shares_a_richer_same_user_projection() {
    let room = Room::new().await;
    let group_id = group(&room, "Live socket crew").await;
    let group = ChatMentionTarget::Group { id: group_id };
    let role = ChatMentionTarget::Role {
        node_id: room.topic,
        role: Role::Developer,
    };
    let primary = rule("ChatSyncGet", "chat", room.chat);
    let mut rich_rules = vec![primary.clone(), rule("ChatSyncGet", "group", group_id)];
    rich_rules.extend(root_names(&room, "ChatSyncGet"));
    let rich = key(&room.world, room.ada.id(), rich_rules).await;
    let narrow = key(&room.world, room.ada.id(), vec![primary.clone()]).await;
    let wrong = key(
        &room.world,
        room.ada.id(),
        vec![rule("ChatGet", "chat", room.chat)],
    )
    .await;
    let server = serve(&room).await;
    let mut denied = Client::signed_in(server.addr, &wrong.secret).await;
    denied.sub(1, &format!("chat:{}", room.chat), Some(0)).await;
    assert_eq!(
        denied.expect("error").await["code"],
        "forbidden",
        "ChatGet alone never admits the existing ChatSyncGet websocket reader"
    );
    let head = room.chat(&room.ada, room.chat).await.seq;
    let mut wide = follow(&server, &rich.secret, 2, room.chat, head).await;
    let mut small = follow(&server, &narrow.secret, 3, room.chat, head).await;
    let literal = "Stored literal: Live socket crew, Acme Web Login.";
    let body = post(vec![
        text(literal),
        mention(group.clone()),
        mention(role.clone()),
    ]);
    let sent = send(&room.world, &room.ada.token, room.chat, &body).await;
    let wide_frame = next_sync(&mut wide).await;
    let small_frame = next_sync(&mut small).await;
    assert_eq!(wide_frame["seq"], small_frame["seq"]);
    let shown = frame_message(&wide_frame, sent.id);
    let hidden = frame_message(&small_frame, sent.id);
    assert_eq!(
        shown, sent,
        "sufficient read sources preserve the ordinary producer's message"
    );
    assert!(label(&shown, &group).contains("Live socket crew"));
    assert!(label(&shown, &role).contains("Login"));
    assert_eq!(label(&hidden, &group), "a group");
    assert_eq!(label(&hidden, &role), "Developers of a private place");
    // The ordinary producer renders mentions as adjacent Text runs too. Only the
    // stored literal stays identical; the live labels deliberately differ above.
    assert!(Room::words(&hidden).starts_with(literal));
    assert!(Room::words(&shown).starts_with(literal));
    assert_eq!(
        hidden.author, shown.author,
        "public human author identity remains shared chat material"
    );
    assert_eq!(hidden.mentions, shown.mentions);

    // A changed key invalidates its old projection through the existing reload protocol.
    // The peer's unchanged key still receives the triggering message normally.
    narrow_key(&room.world, &rich, vec![primary.clone()]).await;
    let mut during_reload = body.clone();
    during_reload.client_id = new_id().to_string();
    let pending = send(&room.world, &room.ada.token, room.chat, &during_reload).await;
    let reset = wide.expect("resync").await;
    assert_eq!(reset["reload"], true);
    assert_eq!(reset["id"], 2);
    let peer = next_sync(&mut small).await;
    assert_eq!(frame_message(&peer, pending.id).id, pending.id);
    let refreshed = sync(&room.world, &rich.secret, room.chat, Default::default())
        .await
        .ok();
    assert_eq!(refreshed.seq, peer["seq"].as_i64().unwrap());
    let reloaded = refreshed
        .messages
        .iter()
        .find(|row| row.id == pending.id)
        .unwrap();
    assert_eq!(label(reloaded, &group), "a group");
    assert_eq!(label(reloaded, &role), "Developers of a private place");
    assert!(Room::words(reloaded).starts_with(literal));
    // The same authenticated connection keeps its original accepted key ceiling. Remove the
    // old paused subscription after replacing its whole view, then follow the fresh HTTP cut.
    wide.send(json!({"op":"unsub","id":2})).await;
    assert_eq!(wide.expect("closed").await["reason"], "unsubscribed");
    wide.sub(2, &format!("chat:{}", room.chat), Some(refreshed.seq))
        .await;
    let ack = wide.expect("subscribed").await;
    assert_eq!(ack["mode"], "delta");
    assert_eq!(ack["seq"], refreshed.seq);
    wide.expect("presence").await;

    let mut again = body.clone();
    again.client_id = new_id().to_string();
    let sent = send(&room.world, &room.ada.token, room.chat, &again).await;
    let wide_frame = next_sync(&mut wide).await;
    let small_frame = next_sync(&mut small).await;
    let projected = frame_message(&wide_frame, sent.id);
    assert_eq!(label(&projected, &group), "a group");
    assert_eq!(label(&projected, &role), "Developers of a private place");
    assert_eq!(
        projected,
        frame_message(&small_frame, sent.id),
        "current narrowed source bounds replace the old rich projection after reload"
    );

    narrow_key(&room.world, &rich, vec![rule("ChatGet", "chat", room.chat)]).await;
    let final_message = room
        .say(
            &room.ada,
            room.chat,
            "Only the still-admitted Sync key receives this",
        )
        .await;
    let closed = wide.until("closed").await;
    assert_eq!(closed.last().unwrap()["reason"], "revoked");
    assert!(closed.iter().all(|frame| {
        frame["op"] != "sync"
            || !frame["data"]["messages"]
                .as_array()
                .is_some_and(|messages| {
                    messages
                        .iter()
                        .any(|message| message["id"] == json!(final_message.id))
                })
    }));
    assert_eq!(
        frame_message(&next_sync(&mut small).await, final_message.id).id,
        final_message.id
    );
    drop((wide, small, denied));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inspection_group_scope_is_rechecked_only_when_the_socket_materializes_the_chat_dto() {
    let room = Room::new().await;
    let inspectors = group(&room, "Socket inspectors").await;
    room.world.app.call(PermissionPageUpdate,&[&"nodes",&room.root]).token(&room.ada.token)
        .raw_body(&json!({"rows":[{"permission":"directs.view","add":[{"holder":{"kind":"group","id":inspectors}}]}]}))
        .send().await.full_permission_page().ok();
    let chat = room
        .group(&room.bea, &[&room.cai], Some("Visible group name"))
        .await;
    let ordinary = room.chat(&room.bea, chat.base.id).await;
    assert!(
        ordinary
            .inspection_holders
            .as_ref()
            .unwrap()
            .iter()
            .any(|holder| matches!(holder,Holder::Group{group} if group.id==inspectors))
    );
    let primary = rule("ChatSyncGet", "chat", chat.base.id);
    let token = key(&room.world, room.bea.id(), vec![primary]).await;
    let server = serve(&room).await;
    let mut reader = follow(&server, &token.secret, 1, chat.base.id, ordinary.seq).await;
    // This initial key has no Group source read. The ordinary session supplied its prior
    // cut; message-only delivery itself does not materialize the inspection-holder DTO.
    let sent = room
        .say(
            &room.bea,
            chat.base.id,
            "This message-only delta does not reread its root inspection holders",
        )
        .await;
    let frame = next_sync(&mut reader).await;
    assert!(
        frame["data"].get("chat").is_none(),
        "this is the existing message-only projection"
    );
    assert_eq!(frame_message(&frame, sent.id).id, sent.id);
    room.world
        .app
        .call(ChatUpdate, &[&chat.base.id])
        .token(&room.bea.token)
        .body(&ChatUpdateBody {
            visibility: None,
            home_id: None,
            name: Some(Some(
                "Renamed while inspection source is unavailable".into(),
            )),
        })
        .send()
        .await
        .ok();
    let closed = reader.until("closed").await;
    assert_eq!(closed.last().unwrap()["reason"], "revoked");
    assert!(
        closed
            .iter()
            .all(|frame| frame["op"] != "sync" || frame["data"].get("chat").is_none()),
        "a required Group source cannot be omitted or replaced with a fake empty holder DTO"
    );
    drop(reader);
    server.stop().await;
}
