//! A machine can continue only its own unfinished content upload.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;
mod workspace;

use http::StatusCode;
use orch_db::queries::storage;
use orch_link_contract::commands::CommandKind;
use orch_types::{Hash, Id};
use serde_json::json;
use workspace::Depot;

#[tokio::test]
async fn a_machine_with_its_own_open_command_cannot_continue_another_machines_upload() {
    let depot = Depot::new().await;
    let command = depot.uploading().await;
    let other = Depot::add_machine(&depot.world, &["environment"]).await;
    let other_command = depot
        .command(other, CommandKind::FsFetch, &json!({"path":"other.txt"}))
        .await;
    let bytes = b"the complete content belongs to its uploading machine";
    let hash = Hash::of(bytes);
    let size = bytes.len() as i64;
    let first = depot
        .content
        .put(depot.machine, command, &hash, 0, size, &bytes[..4])
        .await;
    assert_eq!(first.status, StatusCode::OK);
    let rejected = depot
        .content
        .put(other, other_command, &hash, 4, size, &bytes[4..])
        .await;
    assert_eq!(rejected.status, StatusCode::FORBIDDEN);
    let mut conn = depot.world.conn().await;
    let row = storage::upload(&mut conn, &hash).await.unwrap().unwrap();
    assert!(
        storage::upload_of_command(&mut conn, depot.machine, command, &hash)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        storage::upload_of_command(&mut conn, other, other_command, &hash)
            .await
            .unwrap()
            .is_none()
    );
    drop(conn);
    assert_eq!(row.machine_id, Some(depot.machine));
    assert_eq!(row.received, 4);
    let completed = depot
        .content
        .put(depot.machine, command, &hash, 4, size, &bytes[4..])
        .await;
    assert_eq!(completed.status, StatusCode::OK);
    assert_eq!(completed.complete(), Some(true));
    assert_eq!(
        depot
            .app()
            .blobs
            .read_whole(&hash, size)
            .await
            .unwrap()
            .as_ref(),
        bytes
    );
    // The same second machine can start and finish content of its own.
    let own = b"a second machines own content";
    let accepted = depot
        .content
        .put(
            other,
            other_command,
            &Hash::of(own),
            0,
            own.len() as i64,
            own,
        )
        .await;
    assert_eq!(accepted.status, StatusCode::OK);
    assert_eq!(accepted.complete(), Some(true));
}

#[tokio::test]
async fn only_an_accepted_piece_attributes_an_upload_to_its_verified_command() {
    let depot = Depot::new().await;
    let first_command = depot.uploading().await;
    let next_command = depot.uploading().await;
    assert_ne!(first_command, next_command);
    let bytes = b"a resumed upload changes its exact command";
    let hash = Hash::of(bytes);
    let size = bytes.len() as i64;
    let first = depot
        .content
        .put(depot.machine, first_command, &hash, 0, size, &bytes[..4])
        .await;
    assert_eq!(first.status, StatusCode::OK);

    let machine = depot.machine;
    let attributed = |command: Id| {
        let world = &depot.world;
        async move {
            let mut conn = world.conn().await;
            storage::upload_of_command(&mut conn, machine, command, &hash)
                .await
                .unwrap()
        }
    };
    assert_eq!(attributed(first_command).await.unwrap().received, 4);
    assert!(attributed(next_command).await.is_none());

    let rejected = depot
        .content
        .put(depot.machine, next_command, &hash, 0, size, &bytes[..4])
        .await;
    assert_eq!(rejected.status, StatusCode::CONFLICT);
    assert!(attributed(next_command).await.is_none());
    assert_eq!(attributed(first_command).await.unwrap().received, 4);

    let resumed = depot
        .content
        .put(depot.machine, next_command, &hash, 4, size, &bytes[4..8])
        .await;
    assert_eq!(resumed.status, StatusCode::OK);
    assert!(attributed(first_command).await.is_none());
    assert_eq!(attributed(next_command).await.unwrap().received, 8);

    let finished = depot
        .content
        .put(depot.machine, next_command, &hash, 8, size, &bytes[8..])
        .await;
    assert_eq!(finished.complete(), Some(true));
    assert!(attributed(next_command).await.is_none());
    assert_eq!(
        depot
            .app()
            .blobs
            .read_whole(&hash, size)
            .await
            .unwrap()
            .as_ref(),
        bytes
    );
}
