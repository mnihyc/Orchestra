//! Actual Copy acceptance, dispatcher replies, publication, cleanup custody and one grouped outcome.
use super::*;
use orch_link_contract::commands::{
    Cmd, CommandError, CommandKind, ErrorCode as NativeError, VolumeArchive,
};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{Notify, Semaphore};

struct Gate {
    entered: Semaphore,
    released: std::sync::atomic::AtomicBool,
    changed: Notify,
}
impl Gate {
    fn new() -> Self {
        Self {
            entered: Semaphore::new(0),
            released: std::sync::atomic::AtomicBool::new(false),
            changed: Notify::new(),
        }
    }
    async fn pass(&self) {
        self.entered.add_permits(1);
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.released.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            changed.await;
        }
    }
    fn release(&self) {
        self.released
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.changed.notify_waiters();
    }
}
struct Release(Arc<Gate>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
struct DataMachine {
    archive: VolumeArchive,
    import: Arc<Gate>,
    fail_import: bool,
    fail_cleanup: bool,
    importing: Mutex<Option<Id>>,
    stop: tokio_util::sync::CancellationToken,
}
#[async_trait::async_trait]
impl orch_api_core::testing::Machine for DataMachine {
    fn cancel(&self, id: Id) {
        if *self
            .importing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            == Some(id)
        {
            self.stop.cancel();
        }
    }
    async fn run(&self, command: &Cmd) -> Result<serde_json::Value, CommandError> {
        match command.kind.as_str() {
            "node.volume.carry-check" => Ok(json!({"available":true,"reason":null})),
            "node.mount.measure" => Ok(json!({"file_count":2,"total_bytes":4096})),
            "node.volume.ensure" => Ok(json!({})),
            "node.mount.export" => Ok(json!(self.archive)),
            "node.mount.import" => {
                *self
                    .importing
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(command.id);
                tokio::select! { () = self.import.pass() => {}, () = self.stop.cancelled() => return Err(CommandError::new(NativeError::Conflict,"the native copy was stopped")) }
                if self.fail_import {
                    Err(CommandError::new(
                        NativeError::Conflict,
                        "the target refused the imported data",
                    ))
                } else {
                    Ok(json!({}))
                }
            }
            "node.volume.remove" => {
                if self.fail_cleanup {
                    Err(CommandError::new(
                        NativeError::Conflict,
                        "the target could not remove its partial copy",
                    ))
                } else {
                    Ok(json!({}))
                }
            }
            _ => Err(CommandError::new(
                NativeError::Unsupported,
                "this peer answers only the reviewed mount data work",
            )),
        }
    }
}
async fn machine(world: &World, fail_import: bool, fail_cleanup: bool) -> Arc<DataMachine> {
    let meta = world
        .app
        .app
        .blobs
        .put_bytes(bytes::Bytes::from_static(b"reviewed native archive"))
        .await
        .unwrap();
    world
        .app
        .app
        .blobs
        .register(&mut *world.conn().await, &meta, world.now())
        .await
        .unwrap();
    Arc::new(DataMachine {
        archive: VolumeArchive {
            parts: vec![orch_link_contract::commands::BlobRef {
                hash: meta.hash,
                size: meta.size,
            }],
            size: meta.size,
        },
        import: Arc::new(Gate::new()),
        fail_import,
        fail_cleanup,
        importing: Mutex::new(None),
        stop: tokio_util::sync::CancellationToken::new(),
    })
}
async fn review(
    world: &World,
    people: &resources::Company,
    mount: Id,
    source: Id,
    target: Id,
) -> MountCopyPlan {
    world.app.app.engine.start();
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let plan = world
                .app
                .call(MountCopyPreflight, &[&mount])
                .token(&people.boss.token)
                .body(&MountCopyPreflightBody {
                    source_machine_id: source,
                    target_machine_id: target,
                    into: None,
                })
                .send()
                .await
                .ok()
                .into_full()
                .unwrap();
            if plan.copy.as_ref().is_some_and(|copy| copy.measured) {
                assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
                return plan;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("a native complete count precedes acceptance")
}
async fn start(world: &World, people: &resources::Company, mount: Id, plan: Id) -> Id {
    copy::accepted_copy(
        world
            .app
            .call(MountCopyCreate, &[&mount])
            .token(&people.boss.token)
            .body(&MountCopyCreateBody { preflight_id: plan })
            .send()
            .await
            .ok(),
    )
}
async fn notices(world: &World, person: Id) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT data FROM notifications WHERE user_id=$1 AND data->>'operation_kind'='mount_copy'",
    )
    .bind(person)
    .fetch_all(&world.db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_copy_publishes_only_after_its_native_import_finishes_without_notifying_other_users() {
    let world = World::new().await;
    let (people, mount, source, target) = copy::scene(&world).await;
    name_person(&world, "mount", mount, "mount.manage", people.viewer.id()).await;
    let peer = machine(&world, false, false).await;
    let _release = Release(peer.import.clone());
    let reader = world.app.play(source, peer.clone());
    let writer = world.app.play(target, peer.clone());
    let plan = review(&world, &people, mount, source, target).await;
    let job = start(&world, &people, mount, plan.preflight_id).await;
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        peer.import.entered.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    assert_eq!(start(&world, &people, mount, plan.preflight_id).await, job);
    let locations =
        orch_db::queries::resources::mounts::locations(&mut *world.conn().await, &[mount])
            .await
            .unwrap();
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].machine_id, source);
    assert!(notices(&world, people.viewer.id()).await.is_empty());
    peer.import.release();
    let done = deletion::closed(&world, job).await;
    world.app.app.engine.stop();
    assert_eq!(done.state, "succeeded", "{:?}", done.error);
    let locations =
        orch_db::queries::resources::mounts::locations(&mut *world.conn().await, &[mount])
            .await
            .unwrap();
    assert_eq!(locations.len(), 2);
    assert!(locations.iter().any(|row| row.machine_id == target));
    assert!(
        locations
            .iter()
            .any(|row| row.machine_id == source && row.source == "/srv/private-data")
    );
    assert_eq!(reader.seen_of(CommandKind::NodeMountExport).len(), 1);
    assert!(reader.seen_of(CommandKind::NodeMountImport).is_empty());
    assert_eq!(writer.seen_of(CommandKind::NodeMountImport).len(), 1);
    assert!(writer.seen_of(CommandKind::NodeMountExport).is_empty());
    assert!(notices(&world, people.viewer.id()).await.is_empty());
    assert!(notices(&world, people.boss.id()).await.is_empty());
    let claims: i64 = sqlx::query_scalar("SELECT count(*) FROM mount_copy_claims WHERE job_id=$1")
        .bind(job)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(claims, 0);
}

#[tokio::test]
async fn a_failed_copy_keeps_its_partial_owned_data_when_cleanup_is_refused_and_never_publishes_it()
{
    let world = World::new().await;
    let (people, mount, source, target) = copy::scene(&world).await;
    name_person(&world, "mount", mount, "mount.manage", people.viewer.id()).await;
    let peer = machine(&world, true, true).await;
    let _release = Release(peer.import.clone());
    let _reader = world.app.play(source, peer.clone());
    let writer = world.app.play(target, peer.clone());
    let plan = review(&world, &people, mount, source, target).await;
    let job = start(&world, &people, mount, plan.preflight_id).await;
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        peer.import.entered.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    peer.import.release();
    let done = deletion::closed(&world, job).await;
    world.app.app.engine.stop();
    assert_eq!(done.state, "failed");
    let locations =
        orch_db::queries::resources::mounts::locations(&mut *world.conn().await, &[mount])
            .await
            .unwrap();
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].machine_id, source);
    let kept = orch_db::queries::resources::mounts::kept(&mut *world.conn().await, &[mount])
        .await
        .unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].machine_id, target);
    assert_eq!(kept[0].job_id, Some(job));
    assert!(kept[0].file_count.is_none());
    assert!(kept[0].total_bytes.is_none());
    assert_eq!(writer.seen_of(CommandKind::NodeVolumeRemove).len(), 1);
    assert!(notices(&world, people.viewer.id()).await.is_empty());
}

#[tokio::test]
async fn cancelling_a_copy_joins_its_original_native_import_and_removes_only_its_owned_target() {
    let world = World::new().await;
    let (people, mount, source, target) = copy::scene(&world).await;
    let peer = machine(&world, false, false).await;
    let _release = Release(peer.import.clone());
    let reader = world.app.play(source, peer.clone());
    let writer = world.app.play(target, peer.clone());
    let plan = review(&world, &people, mount, source, target).await;
    let job = start(&world, &people, mount, plan.preflight_id).await;
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        peer.import.entered.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    world
        .app
        .call(orch_api_contract::workspace::JobCancel, &[&job])
        .token(&people.boss.token)
        .send()
        .await
        .ok();
    let done = deletion::closed(&world, job).await;
    world.app.app.engine.stop();
    assert_eq!(done.state, "cancelled", "{:?}", done.error);
    assert!(
        peer.stop.is_cancelled(),
        "the held original native operation must receive its actual stop"
    );
    assert!(
        reader.seen_of(CommandKind::NodeVolumeRemove).is_empty(),
        "cancellation never removes the source"
    );
    let removed = writer.seen_of(CommandKind::NodeVolumeRemove);
    assert_eq!(removed.len(), 1);
    assert_eq!(
        removed[0].body["labels"]["orch.copy_job"],
        json!(job.to_string())
    );
    let locations =
        orch_db::queries::resources::mounts::locations(&mut *world.conn().await, &[mount])
            .await
            .unwrap();
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].machine_id, source);
    assert!(
        orch_db::queries::resources::mounts::kept(&mut *world.conn().await, &[mount])
            .await
            .unwrap()
            .is_empty()
    );
}
