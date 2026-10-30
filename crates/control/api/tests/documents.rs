//! A document over HTTP as a thing: made beside a node, listed, read whole, renamed, moved, deleted and restored, private until shared, read-only under an archive, and who is refused each.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::{Desk, body, paragraph, said};
use orch_api_contract::access::{
    AccessChoice, HolderInput, PageChange, PageChangeName, PageChangeWhole, PermissionPageGet,
    PermissionPageUpdate,
};
use orch_api_contract::common::{ErrorCode, Listed};
use orch_api_contract::documents::{
    BlockType, DeletedFilter, DocumentDelete, DocumentFileDelete, DocumentGet, DocumentMove,
    DocumentMoveBody, DocumentMovePreview, DocumentRestore, DocumentSeen, DocumentSeenBody,
    DocumentSyncGet, DocumentSyncGetQuery, DocumentUpdate, DocumentUpdateBody, Inline,
    NewDocumentAccess, NodeDocumentList, NodeDocumentListQuery,
};
use orch_api_contract::nodes::{FieldSource, NodeArchive, PageState};
use orch_settings::Target;
use orch_types::{Id, new_id};
use places::{make_org, missing_permissions};
use serde_json::json;
use std::collections::HashSet;

// ---------------------------------------------------------------- making one

#[tokio::test]
async fn independently_created_equal_titles_keep_distinct_paths_and_owners() {
    let desk = Desk::new().await;
    let first = desk.make(&desk.ada, "Untitled").await;
    let (left, right) = tokio::join!(
        desk.make_body(&desk.ada, body("Untitled")),
        desk.make_body(&desk.ada, body("Untitled"))
    );
    let left = left.ok();
    let right = right.ok();
    let ids = [first.base.id, left.base.id, right.base.id];
    assert_eq!(ids.into_iter().collect::<HashSet<_>>().len(), 3);
    for made in [&first, &left, &right] {
        assert_eq!(made.base.title, "Untitled");
        assert_eq!(made.base.node.id, desk.topic);
        assert_eq!(made.base.owner.id, desk.ada.id());
        assert!(made.access.owns);
        assert!(
            made.access
                .permissions
                .iter()
                .any(|held| held == "doc.edit")
        );
    }
    let names: Vec<String> = sqlx::query_scalar("SELECT basename FROM documents WHERE id=ANY($1)")
        .bind(ids.as_slice())
        .fetch_all(&desk.world.db.pool)
        .await
        .unwrap();
    assert_eq!(names.len(), 3);
    assert_eq!(
        names
            .iter()
            .map(|name| name.to_ascii_lowercase())
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    assert!(names.iter().any(|name| name == "Untitled.md"));
}

#[tokio::test]
async fn whoever_may_create_makes_a_document_beside_a_node_and_owns_it() {
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let made = desk.make(&ines, "Handbook").await;
    assert_eq!(made.base.title, "Handbook");
    assert_eq!(made.base.owner.username, "ines");
    assert_eq!(made.base.node.id, desk.topic);
    assert_eq!(made.path.last().map(|node| node.id), Some(desk.topic));
    assert_eq!(made.path.first().map(|node| node.id), Some(desk.org));
    assert_eq!(
        (made.base.seq, made.base.block_count, made.base.version),
        (0, 0, 0)
    );
    assert_eq!(made.base.page, PageState::Follows);
    assert!(made.access.owns);
    for permission in ["doc.view", "doc.edit", "doc.manage"] {
        assert!(
            made.access
                .permissions
                .iter()
                .any(|held| held == permission),
            "{permission} in {:?}",
            made.access.permissions
        );
    }
    assert_eq!(made.limits.bytes, 2 * 1024 * 1024);
    assert_eq!(made.limits.blocks, 5_000);
    assert!(
        desk.world
            .audit()
            .await
            .contains(&"document.created".to_owned()),
        "making a document is written down"
    );
}

#[tokio::test]
async fn somebody_who_holds_nothing_is_refused_making_a_document_and_is_told_what_is_missing() {
    let desk = Desk::new().await;
    let stranger = desk.stranger("sam").await;
    let refused = desk.make_body(&stranger, body("Nope")).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "doc.create");
    assert!(missing_permissions(&refused).contains(&"doc.create".to_owned()));
    assert!(
        desk.world
            .audit()
            .await
            .iter()
            .all(|action| action != "document.created"),
        "nothing was made"
    );
}

#[tokio::test]
async fn a_viewer_makes_a_document_as_shipped_and_owns_it_whatever_she_holds_on_the_others_there() {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let ines = desk.member("ines", "interactor").await;
    let made = desk.make(&vera, "Hers").await;
    assert!(made.access.owns);
    assert_eq!(made.base.owner.username, "vera");
    // She owns it, so she writes in it, though a viewer does not write in the documents of others.
    desk.paragraphs(&vera, made.base.id, &["mine"]).await;
    let others = desk.make(&desk.ada, "Ada's").await;
    let refused = desk
        .write(&vera, others.base.id, vec![desk::put(paragraph("no"))])
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "doc.edit");
    // An interactor writes in hers, as the node's page gives them.
    desk.write(&ines, made.base.id, vec![desk::put(paragraph("ines"))])
        .await
        .ok();
}

#[tokio::test]
async fn a_document_made_from_blocks_holds_them_in_the_order_they_were_given() {
    let desk = Desk::new().await;
    let made = desk
        .make_body(
            &desk.ada,
            orch_api_contract::documents::NodeDocumentCreateBody {
                title: "Plan".to_owned(),
                blocks: Some(vec![paragraph("one"), paragraph("two"), paragraph("three")]),
                markdown: None,
                access: None,
            },
        )
        .await
        .ok();
    assert_eq!(made.base.block_count, 3);
    assert_eq!(
        desk.words(&desk.ada, made.base.id).await,
        ["one", "two", "three"]
    );
}

#[tokio::test]
async fn a_document_made_from_markdown_reads_headings_lists_code_and_the_people_named() {
    let desk = Desk::new().await;
    let bobby = desk.member("bobby", "viewer").await;
    let made = desk
        .make_body(
            &desk.ada,
            orch_api_contract::documents::NodeDocumentCreateBody {
                title: "Imported".to_owned(),
                blocks: None,
                markdown: Some(
                    "# Plans\n\nShip it, @bobby.\n\n- one\n- two\n\n```rust\nfn main() {}\n```\n"
                        .to_owned(),
                ),
                access: None,
            },
        )
        .await
        .ok();
    let page = desk.page(&desk.ada, made.base.id).await;
    let types: Vec<BlockType> = page.iter().map(|block| block.block_type).collect();
    assert_eq!(
        types,
        [
            BlockType::Heading,
            BlockType::Paragraph,
            BlockType::Bullet,
            BlockType::Bullet,
            BlockType::Code
        ]
    );
    let named = match &page[1].content {
        orch_api_contract::documents::BlockContent::Inline(runs) => runs
            .iter()
            .find_map(|run| match run {
                Inline::Mention { user_id, user } => Some((*user_id, user.clone())),
                _ => None,
            })
            .expect("a mention"),
        other => panic!("not text: {other:?}"),
    };
    assert_eq!(named.0, bobby.id());
    assert_eq!(
        named.1.unwrap().username,
        "bobby",
        "a reader is given who is meant"
    );
}

#[tokio::test]
async fn blocks_and_markdown_together_and_a_title_that_is_empty_or_too_long_are_refused() {
    let desk = Desk::new().await;
    let both = desk
        .make_body(
            &desk.ada,
            orch_api_contract::documents::NodeDocumentCreateBody {
                title: "Both".to_owned(),
                blocks: Some(vec![paragraph("x")]),
                markdown: Some("y".to_owned()),
                access: None,
            },
        )
        .await
        .problem();
    assert_eq!(both.code, ErrorCode::RequestInvalid);
    assert_eq!(both.extra["errors"][0]["path"], "markdown");
    for title in ["", "   ", &"t".repeat(201)] {
        let refused = desk.make_body(&desk.ada, body(title)).await.problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid, "{title:?}");
        assert_eq!(refused.extra["errors"][0]["path"], "title");
    }
    assert!(
        desk.make_body(&desk.ada, body(&"t".repeat(200)))
            .await
            .body
            .is_ok(),
        "two hundred characters are allowed"
    );
}

#[tokio::test]
async fn documents_switched_off_at_a_node_are_not_made_there_and_the_refusal_names_the_node() {
    let desk = Desk::new().await;
    desk.world
        .set_at(Target::Node(desk.project), "docs.enabled", json!(false))
        .await;
    let refused = desk.make_body(&desk.ada, body("Nope")).await.problem();
    assert_eq!(refused.code, ErrorCode::DocsDisabled);
    assert_eq!(refused.extra["scope"]["id"], json!(desk.project));
    // Switched on again, they are made.
    desk.world
        .set_at(Target::Node(desk.project), "docs.enabled", json!(true))
        .await;
    assert!(desk.make_body(&desk.ada, body("Yes")).await.body.is_ok());
}

// ---------------------------------------------------------------- private until shared

#[tokio::test]
async fn a_private_document_is_its_owners_alone_until_the_owner_names_somebody() {
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let vera = desk.member("vera", "viewer").await;
    let dev = desk.member("dev", "developer").await;
    let mut private = body("Draft");
    private.access = Some(NewDocumentAccess::Private);
    let made = desk.make_body(&ines, private).await.ok();
    assert_eq!(made.base.page, PageState::Restricted);

    // The owner reads it; nobody else does, a developer among them.
    for who in [&vera, &dev] {
        let refused = desk
            .world
            .app
            .call(DocumentGet, &[&made.base.id])
            .token(&who.token)
            .send()
            .await
            .problem();
        assert_eq!(
            refused.code,
            ErrorCode::AuthzDenied,
            "{}",
            who.person.username
        );
    }
    assert!(
        desk.world
            .app
            .call(DocumentGet, &[&made.base.id])
            .token(&ines.token)
            .send()
            .await
            .body
            .is_ok()
    );
    let listed = |who: &places::Who| {
        let call = desk.world.app.call(NodeDocumentList, &[&desk.topic]);
        let call = call
            .token(&who.token)
            .query(&NodeDocumentListQuery::default());
        async move { call.send().await.ok().items }
    };
    assert_eq!(listed(&ines).await.len(), 1);
    assert!(
        listed(&vera).await.is_empty(),
        "a document she may not read is not listed to her"
    );

    // Sharing is the page, by the same request and the same shortcut as for anything.
    let shared = desk
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &made.base.id])
        .token(&ines.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: vera.id() }],
                choice: AccessChoice::FullView,
                export: None,
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        shared.owner.as_ref().map(|owner| owner.username.as_str()),
        Some("ines")
    );
    assert_eq!(listed(&vera).await.len(), 1, "now she sees it");
    // And making it follow the node again is the page too.
    let reset = desk
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &made.base.id])
        .token(&ines.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Reset),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(reset.state, PageState::Follows);
    assert_eq!(listed(&vera).await.len(), 1);
}

#[tokio::test]
async fn the_page_of_a_document_is_read_by_whoever_may_read_it_and_changed_by_those_who_manage_it()
{
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let vera = desk.member("vera", "viewer").await;
    let sam = desk.stranger("sam").await;
    let made = desk.make(&ines, "Handbook").await;
    let page = desk
        .world
        .app
        .call(PermissionPageGet, &[&"documents", &made.base.id])
        .token(&vera.token)
        .send()
        .await
        .ok();
    assert_eq!(page.object.id, made.base.id);
    assert_eq!(
        page.owner.as_ref().map(|owner| owner.username.as_str()),
        Some("ines")
    );
    let refused = desk
        .world
        .app
        .call(PermissionPageGet, &[&"documents", &made.base.id])
        .token(&sam.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let change = PageChange {
        whole: Some(PageChangeWhole::Private),
        ..PageChange::default()
    };
    let refused = desk
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &made.base.id])
        .token(&vera.token)
        .body(&change)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "access.manage");
}

// ---------------------------------------------------------------- listing and reading

#[tokio::test]
async fn the_documents_beside_a_node_are_listed_most_recently_changed_first_a_page_at_a_time() {
    let desk = Desk::new().await;
    let first = desk.make(&desk.ada, "First").await;
    let second = desk.make(&desk.ada, "Second").await;
    let third = desk.make(&desk.ada, "Third").await;
    // Writing in the first makes it the most recently changed.
    desk.world.advance(chrono::Duration::minutes(1));
    desk.paragraphs(&desk.ada, first.base.id, &["hello"]).await;
    let list = |limit: Option<u32>, cursor: Option<String>| {
        let call = desk.world.app.call(NodeDocumentList, &[&desk.topic]);
        let call = call.token(&desk.ada.token).query(&NodeDocumentListQuery {
            limit,
            cursor,
            ..NodeDocumentListQuery::default()
        });
        async move { call.send().await.ok() }
    };
    let all = list(None, None).await;
    let titles: Vec<&str> = all
        .items
        .iter()
        .filter_map(Listed::as_visible)
        .map(|each| each.title.as_str())
        .collect();
    assert_eq!(titles, ["First", "Third", "Second"]);
    assert_eq!(all.next_cursor, None);
    assert_eq!(all.items[0].as_visible().unwrap().block_count, 1);
    assert_eq!(all.items[0].as_visible().unwrap().seq, 1);
    let page_one = list(Some(2), None).await;
    assert_eq!(page_one.items.len(), 2);
    let page_two = list(Some(2), page_one.next_cursor.clone()).await;
    assert_eq!(
        page_two
            .items
            .iter()
            .filter_map(Listed::as_visible)
            .map(|each| each.id)
            .collect::<Vec<_>>(),
        [second.base.id]
    );
    assert_eq!(page_two.next_cursor, None);
    let _ = third;
}

#[tokio::test]
async fn a_list_of_a_place_holds_the_documents_of_the_nodes_below_it_when_asked() {
    let desk = Desk::new().await;
    desk.make(&desk.ada, "In the topic").await;
    let elsewhere = desk.another_topic("Elsewhere").await;
    desk.make_at(&desk.ada, elsewhere, body("In another topic"))
        .await
        .ok();
    let at = |node: Id, under: Option<bool>| {
        let call = desk.world.app.call(NodeDocumentList, &[&node]);
        let call = call.token(&desk.ada.token).query(&NodeDocumentListQuery {
            under,
            ..NodeDocumentListQuery::default()
        });
        async move { call.send().await.ok().items }
    };
    assert_eq!(at(desk.topic, None).await.len(), 1);
    assert_eq!(
        at(desk.project, None).await.len(),
        0,
        "beside the project there are none"
    );
    assert_eq!(at(desk.project, Some(true)).await.len(), 2);
    assert_eq!(at(desk.org, Some(true)).await.len(), 2);
}

#[tokio::test]
async fn a_stranger_lists_nothing_beside_a_node_and_is_refused_reading_one() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let sam = desk.stranger("sam").await;
    let refused = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&sam.token)
        .query(&NodeDocumentListQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "topic.view");
    let refused = desk
        .world
        .app
        .call(DocumentGet, &[&made.base.id])
        .token(&sam.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "doc.view");
}

#[tokio::test]
async fn a_document_is_read_whole_and_a_reader_is_given_names_for_who_wrote_what() {
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let vera = desk.member("vera", "viewer").await;
    let made = desk.make(&ines, "Handbook").await;
    desk.paragraphs(&ines, made.base.id, &["alpha", "beta"])
        .await;
    let synced = desk.sync(&vera, made.base.id).await;
    assert_eq!(synced.seq, 1);
    assert!(synced.complete && synced.since.is_none());
    assert_eq!(synced.document.base.block_count, 2);
    assert!(synced.document.base.updated_by.is_some());
    assert_eq!(synced.blocks.len(), 2);
    for block in &synced.blocks {
        let by = match block.updated_by.as_ref().unwrap() {
            orch_api_contract::documents::Author::User(user) => user.username.clone(),
            other => panic!("not a person: {other:?}"),
        };
        assert_eq!(by, "ines");
        assert!(!block.removed);
    }
    assert!(
        !synced
            .document
            .access
            .permissions
            .contains(&"doc.edit".to_owned())
    );
    assert!(
        synced
            .document
            .access
            .permissions
            .contains(&"doc.comment".to_owned())
    );
}

#[tokio::test]
async fn a_sync_from_a_position_holds_what_changed_after_it_removed_blocks_among_them() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk
        .paragraphs(&desk.ada, made.base.id, &["a", "b", "c"])
        .await;
    let before = desk.sync(&desk.ada, made.base.id).await.seq;
    desk.write(
        &desk.ada,
        made.base.id,
        vec![desk::rewrite(&blocks[0], "a!"), desk::remove(&blocks[2])],
    )
    .await
    .ok();
    let since = desk
        .world
        .app
        .call(DocumentSyncGet, &[&made.base.id])
        .token(&desk.ada.token)
        .query(&DocumentSyncGetQuery {
            since: Some(before),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(since.since, Some(before));
    assert!(since.complete);
    let mut changed: Vec<(String, bool)> = since
        .blocks
        .iter()
        .map(|block| (said(block), block.removed))
        .collect();
    changed.sort();
    assert_eq!(changed, [("a!".to_owned(), false), ("c".to_owned(), true)]);
    assert_eq!(
        since.document.base.seq,
        before + 1,
        "the document is always there"
    );
}

#[tokio::test]
async fn a_sync_from_below_what_was_purged_says_it_is_not_complete() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk.paragraphs(&desk.ada, made.base.id, &["a", "b"]).await;
    desk.write(&desk.ada, made.base.id, vec![desk::remove(&blocks[0])])
        .await
        .ok();
    // The removed block is purged for good, and the document remembers up to where.
    {
        let mut conn = desk.world.conn().await;
        let purged = orch_db::queries::documents::purge_removed_blocks(
            &mut conn,
            desk.world.now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();
        assert_eq!(purged, 1);
    }
    let behind = desk
        .world
        .app
        .call(DocumentSyncGet, &[&made.base.id])
        .token(&desk.ada.token)
        .query(&DocumentSyncGetQuery {
            since: Some(0),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(!behind.complete, "the client loads without a position");
    assert!(behind.blocks.is_empty());
    let loaded = desk.sync(&desk.ada, made.base.id).await;
    assert!(loaded.complete);
    assert_eq!(loaded.blocks.len(), 1);
}

#[tokio::test]
async fn how_far_a_person_has_had_a_document_on_screen_is_theirs_alone_and_never_moves_back() {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    desk.paragraphs(&desk.ada, made.base.id, &["a"]).await;
    let say = |who: &places::Who, seq: i64| {
        let call = desk.world.app.call(DocumentSeen, &[&made.base.id]);
        let call = call.token(&who.token).body(&DocumentSeenBody { seq });
        async move { call.send().await }
    };
    assert_eq!(say(&vera, 1).await.status, 204);
    assert_eq!(
        say(&vera, 0).await.status,
        204,
        "saying less changes nothing"
    );
    let read = |who: &places::Who| {
        let call = desk.world.app.call(DocumentGet, &[&made.base.id]);
        let call = call.token(&who.token);
        async move { call.send().await.ok().base.seen_seq }
    };
    assert_eq!(read(&vera).await, Some(1));
    assert_eq!(read(&desk.ada).await, None, "somebody else's is not hers");
    // A number that does not exist yet is cut to the sequence there is.
    say(&vera, 99).await.ok();
    assert_eq!(read(&vera).await, Some(1));
    let sam = desk.stranger("sam").await;
    assert_eq!(say(&sam, 1).await.problem().code, ErrorCode::AuthzDenied);
}

// ---------------------------------------------------------------- the document as a thing

#[tokio::test]
async fn an_editor_renames_a_document_and_a_viewer_cannot() {
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let vera = desk.member("vera", "viewer").await;
    let made = desk.make(&ines, "Handbook").await;
    let renamed = desk
        .world
        .app
        .call(DocumentUpdate, &[&made.base.id])
        .token(&ines.token)
        .body(&DocumentUpdateBody {
            title: Some("  Guide  ".to_owned()),
            description: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(renamed.base.title, "Guide");
    assert_eq!(renamed.base.seq, 1, "a rename is a change like any other");
    let refused = desk
        .world
        .app
        .call(DocumentUpdate, &[&made.base.id])
        .token(&vera.token)
        .body(&DocumentUpdateBody {
            title: Some("Mine".to_owned()),
            description: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "doc.edit");
    let empty = desk
        .world
        .app
        .call(DocumentUpdate, &[&made.base.id])
        .token(&ines.token)
        .body(&DocumentUpdateBody {
            title: Some(String::new()),
            description: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(empty.code, ErrorCode::RequestInvalid);
    assert!(
        desk.world
            .audit()
            .await
            .contains(&"document.renamed".to_owned())
    );
}

#[tokio::test]
async fn an_editor_describes_a_document_and_lists_show_its_first_line_or_else_its_first_paragraph()
{
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let vera = desk.member("vera", "viewer").await;
    let made = desk.make(&ines, "Handbook").await;
    assert_eq!(
        (
            made.base.description.as_str(),
            made.base.description_source,
            made.base.description_line.as_str()
        ),
        ("", FieldSource::Auto, ""),
        "a new document has no description and nothing to show for one"
    );
    desk.paragraphs(
        &ines,
        made.base.id,
        &[
            "Ship the redirect fix behind the flag.",
            "Then announce it.",
        ],
    )
    .await;
    let read = |who: &places::Who| {
        let call = desk
            .world
            .app
            .call(DocumentGet, &[&made.base.id])
            .token(&who.token);
        async move { call.send().await.ok() }
    };
    let undescribed = read(&vera).await;
    assert_eq!(undescribed.base.description, "");
    assert_eq!(
        undescribed.base.description_line, "Ship the redirect fix behind the flag.",
        "without a description a reader is given the first line of the first paragraph"
    );
    let describe = |who: &places::Who, text: String| {
        let call = desk
            .world
            .app
            .call(DocumentUpdate, &[&made.base.id])
            .token(&who.token)
            .body(&DocumentUpdateBody {
                title: None,
                description: Some(text),
            });
        async move { call.send().await }
    };
    let described = describe(&ines, "How we ship\nand how we roll back".to_owned())
        .await
        .ok();
    assert_eq!(described.base.title, "Handbook", "the title stays");
    assert_eq!(
        described.base.description,
        "How we ship\nand how we roll back"
    );
    assert_eq!(described.base.description_source, FieldSource::User);
    assert_eq!(described.base.description_line, "How we ship");
    assert_eq!(
        described.base.seq,
        undescribed.base.seq + 1,
        "a description is a change like any other"
    );
    let listed = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&vera.token)
        .query(&NodeDocumentListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        listed.items[0].as_visible().unwrap().description_line,
        "How we ship",
        "where documents are listed, the first line stands beside the title"
    );
    let refused = describe(&vera, "Mine".to_owned()).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "doc.edit");
    let long = describe(&ines, "x".repeat(4_001)).await.problem();
    assert_eq!(long.code, ErrorCode::RequestInvalid);
    let nothing = desk
        .world
        .app
        .call(DocumentUpdate, &[&made.base.id])
        .token(&ines.token)
        .body(&DocumentUpdateBody::default())
        .send()
        .await
        .problem();
    assert_eq!(nothing.code, ErrorCode::RequestInvalid);
    let cleared = describe(&ines, String::new()).await.ok();
    assert_eq!(
        (
            cleared.base.description.as_str(),
            cleared.base.description_source,
            cleared.base.description_line.as_str()
        ),
        (
            "",
            FieldSource::User,
            "Ship the redirect fix behind the flag."
        ),
        "taken away, the first paragraph is shown again, and automation does not write one"
    );
    assert_eq!(read(&vera).await.base.description, "");
}

#[tokio::test]
async fn a_document_is_deleted_by_whoever_manages_it_and_brought_back_by_them() {
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let dev = desk.member("dev", "developer").await;
    let made = desk.make(&ines, "Handbook").await;
    // An interactor made it, so owns it and manages it; a bystander developer manages anything here.
    let refused = {
        let vera = desk.member("vera", "viewer").await;
        desk.world
            .app
            .call(DocumentDelete, &[&made.base.id])
            .token(&vera.token)
            .send()
            .await
            .problem()
    };
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "doc.manage");

    let gone = desk
        .world
        .app
        .call(DocumentDelete, &[&made.base.id])
        .token(&dev.token)
        .send()
        .await;
    assert_eq!(gone.status, 204);
    let missing = desk
        .world
        .app
        .call(DocumentGet, &[&made.base.id])
        .token(&ines.token)
        .send()
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
    let live = desk
        .world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(&ines.token)
        .query(&NodeDocumentListQuery::default())
        .send()
        .await
        .ok();
    assert!(live.items.is_empty());
    // Whoever could have deleted it sees it among the deleted; somebody else does not.
    let deleted = |who: &places::Who| {
        let call = desk.world.app.call(NodeDocumentList, &[&desk.topic]);
        let call = call.token(&who.token).query(&NodeDocumentListQuery {
            deleted: Some(DeletedFilter::Only),
            ..NodeDocumentListQuery::default()
        });
        async move { call.send().await.ok().items }
    };
    let seen = deleted(&dev).await;
    assert_eq!(seen.len(), 1);
    assert!(seen[0].as_visible().unwrap().deleted);
    let vera = desk.member("viewer2", "viewer").await;
    assert!(deleted(&vera).await.is_empty());

    // Bringing it back needs what deleting needed, on the document as it was.
    let refused = desk
        .world
        .app
        .call(DocumentRestore, &[&made.base.id])
        .token(&vera.token)
        .raw_body(&serde_json::json!({}))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let back = desk
        .world
        .app
        .call(DocumentRestore, &[&made.base.id])
        .token(&dev.token)
        .raw_body(&serde_json::json!({}))
        .send()
        .await
        .ok();
    assert!(!back.base.deleted);
    assert!(
        desk.world
            .app
            .call(DocumentGet, &[&made.base.id])
            .token(&ines.token)
            .send()
            .await
            .body
            .is_ok()
    );
    let audit = desk.world.audit().await;
    assert!(audit.contains(&"document.deleted".to_owned()));
    assert!(audit.contains(&"document.restored".to_owned()));
    // What is not deleted is not restored.
    let nothing = desk
        .world
        .app
        .call(DocumentRestore, &[&made.base.id])
        .token(&dev.token)
        .raw_body(&serde_json::json!({}))
        .send()
        .await
        .problem();
    assert_eq!(nothing.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_document_moves_beside_another_node_of_its_tree_and_nowhere_else() {
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let made = desk.make(&ines, "Handbook").await;
    let elsewhere = desk.another_topic("Elsewhere").await;
    let moved = desk
        .world
        .app
        .call(DocumentMove, &[&made.base.id])
        .token(&ines.token)
        .body(&DocumentMoveBody { node_id: elsewhere })
        .send()
        .await
        .ok();
    assert_eq!(moved.base.node.id, elsewhere);
    assert_eq!(moved.path.last().map(|node| node.id), Some(elsewhere));
    assert!(
        desk.world
            .audit()
            .await
            .contains(&"document.moved".to_owned())
    );

    // Another tree of orgs is another set of people: refused, with the reason.
    let other = make_org(&desk.world, &desk.ada, "Other", "other").await;
    let refused = desk
        .world
        .app
        .call(DocumentMove, &[&made.base.id])
        .token(&desk.ada.token)
        .body(&DocumentMoveBody {
            node_id: other.base.id,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NodeWrongParent);
    // Beside the node it is beside, nothing changes.
    let same = desk
        .world
        .app
        .call(DocumentMove, &[&made.base.id])
        .token(&ines.token)
        .body(&DocumentMoveBody { node_id: elsewhere })
        .send()
        .await
        .ok();
    assert_eq!(same.base.node.id, elsewhere);
}

#[tokio::test]
async fn moving_takes_the_right_to_manage_it_and_the_right_to_create_where_it_goes() {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let elsewhere = desk.another_topic("Elsewhere").await;
    for call in [true, false] {
        let request = DocumentMoveBody { node_id: elsewhere };
        let refused = if call {
            desk.world
                .app
                .call(DocumentMove, &[&made.base.id])
                .token(&vera.token)
                .body(&request)
                .send()
                .await
                .problem()
        } else {
            desk.world
                .app
                .call(DocumentMovePreview, &[&made.base.id])
                .token(&vera.token)
                .body(&request)
                .send()
                .await
                .problem()
        };
        assert_eq!(refused.code, ErrorCode::AuthzDenied);
        assert_eq!(refused.extra["required"], "doc.manage");
    }
    // Managing it is not enough: where it goes, the right to create is asked too.
    let dev = desk.member("dev", "developer").await;
    let apart = desk.another_topic("Apart").await;
    desk.world
        .set_at(Target::Node(apart), "docs.enabled", json!(false))
        .await;
    let off = desk
        .world
        .app
        .call(DocumentMove, &[&made.base.id])
        .token(&dev.token)
        .body(&DocumentMoveBody { node_id: apart })
        .send()
        .await
        .problem();
    assert_eq!(off.code, ErrorCode::DocsDisabled, "documents are off there");
}

#[tokio::test]
async fn the_preview_of_a_move_says_who_would_gain_and_who_would_lose_what() {
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    // Somebody holds a role in the topic the document would go to, and nowhere else.
    let elsewhere = desk.another_topic("Elsewhere").await;
    let tess = desk.stranger("tess").await;
    places::give_role(&desk.world, elsewhere, &tess, "viewer", &desk.ada).await;
    let preview = desk
        .world
        .app
        .call(DocumentMovePreview, &[&made.base.id])
        .token(&desk.ada.token)
        .body(&DocumentMoveBody { node_id: elsewhere })
        .send()
        .await
        .ok();
    assert_eq!(preview.pages, 1);
    let gaining: Vec<&str> = preview
        .gaining
        .iter()
        .map(|person| person.user.username.as_str())
        .collect();
    assert!(gaining.contains(&"tess"), "{gaining:?}");
    assert!(
        preview
            .gaining
            .iter()
            .find(|person| person.user.username == "tess")
            .unwrap()
            .permissions
            .contains(&"doc.view".to_owned())
    );
    assert!(
        preview
            .losing
            .iter()
            .all(|person| person.user.username != "ines")
    );
    let _ = ines;
    // Nothing moved by asking.
    let read = desk
        .world
        .app
        .call(DocumentGet, &[&made.base.id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(read.base.node.id, desk.topic);
}

// ---------------------------------------------------------------- read-only under an archive

#[tokio::test]
async fn under_an_archived_node_a_document_is_read_but_nothing_in_it_is_changed() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk.paragraphs(&desk.ada, made.base.id, &["kept"]).await;
    desk.world
        .app
        .call(NodeArchive, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&places::archive_body(&desk.world, &desk.ada.token, desk.topic, true).await)
        .send()
        .await
        .ok();
    let read = desk
        .world
        .app
        .call(DocumentGet, &[&made.base.id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert!(read.base.archived);
    assert_eq!(desk.words(&desk.ada, made.base.id).await, ["kept"]);
    let refused = [
        desk.write(
            &desk.ada,
            made.base.id,
            vec![desk::rewrite(&blocks[0], "changed")],
        )
        .await
        .problem(),
        desk.world
            .app
            .call(DocumentUpdate, &[&made.base.id])
            .token(&desk.ada.token)
            .body(&DocumentUpdateBody {
                title: Some("New".to_owned()),
                description: None,
            })
            .send()
            .await
            .problem(),
        desk.world
            .app
            .call(DocumentDelete, &[&made.base.id])
            .token(&desk.ada.token)
            .send()
            .await
            .problem(),
        desk.make_body(&desk.ada, body("Another")).await.problem(),
    ];
    for problem in refused {
        assert_eq!(problem.code, ErrorCode::NodeArchived, "{}", problem.detail);
        assert_eq!(problem.extra["archived_node"]["id"], json!(desk.topic));
    }
    assert_eq!(desk.words(&desk.ada, made.base.id).await, ["kept"]);
}

// ---------------------------------------------------------------- files

#[tokio::test]
async fn a_file_is_taken_out_of_a_document_unless_a_live_block_names_it() {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let (loose, named) = (new_id(), new_id());
    {
        let mut conn = desk.world.conn().await;
        sqlx::query("INSERT INTO blobs (hash, size) VALUES ($1, 3) ON CONFLICT DO NOTHING")
            .bind(vec![7_u8; 32])
            .execute(&mut *conn)
            .await
            .ok();
        for (id, name) in [(loose, "loose.txt"), (named, "named.png")] {
            sqlx::query(
                "INSERT INTO document_files (id, document_id, blob, name, media_type, size, is_text, added_by)
                 VALUES ($1, $2, $3, $4, 'text/plain', 3, true, $5)",
            )
            .bind(id)
            .bind(made.base.id)
            .bind(vec![7_u8; 32])
            .bind(name)
            .bind(desk.ada.id())
            .execute(&mut *conn)
            .await
            .unwrap();
        }
    }
    let named_block = orch_api_contract::documents::NewBlock {
        id: new_id(),
        block_type: BlockType::File,
        props: Some(orch_api_contract::documents::BlockProps::File(
            orch_api_contract::documents::FileProps {
                display: None,
                file_id: named,
            },
        )),
        content: None,
        children: None,
    };
    desk.write(&desk.ada, made.base.id, vec![desk::put(named_block)])
        .await
        .ok();
    let delete = |who: &places::Who, file: Id| {
        let call = desk
            .world
            .app
            .call(DocumentFileDelete, &[&made.base.id, &file]);
        let call = call.token(&who.token);
        async move { call.send().await }
    };
    assert_eq!(
        delete(&vera, loose).await.problem().code,
        ErrorCode::AuthzDenied
    );
    let in_use = delete(&desk.ada, named).await.problem();
    assert_eq!(in_use.code, ErrorCode::ResourceInUse);
    assert_eq!(delete(&desk.ada, loose).await.status, 204);
    assert_eq!(
        delete(&desk.ada, loose).await.problem().code,
        ErrorCode::NotFound
    );
    let read = desk
        .world
        .app
        .call(DocumentGet, &[&made.base.id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        read.files
            .iter()
            .map(|file| file.name.as_str())
            .collect::<Vec<_>>(),
        ["named.png"]
    );
    assert_eq!(
        read.base.files_bytes, 3,
        "the size of the files it holds is counted again"
    );
}

// ---------------------------------------------------------------- what it needs, request by request

#[tokio::test]
async fn every_bound_request_about_a_document_is_refused_to_somebody_who_holds_nothing() {
    use orch_api_contract::documents as d;
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk.paragraphs(&desk.ada, made.base.id, &["x"]).await;
    let sam = desk.stranger("sam").await;
    let id = made.base.id;
    let app = &desk.world.app;
    let token = sam.token.as_str();
    let some = new_id();
    let mut refusals: Vec<(&str, orch_api_contract::common::Problem)> = Vec::new();
    refusals.push((
        "NodeDocumentList",
        app.call(d::NodeDocumentList, &[&desk.topic])
            .token(token)
            .query(&d::NodeDocumentListQuery::default())
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "NodeDocumentCreate",
        app.call(d::NodeDocumentCreate, &[&desk.topic])
            .token(token)
            .body(&body("x"))
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentGet",
        app.call(d::DocumentGet, &[&id])
            .token(token)
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentSyncGet",
        app.call(d::DocumentSyncGet, &[&id])
            .token(token)
            .query(&d::DocumentSyncGetQuery::default())
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentUpdate",
        app.call(d::DocumentUpdate, &[&id])
            .token(token)
            .body(&d::DocumentUpdateBody {
                title: Some("x".into()),
                description: None,
            })
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentOpsCreate",
        app.call(d::DocumentOpsCreate, &[&id])
            .token(token)
            .body(&d::DocumentOpsCreateBody {
                expected_seq: None,
                client_id: new_id().to_string(),
                ops: vec![desk::remove(&blocks[0])],
            })
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentVersionList",
        app.call(d::DocumentVersionList, &[&id])
            .token(token)
            .query(&d::DocumentVersionListQuery::default())
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentChangesGet",
        app.call(d::DocumentChangesGet, &[&id])
            .token(token)
            .query(&d::DocumentChangesGetQuery {
                inspect_deleted: None,
                from: "0".into(),
                to: None,
            })
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentVersionGet",
        app.call(d::DocumentVersionGet, &[&id, &1])
            .token(token)
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentVersionRestore",
        app.call(d::DocumentVersionRestore, &[&id, &1])
            .token(token)
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentFileDelete",
        app.call(d::DocumentFileDelete, &[&id, &some])
            .token(token)
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentSeen",
        app.call(d::DocumentSeen, &[&id])
            .token(token)
            .body(&d::DocumentSeenBody { seq: 1 })
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentExportGet",
        problem_of(
            app.call(d::DocumentExportGet, &[&id])
                .token(token)
                .query(&d::DocumentExportGetQuery::default())
                .send_bytes()
                .await,
        ),
    ));
    refusals.push((
        "DocumentDiscussionList",
        app.call(d::DocumentDiscussionList, &[&id])
            .token(token)
            .query(&d::DocumentDiscussionListQuery::default())
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentDiscussionCreate",
        app.call(d::DocumentDiscussionCreate, &[&id])
            .token(token)
            .body(&d::DocumentDiscussionCreateBody {
                block_id: blocks[0].id,
                quote: None,
                content: desk::text("hello"),
                client_id: new_id().to_string(),
            })
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentCommentCreate",
        app.call(d::DocumentCommentCreate, &[&id, &some])
            .token(token)
            .body(&d::DocumentCommentCreateBody {
                content: desk::text("hello"),
                client_id: new_id().to_string(),
            })
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentDiscussionResolve",
        app.call(d::DocumentDiscussionResolve, &[&id, &some])
            .token(token)
            .body(&d::DocumentDiscussionResolveBody { resolved: true })
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentCommentUpdate",
        app.call(d::DocumentCommentUpdate, &[&id, &some])
            .token(token)
            .body(&d::DocumentCommentUpdateBody {
                content: desk::text("x"),
                client_id: None,
                expected_seq: None,
            })
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentCommentDelete",
        app.call(d::DocumentCommentDelete, &[&id, &some])
            .token(token)
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentCommentReactionSet",
        app.call(d::DocumentCommentReactionSet, &[&id, &some])
            .token(token)
            .body(&d::DocumentCommentReactionSetBody {
                emoji: "+1".into(),
                on: true,
            })
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentMovePreview",
        app.call(d::DocumentMovePreview, &[&id])
            .token(token)
            .body(&d::DocumentMoveBody {
                node_id: desk.project,
            })
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentMove",
        app.call(d::DocumentMove, &[&id])
            .token(token)
            .body(&d::DocumentMoveBody {
                node_id: desk.project,
            })
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentDelete",
        app.call(d::DocumentDelete, &[&id])
            .token(token)
            .send()
            .await
            .problem(),
    ));
    refusals.push((
        "DocumentRestore",
        app.call(d::DocumentRestore, &[&id])
            .token(token)
            .send()
            .await
            .problem(),
    ));
    let mut seen = HashSet::new();
    for (name, problem) in &refusals {
        assert_eq!(
            problem.code,
            ErrorCode::AuthzDenied,
            "{name}: {}",
            problem.detail
        );
        assert_eq!(problem.status, 403, "{name}");
        seen.insert(*name);
    }
    // Nothing of a refused call was done.
    assert_eq!(desk.words(&desk.ada, id).await, ["x"]);
    assert_eq!(seen.len(), refusals.len());
}

fn problem_of(reply: orch_api_core::testing::RawReply) -> orch_api_contract::common::Problem {
    reply.problem()
}

#[test]
fn every_request_about_a_document_is_bound() {
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let bound: HashSet<&str> = registry.bound().iter().map(|route| route.name).collect();
    for route in orch_api_contract::documents::ROUTES {
        assert!(bound.contains(route.name), "{} is not bound", route.name);
    }
}
