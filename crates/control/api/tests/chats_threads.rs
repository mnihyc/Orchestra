//! The chat of a thread over HTTP: items of the conversation quoted into it and counted on the item, who may quote, and the items of a thread forwarded into its own chat.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::chats::{
    ChatMessage, ChatMessageCreate, ChatMessageCreateBody, ChatMessageDelete, ChatMessageList,
    ChatMessageListQuery, ChatMessageUpdate, ChatMessageUpdateBody, ChatPart, ForwardCreate,
    ForwardCreateBody, ForwardDestination, ForwardDestinationChat, ForwardSource,
    ForwardSourceThread,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::{ThreadSyncGet, ThreadSyncGetQuery};
use orch_db::queries::access::PageKey;
use orch_db::queries::access::testing::add_entry;
use orch_types::{Id, new_id};
use places::Who;
use rig::Scene;
use std::time::Duration;

/// The items a thread has after one exchange: what the person said, what the agent said, and a marker.
async fn items_of(s: &Scene, thread: Id) -> Vec<(Id, String)> {
    sqlx::query_as("SELECT id, kind FROM items WHERE thread_id = $1 ORDER BY seq_created")
        .bind(thread)
        .fetch_all(&s.rig.world.db.pool)
        .await
        .unwrap()
}

async fn refs_of(s: &Scene, item: Id) -> (i32, i64) {
    sqlx::query_as("SELECT chat_refs, seq_updated FROM items WHERE id = $1")
        .bind(item)
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap()
}

fn quote(thread: Id, item: Id) -> ChatPart {
    ChatPart::ItemRef {
        thread_id: thread,
        item_id: item,
    }
}

fn text(words: &str) -> ChatPart {
    ChatPart::Text {
        text: words.to_owned(),
    }
}

async fn send(
    s: &Scene,
    by: &Who,
    chat: Id,
    parts: Vec<ChatPart>,
    client: &str,
) -> orch_api_core::testing::Reply<orch_api_contract::chats::ChatMessageCreateReply> {
    s.rig
        .world
        .app
        .call(ChatMessageCreate, &[&chat])
        .token(&by.token)
        .body(&ChatMessageCreateBody {
            agent_review: None,
            parts,
            reply_to: None,
            client_id: client.to_owned(),
        })
        .send()
        .await
}

async fn agent_message(s: &Scene, thread: Id) -> Id {
    items_of(s, thread)
        .await
        .into_iter()
        .find(|(_, kind)| kind == "agent_message")
        .map(|(id, _)| id)
        .unwrap()
}

async fn thread_sync_items(s: &Scene, thread: Id, since: Option<i64>) -> Vec<(Id, i64)> {
    s.rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadSyncGetQuery {
            since,
            limit: None,
            ..Default::default()
        })
        .send()
        .await
        .ok()
        .items
        .iter()
        .map(|item| (item.id, item.chat_refs))
        .collect()
}

#[tokio::test]
async fn an_item_quoted_into_the_chat_of_its_thread_is_counted_on_the_item_until_the_message_goes()
{
    let s = Scene::new().await;
    let thread = s.thread().await;
    let (id, chat) = (thread.base.id, thread.base.chat_id);
    s.exchange(id, 0, "Why does it loop?", "A stale cookie.")
        .await;
    let item = agent_message(&s, id).await;
    let (before_refs, before_seq) = refs_of(&s, item).await;
    assert_eq!(before_refs, 0);
    let thread_seq_before = thread_sync_items(&s, id, None).await;
    assert!(thread_seq_before.iter().all(|(_, refs)| *refs == 0));

    let quoted = send(
        &s,
        &s.ada,
        chat,
        vec![text("this one"), quote(id, item)],
        "q1",
    )
    .await
    .ok()
    .message;
    let (refs, seq) = refs_of(&s, item).await;
    assert_eq!(refs, 1);
    assert!(
        seq > before_seq,
        "the item is stamped in the thread's sequence"
    );
    // Whoever has the thread open learns of it through the thread's own feed.
    let changed = thread_sync_items(&s, id, Some(before_seq)).await;
    assert!(changed.contains(&(item, 1)), "{changed:?}");

    let second = send(&s, &s.cai, chat, vec![quote(id, item)], "q2")
        .await
        .ok()
        .message;
    assert_eq!(refs_of(&s, item).await.0, 2);
    // A message that quotes an item twice quotes it once.
    send(
        &s,
        &s.ada,
        chat,
        vec![quote(id, item), quote(id, item)],
        "q3",
    )
    .await
    .ok();
    assert_eq!(refs_of(&s, item).await.0, 3);

    let by_item = |item: Id| {
        let call = s
            .rig
            .world
            .app
            .call(ChatMessageList, &[&chat])
            .token(&s.cai.token)
            .query(&ChatMessageListQuery {
                item_id: Some(item),
                ..ChatMessageListQuery::default()
            });
        async move { call.send().await.ok().messages }
    };
    let found: Vec<Id> = by_item(item)
        .await
        .iter()
        .map(|m: &ChatMessage| m.id)
        .collect();
    assert_eq!(found.len(), 3);
    assert_eq!(found[0], quoted.id);
    assert!(by_item(new_id()).await.is_empty());

    // Taking the quote out of a message, and deleting a message, uncount it.
    let edited = s
        .rig
        .world
        .app
        .call(ChatMessageUpdate, &[&chat, &second.id])
        .token(&s.cai.token)
        .body(&ChatMessageUpdateBody {
            parts: vec![text("never mind")],
        })
        .send()
        .await
        .ok();
    assert_eq!(edited.parts.len(), 1);
    assert_eq!(refs_of(&s, item).await.0, 2);
    s.rig
        .world
        .app
        .call(ChatMessageDelete, &[&chat, &quoted.id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(refs_of(&s, item).await.0, 1);
    assert_eq!(by_item(item).await.len(), 1);
}

#[tokio::test]
async fn deleting_while_an_edit_adds_a_quote_preserves_the_reference_until_a_fresh_delete() {
    let s = Scene::new().await;
    let thread = s.thread().await;
    let (thread_id, chat) = (thread.base.id, thread.base.chat_id);
    s.exchange(thread_id, 0, "Question", "Answer").await;
    let item = agent_message(&s, thread_id).await;
    let message = send(&s, &s.ada, chat, vec![text("Before")], "delete-race")
        .await
        .ok()
        .message;

    // The edit takes the thread and chat locks in their usual order, but remains uncommitted.
    let mut first = s.rig.world.db.pool.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *first)
        .await
        .unwrap();
    let thread_seq = orch_db::sequences::next(&mut first, thread_id)
        .await
        .unwrap()
        .unwrap();
    let revision: i64 = sqlx::query_scalar(
        "UPDATE chats SET next_seq = next_seq + 1 WHERE id = $1 RETURNING next_seq - 1",
    )
    .bind(chat)
    .fetch_one(&mut *first)
    .await
    .unwrap();
    sqlx::query("UPDATE chat_messages SET content = $2, seq_updated = $3 WHERE id = $1")
        .bind(message.id)
        .bind(serde_json::to_value(vec![text("After"), quote(thread_id, item)]).unwrap())
        .bind(revision)
        .execute(&mut *first)
        .await
        .unwrap();
    orch_db::queries::chats::add_item_refs(&mut first, message.id, &[item])
        .await
        .unwrap();
    orch_db::queries::chats::count_item_quotes(
        &mut first,
        &[item],
        1,
        thread_seq,
        s.rig.world.now(),
    )
    .await
    .unwrap();

    let app = s.rig.world.app.clone();
    let token = s.ada.token.clone();
    let message_id = message.id;
    let pending = tokio::spawn(async move {
        app.call(ChatMessageDelete, &[&chat, &message_id])
            .token(&token)
            .send()
            .await
    });
    // A database lock wait proves the delete read the previous version before this edit commits.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = current_database() AND $1 = ANY(pg_blocking_pids(pid)))",
            )
            .bind(pid)
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the delete waits for the edit");
    first.commit().await.unwrap();
    let problem = pending.await.unwrap().problem();
    assert_eq!(problem.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(problem.extra["seq_updated"], revision);
    assert_eq!(refs_of(&s, item).await.0, 1);
    let live: bool =
        sqlx::query_scalar("SELECT deleted_at IS NULL FROM chat_messages WHERE id = $1")
            .bind(message.id)
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap();
    assert!(live);

    s.rig
        .world
        .app
        .call(ChatMessageDelete, &[&chat, &message.id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(refs_of(&s, item).await.0, 0);
    let links: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chat_item_refs WHERE message_id = $1")
            .bind(message.id)
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap();
    assert_eq!(links, 0);
}

#[tokio::test]
async fn a_thread_mention_is_hidden_from_the_inbox_when_the_thread_becomes_private() {
    use orch_api_contract::access::{PageChange, PageChangeWhole, PermissionPageUpdate};
    use orch_api_contract::notifications::{NotificationList, NotificationListQuery};
    use orch_api_contract::threads::Delivery;

    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.say(
        &s.ada,
        thread,
        "Please look @cai",
        Delivery::Queue,
        "mention",
    )
    .await
    .ok();
    let query = NotificationListQuery {
        kind: Some("mention".to_owned()),
        ..NotificationListQuery::default()
    };
    let inbox = || {
        s.rig
            .world
            .app
            .call(NotificationList, &[])
            .token(&s.cai.token)
            .query(&query)
            .send()
    };
    assert!(
        inbox()
            .await
            .ok()
            .items
            .iter()
            .any(|row| row.thread_id == Some(thread))
    );
    s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &thread])
        .token(&s.ada.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    assert!(
        inbox()
            .await
            .ok()
            .items
            .iter()
            .all(|row| row.thread_id != Some(thread))
    );
}

#[tokio::test]
async fn an_item_is_quoted_only_in_the_chat_of_its_own_thread() {
    let s = Scene::new().await;
    let thread = s.thread().await;
    let other = s.thread_by(&s.ada, "Another").await;
    s.exchange(thread.base.id, 0, "one", "first").await;
    s.exchange(other.base.id, 0, "two", "second").await;
    let item = agent_message(&s, thread.base.id).await;
    let theirs = agent_message(&s, other.base.id).await;
    // In the chat of the topic, which is no thread's.
    let topic_chat = s.tree.topic.base.chat_id;
    let wrong_place = send(
        &s,
        &s.ada,
        topic_chat,
        vec![quote(thread.base.id, item)],
        "a",
    )
    .await
    .problem();
    assert_eq!(wrong_place.code, ErrorCode::RequestInvalid);
    // An item of another thread, and one that is nowhere.
    for (wrong, client) in [
        (quote(other.base.id, theirs), "b"),
        (quote(thread.base.id, theirs), "c"),
        (quote(thread.base.id, new_id()), "d"),
    ] {
        let problem = send(&s, &s.ada, thread.base.chat_id, vec![wrong], client)
            .await
            .problem();
        assert_eq!(problem.code, ErrorCode::RequestInvalid, "{client}");
    }
    assert_eq!(refs_of(&s, item).await.0, 0);
    assert_eq!(refs_of(&s, theirs).await.0, 0);
}

#[tokio::test]
async fn somebody_who_reads_only_the_chat_of_a_thread_cannot_quote_its_items() {
    let s = Scene::new().await;
    let thread = s.thread().await;
    s.exchange(thread.base.id, 0, "one", "first").await;
    let item = agent_message(&s, thread.base.id).await;
    // Bea is a stranger named in the chat rows of the thread alone.
    {
        let mut conn = s.rig.world.conn().await;
        for permission in ["chat.view", "chat.interact"] {
            add_entry(
                &mut conn,
                &PageKey::new("node", thread.base.id, "live"),
                "any",
                permission,
                ("user", Some(s.bea.id()), None),
                None,
                s.rig.world.now(),
            )
            .await
            .unwrap();
        }
    }
    let plain = send(
        &s,
        &s.bea,
        thread.base.chat_id,
        vec![text("hello")],
        "plain",
    )
    .await;
    assert_eq!(plain.status, 201, "she writes in the chat");
    let quoted = send(
        &s,
        &s.bea,
        thread.base.chat_id,
        vec![quote(thread.base.id, item)],
        "quote",
    )
    .await
    .problem();
    assert_eq!(quoted.code, ErrorCode::AuthzDenied);
    assert_eq!(quoted.extra["required"], "thread.view");
    assert_eq!(refs_of(&s, item).await.0, 0);
    // Somebody who reads the thread quotes it.
    send(
        &s,
        &s.cai,
        thread.base.chat_id,
        vec![quote(thread.base.id, item)],
        "ok",
    )
    .await
    .ok();
}

#[tokio::test]
async fn items_of_a_thread_are_forwarded_into_its_own_chat_as_quotes_and_into_no_other_chat_yet() {
    let s = Scene::new().await;
    let thread = s.thread().await;
    s.exchange(thread.base.id, 0, "Why?", "Because.").await;
    let items = items_of(&s, thread.base.id).await;
    let said: Vec<Id> = items
        .iter()
        .filter(|(_, kind)| kind == "user_message" || kind == "agent_message")
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(said.len(), 2);
    let request = |items: &[Id], to: Id, client: &str| ForwardCreateBody {
        from: ForwardSource::Thread(ForwardSourceThread {
            thread_id: thread.base.id,
            item_ids: items.to_vec(),
        }),
        to: vec![ForwardDestination::Chat(ForwardDestinationChat {
            chat_id: to,
        })],
        comment: Some("look at this".to_owned()),
        client_id: client.to_owned(),
    };
    let call = |body: ForwardCreateBody, by: &Who| {
        let call = s
            .rig
            .world
            .app
            .call(ForwardCreate, &[])
            .token(&by.token)
            .body(&body);
        async move { call.send().await }
    };
    let sent = call(request(&said, thread.base.chat_id, "a"), &s.cai)
        .await
        .ok();
    let message = &sent.chats[0].message;
    assert_eq!(message.parts.len(), 3);
    assert!(matches!(&message.parts[0], ChatPart::Text { text } if text == "look at this"));
    assert_eq!(message.parts[1], quote(thread.base.id, said[0]));
    assert_eq!(message.parts[2], quote(thread.base.id, said[1]));
    for item in &said {
        assert_eq!(refs_of(&s, *item).await.0, 1, "the quotes stay live");
    }
    // Into another chat they are copies in the written form, by somebody who may write there.
    let elsewhere = call(request(&said, s.tree.topic.base.chat_id, "b"), &s.ada)
        .await
        .ok();
    assert!(
        elsewhere.chats[0]
            .message
            .parts
            .iter()
            .any(|part| matches!(part, ChatPart::Forwarded(_)))
    );
    for item in &said {
        assert_eq!(refs_of(&s, *item).await.0, 1, "a copy is no live quote");
    }
    // An approval or a line of history is not forwarded.
    let marker = items
        .iter()
        .find(|(_, kind)| kind == "marker")
        .map(|(id, _)| *id)
        .unwrap();
    let refused = call(request(&[marker], thread.base.chat_id, "c"), &s.cai)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    // Somebody who cannot view the thread forwards none of it.
    let stranger = call(request(&said, thread.base.chat_id, "d"), &s.bea)
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::AuthzDenied);
}
