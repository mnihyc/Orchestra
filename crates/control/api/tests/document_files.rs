//! Files in a document over HTTP: put in from a body or from a finished upload, kept as long as the document holds them, shown or offered as downloads, read by whoever the document's page lets read it, bounded by what its place allows, and the archives among them looked into one entry at a time.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use http::StatusCode;
use orch_api_contract::access::{
    AccessChoice, HolderInput, PageChange, PageChangeName, PermissionPageUpdate,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    BlockProps, BlockType, DocumentFile, DocumentFileArchiveEntryGet,
    DocumentFileArchiveEntryGetQuery, DocumentFileArchiveList, DocumentFileArchiveListQuery,
    DocumentFileBlobBody, DocumentFileCreate, DocumentFileCreateQuery, DocumentFileDelete,
    DocumentFileGet, DocumentFileGetQuery, DocumentGet, FileProps, NewBlock, NewDocumentAccess,
};
use orch_api_contract::nodes::NodeArchive;
use orch_api_contract::threads::{
    FILENAME_HEADER, UploadSessionComplete, UploadSessionCreate, UploadSessionCreateBody,
    UploadSessionPut, UploadSessionPutQuery,
};
use orch_api_contract::workspace::{ArchiveCompression, ArchiveEntryKind, ArchiveFormat};
use orch_api_core::testing::{RawReply, Reply};
use orch_settings::Target;
use orch_types::{Hash, Id, new_id};
use places::Who;
use serde_json::json;
use std::io::Write;

fn png() -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.extend_from_slice(b"not really a picture, but it begins as one");
    bytes
}

/// `by` puts a file into `document`, sending its bytes as the body.
async fn put(
    desk: &Desk,
    by: &Who,
    document: Id,
    name: &str,
    bytes: &[u8],
    overwrite: bool,
) -> Reply<DocumentFile> {
    desk.world
        .app
        .call(DocumentFileCreate, &[&document])
        .token(&by.token)
        .query(&DocumentFileCreateQuery {
            overwrite: overwrite.then_some(true),
        })
        .header(FILENAME_HEADER, name)
        .bytes(bytes)
        .send()
        .await
}

/// `by` asks for a file of `document`.
async fn get(
    desk: &Desk,
    by: &Who,
    document: Id,
    file: Id,
    download: Option<bool>,
    range: Option<&str>,
) -> RawReply {
    let call = desk
        .world
        .app
        .call(DocumentFileGet, &[&document, &file])
        .token(&by.token)
        .query(&DocumentFileGetQuery {
            download,
            ..Default::default()
        });
    let call = match range {
        Some(range) => call.header("range", range),
        None => call,
    };
    call.send_bytes().await
}

fn header<'a>(reply: &'a RawReply, name: &str) -> &'a str {
    reply
        .headers
        .get(name)
        .unwrap_or_else(|| panic!("no header {name}"))
        .to_str()
        .unwrap()
}

async fn pins(desk: &Desk, hash: &Hash) -> i32 {
    sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash = $1")
        .bind(hash.as_bytes().to_vec())
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap()
}

/// A block that shows a file of the document.
fn showing(file: Id) -> NewBlock {
    NewBlock {
        id: new_id(),
        block_type: BlockType::File,
        props: Some(BlockProps::File(FileProps {
            display: None,
            file_id: file,
        })),
        content: None,
        children: None,
    }
}

async fn files_of(desk: &Desk, by: &Who, document: Id) -> Vec<(String, i64)> {
    desk.world
        .app
        .call(DocumentGet, &[&document])
        .token(&by.token)
        .send()
        .await
        .ok()
        .files
        .into_iter()
        .map(|file| (file.name, file.size))
        .collect()
}

// ---------------------------------------------------------------- putting a file in

#[tokio::test]
async fn a_file_is_put_into_a_document_and_listed_with_what_it_is() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let before = desk.sync(&desk.ada, made.base.id).await.seq;
    let image = png();
    let kept = put(&desk, &desk.ada, made.base.id, "diagram.png", &image, false)
        .await
        .ok();
    assert_eq!(kept.name, "diagram.png");
    assert_eq!(kept.media_type, "image/png");
    assert_eq!(kept.size, i64::try_from(image.len()).unwrap());
    assert!(!kept.is_text);
    assert!(
        matches!(&kept.added_by, Some(orch_api_contract::documents::Author::User(user)) if user.username == "ada")
    );
    let note = put(
        &desk,
        &desk.ada,
        made.base.id,
        "notes.md",
        b"# words\n",
        false,
    )
    .await
    .ok();
    assert_eq!(note.media_type, "text/markdown");
    assert!(
        note.is_text,
        "a file whose bytes are text can be read where it lies"
    );

    let read = desk
        .world
        .app
        .call(DocumentGet, &[&made.base.id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let names: Vec<&str> = read.files.iter().map(|file| file.name.as_str()).collect();
    assert_eq!(names, ["diagram.png", "notes.md"]);
    assert_eq!(
        read.base.files_bytes,
        i64::try_from(image.len()).unwrap() + 8
    );
    assert!(
        desk.sync(&desk.ada, made.base.id).await.seq > before,
        "putting a file in stamps the document, so that every open copy lists it"
    );
    assert!(
        desk.words(&desk.ada, made.base.id).await.is_empty(),
        "the text is not touched"
    );
    // The document holds the bytes: they are pinned for as long as it does.
    assert_eq!(
        pins(&desk, &blob_of(&desk, made.base.id, kept.id).await).await,
        1
    );
}

/// The hash of the blob that holds a file of a document, as the database has it.
async fn blob_of(desk: &Desk, document: Id, file: Id) -> Hash {
    let raw: Vec<u8> =
        sqlx::query_scalar("SELECT blob FROM document_files WHERE document_id = $1 AND id = $2")
            .bind(document)
            .bind(file)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    Hash::from_slice(&raw).unwrap()
}

#[tokio::test]
async fn a_second_file_of_the_same_name_is_refused_unless_it_is_to_replace_the_first() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let id = made.base.id;
    let first = put(&desk, &desk.ada, id, "data.csv", b"a,b\n1,2\n", false)
        .await
        .ok();
    let before = blob_of(&desk, id, first.id).await;
    desk.write(&desk.ada, id, vec![desk::put(showing(first.id))])
        .await
        .ok();
    let refused = put(&desk, &desk.ada, id, "data.csv", b"other", false)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictExists);
    assert_eq!(refused.extra["existing_id"], json!(first.id));
    assert_eq!(
        files_of(&desk, &desk.ada, id).await,
        [("data.csv".to_owned(), 8)]
    );

    let replaced = put(&desk, &desk.ada, id, "data.csv", b"c,d,e\n3,4,5\n", true)
        .await
        .ok();
    assert_eq!(
        replaced.id, first.id,
        "the blocks that name a file show the new bytes"
    );
    assert_eq!(replaced.size, 12);
    let read = get(&desk, &desk.ada, id, first.id, None, None).await;
    assert_eq!(read.raw.as_ref(), b"c,d,e\n3,4,5\n");
    assert_eq!(
        files_of(&desk, &desk.ada, id).await,
        [("data.csv".to_owned(), 12)]
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentGet, &[&id])
            .token(&desk.ada.token)
            .send()
            .await
            .ok()
            .base
            .files_bytes,
        12,
        "what was replaced no longer counts"
    );
    assert_eq!(pins(&desk, &before).await, 0, "the old bytes are let go");
    assert_eq!(pins(&desk, &blob_of(&desk, id, replaced.id).await).await, 1);

    // The same bytes put in again stay pinned once.
    put(&desk, &desk.ada, id, "data.csv", b"c,d,e\n3,4,5\n", true)
        .await
        .ok();
    assert_eq!(pins(&desk, &blob_of(&desk, id, replaced.id).await).await, 1);
}

#[tokio::test]
async fn a_document_holds_files_up_to_what_its_place_allows() {
    let desk = Desk::new().await;
    desk.world
        .set_at(
            Target::Node(desk.topic),
            "docs.file_max_bytes",
            json!(1_000),
        )
        .await;
    desk.world
        .set_at(
            Target::Node(desk.topic),
            "docs.files_max_bytes",
            json!(1_500),
        )
        .await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let id = made.base.id;
    put(&desk, &desk.ada, id, "one.bin", &[1u8; 900], false)
        .await
        .ok();

    let too_big = put(&desk, &desk.ada, id, "big.bin", &[2u8; 1_100], false)
        .await
        .problem();
    assert_eq!(too_big.code, ErrorCode::DocsTooLarge);
    assert_eq!(too_big.status, 413);
    assert_eq!(too_big.extra["limit"], "file_bytes");
    assert_eq!(too_big.extra["value"], 1_000);
    assert_eq!(too_big.extra["scope"]["id"], json!(desk.topic));

    let too_many = put(&desk, &desk.ada, id, "two.bin", &[3u8; 900], false)
        .await
        .problem();
    assert_eq!(too_many.code, ErrorCode::DocsTooLarge);
    assert_eq!(too_many.extra["limit"], "files_bytes");
    assert_eq!(too_many.extra["value"], 1_500);
    assert_eq!(too_many.extra["scope"]["id"], json!(desk.topic));
    assert_eq!(
        files_of(&desk, &desk.ada, id).await,
        [("one.bin".to_owned(), 900)],
        "a refused file leaves nothing in the document"
    );

    // A file that replaces another counts for the difference.
    put(&desk, &desk.ada, id, "one.bin", &[4u8; 1_000], true)
        .await
        .ok();
    put(&desk, &desk.ada, id, "two.bin", &[5u8; 500], false)
        .await
        .ok();
    let full = put(&desk, &desk.ada, id, "three.bin", &[6u8; 1], false)
        .await
        .problem();
    assert_eq!(full.extra["limit"], "files_bytes");
}

#[tokio::test]
async fn a_body_larger_than_a_request_carries_is_told_to_go_in_pieces_and_leaves_nothing() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let limit = i64::try_from(orch_api_contract::common::MAX_BODY_BYTES).unwrap();
    let bytes = vec![9u8; usize::try_from(limit).unwrap() + 1];
    let refused = put(&desk, &desk.ada, made.base.id, "huge.bin", &bytes, false)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestTooLarge);
    assert_eq!(refused.extra["limit"], limit);
    assert!(
        !desk
            .world
            .app
            .app
            .blobs
            .store()
            .exists(&Hash::of(&bytes))
            .await
            .unwrap()
    );
}

async fn upload_in_pieces(desk: &Desk, by: &Who, name: &str, bytes: &[u8]) -> Hash {
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
    for chunk in bytes.chunks(1_000) {
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

#[tokio::test]
async fn a_finished_upload_in_pieces_is_put_in_by_its_hash_and_a_name() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let id = made.base.id;
    let bytes: Vec<u8> = (0..6_000)
        .map(|at| u8::try_from(at % 253).unwrap())
        .collect();
    let blob = upload_in_pieces(&desk, &desk.ada, "film.mp4", &bytes).await;
    let kept = desk
        .world
        .app
        .call(DocumentFileCreate, &[&id])
        .token(&desk.ada.token)
        .query(&DocumentFileCreateQuery::default())
        .raw_body(
            &serde_json::to_value(DocumentFileBlobBody {
                blob,
                name: "film.mp4".to_owned(),
            })
            .unwrap(),
        )
        .send()
        .await
        .ok();
    assert_eq!(kept.media_type, "video/mp4");
    assert_eq!(kept.size, 6_000);
    let read = get(&desk, &desk.ada, id, kept.id, None, None).await;
    assert_eq!(read.raw.as_ref(), bytes.as_slice());

    // A hash the store does not hold names nothing.
    let unknown = desk
        .world
        .app
        .call(DocumentFileCreate, &[&id])
        .token(&desk.ada.token)
        .query(&DocumentFileCreateQuery::default())
        .raw_body(&json!({ "blob": Hash::of(b"never uploaded"), "name": "x.bin" }))
        .send()
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::NotFound);

    // And one that is larger than the place allows is refused with the limit.
    desk.world
        .set_at(
            Target::Node(desk.topic),
            "docs.file_max_bytes",
            json!(5_000),
        )
        .await;
    let refused = desk
        .world
        .app
        .call(DocumentFileCreate, &[&id])
        .token(&desk.ada.token)
        .query(&DocumentFileCreateQuery {
            overwrite: Some(true),
        })
        .raw_body(&json!({ "blob": blob, "name": "film.mp4" }))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::DocsTooLarge);
    assert_eq!(refused.extra["limit"], "file_bytes");
    assert_eq!(refused.extra["value"], 5_000);
}

#[tokio::test]
async fn a_file_needs_a_name_either_in_the_header_or_with_the_hash_of_an_upload() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    for reply in [
        desk.world
            .app
            .call(DocumentFileCreate, &[&made.base.id])
            .token(&desk.ada.token)
            .query(&DocumentFileCreateQuery::default())
            .bytes(b"nameless")
            .send()
            .await,
        desk.world
            .app
            .call(DocumentFileCreate, &[&made.base.id])
            .token(&desk.ada.token)
            .query(&DocumentFileCreateQuery::default())
            .raw_body(&json!({ "name": "only a name" }))
            .send()
            .await,
    ] {
        assert_eq!(reply.problem().code, ErrorCode::RequestInvalid);
    }
}

// ---------------------------------------------------------------- reading a file

#[tokio::test]
async fn pictures_films_and_sounds_are_shown_with_ranges_and_everything_else_is_a_download() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let id = made.base.id;
    let film: Vec<u8> = (0..500).map(|at| u8::try_from(at % 251).unwrap()).collect();
    for (name, bytes, kind, inline) in [
        ("diagram.png", png(), "image/png", true),
        ("clip.mp4", film.clone(), "video/mp4", true),
        ("song.mp3", b"sound".to_vec(), "audio/mpeg", true),
        (
            "report.pdf",
            b"%PDF-1.7 body".to_vec(),
            "application/pdf",
            false,
        ),
        ("data.csv", b"a,b\n1,2\n".to_vec(), "text/csv", false),
        ("run.sh", b"echo hi\n".to_vec(), "text/plain", false),
        (
            "page.html",
            b"<script>1</script>".to_vec(),
            "text/html",
            false,
        ),
        ("vector.svg", b"<svg/>".to_vec(), "image/svg+xml", false),
    ] {
        let kept = put(&desk, &desk.ada, id, name, &bytes, false).await.ok();
        assert_eq!(kept.media_type, kind, "{name}");
        let read = get(&desk, &desk.ada, id, kept.id, None, None).await;
        assert_eq!(read.status, StatusCode::OK, "{name}");
        assert_eq!(read.raw.as_ref(), bytes.as_slice(), "{name}");
        let served = match name {
            "page.html" | "vector.svg" => "application/octet-stream",
            _ => kind,
        };
        assert_eq!(header(&read, "content-type"), served, "{name}");
        let disposition = header(&read, "content-disposition");
        assert_eq!(
            disposition.starts_with("inline;"),
            inline,
            "{name}: {disposition}"
        );
        assert!(disposition.contains(name), "{name}: {disposition}");
        assert_eq!(header(&read, "accept-ranges"), "bytes");
        assert_eq!(header(&read, "etag"), format!("\"{}\"", Hash::of(&bytes)));
        assert_eq!(header(&read, "x-content-type-options"), "nosniff");

        // A download is asked for.
        let asked = get(&desk, &desk.ada, id, kept.id, Some(true), None).await;
        assert!(header(&asked, "content-disposition").starts_with("attachment;"));
        assert_eq!(header(&asked, "content-type"), served, "{name}");
    }

    // A film is played from a range, and so is a download.
    let clip = files_of(&desk, &desk.ada, id)
        .await
        .into_iter()
        .position(|(name, _)| name == "clip.mp4")
        .unwrap();
    let listed = desk
        .world
        .app
        .call(DocumentGet, &[&id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok()
        .files;
    let part = get(
        &desk,
        &desk.ada,
        id,
        listed[clip].id,
        None,
        Some("bytes=100-199"),
    )
    .await;
    assert_eq!(part.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.raw.as_ref(), &film[100..200]);
    assert_eq!(header(&part, "content-range"), "bytes 100-199/500");
    let beyond = get(
        &desk,
        &desk.ada,
        id,
        listed[clip].id,
        None,
        Some("bytes=900-"),
    )
    .await;
    assert_eq!(beyond.status, StatusCode::RANGE_NOT_SATISFIABLE);
    let tag = format!("\"{}\"", Hash::of(&film));
    let cached = desk
        .world
        .app
        .call(DocumentFileGet, &[&id, &listed[clip].id])
        .token(&desk.ada.token)
        .query(&DocumentFileGetQuery::default())
        .header("if-none-match", &tag)
        .send_bytes()
        .await;
    assert_eq!(cached.status, StatusCode::NOT_MODIFIED);
}

// ---------------------------------------------------------------- who reads and writes a file

#[tokio::test]
async fn a_file_follows_the_page_of_its_document() {
    let desk = Desk::new().await;
    let vera = desk.member("vera", "viewer").await;
    let ines = desk.member("ines", "interactor").await;
    let sam = desk.stranger("sam").await;
    let open = desk.make(&desk.ada, "Open").await;
    let private = desk
        .make_body(
            &desk.ada,
            orch_api_contract::documents::NodeDocumentCreateBody {
                access: Some(NewDocumentAccess::Private),
                ..desk::body("Closed")
            },
        )
        .await
        .ok();
    let bytes = b"words in a file".to_vec();
    let in_open = put(&desk, &desk.ada, open.base.id, "a.txt", &bytes, false)
        .await
        .ok();
    let in_private = put(&desk, &desk.ada, private.base.id, "b.txt", &bytes, false)
        .await
        .ok();

    // The document that follows its node is read by those who see the place, and written by those who write there.
    for reader in [&vera, &ines] {
        let read = get(&desk, reader, open.base.id, in_open.id, None, None).await;
        assert_eq!(read.raw.as_ref(), bytes.as_slice());
    }
    let refused = put(&desk, &vera, open.base.id, "c.txt", b"x", false)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(places::missing_permissions(&refused), ["doc.edit"]);
    put(&desk, &ines, open.base.id, "c.txt", b"x", false)
        .await
        .ok();
    // A stranger to the org reads nothing.
    let stranger = get(&desk, &sam, open.base.id, in_open.id, None, None).await;
    assert_eq!(stranger.status, StatusCode::FORBIDDEN);
    assert_eq!(
        places::missing_permissions(&stranger.problem()),
        ["doc.view"]
    );

    // The private one is its owners' alone, and its files with it.
    for outsider in [&vera, &ines, &sam] {
        let refused = get(&desk, outsider, private.base.id, in_private.id, None, None).await;
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
        assert_eq!(refused.problem().code, ErrorCode::AuthzDenied);
    }
    let refused = put(&desk, &ines, private.base.id, "z.txt", b"x", false)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(
        get(&desk, &desk.ada, private.base.id, in_private.id, None, None)
            .await
            .status,
        StatusCode::OK
    );
    // Sharing it is the page: Vera is named in its View row and reads its files from then on, and
    // writes none.
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &private.base.id])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: vera.id() }],
                choice: AccessChoice::FullView,
                options: None,
                expires_at: None,
                note: None,
                export: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        get(&desk, &vera, private.base.id, in_private.id, None, None)
            .await
            .raw
            .as_ref(),
        bytes.as_slice()
    );
    assert_eq!(
        put(&desk, &vera, private.base.id, "z.txt", b"x", false)
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        get(&desk, &ines, private.base.id, in_private.id, None, None)
            .await
            .status,
        StatusCode::FORBIDDEN,
        "the others stay out"
    );

    // A file is found only among the files of the document in the path, whatever else the caller reads.
    let crossed = get(&desk, &desk.ada, open.base.id, in_private.id, None, None).await;
    assert_eq!(crossed.status, StatusCode::NOT_FOUND);
    let nothing = get(&desk, &desk.ada, open.base.id, new_id(), None, None).await;
    assert_eq!(nothing.status, StatusCode::NOT_FOUND);
    // Nobody who is not signed in.
    let anonymous = desk
        .world
        .app
        .call(DocumentFileGet, &[&open.base.id, &in_open.id])
        .query(&DocumentFileGetQuery::default())
        .send_bytes()
        .await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_document_that_is_archived_is_read_and_takes_no_file_and_one_that_is_deleted_is_not_there()
 {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let id = made.base.id;
    let kept = put(&desk, &desk.ada, id, "a.txt", b"words", false)
        .await
        .ok();
    desk.world
        .app
        .call(NodeArchive, &[&desk.project])
        .token(&desk.ada.token)
        .body(&places::archive_body(&desk.world, &desk.ada.token, desk.project, true).await)
        .send()
        .await
        .ok();
    let refused = put(&desk, &desk.ada, id, "b.txt", b"x", false)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NodeArchived);
    let read = get(&desk, &desk.ada, id, kept.id, None, None).await;
    assert_eq!(
        read.status,
        StatusCode::OK,
        "an archived place is read-only"
    );
}

#[tokio::test]
async fn a_deleted_document_gives_up_none_of_its_files() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let id = made.base.id;
    let kept = put(&desk, &desk.ada, id, "a.txt", b"words", false)
        .await
        .ok();
    desk.world
        .app
        .call(orch_api_contract::documents::DocumentDelete, &[&id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        get(&desk, &desk.ada, id, kept.id, None, None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        put(&desk, &desk.ada, id, "b.txt", b"x", false)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn taking_a_file_out_lets_go_of_its_bytes_and_a_block_that_names_it_keeps_it_in() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let id = made.base.id;
    let kept = put(&desk, &desk.ada, id, "a.txt", b"words", false)
        .await
        .ok();
    let hash = blob_of(&desk, id, kept.id).await;
    assert_eq!(pins(&desk, &hash).await, 1);
    let block = showing(kept.id);
    let block_id = block.id;
    desk.write(&desk.ada, id, vec![desk::put(block)]).await.ok();
    let delete = || async {
        desk.world
            .app
            .call(DocumentFileDelete, &[&id, &kept.id])
            .token(&desk.ada.token)
            .send()
            .await
    };
    assert_eq!(delete().await.problem().code, ErrorCode::ResourceInUse);
    assert_eq!(pins(&desk, &hash).await, 1);
    desk.write(
        &desk.ada,
        id,
        vec![orch_api_contract::documents::DocumentOp::Remove { id: block_id }],
    )
    .await
    .ok();
    assert_eq!(delete().await.status, 204);
    assert_eq!(
        pins(&desk, &hash).await,
        0,
        "the document no longer holds the bytes"
    );
    assert_eq!(
        get(&desk, &desk.ada, id, kept.id, None, None).await.status,
        StatusCode::NOT_FOUND
    );
}

// ---------------------------------------------------------------- archives

fn zip_bytes(encrypted: bool) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("top.txt", options).unwrap();
    writer.write_all(b"top level").unwrap();
    writer.add_directory("docs/", options).unwrap();
    writer
        .start_file(
            "docs/readme.md",
            options.compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
    writer.write_all(&b"# readme ".repeat(100)).unwrap();
    writer.start_file("docs/deep/er/file.txt", options).unwrap();
    writer.write_all(b"deep").unwrap();
    writer
        .start_file(
            "big.bin",
            options.compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
    writer.write_all(&vec![0u8; 2 * 1024 * 1024 + 1]).unwrap();
    let mut bytes = writer.finish().unwrap().into_inner();
    if encrypted {
        // Marks every entry as asking for a password: bit 0 of the flags in each header.
        let mut at = 0;
        while let Some(found) = bytes[at..]
            .windows(4)
            .position(|w| w == b"PK\x03\x04" || w == b"PK\x01\x02")
        {
            let header = at + found;
            let flags = if &bytes[header..header + 4] == b"PK\x03\x04" {
                header + 6
            } else {
                header + 8
            };
            bytes[flags] |= 1;
            at = header + 4;
        }
    }
    bytes
}

fn add_file(builder: &mut tar::Builder<Vec<u8>>, name: &str, content: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(1_700_000_000);
    header.set_cksum();
    builder.append_data(&mut header, name, content).unwrap();
}

fn tar_bytes() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    add_file(&mut builder, "top.txt", b"top level");
    let mut dir = tar::Header::new_gnu();
    dir.set_entry_type(tar::EntryType::Directory);
    dir.set_size(0);
    dir.set_mode(0o755);
    dir.set_mtime(1_700_000_100);
    dir.set_cksum();
    builder
        .append_data(&mut dir, "docs/", std::io::empty())
        .unwrap();
    add_file(&mut builder, "docs/readme.md", &b"# readme ".repeat(100));
    add_file(&mut builder, "docs/deep/er/file.txt", b"deep");
    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_size(0);
    link.set_cksum();
    builder.append_link(&mut link, "latest", "top.txt").unwrap();
    builder.into_inner().unwrap()
}

fn compressed(kind: Option<ArchiveCompression>, plain: &[u8]) -> Vec<u8> {
    match kind {
        None => plain.to_vec(),
        Some(ArchiveCompression::Gzip) => {
            let mut out = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            out.write_all(plain).unwrap();
            out.finish().unwrap()
        }
        Some(ArchiveCompression::Zstd) => zstd::encode_all(plain, 1).unwrap(),
        Some(ArchiveCompression::Xz) => {
            let mut out = liblzma::write::XzEncoder::new(Vec::new(), 1);
            out.write_all(plain).unwrap();
            out.finish().unwrap()
        }
        Some(ArchiveCompression::Bzip2) => {
            let mut out = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
            out.write_all(plain).unwrap();
            out.finish().unwrap()
        }
    }
}

async fn list_archive(
    desk: &Desk,
    by: &Who,
    document: Id,
    file: Id,
    inside: Option<&str>,
    limit: Option<u32>,
) -> Reply<orch_api_contract::workspace::ArchiveListing> {
    desk.world
        .app
        .call(DocumentFileArchiveList, &[&document, &file])
        .token(&by.token)
        .query(&DocumentFileArchiveListQuery {
            inside: inside.map(str::to_owned),
            limit,
            ..Default::default()
        })
        .send()
        .await
}

async fn entry_of(desk: &Desk, by: &Who, document: Id, file: Id, entry: &str) -> RawReply {
    desk.world
        .app
        .call(DocumentFileArchiveEntryGet, &[&document, &file])
        .token(&by.token)
        .query(&DocumentFileArchiveEntryGetQuery {
            entry: entry.to_owned(),
            inspect_deleted: None,
        })
        .send_bytes()
        .await
}

#[tokio::test]
async fn what_an_archive_holds_is_listed_one_directory_at_a_time_and_one_entry_is_read_without_unpacking_the_rest()
 {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let id = made.base.id;
    let tar = tar_bytes();
    let cases: Vec<(&str, Vec<u8>, ArchiveFormat, Option<ArchiveCompression>)> = vec![
        ("a.zip", zip_bytes(false), ArchiveFormat::Zip, None),
        ("a.tar", compressed(None, &tar), ArchiveFormat::Tar, None),
        (
            "a.tar.gz",
            compressed(Some(ArchiveCompression::Gzip), &tar),
            ArchiveFormat::Tar,
            Some(ArchiveCompression::Gzip),
        ),
        (
            "a.tar.zst",
            compressed(Some(ArchiveCompression::Zstd), &tar),
            ArchiveFormat::Tar,
            Some(ArchiveCompression::Zstd),
        ),
        (
            "a.tar.xz",
            compressed(Some(ArchiveCompression::Xz), &tar),
            ArchiveFormat::Tar,
            Some(ArchiveCompression::Xz),
        ),
        (
            "a.tar.bz2",
            compressed(Some(ArchiveCompression::Bzip2), &tar),
            ArchiveFormat::Tar,
            Some(ArchiveCompression::Bzip2),
        ),
    ];
    for (name, bytes, format, compression) in cases {
        let kept = put_large(&desk, id, name, &bytes).await;
        let top = list_archive(&desk, &desk.ada, id, kept.id, None, None)
            .await
            .ok();
        assert_eq!(top.format, format, "{name}");
        assert_eq!(top.compression, compression, "{name}");
        assert!(!top.encrypted && !top.truncated, "{name}");
        let shown: Vec<(&str, ArchiveEntryKind)> = top
            .entries
            .iter()
            .map(|entry| (entry.path.as_str(), entry.kind))
            .collect();
        assert_eq!(
            shown.first().map(|first| first.1),
            Some(ArchiveEntryKind::Dir),
            "{name}: directories first"
        );
        assert!(
            shown.contains(&("docs", ArchiveEntryKind::Dir)),
            "{name}: {shown:?}"
        );
        assert!(
            shown.contains(&("top.txt", ArchiveEntryKind::File)),
            "{name}: {shown:?}"
        );
        let docs = list_archive(&desk, &desk.ada, id, kept.id, Some("docs"), None)
            .await
            .ok();
        let shown: Vec<&str> = docs
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(shown, ["docs/deep", "docs/readme.md"], "{name}");
        let one = list_archive(&desk, &desk.ada, id, kept.id, None, Some(1))
            .await
            .ok();
        assert_eq!(one.entries.len(), 1, "{name}");
        assert!(
            one.truncated,
            "{name}: there is more than the one asked for"
        );

        let entry = entry_of(&desk, &desk.ada, id, kept.id, "docs/deep/er/file.txt").await;
        assert_eq!(entry.status, StatusCode::OK, "{name}");
        assert_eq!(entry.raw.as_ref(), b"deep", "{name}");
        let readme = entry_of(&desk, &desk.ada, id, kept.id, "docs/readme.md").await;
        assert_eq!(readme.raw.len(), 900, "{name}");
        assert_eq!(header(&readme, "content-type"), "text/markdown", "{name}");
        // Nothing is run or followed: a link is a link, a directory is no file, a name that is not there is not there.
        let missing = entry_of(&desk, &desk.ada, id, kept.id, "nope.txt").await;
        assert_eq!(missing.status, StatusCode::NOT_FOUND, "{name}");
        let dir = entry_of(&desk, &desk.ada, id, kept.id, "docs").await;
        assert_eq!(
            dir.status,
            StatusCode::CONFLICT,
            "{name}: a directory is no file"
        );
        let climbing = entry_of(&desk, &desk.ada, id, kept.id, "docs/../top.txt").await;
        assert_eq!(climbing.status, StatusCode::BAD_REQUEST, "{name}");
    }
}

/// Puts a file in whatever its size, in pieces when a request cannot carry it.
async fn put_large(desk: &Desk, document: Id, name: &str, bytes: &[u8]) -> DocumentFile {
    let limit = orch_api_contract::common::MAX_BODY_BYTES;
    if bytes.len() <= limit {
        return put(desk, &desk.ada, document, name, bytes, false)
            .await
            .ok();
    }
    let blob = upload_in_pieces_large(desk, &desk.ada, name, bytes).await;
    desk.world
        .app
        .call(DocumentFileCreate, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentFileCreateQuery::default())
        .raw_body(&json!({ "blob": blob, "name": name }))
        .send()
        .await
        .ok()
}

async fn upload_in_pieces_large(desk: &Desk, by: &Who, name: &str, bytes: &[u8]) -> Hash {
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
    for chunk in bytes.chunks(orch_api_contract::common::UPLOAD_CHUNK_BYTES) {
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

#[tokio::test]
async fn an_entry_larger_than_a_preview_and_an_archive_that_asks_for_a_password_are_not_opened() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let id = made.base.id;
    let plain = put_large(&desk, id, "plain.zip", &zip_bytes(false)).await;
    let big = entry_of(&desk, &desk.ada, id, plain.id, "big.bin").await;
    assert_eq!(big.status, StatusCode::PAYLOAD_TOO_LARGE);
    let problem = big.problem();
    assert_eq!(problem.code, ErrorCode::RequestTooLarge);
    assert_eq!(problem.extra["limit"], 2_097_152);

    let locked = put_large(&desk, id, "locked.zip", &zip_bytes(true)).await;
    let listing = list_archive(&desk, &desk.ada, id, locked.id, None, None)
        .await
        .ok();
    assert!(
        listing.encrypted,
        "names are listed where the format lets them be read"
    );
    assert!(!listing.entries.is_empty());
    let refused = entry_of(&desk, &desk.ada, id, locked.id, "top.txt").await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert_eq!(refused.problem().code, ErrorCode::CapabilityUnsupported);
}

#[tokio::test]
async fn a_file_that_is_no_archive_is_not_looked_into_and_looking_asks_what_reading_the_file_asks()
{
    let desk = Desk::new().await;
    let sam = desk.stranger("sam").await;
    let vera = desk.member("vera", "viewer").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let id = made.base.id;
    let text = put(&desk, &desk.ada, id, "words.txt", b"just words", false)
        .await
        .ok();
    let refused = list_archive(&desk, &desk.ada, id, text.id, None, None)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::CapabilityUnsupported);
    assert_eq!(refused.status, 409);
    let zero = list_archive(&desk, &desk.ada, id, text.id, None, Some(0))
        .await
        .problem();
    assert_eq!(zero.code, ErrorCode::RequestInvalid);

    let archive = put(&desk, &desk.ada, id, "a.tar", &tar_bytes(), false)
        .await
        .ok();
    list_archive(&desk, &vera, id, archive.id, None, None)
        .await
        .ok();
    assert_eq!(
        entry_of(&desk, &vera, id, archive.id, "top.txt")
            .await
            .raw
            .as_ref(),
        b"top level"
    );
    // Whoever may not read the document looks into nothing of it, and a file of another document is not found.
    let denied = list_archive(&desk, &sam, id, archive.id, None, None)
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    assert_eq!(places::missing_permissions(&denied), ["doc.view"]);
    assert_eq!(
        entry_of(&desk, &sam, id, archive.id, "top.txt")
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let other = desk.make(&desk.ada, "Other").await;
    assert_eq!(
        list_archive(&desk, &desk.ada, other.base.id, archive.id, None, None)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}
