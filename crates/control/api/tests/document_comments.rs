//! Discussions beside a document over HTTP: opened on a block or on words of it, answered, resolved, reacted to, edited and deleted; who may, what each change leaves of the text, and who is told.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use chrono::Duration;
use desk::{Desk, rewrite, text};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    DiscussionResolvedFilter, DocumentBlock, DocumentComment, DocumentCommentCreate,
    DocumentCommentCreateBody, DocumentCommentDelete, DocumentCommentReactionSet,
    DocumentCommentReactionSetBody, DocumentCommentUpdate, DocumentCommentUpdateBody,
    DocumentDiscussion, DocumentDiscussionCreate, DocumentDiscussionCreateBody,
    DocumentDiscussionList, DocumentDiscussionListQuery, DocumentDiscussionResolve,
    DocumentDiscussionResolveBody, DocumentGet, DocumentSyncGet, DocumentSyncGetQuery,
    DocumentVersionList, DocumentVersionListQuery, Inline,
};
use orch_api_contract::nodes::NodeArchive;
use orch_types::{Id, new_id};
use places::Who;

/// Ada's document with one block of words, and the block.
async fn page(desk: &Desk) -> (Id, DocumentBlock) {
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk
        .paragraphs(
            &desk.ada,
            made.base.id,
            &["The second paragraph says something."],
        )
        .await;
    (made.base.id, blocks[0].clone())
}

async fn open(
    desk: &Desk,
    by: &Who,
    document: Id,
    block: Id,
    quote: Option<&str>,
    words: &str,
) -> DocumentDiscussion {
    desk.world
        .app
        .call(DocumentDiscussionCreate, &[&document])
        .token(&by.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: block,
            quote: quote.map(str::to_owned),
            content: text(words),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
}

async fn answer(
    desk: &Desk,
    by: &Who,
    document: Id,
    discussion: Id,
    words: &str,
) -> DocumentComment {
    desk.world
        .app
        .call(DocumentCommentCreate, &[&document, &discussion])
        .token(&by.token)
        .body(&DocumentCommentCreateBody {
            content: text(words),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
}

async fn discussions(
    desk: &Desk,
    by: &Who,
    document: Id,
    resolved: Option<DiscussionResolvedFilter>,
) -> Vec<DocumentDiscussion> {
    desk.world
        .app
        .call(DocumentDiscussionList, &[&document])
        .token(&by.token)
        .query(&DocumentDiscussionListQuery {
            resolved,
            ..DocumentDiscussionListQuery::default()
        })
        .send()
        .await
        .ok()
        .items
}

async fn notices(desk: &Desk, user: Id) -> Vec<(String, String, bool)> {
    sqlx::query_as(
        "SELECT kind, body, read_at IS NOT NULL FROM notifications WHERE user_id = $1 ORDER BY inbox_seq",
    )
    .bind(user)
    .fetch_all(&desk.world.db.pool)
    .await
    .unwrap()
}

fn said(comment: &DocumentComment) -> String {
    comment
        .content
        .iter()
        .map(|run| match run {
            Inline::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect()
}

// ---------------------------------------------------------------- opening and answering

#[tokio::test]
async fn a_viewer_opens_a_discussion_on_words_of_a_block_and_it_is_anchored_for_as_long_as_they_are_there()
 {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let (document, block) = page(&desk).await;
    let opened = open(
        &desk,
        &vera,
        document,
        block.id,
        Some("second paragraph"),
        "Is this right?",
    )
    .await;
    assert_eq!(opened.block_id, Some(block.id));
    assert_eq!(opened.quote, "second paragraph");
    assert!(opened.anchored);
    assert!(opened.resolved.is_none());
    assert_eq!(opened.created_by.username, "vera");
    assert_eq!(opened.comments.len(), 1);
    assert_eq!(said(&opened.comments[0]), "Is this right?");
    // The words are changed: the discussion stays beside the block, no longer on the words.
    desk.write(
        &desk.ada,
        document,
        vec![rewrite(&block, "A different paragraph.")],
    )
    .await
    .ok();
    let after = discussions(&desk, &vera, document, None).await;
    assert_eq!(after[0].block_id, Some(block.id));
    assert!(!after[0].anchored);
    // The block is removed: it is about the document as a whole.
    let current = desk.page(&desk.ada, document).await;
    desk.write(&desk.ada, document, vec![desk::remove(&current[0])])
        .await
        .ok();
    let gone = discussions(&desk, &vera, document, None).await;
    assert_eq!(gone[0].block_id, None);
    assert!(!gone[0].anchored);
}

#[tokio::test]
async fn a_discussion_is_answered_and_the_one_with_the_newest_answer_is_listed_first() {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let (document, block) = page(&desk).await;
    let first = open(&desk, &vera, document, block.id, None, "First question").await;
    desk.world.advance(Duration::minutes(1));
    let second = open(
        &desk,
        &desk.ada,
        document,
        block.id,
        None,
        "Second question",
    )
    .await;
    desk.world.advance(Duration::minutes(1));
    let reply = answer(&desk, &desk.ada, document, first.id, "An answer").await;
    assert_eq!(reply.author.username, "ada");
    assert_eq!(said(&reply), "An answer");
    assert_eq!(reply.discussion_id, first.id);
    let list = discussions(&desk, &vera, document, None).await;
    assert_eq!(
        list.iter().map(|each| each.id).collect::<Vec<_>>(),
        [first.id, second.id]
    );
    assert_eq!(list[0].comments.len(), 2, "with all its comments");
    // Paged.
    let page_one = desk
        .world
        .app
        .call(DocumentDiscussionList, &[&document])
        .token(&vera.token)
        .query(&DocumentDiscussionListQuery {
            limit: Some(1),
            ..DocumentDiscussionListQuery::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(page_one.items.len(), 1);
    let page_two = desk
        .world
        .app
        .call(DocumentDiscussionList, &[&document])
        .token(&vera.token)
        .query(&DocumentDiscussionListQuery {
            limit: Some(1),
            cursor: page_one.next_cursor,
            ..DocumentDiscussionListQuery::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(page_two.items[0].id, second.id);
    assert_eq!(page_two.next_cursor, None);
}

#[tokio::test]
async fn a_request_sent_twice_answers_as_the_first_did_and_another_persons_identifier_is_refused() {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let (document, block) = page(&desk).await;
    let body = DocumentDiscussionCreateBody {
        block_id: block.id,
        quote: None,
        content: text("Once"),
        client_id: new_id().to_string(),
    };
    let send = |by: &Who| {
        let call = desk.world.app.call(DocumentDiscussionCreate, &[&document]);
        let call = call.token(&by.token).body(&body);
        async move { call.send().await }
    };
    let first = send(&vera).await.ok();
    let again = send(&vera).await.ok();
    assert_eq!(first.id, again.id);
    assert_eq!(discussions(&desk, &vera, document, None).await.len(), 1);
    let stolen = send(&desk.ada).await.problem();
    assert_eq!(stolen.code, ErrorCode::ConflictExists);
}

#[tokio::test]
async fn nothing_is_opened_on_a_block_that_is_not_there_and_what_is_said_has_to_say_something() {
    let desk = Desk::new().await;
    let (document, block) = page(&desk).await;
    let missing = desk
        .world
        .app
        .call(DocumentDiscussionCreate, &[&document])
        .token(&desk.ada.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: new_id(),
            quote: None,
            content: text("Where?"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::RequestInvalid);
    assert_eq!(missing.extra["errors"][0]["path"], "block_id");
    for content in [text("   "), Vec::new()] {
        let empty = desk
            .world
            .app
            .call(DocumentDiscussionCreate, &[&document])
            .token(&desk.ada.token)
            .body(&DocumentDiscussionCreateBody {
                block_id: block.id,
                quote: None,
                content,
                client_id: new_id().to_string(),
            })
            .send()
            .await
            .problem();
        assert_eq!(empty.code, ErrorCode::RequestInvalid);
        assert_eq!(empty.extra["errors"][0]["path"], "content");
    }
    let long = desk
        .world
        .app
        .call(DocumentDiscussionCreate, &[&document])
        .token(&desk.ada.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: block.id,
            quote: Some("q".repeat(401)),
            content: text("Hi"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .problem();
    assert_eq!(long.extra["errors"][0]["path"], "quote");
    assert!(
        discussions(
            &desk,
            &desk.ada,
            document,
            Some(DiscussionResolvedFilter::Include)
        )
        .await
        .is_empty()
    );
}

// ---------------------------------------------------------------- resolving

#[tokio::test]
async fn an_editor_or_whoever_wrote_in_a_discussion_resolves_it_and_opens_it_again() {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let walt = desk.member("walt", "viewer").await;
    let (document, block) = page(&desk).await;
    let opened = open(&desk, &vera, document, block.id, None, "Question").await;
    let resolve = |by: &Who, resolved: bool| {
        let call = desk
            .world
            .app
            .call(DocumentDiscussionResolve, &[&document, &opened.id]);
        let call = call
            .token(&by.token)
            .body(&DocumentDiscussionResolveBody { resolved });
        async move { call.send().await }
    };
    // A viewer who wrote nothing there may not; the one who asked may; an editor may.
    let refused = resolve(&walt, true).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "doc.edit");
    let resolved = resolve(&vera, true).await.ok();
    assert_eq!(resolved.resolved.as_ref().unwrap().by.username, "vera");
    assert_eq!(
        discussions(&desk, &vera, document, None).await.len(),
        0,
        "resolved ones are left out as shipped"
    );
    assert_eq!(
        discussions(&desk, &vera, document, Some(DiscussionResolvedFilter::Only))
            .await
            .len(),
        1
    );
    assert_eq!(
        discussions(
            &desk,
            &vera,
            document,
            Some(DiscussionResolvedFilter::Include)
        )
        .await
        .len(),
        1
    );
    let again = resolve(&desk.ada, false).await.ok();
    assert!(again.resolved.is_none(), "opened again");
    assert_eq!(discussions(&desk, &vera, document, None).await.len(), 1);
}

// ---------------------------------------------------------------- the comments

#[tokio::test]
async fn a_comment_is_edited_by_its_author_alone_and_deleted_by_them_or_whoever_manages_the_document()
 {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let walt = desk.member("walt", "viewer").await;
    let dev = desk.member("dev", "developer").await;
    let (document, block) = page(&desk).await;
    let opened = open(&desk, &vera, document, block.id, None, "Draft").await;
    let comment = opened.comments[0].id;
    let edit = |by: &Who, words: &str| {
        let call = desk
            .world
            .app
            .call(DocumentCommentUpdate, &[&document, &comment]);
        let call = call.token(&by.token).body(&DocumentCommentUpdateBody {
            content: text(words),
            client_id: None,
            expected_seq: None,
        });
        async move { call.send().await }
    };
    let refused = edit(&desk.ada, "Mine now").await.problem();
    assert_eq!(
        refused.code,
        ErrorCode::AuthzDenied,
        "even the owner of the document"
    );
    let edited = edit(&vera, "Better").await.ok();
    assert_eq!(said(&edited), "Better");
    assert!(edited.edited_at.is_some());
    let delete = |by: &Who| {
        let call = desk
            .world
            .app
            .call(DocumentCommentDelete, &[&document, &comment]);
        let call = call.token(&by.token);
        async move { call.send().await }
    };
    let refused = delete(&walt).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "doc.manage");
    assert_eq!(delete(&dev).await.status, 204);
    // It stays as an emptied line.
    let left = &discussions(&desk, &vera, document, None).await[0].comments;
    assert_eq!(left.len(), 1);
    assert!(left[0].deleted);
    assert!(left[0].content.is_empty());
    assert!(
        desk.world
            .audit()
            .await
            .contains(&"document.comment_deleted".to_owned()),
        "somebody else's comment deleted is written down"
    );
    // A deleted comment is not edited.
    assert_eq!(
        edit(&vera, "Back").await.problem().code,
        ErrorCode::NotFound
    );
    // Her own she deletes without anybody's leave, and that is no audit row.
    let second = answer(&desk, &vera, document, opened.id, "Another").await;
    let before = desk.world.audit().await.len();
    let own = desk
        .world
        .app
        .call(DocumentCommentDelete, &[&document, &second.id])
        .token(&vera.token)
        .send()
        .await;
    assert_eq!(own.status, 204);
    assert_eq!(desk.world.audit().await.len(), before);
}

#[tokio::test]
async fn reactions_are_put_on_and_taken_off_one_person_at_a_time() {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let (document, block) = page(&desk).await;
    let opened = open(&desk, &desk.ada, document, block.id, None, "Thoughts?").await;
    let comment = opened.comments[0].id;
    let react = |by: &Who, emoji: &str, on: bool| {
        let call = desk
            .world
            .app
            .call(DocumentCommentReactionSet, &[&document, &comment]);
        let call = call.token(&by.token).body(&DocumentCommentReactionSetBody {
            emoji: emoji.to_owned(),
            on,
        });
        async move { call.send().await }
    };
    react(&desk.ada, "👍", true).await.ok();
    let reacted = react(&vera, "👍", true).await.ok();
    assert_eq!(reacted.reactions.len(), 1);
    assert_eq!(reacted.reactions[0].emoji, "👍");
    assert_eq!(reacted.reactions[0].users, [desk.ada.id(), vera.id()]);
    let again = react(&vera, "👍", true).await.ok();
    assert_eq!(again.reactions[0].users.len(), 2, "twice is once");
    let off = react(&desk.ada, "👍", false).await.ok();
    assert_eq!(off.reactions[0].users, [vera.id()]);
    let none = react(&vera, "👍", false).await.ok();
    assert!(none.reactions.is_empty());
    let refused = react(&vera, "not an emoji", true).await.problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert_eq!(refused.extra["errors"][0]["path"], "emoji");
}

// ---------------------------------------------------------------- beside the text, and the archive

#[tokio::test]
async fn a_discussion_moves_the_sequence_of_the_document_and_leaves_its_text_its_versions_and_its_authors_alone()
 {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let (document, block) = page(&desk).await;
    let before = desk
        .world
        .app
        .call(DocumentGet, &[&document])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    desk.world.advance(Duration::hours(2));
    let opened = open(&desk, &vera, document, block.id, None, "Question").await;
    answer(&desk, &desk.ada, document, opened.id, "Answer").await;
    let after = desk
        .world
        .app
        .call(DocumentGet, &[&document])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        after.base.seq,
        before.base.seq + 2,
        "each took the next number"
    );
    assert_eq!(after.base.updated_at, before.base.updated_at);
    assert_eq!(after.base.updated_by, before.base.updated_by);
    assert_eq!(after.base.version, before.base.version);
    assert_eq!(after.base.open_discussions, 1);
    // No version was cut because of it, and the one a later change cuts holds the text alone.
    let versions = desk
        .world
        .app
        .call(DocumentVersionList, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentVersionListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(versions.items.len(), 1, "the open span of the first batch");
    assert_eq!(versions.items[0].changed.updated, 0);
    // They arrive with the document, whole and from a position, as blocks do.
    let synced = desk
        .world
        .app
        .call(DocumentSyncGet, &[&document])
        .token(&vera.token)
        .query(&DocumentSyncGetQuery {
            since: Some(before.base.seq),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(synced.blocks.is_empty(), "no block changed");
    assert_eq!(synced.discussions.len(), 1);
    assert_eq!(synced.discussions[0].comments.len(), 2);
    assert_eq!(desk.sync(&vera, document).await.discussions.len(), 1);
}

#[tokio::test]
async fn under_an_archived_node_nothing_is_added_to_a_discussion_or_changed_in_one() {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let (document, block) = page(&desk).await;
    let opened = open(&desk, &vera, document, block.id, None, "Before").await;
    let comment = opened.comments[0].id;
    desk.world
        .app
        .call(NodeArchive, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&places::archive_body(&desk.world, &desk.ada.token, desk.topic, true).await)
        .send()
        .await
        .ok();
    let refused = [
        desk.world
            .app
            .call(DocumentDiscussionCreate, &[&document])
            .token(&vera.token)
            .body(&DocumentDiscussionCreateBody {
                block_id: block.id,
                quote: None,
                content: text("After"),
                client_id: new_id().to_string(),
            })
            .send()
            .await
            .problem(),
        desk.world
            .app
            .call(DocumentCommentCreate, &[&document, &opened.id])
            .token(&vera.token)
            .body(&DocumentCommentCreateBody {
                content: text("After"),
                client_id: new_id().to_string(),
            })
            .send()
            .await
            .problem(),
        desk.world
            .app
            .call(DocumentCommentReactionSet, &[&document, &comment])
            .token(&vera.token)
            .body(&DocumentCommentReactionSetBody {
                emoji: "👍".to_owned(),
                on: true,
            })
            .send()
            .await
            .problem(),
        desk.world
            .app
            .call(DocumentCommentUpdate, &[&document, &comment])
            .token(&vera.token)
            .body(&DocumentCommentUpdateBody {
                content: text("Edited"),
                client_id: None,
                expected_seq: None,
            })
            .send()
            .await
            .problem(),
        desk.world
            .app
            .call(DocumentDiscussionResolve, &[&document, &opened.id])
            .token(&vera.token)
            .body(&DocumentDiscussionResolveBody { resolved: true })
            .send()
            .await
            .problem(),
    ];
    for problem in refused {
        assert_eq!(problem.code, ErrorCode::NodeArchived, "{}", problem.detail);
    }
    let delete = desk
        .world
        .app
        .call(DocumentCommentDelete, &[&document, &comment])
        .token(&vera.token)
        .send()
        .await
        .problem();
    assert_eq!(delete.code, ErrorCode::NodeArchived);
    // They are read all the same.
    let read = discussions(&desk, &vera, document, None).await;
    assert_eq!(read[0].comments.len(), 1);
    assert_eq!(said(&read[0].comments[0]), "Before");
}

// ---------------------------------------------------------------- who is told

#[tokio::test]
async fn an_answer_tells_whoever_started_the_discussion_and_whoever_wrote_in_it_in_one_notice_each_that_every_answer_renews()
 {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let walt = desk.member("walt", "viewer").await;
    let (document, block) = page(&desk).await;
    let opened = open(
        &desk,
        &desk.ada,
        document,
        block.id,
        None,
        "What do you think?",
    )
    .await;
    assert!(
        notices(&desk, desk.ada.id()).await.is_empty(),
        "nobody was answered yet"
    );
    answer(&desk, &vera, document, opened.id, "I like it").await;
    let told = notices(&desk, desk.ada.id()).await;
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].0, "doc.comment");
    assert_eq!(told[0].1, "I like it");
    assert!(
        notices(&desk, vera.id()).await.is_empty(),
        "not the one who answered"
    );
    // The next answer renews the one notice of each, and tells the one who wrote before.
    desk.world.advance(Duration::minutes(1));
    answer(&desk, &walt, document, opened.id, "Me too").await;
    let told = notices(&desk, desk.ada.id()).await;
    assert_eq!(told.len(), 1, "one notice for the discussion");
    assert_eq!(told[0].1, "Me too");
    assert_eq!(notices(&desk, vera.id()).await[0].1, "Me too");
    // Once it is read, the next answer makes it unread again.
    sqlx::query("UPDATE notifications SET read_at = now() WHERE user_id = $1")
        .bind(desk.ada.id())
        .execute(&desk.world.db.pool)
        .await
        .unwrap();
    assert!(notices(&desk, desk.ada.id()).await[0].2, "read");
    answer(&desk, &vera, document, opened.id, "One more thing").await;
    let told = notices(&desk, desk.ada.id()).await;
    assert_eq!(told.len(), 1);
    assert!(!told[0].2, "unread again");
    assert_eq!(told[0].1, "One more thing");
}

#[tokio::test]
async fn naming_somebody_in_a_comment_does_not_notify_or_subscribe_them() {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let (document, block) = page(&desk).await;
    let mention = |words: &str| {
        vec![
            Inline::Text {
                text: format!("{words} "),
                marks: None,
                href: None,
                color: None,
                background: None,
            },
            Inline::Mention {
                user_id: vera.id(),
                user: None,
            },
        ]
    };
    let opened = desk
        .world
        .app
        .call(DocumentDiscussionCreate, &[&document])
        .token(&desk.ada.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: block.id,
            quote: None,
            content: mention("Over to"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let told = notices(&desk, vera.id()).await;
    assert!(
        told.is_empty(),
        "naming a reader does not send a notice: {told:?}"
    );
    // A reader of the comment is given who is meant.
    match &opened.comments[0].content[1] {
        Inline::Mention { user, .. } => assert_eq!(user.as_ref().unwrap().username, "vera"),
        other => panic!("not a mention: {other:?}"),
    }
    desk.world
        .app
        .call(DocumentCommentCreate, &[&document, &opened.id])
        .token(&desk.ada.token)
        .body(&DocumentCommentCreateBody {
            content: mention("Again,"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let told = notices(&desk, vera.id()).await;
    assert!(
        told.is_empty(),
        "a mention alone does not subscribe the reader: {told:?}"
    );
}
