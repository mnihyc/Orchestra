//! Scoped message mutations preserve their ordinary writer and bound response content.
//!
//! Existing chats_node controls cover archive, expiry, emoji limits and closed chat policy;
//! chats_threads covers delete's quote/chat lock ordering. These cases isolate key scopes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use chat_world::{Room, text};
use orch_api_contract::access::Role;
use orch_api_contract::chats::{
    ChatMentionTarget, ChatMessage, ChatMessageDelete, ChatMessagePinDelete, ChatMessagePinSet,
    ChatMessageReactionAdd, ChatMessageReactionRemove, ChatPart,
};
use orch_api_contract::directory::{GroupMemberAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_types::{Id, new_id};
use serde_json::{Value, json};

#[derive(Clone, Copy)]
enum Mutation {
    Delete,
    ReactionAdd,
    ReactionRemove,
    PinSet,
    PinDelete,
}

impl Mutation {
    fn operation(self) -> &'static str {
        match self {
            Self::Delete => "ChatMessageDelete.own",
            Self::ReactionAdd => "ChatMessageReactionAdd",
            Self::ReactionRemove => "ChatMessageReactionRemove",
            Self::PinSet => "ChatMessagePinSet",
            Self::PinDelete => "ChatMessagePinDelete",
        }
    }
}

fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},
        "allow":[{"operation":operation,"actions":actions}]})
}

/// A real short-lived stored token in rule mode; no legacy permission fallback.
async fn key(room: &Room, user: Id, rules: Vec<Value>) -> String {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let stored = serde_json::to_value(scope).unwrap();
    let secret = new_token(API_TOKEN_PREFIX);
    create_api_token_scoped(
        &mut *room.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Message mutation scope control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: room.world.now(),
            // These fixed-clock request sequences fit the existing one-hour test lifetime.
            expires_at: Some(room.world.now() + chrono::Duration::hours(1)),
        },
        Some(&stored),
    )
    .await
    .unwrap();
    secret
}

/// Raw replies compile against both the old full DTO and the proposed typed union.
async fn mutate(
    room: &Room,
    token: &str,
    chat: Id,
    message: Id,
    mutation: Mutation,
) -> (u16, Value) {
    let emoji = "%F0%9F%91%8D";
    let reply = match mutation {
        Mutation::Delete => {
            room.world
                .app
                .call(ChatMessageDelete, &[&chat, &message])
                .token(token)
                .send_bytes()
                .await
        }
        Mutation::ReactionAdd => {
            room.world
                .app
                .call(ChatMessageReactionAdd, &[&chat, &message, &emoji])
                .token(token)
                .send_bytes()
                .await
        }
        Mutation::ReactionRemove => {
            room.world
                .app
                .call(ChatMessageReactionRemove, &[&chat, &message, &emoji])
                .token(token)
                .send_bytes()
                .await
        }
        Mutation::PinSet => {
            room.world
                .app
                .call(ChatMessagePinSet, &[&chat, &message])
                .token(token)
                .send_bytes()
                .await
        }
        Mutation::PinDelete => {
            room.world
                .app
                .call(ChatMessagePinDelete, &[&chat, &message])
                .token(token)
                .send_bytes()
                .await
        }
    };
    (
        reply.status.as_u16(),
        serde_json::from_slice(&reply.raw).unwrap(),
    )
}

fn full(reply: (u16, Value), chat: Id, message: Id) -> Value {
    let (status, shown) = reply;
    assert_eq!(status, 200, "{shown}");
    assert_eq!(
        shown["id"],
        json!(message),
        "the ordinary full wire remains a ChatMessage"
    );
    assert_eq!(shown["chat_id"], json!(chat));
    assert!(shown.get("parts").is_some(), "{shown}");
    assert!(shown.get("projection").is_none(), "{shown}");
    shown
}

fn receipt(reply: (u16, Value), chat: Id, message: Id) -> i64 {
    let (status, shown) = reply;
    assert_eq!(status, 200, "{shown}");
    let seq = shown["seq"]
        .as_i64()
        .expect("a receipt has the accepted chat position");
    assert_eq!(
        shown,
        json!({"projection":"receipt","chat_id":chat,
        "message_id":message,"seq":seq}),
        "no message history, live names, reactions or pin enters an acknowledgement"
    );
    seq
}

async fn current(room: &Room, message: Id) -> ChatMessage {
    room.sync(&room.ada, room.chat)
        .await
        .messages
        .into_iter()
        .find(|row| row.id == message)
        .expect("the ordinary reader still sees the affected row")
}

async fn ordinary_idempotent(room: &Room, message: Id, mutation: Mutation) -> Value {
    let first = full(
        mutate(room, &room.ada.token, room.chat, message, mutation).await,
        room.chat,
        message,
    );
    let again = full(
        mutate(room, &room.ada.token, room.chat, message, mutation).await,
        room.chat,
        message,
    );
    assert_eq!(
        again["seq_updated"], first["seq_updated"],
        "the ordinary duplicate changes no message revision"
    );
    again
}

#[tokio::test]
async fn reaction_add_write_only_acknowledges_the_real_idempotent_writer() {
    let room = Room::new().await;
    let message = room
        .say(&room.ada, room.chat, "React to this stored message")
        .await;
    let ordinary = ordinary_idempotent(&room, message.id, Mutation::ReactionAdd).await;
    assert_eq!(ordinary["reactions"][0]["emoji"], "👍");
    assert_eq!(ordinary["reactions"][0]["users"], json!([room.ada.id()]));
    full(
        mutate(
            &room,
            &room.ada.token,
            room.chat,
            message.id,
            Mutation::ReactionRemove,
        )
        .await,
        room.chat,
        message.id,
    );
    let token = key(
        &room,
        room.ada.id(),
        vec![rule(
            Mutation::ReactionAdd.operation(),
            "chat",
            room.chat,
            &["write"],
        )],
    )
    .await;
    let accepted = receipt(
        mutate(&room, &token, room.chat, message.id, Mutation::ReactionAdd).await,
        room.chat,
        message.id,
    );
    let read = current(&room, message.id).await;
    assert_eq!(read.reactions.len(), 1);
    assert_eq!(read.reactions[0].users, [room.ada.id()]);
    assert_eq!(read.reactions[0].emoji, "👍");
    assert_eq!(
        receipt(
            mutate(&room, &token, room.chat, message.id, Mutation::ReactionAdd).await,
            room.chat,
            message.id
        ),
        accepted
    );
}

#[tokio::test]
async fn reaction_remove_write_only_acknowledges_only_the_callers_reaction() {
    let room = Room::new().await;
    let message = room
        .say(&room.ada, room.chat, "Keep other people's reactions")
        .await;
    full(
        mutate(
            &room,
            &room.ada.token,
            room.chat,
            message.id,
            Mutation::ReactionAdd,
        )
        .await,
        room.chat,
        message.id,
    );
    let ordinary = ordinary_idempotent(&room, message.id, Mutation::ReactionRemove).await;
    assert_eq!(ordinary["reactions"], json!([]));
    full(
        mutate(
            &room,
            &room.ada.token,
            room.chat,
            message.id,
            Mutation::ReactionAdd,
        )
        .await,
        room.chat,
        message.id,
    );
    full(
        mutate(
            &room,
            &room.bea.token,
            room.chat,
            message.id,
            Mutation::ReactionAdd,
        )
        .await,
        room.chat,
        message.id,
    );
    let token = key(
        &room,
        room.ada.id(),
        vec![rule(
            Mutation::ReactionRemove.operation(),
            "chat",
            room.chat,
            &["write"],
        )],
    )
    .await;
    let accepted = receipt(
        mutate(
            &room,
            &token,
            room.chat,
            message.id,
            Mutation::ReactionRemove,
        )
        .await,
        room.chat,
        message.id,
    );
    let read = current(&room, message.id).await;
    assert_eq!(read.reactions.len(), 1);
    assert_eq!(read.reactions[0].users, [room.bea.id()]);
    assert_eq!(
        receipt(
            mutate(
                &room,
                &token,
                room.chat,
                message.id,
                Mutation::ReactionRemove
            )
            .await,
            room.chat,
            message.id
        ),
        accepted
    );
}

#[tokio::test]
async fn pin_set_write_only_acknowledges_the_real_idempotent_writer() {
    let room = Room::new().await;
    let message = room
        .say(&room.ada, room.chat, "Pin this stored message")
        .await;
    let ordinary = ordinary_idempotent(&room, message.id, Mutation::PinSet).await;
    assert_eq!(ordinary["pinned"]["by"]["id"], json!(room.ada.id()));
    full(
        mutate(
            &room,
            &room.ada.token,
            room.chat,
            message.id,
            Mutation::PinDelete,
        )
        .await,
        room.chat,
        message.id,
    );
    let token = key(
        &room,
        room.ada.id(),
        vec![rule(
            Mutation::PinSet.operation(),
            "chat",
            room.chat,
            &["write"],
        )],
    )
    .await;
    let accepted = receipt(
        mutate(&room, &token, room.chat, message.id, Mutation::PinSet).await,
        room.chat,
        message.id,
    );
    assert_eq!(
        current(&room, message.id).await.pinned.unwrap().by.id,
        room.ada.id()
    );
    assert_eq!(
        receipt(
            mutate(&room, &token, room.chat, message.id, Mutation::PinSet).await,
            room.chat,
            message.id
        ),
        accepted
    );
}

#[tokio::test]
async fn pin_delete_write_only_can_remove_another_senders_pin() {
    let room = Room::new().await;
    let message = room
        .say(&room.ada, room.chat, "A pin belongs to the shared chat")
        .await;
    full(
        mutate(
            &room,
            &room.bea.token,
            room.chat,
            message.id,
            Mutation::PinSet,
        )
        .await,
        room.chat,
        message.id,
    );
    let ordinary = ordinary_idempotent(&room, message.id, Mutation::PinDelete).await;
    assert!(ordinary["pinned"].is_null());
    full(
        mutate(
            &room,
            &room.bea.token,
            room.chat,
            message.id,
            Mutation::PinSet,
        )
        .await,
        room.chat,
        message.id,
    );
    let token = key(
        &room,
        room.ada.id(),
        vec![rule(
            Mutation::PinDelete.operation(),
            "chat",
            room.chat,
            &["write"],
        )],
    )
    .await;
    let accepted = receipt(
        mutate(&room, &token, room.chat, message.id, Mutation::PinDelete).await,
        room.chat,
        message.id,
    );
    assert!(current(&room, message.id).await.pinned.is_none());
    assert_eq!(
        receipt(
            mutate(&room, &token, room.chat, message.id, Mutation::PinDelete).await,
            room.chat,
            message.id
        ),
        accepted
    );
}

#[tokio::test]
async fn delete_selects_stored_author_or_moderator_without_crossing_chat_identity() {
    let room = Room::new().await;
    let own = room
        .say(&room.ada, room.chat, "Ordinary author deletion")
        .await;
    let first = full(
        mutate(&room, &room.ada.token, room.chat, own.id, Mutation::Delete).await,
        room.chat,
        own.id,
    );
    assert_eq!(first["deleted"], true);
    // A tombstone is returned before the ordinary author/moderator write decision.
    let again = full(
        mutate(&room, &room.bea.token, room.chat, own.id, Mutation::Delete).await,
        room.chat,
        own.id,
    );
    assert_eq!(again["seq_updated"], first["seq_updated"]);
    let moderated = room
        .say(&room.bea, room.chat, "Ordinary moderator deletion")
        .await;
    let gone = full(
        mutate(
            &room,
            &room.cai.token,
            room.chat,
            moderated.id,
            Mutation::Delete,
        )
        .await,
        room.chat,
        moderated.id,
    );
    assert_eq!(gone["deleted"], true);
    let own = room
        .say(&room.ada, room.chat, "Scoped author deletion")
        .await;
    let own_token = key(
        &room,
        room.ada.id(),
        vec![rule("ChatMessageDelete.own", "chat", room.chat, &["write"])],
    )
    .await;
    receipt(
        mutate(&room, &own_token, room.chat, own.id, Mutation::Delete).await,
        room.chat,
        own.id,
    );
    assert!(current(&room, own.id).await.deleted);
    let other = room
        .say(&room.bea, room.chat, "Write does not lend moderation")
        .await;
    let (status, refused) = mutate(&room, &own_token, room.chat, other.id, Mutation::Delete).await;
    assert_eq!(status, 403, "{refused}");
    assert!(!current(&room, other.id).await.deleted);
    let moderate_token = key(
        &room,
        room.ada.id(),
        vec![rule(
            "ChatMessageDelete.moderate",
            "chat",
            room.chat,
            &["manage"],
        )],
    )
    .await;
    receipt(
        mutate(
            &room,
            &moderate_token,
            room.chat,
            other.id,
            Mutation::Delete,
        )
        .await,
        room.chat,
        other.id,
    );
    assert!(current(&room, other.id).await.deleted);
    let elsewhere = room.direct(&room.ada, &room.bea).await.base.id;
    let foreign = room
        .say(&room.ada, elsewhere, "Belongs to another chat")
        .await;
    let (status, refused) =
        mutate(&room, &own_token, room.chat, foreign.id, Mutation::Delete).await;
    assert_eq!(status, 404, "{refused}");
    let row =
        orch_db::queries::chats::message(&mut *room.world.conn().await, elsewhere, foreign.id)
            .await
            .unwrap()
            .unwrap();
    assert!(
        !row.is_deleted(),
        "a mismatched message path writes nothing to either chat"
    );
}

fn mention_label<'a>(message: &'a Value, target: &ChatMentionTarget) -> &'a str {
    message["mention_names"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["target"] == json!(target))
        .unwrap()["name"]
        .as_str()
        .unwrap()
}

#[tokio::test]
async fn mutation_read_is_same_operation_and_live_group_role_labels_remain_bounded() {
    let room = Room::new().await;
    let group_name = "Protected mutation crew";
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
    let literal = format!("Literal already shared: {group_name}. ");
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
    let message = room
        .send(&room.ada, room.chat, parts, &new_id().to_string())
        .await
        .ok()
        .message;
    let ordinary = full(
        mutate(
            &room,
            &room.ada.token,
            room.chat,
            message.id,
            Mutation::PinSet,
        )
        .await,
        room.chat,
        message.id,
    );
    assert!(mention_label(&ordinary, &group_target).contains(group_name));
    assert!(mention_label(&ordinary, &role_target).contains("Login"));
    let stored: Value = sqlx::query_scalar("SELECT content FROM chat_messages WHERE id=$1")
        .bind(message.id)
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
    let operation = Mutation::PinSet.operation();
    let unrelated = key(
        &room,
        room.ada.id(),
        vec![
            rule(operation, "chat", room.chat, &["write"]),
            rule("ChatGet", "chat", room.chat, &["read"]),
        ],
    )
    .await;
    receipt(
        mutate(&room, &unrelated, room.chat, message.id, Mutation::PinSet).await,
        room.chat,
        message.id,
    );
    let bounded = key(
        &room,
        room.ada.id(),
        vec![rule(operation, "chat", room.chat, &["write", "read"])],
    )
    .await;
    let hidden = full(
        mutate(&room, &bounded, room.chat, message.id, Mutation::PinSet).await,
        room.chat,
        message.id,
    );
    assert_eq!(mention_label(&hidden, &group_target), "a group");
    assert_eq!(
        mention_label(&hidden, &role_target),
        "Developers of a private place"
    );
    assert!(
        hidden["parts"].to_string().contains(&literal),
        "literal stored text is not a name lookup to scrub"
    );
    let allowed = key(
        &room,
        room.ada.id(),
        vec![
            rule(operation, "chat", room.chat, &["write", "read"]),
            rule(operation, "group", group.id, &["read"]),
            rule(operation, "topic", room.topic, &["read"]),
            rule(operation, "project", room.project, &["read"]),
            rule(operation, "org", room.root, &["read"]),
        ],
    )
    .await;
    let shown = full(
        mutate(&room, &allowed, room.chat, message.id, Mutation::PinSet).await,
        room.chat,
        message.id,
    );
    assert_eq!(shown["mention_names"], ordinary["mention_names"]);
    assert_eq!(shown["parts"], ordinary["parts"]);
    let after: Value = sqlx::query_scalar("SELECT content FROM chat_messages WHERE id=$1")
        .bind(message.id)
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        after, stored,
        "response name projection never rewrites stored message material"
    );
}

#[tokio::test]
async fn delete_refuses_the_unreadable_chat_before_disclosing_whether_its_message_exists() {
    let room = Room::new().await;
    let kept = room
        .say(&room.ada, room.chat, "A private message existence control")
        .await;
    let absent = new_id();
    // The native writer first loads this node chat. A missing message must not
    // turn the same caller's ordinary ChatView refusal into a distinct 404.
    for message in [kept.id, absent] {
        let (status, problem) =
            mutate(&room, &room.eve.token, room.chat, message, Mutation::Delete).await;
        assert_eq!(status, 403, "ordinary chat refusal: {problem}");
        assert_eq!(problem["code"], "authz.denied");
        assert_eq!(problem["required"], "chat.view");
    }
    let token = key(
        &room,
        room.eve.id(),
        vec![
            rule("ChatMessageDelete.own", "chat", room.chat, &["write"]),
            rule("ChatMessageDelete.moderate", "chat", room.chat, &["manage"]),
        ],
    )
    .await;
    for message in [kept.id, absent] {
        let (status, problem) = mutate(&room, &token, room.chat, message, Mutation::Delete).await;
        assert_eq!(status, 403, "the key grants no ordinary reading: {problem}");
        assert_eq!(problem["code"], "authz.denied");
        assert!(
            !problem
                .to_string()
                .contains("A private message existence control")
        );
    }
    let reader = key(
        &room,
        room.eve.id(),
        vec![
            rule(
                "ChatMessageDelete.own",
                "chat",
                room.chat,
                &["write", "read"],
            ),
            rule(
                "ChatMessageDelete.moderate",
                "chat",
                room.chat,
                &["manage", "read"],
            ),
        ],
    )
    .await;
    for message in [kept.id, absent] {
        let (status, problem) = mutate(&room, &reader, room.chat, message, Mutation::Delete).await;
        assert_eq!(
            status, 403,
            "a Read scope grants no account privilege: {problem}"
        );
        assert_eq!(problem["code"], "authz.denied");
        for label in ["Login", "Web", "Acme"] {
            assert!(
                !problem.to_string().contains(label),
                "a refusal must not disclose an ungranted live place label: {problem}"
            );
        }
    }
    assert!(!current(&room, kept.id).await.deleted);
}

#[tokio::test]
async fn a_delete_refusal_uses_the_same_tokens_current_mode_after_waiting_for_its_identity() {
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token};
    use tower::ServiceExt;

    // Reuse the established provider-creation identity barrier's bounded wait. Polling
    // yields instead of adding a timing threshold; completion before the lock is a failure.
    const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
    let room = Room::new().await;
    // The legacy account may know this exact place's name while still lacking
    // ChatView. Its later Chat-scoped key receives no Topic Read name grant.
    orch_db::queries::access::testing::add_entry(
        &mut *room.world.conn().await,
        &orch_db::queries::access::PageKey::new("node", room.topic, "live"),
        "any",
        "topic.view",
        ("user", Some(room.eve.id()), None),
        None,
        room.world.now(),
    )
    .await
    .unwrap();
    let kept = room
        .say(
            &room.ada,
            room.chat,
            "The identity wait must not delete this message",
        )
        .await;
    let token = new_token(API_TOKEN_PREFIX);
    let token_id = new_id();
    create_api_token(
        &mut *room.world.conn().await,
        &NewApiToken {
            id: token_id,
            user_id: room.eve.id(),
            name: "Converting chat mutation credential",
            token_hash: &token_hash(&token),
            scopes: &["*".to_owned()],
            node_id: None,
            at: room.world.now(),
            expires_at: None,
        },
    )
    .await
    .unwrap();
    let (status, legacy) = mutate(&room, &token, room.chat, kept.id, Mutation::Delete).await;
    assert_eq!(status, 403, "untouched legacy premise: {legacy}");
    assert_eq!(legacy["code"], "authz.denied");
    assert_eq!(
        legacy["required"], "chat.view",
        "the fixture grants TopicView only, never ChatView"
    );
    assert!(
        legacy.to_string().contains("Login"),
        "the actual legacy refusal is rich before the same token changes mode: {legacy}"
    );

    let mut blocker = room.world.db.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_person(&mut blocker, room.eve.id())
        .await
        .unwrap()
        .unwrap();
    let locked: Id =
        sqlx::query_scalar("SELECT id FROM api_tokens WHERE id=$1 AND user_id=$2 FOR UPDATE")
            .bind(token_id)
            .bind(room.eve.id())
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
    assert_eq!(locked, token_id);
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(room.world.app.app.clone());
    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/chats/{}/messages/{}", room.chat, kept.id))
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let mut action = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    let observed = tokio::time::timeout(WAIT, async {
        loop {
            if action.is_finished() {
                break false;
            }
            // This test owns its database and the only request using this locked token.
            // Match its actual blocker, independent of which current-identity SQL is used.
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))",
            )
            .bind(blocker_pid)
            .fetch_one(&room.world.db.pool)
            .await
            .unwrap();
            if waiting {
                break true;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or(false);
    if !observed {
        action.abort();
        let _ = action.await;
        blocker.rollback().await.unwrap();
        assert!(
            observed,
            "the same request must wait at its current-identity boundary before constructing a refusal"
        );
        return;
    }
    let scope = json!({"rules":[
        rule("ChatMessageDelete.own", "chat", room.chat, &["write", "read"]),
        rule("ChatMessageDelete.moderate", "chat", room.chat, &["manage", "read"]),
    ]});
    let parsed: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(scope.clone()).unwrap();
    assert!(parsed.validate().is_empty());
    sqlx::query("UPDATE api_tokens SET scopes='{}'::text[],operation_scope=$2 WHERE id=$1")
        .bind(token_id)
        .bind(scope)
        .execute(&mut *blocker)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let result = tokio::time::timeout(WAIT, &mut action).await;
    if result.is_err() {
        action.abort();
        let _ = action.await;
    }
    let response = result.expect("released chat deletion completes").unwrap();
    assert_eq!(response.status(), http::StatusCode::FORBIDDEN);
    let raw = axum::body::to_bytes(
        response.into_body(),
        orch_api_contract::common::MAX_JSON_BYTES,
    )
    .await
    .unwrap();
    let problem: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(problem["code"], "authz.denied");
    for name in ["Login", "Web", "Acme"] {
        assert!(
            !String::from_utf8_lossy(&raw).contains(name),
            "the token's current mode must bound error names after its wait: {problem}"
        );
    }
    assert!(!current(&room, kept.id).await.deleted);
}
