//! What machines upload to and download from the store of content: pieces that go on where they stopped, a whole that is checked against its hash, and the rules that bind every request to a command that is open for the machine that asks.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;
mod workspace;

use http::StatusCode;
use orch_blob_contract::range::BlobMeta;
use orch_db::queries::storage;
use orch_link_contract::commands::CommandKind;
use orch_link_contract::content::MAX_CHUNK_BYTES;
use orch_pki::certificates::ServiceName;
use orch_types::{Hash, Id, new_id};
use serde_json::json;
use workspace::{Content, Depot, finish};

/// Bytes that do not repeat inside a piece, so that a piece in the wrong place is noticed.
fn bytes_of(len: usize) -> Vec<u8> {
    (0..len)
        .map(|at| u8::try_from((at * 31 + at / 251) % 256).unwrap())
        .collect()
}

#[tokio::test]
async fn a_file_uploaded_in_pieces_is_in_the_store_once_the_last_piece_has_arrived() {
    let depot = Depot::new().await;
    let command = depot.uploading().await;
    let bytes = bytes_of(10_000);
    let hash = Hash::of(&bytes);
    let size = i64::try_from(bytes.len()).unwrap();

    let first = depot
        .content
        .put(depot.machine, command, &hash, 0, size, &bytes[..4_096])
        .await;
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(
        (first.received(), first.complete()),
        (Some(4_096), Some(false))
    );
    assert!(
        depot
            .app()
            .blobs
            .store()
            .size(&hash)
            .await
            .unwrap()
            .is_none(),
        "half of a file is no file"
    );

    let second = depot
        .content
        .put(
            depot.machine,
            command,
            &hash,
            4_096,
            size,
            &bytes[4_096..8_192],
        )
        .await;
    assert_eq!(
        (second.received(), second.complete()),
        (Some(8_192), Some(false))
    );
    let last = depot
        .content
        .put(depot.machine, command, &hash, 8_192, size, &bytes[8_192..])
        .await;
    assert_eq!(last.status, StatusCode::OK);
    assert_eq!((last.received(), last.complete()), (Some(size), Some(true)));

    assert_eq!(
        depot.app().blobs.store().size(&hash).await.unwrap(),
        Some(size)
    );
    let mut conn = depot.world.conn().await;
    let row = storage::blob(&mut conn, &hash).await.unwrap().unwrap();
    assert_eq!(
        (row.size, row.pin_count),
        (size, 0),
        "it is recorded, and nothing holds it yet"
    );
    assert!(
        storage::upload(&mut conn, &hash).await.unwrap().is_none(),
        "the record of the pieces is gone with the upload"
    );
}

#[tokio::test]
async fn an_upload_that_was_cut_goes_on_where_it_stopped_and_what_arrives_is_whole() {
    let depot = Depot::new().await;
    let command = depot.uploading().await;
    let bytes = bytes_of(12_000);
    let hash = Hash::of(&bytes);
    let size = i64::try_from(bytes.len()).unwrap();

    depot
        .content
        .put(depot.machine, command, &hash, 0, size, &bytes[..5_000])
        .await;
    // The machine lost the answer. It asks where the upload stands, and goes on from there.
    let asked = depot.content.head(depot.machine, command, &hash).await;
    assert_eq!(asked.status, StatusCode::OK);
    assert_eq!(
        (asked.received(), asked.complete()),
        (Some(5_000), Some(false))
    );

    // A piece that begins anywhere else is told where to begin, and nothing is written.
    let again = depot
        .content
        .put(depot.machine, command, &hash, 0, size, &bytes[..5_000])
        .await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    assert_eq!(again.received(), Some(5_000));
    let ahead = depot
        .content
        .put(depot.machine, command, &hash, 9_000, size, &bytes[9_000..])
        .await;
    assert_eq!(ahead.status, StatusCode::CONFLICT);
    assert_eq!(ahead.received(), Some(5_000));

    let rest = depot
        .content
        .put(depot.machine, command, &hash, 5_000, size, &bytes[5_000..])
        .await;
    assert_eq!((rest.status, rest.complete()), (StatusCode::OK, Some(true)));

    let held = depot.app().blobs.read_whole(&hash, size).await.unwrap();
    assert_eq!(
        held.as_ref(),
        bytes.as_slice(),
        "the file is whole and in order"
    );
}

#[tokio::test]
async fn an_upload_may_only_begin_with_its_first_piece() {
    let depot = Depot::new().await;
    let command = depot.uploading().await;
    let bytes = bytes_of(3_000);
    let hash = Hash::of(&bytes);
    let answer = depot
        .content
        .put(depot.machine, command, &hash, 1_000, 3_000, &bytes[1_000..])
        .await;
    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(
        answer.received(),
        Some(0),
        "it is told to begin at the start"
    );
    let probe = depot.content.head(depot.machine, command, &hash).await;
    assert_eq!(probe.status, StatusCode::NOT_FOUND, "nothing was begun");
}

#[tokio::test]
async fn content_that_is_not_what_its_hash_says_is_thrown_away_and_refused() {
    let depot = Depot::new().await;
    let command = depot.uploading().await;
    let claimed = Hash::of(b"what the machine said it was sending");
    let sent = bytes_of(2_000);
    let answer = depot
        .content
        .put(depot.machine, command, &claimed, 0, 2_000, &sent)
        .await;
    assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(answer.json()["code"], "hash_mismatch");
    assert!(
        depot
            .app()
            .blobs
            .store()
            .size(&claimed)
            .await
            .unwrap()
            .is_none(),
        "nothing is stored under a hash it does not have"
    );
    let probe = depot.content.head(depot.machine, command, &claimed).await;
    assert_eq!(
        probe.status,
        StatusCode::NOT_FOUND,
        "the pieces are gone, so the machine begins again and does not go on from bytes that are wrong"
    );
}

#[tokio::test]
async fn content_the_store_holds_needs_nothing_more() {
    let depot = Depot::new().await;
    let command = depot.uploading().await;
    let bytes = bytes_of(1_500);
    let hash = depot.store(&bytes).await;
    let answer = depot
        .content
        .put(depot.machine, command, &hash, 0, 1_500, &bytes)
        .await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(
        (answer.received(), answer.complete()),
        (Some(1_500), Some(true))
    );
    let probe = depot.content.head(depot.machine, command, &hash).await;
    assert_eq!(
        (probe.received(), probe.complete()),
        (Some(1_500), Some(true))
    );
}

#[tokio::test]
async fn a_piece_carries_at_most_four_mebibytes() {
    let depot = Depot::new().await;
    let command = depot.uploading().await;
    let over = usize::try_from(MAX_CHUNK_BYTES).unwrap() + 1;
    let bytes = vec![9_u8; over];
    let hash = Hash::of(&bytes);
    let answer = depot
        .content
        .put(
            depot.machine,
            command,
            &hash,
            0,
            i64::try_from(over).unwrap(),
            &bytes,
        )
        .await;
    assert_eq!(answer.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn a_file_larger_than_the_system_stores_is_refused_before_any_of_it_is_taken() {
    let depot = Depot::new().await;
    depot
        .world
        .set("storage.max_blob_bytes", json!(1_000))
        .await;
    let command = depot.uploading().await;
    let bytes = bytes_of(2_000);
    let hash = Hash::of(&bytes);
    let answer = depot
        .content
        .put(depot.machine, command, &hash, 0, 2_000, &bytes[..1_000])
        .await;
    assert_eq!(answer.status, StatusCode::PAYLOAD_TOO_LARGE);
    let mut conn = depot.world.conn().await;
    assert!(storage::upload(&mut conn, &hash).await.unwrap().is_none());
}

#[tokio::test]
async fn a_piece_that_reaches_past_the_size_it_was_announced_with_is_refused() {
    let depot = Depot::new().await;
    let command = depot.uploading().await;
    let bytes = bytes_of(3_000);
    let hash = Hash::of(&bytes);
    let answer = depot
        .content
        .put(depot.machine, command, &hash, 0, 2_000, &bytes)
        .await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_machine_is_asked_which_of_some_content_the_system_lacks() {
    let depot = Depot::new().await;
    let command = depot.uploading().await;
    let held = depot.store(b"held").await;
    let lacking = Hash::of(b"lacking");
    let answer = depot
        .content
        .missing(depot.machine, command, &[held, lacking])
        .await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.json()["missing"], json!([lacking.to_string()]));

    let too_many: Vec<Hash> = (0..1_001_u32).map(|n| Hash::of(&n.to_le_bytes())).collect();
    let refused = depot
        .content
        .missing(depot.machine, command, &too_many)
        .await;
    assert_eq!(
        refused.status,
        StatusCode::BAD_REQUEST,
        "a thousand at once, and no more"
    );
}

#[tokio::test]
async fn content_the_store_holds_without_a_row_is_recorded_when_a_machine_asks_after_it() {
    let depot = Depot::new().await;
    let command = depot.uploading().await;
    let meta = depot
        .app()
        .blobs
        .put_bytes(bytes::Bytes::from_static(b"in the store, not in the table"))
        .await
        .unwrap();
    depot
        .content
        .missing(depot.machine, command, &[meta.hash])
        .await;
    let mut conn = depot.world.conn().await;
    let row = storage::blob(&mut conn, &meta.hash).await.unwrap();
    assert_eq!(
        row.map(|row| row.size),
        Some(meta.size),
        "the store is the truth about what is held, and the table follows it"
    );
}

// ---------------------------------------------------------------- whose request it is

#[tokio::test]
async fn only_a_command_that_is_open_for_the_machine_moves_content() {
    let depot = Depot::new().await;
    let other = Depot::add_machine(&depot.world, &["environment"]).await;
    let bytes = bytes_of(500);
    let hash = Hash::of(&bytes);
    let size = 500;

    // No command named.
    let request = depot.content.request(
        http::Method::PUT,
        &format!("/internal/link/blobs/{hash}?offset=0&size={size}"),
        Some(depot.machine),
        None,
        &[],
        bytes.clone().into(),
    );
    assert_eq!(
        depot.content.send(request).await.status,
        StatusCode::BAD_REQUEST
    );

    // A command nobody issued.
    let unknown = depot
        .content
        .put(depot.machine, new_id(), &hash, 0, size, &bytes)
        .await;
    assert_eq!(unknown.status, StatusCode::FORBIDDEN);

    // A command of another machine.
    let theirs = depot
        .command(
            other,
            CommandKind::FsFetch,
            &json!({ "environment_id": new_id(), "path": "a" }),
        )
        .await;
    let borrowed = depot
        .content
        .put(depot.machine, theirs, &hash, 0, size, &bytes)
        .await;
    assert_eq!(borrowed.status, StatusCode::FORBIDDEN);

    // A command that is over.
    let over = depot.uploading().await;
    finish(depot.app(), over).await;
    let late = depot
        .content
        .put(depot.machine, over, &hash, 0, size, &bytes)
        .await;
    assert_eq!(late.status, StatusCode::GONE);

    // A machine that is no machine of the system.
    let stranger = depot
        .content
        .put(new_id(), over, &hash, 0, size, &bytes)
        .await;
    assert_eq!(stranger.status, StatusCode::FORBIDDEN);

    // Nothing of it was kept.
    let mut conn = depot.world.conn().await;
    assert!(storage::upload(&mut conn, &hash).await.unwrap().is_none());
}

#[tokio::test]
async fn a_machine_that_was_revoked_is_let_nowhere_near_the_store() {
    let depot = Depot::new().await;
    let command = depot.uploading().await;
    sqlx::query("UPDATE machines SET status = 'revoked' WHERE id = $1")
        .bind(depot.machine)
        .execute(&depot.world.db.pool)
        .await
        .unwrap();
    let bytes = bytes_of(100);
    let answer = depot
        .content
        .put(depot.machine, command, &Hash::of(&bytes), 0, 100, &bytes)
        .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_gateway_alone_hands_over_the_requests_for_content() {
    let depot = Depot::new().await;
    let command = depot.uploading().await;
    let bytes = bytes_of(100);
    let hash = Hash::of(&bytes);
    for service in [None, Some(ServiceName::Api), Some(ServiceName::Worker)] {
        let content = Content::as_service(depot.app(), service);
        let answer = content
            .put(depot.machine, command, &hash, 0, 100, &bytes)
            .await;
        assert_eq!(answer.status, StatusCode::FORBIDDEN, "{service:?}");
    }
}

#[tokio::test]
async fn a_command_that_moves_no_content_takes_none() {
    let depot = Depot::new().await;
    let listing = depot
        .command(
            depot.machine,
            CommandKind::FsList,
            &json!({ "environment_id": new_id(), "path": "" }),
        )
        .await;
    let bytes = bytes_of(100);
    let hash = Hash::of(&bytes);
    let up = depot
        .content
        .put(depot.machine, listing, &hash, 0, 100, &bytes)
        .await;
    assert_eq!(up.status, StatusCode::FORBIDDEN);
    let stored = depot.store(&bytes).await;
    let down = depot
        .content
        .get(depot.machine, listing, &stored, None)
        .await;
    assert_eq!(down.status, StatusCode::FORBIDDEN);
    let asked = depot.content.head(depot.machine, listing, &stored).await;
    assert_eq!(asked.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_command_that_fetches_a_file_for_a_viewer_may_put_up_the_entry_it_read() {
    let depot = Depot::new().await;
    let reading = depot
        .command(
            depot.machine,
            CommandKind::FsArchiveRead,
            &json!({ "environment_id": new_id(), "path": "a.zip", "entry": "readme.txt" }),
        )
        .await;
    let bytes = bytes_of(300);
    let hash = depot
        .content
        .upload(depot.machine, reading, &bytes, 4_096)
        .await;
    assert_eq!(hash, Hash::of(&bytes));
}

// ---------------------------------------------------------------- downloads

#[tokio::test]
async fn a_machine_is_served_the_content_its_command_names_and_no_other() {
    let depot = Depot::new().await;
    let bytes = bytes_of(10_000);
    let named = depot.store(&bytes).await;
    let unnamed = depot.store(b"stored, but not for this command").await;
    let writing = depot
        .command(
            depot.machine,
            CommandKind::FsWrite,
            &json!({ "environment_id": new_id(), "path": "a.bin", "blob": named }),
        )
        .await;

    let whole = depot
        .content
        .get(depot.machine, writing, &named, None)
        .await;
    assert_eq!(whole.status, StatusCode::OK);
    assert_eq!(whole.body.as_ref(), bytes.as_slice());
    assert_eq!(whole.header("accept-ranges").as_deref(), Some("bytes"));

    let refused = depot
        .content
        .get(depot.machine, writing, &unnamed, None)
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_download_goes_on_from_the_byte_a_range_names() {
    let depot = Depot::new().await;
    let bytes = bytes_of(10_000);
    let named = depot.store(&bytes).await;
    let writing = depot
        .command(
            depot.machine,
            CommandKind::FsWrite,
            &json!({ "environment_id": new_id(), "path": "a.bin", "blob": named }),
        )
        .await;

    let part = depot
        .content
        .get(depot.machine, writing, &named, Some("bytes=4000-4999"))
        .await;
    assert_eq!(part.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.body.as_ref(), &bytes[4_000..5_000]);
    assert_eq!(
        part.header("content-range").as_deref(),
        Some("bytes 4000-4999/10000")
    );

    let tail = depot
        .content
        .get(depot.machine, writing, &named, Some("bytes=9000-"))
        .await;
    assert_eq!(tail.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(tail.body.as_ref(), &bytes[9_000..]);

    let beyond = depot
        .content
        .get(depot.machine, writing, &named, Some("bytes=20000-"))
        .await;
    assert_eq!(beyond.status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        beyond.header("content-range").as_deref(),
        Some("bytes */10000")
    );
}

#[tokio::test]
async fn content_that_is_not_in_the_store_is_not_found() {
    let depot = Depot::new().await;
    let absent = Hash::of(b"nobody uploaded this");
    let writing = depot
        .command(
            depot.machine,
            CommandKind::FsWrite,
            &json!({ "environment_id": new_id(), "path": "a.bin", "blob": absent }),
        )
        .await;
    let answer = depot
        .content
        .get(depot.machine, writing, &absent, None)
        .await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_command_that_writes_a_tree_down_is_given_the_files_its_list_names() {
    use orch_engine::manifest::{Entry, EntryKind, Header, build};
    let depot = Depot::new().await;
    let inside = depot.store(b"a file the list names").await;
    let outside = depot.store(b"a file that no list names").await;
    let written = build(
        &Header {
            v: 1,
            root: "/workspace".to_owned(),
            generated_at: orch_types::Time::new(depot.world.now()),
            generation: 1,
            case_sensitive: true,
        },
        vec![Entry {
            path: "src/a.txt".to_owned(),
            kind: EntryKind::File,
            size: 21,
            mtime_ns: 1,
            mode: 0o644,
            hash: Some(inside),
            link: None,
        }],
    )
    .unwrap();
    let list = depot
        .app()
        .blobs
        .put_bytes(written.bytes.clone().into())
        .await
        .unwrap();
    assert_eq!(list.hash, written.hash);
    let materializing = depot
        .command(
            depot.machine,
            CommandKind::FsMaterialize,
            &json!({
                "environment_id": new_id(), "job_id": null, "manifest_blob": list.hash,
                "target_root": "/workspace/web", "purpose": "delivery"
            }),
        )
        .await;

    let file = depot
        .content
        .get(depot.machine, materializing, &inside, None)
        .await;
    assert_eq!(file.status, StatusCode::OK, "{file:?}");
    assert_eq!(file.body.as_ref(), b"a file the list names");
    let list_itself = depot
        .content
        .get(depot.machine, materializing, &list.hash, None)
        .await;
    assert_eq!(
        list_itself.status,
        StatusCode::OK,
        "the list is in the command's body"
    );
    let other = depot
        .content
        .get(depot.machine, materializing, &outside, None)
        .await;
    assert_eq!(other.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn no_more_than_four_transfers_run_for_a_machine_at_once() {
    let depot = Depot::new().await;
    let bytes = bytes_of(1_000);
    let named = depot.store(&bytes).await;
    let writing = depot
        .command(
            depot.machine,
            CommandKind::FsWrite,
            &json!({ "environment_id": new_id(), "path": "a.bin", "blob": named }),
        )
        .await;
    let held: Vec<_> = (0..4)
        .map(|_| depot.app().blobs.begin_transfer(depot.machine).unwrap())
        .collect();
    let refused = depot
        .content
        .get(depot.machine, writing, &named, None)
        .await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    // Another machine has transfers of its own.
    let other = Depot::add_machine(&depot.world, &["environment"]).await;
    let theirs = depot
        .command(
            other,
            CommandKind::FsWrite,
            &json!({ "environment_id": new_id(), "path": "a.bin", "blob": named }),
        )
        .await;
    assert_eq!(
        depot.content.get(other, theirs, &named, None).await.status,
        StatusCode::OK
    );
    drop(held);
    let again = depot
        .content
        .get(depot.machine, writing, &named, None)
        .await;
    assert_eq!(
        again.status,
        StatusCode::OK,
        "a transfer that ended frees its place"
    );
}

// ---------------------------------------------------------------- what a machine makes on its own

#[tokio::test]
async fn a_machine_with_an_environment_uploads_the_list_of_its_tree_under_no_command() {
    let depot = Depot::new().await;
    let bytes = bytes_of(2_500);
    let hash = depot
        .content
        .upload(depot.machine, Id::nil(), &bytes, 1_024)
        .await;
    assert_eq!(
        depot.app().blobs.store().size(&hash).await.unwrap(),
        Some(2_500)
    );
}

#[tokio::test]
async fn content_made_on_a_machines_own_account_is_for_uploading_only_and_for_machines_with_an_environment()
 {
    let depot = Depot::new().await;
    let bytes = bytes_of(100);
    let hash = depot.store(&bytes).await;
    let down = depot
        .content
        .get(depot.machine, Id::nil(), &hash, None)
        .await;
    assert_eq!(
        down.status,
        StatusCode::FORBIDDEN,
        "nothing is downloaded without a command"
    );

    let provider_only = Depot::add_machine(&depot.world, &["provider"]).await;
    let fresh = bytes_of(120);
    let up = depot
        .content
        .put(provider_only, Id::nil(), &Hash::of(&fresh), 0, 120, &fresh)
        .await;
    assert_eq!(
        up.status,
        StatusCode::FORBIDDEN,
        "only a machine that has an environment has a tree to list"
    );
}

#[tokio::test]
async fn a_blob_that_arrived_is_a_blob_of_the_size_it_was_announced_with() {
    let depot = Depot::new().await;
    let command = depot.uploading().await;
    let bytes = bytes_of(6_000);
    let hash = depot
        .content
        .upload(depot.machine, command, &bytes, 2_048)
        .await;
    let meta = BlobMeta { hash, size: 6_000 };
    let mut conn = depot.world.conn().await;
    let row = storage::blob(&mut conn, &meta.hash).await.unwrap().unwrap();
    assert_eq!(row.size, meta.size);
}
