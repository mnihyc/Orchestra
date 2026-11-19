//! Permanent HTTP trail and surviving snapshot bytes after completed private references are collected.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::nodes::{AuditObjectState, NodeAuditList, NodeAuditListQuery, NodeGet};
use orch_api_contract::ownership::{NodeDelete, NodeDeleteBody};
use orch_api_contract::workspace::{SnapshotFileContentGet, SnapshotPathQuery};
use orch_db::queries::{notifications::upkeep, storage};
use orch_engine::manifest::{self, Entry, EntryKind, Header};
use orch_types::{Time, new_id};
use serde_json::json;

async fn sweep(scene: &rig::Scene) {
    let mut tx = scene.rig.db.begin().await.unwrap();
    upkeep::purge_soft_deleted(
        &mut tx,
        scene.rig.world.now() - chrono::Duration::days(30),
        scene.rig.world.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn completed_records_do_not_hold_deleted_threads_and_shared_bytes_remain_readable() {
    let scene = rig::Scene::new().await;
    let dead = scene.thread_by(&scene.ada, "Removed thread").await.base.id;
    let live = scene
        .thread_by(&scene.ada, "Surviving thread")
        .await
        .base
        .id;
    let bytes = bytes::Bytes::from_static(b"the independent snapshot's evidence\n");
    let blob = scene
        .rig
        .world
        .app
        .app
        .blobs
        .put_bytes(bytes.clone())
        .await
        .unwrap();
    let written = manifest::build(
        &Header {
            v: manifest::MANIFEST_VERSION,
            root: "/workspace/web".to_owned(),
            generated_at: Time::new(scene.rig.world.now()),
            generation: 1,
            case_sensitive: true,
        },
        vec![Entry {
            path: "evidence.txt".to_owned(),
            kind: EntryKind::File,
            size: blob.size,
            mtime_ns: 0,
            mode: 0o644,
            hash: Some(blob.hash),
            link: None,
        }],
    )
    .unwrap();
    let list = scene
        .rig
        .world
        .app
        .app
        .blobs
        .put_bytes(bytes::Bytes::from(written.bytes))
        .await
        .unwrap();
    let mut conn = scene.rig.db.acquire().await.unwrap();
    storage::register_blob(&mut conn, &blob.hash, blob.size, scene.rig.world.now())
        .await
        .unwrap();
    storage::register_blob(&mut conn, &list.hash, list.size, scene.rig.world.now())
        .await
        .unwrap();
    let snapshots = [new_id(), new_id()];
    for (thread, snapshot) in [dead, live].into_iter().zip(snapshots) {
        storage::begin_snapshot(
            &mut conn,
            &storage::NewSnapshot {
                id: snapshot,
                thread_id: Some(thread),
                environment_id: scene.on.environment,
                env_generation: 1,
                label: "Independent evidence",
                selection: &json!({"include":["**"],"exclude":[]}),
                created_by: Some(scene.ada.id()),
                at: scene.rig.world.now(),
            },
        )
        .await
        .unwrap();
        storage::end_snapshot(
            &mut conn,
            snapshot,
            &storage::SnapshotEnd {
                state: storage::snapshot_state::COMPLETE,
                file_count: 1,
                total_bytes: blob.size,
                missing_count: 0,
                manifest_blob: Some(list.hash),
                at: scene.rig.world.now(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            storage::pin_snapshot(&mut conn, snapshot, &[list.hash, blob.hash, blob.hash])
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            storage::pin_snapshot(&mut conn, snapshot, &[list.hash, blob.hash])
                .await
                .unwrap(),
            2
        );
    }
    let job = new_id();
    sqlx::query(
        "INSERT INTO jobs(id,kind,node_id,state,created_by) VALUES($1,'export',$2,'running',$3)",
    )
    .bind(job)
    .bind(dead)
    .bind(scene.ada.id())
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("INSERT INTO approvals(id,kind,state,node_id,thread_id,title,request,required_permission) VALUES($1,'harness','answered',$2,$2,'Completed private answer','{}','thread.interact')")
        .bind(new_id()).bind(dead).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO leases(id,provider_id,thread_id,allowed_models,closed_at,close_reason) VALUES($1,$2,$3,$4,$5,'turn_ended')")
        .bind(new_id()).bind(scene.on.provider).bind(dead).bind(vec![rig::MODEL]).bind(scene.rig.world.now()).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO secrets(id,owner_kind,owner_id,name,sealed) VALUES($1,'setting',$2,'PRIVATE_TOKEN',$3)")
        .bind(new_id()).bind(dead).bind(vec![7_u8;32]).execute(&mut *conn).await.unwrap();
    drop(conn);
    scene
        .rig
        .world
        .app
        .call(NodeDelete, &[&dead])
        .token(&scene.ada.token)
        .body(&NodeDeleteBody::default())
        .send()
        .await
        .ok();
    scene.rig.world.advance(chrono::Duration::days(31));
    sweep(&scene).await;
    let mut conn = scene.rig.db.acquire().await.unwrap();
    let held: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT purged_at FROM audit_object_state WHERE object_kind='node' AND object_id=$1",
    )
    .bind(dead)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert!(
        held.is_none(),
        "an active job keeps retained inspection and does not publish a purge day"
    );
    let pins: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(blob.hash.as_bytes().as_slice())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(pins, 2, "rejected collection releases no content");
    sqlx::query("UPDATE jobs SET state='succeeded',finished_at=$2 WHERE id=$1")
        .bind(job)
        .bind(scene.rig.world.now())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    sweep(&scene).await;
    sweep(&scene).await;
    let mut conn = scene.rig.db.acquire().await.unwrap();
    let gone:bool = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM nodes WHERE id=$1) AND NOT EXISTS(SELECT 1 FROM approvals WHERE thread_id=$1) AND NOT EXISTS(SELECT 1 FROM leases WHERE thread_id=$1) AND NOT EXISTS(SELECT 1 FROM jobs WHERE node_id=$1) AND NOT EXISTS(SELECT 1 FROM secrets WHERE owner_id=$1)")
        .bind(dead).fetch_one(&mut *conn).await.unwrap();
    assert!(
        gone,
        "completed private records and credentials must not hold the full thread"
    );
    for hash in [blob.hash, list.hash] {
        let pins: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
            .bind(hash.as_bytes().as_slice())
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            pins, 1,
            "another snapshot owns its own pin even after repeated cleanup"
        );
    }
    assert!(
        storage::snapshot(&mut conn, snapshots[0])
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        storage::snapshot(&mut conn, snapshots[1])
            .await
            .unwrap()
            .is_some()
    );
    drop(conn);
    let trail = scene
        .rig
        .world
        .app
        .call(NodeAuditList, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .query(&NodeAuditListQuery::default())
        .send()
        .await
        .ok();
    let event = trail
        .items
        .iter()
        .find(|row| row.object.id == Some(dead) && row.action == "node.deleted")
        .unwrap();
    assert_eq!(event.object.name.as_deref(), Some("Removed thread"));
    assert_eq!(event.object.state, AuditObjectState::Purged);
    assert_eq!(
        event.object.purged_at,
        Some(Time::new(scene.rig.world.now()))
    );
    assert!(!event.object.can_inspect && !event.object.can_restore);
    assert_eq!(
        scene
            .rig
            .world
            .app
            .call(NodeGet, &[&dead])
            .token(&scene.ada.token)
            .send()
            .await
            .status,
        404
    );
    let read = scene
        .rig
        .world
        .app
        .call(SnapshotFileContentGet, &[&snapshots[1]])
        .token(&scene.ada.token)
        .query(&SnapshotPathQuery {
            path: "evidence.txt".to_owned(),
        })
        .send_bytes()
        .await;
    assert_eq!(read.status, http::StatusCode::OK);
    assert_eq!(read.raw.as_ref(), bytes.as_ref());
}
