//! Live mount replies, restart replay and recreate fallback over the node's actual command boundary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use async_trait::async_trait;
use common::Rig;
use orch_driver_contract::descriptor::DriverDescriptor;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::mounts::{
    AttachedMount, MountChange, MountKind, MountMode, MountPlacement, MountSpec,
};
use orch_driver_contract::runtime::{RuntimeKind, RuntimeSpec};
use orch_driver_contract::units::{UnitRef, UnitState, UnitStateKind};
use orch_driver_contract::volumes::VolumeRequest;
use orch_link_contract::commands::{NodeMountsApply, SerialKey};
use orch_node_core::drivers::{DriverHost, HostDriver};
use orch_testkit::doubles::driver::MemoryDriver;
use orch_types::Time;
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Namespace {
    actual: Vec<MountSpec>,
    accepted: Vec<MountChange>,
    fail: bool,
    permanent: bool,
    attempts: usize,
}

/// An ordinary in-memory unit with a mount namespace that is lost only when the unit starts again.
struct LiveDriver {
    memory: Arc<MemoryDriver>,
    namespace: Arc<Mutex<Namespace>>,
}

#[async_trait]
impl EnvironmentDriver for LiveDriver {
    fn describe(&self) -> DriverDescriptor {
        let mut described = self.memory.describe();
        described.live_changes.mounts = true;
        // A private view is a layer over a mount placed at its target: a driver that offers views places mounts.
        described.mounts = MountPlacement::Placed;
        described.private_views = true;
        described
    }

    async fn create(&self, spec: &RuntimeSpec) -> Result<UnitRef, DriverError> {
        let first = self.memory.unit(&spec.name).is_none();
        let unit = self.memory.create(spec).await?;
        if first {
            self.namespace.lock().unwrap().actual = spec.mounts.clone();
        }
        Ok(unit)
    }

    async fn start(&self, unit: &UnitRef) -> Result<(), DriverError> {
        if self.memory.state_of(&unit.name) != UnitStateKind::Running {
            self.memory.start(unit).await?;
            self.namespace.lock().unwrap().actual =
                self.memory.unit(&unit.name).unwrap().spec.mounts;
        }
        Ok(())
    }

    async fn stop(&self, unit: &UnitRef, grace: Duration) -> Result<(), DriverError> {
        self.memory.stop(unit, grace).await
    }

    async fn remove(&self, unit: &UnitRef) -> Result<(), DriverError> {
        self.memory.remove(unit).await
    }

    async fn inspect(&self, unit: &UnitRef) -> Result<UnitState, DriverError> {
        let mut state = self.memory.inspect(unit).await?;
        if state.state == UnitStateKind::Running {
            let starts = self.memory.unit(&unit.name).unwrap().starts;
            // A deterministic distinct identity per namespace, independent of the node's clock.
            state.started_at = Some(Time::new(
                chrono::DateTime::from_timestamp(i64::from(starts), 0).unwrap(),
            ));
        }
        Ok(state)
    }

    async fn list(&self, system: &str) -> Result<Vec<UnitRef>, DriverError> {
        self.memory.list(system).await
    }

    async fn ensure_volume(&self, volume: &VolumeRequest) -> Result<(), DriverError> {
        self.memory.ensure_volume(volume).await
    }

    async fn remove_volume(&self, name: &str) -> Result<(), DriverError> {
        self.memory.remove_volume(name).await
    }

    async fn apply_mounts(
        &self,
        _unit: &UnitRef,
        change: &MountChange,
    ) -> Result<Vec<AttachedMount>, DriverError> {
        let mut namespace = self.namespace.lock().unwrap();
        namespace.attempts += 1;
        if namespace.permanent {
            return Err(DriverError::Unsupported(
                "the kernel cannot safely apply this mount".into(),
            ));
        }
        if namespace.fail {
            return Err(DriverError::Unavailable(
                "the helper is unavailable on this host".into(),
            ));
        }
        if change.before != namespace.actual {
            return Err(DriverError::InvalidSpec(
                "the caller used an old namespace baseline".into(),
            ));
        }
        namespace.actual = change.after.clone();
        namespace.accepted.push(change.clone());
        Ok(change
            .after
            .iter()
            .map(|mount| AttachedMount {
                name: mount.name.clone(),
                path: mount.target.clone(),
                mode: mount.mode,
            })
            .collect())
    }
}

fn mount() -> MountSpec {
    MountSpec {
        name: "data".into(),
        kind: MountKind::Volume,
        source: "orch-test-live-data".into(),
        target: "/data/project".into(),
        mode: MountMode::ReadOnly,
        layer: None,
    }
}

fn drivers(rig: &Rig, live: Arc<LiveDriver>) -> Arc<DriverHost> {
    let contract: Arc<dyn EnvironmentDriver> = live;
    DriverHost::of(
        &rig.settings,
        Arc::clone(&rig.desk),
        vec![HostDriver::new(contract).with_extras(rig.extras.clone())],
    )
}

#[tokio::test]
async fn a_node_restart_keeps_the_accepted_set_and_a_unit_restart_replays_the_desired_set() {
    let mut rig = Rig::new();
    rig.have_connector();
    let namespace = Arc::new(Mutex::new(Namespace::default()));
    let live = Arc::new(LiveDriver {
        memory: Arc::clone(&rig.driver),
        namespace: Arc::clone(&namespace),
    });
    rig.start_with(drivers(&rig, Arc::clone(&live))).await;
    let spec = rig.spec(RuntimeKind::Environment, "live-replay");
    let unit = rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;
    let desired = vec![mount()];
    let request = NodeMountsApply {
        runtime_id: spec.runtime_id,
        mounts: json!(desired),
    };
    let reply = rig
        .succeed(&SerialKey::Runtime(spec.runtime_id), &request)
        .await;
    assert_eq!(reply["applied"], true);
    assert_eq!(reply["mounts"][0]["mode"], "read_only");
    assert!(reply["reason"].is_null());
    assert_eq!(rig.driver.unit(&spec.name).unwrap().starts, 1);
    rig.kill().await;
    // A new driver object has no in-process mount cache, just the surviving unit and node ledger.
    let live = Arc::new(LiveDriver {
        memory: Arc::clone(&rig.driver),
        namespace: Arc::clone(&namespace),
    });
    rig.start_with(drivers(&rig, Arc::clone(&live))).await;
    rig.role().runtimes().reconcile_live_mounts().await;
    assert_eq!(
        namespace.lock().unwrap().accepted.len(),
        1,
        "an unchanged namespace is not mounted twice"
    );
    let detach = NodeMountsApply {
        runtime_id: spec.runtime_id,
        mounts: json!([]),
    };
    assert_eq!(
        rig.succeed(&SerialKey::Runtime(spec.runtime_id), &detach)
            .await["applied"],
        true
    );
    assert_eq!(
        namespace.lock().unwrap().accepted.last().unwrap().before,
        desired
    );
    assert!(namespace.lock().unwrap().actual.is_empty());
    rig.succeed(&SerialKey::Runtime(spec.runtime_id), &request)
        .await;
    live.stop(&unit, Duration::ZERO).await.unwrap();
    live.start(&unit).await.unwrap(); // A Docker/daemon-driven restart, not node.runtime.start.
    assert!(namespace.lock().unwrap().actual.is_empty());
    rig.role().runtimes().reconcile_live_mounts().await;
    assert_eq!(namespace.lock().unwrap().actual, desired);
    assert!(
        namespace
            .lock()
            .unwrap()
            .accepted
            .last()
            .unwrap()
            .before
            .is_empty()
    );
    let accepted = namespace.lock().unwrap().accepted.len();
    rig.role().runtimes().reconcile_live_mounts().await;
    assert_eq!(namespace.lock().unwrap().accepted.len(), accepted);
    assert_eq!(rig.driver.unit(&spec.name).unwrap().unit, unit);
    assert_eq!(rig.driver.unit(&spec.name).unwrap().starts, 2);
    rig.stop().await;
}

#[tokio::test]
async fn a_stopped_units_approved_mount_set_survives_node_restart_and_applies_at_start() {
    let mut rig = Rig::new();
    rig.have_connector();
    let namespace = Arc::new(Mutex::new(Namespace::default()));
    let live = Arc::new(LiveDriver {
        memory: Arc::clone(&rig.driver),
        namespace: Arc::clone(&namespace),
    });
    rig.start_with(drivers(&rig, Arc::clone(&live))).await;
    let spec = rig.spec(RuntimeKind::Environment, "pending-start");
    rig.made(&spec).await;
    let desired = vec![mount()];
    let request = NodeMountsApply {
        runtime_id: spec.runtime_id,
        mounts: json!(desired),
    };
    let reply = rig
        .succeed(&SerialKey::Runtime(spec.runtime_id), &request)
        .await;
    assert_eq!(reply["applied"], false);
    assert!(reply["reason"].as_str().unwrap().contains("next start"));
    assert!(namespace.lock().unwrap().actual.is_empty());
    rig.kill().await;
    rig.start_with(drivers(&rig, Arc::clone(&live))).await;
    rig.start_runtime(spec.runtime_id).await;
    assert_eq!(namespace.lock().unwrap().actual, desired);
    assert_eq!(rig.driver.unit(&spec.name).unwrap().starts, 1);
    let stored = rig.role().ledger().mounts(spec.runtime_id).unwrap();
    assert_eq!(stored.applied, desired);
    assert!(stored.applied_started_at.is_some());
    rig.stop().await;
}

#[tokio::test]
async fn unsupported_live_mounts_explain_recreate_and_leave_the_owned_unit_running() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "unsupported-live");
    let unit = rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;
    let desired = vec![mount()];
    let request = NodeMountsApply {
        runtime_id: spec.runtime_id,
        mounts: json!(desired),
    };
    let reply = rig
        .succeed(&SerialKey::Runtime(spec.runtime_id), &request)
        .await;
    assert_eq!(reply["applied"], false);
    assert!(reply["reason"].as_str().unwrap().contains("recreate"));
    assert_eq!(rig.driver.unit(&spec.name).unwrap().unit, unit);
    assert_eq!(rig.driver.unit(&spec.name).unwrap().starts, 1);
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
    assert_eq!(
        rig.role().ledger().mounts(spec.runtime_id).unwrap().wanted,
        desired
    );
    assert!(
        rig.role()
            .ledger()
            .mounts(spec.runtime_id)
            .unwrap()
            .pending_reason
            .as_deref()
            .unwrap()
            .contains("recreate")
    );
    rig.until("the pending mount reason on the runtime stream", || {
        rig.states_of(spec.runtime_id).iter().any(|(_, detail)| {
            detail.as_deref().is_some_and(|detail| {
                detail.contains("mount changes pending") && detail.contains("recreate")
            })
        })
    })
    .await;
    rig.stop().await;
}

#[tokio::test]
async fn a_failed_helper_keeps_the_change_pending_until_a_later_safe_reconciliation() {
    let mut rig = Rig::new();
    rig.have_connector();
    let namespace = Arc::new(Mutex::new(Namespace {
        fail: true,
        ..Namespace::default()
    }));
    let live = Arc::new(LiveDriver {
        memory: Arc::clone(&rig.driver),
        namespace: Arc::clone(&namespace),
    });
    rig.start_with(drivers(&rig, live)).await;
    let spec = rig.spec(RuntimeKind::Environment, "failed-live");
    let unit = rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;
    let desired = vec![mount()];
    let request = NodeMountsApply {
        runtime_id: spec.runtime_id,
        mounts: json!(desired),
    };
    let reply = rig
        .succeed(&SerialKey::Runtime(spec.runtime_id), &request)
        .await;
    assert_eq!(reply["applied"], false);
    assert!(
        reply["reason"]
            .as_str()
            .unwrap()
            .contains("helper is unavailable")
    );
    assert!(reply["reason"].as_str().unwrap().contains("recreate"));
    assert!(namespace.lock().unwrap().actual.is_empty());
    assert!(
        rig.role()
            .ledger()
            .mounts(spec.runtime_id)
            .unwrap()
            .applied
            .is_empty()
    );
    assert!(
        rig.role()
            .ledger()
            .mounts(spec.runtime_id)
            .unwrap()
            .pending_reason
            .is_some()
    );
    namespace.lock().unwrap().fail = false;
    rig.role().runtimes().reconcile_live_mounts().await;
    assert_eq!(namespace.lock().unwrap().actual, desired);
    assert!(
        rig.role()
            .ledger()
            .mounts(spec.runtime_id)
            .unwrap()
            .pending_reason
            .is_none()
    );
    assert_eq!(rig.driver.unit(&spec.name).unwrap().unit, unit);
    assert_eq!(rig.driver.unit(&spec.name).unwrap().starts, 1);
    rig.stop().await;
}

#[tokio::test]
async fn a_private_layer_created_live_is_owned_and_cannot_be_deleted_under_a_running_unit() {
    use orch_link_contract::commands::{NodeVolumeEnsure, NodeVolumeRemove};
    let mut rig = Rig::new();
    rig.have_connector();
    let live = Arc::new(LiveDriver {
        memory: Arc::clone(&rig.driver),
        namespace: Arc::new(Mutex::new(Namespace::default())),
    });
    rig.start_with(drivers(&rig, live)).await;
    let spec = rig.spec(RuntimeKind::Environment, "private-live-owner");
    rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;
    let layer = format!("orch-over-{}-data", spec.runtime_id);
    rig.succeed(
        &SerialKey::Volume(layer.clone()),
        &NodeVolumeEnsure {
            driver: "memory".into(),
            name: layer.clone(),
            storage: None,
            labels: Default::default(),
        },
    )
    .await;
    let private = MountSpec {
        mode: MountMode::Private,
        layer: Some(orch_driver_contract::mounts::MountLayer {
            name: layer.clone(),
            storage: None,
        }),
        ..mount()
    };
    rig.succeed(
        &SerialKey::Runtime(spec.runtime_id),
        &NodeMountsApply {
            runtime_id: spec.runtime_id,
            mounts: json!([private]),
        },
    )
    .await;
    assert!(
        rig.role()
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .volumes
            .contains(&layer)
    );
    let error = rig
        .refused(
            &SerialKey::Volume(layer.clone()),
            &NodeVolumeRemove {
                driver: "memory".into(),
                name: layer.clone(),
                storage: None,
                labels: Default::default(),
            },
        )
        .await;
    assert_eq!(error.code, "conflict");
    assert!(
        rig.driver
            .volumes()
            .iter()
            .any(|volume| volume.name == layer)
    );
    rig.succeed(
        &SerialKey::Runtime(spec.runtime_id),
        &NodeMountsApply {
            runtime_id: spec.runtime_id,
            mounts: json!([]),
        },
    )
    .await;
    assert!(
        rig.role()
            .ledger()
            .get(spec.runtime_id)
            .unwrap()
            .volumes
            .contains(&layer),
        "detached private changes remain owned environment data"
    );
    rig.stop().await;
}

#[tokio::test]
async fn reverting_a_failed_edit_cancels_its_retained_desire_before_reconciliation_can_apply_it() {
    let mut rig = Rig::new();
    rig.have_connector();
    let namespace = Arc::new(Mutex::new(Namespace::default()));
    let live = Arc::new(LiveDriver {
        memory: Arc::clone(&rig.driver),
        namespace: Arc::clone(&namespace),
    });
    rig.start_with(drivers(&rig, live)).await;
    let spec = rig.spec(RuntimeKind::Environment, "reverted-live-edit");
    rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;
    let original = NodeMountsApply {
        runtime_id: spec.runtime_id,
        mounts: json!([mount()]),
    };
    rig.succeed(&SerialKey::Runtime(spec.runtime_id), &original)
        .await;
    namespace.lock().unwrap().fail = true;
    let abandoned = MountSpec {
        target: "/data/abandoned".into(),
        ..mount()
    };
    let failed = rig
        .succeed(
            &SerialKey::Runtime(spec.runtime_id),
            &NodeMountsApply {
                runtime_id: spec.runtime_id,
                mounts: json!([abandoned.clone()]),
            },
        )
        .await;
    assert_eq!(failed["applied"], false);
    assert_eq!(
        rig.role().ledger().mounts(spec.runtime_id).unwrap().wanted,
        [abandoned]
    );
    assert_eq!(
        rig.succeed(&SerialKey::Runtime(spec.runtime_id), &original)
            .await["applied"],
        true
    );
    namespace.lock().unwrap().fail = false;
    rig.role().runtimes().reconcile_live_mounts().await;
    assert_eq!(namespace.lock().unwrap().actual, [mount()]);
    assert_eq!(namespace.lock().unwrap().accepted.len(), 1);
    assert_eq!(
        rig.role().ledger().mounts(spec.runtime_id).unwrap().wanted,
        [mount()]
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_permanent_refusal_waits_for_a_new_namespace_instead_of_running_a_helper_each_tick() {
    let mut rig = Rig::new();
    rig.have_connector();
    let namespace = Arc::new(Mutex::new(Namespace {
        permanent: true,
        ..Namespace::default()
    }));
    let live = Arc::new(LiveDriver {
        memory: Arc::clone(&rig.driver),
        namespace: Arc::clone(&namespace),
    });
    rig.start_with(drivers(&rig, Arc::clone(&live))).await;
    let spec = rig.spec(RuntimeKind::Environment, "permanent-live-refusal");
    let unit = rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;
    let request = NodeMountsApply {
        runtime_id: spec.runtime_id,
        mounts: json!([mount()]),
    };
    assert_eq!(
        rig.succeed(&SerialKey::Runtime(spec.runtime_id), &request)
            .await["applied"],
        false
    );
    let attempts = namespace.lock().unwrap().attempts;
    rig.role().runtimes().reconcile_live_mounts().await;
    rig.role().runtimes().reconcile_live_mounts().await;
    assert_eq!(namespace.lock().unwrap().attempts, attempts);
    namespace.lock().unwrap().permanent = false;
    live.stop(&unit, Duration::ZERO).await.unwrap();
    live.start(&unit).await.unwrap();
    rig.role().runtimes().reconcile_live_mounts().await;
    assert_eq!(namespace.lock().unwrap().actual, [mount()]);
    assert!(
        rig.role()
            .ledger()
            .mounts(spec.runtime_id)
            .unwrap()
            .pending_reason
            .is_none()
    );
    rig.stop().await;
}

#[tokio::test]
async fn an_interrupted_unacknowledged_namespace_is_stopped_and_reported_without_losing_the_owned_unit()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    let namespace = Arc::new(Mutex::new(Namespace::default()));
    let live = Arc::new(LiveDriver {
        memory: Arc::clone(&rig.driver),
        namespace: Arc::clone(&namespace),
    });
    rig.start_with(drivers(&rig, Arc::clone(&live))).await;
    let spec = rig.spec(RuntimeKind::Environment, "interrupted-live-change");
    let unit = rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;
    let role = rig.role();
    let mut configuration = role.ledger().mounts(spec.runtime_id).unwrap();
    configuration.wanted = vec![mount()];
    role.ledger()
        .set_mounts(spec.runtime_id, configuration)
        .unwrap();
    // Model a kill after the durable intent and some effects, before the complete acknowledgment.
    namespace.lock().unwrap().actual = vec![mount()];
    rig.kill().await;
    rig.start_with(drivers(&rig, live)).await;
    rig.role().runtimes().reconcile_live_mounts().await;
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);
    assert_eq!(rig.driver.unit(&spec.name).unwrap().unit, unit);
    assert_eq!(rig.driver.unit(&spec.name).unwrap().starts, 1);
    assert!(
        rig.role()
            .ledger()
            .mounts(spec.runtime_id)
            .unwrap()
            .pending_reason
            .as_deref()
            .unwrap()
            .contains("interrupted")
    );
    rig.start_runtime(spec.runtime_id).await;
    assert_eq!(namespace.lock().unwrap().actual, [mount()]);
    assert_eq!(rig.driver.unit(&spec.name).unwrap().unit, unit);
    rig.stop().await;
}
