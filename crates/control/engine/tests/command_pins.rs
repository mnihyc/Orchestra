//! What the commands of a job hold for it: the content their machine uploaded or was told the system holds, held until
//! the job ends, and removed with its end when nothing else holds it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::World;
use orch_blob_contract::store::{BlobStore, stream_of};
use orch_blob_local::LocalBlobStore;
use orch_db::queries::storage;
use orch_types::{Hash, new_id};
use std::sync::Arc;

#[tokio::test]
async fn the_end_of_a_job_lets_go_of_what_its_commands_held_and_removes_what_nothing_else_holds() {
    let world = World::new().await;
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(
        LocalBlobStore::open(directory.path().join("blobs"))
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store.clone());
    let (alone, shared) = (
        Hash::of(b"what only the transfer holds"),
        Hash::of(b"what a snapshot holds as well"),
    );
    let mut conn = world.pool.acquire().await.unwrap();
    for (hash, bytes) in [
        (alone, b"what only the transfer holds".as_slice()),
        (shared, b"what a snapshot holds as well".as_slice()),
    ] {
        store
            .put(
                &hash,
                Some(i64::try_from(bytes.len()).unwrap()),
                stream_of(vec![bytes::Bytes::copy_from_slice(bytes)]),
            )
            .await
            .unwrap();
        storage::register_blob(
            &mut conn,
            &hash,
            i64::try_from(bytes.len()).unwrap(),
            world.clock.now(),
        )
        .await
        .unwrap();
    }
    // A snapshot holds one of them for its own reasons.
    storage::pin_blobs(&mut conn, &[shared]).await.unwrap();
    let (job, command) = (new_id(), new_id());
    storage::pin_for_command(&mut conn, command, job, &[alone, shared], world.clock.now())
        .await
        .unwrap();
    drop(conn);

    world.engine().release_command_pins(job).await.unwrap();

    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        storage::blob(&mut conn, &alone).await.unwrap().is_none(),
        "what nothing else holds goes with the job"
    );
    assert!(!store.exists(&alone).await.unwrap());
    assert_eq!(
        storage::blob(&mut conn, &shared)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        1,
        "what a snapshot holds stays, with the snapshot's pin alone"
    );
    assert!(store.exists(&shared).await.unwrap());
    world.engine().release_command_pins(job).await.unwrap();
    assert_eq!(
        storage::blob(&mut conn, &shared)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        1,
        "a second end takes nothing more"
    );
    world.engine().stop();
}
