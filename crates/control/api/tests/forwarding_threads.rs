//! The items of a conversation forwarded into a chat and to an agent's thread, and a conversation exported in its written form: the same words whoever reads them and however they travel.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::chats::{
    ChatPart, ForwardCreate, ForwardCreateBody, ForwardDestination, ForwardDestinationChat,
    ForwardDestinationThread, ForwardSource, ForwardSourceThread,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::{
    ContentPart, ExportDetail, ExportFormat, ForwardedAuthor, ForwardedFrom, Thread, ThreadExport,
    ThreadExportQuery,
};
use orch_types::Id;
use places::Who;
use rig::Scene;

async fn said_items(s: &Scene, thread: Id) -> Vec<Id> {
    sqlx::query_scalar(
        "SELECT id FROM items WHERE thread_id = $1 AND kind IN ('user_message', 'agent_message') ORDER BY seq_created",
    )
    .bind(thread)
    .fetch_all(&s.rig.world.db.pool)
    .await
    .unwrap()
}

fn request(
    thread: Id,
    items: &[Id],
    to: Vec<ForwardDestination>,
    comment: Option<&str>,
    client: &str,
) -> ForwardCreateBody {
    ForwardCreateBody {
        from: ForwardSource::Thread(ForwardSourceThread {
            thread_id: thread,
            item_ids: items.to_vec(),
        }),
        to,
        comment: comment.map(str::to_owned),
        client_id: client.to_owned(),
    }
}

fn chat(id: Id) -> ForwardDestination {
    ForwardDestination::Chat(ForwardDestinationChat { chat_id: id })
}

fn thread(id: Id) -> ForwardDestination {
    ForwardDestination::Thread(ForwardDestinationThread { thread_id: id })
}

async fn forward(
    s: &Scene,
    by: &Who,
    body: &ForwardCreateBody,
) -> orch_api_core::testing::Reply<orch_api_contract::chats::ForwardCreateReply> {
    s.rig
        .world
        .app
        .call(ForwardCreate, &[])
        .token(&by.token)
        .body(body)
        .send()
        .await
}

/// A conversation with one exchange, and another thread to forward to.
async fn two_threads() -> (Scene, Thread, Thread) {
    let s = Scene::new().await;
    let source = s.thread_by(&s.ada, "Fix login").await;
    s.exchange(source.base.id, 0, "Why does it loop?", "A stale cookie.")
        .await;
    let other = s.thread_by(&s.ada, "Notes").await;
    (s, source, other)
}

async fn exported(s: &Scene, by: &Who, thread: Id, detail: ExportDetail) -> String {
    let reply = s
        .rig
        .world
        .app
        .call(ThreadExport, &[&thread])
        .token(&by.token)
        .query(&ThreadExportQuery {
            continuation_id: None,
            projection_revision: None,
            format: ExportFormat::Markdown,
            detail: Some(detail),
            from: None,
            to: None,
            include: None,
        })
        .send_bytes()
        .await;
    assert_eq!(reply.status, 200);
    String::from_utf8(reply.raw.to_vec()).unwrap()
}

#[tokio::test]
async fn items_forwarded_into_another_chat_arrive_as_copies_in_the_written_form_that_an_export_has()
{
    let (s, source, other) = two_threads().await;
    let items = said_items(&s, source.base.id).await;
    assert_eq!(items.len(), 2);
    let made = forward(
        &s,
        &s.ada,
        &request(
            source.base.id,
            &items,
            vec![chat(other.base.chat_id)],
            Some("for the notes"),
            "a",
        ),
    )
    .await;
    assert_eq!(made.status, http::StatusCode::CREATED);
    let message = made.ok().chats.remove(0).message;
    assert_eq!(
        message.parts.len(),
        3,
        "the comment, a person's message, the agent's turn"
    );
    let ChatPart::Forwarded(person) = &message.parts[1] else {
        panic!("{:?}", message.parts[1]);
    };
    assert!(matches!(&person.author, Some(ForwardedAuthor::User(user)) if user.id == s.ada.id()));
    assert_eq!(person.text, "Why does it loop?");
    assert!(
        matches!(&person.from, ForwardedFrom::Thread { thread } if thread.id == source.base.id),
        "{:?}",
        person.from
    );
    let ChatPart::Forwarded(agent) = &message.parts[2] else {
        panic!("{:?}", message.parts[2]);
    };
    assert!(matches!(&agent.author, Some(ForwardedAuthor::Agent(_))));
    assert!(agent.text.contains("A stale cookie."), "{}", agent.text);
    // The words of the agent's part are words of the written form that a person exports.
    let whole = exported(&s, &s.ada, source.base.id, ExportDetail::Full).await;
    assert!(
        whole.contains(agent.text.trim()),
        "{whole}\n---\n{}",
        agent.text
    );
    assert!(whole.contains("Why does it loop?"));
    // The copy does not change when the original is quoted live: nothing in the source counts it.
    let refs: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(chat_refs), 0)::bigint FROM items WHERE thread_id = $1",
    )
    .bind(source.base.id)
    .fetch_one(&s.rig.world.db.pool)
    .await
    .unwrap();
    assert_eq!(refs, 0);
}

#[tokio::test]
async fn into_the_chat_of_their_own_thread_the_items_stay_live_quotes_beside_copies_elsewhere() {
    let (s, source, other) = two_threads().await;
    let items = said_items(&s, source.base.id).await;
    let made = forward(
        &s,
        &s.ada,
        &request(
            source.base.id,
            &items,
            vec![chat(other.base.chat_id), chat(source.base.chat_id)],
            None,
            "a",
        ),
    )
    .await
    .ok();
    assert_eq!(made.chats.len(), 2);
    let own = made
        .chats
        .iter()
        .find(|each| each.chat_id == source.base.chat_id)
        .unwrap();
    assert!(
        own.message
            .parts
            .iter()
            .all(|part| matches!(part, ChatPart::ItemRef { .. }))
    );
    let elsewhere = made
        .chats
        .iter()
        .find(|each| each.chat_id == other.base.chat_id)
        .unwrap();
    assert!(
        elsewhere
            .message
            .parts
            .iter()
            .all(|part| matches!(part, ChatPart::Forwarded(_)))
    );
}

#[tokio::test]
async fn items_forwarded_to_an_agents_thread_become_an_input_of_the_forwarder_that_is_queued() {
    let (s, source, other) = two_threads().await;
    let items = said_items(&s, source.base.id).await;
    let asked = request(
        source.base.id,
        &items,
        vec![thread(other.base.id)],
        Some("have a look"),
        "a",
    );
    let made = forward(&s, &s.ada, &asked).await;
    assert_eq!(made.status, http::StatusCode::CREATED);
    let made = made.ok();
    assert!(made.chats.is_empty());
    let input = &made.threads[0].input;
    assert_eq!(made.threads[0].thread_id, other.base.id);
    assert_eq!(input.author.as_ref().map(|by| by.id), Some(s.ada.id()));
    assert_eq!(input.delivery, orch_api_contract::threads::Delivery::Queue);
    assert!(matches!(&input.content[0], ContentPart::Text { text } if text == "have a look"));
    assert!(
        input.content[1..]
            .iter()
            .all(|part| matches!(part, ContentPart::Forwarded(_)))
    );
    let waiting: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_inputs WHERE thread_id = $1 AND author_id = $2",
    )
    .bind(other.base.id)
    .bind(s.ada.id())
    .fetch_one(&s.rig.world.db.pool)
    .await
    .unwrap();
    assert_eq!(waiting, 1);
    // Said again, it is the same input.
    let again = forward(&s, &s.ada, &asked).await;
    assert_eq!(again.status, http::StatusCode::OK);
    assert_eq!(again.ok().threads[0].input.id, input.id);
}

#[tokio::test]
async fn a_forward_is_all_or_nothing_and_names_the_destination_that_refused() {
    let (s, source, other) = two_threads().await;
    let items = said_items(&s, source.base.id).await;
    // Dan may drive the threads of the org and read them, and has no use of the provider of the other thread.
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_messages WHERE chat_id = $1")
        .bind(other.base.chat_id)
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap();
    // Where the place refuses what is written by somebody who may not use a thread's model (rather
    // than holding it, as shipped), the thread is a destination that refuses.
    sqlx::query("INSERT INTO settings (node_id,key,value) VALUES ($1,'thread.inputs_without_use','\"refused\"'::jsonb)")
        .bind(s.tree.topic.base.id)
        .execute(&s.rig.world.db.pool)
        .await
        .unwrap();
    let refused = forward(
        &s,
        &s.dan,
        &request(
            source.base.id,
            &items,
            vec![chat(other.base.chat_id), thread(other.base.id)],
            None,
            "a",
        ),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied, "{refused:?}");
    assert_eq!(
        refused.extra["object"],
        serde_json::json!({ "thread_id": other.base.id })
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_messages WHERE chat_id = $1")
        .bind(other.base.chat_id)
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap();
    assert_eq!(after, before, "nothing is written anywhere");
}

#[tokio::test]
async fn somebody_who_cannot_read_the_conversation_forwards_nothing_of_it() {
    let (s, source, other) = two_threads().await;
    let items = said_items(&s, source.base.id).await;
    let refused = forward(
        &s,
        &s.bea,
        &request(
            source.base.id,
            &items,
            vec![thread(other.base.id)],
            None,
            "a",
        ),
    )
    .await
    .problem();
    assert!(
        matches!(refused.code, ErrorCode::AuthzDenied | ErrorCode::NotFound),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_selection_that_says_nothing_to_forward_is_refused() {
    let (s, source, other) = two_threads().await;
    sqlx::query(
        "INSERT INTO items (id, thread_id, segment_id, turn_id, stream, kind, status, payload, seq_created, seq_updated)
         SELECT $1, $2, current_segment_id, current_turn_id, 'main', 'reasoning', 'completed',
                '{\"summary\":\"thinking\",\"text_len\":8,\"hidden\":false}', 999, 999
           FROM threads WHERE node_id = $2",
    )
    .bind(orch_types::new_id())
    .bind(source.base.id)
    .execute(&s.rig.world.db.pool)
    .await
    .unwrap();
    let reasoning: Id =
        sqlx::query_scalar("SELECT id FROM items WHERE thread_id = $1 AND kind = 'reasoning'")
            .bind(source.base.id)
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap();
    let refused = forward(
        &s,
        &s.ada,
        &request(
            source.base.id,
            &[reasoning],
            vec![chat(other.base.chat_id)],
            None,
            "a",
        ),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn the_written_form_of_a_conversation_is_served_at_three_depths_and_with_the_chat_when_asked()
{
    let (s, source, _) = two_threads().await;
    let id = source.base.id;
    let brief = exported(&s, &s.cai, id, ExportDetail::Brief).await;
    let normal = exported(&s, &s.cai, id, ExportDetail::Normal).await;
    let full = exported(&s, &s.cai, id, ExportDetail::Full).await;
    assert!(brief.starts_with("# Fix login\nThread · "), "{brief}");
    assert!(normal.contains("A stale cookie."));
    assert!(full.len() >= normal.len() && normal.len() >= brief.len());
    // What a person exports is the text that an agent which reads the conversation is given.
    let mut conn = s.rig.world.db.pool.acquire().await.unwrap();
    let read = orch_engine::written::read_conversation(
        &mut conn,
        id,
        (None, None),
        orch_engine::written::Detail::Normal,
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(normal, read.text);
    drop(conn);
    // The chat after the conversation, for somebody who may read it.
    let with_chat = s
        .rig
        .world
        .app
        .call(ThreadExport, &[&id])
        .token(&s.cai.token)
        .query(&ThreadExportQuery {
            continuation_id: None,
            projection_revision: None,
            format: ExportFormat::Markdown,
            detail: None,
            from: None,
            to: None,
            include: Some("chat".to_owned()),
        })
        .send_bytes()
        .await;
    assert_eq!(with_chat.status, 200);
    assert!(
        String::from_utf8(with_chat.raw.to_vec())
            .unwrap()
            .contains("Chat of Fix login")
    );
    // Nobody who may not read the thread has it.
    let refused = s
        .rig
        .world
        .app
        .call(ThreadExport, &[&id])
        .token(&s.bea.token)
        .query(&ThreadExportQuery {
            continuation_id: None,
            projection_revision: None,
            format: ExportFormat::Markdown,
            detail: None,
            from: None,
            to: None,
            include: None,
        })
        .send_bytes()
        .await;
    assert_eq!(refused.status, 403);
}
