//! Batches of changes to a document over HTTP: each operation, two people writing at once, a block that was changed meanwhile, a batch sent twice, the limits, and who is told that they are named.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::{Desk, paragraph, put, put_after, put_under, remove, rewrite, said, text};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    BlockContent, BlockProps, BlockType, DocumentOp, DocumentOpsCreate, DocumentOpsCreateBody,
    HeadingProps, Inline, NewBlock,
};
use orch_settings::Target;
use orch_types::{Id, new_id};
use serde_json::json;

fn heading(words: &str, level: u32) -> NewBlock {
    NewBlock {
        id: new_id(),
        block_type: BlockType::Heading,
        props: Some(BlockProps::Heading(HeadingProps {
            color: None,
            background: None,
            align: None,
            level,
            toggle: None,
        })),
        content: Some(BlockContent::Inline(text(words))),
        children: None,
    }
}

fn nested(levels: usize) -> NewBlock {
    let mut block = paragraph("deepest");
    for level in 1..levels {
        let mut parent = NewBlock {
            id: new_id(),
            block_type: BlockType::Toggle,
            props: None,
            content: Some(BlockContent::Inline(text(&format!("level {level}")))),
            children: None,
        };
        parent.children = Some(vec![block]);
        block = parent;
    }
    block
}

async fn notices(desk: &Desk, user: Id) -> Vec<(String, String)> {
    sqlx::query_as("SELECT kind, title FROM notifications WHERE user_id = $1 ORDER BY inbox_seq")
        .bind(user)
        .fetch_all(&desk.world.db.pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------- what each operation does

#[tokio::test]
async fn an_insert_puts_a_block_with_its_children_where_it_is_told_to_and_a_missing_sibling_puts_it_last()
 {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let firsts = desk
        .paragraphs(&desk.ada, made.base.id, &["one", "three"])
        .await;
    // After the first, so between the two.
    desk.write(
        &desk.ada,
        made.base.id,
        vec![put_after(paragraph("two"), firsts[0].id)],
    )
    .await
    .ok();
    // First of all.
    desk.write(&desk.ada, made.base.id, vec![put(paragraph("zero"))])
        .await
        .ok();
    // After a sibling that is not there: at the end.
    desk.write(
        &desk.ada,
        made.base.id,
        vec![put_after(paragraph("last"), new_id())],
    )
    .await
    .ok();
    assert_eq!(
        desk.words(&desk.ada, made.base.id).await,
        ["zero", "one", "two", "three", "last"]
    );
    // A toggle holds what is under it, written with it.
    let toggle = NewBlock {
        id: new_id(),
        block_type: BlockType::Toggle,
        props: None,
        content: Some(BlockContent::Inline(text("More"))),
        children: Some(vec![paragraph("inside")]),
    };
    let toggle_id = toggle.id;
    let reply = desk
        .write(&desk.ada, made.base.id, vec![put(toggle)])
        .await
        .ok();
    assert_eq!(
        reply.blocks.len(),
        2,
        "the batch touched the toggle and what is under it"
    );
    let under = reply
        .blocks
        .iter()
        .find(|block| block.parent_id == Some(toggle_id))
        .unwrap();
    assert_eq!(said(under), "inside");
}

#[tokio::test]
async fn an_update_changes_the_type_the_props_and_the_content_of_a_block_and_stamps_it() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk.paragraphs(&desk.ada, made.base.id, &["plain"]).await;
    let reply = desk
        .write(
            &desk.ada,
            made.base.id,
            vec![DocumentOp::Update {
                id: blocks[0].id,
                base_seq: blocks[0].seq_updated,
                block_type: Some(BlockType::Heading),
                props: Some(BlockProps::Heading(HeadingProps {
                    color: Some(3),
                    background: None,
                    align: None,
                    level: 2,
                    toggle: Some(true),
                })),
                content: Some(BlockContent::Inline(text("Titled"))),
            }],
        )
        .await
        .ok();
    assert_eq!(reply.seq, 2);
    assert_eq!(reply.document.seq, 2);
    let changed = &reply.blocks[0];
    assert_eq!(changed.block_type, BlockType::Heading);
    assert_eq!(said(changed), "Titled");
    assert!(matches!(&changed.props, BlockProps::Heading(p) if p.level == 2 && p.color == Some(3)));
    assert_eq!((changed.seq_created, changed.seq_updated), (1, 2));
    // Only the props: the type stays what it is, and the words stay.
    let again = desk
        .write(
            &desk.ada,
            made.base.id,
            vec![DocumentOp::Update {
                id: changed.id,
                base_seq: changed.seq_updated,
                block_type: None,
                props: Some(BlockProps::Heading(HeadingProps {
                    color: None,
                    background: None,
                    align: None,
                    level: 3,
                    toggle: None,
                })),
                content: None,
            }],
        )
        .await
        .ok();
    assert_eq!(again.blocks[0].block_type, BlockType::Heading);
    assert_eq!(said(&again.blocks[0]), "Titled");
}

#[tokio::test]
async fn a_move_takes_a_block_with_what_is_under_it_and_a_remove_leaves_a_row_that_says_so() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk
        .paragraphs(&desk.ada, made.base.id, &["a", "b", "c"])
        .await;
    let toggle = NewBlock {
        id: new_id(),
        block_type: BlockType::Toggle,
        props: None,
        content: Some(BlockContent::Inline(text("Box"))),
        children: None,
    };
    let toggle_id = toggle.id;
    desk.write(
        &desk.ada,
        made.base.id,
        vec![put_after(toggle, blocks[2].id)],
    )
    .await
    .ok();
    // c goes under the toggle; a goes after b.
    desk.write(
        &desk.ada,
        made.base.id,
        vec![
            DocumentOp::Move {
                id: blocks[2].id,
                parent_id: Some(toggle_id),
                after_id: None,
            },
            DocumentOp::Move {
                id: blocks[0].id,
                parent_id: None,
                after_id: Some(blocks[1].id),
            },
        ],
    )
    .await
    .ok();
    assert_eq!(desk.words(&desk.ada, made.base.id).await, ["b", "a", "Box"]);
    let synced = desk.sync(&desk.ada, made.base.id).await;
    let c = synced
        .blocks
        .iter()
        .find(|block| block.id == blocks[2].id)
        .unwrap();
    assert_eq!(c.parent_id, Some(toggle_id));
    // Removing the toggle removes what is under it; removing what is gone changes nothing.
    let toggle_row = synced
        .blocks
        .iter()
        .find(|block| block.id == toggle_id)
        .unwrap();
    let before = synced.seq;
    let reply = desk
        .write(&desk.ada, made.base.id, vec![remove(toggle_row)])
        .await
        .ok();
    assert_eq!(reply.seq, before + 1);
    assert!(reply.blocks.iter().all(|block| block.removed));
    assert_eq!(
        reply.blocks.len(),
        2,
        "the toggle and the paragraph under it"
    );
    let nothing = desk
        .write(&desk.ada, made.base.id, vec![remove(toggle_row)])
        .await
        .ok();
    assert_eq!(nothing.seq, before + 1, "nothing was stamped");
    assert!(nothing.blocks.is_empty());
    assert_eq!(desk.words(&desk.ada, made.base.id).await, ["b", "a"]);
}

// ---------------------------------------------------------------- two people, one block

#[tokio::test]
async fn two_people_changing_different_blocks_both_succeed() {
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk
        .paragraphs(&desk.ada, made.base.id, &["first", "second"])
        .await;
    // Both read the document at the same state.
    let (a, b) = (&blocks[0], &blocks[1]);
    desk.write(&desk.ada, made.base.id, vec![rewrite(a, "first, by ada")])
        .await
        .ok();
    // Ines wrote on the state before Ada's change, in another block: no conflict.
    desk.write(&ines, made.base.id, vec![rewrite(b, "second, by ines")])
        .await
        .ok();
    assert_eq!(
        desk.words(&desk.ada, made.base.id).await,
        ["first, by ada", "second, by ines"]
    );
}

#[tokio::test]
async fn the_same_block_changed_meanwhile_refuses_the_second_batch_with_the_block_as_it_is_and_applies_nothing_of_it()
 {
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk
        .paragraphs(&desk.ada, made.base.id, &["first", "second"])
        .await;
    let stale = blocks[0].clone();
    desk.write(
        &desk.ada,
        made.base.id,
        vec![rewrite(&stale, "first, by ada")],
    )
    .await
    .ok();
    let before = desk.sync(&desk.ada, made.base.id).await.seq;
    // Ines's batch writes a new block and changes the block Ada changed.
    let extra = paragraph("a new thought");
    let extra_id = extra.id;
    let refused = desk
        .write(
            &ines,
            made.base.id,
            vec![put(extra), rewrite(&stale, "first, by ines")],
        )
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(refused.status, 409);
    let current = refused.extra["current"].as_array().unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0]["id"], json!(stale.id));
    assert_eq!(current[0]["content"][0]["text"], "first, by ada");
    assert_eq!(current[0]["removed"], false);
    // Nothing of the batch was applied: not even what did not conflict.
    let after = desk.sync(&desk.ada, made.base.id).await;
    assert_eq!(after.seq, before);
    assert!(after.blocks.iter().all(|block| block.id != extra_id));
    assert_eq!(
        desk.words(&desk.ada, made.base.id).await,
        ["first, by ada", "second"]
    );
}

#[tokio::test]
async fn a_block_removed_meanwhile_is_a_conflict_that_says_it_was_removed() {
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk
        .paragraphs(&desk.ada, made.base.id, &["first", "second"])
        .await;
    desk.write(&desk.ada, made.base.id, vec![remove(&blocks[0])])
        .await
        .ok();
    // Changing it, moving it, and building on it as a parent or a neighbour all conflict.
    let refused = desk
        .write(
            &ines,
            made.base.id,
            vec![rewrite(&blocks[0], "still here?")],
        )
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(refused.extra["current"][0]["removed"], true);
    let under = desk
        .write(
            &ines,
            made.base.id,
            vec![put_under(paragraph("child"), blocks[0].id)],
        )
        .await
        .problem();
    assert_eq!(under.code, ErrorCode::ConflictStaleRevision);
}

#[tokio::test]
async fn a_batch_sent_twice_is_applied_once_and_answers_as_the_first_time_did() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let body = DocumentOpsCreateBody {
        expected_seq: None,
        client_id: new_id().to_string(),
        ops: vec![put(paragraph("once"))],
    };
    let send = || async {
        desk.world
            .app
            .call(DocumentOpsCreate, &[&made.base.id])
            .token(&desk.ada.token)
            .body(&body)
            .send()
            .await
            .ok()
    };
    let first = send().await;
    let again = send().await;
    assert_eq!(first.seq, again.seq);
    assert_eq!(first.blocks.len(), 1);
    assert_eq!(again.blocks.len(), 1);
    assert_eq!(first.blocks[0].id, again.blocks[0].id);
    assert_eq!(desk.words(&desk.ada, made.base.id).await, ["once"]);
    // Another person's batch with the same value is theirs and is applied.
    let ines = desk.member("ines", "interactor").await;
    let theirs = desk
        .world
        .app
        .call(DocumentOpsCreate, &[&made.base.id])
        .token(&ines.token)
        .body(&DocumentOpsCreateBody {
            expected_seq: None,
            client_id: body.client_id.clone(),
            ops: vec![put(paragraph("ines"))],
        })
        .send()
        .await
        .ok();
    assert_eq!(theirs.seq, first.seq + 1);
}

#[tokio::test]
async fn a_block_whose_identifier_is_in_use_is_refused_and_so_is_one_that_already_exists_in_another_document()
 {
    let desk = Desk::new().await;
    let one = desk.make(&desk.ada, "One").await;
    let two = desk.make(&desk.ada, "Two").await;
    let blocks = desk.paragraphs(&desk.ada, one.base.id, &["taken"]).await;
    let mut copy = paragraph("copy");
    copy.id = blocks[0].id;
    let refused = desk
        .write(&desk.ada, two.base.id, vec![put(copy)])
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictExists);
    let twice = paragraph("twice");
    let mut other = paragraph("other");
    other.id = twice.id;
    let refused = desk
        .write(&desk.ada, two.base.id, vec![put(twice), put(other)])
        .await
        .problem();
    assert_eq!(
        refused.code,
        ErrorCode::ConflictExists,
        "within one batch as well"
    );
    assert!(desk.words(&desk.ada, two.base.id).await.is_empty());
}

// ---------------------------------------------------------------- who may

#[tokio::test]
async fn a_viewer_comments_and_cannot_edit_as_shipped() {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk.paragraphs(&desk.ada, made.base.id, &["text"]).await;
    let refused = desk
        .write(&vera, made.base.id, vec![rewrite(&blocks[0], "vandalism")])
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "doc.edit");
    assert_eq!(desk.words(&vera, made.base.id).await, ["text"]);
    // But she opens a discussion on it.
    let opened = desk
        .world
        .app
        .call(
            orch_api_contract::documents::DocumentDiscussionCreate,
            &[&made.base.id],
        )
        .token(&vera.token)
        .body(
            &orch_api_contract::documents::DocumentDiscussionCreateBody {
                block_id: blocks[0].id,
                quote: None,
                content: text("Is this right?"),
                client_id: new_id().to_string(),
            },
        )
        .send()
        .await;
    assert_eq!(opened.status, 201);
}

// ---------------------------------------------------------------- shapes and limits

#[tokio::test]
async fn a_block_of_the_wrong_shape_is_refused_naming_the_operation_and_the_member() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let refused = desk
        .write(
            &desk.ada,
            made.base.id,
            vec![put(paragraph("fine")), put(heading("Big", 9))],
        )
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    let path = refused.extra["errors"][0]["path"].as_str().unwrap();
    assert!(path.starts_with("ops[1]"), "{path}");
    // A columns block holds columns alone.
    let columns = NewBlock {
        id: new_id(),
        block_type: BlockType::Columns,
        props: None,
        content: None,
        children: Some(vec![paragraph("not a column"), paragraph("nor this")]),
    };
    let refused = desk
        .write(&desk.ada, made.base.id, vec![put(columns)])
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    // A link is http, https or mailto, or a path of the system.
    let linked = NewBlock {
        id: new_id(),
        block_type: BlockType::Paragraph,
        props: None,
        content: Some(BlockContent::Inline(vec![Inline::Text {
            text: "click".to_owned(),
            marks: None,
            href: Some("javascript:alert(1)".to_owned()),
            color: None,
            background: None,
        }])),
        children: None,
    };
    let refused = desk
        .write(&desk.ada, made.base.id, vec![put(linked)])
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert!(
        desk.words(&desk.ada, made.base.id).await.is_empty(),
        "nothing of any of it"
    );
    let not_an_id = desk
        .world
        .app
        .call(DocumentOpsCreate, &[&made.base.id])
        .token(&desk.ada.token)
        .body(&DocumentOpsCreateBody {
            expected_seq: None,
            client_id: "not a uuid".to_owned(),
            ops: Vec::new(),
        })
        .send()
        .await
        .problem();
    assert_eq!(not_an_id.extra["errors"][0]["path"], "client_id");
}

#[tokio::test]
async fn one_block_may_not_be_larger_than_sixty_four_kibibytes() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let refused = desk
        .write(
            &desk.ada,
            made.base.id,
            vec![put(paragraph(&"x".repeat(70_000)))],
        )
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::DocsTooLarge);
    assert_eq!(refused.status, 413);
    assert_eq!(refused.extra["limit"], "block_bytes");
    assert_eq!(refused.extra["value"], 65_536);
    assert_eq!(refused.extra["scope"], json!(null));
    desk.write(
        &desk.ada,
        made.base.id,
        vec![put(paragraph(&"x".repeat(60_000)))],
    )
    .await
    .ok();
}

#[tokio::test]
async fn a_document_may_not_hold_more_blocks_than_its_place_allows_and_the_refusal_names_the_place()
{
    let desk = Desk::new().await;
    desk.world
        .set_at(Target::Node(desk.project), "docs.max_blocks", json!(100))
        .await;
    let made = desk.make(&desk.ada, "Handbook").await;
    assert_eq!(made.limits.blocks, 100, "a document says what it may hold");
    let words: Vec<String> = (0..101).map(|each| format!("block {each}")).collect();
    let ops: Vec<DocumentOp> = words.iter().map(|each| put(paragraph(each))).collect();
    let refused = desk.write(&desk.ada, made.base.id, ops).await.problem();
    assert_eq!(refused.code, ErrorCode::DocsTooLarge);
    assert_eq!(refused.extra["limit"], "blocks");
    assert_eq!(refused.extra["value"], 100);
    assert_eq!(refused.extra["scope"]["id"], json!(desk.project));
    assert!(desk.words(&desk.ada, made.base.id).await.is_empty());
    let ops: Vec<DocumentOp> = words
        .iter()
        .take(100)
        .map(|each| put(paragraph(each)))
        .collect();
    desk.write(&desk.ada, made.base.id, ops).await.ok();
}

#[tokio::test]
async fn a_document_may_not_hold_more_text_than_its_place_allows() {
    let desk = Desk::new().await;
    desk.world
        .set_at(Target::Node(desk.topic), "docs.max_bytes", json!(65_536))
        .await;
    let made = desk.make(&desk.ada, "Handbook").await;
    desk.write(
        &desk.ada,
        made.base.id,
        vec![put(paragraph(&"a".repeat(30_000)))],
    )
    .await
    .ok();
    desk.write(
        &desk.ada,
        made.base.id,
        vec![put(paragraph(&"b".repeat(30_000)))],
    )
    .await
    .ok();
    let refused = desk
        .write(
            &desk.ada,
            made.base.id,
            vec![put(paragraph(&"c".repeat(30_000)))],
        )
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::DocsTooLarge);
    assert_eq!(refused.extra["limit"], "bytes");
    assert_eq!(refused.extra["scope"]["id"], json!(desk.topic));
    assert_eq!(desk.words(&desk.ada, made.base.id).await.len(), 2);
    // Making room makes room: what is removed no longer counts.
    let page = desk.page(&desk.ada, made.base.id).await;
    desk.write(
        &desk.ada,
        made.base.id,
        vec![remove(&page[0]), put(paragraph(&"c".repeat(30_000)))],
    )
    .await
    .ok();
}

#[tokio::test]
async fn a_batch_holds_at_most_five_hundred_operations_and_blocks_nest_at_most_sixteen_deep() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let many: Vec<DocumentOp> = (0..501)
        .map(|_| DocumentOp::Remove { id: new_id() })
        .collect();
    let refused = desk.write(&desk.ada, made.base.id, many).await.problem();
    assert_eq!(refused.code, ErrorCode::DocsTooLarge);
    assert_eq!(refused.extra["limit"], "operations");
    assert_eq!(refused.extra["value"], 500);
    let just_enough: Vec<DocumentOp> = (0..500)
        .map(|_| DocumentOp::Remove { id: new_id() })
        .collect();
    desk.write(&desk.ada, made.base.id, just_enough).await.ok();

    let refused = desk
        .write(&desk.ada, made.base.id, vec![put(nested(18))])
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::DocsTooLarge);
    assert_eq!(refused.extra["limit"], "depth");
    assert_eq!(refused.extra["value"], 16);
    desk.write(&desk.ada, made.base.id, vec![put(nested(17))])
        .await
        .ok();
}

// ---------------------------------------------------------------- the people it names

#[tokio::test]
async fn naming_people_in_a_block_does_not_notify_them() {
    let desk = Desk::new().await;
    let bobby = desk.member("bobby", "viewer").await;
    let sam = desk.stranger("sam").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let mention = |who: &places::Who, words: &str| NewBlock {
        id: new_id(),
        block_type: BlockType::Paragraph,
        props: None,
        content: Some(BlockContent::Inline(vec![
            Inline::Text {
                text: format!("{words} "),
                marks: None,
                href: None,
                color: None,
                background: None,
            },
            Inline::Mention {
                user_id: who.id(),
                user: None,
            },
        ])),
        children: None,
    };
    desk.write(
        &desk.ada,
        made.base.id,
        vec![
            put(mention(&bobby, "Please look,")),
            put(mention(&sam, "And you,")),
        ],
    )
    .await
    .ok();
    let told = notices(&desk, bobby.id()).await;
    assert!(
        told.is_empty(),
        "a mention alone does not send a notice: {told:?}"
    );
    assert!(
        notices(&desk, sam.id()).await.is_empty(),
        "naming somebody tells them nothing they could not open"
    );
    // Writing the name again also sends nothing.
    desk.write(
        &desk.ada,
        made.base.id,
        vec![
            put(mention(&bobby, "Again,")),
            put(mention(&bobby, "and again,")),
        ],
    )
    .await
    .ok();
    assert!(notices(&desk, bobby.id()).await.is_empty());
    // And naming oneself tells nobody.
    desk.write(
        &desk.ada,
        made.base.id,
        vec![put(mention(&desk.ada, "Note to self:"))],
    )
    .await
    .ok();
    assert!(notices(&desk, desk.ada.id()).await.is_empty());
}
