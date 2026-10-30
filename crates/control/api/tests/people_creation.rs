//! Ordinary copied and imported documents retain current guards, fresh material identities, independent files and truthful retry receipts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use orch_api_contract::access::{
    AccessChoice, HolderInput, PageChange, PageChangeName, PermissionPageUpdate,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    BlockProps, BlockType, DocumentDelete, DocumentDiscussionCreate, DocumentDiscussionCreateBody,
    DocumentFileCreate, DocumentFileCreateQuery, DocumentFileDelete, DocumentFileGet,
    DocumentFileGetQuery, DocumentImportRequest, DocumentImportRequestFile,
    DocumentImportRequestFileFormat as Format, DocumentOp, FileProps, NewBlock, NewDocumentAccess,
    NodeDocumentCopy, NodeDocumentCopyBody, NodeDocumentImport,
};
use orch_api_contract::threads::{
    FILENAME_HEADER, UploadSessionComplete, UploadSessionCreate, UploadSessionCreateBody,
    UploadSessionPut, UploadSessionPutQuery,
};
use orch_types::{Hash, new_id};
use places::Who;
use serde_json::json;
use std::io::Write;

/// Uploads a file of any kind in pieces: the one-request upload takes images alone.
async fn upload(desk: &Desk, by: &Who, name: &str, bytes: &[u8]) -> Hash {
    let app = &desk.world.app;
    let session = app
        .call(UploadSessionCreate, &[])
        .token(&by.token)
        .body(&UploadSessionCreateBody {
            size: i64::try_from(bytes.len()).unwrap(),
            name: name.to_owned(),
            media_type: None,
        })
        .send()
        .await
        .ok();
    let mut offset = 0;
    for chunk in bytes.chunks(64 * 1024) {
        app.call(UploadSessionPut, &[&session.upload_id])
            .token(&by.token)
            .query(&UploadSessionPutQuery {
                offset: i64::try_from(offset).unwrap(),
            })
            .bytes(chunk)
            .send()
            .await
            .ok();
        offset += chunk.len();
    }
    app.call(UploadSessionComplete, &[&session.upload_id])
        .token(&by.token)
        .send()
        .await
        .ok()
        .blob
}

async fn import(
    desk: &Desk,
    files: Vec<DocumentImportRequestFile>,
    client: &str,
) -> orch_api_contract::documents::DocumentImportResult {
    desk.world
        .app
        .call(NodeDocumentImport, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&DocumentImportRequest {
            files,
            client_id: client.to_owned(),
        })
        .send()
        .await
        .ok()
}

fn file(blob: Hash, name: &str, format: Format) -> DocumentImportRequestFile {
    DocumentImportRequestFile {
        blob,
        name: name.to_owned(),
        format,
    }
}

#[tokio::test]
async fn a_private_source_is_refused_until_view_is_held_then_copy_has_fresh_ownership_blocks_and_no_source_access_or_discussions()
 {
    let desk = Desk::new().await;
    let bea = desk.member("bea", "viewer").await;
    let mut body = desk::body("Private source");
    body.access = Some(NewDocumentAccess::Private);
    let source = desk.make_body(&desk.ada, body).await.ok();
    // A new exact basename cannot replace the source in its current directory.
    let destination = desk.another_topic("Copied documents").await;
    let source_blocks = desk
        .paragraphs(&desk.ada, source.base.id, &["Current text"])
        .await;
    desk.world
        .app
        .call(DocumentDiscussionCreate, &[&source.base.id])
        .token(&desk.ada.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: source_blocks[0].id,
            quote: None,
            content: desk::text("Source discussion"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let body = NodeDocumentCopyBody {
        source_document_id: source.base.id,
        title: None,
        access: None,
        client_id: "copy-current".to_owned(),
    };
    let denied = desk
        .world
        .app
        .call(NodeDocumentCopy, &[&destination])
        .token(&bea.token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::NotFound);
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &source.base.id])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: bea.id() }],
                choice: AccessChoice::FullView,
                export: Some(false),
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let made = desk
        .world
        .app
        .call(NodeDocumentCopy, &[&destination])
        .token(&bea.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(made.base.owner.id, bea.id());
    assert_eq!(made.base.title, "Private source");
    let copied = desk.sync(&bea, made.base.id).await;
    assert_ne!(copied.blocks[0].id, source_blocks[0].id);
    assert!(copied.discussions.is_empty());
    let source_entries: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_id=$2 AND permission<>'access.own'")
        .bind(made.base.id).bind(bea.id()).fetch_one(&desk.world.db.pool).await.unwrap();
    assert_eq!(
        source_entries, 0,
        "only new ownership and destination defaults apply"
    );
    let again = desk
        .world
        .app
        .call(NodeDocumentCopy, &[&destination])
        .token(&bea.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(again.base.id, made.base.id);
    assert_eq!(desk.words(&bea, made.base.id).await, ["Current text"]);
}

#[tokio::test]
async fn copied_files_have_fresh_references_independent_pins_and_survive_removal_of_the_original_file()
 {
    let desk = Desk::new().await;
    let source = desk.make(&desk.ada, "Files").await;
    let payload = b"a retained copy";
    let kept = desk
        .world
        .app
        .call(DocumentFileCreate, &[&source.base.id])
        .token(&desk.ada.token)
        .query(&DocumentFileCreateQuery { overwrite: None })
        .header(FILENAME_HEADER, "notes.txt")
        .bytes(payload)
        .send()
        .await
        .ok();
    let block = NewBlock {
        id: new_id(),
        block_type: BlockType::File,
        props: Some(BlockProps::File(FileProps {
            display: None,
            file_id: kept.id,
        })),
        content: None,
        children: None,
    };
    let source_block = block.id;
    desk.write(
        &desk.ada,
        source.base.id,
        vec![DocumentOp::Insert {
            block,
            parent_id: None,
            after_id: None,
        }],
    )
    .await
    .ok();
    let source_hash =
        orch_db::queries::documents::file(&mut *desk.world.conn().await, source.base.id, kept.id)
            .await
            .unwrap()
            .unwrap()
            .blob;
    let before: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(source_hash.as_bytes().as_slice())
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
    let body = NodeDocumentCopyBody {
        source_document_id: source.base.id,
        title: Some("Copied files".to_owned()),
        access: None,
        client_id: "copy-files".to_owned(),
    };
    let copy = desk
        .world
        .app
        .call(NodeDocumentCopy, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_ne!(copy.files[0].id, kept.id);
    let copied = desk.sync(&desk.ada, copy.base.id).await;
    // Props without optional members have the same wire shape for images and files; the block's
    // type says how they are read.
    assert_eq!(copied.blocks[0].block_type, BlockType::File);
    assert_eq!(
        serde_json::to_value(&copied.blocks[0].props).unwrap(),
        json!({"file_id": copy.files[0].id}),
        "{:?} with {:?}",
        copied.blocks,
        copy.files
    );
    desk.world
        .app
        .call(NodeDocumentCopy, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    let after: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(source_hash.as_bytes().as_slice())
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
    assert_eq!(after, before + 1, "a repeat creates no further pin");
    desk.write(
        &desk.ada,
        source.base.id,
        vec![DocumentOp::Remove { id: source_block }],
    )
    .await
    .ok();
    desk.world
        .app
        .call(DocumentFileDelete, &[&source.base.id, &kept.id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    desk.world
        .app
        .call(DocumentDelete, &[&source.base.id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let bytes = desk
        .world
        .app
        .call(DocumentFileGet, &[&copy.base.id, &copy.files[0].id])
        .token(&desk.ada.token)
        .query(&DocumentFileGetQuery {
            download: None,
            ..Default::default()
        })
        .send_bytes()
        .await;
    assert_eq!(bytes.raw.as_ref(), payload);
}

#[tokio::test]
async fn mixed_markdown_and_html_sources_keep_the_title_once_and_each_loss_remains_on_a_retry() {
    let desk = Desk::new().await;
    let md = upload(&desk, &desk.ada, "one.md", b"# First\n\nA paragraph").await;
    let html = upload(&desk, &desk.ada, "two.html", b"<h1>Second</h1><ul><li>Outer<ul><li>Inner</li></ul></li></ul><table><tr><th>Name</th></tr><tr><td>Ada</td></tr></table><script>never run</script>").await;
    let files = vec![
        file(md, "one.md", Format::Markdown),
        file(html, "two.html", Format::Html),
    ];
    let first = import(&desk, files.clone(), "two-sources").await;
    assert_eq!(first.entries.len(), 2);
    assert_eq!(first.entries[0].document.as_ref().unwrap().title, "First");
    assert_eq!(first.entries[1].document.as_ref().unwrap().title, "Second");
    let second = desk
        .sync(&desk.ada, first.entries[1].document.as_ref().unwrap().id)
        .await;
    assert_eq!(
        second
            .blocks
            .iter()
            .filter(|block| block.block_type == BlockType::Bullet)
            .count(),
        2
    );
    assert!(
        second
            .blocks
            .iter()
            .any(|block| matches!(&block.props, BlockProps::Table(props) if props.header_row))
    );
    assert!(
        second
            .blocks
            .iter()
            .all(|block| block.block_type != BlockType::Heading)
    );
    assert_eq!(first.entries[1].warnings.len(), 1);
    let again = import(&desk, files, "two-sources").await;
    assert_eq!(
        first
            .entries
            .iter()
            .map(|entry| entry.document.as_ref().unwrap().id)
            .collect::<Vec<_>>(),
        again
            .entries
            .iter()
            .map(|entry| entry.document.as_ref().unwrap().id)
            .collect::<Vec<_>>()
    );
    assert_eq!(again.entries[1].warnings, first.entries[1].warnings);
}

fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[tokio::test]
async fn a_copy_larger_than_one_operation_batch_is_one_visible_document_with_all_current_blocks() {
    let desk = Desk::new().await;
    let count = usize::try_from(orch_api_contract::documents::DOCUMENT_OPS_MAX).unwrap() + 1;
    let mut body = desk::body("Many paragraphs");
    body.markdown = Some(
        (0..count)
            .map(|index| format!("Paragraph {index}"))
            .collect::<Vec<_>>()
            .join("\n\n"),
    );
    let source = desk.make_body(&desk.ada, body).await.ok();
    let destination = desk.another_topic("Copied documents").await;
    let made = desk
        .world
        .app
        .call(NodeDocumentCopy, &[&destination])
        .token(&desk.ada.token)
        .body(&NodeDocumentCopyBody {
            source_document_id: source.base.id,
            title: None,
            access: None,
            client_id: "whole-large-copy".to_owned(),
        })
        .send()
        .await
        .ok();
    assert_eq!(desk.words(&desk.ada, made.base.id).await.len(), count);
    assert_eq!(
        desk.words(&desk.ada, made.base.id).await,
        desk.words(&desk.ada, source.base.id).await
    );
}

#[tokio::test]
async fn expanded_archive_bytes_are_bounded_before_any_document_is_created() {
    let desk = Desk::new().await;
    // The compressed upload fits below the cap, while its actual text is twice that allowance.
    const ALLOWANCE: usize = 1024;
    // The largest copy is a policy of a project or of a place above it, which its topics follow.
    desk.world
        .set_at(
            orch_settings::Target::Node(desk.project),
            "sharing.snapshot_max_bytes",
            json!(ALLOWANCE),
        )
        .await;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file(
            "large.md",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
    writer.write_all(&vec![b'a'; ALLOWANCE * 2]).unwrap();
    let archive = writer.finish().unwrap().into_inner();
    assert!(archive.len() < ALLOWANCE);
    let blob = upload(&desk, &desk.ada, "expanded.zip", &archive).await;
    let result = import(
        &desk,
        vec![file(blob, "expanded.zip", Format::Archive)],
        "archive-bytes",
    )
    .await;
    assert!(result.entries[0].document.is_none());
    assert!(result.entries[0].error.is_some());
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE node_id=$1")
        .bind(desk.topic)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
    assert_eq!(total, 0);
}

#[tokio::test]
async fn archive_entries_report_unsafe_and_unsupported_sources_without_losing_the_valid_document() {
    let desk = Desk::new().await;
    let archive = zip(&[
        ("good.md", b"# Good\n\nText"),
        ("../escape.md", b"no"),
        ("/absolute.md", b"no"),
        ("program.exe", b"no"),
    ]);
    let blob = upload(&desk, &desk.ada, "writing.zip", &archive).await;
    let result = import(
        &desk,
        vec![file(blob, "writing.zip", Format::Archive)],
        "archive-paths",
    )
    .await;
    assert_eq!(result.entries.len(), 4);
    assert_eq!(
        result
            .entries
            .iter()
            .filter(|entry| entry.document.is_some())
            .count(),
        1
    );
    assert!(
        result.entries[1..]
            .iter()
            .all(|entry| entry.error.is_some())
    );
}

#[tokio::test]
async fn an_archive_over_the_entry_bound_and_a_refused_entry_create_no_unreported_documents() {
    let desk = Desk::new().await;
    let names: Vec<_> = (0..=orch_api_contract::common::MAX_LIMIT)
        .map(|index| format!("{index}.md"))
        .collect();
    let entries: Vec<_> = names
        .iter()
        .map(|name| (name.as_str(), b"text".as_slice()))
        .collect();
    let archive = zip(&entries);
    let blob = upload(&desk, &desk.ada, "too-many.zip", &archive).await;
    let result = import(
        &desk,
        vec![file(blob, "too-many.zip", Format::Archive)],
        "archive-count",
    )
    .await;
    assert_eq!(result.entries.len(), 1);
    assert!(result.entries[0].document.is_none());
    assert!(result.entries[0].error.is_some());
    let bad = upload(&desk, &desk.ada, "bad.md", &[0xff, 0xfe]).await;
    let good = upload(&desk, &desk.ada, "good.md", b"# Fine\n\nKept").await;
    let result = import(
        &desk,
        vec![
            file(bad, "bad.md", Format::Markdown),
            file(good, "good.md", Format::Markdown),
        ],
        "mixed-result",
    )
    .await;
    assert!(result.entries[0].error.is_some());
    assert_eq!(result.entries[1].document.as_ref().unwrap().title, "Fine");
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE node_id=$1")
        .bind(desk.topic)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
    assert_eq!(total, 1);
}
