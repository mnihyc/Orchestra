//! A document exported with its files carries their bytes: an HTML page holds its pictures inside it and, with the attachments asked for, is an archive of the page and of them; Typst source is an archive with the pictures it shows and the files asked for.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use desk::Desk;
use orch_api_contract::documents::{
    BlockProps, BlockType, DocumentExport, DocumentExportChoice, DocumentExportGet,
    DocumentExportGetQuery, DocumentFile, DocumentFileCreate, DocumentFileCreateQuery, FileProps,
    ImageProps, NewBlock,
};
use orch_api_contract::threads::FILENAME_HEADER;
use orch_api_core::testing::RawReply;
use orch_types::{Id, new_id};
use places::Who;
use std::io::Read;

fn png() -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.extend_from_slice(b"a picture of the plan, as far as a test draws one");
    bytes
}

const NOTES: &[u8] = b"the notes beside the plan";

/// `by` puts a file into `document`, sending its bytes as the body.
async fn put_file(desk: &Desk, by: &Who, document: Id, name: &str, bytes: &[u8]) -> DocumentFile {
    desk.world
        .app
        .call(DocumentFileCreate, &[&document])
        .token(&by.token)
        .query(&DocumentFileCreateQuery::default())
        .header(FILENAME_HEADER, name)
        .bytes(bytes)
        .send()
        .await
        .ok()
}

/// A document that shows a picture and names a file of notes.
async fn handbook(desk: &Desk) -> Id {
    let id = desk.make(&desk.ada, "Handbook").await.base.id;
    let picture = put_file(desk, &desk.ada, id, "picture.png", &png()).await;
    let notes = put_file(desk, &desk.ada, id, "notes.txt", NOTES).await;
    let shows = NewBlock {
        id: new_id(),
        block_type: BlockType::Image,
        props: Some(BlockProps::Image(ImageProps {
            display: None,
            file_id: picture.id,
            width: None,
            align: None,
        })),
        content: None,
        children: None,
    };
    let names = NewBlock {
        id: new_id(),
        block_type: BlockType::File,
        props: Some(BlockProps::File(FileProps {
            display: None,
            file_id: notes.id,
        })),
        content: None,
        children: None,
    };
    desk.write(&desk.ada, id, vec![desk::put(names), desk::put(shows)])
        .await
        .ok();
    id
}

async fn export(desk: &Desk, document: Id, format: DocumentExport, attachments: bool) -> RawReply {
    desk.world
        .app
        .call(DocumentExportGet, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentExportGetQuery {
            format: Some(format),
            attachments: attachments.then_some(DocumentExportChoice::Include),
            comments: None,
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

/// The entries of an archive, each with its bytes.
fn entries(reply: &RawReply) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(reply.raw.to_vec())).unwrap();
    (0..archive.len())
        .map(|index| {
            let mut file = archive.by_index(index).unwrap();
            let mut content = Vec::new();
            file.read_to_end(&mut content).unwrap();
            (file.name().unwrap().into_owned(), content)
        })
        .collect()
}

fn entry<'a>(files: &'a [(String, Vec<u8>)], name: &str) -> &'a [u8] {
    &files
        .iter()
        .find(|(each, _)| each == name)
        .unwrap_or_else(|| panic!("no entry {name} among {:?}", names(files)))
        .1
}

fn names(files: &[(String, Vec<u8>)]) -> Vec<&str> {
    files.iter().map(|(name, _)| name.as_str()).collect()
}

#[tokio::test]
async fn an_html_page_holds_its_picture_inside_it_and_with_its_attachments_is_an_archive_of_them() {
    let desk = Desk::new().await;
    let id = handbook(&desk).await;
    let inside = format!("data:image/png;base64,{}", STANDARD.encode(png()));

    let alone = export(&desk, id, DocumentExport::Html, false).await;
    assert_eq!(alone.status, 200);
    assert!(header(&alone, "content-type").starts_with("text/html"));
    let page = String::from_utf8(alone.raw.to_vec()).unwrap();
    assert!(page.contains(&inside), "the picture is in the page: {page}");
    assert!(
        page.contains("not in this file"),
        "the notes are named and not carried: {page}"
    );

    let with_files = export(&desk, id, DocumentExport::Html, true).await;
    assert_eq!(with_files.status, 200);
    assert_eq!(header(&with_files, "content-type"), "application/zip");
    let files = entries(&with_files);
    let page = String::from_utf8(entry(&files, "Handbook.html").to_vec()).unwrap();
    assert!(page.contains(&inside));
    assert!(page.contains("href=\"files/notes.txt\""), "{page}");
    assert_eq!(entry(&files, "files/notes.txt"), NOTES);
}

#[tokio::test]
async fn a_typst_source_comes_with_the_picture_it_shows_and_the_files_asked_for() {
    let desk = Desk::new().await;
    let id = handbook(&desk).await;

    let alone = entries(&export(&desk, id, DocumentExport::Typst, false).await);
    let source = String::from_utf8(entry(&alone, "main.typ").to_vec()).unwrap();
    let (picture, bytes) = alone
        .iter()
        .find(|(name, _)| name.starts_with("images/"))
        .unwrap_or_else(|| panic!("no picture among {:?}", names(&alone)));
    assert_eq!(bytes, &png(), "the picture's own bytes");
    assert!(
        source.contains(&format!("image(\"{picture}\"")),
        "the source shows it from where it lies: {source}"
    );
    assert!(
        !alone.iter().any(|(name, _)| name.starts_with("files/")),
        "the notes stay out unless they are asked for: {:?}",
        names(&alone)
    );

    let with_files = entries(&export(&desk, id, DocumentExport::Typst, true).await);
    assert_eq!(entry(&with_files, "files/notes.txt"), NOTES);
    let source = String::from_utf8(entry(&with_files, "main.typ").to_vec()).unwrap();
    assert!(source.contains("#link(\"files/notes.txt\")"), "{source}");
}
