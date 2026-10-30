//! Cancelling the command future must stop the mount work before a caller can treat it as stopped.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use async_trait::async_trait;
use common::Rig;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::volumes::VolumeRequest;
use orch_link_contract::commands::{MountData, NodeMountExport};
use orch_node_core::drivers::{DriverExtras, DriverHost, HostDriver};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

use orch_connector_core::roles::{Call, RoleHandler};
use orch_driver_contract::runtime::{RuntimeKind, VolumeRef, VolumeRole};
use orch_driver_contract::units::UnitRef;
use orch_driver_contract::volumes::{OwnedVolumeSelector, VolumeMeasure};
use orch_link::commands::CommandContext;
use orch_link_contract::commands::{
    BlobRef, Command, NodeMountImport, NodeMountMeasure, NodeRuntimeVolumeImport,
    NodeRuntimeVolumesExport, NodeRuntimeVolumesMeasure, NodeVolumeEnsure, NodeVolumeRemove,
    SelectedVolume, SerialKey, VolumeArchive,
};
use orch_types::{Id, new_id};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;
use std::sync::atomic::AtomicU32;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Export {
    started: Notify,
    release: Notify,
    finished: Notify,
    wrote: AtomicBool,
}

struct Finished(Arc<Export>);

impl Drop for Finished {
    fn drop(&mut self) {
        self.0.finished.notify_one();
    }
}

struct SlowExport(Arc<Export>);

#[async_trait]
impl DriverExtras for SlowExport {
    async fn export_volume(
        &self,
        _: &VolumeRequest,
        _: &std::path::Path,
    ) -> Result<(), DriverError> {
        let _finished = Finished(Arc::clone(&self.0));
        self.0.started.notify_one();
        self.0.release.notified().await;
        self.0.wrote.store(true, Ordering::SeqCst);
        // No archive or upload is needed: the effect after cancellation is already observable.
        Err(DriverError::Unavailable("the test export has ended".into()))
    }
}

#[tokio::test]
async fn cancelling_a_mount_export_stops_the_driver_before_it_can_continue_writing() {
    let mut rig = Rig::new();
    let state = Arc::new(Export::default());
    let contract: Arc<dyn EnvironmentDriver> = rig.driver.clone();
    let drivers = DriverHost::of(
        &rig.settings,
        Arc::clone(&rig.desk),
        vec![HostDriver::new(contract).with_extras(Arc::new(SlowExport(Arc::clone(&state))))],
    );
    rig.start_with(drivers).await;
    let runtimes = rig.role().runtimes().clone();
    let mount = orch_types::new_id();
    let body = NodeMountExport {
        work: None,
        driver: "memory".into(),
        job_id: orch_types::new_id(),
        mount_id: mount,
        data: MountData::Volume {
            name: format!("orch-mount-{mount}"),
            storage: None,
        },
        part_bytes: 1024,
    };
    let command =
        tokio::spawn(async move { runtimes.export_mount(orch_types::new_id(), body).await });
    tokio::time::timeout(common::PATIENCE, state.started.notified())
        .await
        .unwrap();
    command.abort();
    assert!(command.await.unwrap_err().is_cancelled());
    state.release.notify_one();
    tokio::time::timeout(common::PATIENCE, state.finished.notified())
        .await
        .unwrap();
    let wrote = state.wrote.load(Ordering::SeqCst);
    rig.stop().await;
    assert!(
        !wrote,
        "the driver continued after the cancelled command future ended"
    );
}

/// A bounded chunk and short pause make a real child's file growth visible without filling the disk.
const HELPER_PIECE: usize = 64 * 1024;
const OBSERVATION_PAUSE: Duration = Duration::from_millis(100);

#[test]
#[ignore = "started as a child process by the cancellation tests"]
fn a_native_copy_helper_keeps_writing_until_its_owner_stops_it() {
    use std::io::Write;
    let path = std::env::var_os("ORCH_TEST_COPY_HELPER_OUTPUT").unwrap();
    let mut file = std::fs::File::create(path).unwrap();
    loop {
        file.write_all(&[0; HELPER_PIECE]).unwrap();
        file.flush().unwrap();
        std::thread::sleep(OBSERVATION_PAUSE);
    }
}

struct NativeCopy {
    output: PathBuf,
    child: Mutex<Option<tokio::process::Child>>,
    pid: AtomicU32,
    started: Notify,
    cleaning: Notify,
    release_cleanup: Notify,
    cleaned: AtomicBool,
    resumed: AtomicBool,
    resumed_early: AtomicBool,
    stopped_commands: Mutex<Vec<Id>>,
}

impl NativeCopy {
    fn new(output: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            output,
            child: Mutex::new(None),
            pid: AtomicU32::new(0),
            started: Notify::new(),
            cleaning: Notify::new(),
            release_cleanup: Notify::new(),
            cleaned: AtomicBool::new(false),
            resumed: AtomicBool::new(false),
            resumed_early: AtomicBool::new(false),
            stopped_commands: Mutex::new(Vec::new()),
        })
    }

    async fn run<T>(&self) -> Result<T, DriverError> {
        let child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "a_native_copy_helper_keeps_writing_until_its_owner_stops_it",
                "--ignored",
                "--nocapture",
            ])
            .env("ORCH_TEST_COPY_HELPER_OUTPUT", &self.output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        self.pid.store(child.id().unwrap(), Ordering::SeqCst);
        *self.child.lock().unwrap() = Some(child);
        tokio::time::timeout(common::PATIENCE, async {
            while !matches!(std::fs::metadata(&self.output), Ok(meta) if meta.len() > 0) {
                tokio::time::sleep(OBSERVATION_PAUSE).await;
            }
        })
        .await
        .unwrap();
        self.started.notify_one();
        std::future::pending().await
    }
}

struct NativeExtras {
    work: Arc<NativeCopy>,
    base: Arc<common::MemoryExtras>,
}

#[async_trait]
impl DriverExtras for NativeExtras {
    async fn measure_volume(&self, _: &VolumeRequest) -> Result<VolumeMeasure, DriverError> {
        self.work.run().await
    }
    async fn export_volume(
        &self,
        _: &VolumeRequest,
        _: &std::path::Path,
    ) -> Result<(), DriverError> {
        self.work.run().await
    }
    async fn import_volume_stream(
        &self,
        _: &VolumeRequest,
        _: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
    ) -> Result<(), DriverError> {
        self.work.run().await
    }
    async fn stop_volume_work(&self, command: Id) -> Result<(), DriverError> {
        self.work.stopped_commands.lock().unwrap().push(command);
        let child = self.work.child.lock().unwrap().take();
        if let Some(mut child) = child {
            child.start_kill().unwrap();
            child.wait().await.unwrap();
        }
        self.work.cleaning.notify_one();
        self.work.release_cleanup.notified().await;
        self.work.cleaned.store(true, Ordering::SeqCst);
        Ok(())
    }
    async fn volume_exists(&self, volume: &VolumeRequest) -> Result<bool, DriverError> {
        Ok(self.base.driver.volume(&volume.name).is_some())
    }
    async fn suspend(&self, unit: &UnitRef) -> Result<(), DriverError> {
        self.base.suspend(unit).await
    }
    async fn resume(&self, unit: &UnitRef) -> Result<(), DriverError> {
        self.work
            .resumed_early
            .store(!self.work.cleaned.load(Ordering::SeqCst), Ordering::SeqCst);
        self.work.resumed.store(true, Ordering::SeqCst);
        self.base.resume(unit).await
    }
    async fn enrolled(&self, unit: &UnitRef) -> Option<bool> {
        self.base.enrolled(unit).await
    }
}

#[derive(Debug, Clone, Copy)]
enum CopyKind {
    MountCount,
    MountExport,
    MountImport,
    HomeCount,
    HomeExport,
    HomeImport,
}

#[derive(Clone, Copy)]
enum StopBy {
    Token,
    DropWaiter,
    Shutdown,
}

async fn native_cancellation(kind: CopyKind, stop_by: StopBy) {
    // This helper pauses a running source, then checks that cleanup precedes its resume.
    let mut rig = Rig::with_driver(
        orch_testkit::doubles::driver::MemoryDriver::new("memory")
            .replacing()
            .with_suspend(),
    );
    let state = NativeCopy::new(rig.data.path().join("helper-output"));
    let contract: Arc<dyn EnvironmentDriver> = rig.driver.clone();
    let drivers = DriverHost::of(
        &rig.settings,
        Arc::clone(&rig.desk),
        vec![
            HostDriver::new(contract).with_extras(Arc::new(NativeExtras {
                work: Arc::clone(&state),
                base: Arc::clone(&rig.extras),
            })),
        ],
    );
    rig.have_connector();
    rig.start_with(drivers).await;
    let command = new_id();
    let mut spec = rig.spec(RuntimeKind::Environment, "copy-source");
    let runtime = spec.runtime_id;
    let mount = new_id();
    let source_runtime = matches!(kind, CopyKind::HomeCount | CopyKind::HomeExport);
    let name = if matches!(
        kind,
        CopyKind::HomeCount | CopyKind::HomeExport | CopyKind::HomeImport
    ) {
        format!("orch-home-{runtime}")
    } else {
        format!("orch-mount-{mount}")
    };
    let selected = SelectedVolume {
        selector: OwnedVolumeSelector::Home,
        volume: VolumeRef {
            name: name.clone(),
            storage: None,
            directory: None,
        },
    };
    if source_runtime {
        spec.volumes
            .insert(VolumeRole::Home, selected.volume.clone());
        rig.made(&spec).await;
        rig.start_runtime(runtime).await;
    } else {
        rig.succeed(
            &SerialKey::Volume(name.clone()),
            &NodeVolumeEnsure {
                driver: "memory".into(),
                name: name.clone(),
                storage: None,
                labels: BTreeMap::new(),
            },
        )
        .await;
    }
    let data = MountData::Volume {
        name: name.clone(),
        storage: None,
    };
    let bytes = bytes::Bytes::from_static(b"a verified archive part");
    let hash = rig.blobs.insert(bytes.clone());
    rig.blobs.open_command(command);
    let archive = VolumeArchive {
        parts: vec![BlobRef {
            hash,
            size: bytes.len() as i64,
        }],
        size: bytes.len() as i64,
    };
    let body = match kind {
        CopyKind::MountCount => Command::NodeMountMeasure(NodeMountMeasure {
            work: None,
            driver: "memory".into(),
            mount_id: mount,
            data,
        }),
        CopyKind::MountExport => Command::NodeMountExport(NodeMountExport {
            work: None,
            driver: "memory".into(),
            job_id: new_id(),
            mount_id: mount,
            data,
            part_bytes: 1024,
        }),
        CopyKind::MountImport => Command::NodeMountImport(NodeMountImport {
            work: None,
            driver: "memory".into(),
            job_id: new_id(),
            mount_id: mount,
            name: name.clone(),
            storage: None,
            archive,
        }),
        CopyKind::HomeCount => Command::NodeRuntimeVolumesMeasure(NodeRuntimeVolumesMeasure {
            work: None,
            runtime_id: runtime,
            volumes: vec![selected],
        }),
        CopyKind::HomeExport => Command::NodeRuntimeVolumesExport(NodeRuntimeVolumesExport {
            work: None,
            runtime_id: runtime,
            volumes: vec![selected],
            part_bytes: 1024,
        }),
        CopyKind::HomeImport => Command::NodeRuntimeVolumeImport(NodeRuntimeVolumeImport {
            work: None,
            provider_id: None,
            runtime_id: runtime,
            driver: "memory".into(),
            environment_id: new_id(),
            selected,
            archive,
        }),
    };
    let cancel = CancellationToken::new();
    let role = rig.role();
    let call = Call {
        context: CommandContext {
            id: command,
            key: None,
            deadline: rig.deadline(),
            cancel: cancel.clone(),
        },
        clamps: Vec::new(),
    };
    let mut task = Some(tokio::spawn(async move { role.handle(call, body).await }));
    tokio::time::timeout(common::PATIENCE, state.started.notified())
        .await
        .unwrap();
    let shutdown = match stop_by {
        StopBy::DropWaiter => {
            let task = task.take().unwrap();
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            None
        }
        StopBy::Token => {
            cancel.cancel();
            None
        }
        StopBy::Shutdown => {
            let role = rig.role();
            Some(tokio::spawn(async move { role.stop().await }))
        }
    };
    tokio::time::timeout(common::PATIENCE, state.cleaning.notified())
        .await
        .unwrap();
    assert_eq!(*state.stopped_commands.lock().unwrap(), vec![command]);
    let pid = state.pid.load(Ordering::SeqCst);
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "the native helper survived cancellation"
    );
    let staging = rig
        .data
        .path()
        .join("cache")
        .join("commands")
        .join(command.to_string());
    if !matches!(kind, CopyKind::MountCount | CopyKind::HomeCount) {
        assert!(staging.exists(), "the copy never owned a staging directory");
    }
    let stopped_bytes = std::fs::metadata(&state.output).unwrap().len();
    assert!(stopped_bytes > 0);
    let role = rig.role();
    let remove = NodeVolumeRemove {
        driver: "memory".into(),
        name: name.clone(),
        storage: None,
        labels: BTreeMap::new(),
    };
    let call = Call {
        context: CommandContext {
            id: new_id(),
            key: None,
            deadline: rig.deadline(),
            cancel: CancellationToken::new(),
        },
        clamps: Vec::new(),
    };
    let undo =
        tokio::spawn(async move { role.handle(call, Command::NodeVolumeRemove(remove)).await });
    tokio::time::sleep(OBSERVATION_PAUSE).await;
    assert!(
        !undo.is_finished(),
        "volume serialization was released before cleanup joined"
    );
    assert!(
        !task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished),
        "the command acknowledged cancellation before cleanup joined"
    );
    assert!(
        !state.resumed_early.load(Ordering::SeqCst),
        "the source resumed while cleanup was pending"
    );
    assert_eq!(
        std::fs::metadata(&state.output).unwrap().len(),
        stopped_bytes,
        "the helper file grew after its process was joined"
    );
    assert!(
        !shutdown
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished),
        "node shutdown released its copy worker before cleanup joined"
    );
    state.release_cleanup.notify_one();
    if let Some(task) = task {
        assert!(
            tokio::time::timeout(common::PATIENCE, task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
    }
    let undo = tokio::time::timeout(common::PATIENCE, undo)
        .await
        .unwrap()
        .unwrap();
    if source_runtime {
        assert_eq!(
            undo.unwrap_err().code,
            "conflict",
            "a running source still owns its volume"
        );
    } else {
        undo.unwrap();
    }
    if let Some(shutdown) = shutdown {
        tokio::time::timeout(common::PATIENCE, shutdown)
            .await
            .unwrap()
            .unwrap();
    }
    assert!(state.cleaned.load(Ordering::SeqCst));
    if matches!(kind, CopyKind::HomeExport) {
        assert!(state.resumed.load(Ordering::SeqCst));
        assert!(!state.resumed_early.load(Ordering::SeqCst));
        assert!(!rig.role().ledger().volume_copy_holds().contains(&runtime));
    }
    rig.stop().await;
    assert_eq!(
        std::fs::metadata(&state.output).unwrap().len(),
        stopped_bytes,
        "the file grew after cancellation was acknowledged"
    );
    assert!(!staging.exists(), "command staging survived cancellation");
}

#[tokio::test]
async fn a_mount_copy_joins_its_native_helper_and_holds_the_volume_until_cleanup_ends() {
    native_cancellation(CopyKind::MountExport, StopBy::Token).await;
    native_cancellation(CopyKind::MountImport, StopBy::Token).await;
}

#[tokio::test]
async fn a_measurement_joins_its_native_helper_and_holds_the_volume_until_cleanup_ends() {
    native_cancellation(CopyKind::MountCount, StopBy::Token).await;
    native_cancellation(CopyKind::HomeCount, StopBy::Token).await;
}

#[tokio::test]
async fn a_volume_carry_joins_its_native_helper_before_resuming_the_source_or_allowing_undo() {
    native_cancellation(CopyKind::HomeExport, StopBy::Token).await;
    native_cancellation(CopyKind::HomeImport, StopBy::Token).await;
}

#[tokio::test]
async fn dropping_a_waiter_still_stops_and_joins_measurement_and_volume_carry_work() {
    native_cancellation(CopyKind::MountCount, StopBy::DropWaiter).await;
    native_cancellation(CopyKind::HomeExport, StopBy::DropWaiter).await;
}

#[tokio::test]
async fn node_shutdown_joins_its_native_copy_helper_and_final_cleanup() {
    native_cancellation(CopyKind::MountExport, StopBy::Shutdown).await;
}
