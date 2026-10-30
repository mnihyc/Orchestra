//! What no document needs any more goes: a document left alone gets its version, a file nothing names lets go of its bytes after its time while one a block or a kept version names stays, and a purged document lets go of the bytes its files held.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use chrono::Duration;
use desk::Desk;
use orch_api_contract::documents::{
    BlockProps, BlockType, DocumentDelete, DocumentFile, DocumentFileCreate,
    DocumentFileCreateQuery, DocumentGet, DocumentOp, FileProps, NewBlock,
};
use orch_api_contract::threads::FILENAME_HEADER;
use orch_api_core::documents_upkeep;
use orch_db::Db;
use orch_db::queries::notifications::upkeep;
use orch_types::{Hash, Id, new_id};
use places::Who;

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

async fn pins(desk: &Desk, hash: &Hash) -> i32 {
    sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash = $1")
        .bind(hash.as_bytes().to_vec())
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap()
}

/// The names of the files a document lists, in order.
async fn files_of(desk: &Desk, by: &Who, document: Id) -> Vec<String> {
    let mut names: Vec<String> = desk
        .world
        .app
        .call(DocumentGet, &[&document])
        .token(&by.token)
        .send()
        .await
        .ok()
        .files
        .into_iter()
        .map(|file| file.name)
        .collect();
    names.sort();
    names
}

fn db(desk: &Desk) -> Db {
    Db::from_pool(desk.world.db.pool.clone())
}

#[tokio::test]
async fn a_file_nothing_names_any_more_goes_after_its_time_and_one_a_block_or_a_kept_version_names_stays()
 {
    let desk = Desk::new().await;
    let app = &desk.world.app.app;
    let id = desk.make(&desk.ada, "Handbook").await.base.id;
    let shown = put_file(&desk, &desk.ada, id, "shown.txt", b"shown words").await;
    let kept = put_file(&desk, &desk.ada, id, "kept.txt", b"kept words").await;
    let dropped = put_file(&desk, &desk.ada, id, "dropped.txt", b"dropped words").await;
    let (shows, keeps) = (showing(shown.id), showing(kept.id));
    let keeps_id = keeps.id;
    desk.write(&desk.ada, id, vec![desk::put(shows), desk::put(keeps)])
        .await
        .ok();

    // Left alone for as long as a document waits, it is given a version that holds both blocks.
    desk.world.advance(Duration::seconds(
        i64::try_from(app.config.documents.version_idle_secs).unwrap(),
    ));
    let cut = documents_upkeep::cut_versions(&db(&desk), app.clock.as_ref(), &app.config)
        .await
        .unwrap();
    assert!(cut >= 1);
    let versions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_versions WHERE document_id = $1")
            .bind(id)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    assert_eq!(versions, 1, "the document left alone got its version");

    // Then the block of the kept file goes, and a file is shown and taken away again before any
    // other version is cut: no version and no block names that one any more.
    let drops = showing(dropped.id);
    let drops_id = drops.id;
    desk.write(
        &desk.ada,
        id,
        vec![DocumentOp::Remove { id: keeps_id }, desk::put(drops)],
    )
    .await
    .ok();
    desk.write(&desk.ada, id, vec![DocumentOp::Remove { id: drops_id }])
        .await
        .ok();
    let (shown_bytes, dropped_bytes) = (
        blob_of(&desk, id, shown.id).await,
        blob_of(&desk, id, dropped.id).await,
    );

    // A day's purge before the removed blocks are old enough takes nothing.
    desk.world.advance(Duration::days(2));
    let early = documents_upkeep::purge(&db(&desk), app.clock.as_ref(), &app.config)
        .await
        .unwrap();
    assert_eq!(
        early.files, 0,
        "a removed block can still come back: {early:?}"
    );

    desk.world.advance(Duration::days(30));
    let loose = put_file(&desk, &desk.ada, id, "loose.txt", b"loose words").await;
    let purged = documents_upkeep::purge(&db(&desk), app.clock.as_ref(), &app.config)
        .await
        .unwrap();
    assert_eq!(purged.files, 1, "{purged:?}");
    assert_eq!(
        files_of(&desk, &desk.ada, id).await,
        vec!["kept.txt", "loose.txt", "shown.txt"],
        "a block names one, a kept version the other, and the third came a moment ago"
    );
    assert_eq!(
        pins(&desk, &dropped_bytes).await,
        0,
        "the document no longer holds the bytes of the file nothing named"
    );
    assert_eq!(pins(&desk, &shown_bytes).await, 1);
    assert_eq!(loose.name, "loose.txt");
}

#[tokio::test]
async fn a_purged_document_lets_go_of_the_bytes_its_files_held_and_another_that_holds_them_keeps_them()
 {
    let desk = Desk::new().await;
    let gone = desk.make(&desk.ada, "Old notes").await.base.id;
    let stays = desk.make(&desk.ada, "New notes").await.base.id;
    let first = put_file(&desk, &desk.ada, gone, "plan.txt", b"the same words").await;
    put_file(&desk, &desk.ada, stays, "plan.txt", b"the same words").await;
    let bytes = blob_of(&desk, gone, first.id).await;
    assert_eq!(
        pins(&desk, &bytes).await,
        2,
        "each document holds the bytes once"
    );
    let deleted = desk
        .world
        .app
        .call(DocumentDelete, &[&gone])
        .token(&desk.ada.token)
        .send()
        .await;
    assert_eq!(deleted.status, 204);

    desk.world.advance(Duration::days(31));
    let mut tx = db(&desk).begin().await.unwrap();
    let purged = upkeep::purge_soft_deleted(
        &mut tx,
        desk.world.now() - Duration::days(30),
        desk.world.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(
        purged
            .iter()
            .any(|each| each.table == "documents" && each.rows == 1),
        "{purged:?}"
    );
    assert_eq!(
        pins(&desk, &bytes).await,
        1,
        "the purged document no longer holds the bytes, and the other still does"
    );
}
