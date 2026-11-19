//! One chat post retains all of its actual source/destination scopes, including agent inputs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::chats::{
    ChatMentionPreview, ChatMentionPreviewBody, ChatMentionPreviewReply, ChatMentionTarget,
    ChatMessageCreate, ChatMessageCreateBody, ChatMessageCreateResponse, ChatPart,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::{ContentPart, ForwardedAuthor};
use orch_api_core::testing::Reply;
use orch_types::{Id, new_id};
use rig::Scene;
use serde_json::{Value, json};

fn text(words: &str) -> ChatPart {
    ChatPart::Text { text: words.into() }
}

fn agent(thread: Id) -> ChatPart {
    ChatPart::Mention {
        target: ChatMentionTarget::Agent { thread_id: thread },
    }
}

fn post(parts: Vec<ChatPart>) -> ChatMessageCreateBody {
    ChatMessageCreateBody {
        parts,
        reply_to: None,
        agent_review: None,
        client_id: new_id().to_string(),
    }
}

/// Every effect uses this same stable operation ID; each rule binds its actions to its own object.
fn rule(kind: &str, id: Id, actions: &[&str]) -> Value {
    json!({
        "target":{"kind":kind,"id":id,"reach":"self"},
        "allow":[{"operation":"ChatMessageCreate","actions":actions}]
    })
}

async fn key(s: &Scene, rules: Vec<Value>) -> String {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let stored = serde_json::to_value(scope).unwrap();
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let now = s.rig.world.now();
    let mut conn = s.rig.world.conn().await;
    create_api_token_scoped(
        &mut conn,
        &NewApiToken {
            id: new_id(),
            user_id: s.ada.id(),
            name: "Chat effect control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: now,
            // This short fixed-clock request sequence fits the ordinary place lifetime.
            expires_at: Some(now + chrono::Duration::hours(1)),
        },
        Some(&stored),
    )
    .await
    .unwrap();
    secret
}

async fn held_thread(s: &Scene, name: &str) -> orch_api_contract::threads::Thread {
    let thread = s.thread_by(&s.ada, name).await;
    assert!(
        thread.input_access.allowed,
        "ordinary user/model authority is positive"
    );
    // Existing Stop state keeps the normal input producer queued without disabling its model.
    sqlx::query("UPDATE threads SET queue_held=true WHERE node_id=$1")
        .bind(thread.base.id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    thread
}

async fn send(
    s: &Scene,
    token: &str,
    chat: Id,
    body: &ChatMessageCreateBody,
) -> Reply<ChatMessageCreateResponse> {
    s.rig
        .world
        .app
        .call(ChatMessageCreate, &[&chat])
        .token(token)
        .body(body)
        .send()
        .await
}

/// The normal preview executes and rolls back the exact ordinary forwarding producer. A successful
/// review proves the same user's chat, target-thread, model and provider prerequisites before key cuts.
async fn reviewed(
    s: &Scene,
    chat: Id,
    target: Id,
    reply: Option<Id>,
) -> (ChatMessageCreateBody, ChatMentionPreviewReply) {
    let mut body = post(vec![text("Please examine this"), agent(target)]);
    body.reply_to = reply;
    let preview = s
        .rig
        .world
        .app
        .call(ChatMentionPreview, &[&chat])
        .token(&s.ada.token)
        .body(&ChatMentionPreviewBody {
            parts: body.parts.clone(),
            reply_to: reply,
        })
        .send()
        .await
        .ok();
    assert_eq!(preview.agent_asks.len(), 1);
    assert_eq!(preview.agent_asks[0].thread.id, target);
    assert_eq!(preview.agent_asks[0].model.as_deref(), Some(rig::MODEL));
    assert_eq!(
        preview.agent_asks[0].input.author.as_ref().map(|by| by.id),
        Some(s.ada.id())
    );
    body.agent_review = preview.agent_review;
    assert!(body.agent_review.is_some());
    (body, preview)
}

async fn counts(s: &Scene, chat: Id, target: Id) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM chat_messages WHERE chat_id=$1), (SELECT count(*) FROM thread_inputs), (SELECT count(*) FROM thread_inputs WHERE thread_id=$2)")
        .bind(chat).bind(target).fetch_one(s.rig.db.pool()).await.unwrap()
}

fn receipt(reply: Reply<ChatMessageCreateResponse>, chat: Id, status: u16) -> Id {
    assert_eq!(reply.status.as_u16(), status, "{:?}", reply.body);
    let raw: Value = serde_json::from_slice(&reply.raw).unwrap();
    let ChatMessageCreateResponse::Receipt(row) = reply.ok() else {
        panic!("this key was not granted chat response-content Read");
    };
    assert_eq!(row.chat_id, chat);
    assert_eq!(
        raw,
        json!({"projection":"receipt","chat_id":chat,"message_id":row.message_id,"seq":row.seq})
    );
    row.message_id
}

async fn input_content(s: &Scene, thread: Id) -> (Id, Vec<ContentPart>) {
    let (author, content): (Id, Value) = sqlx::query_as(
        "SELECT author_id,content FROM thread_inputs WHERE thread_id=$1 ORDER BY created_at,id",
    )
    .bind(thread)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    (author, serde_json::from_value(content).unwrap())
}

#[tokio::test]
async fn an_agent_ask_needs_target_thread_write_and_commits_message_and_input_together() {
    let s = Scene::new().await;
    let target = held_thread(&s, "Ask target").await.base.id;
    let chat = s.tree.topic.base.chat_id;
    let (body, _) = reviewed(&s, chat, target, None).await;
    let before = counts(&s, chat, target).await;
    let narrow = key(
        &s,
        vec![
            rule("chat", chat, &["write", "read"]),
            // Target Read permits copied/displayed target labels; only target Write is missing.
            rule("thread", target, &["read"]),
        ],
    )
    .await;
    let denied = send(&s, &narrow, chat, &body).await.problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    assert_eq!(
        counts(&s, chat, target).await,
        before,
        "no partial chat post or input survives"
    );
    let full = key(
        &s,
        vec![
            rule("chat", chat, &["write", "read"]),
            rule("thread", target, &["write", "read"]),
        ],
    )
    .await;
    let accepted = send(&s, &full, chat, &body).await;
    assert_eq!(accepted.status, 201, "{:?}", accepted.body);
    let accepted = accepted
        .ok()
        .into_full()
        .expect("all response Read facets are granted")
        .message;
    assert_eq!(accepted.asked_agents.len(), 1);
    assert_eq!(accepted.asked_agents[0].thread.id, target);
    assert_eq!(
        counts(&s, chat, target).await,
        (before.0 + 1, before.1 + 1, before.2 + 1)
    );
    let (author, content) = input_content(&s, target).await;
    assert_eq!(author, s.ada.id());
    assert!(content.iter().any(|part|matches!(part,ContentPart::Forwarded(copy) if copy.text.contains("Please examine this"))));
    let repeated = send(&s, &full, chat, &body).await;
    assert_eq!(repeated.status, 200);
    assert_eq!(repeated.ok().into_full().unwrap().message.id, accepted.id);
    assert_eq!(
        counts(&s, chat, target).await,
        (before.0 + 1, before.1 + 1, before.2 + 1)
    );
    assert!(s.rig.turn_starts(target).is_empty());
    s.rig.engine.stop();
}

#[tokio::test]
async fn an_agent_reply_ask_cannot_copy_chat_source_without_this_operations_read() {
    let s = Scene::new().await;
    let target = held_thread(&s, "Reply target").await.base.id;
    let chat = s.tree.topic.base.chat_id;
    let original = send(
        &s,
        &s.eve.token,
        chat,
        &post(vec![text("Original material written by another person")]),
    )
    .await
    .ok()
    .into_full()
    .unwrap()
    .message;
    let (body, preview) = reviewed(&s, chat, target, Some(original.id)).await;
    let ContentPart::Forwarded(source) = &preview.agent_asks[0].input.content[0] else {
        panic!("the real producer forwards the original reply first")
    };
    assert_eq!(source.text, "Original material written by another person");
    assert!(matches!(&source.author,Some(ForwardedAuthor::User(by)) if by.id==s.eve.id()));
    let before = counts(&s, chat, target).await;
    let narrow = key(
        &s,
        vec![
            rule("chat", chat, &["write"]),
            rule("thread", target, &["write", "read"]),
        ],
    )
    .await;
    assert_eq!(
        send(&s, &narrow, chat, &body).await.problem().code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(counts(&s, chat, target).await, before);
    let full = key(
        &s,
        vec![
            rule("chat", chat, &["write", "read"]),
            rule("thread", target, &["write", "read"]),
        ],
    )
    .await;
    assert_eq!(send(&s, &full, chat, &body).await.status, 201);
    let (_, content) = input_content(&s, target).await;
    let ContentPart::Forwarded(source) = &content[0] else {
        panic!("the accepted input retains the original source")
    };
    assert_eq!(source.text, "Original material written by another person");
    assert!(matches!(&source.author,Some(ForwardedAuthor::User(by)) if by.id==s.eve.id()));
    assert_eq!(
        counts(&s, chat, target).await,
        (before.0 + 1, before.1 + 1, before.2 + 1)
    );
    assert!(s.rig.turn_starts(target).is_empty());
    s.rig.engine.stop();
}

#[tokio::test]
async fn an_item_quote_needs_source_thread_read_but_not_thread_write() {
    let s = Scene::new().await;
    let thread = s.thread().await;
    let id = thread.base.id;
    let chat = thread.base.chat_id;
    // Existing thread/chat fixture path: a real scripted exchange supplies the item to quote.
    s.exchange(id, 0, "Explain this", "A source item to quote")
        .await;
    let item:Id=sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND kind='agent_message' ORDER BY seq_created LIMIT 1")
        .bind(id).fetch_one(s.rig.db.pool()).await.unwrap();
    let before = counts(&s, chat, id).await;
    let refs_before: i32 = sqlx::query_scalar("SELECT chat_refs FROM items WHERE id=$1")
        .bind(item)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let starts = s.rig.turn_starts(id).len();
    let body = post(vec![
        text("This item"),
        ChatPart::ItemRef {
            thread_id: id,
            item_id: item,
        },
    ]);
    let narrow = key(&s, vec![rule("chat", chat, &["write"])]).await;
    assert_eq!(
        send(&s, &narrow, chat, &body).await.problem().code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(counts(&s, chat, id).await, before);
    let refs_denied: i32 = sqlx::query_scalar("SELECT chat_refs FROM items WHERE id=$1")
        .bind(item)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(refs_denied, refs_before);
    let permitted = key(
        &s,
        vec![
            rule("chat", chat, &["write"]),
            rule("thread", id, &["read"]),
        ],
    )
    .await;
    receipt(send(&s, &permitted, chat, &body).await, chat, 201);
    let refs_after: i32 = sqlx::query_scalar("SELECT chat_refs FROM items WHERE id=$1")
        .bind(item)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(refs_after, refs_before + 1);
    assert_eq!(
        counts(&s, chat, id).await,
        (before.0 + 1, before.1, before.2),
        "quote counters do not direct an agent"
    );
    assert_eq!(s.rig.turn_starts(id).len(), starts);
    s.rig.engine.stop();
}

#[tokio::test]
async fn an_early_replay_ignores_new_out_of_scope_agent_effects_and_returns_only_its_receipt() {
    let s = Scene::new().await;
    let target = held_thread(&s, "Unused replacement target").await.base.id;
    let chat = s.tree.topic.base.chat_id;
    let plain = post(vec![text("The accepted original body")]);
    let only_chat = key(&s, vec![rule("chat", chat, &["write"])]).await;
    let original = receipt(send(&s, &only_chat, chat, &plain).await, chat, 201);
    let (mut replacement, _) = reviewed(&s, chat, target, None).await;
    replacement.client_id = plain.client_id.clone();
    let before = counts(&s, chat, target).await;
    assert_eq!(
        receipt(send(&s, &only_chat, chat, &replacement).await, chat, 200),
        original
    );
    assert_eq!(
        counts(&s, chat, target).await,
        before,
        "the replacement body is not a new act"
    );
    let stored: Value = sqlx::query_scalar("SELECT content FROM chat_messages WHERE id=$1")
        .bind(original)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(stored, json!(plain.parts));
    assert!(s.rig.turn_starts(target).is_empty());
    s.rig.engine.stop();
}

#[tokio::test]
async fn plain_human_mentions_and_replies_remain_writes_while_forged_forwarded_parts_are_refused() {
    let s = Scene::new().await;
    let target = held_thread(&s, "Only used for a valid copied-part fixture")
        .await
        .base
        .id;
    let chat = s.tree.topic.base.chat_id;
    let original = send(
        &s,
        &s.eve.token,
        chat,
        &post(vec![text("An existing reply")]),
    )
    .await
    .ok()
    .into_full()
    .unwrap()
    .message;
    let mut plain = post(vec![
        text("A human reply"),
        ChatPart::Mention {
            target: ChatMentionTarget::User { id: s.eve.id() },
        },
    ]);
    plain.reply_to = Some(original.id);
    let only_chat = key(&s, vec![rule("chat", chat, &["write"])]).await;
    let before = counts(&s, chat, target).await;
    receipt(send(&s, &only_chat, chat, &plain).await, chat, 201);
    assert_eq!(
        counts(&s, chat, target).await,
        (before.0 + 1, before.1, before.2)
    );
    let (_, preview) = reviewed(&s, chat, target, Some(original.id)).await;
    let ContentPart::Forwarded(copy) = &preview.agent_asks[0].input.content[0] else {
        panic!("a structurally valid forwarded part")
    };
    let forged = post(vec![ChatPart::Forwarded(copy.clone())]);
    // Source scope is sufficient: rejection must be the existing shape rule, not a missing Read.
    let full = key(
        &s,
        vec![
            rule("chat", chat, &["write", "read"]),
            rule("thread", target, &["write", "read"]),
        ],
    )
    .await;
    let before = counts(&s, chat, target).await;
    assert_eq!(
        send(&s, &full, chat, &forged).await.problem().code,
        ErrorCode::RequestInvalid
    );
    assert_eq!(counts(&s, chat, target).await, before);
    assert!(s.rig.turn_starts(target).is_empty());
    s.rig.engine.stop();
}

async fn asked_input(s: &Scene, chat: Id, message: Id, target: Id) -> Id {
    let row = orch_db::queries::chats::message(&mut *s.rig.world.conn().await, chat, message)
        .await
        .unwrap()
        .unwrap();
    row.asked_agents
        .as_array()
        .unwrap()
        .iter()
        .find(|asked| asked["thread_id"] == json!(target))
        .unwrap()["input_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn message_identity(reply: Reply<ChatMessageCreateResponse>) -> Id {
    assert_eq!(reply.status.as_u16(), 201, "{:?}", reply.body);
    match reply.ok() {
        ChatMessageCreateResponse::Full(reply) => reply.message.id,
        ChatMessageCreateResponse::Receipt(receipt) => receipt.message_id,
    }
}

async fn release_ask(s: &Scene, target: Id) {
    use orch_api_contract::threads::{ThreadQueueResume, ThreadQueueResumeBody};
    let row = orch_db::queries::threads::thread(&mut *s.rig.world.conn().await, target)
        .await
        .unwrap()
        .unwrap();
    s.rig
        .world
        .app
        .call(ThreadQueueResume, &[&target])
        .token(&s.ada.token)
        .body(&ThreadQueueResumeBody {
            revision: row.queue_revision,
        })
        .send()
        .await
        .ok();
    s.rig.engine.preflight(target).await.unwrap();
}

async fn key_identity(s: &Scene, secret: &str) -> Id {
    let hash = orch_api_core::middleware::authentication::token_hash(secret);
    sqlx::query_scalar("SELECT id FROM api_tokens WHERE token_hash=$1")
        .bind(hash)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn an_agent_ask_executes_under_its_actual_chat_message_origin() {
    let s = Scene::new().await;
    let target = held_thread(&s, "Actual chat origin").await.base.id;
    let chat = s.tree.topic.base.chat_id;
    let (body, _) = reviewed(&s, chat, target, None).await;
    let secret = key(
        &s,
        vec![
            rule("chat", chat, &["write", "read"]),
            rule("thread", target, &["write"]),
        ],
    )
    .await;
    let message = message_identity(send(&s, &secret, chat, &body).await);
    let input = asked_input(&s, chat, message, target).await;
    let authority: Value =
        sqlx::query_scalar("SELECT submission_authority FROM thread_inputs WHERE id=$1")
            .bind(input)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    // Acceptance needed source Chat Read to create the copied input; delivery uses saved text.
    let token = key_identity(&s, &secret).await;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(token)
        .bind(json!({"rules":[rule("chat",chat,&["write"]),rule("thread",target,&["write"])]}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    release_ask(&s, target).await;
    let start = s.rig.turn_start(target, 0).await;
    assert!(start.text.contains("Please examine this"));
    assert!(start.native_action.is_none());
    assert_eq!(
        authority["chat"],
        json!({"chat_id":chat,"message_id":message,"thread_id":target,"input_id":input})
    );
    assert_eq!(authority["origins"][0]["operation"], "ChatMessageCreate");
    assert!(!authority.to_string().contains("ThreadInputCreate"));
    assert!(!authority.to_string().contains("node_ancestors"));
    // Receipt of turn.start precedes the native turn.started acknowledgement.
    s.rig
        .answer(target, &start, "chat-origin-done", "done")
        .await;
    s.rig.wait_turn(start.turn_id, "running").await;
    let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "delivered");
    assert_eq!(row.author_id, Some(s.ada.id()));
    s.rig
        .say(
            target,
            rig::turn_ended(
                start.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    s.rig.engine.stop();
}

#[tokio::test]
async fn an_agent_ask_rechecks_live_key_recipient_policy_and_actual_association() {
    for change in ["revoked", "target", "association"] {
        let s = Scene::new().await;
        let target = held_thread(&s, "Restricted chat origin").await.base.id;
        let other = held_thread(&s, "Different recipient").await.base.id;
        let chat = s.tree.topic.base.chat_id;
        let (body, _) = reviewed(&s, chat, target, None).await;
        let secret = key(
            &s,
            vec![
                rule("chat", chat, &["write", "read"]),
                rule("thread", target, &["write"]),
                rule("thread", other, &["write"]),
            ],
        )
        .await;
        let message = message_identity(send(&s, &secret, chat, &body).await);
        let input = asked_input(&s, chat, message, target).await;
        let token = key_identity(&s, &secret).await;
        let mut conn = s.rig.world.conn().await;
        match change {
            "revoked" => {
                orch_db::queries::identity::revoke_api_token(
                    &mut conn,
                    token,
                    s.ada.id(),
                    s.rig.world.now(),
                )
                .await
                .unwrap();
            }
            "target" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1").bind(token)
                    .bind(json!({"rules":[rule("chat",chat,&["write","read"]),rule("thread",other,&["write"])]}))
                    .execute(&mut *conn).await.unwrap();
            }
            "association" => {
                // Both receivers are scoped and ordinarily allowed: only the false retained relationship fails.
                sqlx::query("UPDATE thread_inputs SET submission_authority=jsonb_set(submission_authority,'{chat,thread_id}',$2) WHERE id=$1")
                    .bind(input).bind(json!(other)).execute(&mut *conn).await.unwrap();
            }
            _ => unreachable!(),
        }
        drop(conn);
        release_ask(&s, target).await;
        assert!(s.rig.turn_starts(target).is_empty(), "{change}");
        let accepted =
            orch_db::queries::chats::message(&mut *s.rig.world.conn().await, chat, message)
                .await
                .unwrap()
                .unwrap();
        assert!(
            accepted.deleted_at.is_none(),
            "refusing future work does not undo the committed post"
        );
        let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(row.state, "delivered", "{change}");
        s.rig.engine.stop();
    }
}

#[tokio::test]
async fn copied_reply_text_survives_deletion_without_reopening_original_snapshot_reads() {
    use orch_api_contract::chats::ChatMessageDelete;
    let s = Scene::new().await;
    let target = held_thread(&s, "Retained quote destination").await.base.id;
    let chat = s.tree.topic.base.chat_id;
    let original_text = "The completed quote stays in the accepted input";
    let original =
        message_identity(send(&s, &s.eve.token, chat, &post(vec![text(original_text)])).await);
    let (body, _) = reviewed(&s, chat, target, Some(original)).await;
    let secret = key(
        &s,
        vec![
            rule("chat", chat, &["write", "read"]),
            rule("thread", target, &["write"]),
        ],
    )
    .await;
    let message = message_identity(send(&s, &secret, chat, &body).await);
    let input = asked_input(&s, chat, message, target).await;
    s.rig
        .world
        .app
        .call(ChatMessageDelete, &[&chat, &original])
        .token(&s.eve.token)
        .send()
        .await
        .ok();
    s.rig
        .world
        .app
        .call(ChatMessageDelete, &[&chat, &message])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let token = key_identity(&s, &secret).await;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(token)
        .bind(json!({"rules":[rule("chat",chat,&["write"]),rule("thread",target,&["write"])]}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let source = orch_db::queries::chats::message(&mut *s.rig.world.conn().await, chat, original)
        .await
        .unwrap()
        .unwrap();
    assert!(source.deleted_at.is_some());
    assert!(!source.content.to_string().contains(original_text));
    release_ask(&s, target).await;
    let start = s.rig.turn_start(target, 0).await;
    assert!(start.text.contains(original_text));
    assert!(start.text.contains("Please examine this"));
    s.rig
        .answer(target, &start, "snapshot-origin-done", "done")
        .await;
    s.rig.wait_turn(start.turn_id, "running").await;
    let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "delivered");
    s.rig
        .say(
            target,
            rig::turn_ended(
                start.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    s.rig.engine.stop();
}

#[tokio::test]
async fn future_attachment_delivery_still_requires_a_current_complete_blob_witness() {
    use orch_api_contract::threads::{FILENAME_HEADER, UploadCreate};
    let s = Scene::new().await;
    let target = held_thread(&s, "Current attachment source").await.base.id;
    let chat = s.tree.topic.base.chat_id;
    let upload = s
        .rig
        .world
        .app
        .call(UploadCreate, &[])
        .token(&s.ada.token)
        .header("content-type", "image/jpeg")
        .header(FILENAME_HEADER, "witness.jpg")
        .bytes(b"retained bytes for a source-boundary control")
        .send()
        .await
        .ok();
    let mut body = post(vec![
        text("Use this retained attachment"),
        ChatPart::Attachment {
            blob: upload.blob,
            name: upload.name,
            size: upload.size,
            media_type: upload.media_type,
        },
        agent(target),
    ]);
    let preview = s
        .rig
        .world
        .app
        .call(ChatMentionPreview, &[&chat])
        .token(&s.ada.token)
        .body(&ChatMentionPreviewBody {
            parts: body.parts.clone(),
            reply_to: None,
        })
        .send()
        .await
        .ok();
    body.agent_review = preview.agent_review;
    assert!(body.agent_review.is_some());
    let upload_rule = json!({"target":{"kind":"account","id":s.ada.id(),"reach":"self"},"resources":["upload"],
        "allow":[{"operation":"ChatMessageCreate","actions":["read"]}]});
    let secret = key(
        &s,
        vec![
            rule("chat", chat, &["write", "read"]),
            rule("thread", target, &["write"]),
            upload_rule,
        ],
    )
    .await;
    let message = message_identity(send(&s, &secret, chat, &body).await);
    let input = asked_input(&s, chat, message, target).await;
    let token = key_identity(&s, &secret).await;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(token)
        .bind(json!({"rules":[rule("chat",chat,&["write"]),rule("thread",target,&["write"])]}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    release_ask(&s, target).await;
    let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.state, "cancelled",
        "source scope is refused before attachment/native execution"
    );
    assert!(s.rig.turn_starts(target).is_empty());
    assert!(row.content.to_string().contains(&upload.blob.to_string()));
    s.rig.engine.stop();
}
