//! Files sent from a browser: an image whole, a larger file in pieces that go on where they stopped and end as one blob, and a file attached to a thread so that it is kept until the input that carries it is delivered; each needing a person who is signed in, and the pieces only their beginner.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;
mod workspace;

use chrono::Duration;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::UploadSessionDelete;
use orch_api_contract::threads::{
    ContentPart, Delivery, FILENAME_HEADER, IMAGE_UPLOAD_MAX_BYTES, ImageMediaType,
    ThreadAttachmentBlobBody, ThreadAttachmentCreate, ThreadAttachmentCreateReply, ThreadBlobGet,
    ThreadBlobGetQuery, ThreadInputCreate, ThreadInputCreateBody, UploadBlobGet,
    UploadBlobGetQuery, UploadCreate, UploadSessionComplete, UploadSessionCreate,
    UploadSessionCreateBody, UploadSessionPut, UploadSessionPutQuery, UploadedBlob,
};

#[tokio::test]
async fn a_private_preview_is_bounded_and_does_not_grant_another_person_the_original() {
    let site = Site::new().await;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(800, 600)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let stored = site
        .app()
        .call(UploadCreate, &[])
        .token(&site.ada.token)
        .header("content-type", "image/png")
        .header(FILENAME_HEADER, "photo.png")
        .bytes(png.get_ref())
        .send()
        .await
        .ok();
    let query = UploadBlobGetQuery {
        name: stored.name.clone(),
        preview: Some(true),
        ..Default::default()
    };
    let read = site
        .app()
        .call(UploadBlobGet, &[&stored.blob])
        .token(&site.ada.token)
        .query(&query)
        .send_bytes()
        .await;
    assert_eq!(read.status, http::StatusCode::OK);
    assert_eq!(read.headers["content-type"], "application/json");
    assert_eq!(read.headers["x-content-type-options"], "nosniff");
    assert_eq!(read.headers["cache-control"], "private, no-store");
    let preview: serde_json::Value = serde_json::from_slice(&read.raw).unwrap();
    assert_eq!(preview["state"], "ready");
    assert_eq!(preview["size"], stored.size);
    assert_eq!(preview["width"], 800);
    assert_eq!(preview["height"], 600);
    use base64::Engine as _;
    let encoded = preview["image"]
        .as_str()
        .unwrap()
        .strip_prefix("data:image/png;base64,")
        .unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    let thumbnail = image::load_from_memory(&bytes).unwrap();
    assert!(thumbnail.width() <= 512 && thumbnail.height() <= 512);
    assert!(bytes.len() <= 262_144);
    let refused = site
        .app()
        .call(UploadBlobGet, &[&stored.blob])
        .token(&site.eve.token)
        .query(&query)
        .send_bytes()
        .await;
    assert!(!refused.status.is_success());
    assert!(!String::from_utf8_lossy(&refused.raw).contains("photo.png"));
    let again = site
        .app()
        .call(UploadBlobGet, &[&stored.blob])
        .token(&site.ada.token)
        .query(&query)
        .send_bytes()
        .await;
    assert_eq!(again.raw, read.raw, "one content keeps one derived preview");
    assert!(
        site.world
            .app
            .app
            .blobs
            .store()
            .delete(&stored.blob)
            .await
            .unwrap()
    );
    let gone = site
        .app()
        .call(UploadBlobGet, &[&stored.blob])
        .token(&site.ada.token)
        .query(&query)
        .send_bytes()
        .await;
    assert_eq!(
        gone.status,
        http::StatusCode::NOT_FOUND,
        "a removed original never leaves a readable cached preview"
    );
}

#[tokio::test]
async fn preview_settings_apply_before_pixels_are_decoded_and_a_changed_limit_gets_a_new_preview() {
    let site = Site::new().await;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(100, 100)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let stored = site
        .app()
        .call(UploadCreate, &[])
        .token(&site.ada.token)
        .header("content-type", "image/png")
        .header(FILENAME_HEADER, "photo.png")
        .bytes(png.get_ref())
        .send()
        .await
        .ok();
    site.world.set("files.preview_pixels", json!(9999)).await;
    let query = UploadBlobGetQuery {
        name: stored.name,
        preview: Some(true),
        ..Default::default()
    };
    let read = site
        .app()
        .call(UploadBlobGet, &[&stored.blob])
        .token(&site.ada.token)
        .query(&query)
        .send_bytes()
        .await;
    assert_eq!(read.status, http::StatusCode::OK);
    let preview: serde_json::Value = serde_json::from_slice(&read.raw).unwrap();
    assert_eq!(preview["state"], "too_large");
    assert!(preview["image"].is_null());
    site.world.set("files.preview_pixels", json!(10000)).await;
    let again = site
        .app()
        .call(UploadBlobGet, &[&stored.blob])
        .token(&site.ada.token)
        .query(&query)
        .send_bytes()
        .await;
    let preview: serde_json::Value = serde_json::from_slice(&again.raw).unwrap();
    assert_eq!(preview["state"], "ready");
}
use orch_db::queries::storage;
use orch_types::{Hash, Id, new_id};
use places::Who;
use serde_json::json;
use workspace::Site;

/// Bytes that do not repeat inside a piece, so that a piece in the wrong place is noticed.
fn bytes_of(len: usize) -> Vec<u8> {
    (0..len)
        .map(|at| u8::try_from((at * 29 + at / 241) % 256).unwrap())
        .collect()
}

async fn begin(site: &Site, by: &Who, size: i64, name: &str) -> Id {
    site.app()
        .call(UploadSessionCreate, &[])
        .token(&by.token)
        .body(&UploadSessionCreateBody {
            size,
            name: name.to_owned(),
            media_type: None,
        })
        .send()
        .await
        .ok()
        .upload_id
}

async fn put(
    site: &Site,
    by: &Who,
    upload: Id,
    offset: i64,
    piece: &[u8],
) -> orch_api_core::testing::Reply<orch_api_contract::threads::UploadSessionPutReply> {
    site.app()
        .call(UploadSessionPut, &[&upload])
        .token(&by.token)
        .query(&UploadSessionPutQuery { offset })
        .bytes(piece)
        .send()
        .await
}

async fn end(site: &Site, by: &Who, upload: Id) -> orch_api_core::testing::Reply<UploadedBlob> {
    site.app()
        .call(UploadSessionComplete, &[&upload])
        .token(&by.token)
        .send()
        .await
}

#[tokio::test]
async fn cancelling_an_upload_discards_its_partial_bytes_and_never_writes_a_workspace_file() {
    let site = Site::new().await;
    let upload = begin(&site, &site.ada, 20, "cancelled.txt").await;
    put(&site, &site.ada, upload, 0, b"partial").await.ok();
    let path = site.app().app.blobs.part_of_session(upload);
    assert!(path.exists());
    assert_eq!(
        site.app()
            .call(UploadSessionDelete, &[&upload])
            .token(&site.ada.token)
            .send()
            .await
            .status,
        http::StatusCode::NO_CONTENT
    );
    assert!(!path.exists());
    assert_eq!(
        end(&site, &site.ada, upload).await.problem().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        put(&site, &site.ada, upload, 7, b"remaining")
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    assert!(site.disk.read("cancelled.txt").is_none());
}

#[tokio::test]
async fn another_person_cannot_cancel_an_upload() {
    let site = Site::new().await;
    let upload = begin(&site, &site.ada, 3, "mine.txt").await;
    let refused = site
        .app()
        .call(UploadSessionDelete, &[&upload])
        .token(&site.vic.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
    put(&site, &site.ada, upload, 0, b"all").await.ok();
    assert_eq!(end(&site, &site.ada, upload).await.ok().size, 3);
}

#[tokio::test]
async fn discarding_redundant_bytes_does_not_remove_another_unsaved_copy_with_the_same_hash() {
    use orch_api_contract::threads::{ThreadDraftSet, UploadBlobDelete, UploadBlobDeleteQuery};
    let site = Site::new().await;
    let first = attach_raw(&site, &site.ada, site.thread, "same.txt", b"same bytes")
        .await
        .ok();
    let second = attach_raw(&site, &site.ada, site.thread, "same.txt", b"same bytes")
        .await
        .ok();
    assert_eq!(first.blob, second.blob);
    assert_ne!(first.path, second.path);
    site.app()
        .call(UploadBlobDelete, &[&first.blob])
        .token(&site.ada.token)
        .query(&UploadBlobDeleteQuery::default())
        .send()
        .await
        .ok();
    let mut conn = site.world.conn().await;
    assert_eq!(
        storage::blob(&mut conn, &first.blob)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        2
    );
    drop(conn);
    site.app()
        .call(UploadBlobDelete, &[&second.blob])
        .token(&site.ada.token)
        .query(&UploadBlobDeleteQuery {
            thread: Some(site.thread),
            path: Some(second.path),
        })
        .send()
        .await
        .ok();
    let mut conn = site.world.conn().await;
    assert_eq!(
        storage::blob(&mut conn, &first.blob)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        1
    );
    drop(conn);
    site.app().call(ThreadDraftSet, &[&site.thread]).token(&site.ada.token)
        .raw_body(&json!({"content":[{"type":"file","blob":first.blob,"path":first.path,"name":first.name,"size":first.size}],"delivery":"queue"}))
        .send().await.ok();
}

// ---------------------------------------------------------------- an image, whole

#[tokio::test]
async fn an_image_uploaded_whole_is_a_blob_named_by_its_hash() {
    let site = Site::new().await;
    let image = bytes_of(3_000);
    let stored = site
        .app()
        .call(UploadCreate, &[])
        .token(&site.ada.token)
        .header("content-type", "image/png")
        .header(FILENAME_HEADER, "screenshot.png")
        .bytes(&image)
        .send()
        .await
        .ok();
    assert_eq!(stored.blob, Hash::of(&image));
    assert_eq!(stored.size, 3_000);
    assert_eq!(stored.media_type, "image/png");
    assert_eq!(stored.name, "screenshot.png");
    let held = site
        .app()
        .app
        .blobs
        .read_whole(&stored.blob, 10_000)
        .await
        .unwrap();
    assert_eq!(held.as_ref(), image.as_slice());
    let mut conn = site.world.conn().await;
    assert!(
        storage::blob(&mut conn, &stored.blob)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_name_that_is_not_plain_ascii_arrives_as_the_browser_escaped_it() {
    let site = Site::new().await;
    let stored = site
        .app()
        .call(UploadCreate, &[])
        .token(&site.ada.token)
        .header("content-type", "image/jpeg")
        .header(FILENAME_HEADER, "caf%C3%A9.jpg")
        .bytes(b"jpeg bytes")
        .send()
        .await
        .ok();
    assert_eq!(stored.name, "café.jpg");
}

#[tokio::test]
async fn only_an_image_is_taken_in_one_request() {
    let site = Site::new().await;
    for kind in [
        "text/plain",
        "application/pdf",
        "application/octet-stream",
        "image/svg+xml",
    ] {
        let refused = site
            .app()
            .call(UploadCreate, &[])
            .token(&site.ada.token)
            .header("content-type", kind)
            .bytes(b"x")
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid, "{kind}");
    }
}

#[tokio::test]
async fn an_image_larger_than_the_limit_is_refused() {
    let site = Site::new().await;
    let over = vec![0_u8; usize::try_from(IMAGE_UPLOAD_MAX_BYTES).unwrap() + 1];
    let refused = site
        .app()
        .call(UploadCreate, &[])
        .token(&site.ada.token)
        .header("content-type", "image/png")
        .bytes(&over)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestTooLarge);
}

#[tokio::test]
async fn uploading_needs_a_person_who_is_signed_in() {
    let site = Site::new().await;
    let unsigned = site
        .app()
        .call(UploadCreate, &[])
        .header("content-type", "image/png")
        .bytes(b"x")
        .send()
        .await
        .problem();
    assert_eq!(unsigned.status, 401);
    let begun = site
        .app()
        .call(UploadSessionCreate, &[])
        .body(&UploadSessionCreateBody {
            size: 10,
            name: "a".to_owned(),
            media_type: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(begun.status, 401);
}

// ---------------------------------------------------------------- a file, in pieces

#[tokio::test]
async fn a_file_goes_up_in_pieces_and_ends_as_one_blob_with_its_name_and_type() {
    let site = Site::new().await;
    let bytes = bytes_of(10_000);
    let upload = begin(&site, &site.ada, 10_000, "report.pdf").await;
    for (offset, end_at) in [(0_usize, 4_000_usize), (4_000, 8_000), (8_000, 10_000)] {
        let sent = put(
            &site,
            &site.ada,
            upload,
            i64::try_from(offset).unwrap(),
            &bytes[offset..end_at],
        )
        .await
        .ok();
        assert_eq!(sent.received, i64::try_from(end_at).unwrap());
    }
    let done = end(&site, &site.ada, upload).await.ok();
    assert_eq!(done.blob, Hash::of(&bytes));
    assert_eq!(done.size, 10_000);
    assert_eq!(done.name, "report.pdf");
    assert_eq!(done.media_type, "application/pdf");
    let held = site
        .app()
        .app
        .blobs
        .read_whole(&done.blob, 20_000)
        .await
        .unwrap();
    assert_eq!(held.as_ref(), bytes.as_slice(), "the pieces are in order");

    // The upload is over: its pieces are gone, and it cannot be ended again.
    let part = site.app().app.blobs.part_of_session(upload);
    assert!(!tokio::fs::try_exists(&part).await.unwrap());
    assert_eq!(
        end(&site, &site.ada, upload).await.problem().code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn the_size_of_a_piece_is_the_one_the_contract_states() {
    let site = Site::new().await;
    let begun = site
        .app()
        .call(UploadSessionCreate, &[])
        .token(&site.ada.token)
        .body(&UploadSessionCreateBody {
            size: 5,
            name: "a.txt".to_owned(),
            media_type: Some("text/plain".to_owned()),
        })
        .send()
        .await
        .ok();
    assert_eq!(
        begun.chunk_bytes,
        i64::try_from(orch_api_contract::common::UPLOAD_CHUNK_BYTES).unwrap()
    );
    assert!(begun.expires_at.get() > site.world.now() + Duration::hours(23));
    assert!(begun.expires_at.get() <= site.world.now() + Duration::hours(24));
}

#[tokio::test]
async fn an_upload_that_was_cut_goes_on_from_the_bytes_that_arrived() {
    let site = Site::new().await;
    let bytes = bytes_of(9_000);
    let upload = begin(&site, &site.ada, 9_000, "data.bin").await;
    put(&site, &site.ada, upload, 0, &bytes[..4_000]).await.ok();

    // The answer was lost; the browser sends the same piece again.
    let again = put(&site, &site.ada, upload, 0, &bytes[..4_000])
        .await
        .problem();
    assert_eq!(again.code, ErrorCode::UploadOffset);
    assert_eq!(
        again.extra["received"],
        json!(4_000),
        "it is told where to go on"
    );
    let ahead = put(&site, &site.ada, upload, 6_000, &bytes[6_000..])
        .await
        .problem();
    assert_eq!(ahead.code, ErrorCode::UploadOffset);
    assert_eq!(ahead.extra["received"], json!(4_000));

    put(&site, &site.ada, upload, 4_000, &bytes[4_000..])
        .await
        .ok();
    let done = end(&site, &site.ada, upload).await.ok();
    assert_eq!(
        done.blob,
        Hash::of(&bytes),
        "what arrived after the cut is whole"
    );
}

#[tokio::test]
async fn an_upload_is_not_ended_before_all_of_it_has_arrived() {
    let site = Site::new().await;
    let bytes = bytes_of(6_000);
    let upload = begin(&site, &site.ada, 6_000, "data.bin").await;
    put(&site, &site.ada, upload, 0, &bytes[..2_000]).await.ok();
    let early = end(&site, &site.ada, upload).await.problem();
    assert_eq!(early.code, ErrorCode::UploadOffset);
    assert_eq!(early.extra["received"], json!(2_000));
}

#[tokio::test]
async fn a_piece_may_not_reach_past_the_size_the_upload_was_begun_with() {
    let site = Site::new().await;
    let upload = begin(&site, &site.ada, 1_000, "data.bin").await;
    let past = put(&site, &site.ada, upload, 0, &bytes_of(1_001))
        .await
        .problem();
    assert_eq!(past.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_piece_carries_at_most_the_size_of_a_piece() {
    let site = Site::new().await;
    let size = i64::try_from(orch_api_contract::common::UPLOAD_CHUNK_BYTES).unwrap() * 2;
    let upload = begin(&site, &site.ada, size, "big.bin").await;
    let over = vec![1_u8; orch_api_contract::common::UPLOAD_CHUNK_BYTES + 1];
    let refused = put(&site, &site.ada, upload, 0, &over).await.problem();
    assert_eq!(refused.code, ErrorCode::RequestTooLarge);
}

#[tokio::test]
async fn an_empty_file_is_a_file() {
    let site = Site::new().await;
    let upload = begin(&site, &site.ada, 0, "empty.txt").await;
    let done = end(&site, &site.ada, upload).await.ok();
    assert_eq!((done.size, done.blob), (0, Hash::of(b"")));
}

#[tokio::test]
async fn an_upload_is_its_beginners_alone() {
    let site = Site::new().await;
    let upload = begin(&site, &site.ada, 100, "mine.bin").await;
    let bytes = bytes_of(100);
    // Another person of the same org, and one who is no member of anything: neither can tell it is there.
    for other in [&site.dev, &site.eve] {
        let piece = put(&site, other, upload, 0, &bytes).await.problem();
        assert_eq!(piece.code, ErrorCode::NotFound);
        let ended = end(&site, other, upload).await.problem();
        assert_eq!(ended.code, ErrorCode::NotFound);
    }
    // Nothing of theirs got in.
    assert_eq!(
        put(&site, &site.ada, upload, 0, &bytes).await.ok().received,
        100
    );
}

#[tokio::test]
async fn an_upload_that_ran_out_is_gone() {
    let site = Site::new().await;
    let upload = begin(&site, &site.ada, 100, "late.bin").await;
    site.world.advance(Duration::hours(25));
    let late = put(&site, &site.ada, upload, 0, &bytes_of(100))
        .await
        .problem();
    assert_eq!(late.code, ErrorCode::Gone);
    assert_eq!(
        end(&site, &site.ada, upload).await.problem().code,
        ErrorCode::Gone
    );
}

#[tokio::test]
async fn an_upload_needs_a_name_a_size_and_a_type_that_make_sense() {
    let site = Site::new().await;
    let begin_with = |size: i64, name: &str, media_type: Option<&str>| {
        let call = site
            .app()
            .call(UploadSessionCreate, &[])
            .token(&site.ada.token)
            .body(&UploadSessionCreateBody {
                size,
                name: name.to_owned(),
                media_type: media_type.map(str::to_owned),
            });
        async move { call.send().await.problem().code }
    };
    assert_eq!(begin_with(10, "  ", None).await, ErrorCode::RequestInvalid);
    assert_eq!(
        begin_with(10, "a\nb", None).await,
        ErrorCode::RequestInvalid
    );
    assert_eq!(begin_with(-1, "a", None).await, ErrorCode::RequestInvalid);
    assert_eq!(
        begin_with(10, "a", Some("pdf")).await,
        ErrorCode::RequestInvalid
    );
    assert_eq!(
        begin_with(10, "a", Some("application/p df")).await,
        ErrorCode::RequestInvalid
    );
    let long = "n".repeat(256);
    assert_eq!(begin_with(10, &long, None).await, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_file_larger_than_the_system_stores_is_refused_when_the_upload_begins() {
    let site = Site::new().await;
    site.world.set("storage.max_blob_bytes", json!(1_000)).await;
    let refused = site
        .app()
        .call(UploadSessionCreate, &[])
        .token(&site.ada.token)
        .body(&UploadSessionCreateBody {
            size: 1_001,
            name: "big.bin".to_owned(),
            media_type: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestTooLarge);
    assert_eq!(refused.extra["limit"], json!(1_000));
}

// ---------------------------------------------------------------- attached to a thread

async fn attach_raw(
    site: &Site,
    by: &Who,
    thread: Id,
    name: &str,
    bytes: &[u8],
) -> orch_api_core::testing::Reply<ThreadAttachmentCreateReply> {
    site.app()
        .call(ThreadAttachmentCreate, &[&thread])
        .token(&by.token)
        .header(FILENAME_HEADER, name)
        .bytes(bytes)
        .send()
        .await
}

#[tokio::test]
async fn a_file_attached_to_a_thread_is_kept_and_told_the_path_it_will_have() {
    let site = Site::new().await;
    let bytes = b"notes for the agent\n".to_vec();
    let attached = attach_raw(&site, &site.ada, site.thread, "notes.txt", &bytes)
        .await
        .ok();
    assert_eq!(attached.name, "notes.txt");
    assert_eq!(attached.size, 20);
    assert_eq!(attached.blob, Hash::of(&bytes));
    assert!(
        attached.path.starts_with(".orch/attachments/"),
        "{}",
        attached.path
    );
    assert_eq!(attached.path, ".orch/attachments/notes.txt");

    let mut conn = site.world.conn().await;
    let row = storage::blob(&mut conn, &attached.blob)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.pin_count, 1,
        "it is kept for as long as the input that carries it"
    );
    assert!(
        !site.disk.exists(&attached.path),
        "nothing is written into the workspace until the input is delivered"
    );
}

#[tokio::test]
async fn two_files_of_one_name_get_two_paths() {
    let site = Site::new().await;
    let first = attach_raw(&site, &site.ada, site.thread, "a.txt", b"one")
        .await
        .ok();
    let second = attach_raw(&site, &site.ada, site.thread, "a.txt", b"two")
        .await
        .ok();
    assert_ne!(first.path, second.path);
}

#[tokio::test]
async fn a_private_pasted_draft_can_be_read_and_edited_without_writing_the_workspace_and_removal_releases_it()
 {
    use orch_api_contract::threads::{ThreadDraftSet, UploadBlobDelete, UploadBlobDeleteQuery};
    let site = Site::new().await;
    let original = b"private pasted words";
    let attached = attach_raw(
        &site,
        &site.ada,
        site.thread,
        "pasted-private.txt",
        original,
    )
    .await
    .ok();
    let part = json!({"type":"pasted_text","blob":attached.blob,"path":attached.path,"name":attached.name,"size":original.len(),"characters":original.len(),"ascii_characters":original.len()});
    site.app()
        .call(ThreadDraftSet, &[&site.thread])
        .token(&site.ada.token)
        .raw_body(&json!({"content":[part],"delivery":"queue"}))
        .send()
        .await
        .ok();
    let private = blob_of_thread(&site, &site.ada, &attached.blob, false).await;
    assert_eq!(private.status, http::StatusCode::OK);
    assert_eq!(private.raw.as_ref(), original);
    assert_eq!(
        blob_of_thread(&site, &site.vic, &attached.blob, false)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let mut conn = site.world.conn().await;
    assert_eq!(
        storage::blob(&mut conn, &attached.blob)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        1
    );
    let directory: String =
        sqlx::query_scalar("SELECT workspace_dir FROM threads WHERE node_id=$1")
            .bind(site.thread)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    drop(conn);
    assert!(!site.disk.exists(&format!("{directory}/{}", attached.path)));

    let upload = begin(&site, &site.ada, 6, "pasted-private.txt").await;
    put(&site, &site.ada, upload, 0, b"edited").await.ok();
    let edited = end(&site, &site.ada, upload).await.ok();
    let replaced = site
        .app()
        .call(ThreadAttachmentCreate, &[&site.thread])
        .token(&site.ada.token)
        .raw_body(&json!({"blob":edited.blob,"name":"pasted-private.txt","replace":attached.path}))
        .send()
        .await
        .ok();
    assert_eq!(
        replaced.path, attached.path,
        "editing a private paste keeps its destination"
    );
    assert!(!site.disk.exists(&format!("{directory}/{}", replaced.path)));
    let mut conn = site.world.conn().await;
    assert_eq!(
        storage::blob(&mut conn, &edited.blob)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        1,
        "the edited upload is held before the draft save acknowledges its new bytes"
    );
    drop(conn);
    site.app()
        .call(ThreadDraftSet, &[&site.thread])
        .token(&site.ada.token)
        .raw_body(&json!({"content":[part],"delivery":"queue"}))
        .send()
        .await
        .ok();
    let mut conn = site.world.conn().await;
    assert_eq!(
        storage::blob(&mut conn, &edited.blob)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        1,
        "an older device's draft cannot adopt a different pending blob just because its path matches"
    );
    drop(conn);
    site.app()
        .call(ThreadDraftSet, &[&site.thread])
        .token(&site.ada.token)
        .raw_body(&json!({"content":[{"type":"pasted_text","blob":edited.blob,"path":replaced.path,"name":replaced.name,"size":6,"characters":6,"ascii_characters":6}],"delivery":"queue"}))
        .send().await.ok();
    let mut conn = site.world.conn().await;
    assert_eq!(
        storage::blob(&mut conn, &attached.blob)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        0
    );
    assert_eq!(
        storage::blob(&mut conn, &edited.blob)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        1,
        "the saved draft takes the pin without keeping a second reservation pin"
    );
    drop(conn);
    site.app()
        .call(ThreadDraftSet, &[&site.thread])
        .token(&site.ada.token)
        .raw_body(&json!({"content":[],"delivery":"queue"}))
        .send()
        .await
        .ok();
    assert_eq!(
        site.app()
            .call(UploadBlobDelete, &[&edited.blob])
            .token(&site.ada.token)
            .query(&UploadBlobDeleteQuery {
                thread: Some(site.thread),
                path: Some(replaced.path)
            })
            .send()
            .await
            .status,
        http::StatusCode::NO_CONTENT
    );
    let mut conn = site.world.conn().await;
    assert_eq!(
        storage::blob(&mut conn, &attached.blob)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        0
    );
    assert_eq!(
        storage::blob(&mut conn, &edited.blob)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        0
    );
}

#[tokio::test]
async fn duplicate_parts_have_one_draft_pin_and_deleting_the_draft_releases_it() {
    use orch_api_contract::threads::{ThreadDraftDelete, ThreadDraftSet};
    let site = Site::new().await;
    let attached = attach_raw(&site, &site.ada, site.thread, "kept.txt", b"kept")
        .await
        .ok();
    let part = json!({"type":"file","blob":attached.blob,"path":attached.path,"name":attached.name,"size":4});
    site.app()
        .call(ThreadDraftSet, &[&site.thread])
        .token(&site.ada.token)
        .raw_body(&json!({"content":[part,part],"delivery":"queue"}))
        .send()
        .await
        .ok();
    let mut conn = site.world.conn().await;
    assert_eq!(
        storage::blob(&mut conn, &attached.blob)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        1
    );
    drop(conn);
    site.app()
        .call(ThreadDraftDelete, &[&site.thread])
        .token(&site.ada.token)
        .send()
        .await
        .ok();
    let mut conn = site.world.conn().await;
    assert_eq!(
        storage::blob(&mut conn, &attached.blob)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        0
    );
    let references: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM thread_draft_blob_refs WHERE user_id=$1 AND thread_id=$2",
    )
    .bind(site.ada.id())
    .bind(site.thread)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(references, 0);
}

#[tokio::test]
async fn a_forged_legacy_thread_draft_is_not_authority_to_read_or_retain_somebody_elses_bytes() {
    use orch_api_contract::threads::ThreadDraftSet;
    let site = Site::new().await;
    let attached = attach_raw(&site, &site.ada, site.thread, "private.txt", b"private")
        .await
        .ok();
    let parts = json!([{"type":"file","name":attached.name,"path":attached.path,"size":attached.size,"blob":attached.blob}]);
    let mut conn = site.world.conn().await;
    sqlx::query("INSERT INTO drafts(user_id,thread_id,content,delivery,updated_at) VALUES($1,$2,$3,'queue',$4)")
        .bind(site.dev.id()).bind(site.thread).bind(&parts).bind(site.world.now()).execute(&mut *conn).await.unwrap();
    drop(conn);
    assert_eq!(
        blob_of_thread(&site, &site.dev, &attached.blob, false)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        site.app()
            .call(ThreadDraftSet, &[&site.thread])
            .token(&site.dev.token)
            .raw_body(&json!({"content":parts,"delivery":"queue"}))
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        site.app()
            .call(ThreadAttachmentCreate, &[&site.thread])
            .token(&site.dev.token)
            .raw_body(&json!({"blob":attached.blob,"name":"copy.txt"}))
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn a_name_that_climbs_out_of_the_directory_is_made_one_name() {
    let site = Site::new().await;
    let attached = attach_raw(&site, &site.ada, site.thread, "../../etc/passwd", b"x")
        .await
        .ok();
    assert_eq!(attached.path.split('/').count(), 3, "{}", attached.path);
    assert!(attached.path.starts_with(".orch/attachments/"));
    assert!(
        !attached.name.contains('/'),
        "the stored name cannot contain a path"
    );
}

#[tokio::test]
async fn a_file_that_was_uploaded_in_pieces_is_attached_by_its_hash() {
    let site = Site::new().await;
    let bytes = bytes_of(7_000);
    let upload = begin(&site, &site.ada, 7_000, "data.csv").await;
    put(&site, &site.ada, upload, 0, &bytes).await.ok();
    let done = end(&site, &site.ada, upload).await.ok();

    let attached = site
        .app()
        .call(ThreadAttachmentCreate, &[&site.thread])
        .token(&site.ada.token)
        .raw_body(
            &serde_json::to_value(ThreadAttachmentBlobBody {
                blob: done.blob,
                name: "data.csv".to_owned(),
                replace: None,
            })
            .unwrap(),
        )
        .send()
        .await
        .ok();
    assert_eq!((attached.blob, attached.size), (done.blob, 7_000));
    let mut conn = site.world.conn().await;
    let row = storage::blob(&mut conn, &done.blob).await.unwrap().unwrap();
    assert_eq!(row.pin_count, 1);
}

#[tokio::test]
async fn an_upload_the_store_does_not_hold_cannot_be_attached() {
    let site = Site::new().await;
    let refused = site
        .app()
        .call(ThreadAttachmentCreate, &[&site.thread])
        .token(&site.ada.token)
        .raw_body(&json!({ "blob": Hash::of(b"never uploaded"), "name": "x.txt" }))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
    let unreadable = site
        .app()
        .call(ThreadAttachmentCreate, &[&site.thread])
        .token(&site.ada.token)
        .raw_body(&json!({ "nonsense": true }))
        .send()
        .await
        .problem();
    assert_eq!(unreadable.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn attaching_needs_the_right_to_interact_with_the_thread() {
    let site = Site::new().await;
    // A viewer reads the thread and does not drive it.
    let viewer = attach_raw(&site, &site.vic, site.thread, "a.txt", b"x")
        .await
        .problem();
    assert_eq!(viewer.code, ErrorCode::AuthzDenied);
    // Somebody who belongs to nothing there is not told it exists.
    let stranger = attach_raw(&site, &site.eve, site.thread, "a.txt", b"x").await;
    assert!(!stranger.status.is_success(), "{:?}", stranger.status);
    // A developer of the org drives its threads.
    assert!(
        attach_raw(&site, &site.dev, site.thread, "a.txt", b"x")
            .await
            .status
            .is_success()
    );
    // A thread that does not exist is not one.
    let nowhere = attach_raw(&site, &site.ada, new_id(), "a.txt", b"x")
        .await
        .problem();
    assert_eq!(nowhere.code, ErrorCode::NotFound);
}

// ---------------------------------------------------------------- what a thread carries, served

async fn blob_of_thread(
    site: &Site,
    by: &Who,
    hash: &Hash,
    download: bool,
) -> orch_api_core::testing::RawReply {
    site.app()
        .call(ThreadBlobGet, &[&site.thread, hash])
        .token(&by.token)
        .query(&ThreadBlobGetQuery {
            download: download.then_some(true),
            ..Default::default()
        })
        .send_bytes()
        .await
}

#[tokio::test]
async fn an_image_sent_to_a_thread_is_served_to_whoever_may_read_it_and_nothing_else_the_store_holds()
 {
    let site = Site::new().await;
    let png = bytes_of(2_000);
    let uploaded = site
        .app()
        .call(UploadCreate, &[])
        .token(&site.ada.token)
        .header("content-type", "image/png")
        .header(FILENAME_HEADER, "shot.png")
        .bytes(&png)
        .send()
        .await
        .ok();
    // Uploaded, and in no input: the thread does not carry it.
    assert_eq!(
        blob_of_thread(&site, &site.ada, &uploaded.blob, false)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    // Sent to the thread, it is carried.
    let sent = site
        .app()
        .call(ThreadInputCreate, &[&site.thread])
        .token(&site.ada.token)
        .body(&ThreadInputCreateBody {
            plan: false,
            content: vec![ContentPart::Image {
                blob: uploaded.blob,
                media_type: ImageMediaType::Png,
                name: "shot.png".to_owned(),
                path: None,
            }],
            delivery: Delivery::Queue,
            client_id: "an-image".to_owned(),
            bypass: None,
            native_action: None,
        })
        .send()
        .await;
    assert!(sent.status.is_success(), "{:?}", sent.body);

    let viewed = blob_of_thread(&site, &site.vic, &uploaded.blob, false).await;
    assert_eq!(viewed.status, http::StatusCode::OK);
    assert_eq!(viewed.raw.as_ref(), png.as_slice());
    assert_eq!(viewed.headers["content-type"], "image/png");
    assert_eq!(viewed.headers["cache-control"], "private, no-cache");
    assert_eq!(viewed.headers["vary"], "Authorization, Cookie");
    assert!(
        viewed.headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("inline")
    );
    assert_eq!(viewed.headers["x-content-type-options"], "nosniff");
    let saved = blob_of_thread(&site, &site.vic, &uploaded.blob, true).await;
    assert!(
        saved.headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );

    // Somebody who may not read the thread is not served; content the thread does not carry is not
    // found, though the store holds it.
    assert!(
        !blob_of_thread(&site, &site.eve, &uploaded.blob, false)
            .await
            .status
            .is_success()
    );
    let other = site
        .app()
        .app
        .blobs
        .put_bytes(bytes::Bytes::from_static(b"somebody else's"))
        .await
        .unwrap();
    assert_eq!(
        blob_of_thread(&site, &site.ada, &other.hash, false)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn a_file_forwarded_into_a_thread_is_carried_by_it_too() {
    use orch_api_contract::threads::{Forwarded, ForwardedAttachment, ForwardedFrom};
    let site = Site::new().await;
    let notes = site
        .app()
        .app
        .blobs
        .put_bytes(bytes::Bytes::from_static(b"forwarded notes"))
        .await
        .unwrap();
    let forwarded = ContentPart::Forwarded(Box::new(Forwarded {
        r#type: orch_api_contract::threads::ForwardedType::Forwarded,
        from: serde_json::from_value::<ForwardedFrom>(json!({
            "kind": "thread",
            "thread": { "id": new_id(), "kind": "thread", "name": "Team", "redacted": false }
        }))
        .unwrap(),
        source_id: new_id(),
        author: None,
        at: orch_types::Time::new(chrono::Utc::now()),
        text: "see".to_owned(),
        images: Vec::new(),
        attachments: vec![ForwardedAttachment {
            blob: notes.hash,
            name: "notes.txt".to_owned(),
            size: notes.size,
            media_type: "text/plain".to_owned(),
        }],
    }));
    let mut conn = site.world.conn().await;
    let seq = orch_db::sequences::next(&mut conn, site.thread)
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "INSERT INTO thread_inputs (id, thread_id, author_id, source, content, delivery, state, position, seq_updated)
         VALUES ($1, $2, $3, 'user', $4, 'queue', 'pending', 1, $5)",
    )
    .bind(new_id())
    .bind(site.thread)
    .bind(site.ada.id())
    .bind(json!([forwarded]))
    .bind(seq)
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    let served = blob_of_thread(&site, &site.ada, &notes.hash, false).await;
    assert_eq!(served.raw.as_ref(), b"forwarded notes");
    assert!(
        served.headers["content-disposition"]
            .to_str()
            .unwrap()
            .contains("notes.txt")
    );
}
