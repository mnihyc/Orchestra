//! A document written out over HTTP as Markdown, one HTML page, Typst source and JSON: what each holds, what Markdown could not keep, that what is written out reads back as the same blocks, and who may.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::{Desk, body, paragraph, text};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    BlockContent, BlockProps, BlockType, CalloutProps, CalloutTone, Document, DocumentBlock,
    DocumentDiscussionCreate, DocumentDiscussionCreateBody, DocumentExport, DocumentExportChoice,
    DocumentExportGet, DocumentExportGetQuery, DocumentSync, HeadingProps, NewBlock,
    NodeDocumentCreateBody,
};
use orch_api_core::testing::RawReply;
use orch_types::{Id, new_id};
use places::Who;
use std::io::Read;

async fn export(
    desk: &Desk,
    by: &Who,
    document: Id,
    format: DocumentExport,
    comments: bool,
) -> RawReply {
    desk.world
        .app
        .call(DocumentExportGet, &[&document])
        .token(&by.token)
        .query(&DocumentExportGetQuery {
            format: Some(format),
            attachments: None,
            comments: comments.then_some(DocumentExportChoice::Include),
        })
        .send_bytes()
        .await
}

fn header<'a>(reply: &'a RawReply, name: &str) -> &'a str {
    reply
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
}

fn text_of(reply: &RawReply) -> String {
    String::from_utf8(reply.raw.to_vec()).unwrap()
}

fn entries(reply: &RawReply) -> Vec<(String, String)> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(reply.raw.to_vec())).unwrap();
    (0..archive.len())
        .map(|index| {
            let mut file = archive.by_index(index).unwrap();
            let mut content = String::new();
            file.read_to_string(&mut content).unwrap();
            (file.name().unwrap().into_owned(), content)
        })
        .collect()
}

async fn from_markdown(desk: &Desk, title: &str, markdown: &str) -> Document {
    desk.make_body(
        &desk.ada,
        NodeDocumentCreateBody {
            title: title.to_owned(),
            blocks: None,
            markdown: Some(markdown.to_owned()),
            access: None,
        },
    )
    .await
    .ok()
}

/// The types and the words of the top-level blocks, in order.
fn outline(blocks: &[DocumentBlock]) -> Vec<(BlockType, String)> {
    blocks
        .iter()
        .map(|block| (block.block_type, desk::said(block)))
        .collect()
}

const NOTES: &str = "\
# Release notes

Plain **bold** and *italic* with `code`, a [link](https://example.com) and ~~strike~~.

- one
- two

1. first
2. second

- [x] done
- [ ] to do

> Quoted words

```rust
fn main() {}
```

---

| Name | Role |
| --- | --- |
| Ada | Owner |
";

// ---------------------------------------------------------------- Markdown

#[tokio::test]
async fn markdown_written_out_and_read_in_again_gives_the_same_blocks_for_every_type_it_can_carry()
{
    let desk = Desk::new().await;
    let made = from_markdown(&desk, "Notes", NOTES).await;
    let original = desk.page(&desk.ada, made.base.id).await;
    let kinds: Vec<BlockType> = original.iter().map(|block| block.block_type).collect();
    for wanted in [
        BlockType::Heading,
        BlockType::Paragraph,
        BlockType::Bullet,
        BlockType::Numbered,
        BlockType::Todo,
        BlockType::Quote,
        BlockType::Code,
        BlockType::Divider,
        BlockType::Table,
    ] {
        assert!(kinds.contains(&wanted), "{wanted:?} in {kinds:?}");
    }
    let reply = export(
        &desk,
        &desk.ada,
        made.base.id,
        DocumentExport::Markdown,
        false,
    )
    .await;
    assert_eq!(reply.status, 200);
    assert!(header(&reply, "content-type").starts_with("text/markdown"));
    assert_eq!(
        header(&reply, "content-disposition"),
        "attachment; filename=\"Notes.md\"; filename*=UTF-8''Notes.md"
    );
    assert_eq!(
        header(&reply, "x-orch-export-losses"),
        "",
        "it lost nothing"
    );
    let written = text_of(&reply);
    let (title, rest) = written.split_once("\n\n").unwrap();
    assert_eq!(title, "# Notes", "the title heads the file");
    let again = from_markdown(&desk, "Again", rest).await;
    let reread = desk.page(&desk.ada, again.base.id).await;
    assert_eq!(outline(&original), outline(&reread));
    for (a, b) in original.iter().zip(reread.iter()) {
        assert_eq!(a.props, b.props, "{:?}", a.block_type);
        assert_eq!(a.content, b.content, "{:?}", a.block_type);
    }
}

#[tokio::test]
async fn markdown_export_preserves_rich_layout_and_records_no_layout_loss() {
    let desk = Desk::new().await;
    let made = desk
        .make_body(
            &desk.ada,
            NodeDocumentCreateBody {
                title: "Richer".to_owned(),
                blocks: Some(vec![
                    NewBlock {
                        id: new_id(),
                        block_type: BlockType::Callout,
                        props: Some(BlockProps::Callout(CalloutProps {
                            color: None,
                            background: None,
                            align: None,
                            tone: CalloutTone::Warning,
                        })),
                        content: Some(BlockContent::Inline(text("Mind the gap"))),
                        children: None,
                    },
                    NewBlock {
                        id: new_id(),
                        block_type: BlockType::Toggle,
                        props: None,
                        content: Some(BlockContent::Inline(text("More"))),
                        children: Some(vec![paragraph("hidden")]),
                    },
                    NewBlock {
                        id: new_id(),
                        block_type: BlockType::Heading,
                        props: Some(BlockProps::Heading(HeadingProps {
                            color: Some(3),
                            background: None,
                            align: None,
                            level: 2,
                            toggle: Some(true),
                        })),
                        content: Some(BlockContent::Inline(text("Folds and is coloured"))),
                        children: None,
                    },
                ]),
                markdown: None,
                access: None,
            },
        )
        .await
        .ok();
    let reply = export(
        &desk,
        &desk.ada,
        made.base.id,
        DocumentExport::Markdown,
        false,
    )
    .await;
    let losses = header(&reply, "x-orch-export-losses");
    assert_eq!(losses, "", "rich layout is retained in the export");
    let written = text_of(&reply);
    assert!(written.contains("> [!WARNING]"), "{written}");
    assert!(written.contains("> Mind the gap"), "{written}");
    assert!(written.contains("<details>"), "{written}");
    assert!(written.contains("hidden"));
    assert!(written.contains("Folds and is coloured"), "{written}");
    let details: serde_json::Value = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE action = 'document.exported' ORDER BY at DESC LIMIT 1",
    )
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap();
    assert_eq!(details["format"], "markdown");
    assert_eq!(details["losses"], serde_json::Value::Null);
    let (_, body) = written.split_once("\n\n").unwrap();
    let imported = from_markdown(&desk, "Richer again", body).await;
    let original = desk.page(&desk.ada, made.base.id).await;
    let reread = desk.page(&desk.ada, imported.base.id).await;
    assert_eq!(outline(&original), outline(&reread));
    for (before, after) in original.iter().zip(&reread) {
        assert_eq!(before.props, after.props, "{:?}", before.block_type);
        assert_eq!(before.content, after.content, "{:?}", before.block_type);
    }
}

#[tokio::test]
async fn the_discussions_are_written_after_the_text_when_they_are_asked_for_and_not_otherwise() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Reviewed").await;
    let blocks = desk
        .paragraphs(&desk.ada, made.base.id, &["The claim."])
        .await;
    desk.world
        .app
        .call(DocumentDiscussionCreate, &[&made.base.id])
        .token(&desk.ada.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: blocks[0].id,
            quote: Some("claim".to_owned()),
            content: text("Where is the evidence?"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let without = text_of(
        &export(
            &desk,
            &desk.ada,
            made.base.id,
            DocumentExport::Markdown,
            false,
        )
        .await,
    );
    assert!(!without.contains("evidence"));
    let with = text_of(
        &export(
            &desk,
            &desk.ada,
            made.base.id,
            DocumentExport::Markdown,
            true,
        )
        .await,
    );
    assert!(with.contains("Where is the evidence?"), "{with}");
    assert!(with.contains("ada"), "who said it");
    let page = text_of(&export(&desk, &desk.ada, made.base.id, DocumentExport::Html, true).await);
    assert!(page.contains("Where is the evidence?"));
}

// ---------------------------------------------------------------- HTML and Typst

#[tokio::test]
async fn the_html_export_is_one_file_that_needs_nothing_else_and_holds_no_markup_of_the_authors() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Fish & Chips <b>").await;
    desk.paragraphs(
        &desk.ada,
        made.base.id,
        &["<script>alert(1)</script> & more"],
    )
    .await;
    let reply = export(&desk, &desk.ada, made.base.id, DocumentExport::Html, false).await;
    assert_eq!(reply.status, 200);
    assert!(header(&reply, "content-type").starts_with("text/html"));
    assert!(header(&reply, "content-disposition").contains("Fish _ Chips _b_.html"));
    let page = text_of(&reply);
    assert!(
        page.contains("<title>Fish &amp; Chips &lt;b&gt;</title>"),
        "{page}"
    );
    assert!(page.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; more"));
    assert!(
        !page.contains("<script>"),
        "nothing of an author's is markup"
    );
    assert!(
        page.contains("Content-Security-Policy"),
        "and the page keeps whatever it loads in check"
    );
    assert!(
        !page.contains("src=\"http"),
        "it loads nothing from anywhere"
    );
}

#[tokio::test]
async fn the_typst_export_is_an_archive_of_the_source_that_a_pdf_is_made_from_in_the_readers_browser()
 {
    let desk = Desk::new().await;
    let made = from_markdown(
        &desk,
        "Plan",
        "# Goals\n\nShip it $x^2$.\n\n```mermaid\ngraph TD\n```\n",
    )
    .await;
    let reply = export(&desk, &desk.ada, made.base.id, DocumentExport::Typst, false).await;
    assert_eq!(reply.status, 200);
    assert_eq!(header(&reply, "content-type"), "application/zip");
    assert!(header(&reply, "content-disposition").contains("Plan.zip"));
    let files = entries(&reply);
    let source = &files
        .iter()
        .find(|(name, _)| name == "main.typ")
        .expect("the source")
        .1;
    assert!(source.contains("Plan"), "{source}");
    assert!(source.contains("Goals"));
    assert!(source.contains("graph TD"), "a diagram as its source");
}

// ---------------------------------------------------------------- JSON, names and permission

#[tokio::test]
async fn the_json_export_is_what_a_sync_gives_and_holds_the_discussions_only_when_asked() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Data").await;
    let blocks = desk.paragraphs(&desk.ada, made.base.id, &["row"]).await;
    desk.world
        .app
        .call(DocumentDiscussionCreate, &[&made.base.id])
        .token(&desk.ada.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: blocks[0].id,
            quote: None,
            content: text("Note"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let plain = export(&desk, &desk.ada, made.base.id, DocumentExport::Json, false).await;
    assert!(header(&plain, "content-type").starts_with("application/json"));
    let synced: DocumentSync = serde_json::from_slice(&plain.raw).unwrap();
    assert_eq!(synced.blocks.len(), 1);
    assert!(synced.discussions.is_empty());
    let with = export(&desk, &desk.ada, made.base.id, DocumentExport::Json, true).await;
    let synced: DocumentSync = serde_json::from_slice(&with.raw).unwrap();
    assert_eq!(synced.discussions.len(), 1);
    assert_eq!(synced.document.base.title, "Data");
}

#[tokio::test]
async fn a_file_name_is_made_of_the_title_and_nothing_that_could_escape_the_download_folder() {
    let desk = Desk::new().await;
    for (title, name) in [
        ("../../etc/passwd", ".._.._etc_passwd"),
        ("   ", "document"),
        ("Q3: plan/budget", "Q3_ plan_budget"),
    ] {
        let made = desk.make_body(&desk.ada, body(title)).await;
        if title.trim().is_empty() {
            assert!(made.body.is_err(), "an empty title is no title");
            continue;
        }
        let made = made.ok();
        let reply = export(
            &desk,
            &desk.ada,
            made.base.id,
            DocumentExport::Markdown,
            false,
        )
        .await;
        let disposition = header(&reply, "content-disposition");
        assert!(
            disposition.starts_with(&format!(
                "attachment; filename=\"{}.md\"",
                name.trim_matches('.')
            )),
            "{title:?} -> {disposition}"
        );
        assert!(!disposition.contains('/'));
    }
}

#[tokio::test]
async fn whoever_reads_a_document_and_holds_its_export_row_exports_it_and_the_export_is_written_in_its_history()
 {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let sam = desk.stranger("sam").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    desk.paragraphs(&desk.ada, made.base.id, &["text"]).await;
    let before = desk
        .world
        .audit()
        .await
        .iter()
        .filter(|action| *action == "document.exported")
        .count();
    for format in [
        DocumentExport::Markdown,
        DocumentExport::Html,
        DocumentExport::Typst,
        DocumentExport::Json,
    ] {
        let reply = export(&desk, &vera, made.base.id, format, false).await;
        assert_eq!(reply.status, 200, "{format:?}");
        let refused = export(&desk, &sam, made.base.id, format, false)
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied, "{format:?}");
        assert_eq!(refused.extra["required"], "doc.export");
    }
    let after = desk
        .world
        .audit()
        .await
        .iter()
        .filter(|action| *action == "document.exported")
        .count();
    assert_eq!(
        after - before,
        4,
        "each export that was made, and none that was refused"
    );
}
