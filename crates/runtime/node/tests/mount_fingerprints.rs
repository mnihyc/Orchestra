//! Mount content checks run through the node's ownership and directory policy before reaching a driver's read-only helper.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use async_trait::async_trait;
use common::Rig;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::runtime::DirectoryBinding;
use orch_driver_contract::volumes::VolumeRequest;
use orch_link_contract::commands::{MountData, NodeMountFingerprint, SerialKey};
use orch_node_core::drivers::{DriverExtras, DriverHost, HostDriver};
use orch_types::{Hash, new_id};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Fingerprints {
    reads: Mutex<Vec<String>>,
}

#[async_trait]
impl DriverExtras for Fingerprints {
    async fn fingerprint_volume(&self, volume: &VolumeRequest) -> Result<Hash, DriverError> {
        self.reads.lock().unwrap().push(volume.name.clone());
        Ok(Hash::of(b"same contents"))
    }
    async fn fingerprint_directory(
        &self,
        directory: &DirectoryBinding,
    ) -> Result<Hash, DriverError> {
        if cfg!(target_os = "linux") {
            assert!(directory.device.is_some());
        }
        self.reads.lock().unwrap().push(directory.path.clone());
        Ok(Hash::of(b"same contents"))
    }
}

#[tokio::test]
async fn the_command_checks_its_volume_name_and_directory_root_before_reading_content() {
    let mut rig = Rig::new();
    let extras = Arc::new(Fingerprints::default());
    let drivers = DriverHost::of(
        &rig.settings,
        rig.desk.clone(),
        vec![HostDriver::new(rig.driver.clone()).with_extras(extras.clone())],
    );
    rig.start_with(drivers).await;
    let mount_id = new_id();
    let mut request = NodeMountFingerprint {
        work: None,
        driver: "memory".into(),
        mount_id,
        data: MountData::Volume {
            name: format!("orch-mount-{mount_id}"),
            storage: None,
        },
    };
    let reply = rig.succeed(&SerialKey::None, &request).await;
    assert_eq!(
        reply["fingerprint"],
        serde_json::json!(Hash::of(b"same contents"))
    );
    let directory = tempfile::tempdir().unwrap();
    request.data = MountData::Path {
        path: directory.path().to_string_lossy().into_owned(),
    };
    assert_eq!(rig.succeed(&SerialKey::None, &request).await, reply);
    assert_eq!(extras.reads.lock().unwrap().len(), 2);
    request.data = MountData::Volume {
        name: format!("orch-mount-{}", new_id()),
        storage: None,
    };
    assert_eq!(
        rig.refused(&SerialKey::None, &request).await.code,
        "conflict"
    );
    let temporary_root = std::env::temp_dir();
    request.data = MountData::Path {
        path: temporary_root
            .ancestors()
            .last()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    };
    let failure = rig.refused(&SerialKey::None, &request).await;
    assert_eq!(failure.code, "policy.root_not_allowed");
    assert_eq!(
        extras.reads.lock().unwrap().len(),
        2,
        "neither refused location reaches the helper"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_driver_without_content_checks_reports_unsupported_instead_of_a_matching_fingerprint() {
    let mut rig = Rig::new();
    rig.start().await;
    let mount_id = new_id();
    let failure = rig
        .refused(
            &SerialKey::None,
            &NodeMountFingerprint {
                work: None,
                driver: "memory".into(),
                mount_id,
                data: MountData::Volume {
                    name: format!("orch-mount-{mount_id}"),
                    storage: None,
                },
            },
        )
        .await;
    assert_eq!(failure.code, "unsupported");
    rig.stop().await;
}

#[tokio::test]
async fn a_fingerprint_reports_the_declared_command_and_joins_its_read_before_cancel_finishes() {
    use orch_driver_contract::volumes::{VolumeProgress, VolumeWork};
    use orch_link::commands::Progress;
    use orch_link_contract::commands::WorkReport;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Default)]
    struct Reading {
        began: AtomicBool,
        stopped: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
    }
    struct Held {
        work: Arc<dyn VolumeWork>,
        stopped: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
    }
    impl Drop for Held {
        fn drop(&mut self) {
            self.cancelled
                .store(self.work.is_cancelled(), Ordering::SeqCst);
            self.stopped.store(true, Ordering::SeqCst);
        }
    }
    #[async_trait]
    impl DriverExtras for Reading {
        async fn fingerprint_volume_reporting(
            &self,
            _volume: &VolumeRequest,
            work: &Arc<dyn VolumeWork>,
        ) -> Result<Hash, DriverError> {
            let _held = Held {
                work: Arc::clone(work),
                stopped: Arc::clone(&self.stopped),
                cancelled: Arc::clone(&self.cancelled),
            };
            work.report(VolumeProgress {
                files_done: 7,
                bytes_done: 4096,
                directories_done: Some(2),
                ..Default::default()
            });
            self.began.store(true, Ordering::SeqCst);
            work.cancelled().await;
            Err(DriverError::Unavailable(
                "the content read was stopped".into(),
            ))
        }
    }
    let mut rig = Rig::new();
    let reading = Arc::new(Reading::default());
    let drivers = DriverHost::of(
        &rig.settings,
        rig.desk.clone(),
        vec![HostDriver::new(rig.driver.clone()).with_extras(reading.clone())],
    );
    rig.start_with(drivers).await;
    let mount = new_id();
    let job = new_id();
    let body = NodeMountFingerprint {
        driver: "memory".into(),
        mount_id: mount,
        data: MountData::Volume {
            name: format!("orch-mount-{mount}"),
            storage: None,
        },
        work: Some(WorkReport {
            files_total: None,
            bytes_total: None,
            job_id: job,
            phase: "fingerprint_source".into(),
            timeout_secs: 30,
        }),
    };
    let command = rig
        .controller
        .issue(&SerialKey::None, rig.deadline(), &body);
    rig.until("a fingerprint read to begin", || {
        reading.began.load(Ordering::SeqCst)
    })
    .await;
    rig.wait_told("fingerprint counters on the command's live job", |events| {
        events.iter().any(|event| {
            event.kind == "job.progress"
                && event.body["command_id"] == serde_json::json!(command)
                && event.body["job_id"] == serde_json::json!(job)
                && event.body["progress"]["bytes_done"] == 4096
                && event.body["progress"]["phase"] == "fingerprint_source"
        })
    })
    .await;
    rig.controller.link().commands().cancel(command);
    let outcome = tokio::time::timeout(common::PATIENCE, rig.controller.wait_command(command))
        .await
        .unwrap();
    assert!(matches!(outcome, Progress::Cancelled), "{outcome:?}");
    rig.until("the machine's actual stop acknowledgement", || {
        rig.controller.link().commands().answered(command)
    })
    .await;
    assert!(
        reading.stopped.load(Ordering::SeqCst),
        "cancel acknowledged before the read ended"
    );
    assert!(
        reading.cancelled.load(Ordering::SeqCst),
        "the driver did not receive the command cancellation"
    );
    rig.stop().await;
}
