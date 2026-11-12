//! Real keys restrict one forwarding operation's actual sources, destinations and disclosures.
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
    ChatMentionTarget, ChatMessageCreate, ChatMessageCreateBody, ChatPart, ForwardCreate,
    ForwardCreateBody, ForwardCreateReply, ForwardCreateResponse, ForwardDestination,
    ForwardDestinationChat, ForwardDestinationThread, ForwardDestinationUser, ForwardSource,
    ForwardSourceChat, ForwardSourceThread,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::{ContentPart, Forwarded, ForwardedAuthor, ForwardedFrom};
use orch_api_core::testing::Reply;
use orch_types::{Id, new_id};
use serde_json::{Value, json};

fn rule(kind: &str, id: Id, actions: &[&str]) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":"ForwardCreate","actions":actions}]})
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
            name: "Forward operation control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: now,
            // These fixed-clock controls need only an ordinary short lifetime.
            expires_at: Some(now + chrono::Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    secret
}
fn chat(id: Id) -> ForwardDestination {
    ForwardDestination::Chat(ForwardDestinationChat { chat_id: id })
}
fn thread(id: Id) -> ForwardDestination {
    ForwardDestination::Thread(ForwardDestinationThread { thread_id: id })
}
fn request(source: Id, message: Id, to: Vec<ForwardDestination>) -> ForwardCreateBody {
    ForwardCreateBody {
        from: ForwardSource::Chat(ForwardSourceChat {
            chat_id: source,
            message_ids: vec![message],
        }),
        to,
        comment: None,
        client_id: new_id().to_string(),
    }
}
async fn send(
    world: &support::World,
    token: &str,
    body: &ForwardCreateBody,
) -> Reply<ForwardCreateResponse> {
    world
        .app
        .call(ForwardCreate, &[])
        .token(token)
        .body(body)
        .send()
        .await
}
#[derive(Debug, PartialEq, Eq)]
struct Accepted {
    chats: Vec<(Id, Id)>,
    threads: Vec<(Id, Id)>,
}
fn receipt(reply: Reply<ForwardCreateResponse>, status: u16) -> Accepted {
    assert_eq!(reply.status.as_u16(), status, "{:?}", reply.body);
    let raw: Value = serde_json::from_slice(&reply.raw).unwrap();
    let ForwardCreateResponse::Receipt(receipt) = reply.ok() else {
        panic!("Read on every returned destination was not granted")
    };
    let expected = json!({"projection":"receipt",
        "chats":receipt.chats.iter().map(|row|json!({"chat_id":row.chat_id,"message_id":row.message_id,"seq":row.seq})).collect::<Vec<_>>(),
        "threads":receipt.threads.iter().map(|row|json!({"thread_id":row.thread_id,"input_id":row.input_id,"seq":row.seq})).collect::<Vec<_>>()});
    assert_eq!(
        raw, expected,
        "a receipt carries no copied material, names or other current projections"
    );
    assert!(receipt.chats.iter().all(|row| row.seq >= 0));
    assert!(receipt.threads.iter().all(|row| row.seq >= 0));
    Accepted {
        chats: receipt
            .chats
            .into_iter()
            .map(|row| (row.chat_id, row.message_id))
            .collect(),
        threads: receipt
            .threads
            .into_iter()
            .map(|row| (row.thread_id, row.input_id))
            .collect(),
    }
}
fn full(reply: Reply<ForwardCreateResponse>, status: u16) -> ForwardCreateReply {
    assert_eq!(reply.status.as_u16(), status, "{:?}", reply.body);
    reply
        .ok()
        .into_full()
        .expect("the same operation can read every returned destination")
}
async fn messages(world: &support::World, chat: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM chat_messages WHERE chat_id=$1")
        .bind(chat)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
async fn inputs(world: &support::World, thread: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(thread)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
async fn message_parts(world: &support::World, message: Id) -> Vec<ChatPart> {
    let value: Value = sqlx::query_scalar("SELECT content FROM chat_messages WHERE id=$1")
        .bind(message)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    serde_json::from_value(value).unwrap()
}
fn copy(parts: &[ChatPart]) -> &Forwarded {
    parts
        .iter()
        .find_map(|part| match part {
            ChatPart::Forwarded(copy) => Some(copy.as_ref()),
            _ => None,
        })
        .expect("the ordinary copied part exists")
}

#[tokio::test]
async fn forwarding_needs_source_read_separately_and_destination_read_only_changes_projection() {
    let room = Room::new().await;
    let original = room
        .say(&room.bea, room.chat, "Actual source material")
        .await;
    let destination = room
        .group(&room.ada, &[&room.cai], Some("Destination"))
        .await
        .base
        .id;
    assert!(room.chat(&room.ada, destination).await.can.send);
    let body = request(room.chat, original.id, vec![chat(destination)]);
    let before = messages(&room.world, destination).await;
    let no_source = key(
        &room.world,
        room.ada.id(),
        vec![rule("chat", destination, &["write", "read"])],
    )
    .await;
    assert_eq!(
        send(&room.world, &no_source, &body).await.problem().code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(messages(&room.world, destination).await, before);
    let writer = key(
        &room.world,
        room.ada.id(),
        vec![
            rule("chat", room.chat, &["read"]),
            rule("chat", destination, &["write"]),
        ],
    )
    .await;
    let made = receipt(send(&room.world, &writer, &body).await, 201);
    assert_eq!(made.chats.len(), 1);
    assert_eq!(made.chats[0].0, destination);
    assert!(made.threads.is_empty());
    let stored = message_parts(&room.world, made.chats[0].1).await;
    assert_eq!(copy(&stored).text, "Actual source material");
    assert!(
        matches!(&copy(&stored).author,Some(ForwardedAuthor::User(user)) if user.id==room.bea.id())
    );
    assert_eq!(messages(&room.world, destination).await, before + 1);
    assert_eq!(receipt(send(&room.world, &writer, &body).await, 200), made);
    assert_eq!(messages(&room.world, destination).await, before + 1);
    let reader = key(
        &room.world,
        room.ada.id(),
        vec![
            rule("chat", room.chat, &["read"]),
            rule("chat", destination, &["write", "read"]),
        ],
    )
    .await;
    let shown = full(send(&room.world, &reader, &body).await, 200);
    assert_eq!(shown.chats[0].message.id, made.chats[0].1);
    assert_eq!(
        copy(&shown.chats[0].message.parts).text,
        "Actual source material"
    );
    let ordinary = full(send(&room.world, &room.ada.token, &body).await, 200);
    assert_eq!(ordinary.chats[0].message.id, made.chats[0].1);
    assert_eq!(messages(&room.world, destination).await, before + 1);
}

#[tokio::test]
async fn a_thread_item_source_needs_its_own_read_even_when_the_destination_chat_is_writable() {
    let s = rig::Scene::new().await;
    let source = s.thread_by(&s.ada, "Thread source").await.base.id;
    s.exchange(source, 0, "Question to copy", "Answer to copy")
        .await;
    let selected:Vec<Id>=sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND kind IN ('user_message','agent_message') ORDER BY seq_created")
        .bind(source).fetch_all(s.rig.db.pool()).await.unwrap();
    assert_eq!(selected.len(), 2);
    let destination = s.tree.topic.base.chat_id;
    let body = ForwardCreateBody {
        from: ForwardSource::Thread(ForwardSourceThread {
            thread_id: source,
            item_ids: selected,
        }),
        to: vec![chat(destination)],
        comment: None,
        client_id: new_id().to_string(),
    };
    let before = messages(&s.rig.world, destination).await;
    let input_before = inputs(&s.rig.world, source).await;
    let no_source = key(
        &s.rig.world,
        s.ada.id(),
        vec![rule("chat", destination, &["write", "read"])],
    )
    .await;
    assert_eq!(
        send(&s.rig.world, &no_source, &body).await.problem().code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(messages(&s.rig.world, destination).await, before);
    let permitted = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule("thread", source, &["read"]),
            rule("chat", destination, &["write", "read"]),
        ],
    )
    .await;
    let shown = full(send(&s.rig.world, &permitted, &body).await, 201);
    assert_eq!(shown.chats.len(), 1);
    assert!(shown.threads.is_empty());
    let copies: Vec<_> = shown.chats[0]
        .message
        .parts
        .iter()
        .filter_map(|part| match part {
            ChatPart::Forwarded(copy) => Some(copy.as_ref()),
            _ => None,
        })
        .collect();
    assert_eq!(copies.len(), 2);
    assert!(
        copies
            .iter()
            .any(|copy| copy.text.contains("Question to copy"))
    );
    assert!(
        copies
            .iter()
            .any(|copy| copy.text.contains("Answer to copy"))
    );
    assert_eq!(inputs(&s.rig.world, source).await, input_before);
    let quotes: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(chat_refs),0)::bigint FROM items WHERE thread_id=$1",
    )
    .bind(source)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(
        quotes, 0,
        "copying elsewhere does not direct or quote back into the source thread"
    );
    assert_eq!(messages(&s.rig.world, destination).await, before + 1);
    s.rig.engine.stop();
}

#[tokio::test]
async fn mixed_chat_and_thread_destinations_are_atomic_and_all_returned_targets_gate_full_output() {
    let s = rig::Scene::new().await;
    let target = s.thread_by(&s.ada, "Mixed destination").await;
    assert!(target.input_access.allowed);
    sqlx::query("UPDATE threads SET queue_held=true WHERE node_id=$1")
        .bind(target.base.id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let source = s.tree.topic.base.chat_id;
    let original = s
        .rig
        .world
        .app
        .call(ChatMessageCreate, &[&source])
        .token(&s.ada.token)
        .body(&ChatMessageCreateBody {
            parts: vec![text("Mixed source material")],
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
    let destination = target.base.chat_id;
    let target_id = target.base.id;
    let body = request(
        source,
        original.id,
        vec![chat(destination), thread(target_id)],
    );
    let before = (
        messages(&s.rig.world, destination).await,
        inputs(&s.rig.world, target_id).await,
    );
    let no_thread = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule("chat", source, &["read"]),
            rule("chat", destination, &["write", "read"]),
        ],
    )
    .await;
    assert_eq!(
        send(&s.rig.world, &no_thread, &body).await.problem().code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        (
            messages(&s.rig.world, destination).await,
            inputs(&s.rig.world, target_id).await
        ),
        before
    );
    let no_chat = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule("chat", source, &["read"]),
            rule("thread", target_id, &["write", "read"]),
        ],
    )
    .await;
    assert_eq!(
        send(&s.rig.world, &no_chat, &body).await.problem().code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        (
            messages(&s.rig.world, destination).await,
            inputs(&s.rig.world, target_id).await
        ),
        before
    );
    let writer = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule("chat", source, &["read"]),
            rule("chat", destination, &["write"]),
            rule("thread", target_id, &["write"]),
        ],
    )
    .await;
    let accepted = receipt(send(&s.rig.world, &writer, &body).await, 201);
    assert_eq!(accepted.chats.len(), 1);
    assert_eq!(accepted.threads.len(), 1);
    assert_eq!(accepted.chats[0].0, destination);
    assert_eq!(accepted.threads[0].0, target_id);
    let (author, content, held): (Id, Value, bool) =
        sqlx::query_as("SELECT author_id,content,queue_held FROM thread_inputs WHERE id=$1")
            .bind(accepted.threads[0].1)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(author, s.ada.id());
    assert!(held);
    let content: Vec<ContentPart> = serde_json::from_value(content).unwrap();
    assert!(content.iter().any(
        |part| matches!(part,ContentPart::Forwarded(copy) if copy.text=="Mixed source material")
    ));
    let one_read = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule("chat", source, &["read"]),
            rule("chat", destination, &["write", "read"]),
            rule("thread", target_id, &["write"]),
        ],
    )
    .await;
    assert_eq!(
        receipt(send(&s.rig.world, &one_read, &body).await, 200),
        accepted,
        "one readable result cannot expose another destination's projection"
    );
    let every_read = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule("chat", source, &["read"]),
            rule("chat", destination, &["write", "read"]),
            rule("thread", target_id, &["write", "read"]),
        ],
    )
    .await;
    let shown = full(send(&s.rig.world, &every_read, &body).await, 200);
    assert_eq!(shown.chats[0].message.id, accepted.chats[0].1);
    assert_eq!(shown.threads[0].input.id, accepted.threads[0].1);
    assert_eq!(
        (
            messages(&s.rig.world, destination).await,
            inputs(&s.rig.world, target_id).await
        ),
        (before.0 + 1, before.1 + 1)
    );
    assert!(
        s.rig.turn_starts(target_id).is_empty(),
        "queued execution is a separate authority control"
    );
    s.rig.engine.stop();
}

#[tokio::test]
async fn forwarding_clamps_live_source_names_under_its_own_read_grants_without_scrubbing_saved_text()
 {
    let room = Room::new().await;
    let literal = "Stored literal Login ";
    let original = room
        .send(
            &room.bea,
            room.chat,
            vec![
                text(literal),
                ChatPart::Mention {
                    target: ChatMentionTarget::Role {
                        node_id: room.topic,
                        role: Role::Developer,
                    },
                },
            ],
            &new_id().to_string(),
        )
        .await
        .ok()
        .message;
    let destination = room
        .group(&room.ada, &[&room.cai], Some("Name destination"))
        .await
        .base
        .id;
    let body = request(room.chat, original.id, vec![chat(destination)]);
    let mut other = rule("topic", room.topic, &["read"]);
    other["allow"] = json!([{"operation":"ChatMessageCreate","actions":["read"]}]);
    let limited = key(
        &room.world,
        room.ada.id(),
        vec![
            rule("chat", room.chat, &["read"]),
            rule("chat", destination, &["write", "read"]),
            other,
        ],
    )
    .await;
    let hidden = full(send(&room.world, &limited, &body).await, 201);
    let copied = copy(&hidden.chats[0].message.parts);
    assert!(copied.text.contains(literal));
    assert!(copied.text.contains("@Developers of a private place"));
    let ForwardedFrom::Chat { chat: source } = &copied.from else {
        panic!("the source stays a chat")
    };
    assert_eq!(source.id, room.chat);
    assert!(source.node.as_ref().unwrap().name.is_none());
    assert!(source.node.as_ref().unwrap().redacted);
    assert!(
        matches!(&copied.author,Some(ForwardedAuthor::User(user)) if user.id==room.bea.id()&&user.username=="bea")
    );
    assert_eq!(
        copy(&message_parts(&room.world, hidden.chats[0].message.id).await),
        copied,
        "clamping must happen before the new snapshot is stored"
    );
    let mut fresh = body.clone();
    fresh.client_id = new_id().to_string();
    let permitted = key(
        &room.world,
        room.ada.id(),
        vec![
            rule("chat", room.chat, &["read"]),
            rule("chat", destination, &["write", "read"]),
            rule("topic", room.topic, &["read"]),
            rule("project", room.project, &["read"]),
            rule("org", room.root, &["read"]),
        ],
    )
    .await;
    let shown = full(send(&room.world, &permitted, &fresh).await, 201);
    let copied = copy(&shown.chats[0].message.parts);
    assert!(copied.text.contains(literal));
    assert!(copied.text.contains("@Developers of Acme > Web > Login"));
    let ForwardedFrom::Chat { chat: source } = &copied.from else {
        panic!("the source stays a chat")
    };
    assert_eq!(source.node.as_ref().unwrap().name.as_deref(), Some("Login"));
    assert_eq!(
        copy(&message_parts(&room.world, shown.chats[0].message.id).await),
        copied
    );
}

#[tokio::test]
async fn a_new_direct_chat_uses_its_org_creation_target_not_the_recipient_account() {
    let room = Room::new().await;
    let original = room.say(&room.bea, room.chat, "A direct-chat source").await;
    let body = request(
        room.chat,
        original.id,
        vec![ForwardDestination::User(ForwardDestinationUser {
            user_id: room.gus.id(),
        })],
    );
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM chats")
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
    let wrong = key(
        &room.world,
        room.ada.id(),
        vec![
            rule("chat", room.chat, &["read"]),
            rule("account", room.gus.id(), &["write"]),
        ],
    )
    .await;
    assert_eq!(
        send(&room.world, &wrong, &body).await.problem().code,
        ErrorCode::AuthzDenied
    );
    let unchanged: i64 = sqlx::query_scalar("SELECT count(*) FROM chats")
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
    assert_eq!(unchanged, before);
    let mut creation = rule("org", room.root, &["write"]);
    creation["resources"] = json!(["chat"]);
    let permitted = key(
        &room.world,
        room.ada.id(),
        vec![rule("chat", room.chat, &["read"]), creation],
    )
    .await;
    let accepted = receipt(send(&room.world, &permitted, &body).await, 201);
    assert_eq!(accepted.chats.len(), 1);
    assert!(accepted.threads.is_empty());
    let made = room.chat(&room.ada, accepted.chats[0].0).await;
    assert_eq!(made.base.kind, orch_api_contract::chats::ChatKind::Direct);
    assert_eq!(made.base.org_id, room.root);
    let members: Vec<_> = made
        .members
        .unwrap()
        .into_iter()
        .map(|person| person.id)
        .collect();
    assert!(members.contains(&room.ada.id()));
    assert!(members.contains(&room.gus.id()));
    assert_eq!(members.len(), 2);
    assert_eq!(
        copy(&message_parts(&room.world, accepted.chats[0].1).await).text,
        "A direct-chat source"
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM chats")
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        after,
        before + 1,
        "the authorized parent operation really created one chat"
    );
}

async fn queued_target(s: &rig::Scene, name: &str) -> Id {
    let target = s.thread_by(&s.ada, name).await;
    assert!(target.input_access.allowed);
    sqlx::query("UPDATE threads SET queue_held=true WHERE node_id=$1")
        .bind(target.base.id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    target.base.id
}

async fn posted_source(s: &rig::Scene, parts: Vec<ChatPart>) -> Id {
    s.rig
        .world
        .app
        .call(ChatMessageCreate, &[&s.tree.topic.base.chat_id])
        .token(&s.eve.token)
        .body(&ChatMessageCreateBody {
            parts,
            reply_to: None,
            agent_review: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .message
        .id
}

async fn release_forward(s: &rig::Scene, target: Id) {
    use orch_api_contract::threads::{ThreadQueueResume, ThreadQueueResumeBody};
    // Native refusal can cancel a held input and advance the queue revision.
    // Finish that recheck before choosing the queue this person will resume.
    let starts = s.rig.turn_starts(target).len();
    s.rig.engine.preflight(target).await.unwrap();
    assert_eq!(
        s.rig.turn_starts(target).len(),
        starts,
        "settling the held queue must not start a forwarded turn"
    );
    rig::eventually(
        "resume the current queue after native forward cancellation",
        || async {
            let row = orch_db::queries::threads::thread(&mut *s.rig.world.conn().await, target)
                .await
                .unwrap()
                .unwrap();
            let reply = s
                .rig
                .world
                .app
                .call(ThreadQueueResume, &[&target])
                .token(&s.ada.token)
                .body(&ThreadQueueResumeBody {
                    revision: row.queue_revision,
                })
                .send()
                .await;
            match reply.body {
                Ok(_) => true,
                Err(problem) => {
                    assert_eq!(
                        problem.code,
                        ErrorCode::ConflictStaleRevision,
                        "{problem:?}"
                    );
                    assert!(
                        problem
                            .extra
                            .get("current_revision")
                            .is_some_and(Value::is_i64),
                        "{problem:?}"
                    );
                    false
                }
            }
        },
    )
    .await;
    s.rig.engine.preflight(target).await.unwrap();
}

async fn forwarding_key_id(s: &rig::Scene, secret: &str) -> Id {
    sqlx::query_scalar("SELECT id FROM api_tokens WHERE token_hash=$1")
        .bind(orch_api_core::middleware::authentication::token_hash(
            secret,
        ))
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}

async fn finish_forward(
    s: &rig::Scene,
    target: Id,
    start: &orch_link_contract::commands::TurnStart,
) {
    s.rig
        .answer(target, start, "forward-finished", "done")
        .await;
    s.rig
        .say(
            target,
            rig::turn_ended(
                start.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
}

#[tokio::test]
async fn queued_forward_dispatches_completed_text_after_source_deletion_and_read_removal() {
    use orch_api_contract::chats::ChatMessageDelete;
    let s = rig::Scene::new().await;
    let target = queued_target(&s, "Forward dispatch").await;
    let source = s.tree.topic.base.chat_id;
    let words = "Copied text from another author remains accepted work";
    let message = posted_source(&s, vec![text(words)]).await;
    let body = request(source, message, vec![thread(target)]);
    let secret = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule("chat", source, &["read"]),
            rule("thread", target, &["write"]),
        ],
    )
    .await;
    let accepted = receipt(send(&s.rig.world, &secret, &body).await, 201);
    let input = accepted.threads[0].1;
    let before = inputs(&s.rig.world, target).await;
    assert_eq!(
        receipt(send(&s.rig.world, &secret, &body).await, 200),
        accepted
    );
    assert_eq!(
        inputs(&s.rig.world, target).await,
        before,
        "replay preserves the original acceptance"
    );
    s.rig
        .world
        .app
        .call(ChatMessageDelete, &[&source, &message])
        .token(&s.eve.token)
        .send()
        .await
        .ok();
    let token = forwarding_key_id(&s, &secret).await;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(token)
        .bind(json!({"rules":[rule("thread",target,&["write"])]}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    release_forward(&s, target).await;
    let start = s.rig.turn_start(target, 0).await;
    assert!(start.text.contains(words));
    assert!(start.native_action.is_none());
    // A start command is still delivering until the native harness takes it up.
    finish_forward(&s, target, &start).await;
    s.rig.wait_turn(start.turn_id, "completed").await;
    let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "delivered");
    assert_eq!(row.author_id, Some(s.ada.id()));
    let authority = row.submission_authority.unwrap();
    assert_eq!(authority["origins"][0]["operation"], "ForwardCreate");
    assert_eq!(
        authority["forward"],
        json!({"source":{"kind":"chat","chat_id":source,"message_ids":[message]},"thread_id":target,"input_id":input})
    );
    assert!(!authority.to_string().contains("ThreadInputCreate"));
    assert!(!authority.to_string().contains("node_ancestors"));
    s.rig.engine.stop();
}

#[tokio::test]
async fn queued_forward_keeps_verified_thread_item_identity_through_actual_dispatch() {
    let s = rig::Scene::new().await;
    let source = s.thread_by(&s.ada, "Readable thread source").await.base.id;
    s.exchange(source, 0, "Selected question", "Selected answer")
        .await;
    let selected:Vec<Id>=sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND kind IN ('user_message','agent_message') ORDER BY seq_created")
        .bind(source).fetch_all(s.rig.db.pool()).await.unwrap();
    assert_eq!(selected.len(), 2);
    let target = queued_target(&s, "Thread copy recipient").await;
    let body = ForwardCreateBody {
        from: ForwardSource::Thread(ForwardSourceThread {
            thread_id: source,
            item_ids: selected.clone(),
        }),
        to: vec![thread(target)],
        comment: None,
        client_id: new_id().to_string(),
    };
    let secret = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule("thread", source, &["read"]),
            rule("thread", target, &["write"]),
        ],
    )
    .await;
    let accepted = receipt(send(&s.rig.world, &secret, &body).await, 201);
    let input = accepted.threads[0].1;
    release_forward(&s, target).await;
    let start = s.rig.turn_start(target, 0).await;
    assert!(start.text.contains("Selected question"));
    assert!(start.text.contains("Selected answer"));
    finish_forward(&s, target, &start).await;
    s.rig.wait_turn(start.turn_id, "completed").await;
    let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "delivered");
    assert_eq!(
        row.submission_authority.as_ref().unwrap()["forward"]["source"],
        json!({"kind":"thread","thread_id":source,"item_ids":selected})
    );
    s.rig.engine.stop();
}

#[tokio::test]
async fn queued_forward_rechecks_revocation_current_target_and_retained_input_anchor() {
    for change in ["revoked", "target", "anchor"] {
        let s = rig::Scene::new().await;
        let target = queued_target(&s, "Restricted forward").await;
        let other = queued_target(&s, "Other allowed recipient").await;
        let source = s.tree.topic.base.chat_id;
        let message = posted_source(&s, vec![text("Accepted before the boundary changed")]).await;
        let body = request(source, message, vec![thread(target)]);
        let secret = key(
            &s.rig.world,
            s.ada.id(),
            vec![
                rule("chat", source, &["read"]),
                rule("thread", target, &["write"]),
                rule("thread", other, &["write"]),
            ],
        )
        .await;
        let accepted = receipt(send(&s.rig.world, &secret, &body).await, 201);
        let input = accepted.threads[0].1;
        let token = forwarding_key_id(&s, &secret).await;
        match change {
            "revoked" => {
                orch_db::queries::identity::revoke_api_token(
                    &mut *s.rig.world.conn().await,
                    token,
                    s.ada.id(),
                    s.rig.world.now(),
                )
                .await
                .unwrap();
            }
            "target" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1").bind(token)
                .bind(json!({"rules":[rule("chat",source,&["read"]),rule("thread",other,&["write"])]})).execute(s.rig.db.pool()).await.unwrap();
            }
            "anchor" => {
                sqlx::query("UPDATE thread_inputs SET submission_authority=jsonb_set(submission_authority,'{forward,input_id}',$2) WHERE id=$1")
                .bind(input).bind(json!(new_id())).execute(s.rig.db.pool()).await.unwrap();
            }
            _ => unreachable!(),
        }
        release_forward(&s, target).await;
        assert!(s.rig.turn_starts(target).is_empty(), "{change}");
        let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(row.state, "delivered", "{change}");
        assert!(
            row.content
                .to_string()
                .contains("Accepted before the boundary changed"),
            "refusal does not undo copied content"
        );
        s.rig.engine.stop();
    }
}

#[tokio::test]
async fn queued_forward_blob_delivery_needs_a_current_complete_source_after_copy() {
    use orch_api_contract::chats::ChatMessageDelete;
    use orch_api_contract::threads::{FILENAME_HEADER, UploadCreate};
    let s = rig::Scene::new().await;
    let target = queued_target(&s, "Deferred blob witness").await;
    let source = s.tree.topic.base.chat_id;
    let upload = s
        .rig
        .world
        .app
        .call(UploadCreate, &[])
        .token(&s.eve.token)
        .header("content-type", "image/jpeg")
        .header(FILENAME_HEADER, "source.jpg")
        .bytes(b"stored bytes whose current source must remain readable")
        .send()
        .await
        .ok();
    let message = posted_source(
        &s,
        vec![
            text("The attachment was copied"),
            ChatPart::Attachment {
                blob: upload.blob,
                name: upload.name,
                size: upload.size,
                media_type: upload.media_type,
            },
        ],
    )
    .await;
    let body = request(source, message, vec![thread(target)]);
    let secret = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule("chat", source, &["read"]),
            rule("thread", target, &["write"]),
        ],
    )
    .await;
    let accepted = receipt(send(&s.rig.world, &secret, &body).await, 201);
    let input = accepted.threads[0].1;
    s.rig
        .world
        .app
        .call(ChatMessageDelete, &[&source, &message])
        .token(&s.eve.token)
        .send()
        .await
        .ok();
    release_forward(&s, target).await;
    let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.state, "cancelled",
        "saved citation IDs do not replace a current scoped blob witness"
    );
    assert!(row.content.to_string().contains(&upload.blob.to_string()));
    assert!(s.rig.turn_starts(target).is_empty());
    s.rig.engine.stop();
}
