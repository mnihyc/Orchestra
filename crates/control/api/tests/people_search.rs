//! Current attachment and mention filters apply before paging, and a matching document block keeps its own writer.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod desk;
mod places;
mod support;

use desk::{Desk, paragraph, put};
use orch_api_contract::{
    common::Page,
    documents::{Author, BlockContent, DocumentFileCreate, DocumentFileCreateQuery, Inline},
    search::{SearchFound, SearchHit, SearchList, SearchListQuery},
    threads::FILENAME_HEADER,
};
use serde_json::json;

fn query() -> SearchListQuery {
    serde_json::from_value(json!({"q":"needle", "limit":1})).unwrap()
}

#[tokio::test]
async fn a_file_filter_fills_its_first_page_and_names_the_matching_blocks_writer_not_the_last_document_editor()
 {
    let desk = Desk::new().await;
    let writer = desk.member("writer", "interactor").await;
    let doc = desk.make(&desk.ada, "Library").await;
    let matched = desk
        .paragraphs(&writer, doc.base.id, &["needle from the block writer"])
        .await[0]
        .id;
    desk.world
        .app
        .call(DocumentFileCreate, &[&doc.base.id])
        .token(&desk.ada.token)
        .query(&DocumentFileCreateQuery { overwrite: None })
        .header(FILENAME_HEADER, "evidence.txt")
        .bytes(b"file bytes")
        .send()
        .await
        .ok();
    desk.write(
        &desk.ada,
        doc.base.id,
        vec![put(paragraph("A different author changes something else"))],
    )
    .await
    .ok();
    for index in 0..8 {
        let empty = desk.make(&desk.ada, &format!("No file {index}")).await;
        desk.paragraphs(&desk.ada, empty.base.id, &["needle without an attachment"])
            .await;
    }
    let ask = SearchListQuery {
        with_file: Some(true),
        ..query()
    };
    let found: Page<SearchHit> = desk
        .world
        .app
        .call(SearchList, &[])
        .token(&desk.ada.token)
        .query(&ask)
        .send()
        .await
        .ok();
    assert_eq!(found.items.len(), 1);
    assert!(
        found.next_cursor.is_none(),
        "nonmatching newer entries do not affect paging"
    );
    assert!(
        matches!(&found.items[0].found, SearchFound::Document { document, block_id: Some(id), by: Some(Author::User(by)), .. }
        if document.id == doc.base.id && *id == matched && by.id == writer.id())
    );
}

#[tokio::test]
async fn mention_and_chat_filters_use_current_stored_recipients_and_do_not_search_an_inspected_chat()
 {
    let room = chat_world::Room::new().await;
    let direct = room.direct(&room.ada, &room.bea).await;
    let wanted = room.say(&room.ada, direct.base.id, "needle @bea").await;
    for _ in 0..5 {
        room.say(&room.ada, direct.base.id, "needle without a mention")
            .await;
    }
    room.say(&room.ada, room.chat, "needle @bea in another chat")
        .await;
    let ask = SearchListQuery {
        chat_id: Some(direct.base.id),
        mentions_me: Some(true),
        ..query()
    };
    let found = room
        .world
        .app
        .call(SearchList, &[])
        .token(&room.bea.token)
        .query(&ask)
        .send()
        .await
        .ok();
    assert_eq!(found.items.len(), 1);
    assert!(found.next_cursor.is_none());
    assert!(
        matches!(&found.items[0].found, SearchFound::ChatMessage { message, .. } if message.id == wanted.id)
    );
    let inspected = SearchListQuery {
        mentions_me: None,
        ..ask
    };
    assert!(
        room.world
            .app
            .call(SearchList, &[])
            .token(&room.dee.token)
            .query(&inspected)
            .send()
            .await
            .ok()
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn document_mentions_match_the_current_block_and_disappear_after_the_mention_is_removed() {
    let desk = Desk::new().await;
    let doc = desk.make(&desk.ada, "Mentions").await;
    let mut block = paragraph("needle ");
    block.content = Some(BlockContent::Inline(vec![
        Inline::Text {
            text: "needle ".to_owned(),
            marks: None,
            href: None,
            color: None,
            background: None,
        },
        Inline::Mention {
            user_id: desk.ada.id(),
            user: None,
        },
    ]));
    let made = desk
        .write(&desk.ada, doc.base.id, vec![put(block)])
        .await
        .ok()
        .blocks;
    let ask = SearchListQuery {
        mentions_me: Some(true),
        ..query()
    };
    assert_eq!(
        desk.world
            .app
            .call(SearchList, &[])
            .token(&desk.ada.token)
            .query(&ask)
            .send()
            .await
            .ok()
            .items
            .len(),
        1
    );
    desk.write(
        &desk.ada,
        doc.base.id,
        vec![desk::rewrite(
            &made[0],
            "needle still present, mention removed",
        )],
    )
    .await
    .ok();
    assert!(
        desk.world
            .app
            .call(SearchList, &[])
            .token(&desk.ada.token)
            .query(&ask)
            .send()
            .await
            .ok()
            .items
            .is_empty()
    );
}
