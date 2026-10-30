//! A text read into blocks by the one reader, which a paste goes through: Orch Markdown, HTML and plain text, a document's own files and people, the pictures a text carries inside it, and what is kept as words, said by code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    BlockContent, BlockProps, BlockType, DocumentConvert, DocumentConvertBody,
    DocumentConvertFormat, DocumentConvertReply, DocumentGet, Inline, NewBlock,
};
use orch_api_core::testing::Reply;
use places::Who;

/// A picture of one pixel.
const PICTURE: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

async fn convert(
    desk: &Desk,
    by: &Who,
    format: DocumentConvertFormat,
    text: &str,
    document: Option<orch_types::Id>,
) -> Reply<DocumentConvertReply> {
    desk.world
        .app
        .call(DocumentConvert, &[])
        .token(&by.token)
        .body(&DocumentConvertBody {
            format,
            text: text.to_owned(),
            document_id: document,
        })
        .send()
        .await
}

fn kinds(blocks: &[NewBlock]) -> Vec<BlockType> {
    blocks.iter().map(|block| block.block_type).collect()
}

fn words(block: &NewBlock) -> String {
    match &block.content {
        Some(BlockContent::Inline(runs)) => runs
            .iter()
            .filter_map(|run| match run {
                Inline::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect(),
        _ => String::new(),
    }
}

#[tokio::test]
async fn orch_markdown_is_read_into_the_blocks_it_says() {
    let desk = Desk::new().await;
    let text = "# Plan\n\n> [!WARNING]\n> Mind the gap\n\n<details>\n<summary>More</summary>\n\nhidden\n\n</details>\n\n<columns>\n<column ratio=\"2\">\n\nleft\n\n</column>\n<column ratio=\"1\">\n\nright\n\n</column>\n</columns>\n\nA <u>marked</u> word.\n\n<page-break/>\n";
    let reply = convert(
        &desk,
        &desk.ada,
        DocumentConvertFormat::Markdown,
        text,
        None,
    )
    .await
    .ok();
    assert_eq!(
        kinds(&reply.blocks),
        vec![
            BlockType::Heading,
            BlockType::Callout,
            BlockType::Toggle,
            BlockType::Columns,
            BlockType::Paragraph,
            BlockType::PageBreak,
        ]
    );
    assert!(reply.warnings.is_empty(), "{:?}", reply.warnings);
    assert!(reply.files.is_empty());
    let toggle = &reply.blocks[2];
    assert_eq!(words(toggle), "More");
    assert_eq!(toggle.children.as_ref().unwrap().len(), 1);
    assert_eq!(reply.blocks[3].children.as_ref().unwrap().len(), 2);
    let underlined = match &reply.blocks[4].content {
        Some(BlockContent::Inline(runs)) => runs.iter().any(|run| {
            matches!(run, Inline::Text { text, marks: Some(marks), .. }
                if text == "marked" && !marks.is_empty())
        }),
        _ => false,
    };
    assert!(underlined, "{:?}", reply.blocks[4]);
}

#[tokio::test]
async fn a_link_that_leads_nowhere_stays_words_and_a_warning_says_which() {
    let desk = Desk::new().await;
    let reply = convert(
        &desk,
        &desk.ada,
        DocumentConvertFormat::Markdown,
        "See [the notes](notes/other.md) and ![chart](images/chart.png).\n",
        None,
    )
    .await
    .ok();
    assert_eq!(kinds(&reply.blocks), vec![BlockType::Paragraph]);
    let codes: Vec<&str> = reply
        .warnings
        .iter()
        .map(|warning| warning.code.as_str())
        .collect();
    assert!(codes.contains(&"reader.link_unresolved"), "{codes:?}");
    let link = reply
        .warnings
        .iter()
        .find(|warning| warning.code == "reader.link_unresolved")
        .unwrap();
    assert_eq!(link.params["target"], "notes/other.md");
    assert!(link.message.contains("notes/other.md"), "{}", link.message);
}

#[tokio::test]
async fn html_and_plain_text_go_through_the_same_reader() {
    let desk = Desk::new().await;
    let html = convert(
        &desk,
        &desk.ada,
        DocumentConvertFormat::Html,
        "<h2>Title</h2><details><summary>More</summary><p>hidden</p></details><script>alert(1)</script>",
        None,
    )
    .await
    .ok();
    assert_eq!(
        kinds(&html.blocks),
        vec![BlockType::Heading, BlockType::Toggle]
    );
    assert!(
        !html.warnings.is_empty(),
        "the script was removed, and said"
    );
    let plain = convert(
        &desk,
        &desk.ada,
        DocumentConvertFormat::Text,
        "one\n\ntwo\n",
        None,
    )
    .await
    .ok();
    assert_eq!(
        kinds(&plain.blocks),
        vec![BlockType::Paragraph, BlockType::Paragraph]
    );
}

#[tokio::test]
async fn a_picture_inside_the_text_becomes_a_file_of_the_document_it_is_for() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Pasted into").await;
    let text = format!("Look:\n\n![chart](data:image/png;base64,{PICTURE})\n");
    let reply = convert(
        &desk,
        &desk.ada,
        DocumentConvertFormat::Markdown,
        &text,
        Some(made.base.id),
    )
    .await
    .ok();
    assert_eq!(reply.files.len(), 1, "{:?}", reply.files);
    let image = reply
        .blocks
        .iter()
        .find(|block| block.block_type == BlockType::Image)
        .expect("an image block");
    let Some(BlockProps::Image(props)) = &image.props else {
        panic!("{image:?}");
    };
    assert_eq!(props.file_id, reply.files[0].id);
    let document = desk
        .world
        .app
        .call(DocumentGet, &[&made.base.id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(document.files.len(), 1);
    assert_eq!(document.files[0].id, reply.files[0].id);
    assert_eq!(document.files[0].media_type, "image/png");
}

#[tokio::test]
async fn without_a_document_the_pictures_are_left_out_and_the_reply_says_so() {
    let desk = Desk::new().await;
    let text = format!("![chart](data:image/png;base64,{PICTURE})\n");
    let reply = convert(
        &desk,
        &desk.ada,
        DocumentConvertFormat::Markdown,
        &text,
        None,
    )
    .await
    .ok();
    assert!(reply.files.is_empty());
    assert!(
        reply
            .blocks
            .iter()
            .all(|block| block.block_type != BlockType::Image),
        "{:?}",
        reply.blocks
    );
    assert!(
        reply
            .warnings
            .iter()
            .any(|warning| warning.code == "reader.pictures_left_out"),
        "{:?}",
        reply.warnings
    );
}

#[tokio::test]
async fn naming_a_document_needs_the_right_to_edit_it() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Private").await;
    let sam = desk.stranger("sam").await;
    let refused = convert(
        &desk,
        &sam,
        DocumentConvertFormat::Markdown,
        "words",
        Some(made.base.id),
    )
    .await
    .problem();
    assert!(
        matches!(refused.code, ErrorCode::AuthzDenied | ErrorCode::NotFound),
        "{:?}",
        refused.code
    );
}
