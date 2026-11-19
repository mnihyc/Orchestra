//! A hundred authorized chat previews use one message query, skip system lines, hide deleted text and never name an unreadable chat or dereference its item quotes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use chat_world::Room;
use orch_api_contract::chats::{ChatListScope, OrgChatList, OrgChatListQuery};
use orch_types::new_id;
use serde_json::json;
use support::Statements;

#[tokio::test]
async fn a_page_carries_its_last_ordinary_messages_with_no_per_row_message_read() {
    let room = Room::new().await;
    let mut ids = Vec::new();
    let mut hidden = None;
    let pool = &room.world.db.pool;
    for n in 0..=100 {
        let chat = new_id();
        if n < 100 {
            ids.push(chat);
        } else {
            hidden = Some(chat);
        }
        sqlx::query(
            "INSERT INTO chats (id, kind, org_id, name, created_by, next_seq, last_message_seq, last_message_at)
             VALUES ($1, 'group', $2, $3, $4, 4, 3, now())",
        ).bind(chat).bind(room.root).bind(format!("Preview {n:03}")).bind(room.ada.id())
            .execute(pool).await.unwrap();
        for user in [room.ada.id(), room.cai.id()]
            .into_iter()
            .chain((n < 100).then_some(room.bea.id()))
        {
            sqlx::query("INSERT INTO chat_members (chat_id, user_id, added_by, seq_updated) VALUES ($1, $2, $3, 1)")
                .bind(chat).bind(user).bind(room.ada.id()).execute(pool).await.unwrap();
        }
        if n != 99 {
            let content = if n == 97 {
                json!([{ "type": "item_ref", "thread_id": new_id(), "item_id": new_id() }])
            } else {
                json!([{ "type": "text", "text": if n == 100 { "NEVER private".to_owned() } else { format!("shown-{n:03} Ω") } }])
            };
            sqlx::query(
                "INSERT INTO chat_messages (id, chat_id, kind, author_id, content, seq_created, seq_updated, deleted_at)
                 VALUES ($1, $2, 'message', $3, $4, 2, 2, CASE WHEN $5 THEN now() END)",
            ).bind(new_id()).bind(chat).bind(room.cai.id()).bind(content).bind(n == 98)
                .execute(pool).await.unwrap();
        }
        sqlx::query(
            "INSERT INTO chat_messages (id, chat_id, kind, content, seq_created, seq_updated)
             VALUES ($1, $2, 'system', $3, 3, 3)",
        )
        .bind(new_id())
        .bind(chat)
        .bind(json!({ "event": "renamed", "by": room.ada.id(), "name": "system-only" }))
        .execute(pool)
        .await
        .unwrap();
    }
    let mut conn = room.world.conn().await;
    let before = Statements::now();
    let previews = orch_db::queries::chats::last_messages(&mut conn, &ids)
        .await
        .unwrap();
    let queries = Statements::now() - before;
    assert_eq!(
        queries, 1,
        "all hundred previews are one bounded message query"
    );
    assert_eq!(
        previews.len(),
        99,
        "a system-only chat has no ordinary preview"
    );
    drop(conn);

    let page = room
        .world
        .app
        .call(OrgChatList, &[&room.root])
        .token(&room.bea.token)
        .query(&OrgChatListQuery {
            scope: Some(ChatListScope::Mine),
            limit: Some(100),
            ..OrgChatListQuery::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 100);
    assert!(page.items.iter().all(|chat| Some(chat.base.id) != hidden));
    let wire = serde_json::to_string(&page).unwrap();
    assert!(!wire.contains("NEVER private") && !wire.contains("shown-098"));
    for chat in &page.items {
        let name = chat.base.name.as_deref().unwrap();
        match name {
            "Preview 099" => assert!(chat.preview.is_none()),
            "Preview 098" => {
                let preview = chat.preview.as_ref().unwrap();
                assert!(preview.deleted && preview.text.is_empty());
            }
            "Preview 097" => assert!(chat.preview.as_ref().unwrap().text.is_empty()),
            _ => {
                let preview = chat.preview.as_ref().unwrap();
                assert_eq!(preview.text, format!("shown-{} Ω", &name[8..]));
                assert!(!preview.deleted);
                assert!(
                    matches!(&preview.author, Some(orch_api_contract::documents::Author::User(user)) if user.id == room.cai.id())
                );
            }
        }
    }
}
