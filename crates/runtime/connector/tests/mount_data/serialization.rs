//! Native import owns its directory while an attachment waits; another environment can still start.
use super::*;
use async_trait::async_trait;
use bytes::Bytes;
use orch_link::{
    commands::Progress,
    content::{BlobEndpoint, PutRefusal},
    journal::{Entry, Step},
};
use orch_link_contract::{
    commands::{
        CommandBody, MountData, NodeMountExport, NodeMountImport, SerialKey, VolumeArchive,
    },
    content::{BlobProgress, PutQuery, TransferStatus},
};
use orch_testkit::doubles::link::ShapedBlobs;
use orch_types::{Hash, Id};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{Notify, Semaphore};

struct Pause {
    entered: Semaphore,
    open: std::sync::atomic::AtomicBool,
    changed: Notify,
}
impl Pause {
    fn new() -> Self {
        Self {
            entered: Semaphore::new(0),
            open: std::sync::atomic::AtomicBool::new(false),
            changed: Notify::new(),
        }
    }
    async fn pass(&self) {
        self.entered.add_permits(1);
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.open.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            changed.await;
        }
    }
    fn release(&self) {
        self.open.store(true, std::sync::atomic::Ordering::SeqCst);
        self.changed.notify_waiters();
    }
}
struct Release(Arc<Pause>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
struct Endpoint {
    inner: ShapedBlobs,
    held: Mutex<Option<(Id, Arc<Pause>)>>,
}
#[async_trait]
impl BlobEndpoint for Endpoint {
    async fn status(&self, command: Id, hash: &Hash) -> Result<TransferStatus, PutRefusal> {
        self.inner.status(command, hash).await
    }

    async fn head(&self, command: Id, hash: &Hash) -> Result<Option<BlobProgress>, PutRefusal> {
        self.inner.head(command, hash).await
    }
    async fn put(
        &self,
        command: Id,
        hash: &Hash,
        query: PutQuery,
        bytes: Bytes,
    ) -> Result<BlobProgress, PutRefusal> {
        self.inner.put(command, hash, query, bytes).await
    }
    async fn get(
        &self,
        command: Id,
        hash: &Hash,
        offset: i64,
        len: i64,
    ) -> Result<Bytes, PutRefusal> {
        let pause = self
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .filter(|(id, _)| *id == command)
            .map(|(_, pause)| Arc::clone(pause));
        if let Some(pause) = pause {
            pause.pass().await;
        }
        self.inner.get(command, hash, offset, len).await
    }
    async fn missing(&self, command: Id, hashes: &[Hash]) -> Result<Vec<Hash>, PutRefusal> {
        self.inner.missing(command, hashes).await
    }
}
fn issue<B: CommandBody>(rig: &EnvRig, id: Id, key: &SerialKey, body: &B) {
    rig.blobs.open_command(id);
    rig.controller.issue_with_id(id, key, rig.deadline(), body);
}

#[tokio::test]
async fn an_attachment_waiting_for_mount_import_does_not_block_another_environments_native_start() {
    let mut rig = EnvRig::new();
    let source = rig.directory("source");
    std::fs::write(source.join("data"), b"complete copy").unwrap();
    let workspace = rig.directory("workspace");
    let other_workspace = rig.directory("other-workspace");
    let endpoint = Arc::new(Endpoint {
        inner: ShapedBlobs::new(Arc::clone(&rig.blobs), None, i64::MAX),
        held: Mutex::new(None),
    });
    rig.blob_endpoint = Some(endpoint.clone());
    rig.start().await;
    let mount = new_id();
    let job = new_id();
    let name = format!("orch-mount-{mount}");
    let labels = BTreeMap::from([
        ("orch.mount_id".into(), mount.to_string()),
        ("orch.copy_job".into(), job.to_string()),
    ]);
    let archive: VolumeArchive = serde_json::from_value(
        rig.succeed(&NodeMountExport {
            work: None,
            driver: "directory".into(),
            job_id: job,
            mount_id: mount,
            data: MountData::Path {
                path: source.to_str().unwrap().into(),
            },
            part_bytes: 4096,
        })
        .await,
    )
    .unwrap();
    rig.succeed(&NodeVolumeEnsure {
        driver: "directory".into(),
        name: name.clone(),
        storage: None,
        labels: labels.clone(),
    })
    .await;
    let import = new_id();
    let pause = Arc::new(Pause::new());
    let _release = Release(pause.clone());
    *endpoint.held.lock().unwrap_or_else(PoisonError::into_inner) = Some((import, pause.clone()));
    issue(
        &rig,
        import,
        &SerialKey::Volume(name.clone()),
        &NodeMountImport {
            work: None,
            driver: "directory".into(),
            job_id: job,
            mount_id: mount,
            name: name.clone(),
            storage: None,
            archive,
        },
    );
    tokio::time::timeout(rig_environment::PATIENCE, pause.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let attach: EnvAttach = serde_json::from_value(json!({"environment_id":rig.environment,"workspace_root":workspace,"ignore":[],"index":"live",
        "mounts":[{"mount_id":mount,"name":"data","source":name,"volume":{"name":name,"storage":null},"mode":"read_only"}],"variables":{},"secrets":{}})).unwrap();
    let attaching = new_id();
    issue(
        &rig,
        attaching,
        &SerialKey::Environment(rig.environment),
        &attach,
    );
    tokio::time::timeout(rig_environment::PATIENCE, async {
        loop {
            let path = rig
                .data
                .path()
                .join("journal")
                .join(format!("{attaching}.json"));
            if std::fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Entry>(&bytes).ok())
                .is_some_and(|entry| matches!(entry.step, Step::Started))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let other = new_id();
    let mut independent = attach.clone();
    independent.environment_id = other;
    independent.workspace_root = Some(other_workspace.to_str().unwrap().into());
    independent.mounts = None;
    let starting = new_id();
    issue(&rig, starting, &SerialKey::Environment(other), &independent);
    let ready = match tokio::time::timeout(
        rig_environment::PATIENCE,
        rig.controller.wait_command(starting),
    )
    .await
    .expect("an unrelated environment can start while the import and first attachment wait")
    {
        Progress::Succeeded(facts) => facts,
        other => panic!("the independent environment did not start: {other:?}"),
    };
    assert_eq!(
        ready["workspace_root"],
        json!(other_workspace.to_str().unwrap())
    );
    assert!(
        !matches!(
            rig.controller.progress(attaching),
            Some(Progress::Succeeded(_))
        ),
        "the first attachment must wait until its data import really ends"
    );
    pause.release();
    assert!(matches!(
        tokio::time::timeout(
            rig_environment::PATIENCE,
            rig.controller.wait_command(import)
        )
        .await
        .unwrap(),
        Progress::Succeeded(_)
    ));
    assert!(matches!(
        tokio::time::timeout(
            rig_environment::PATIENCE,
            rig.controller.wait_command(attaching)
        )
        .await
        .unwrap(),
        Progress::Succeeded(_)
    ));
    for environment_id in [rig.environment, other] {
        rig.succeed(&EnvDetach { environment_id }).await;
    }
    rig.succeed(&NodeVolumeRemove {
        driver: "directory".into(),
        name,
        storage: None,
        labels,
    })
    .await;
    rig.stop().await;
}
