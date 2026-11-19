//! An archive of texts with what they name: links between its documents are references, its pictures are files of the documents that show them, what leads nowhere stays words and is said, and Word and plain-text files are read as the others.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use orch_api_contract::documents::{
    BlockContent, BlockProps, BlockType, DocumentGet, DocumentImportRequest,
    DocumentImportRequestFile, DocumentImportRequestFileFormat as Format, DocumentImportResult,
    Inline, NodeDocumentImport,
};
use orch_api_contract::threads::{
    UploadSessionComplete, UploadSessionCreate, UploadSessionCreateBody, UploadSessionPut,
    UploadSessionPutQuery,
};
use orch_types::{Hash, Id};
use places::Who;
use std::io::Write;

/// A picture of one pixel.
const PICTURE: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xfc, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x85, 0x01, 0x80, 0x84, 0xa9, 0x8c, 0x21, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

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
    blob: Hash,
    name: &str,
    format: Format,
    client: &str,
) -> DocumentImportResult {
    desk.world
        .app
        .call(NodeDocumentImport, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&DocumentImportRequest {
            files: vec![DocumentImportRequestFile {
                blob,
                name: name.to_owned(),
                format,
            }],
            client_id: client.to_owned(),
        })
        .send()
        .await
        .ok()
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

/// The references that the blocks of a document hold, as (what they say, the document they name).
async fn chips(desk: &Desk, document: Id) -> Vec<(String, Id)> {
    let mut out = Vec::new();
    for block in desk.page(&desk.ada, document).await {
        if let BlockContent::Inline(runs) = &block.content {
            for run in runs {
                if let Inline::Ref { id, label, .. } = run {
                    out.push((label.clone().unwrap_or_default(), *id));
                }
            }
        }
    }
    out
}

#[tokio::test]
async fn a_notion_style_archive_becomes_linked_documents_with_their_pictures() {
    let desk = Desk::new().await;
    let home = "# Home\n\nStart with [the guide](Guide%203c4d.md).\n\n![the shot](Home/shot.png)\n";
    let guide = "# Guide\n\nBack to [home](Home%201a2b.md). More in [elsewhere](Nowhere.md).\n";
    let archive = zip(&[
        ("Home 1a2b.md", home.as_bytes()),
        ("Guide 3c4d.md", guide.as_bytes()),
        ("Home/shot.png", PICTURE),
        ("Home/unused.csv", b"a,b\n1,2\n"),
    ]);
    let blob = upload(&desk, &desk.ada, "export.zip", &archive).await;
    let result = import(&desk, blob, "export.zip", Format::Archive, "notion-like").await;
    assert_eq!(result.entries.len(), 2, "{:?}", result.entries);
    for entry in &result.entries {
        assert!(entry.error.is_none(), "{entry:?}");
    }
    let home = result
        .entries
        .iter()
        .find(|entry| entry.name == "Home 1a2b.md")
        .and_then(|entry| entry.document.as_ref())
        .expect("home");
    let guide = result
        .entries
        .iter()
        .find(|entry| entry.name == "Guide 3c4d.md")
        .expect("guide");
    let guide_id = guide.document.as_ref().expect("guide document").id;
    assert_eq!(home.title, "Home");

    // A link from one entry to another is a reference to the document it became, both ways.
    assert_eq!(
        chips(&desk, home.id).await,
        vec![("the guide".to_owned(), guide_id)]
    );
    assert_eq!(
        chips(&desk, guide_id).await,
        vec![("home".to_owned(), home.id)]
    );
    // What leads nowhere is the words it was written with, and the entry says so.
    assert!(
        guide
            .warnings
            .iter()
            .any(|warning| warning.contains("Nowhere.md")),
        "{:?}",
        guide.warnings
    );

    // The picture is a file of the document that shows it, and only of that one.
    let document = desk
        .world
        .app
        .call(DocumentGet, &[&home.id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(document.files.len(), 1, "{:?}", document.files);
    assert_eq!(document.files[0].name, "shot.png");
    assert_eq!(document.files[0].media_type, "image/png");
    let shown = desk
        .page(&desk.ada, home.id)
        .await
        .into_iter()
        .find(|block| block.block_type == BlockType::Image)
        .expect("an image block");
    let BlockProps::Image(props) = &shown.props else {
        panic!("{:?}", shown.props);
    };
    assert_eq!(props.file_id, document.files[0].id);
    let other = desk
        .world
        .app
        .call(DocumentGet, &[&guide_id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert!(other.files.is_empty());
}

#[tokio::test]
async fn a_picture_that_is_not_in_the_archive_stays_words_and_the_entry_says_so() {
    let desk = Desk::new().await;
    let archive = zip(&[(
        "Page.md",
        b"# Page\n\n![lost](images/lost.png)\n".as_slice(),
    )]);
    let blob = upload(&desk, &desk.ada, "one.zip", &archive).await;
    let result = import(&desk, blob, "one.zip", Format::Archive, "lost-picture").await;
    assert_eq!(result.entries.len(), 1);
    let entry = &result.entries[0];
    assert!(entry.error.is_none(), "{entry:?}");
    assert!(
        entry
            .warnings
            .iter()
            .any(|warning| warning.contains("images/lost.png")),
        "{:?}",
        entry.warnings
    );
    let document = entry.document.as_ref().unwrap();
    let words = desk.words(&desk.ada, document.id).await;
    assert!(words.iter().any(|line| line.contains("lost")), "{words:?}");
}

#[tokio::test]
async fn plain_text_is_imported_and_a_file_of_another_kind_is_still_refused_by_name() {
    let desk = Desk::new().await;
    let archive = zip(&[
        ("notes.txt", b"first\n\nsecond\n".as_slice()),
        ("program.exe", b"no".as_slice()),
    ]);
    let blob = upload(&desk, &desk.ada, "mixed.zip", &archive).await;
    let result = import(&desk, blob, "mixed.zip", Format::Archive, "text-and-exe").await;
    assert_eq!(result.entries.len(), 2, "{:?}", result.entries);
    let notes = result
        .entries
        .iter()
        .find(|entry| entry.name == "notes.txt")
        .unwrap();
    assert!(notes.document.is_some(), "{notes:?}");
    let program = result
        .entries
        .iter()
        .find(|entry| entry.name == "program.exe")
        .unwrap();
    assert!(program.error.is_some() && program.document.is_none());
}
