//! A deliberate agent token accepts one visible post and ordinary sender-owned inputs, or nothing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::chats::{
    ChatMentionList, ChatMentionListQuery, ChatMentionPreview, ChatMentionPreviewBody,
    ChatMentionPreviewReply, ChatMentionTarget, ChatMessageCreate, ChatMessageCreateBody,
    ChatMessageCreateReply, ChatMessageUpdate, ChatMessageUpdateBody, ChatPart,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::{ContentPart, Delivery, ForwardedAuthor, ForwardedFrom};
use orch_types::Id;
use places::Who;
use rig::Scene;

fn text(words: &str) -> ChatPart {
    ChatPart::Text {
        text: words.to_owned(),
    }
}
fn agent(thread: Id) -> ChatPart {
    ChatPart::Mention {
        target: ChatMentionTarget::Agent { thread_id: thread },
    }
}

async fn preview(
    s: &Scene,
    by: &Who,
    chat: Id,
    parts: &[ChatPart],
    reply: Option<Id>,
) -> orch_api_core::testing::Reply<ChatMentionPreviewReply> {
    s.rig
        .world
        .app
        .call(ChatMentionPreview, &[&chat])
        .token(&by.token)
        .body(&ChatMentionPreviewBody {
            parts: parts.to_vec(),
            reply_to: reply,
        })
        .send()
        .await
}
async fn send(
    s: &Scene,
    by: &Who,
    chat: Id,
    body: &ChatMessageCreateBody,
) -> orch_api_core::testing::Reply<ChatMessageCreateReply> {
    s.rig
        .world
        .app
        .call(ChatMessageCreate, &[&chat])
        .token(&by.token)
        .body(body)
        .send()
        .await
}
async fn input_count(s: &Scene) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM thread_inputs")
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap()
}
async fn message_count(s: &Scene, chat: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM chat_messages WHERE chat_id=$1")
        .bind(chat)
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap()
}

/// Where the place refuses what is written by somebody who may not use a thread's model (rather than
/// holding it, as shipped): asking an agent then needs the sender's own use of its provider.
async fn refusing_writers_without_model_use(s: &Scene) {
    sqlx::query("INSERT INTO settings (node_id,key,value) VALUES ($1,'thread.inputs_without_use','\"refused\"'::jsonb)")
        .bind(s.tree.topic.base.id)
        .execute(&s.rig.world.db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_preview_is_real_but_publishes_nothing_and_send_keeps_authors_sender_and_retry_identity()
 {
    let s = Scene::new().await;
    let one = s.thread_by(&s.ada, "Review").await;
    let two = s.thread_by(&s.ada, "Review").await;
    let chat = s.tree.topic.base.chat_id;
    let original = send(
        &s,
        &s.eve,
        chat,
        &ChatMessageCreateBody {
            parts: vec![text("The original observation")],
            reply_to: None,
            agent_review: None,
            client_id: "observation".into(),
        },
    )
    .await
    .ok()
    .message;
    let parts = vec![
        text("Please compare "),
        agent(one.base.id),
        text(" and "),
        agent(two.base.id),
    ];
    let before = input_count(&s).await;
    let messages = message_count(&s, chat).await;
    let commands = s.rig.commands.issued_of("turn.start");
    let review = preview(&s, &s.ada, chat, &parts, Some(original.id))
        .await
        .ok();
    assert_eq!(review.agent_asks.len(), 2);
    for ask in &review.agent_asks {
        assert_eq!(ask.model.as_deref(), Some(rig::MODEL));
        assert_eq!(ask.input.author.as_ref().map(|by| by.id), Some(s.ada.id()));
        assert_eq!(ask.input.delivery, Delivery::Queue);
        let ContentPart::Forwarded(first) = &ask.input.content[0] else {
            panic!("the reply retains the ordinary forwarded origin")
        };
        assert!(matches!(&first.author, Some(ForwardedAuthor::User(by)) if by.id == s.eve.id()));
        assert_eq!(first.text, "The original observation");
        assert!(matches!(&first.from, ForwardedFrom::Chat { chat:from } if from.id == chat));
        let ContentPart::Forwarded(last) = &ask.input.content[1] else {
            panic!("the visible sender post is the next forwarded part")
        };
        assert!(matches!(&last.author, Some(ForwardedAuthor::User(by)) if by.id == s.ada.id()));
        assert_eq!(last.text, "Please compare @Review and @Review");
    }
    assert_eq!(input_count(&s).await, before);
    assert_eq!(message_count(&s, chat).await, messages);
    assert_eq!(s.rig.commands.issued_of("turn.start"), commands);
    let body = ChatMessageCreateBody {
        parts: parts.clone(),
        reply_to: Some(original.id),
        agent_review: review.agent_review,
        client_id: "ask-once".into(),
    };
    let accepted = send(&s, &s.ada, chat, &body).await;
    assert_eq!(accepted.status, http::StatusCode::CREATED);
    let accepted = accepted.ok().message;
    assert_eq!(accepted.asked_agents.len(), 2);
    assert!(
        accepted.mentions.is_empty(),
        "an agent is not a human notification recipient"
    );
    assert_eq!(input_count(&s).await, before + 2);
    assert_eq!(message_count(&s, chat).await, messages + 1);
    for asked in &accepted.asked_agents {
        let (owner, client): (Id, String) = sqlx::query_as(
            "SELECT author_id,client_id FROM thread_inputs WHERE id=$1 AND thread_id=$2",
        )
        .bind(asked.input_id)
        .bind(asked.thread.id)
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap();
        assert_eq!(owner, s.ada.id());
        assert_eq!(client, format!("chat-mention:{}", accepted.id));
    }
    s.rig
        .world
        .app
        .call(ChatMessageUpdate, &[&chat, &accepted.id])
        .token(&s.ada.token)
        .body(&ChatMessageUpdateBody { parts })
        .send()
        .await
        .ok();
    assert_eq!(
        input_count(&s).await,
        before + 2,
        "editing an agent token never asks again"
    );
    s.rig
        .world
        .set_at(
            orch_settings::Target::Node(one.base.id),
            "agent.model",
            serde_json::json!("changed-after-acceptance"),
        )
        .await;
    let repeated = send(&s, &s.ada, chat, &body).await;
    assert_eq!(repeated.status, http::StatusCode::OK);
    assert_eq!(repeated.ok().message.asked_agents, accepted.asked_agents);
    assert_eq!(input_count(&s).await, before + 2);
}

#[tokio::test]
async fn a_missing_review_changed_reply_or_current_receiver_refusal_writes_no_message_and_no_input()
{
    let s = Scene::new().await;
    refusing_writers_without_model_use(&s).await;
    let one = s.thread_by(&s.ada, "Review").await;
    let chat = s.tree.topic.base.chat_id;
    let original = send(
        &s,
        &s.ada,
        chat,
        &ChatMessageCreateBody {
            parts: vec![text("Old observation")],
            reply_to: None,
            agent_review: None,
            client_id: "observation".into(),
        },
    )
    .await
    .ok()
    .message;
    let parts = vec![text("Review "), agent(one.base.id)];
    let before = input_count(&s).await;
    let messages = message_count(&s, chat).await;
    let mut body = ChatMessageCreateBody {
        parts: parts.clone(),
        reply_to: Some(original.id),
        agent_review: None,
        client_id: "refused-ask".into(),
    };
    assert_eq!(
        send(&s, &s.ada, chat, &body).await.problem().code,
        ErrorCode::ConflictStaleRevision
    );
    body.agent_review = preview(&s, &s.ada, chat, &parts, Some(original.id))
        .await
        .ok()
        .agent_review;
    s.rig
        .world
        .app
        .call(ChatMessageUpdate, &[&chat, &original.id])
        .token(&s.ada.token)
        .body(&ChatMessageUpdateBody {
            parts: vec![text("Corrected observation")],
        })
        .send()
        .await
        .ok();
    assert_eq!(
        send(&s, &s.ada, chat, &body).await.problem().code,
        ErrorCode::ConflictStaleRevision
    );
    let refusal = preview(&s, &s.dan, chat, &parts, Some(original.id))
        .await
        .problem();
    assert_eq!(
        refusal.code,
        ErrorCode::AuthzDenied,
        "the actual normal producer checks the sender's provider use"
    );
    assert_eq!(input_count(&s).await, before);
    assert_eq!(message_count(&s, chat).await, messages);
    body.agent_review = preview(&s, &s.ada, chat, &parts, Some(original.id))
        .await
        .ok()
        .agent_review;
    // Losing the source right after review is an ordinary send refusal, even with the review in hand.
    s.rig
        .world
        .set_at(
            orch_settings::Target::Node(s.tree.topic.base.id),
            "chat.enabled",
            serde_json::json!(false),
        )
        .await;
    assert_eq!(
        send(&s, &s.ada, chat, &body).await.problem().code,
        ErrorCode::ChatNotAllowed
    );
    assert_eq!(input_count(&s).await, before);
    assert_eq!(message_count(&s, chat).await, messages);
}

#[tokio::test]
async fn local_main_agents_need_no_chat_share_and_a_visible_reader_sees_the_disabled_choice() {
    let s = Scene::new().await;
    let one = s.thread_by(&s.ada, "Same name").await;
    let two = s.thread_by(&s.ada, "Same name").await;
    for chat in [s.tree.topic.base.chat_id, one.base.chat_id] {
        let page = s
            .rig
            .world
            .app
            .call(ChatMentionList, &[&chat])
            .token(&s.cai.token)
            .query(&ChatMentionListQuery {
                q: Some("Same name".into()),
                limit: Some(100),
                cursor: None,
            })
            .send()
            .await
            .ok();
        let offered: Vec<_> = page
            .items
            .iter()
            .filter(|choice| matches!(choice.target, ChatMentionTarget::Agent { .. }))
            .collect();
        assert_eq!(offered.len(), if chat == one.base.chat_id { 1 } else { 2 });
        assert!(offered.iter().all(|choice| !choice.available
            && choice.reason.as_deref() == Some("thread.interact")
            && choice.started_at.is_some()));
        assert!(offered.iter().all(|choice| {
            choice
                .thread
                .as_ref()
                .is_some_and(|thread| !thread.redacted)
        }));
        if offered.len() == 2 {
            assert_ne!(offered[0].target, offered[1].target);
        }
    }
    let parts = vec![text("@Same name is ordinary authored text")];
    send(
        &s,
        &s.ada,
        s.tree.topic.base.chat_id,
        &ChatMessageCreateBody {
            parts,
            reply_to: None,
            agent_review: None,
            client_id: "plain-agent-name".into(),
        },
    )
    .await
    .ok();
    assert_eq!(
        input_count(&s).await,
        0,
        "an untyped name is not an invocation"
    );
    let _ = two;
}

#[tokio::test]
async fn losing_the_senders_provider_use_after_review_rolls_back_the_visible_post_and_every_input()
{
    let s = Scene::new().await;
    refusing_writers_without_model_use(&s).await;
    let one = s.thread_by(&s.ada, "First review").await;
    let two = s.thread_by(&s.ada, "Second review").await;
    let chat = s.tree.topic.base.chat_id;
    let parts = vec![
        text("Please review "),
        agent(one.base.id),
        text(" and "),
        agent(two.base.id),
    ];
    let review = preview(&s, &s.eve, chat, &parts, None).await.ok();
    let before = message_count(&s, chat).await;
    let inputs = input_count(&s).await;
    // Fixture revocation uses the same exclusive authority fence as the page producer.
    let mut tx = s.rig.world.app.app.db.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    sqlx::query("UPDATE permission_entries SET removed_at=$3,removed_by=$4 WHERE object_kind='provider' AND object_id=$1 AND permission='provider.use' AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL")
        .bind(s.on.provider).bind(s.eve.id()).bind(s.rig.world.now()).bind(s.ada.id()).execute(&mut *tx).await.unwrap();
    orch_authz::epoch::bump(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    let refused = send(
        &s,
        &s.eve,
        chat,
        &ChatMessageCreateBody {
            parts,
            reply_to: None,
            agent_review: review.agent_review,
            client_id: "withdrawn-use".into(),
        },
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(message_count(&s, chat).await, before);
    assert_eq!(input_count(&s).await, inputs);
}

#[tokio::test]
async fn duplicate_agent_sends_and_an_ordinary_mixed_forward_finish_under_the_same_lock_order() {
    use orch_api_contract::chats::{
        ForwardCreate, ForwardCreateBody, ForwardDestination, ForwardDestinationChat,
        ForwardDestinationThread, ForwardSource, ForwardSourceChat,
    };
    let s = Scene::new().await;
    let one = s.thread_by(&s.ada, "First review").await;
    let two = s.thread_by(&s.ada, "Second review").await;
    let chat = s.tree.topic.base.chat_id;
    let source = send(
        &s,
        &s.ada,
        chat,
        &ChatMessageCreateBody {
            parts: vec![text("Ordinary source")],
            reply_to: None,
            agent_review: None,
            client_id: "source".into(),
        },
    )
    .await
    .ok()
    .message;
    let parts = vec![
        text("Ask together "),
        agent(two.base.id),
        agent(one.base.id),
    ];
    let review = preview(&s, &s.ada, chat, &parts, None).await.ok();
    let body = ChatMessageCreateBody {
        parts,
        reply_to: None,
        agent_review: review.agent_review,
        client_id: "simultaneous-ask".into(),
    };
    let ordinary = ForwardCreateBody {
        from: ForwardSource::Chat(ForwardSourceChat {
            chat_id: chat,
            message_ids: vec![source.id],
        }),
        to: vec![
            ForwardDestination::Chat(ForwardDestinationChat {
                chat_id: one.base.chat_id,
            }),
            ForwardDestination::Thread(ForwardDestinationThread {
                thread_id: two.base.id,
            }),
            ForwardDestination::Thread(ForwardDestinationThread {
                thread_id: one.base.id,
            }),
        ],
        comment: None,
        client_id: "simultaneous-forward".into(),
    };
    let (first, duplicate, forwarded) = tokio::time::timeout(rig::PATIENCE, async {
        tokio::join!(
            send(&s, &s.ada, chat, &body),
            send(&s, &s.ada, chat, &body),
            async {
                s.rig
                    .world
                    .app
                    .call(ForwardCreate, &[])
                    .token(&s.ada.token)
                    .body(&ordinary)
                    .send()
                    .await
            }
        )
    })
    .await
    .expect("ordinary Forward and typed sends share one sorted thread-before-chat lock order");
    assert_eq!(
        first.ok().message.asked_agents,
        duplicate.ok().message.asked_agents
    );
    assert_eq!(forwarded.ok().threads.len(), 2);
    assert_eq!(input_count(&s).await, 4);
    assert_eq!(message_count(&s, chat).await, 2);
}
