//! Driver operations and errors.
//!
//! `orch-node` has its own trait for provisioning, [`EnvironmentDriver`]. It loads the drivers its
//! configuration names, each either built in or an external program, and treats all of them through this
//! contract. A driver makes units (a container, a process group, a microVM), keeps their volumes, makes
//! whatever a spec names as its source available on the host, and may propose a spec from something a person
//! already has.
//!
//! | Method | Does |
//! |--------|------|
//! | `describe` | Its declaration; what the host can offer is filled in at start |
//! | `validate` | Checks a spec beyond its schema (optional) |
//! | `create` | Makes a unit, idempotently by name |
//! | `start`, `stop`, `remove` | Lifecycle |
//! | `inspect` | `running`, `stopped`, `absent`, `failed`, with detail |
//! | `list` | Every unit of a system, for reconciliation |
//! | `stats` | Resource use (optional) |
//! | `ensure_volume`, `remove_volume` | Persistent storage for workspace, home and state |
//! | `ensure_image` | Makes a spec's source available on the host; a no-op for a driver without one (optional) |
//! | `import` | Proposes a spec from something a person has (optional); it creates nothing |
//! | `apply_network` | Puts a unit's network policy in force and says what is; `restricted` only where enforced |
//! | `apply_mounts` | Applies the approved complete mount set live where declared; otherwise remains pending for recreate |
//!
//! The unit a driver creates must end up running `orch-connector run --managed` with the variables of
//! [`crate::runtime::LAUNCH_VARIABLES`]. How it gets there is the driver's business. A method a driver does
//! not implement is answered with [`DriverError::Unsupported`] and what depends on it is shown as
//! unavailable; the defaults below do exactly that, or the nothing that is right for a driver without the
//! thing.

use std::time::Duration;

use async_trait::async_trait;
use orch_types::Json;

use crate::descriptor::DriverDescriptor;
use crate::imports::{ImportProposal, ImportRequest};
use crate::network::{EgressMode, NetworkPolicy, UnitNetwork};
use crate::runtime::{RuntimeKind, RuntimeSpec};
use crate::sources::{ImageEnsured, ImageProgress};
use crate::units::{ResourceSample, UnitRef, UnitState};
use crate::volumes::{StorageInfo, VolumeRequest};

/// Why an operation of a driver did not succeed. The codes are those a command's result uses.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DriverError {
    /// An overlapping operation owns the unit; the caller may retry after it finishes.
    #[error("busy: {0}")]
    Busy(String),
    /// The host cannot satisfy the request's memory, process or storage requirements.
    #[error("no capacity: {0}")]
    NoCapacity(String),
    /// The driver does not do that; what depends on it is shown as unavailable.
    #[error("not supported: {0}")]
    Unsupported(String),
    /// The spec is not one the driver can make a unit from.
    #[error("invalid spec: {0}")]
    InvalidSpec(String),
    /// There is no such unit, volume or image.
    #[error("not found: {0}")]
    NotFound(String),
    /// What the driver runs on is not there or does not answer: the daemon, the hypervisor.
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// An image could not be pulled, built or loaded.
    #[error("image: {0}")]
    Image(String),
    /// It took longer than its deadline.
    #[error("timed out")]
    Timeout,
    /// The driver failed in a way that is its own.
    #[error("internal error: {0}")]
    Internal(String),
}

impl DriverError {
    /// The code, as a command's result states it.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Busy(_) => "busy",
            Self::NoCapacity(_) => "no_capacity",
            Self::Unsupported(_) => "unsupported",
            Self::InvalidSpec(_) => "invalid_spec",
            Self::NotFound(_) => "not_found",
            Self::Unavailable(_) => "unavailable",
            Self::Image(_) => "image_failed",
            Self::Timeout => "timeout",
            Self::Internal(_) => "internal",
        }
    }

    /// Whether trying again may help.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(self, Self::Busy(_) | Self::Timeout | Self::Unavailable(_))
    }
}

/// One implementation per driver, used by `orch-node`. It is shared by every command the node is running,
/// so its methods take `&self`.
#[async_trait]
pub trait EnvironmentDriver: Send + Sync + 'static {
    /// Lists only this installation's owned objects, their sizes and their complete dependencies.
    async fn resources(
        &self,
        _system: &str,
    ) -> Result<Vec<crate::resources::Resource>, DriverError> {
        Err(DriverError::Unsupported("resource inventory".into()))
    }

    /// Rechecks ownership and dependencies before removing exactly one inventoried object, never pruning.
    async fn remove_resource(
        &self,
        _system: &str,
        _resource: &crate::resources::ResourceRef,
    ) -> Result<(), DriverError> {
        Err(DriverError::Unsupported("resource removal".into()))
    }

    /// Stops an owned unit selected by a deliberate purge, including helpers a driver may hold.
    async fn stop_resource(
        &self,
        system: &str,
        resource: &crate::resources::ResourceRef,
        grace: Duration,
    ) -> Result<(), DriverError> {
        if resource.kind != crate::resources::ResourceKind::Unit {
            return Err(DriverError::InvalidSpec(
                "only a unit can be stopped".into(),
            ));
        }
        let unit = self
            .list(system)
            .await?
            .into_iter()
            .find(|unit| unit.reference == resource.id)
            .ok_or_else(|| DriverError::NotFound("the owned unit is absent".into()))?;
        self.stop(&unit, grace).await
    }

    /// Verifies complete owned-volume archive import before the controller promises a carry.
    async fn check_volume_carry(&self) -> Result<(), DriverError> {
        Err(DriverError::Unsupported(
            "carrying whole owned volumes".into(),
        ))
    }

    /// Measures the selected owned scope, without interpreting its storage name as a host path.
    async fn measure_volume(
        &self,
        _volume: &VolumeRequest,
        _selector: crate::volumes::OwnedVolumeSelector,
        _work: &std::sync::Arc<dyn crate::volumes::VolumeWork>,
    ) -> Result<crate::volumes::VolumeMeasure, DriverError> {
        Err(DriverError::Unsupported("measuring owned volumes".into()))
    }

    /// Writes a metadata-preserving archive to node-owned staging, which is never a volume location.
    async fn export_volume(
        &self,
        _volume: &VolumeRequest,
        _selector: crate::volumes::OwnedVolumeSelector,
        _into: &std::path::Path,
        _work: &std::sync::Arc<dyn crate::volumes::VolumeWork>,
    ) -> Result<(), DriverError> {
        Err(DriverError::Unsupported("exporting owned volumes".into()))
    }

    /// Imports a verified staged archive before any unit uses the selected owned scope.
    /// Calls `work.input_ready()` after opening its endpoint and before consuming the stream.
    async fn import_volume(
        &self,
        _volume: &VolumeRequest,
        _selector: crate::volumes::OwnedVolumeSelector,
        _from: &std::path::Path,
        _work: &std::sync::Arc<dyn crate::volumes::VolumeWork>,
    ) -> Result<(), DriverError> {
        Err(DriverError::Unsupported("importing owned volumes".into()))
    }

    /// Ends durable helpers for one exact interrupted command before its terminal acknowledgement.
    async fn stop_volume_work(&self, _command: orch_types::Id) -> Result<(), DriverError> {
        Ok(())
    }

    /// Aggregate resources of a driver-managed host; local measurements never replace a missing answer.
    fn host_resources(&self) -> Result<crate::host::HostResources, DriverError> {
        Err(DriverError::Unsupported(
            "host resource observations".into(),
        ))
    }

    /// Checks configuration against the driver's declared schema before it is activated.
    fn validate_configuration(&self, configuration: &Json) -> Result<(), DriverError> {
        if configuration
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
        {
            Ok(())
        } else {
            Err(DriverError::Unsupported(
                "driver configuration validation".into(),
            ))
        }
    }

    /// Estimates installed files to keep before an unavoidable replacement, without stopping work.
    async fn estimate_keep(
        &self,
        _unit: &UnitRef,
    ) -> Result<crate::kept::KeepEstimate, DriverError> {
        Err(DriverError::Unsupported("estimating a kept image".into()))
    }
    /// Records a stopped owned unit's file system locally, idempotently by environment and generation.
    async fn keep_files(
        &self,
        _unit: &UnitRef,
        _environment: orch_types::Id,
        _generation: i64,
        _capture: orch_types::Id,
    ) -> Result<crate::kept::KeptImage, DriverError> {
        Err(DriverError::Unsupported(
            "keeping installed files in an image".into(),
        ))
    }
    /// Owned kept images, including images retained for an archived environment with no unit.
    async fn kept_images(&self, _system: &str) -> Result<Vec<crate::kept::KeptImage>, DriverError> {
        Ok(Vec::new())
    }
    /// Drops only unused owned dangling metadata, retaining the controller's archived/job owners.
    async fn sweep_kept_metadata(
        &self,
        _system: &str,
        _owners: &[crate::kept::KeptOwner],
    ) -> Result<usize, DriverError> {
        Ok(0)
    }
    /// Removes exactly one verified owned name; existing containers/later images retain referenced layers.
    async fn remove_kept_image(&self, _image: &crate::kept::KeptImage) -> Result<(), DriverError> {
        Err(DriverError::Unsupported("removing a kept image".into()))
    }
    /// Streams an owned image archive to a node-owned file for the existing content transfer.
    async fn export_kept_image(
        &self,
        _image: &crate::kept::KeptImage,
        _into: &std::path::Path,
    ) -> Result<(), DriverError> {
        Err(DriverError::Unsupported("moving a kept image".into()))
    }
    /// Loads the verified image archive into this daemon, preserving its environment/generation labels.
    async fn import_kept_image(
        &self,
        _image: &crate::kept::KeptImage,
        _from: &std::path::Path,
    ) -> Result<(), DriverError> {
        Err(DriverError::Unsupported("moving a kept image".into()))
    }
    /// Applies declared limits to the owned running unit without replacing or restarting it.
    async fn apply_resources(
        &self,
        _unit: &UnitRef,
        _resources: &crate::runtime::Resources,
    ) -> Result<(), DriverError> {
        Err(DriverError::Unsupported("live resource changes".into()))
    }

    /// Applies grant changes within the prepared boundary; true means withdrawal needs a restart of this same unit.
    async fn apply_capabilities(
        &self,
        _unit: &UnitRef,
        before: &crate::capabilities::CapabilityState,
        after: &crate::capabilities::CapabilityState,
    ) -> Result<bool, DriverError> {
        let native = |state: &crate::capabilities::CapabilityState| {
            state
                .granted
                .iter()
                .filter(|key| !crate::capabilities::gateway_given(key))
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
        };
        if native(before) == native(after) {
            Ok(false)
        } else {
            Err(DriverError::Unsupported("live capability changes".into()))
        }
    }

    /// Starts with current grants, replaying declared live operations before reporting readiness.
    async fn start_with_capabilities(
        &self,
        unit: &UnitRef,
        state: &crate::capabilities::CapabilityState,
    ) -> Result<(), DriverError> {
        self.start(unit).await?;
        let mut before = state.clone();
        let live = self.describe().live_changes.capabilities;
        before.granted.retain(|key| !live.contains(key));
        self.apply_capabilities(unit, &before, state)
            .await
            .map(|_| ())
    }
    /// Named host storages and their actual sizes; absent when the driver cannot report them.
    fn storages(&self) -> Option<Vec<StorageInfo>> {
        None
    }

    /// Verifies owned retained storage without creating a replacement behind a restore request.
    async fn volume_exists(&self, _volume: &VolumeRequest) -> Result<bool, DriverError> {
        Err(DriverError::Unsupported("verifying kept volumes".into()))
    }

    /// Reads a volume's protected ownership without making or changing it.
    async fn volume_identity(
        &self,
        _volume: &VolumeRequest,
    ) -> Result<Option<crate::volumes::VolumeIdentity>, DriverError> {
        Err(DriverError::Unsupported(
            "inspecting restored volume ownership".into(),
        ))
    }

    /// Holds this unit's processes for a consistent transfer only when the descriptor declares suspend.
    async fn suspend(&self, _unit: &UnitRef) -> Result<(), DriverError> {
        Err(DriverError::Unsupported("suspend".into()))
    }

    /// Releases the processes previously held by suspend.
    async fn resume(&self, _unit: &UnitRef) -> Result<(), DriverError> {
        Err(DriverError::Unsupported("suspend".into()))
    }

    /// Ensures an image and reports long work through the same contract for built-in and external drivers.
    async fn ensure_image_reporting(
        &self,
        spec: &Json,
        _progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        self.ensure_image(spec).await
    }

    /// Rebuilds a source with current bases; drivers without a separate rebuild ensure their source.
    async fn rebuild_image(
        &self,
        spec: &Json,
        progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        self.ensure_image_reporting(spec, progress).await
    }

    /// Looks up a tag's current registry digest without altering units or pulling it.
    async fn registry_digest(&self, _reference: &str) -> Result<Option<String>, DriverError> {
        Ok(None)
    }

    /// Closes outward traffic when the authenticated node link is lost; a reconnect reconciles policies first.
    fn control_link(&self, _connected: bool) {}

    /// The authenticated addresses of this host's control service, kept reachable by restricted units.
    fn control_endpoints(&self, _addresses: Vec<String>) {}

    /// Reads current network state without installing or renewing a policy from a previous link.
    async fn inspect_network(&self, _unit: &UnitRef) -> Result<UnitNetwork, DriverError> {
        Err(DriverError::Unsupported("network observation".to_owned()))
    }

    /// Its declaration. What the host can offer (the sources it builds, the runtime classes it has) is
    /// filled in when the driver starts.
    fn describe(&self) -> DriverDescriptor;

    /// Checks a spec beyond its schema, for an environment or for a provider's runtime.
    async fn validate(&self, kind: RuntimeKind, spec: &Json) -> Result<(), DriverError> {
        let _ = (kind, spec);
        Ok(())
    }

    /// Makes a unit (a container, a process group, a microVM) for `spec`, idempotently by name: asked
    /// again for a name it has, it answers the unit it has.
    async fn create(&self, spec: &RuntimeSpec) -> Result<UnitRef, DriverError>;

    /// Starts the unit.
    async fn start(&self, unit: &UnitRef) -> Result<(), DriverError>;

    /// Stops the unit, giving it `grace` to end by itself before it is made to.
    async fn stop(&self, unit: &UnitRef, grace: Duration) -> Result<(), DriverError>;

    /// Removes the unit, never its volumes.
    async fn remove(&self, unit: &UnitRef) -> Result<(), DriverError>;

    /// The state of the unit: `running`, `stopped`, `absent` or `failed`, with detail.
    async fn inspect(&self, unit: &UnitRef) -> Result<UnitState, DriverError>;

    /// Every unit of the system `system` (the value of `orch.system`), for reconciliation. Units of any
    /// other system, and anything without the label, are not listed.
    async fn list(&self, system: &str) -> Result<Vec<UnitRef>, DriverError>;

    /// What the unit uses now. Optional; a driver without it answers `Unsupported`.
    async fn stats(&self, unit: &UnitRef) -> Result<ResourceSample, DriverError> {
        let _ = unit;
        Err(DriverError::Unsupported("stats".to_owned()))
    }

    /// Applies the complete mount set without replacing the unit; a failure leaves it pending for recreate.
    async fn apply_mounts(
        &self,
        unit: &UnitRef,
        change: &crate::mounts::MountChange,
    ) -> Result<Vec<crate::mounts::AttachedMount>, DriverError> {
        let _ = (unit, change);
        Err(DriverError::Unsupported("live mount changes".to_owned()))
    }

    /// Makes sure a volume exists, on the storage it names or the host's default.
    async fn ensure_volume(&self, volume: &VolumeRequest) -> Result<(), DriverError>;

    /// Removes a volume and its data.
    async fn remove_volume(&self, name: &str) -> Result<(), DriverError>;

    /// Makes whatever the spec names as its source available on this host: pulls a reference, builds a
    /// Dockerfile, loads an archive. A no-op for a driver without one, which is what the default is.
    async fn ensure_image(&self, spec: &Json) -> Result<ImageEnsured, DriverError> {
        let _ = spec;
        Ok(ImageEnsured::nothing())
    }

    /// Proposes a spec from something a person already has, in one of the formats the descriptor declares.
    /// It creates nothing.
    async fn import(&self, request: &ImportRequest) -> Result<ImportProposal, DriverError> {
        let _ = request;
        Err(DriverError::Unsupported("import".to_owned()))
    }

    /// Puts `policy` in force for the unit's network: installs its list revision, closes what it no longer
    /// admits before answering, and answers what is in force. A driver that cannot enforce `restricted`
    /// refuses it and never makes the unit open instead; the default enforces nothing and does just that.
    async fn apply_network(
        &self,
        unit: &UnitRef,
        policy: &NetworkPolicy,
    ) -> Result<UnitNetwork, DriverError> {
        let _ = unit;
        match policy.mode {
            EgressMode::Open => Ok(UnitNetwork::open(policy)),
            EgressMode::Restricted => Err(DriverError::Unsupported(
                "network.restricted_unavailable: this driver enforces no restricted network"
                    .to_owned(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mounts::MountPlacement;
    use crate::units::UnitStateKind;
    use futures::executor::block_on;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// A test driver that confines nothing, has no capabilities, no suspend, no sources and no imports, and
    /// keeps its units in memory.
    struct InMemory {
        units: Mutex<BTreeMap<String, UnitState>>,
    }

    #[async_trait]
    impl EnvironmentDriver for InMemory {
        fn describe(&self) -> DriverDescriptor {
            serde_json::from_value(json!({
                "name": "memory", "version": "1", "title": "Units in memory",
                "spec_schemas": { "environment": {}, "provider": {} },
                "confinement": "none", "recreate": "restart", "process_scope": "started"
            }))
            .unwrap()
        }
        async fn create(&self, spec: &RuntimeSpec) -> Result<UnitRef, DriverError> {
            self.units
                .lock()
                .unwrap()
                .entry(spec.name.clone())
                .or_insert_with(|| UnitState::of(UnitStateKind::Stopped));
            Ok(UnitRef {
                reference: spec.name.clone(),
                name: spec.name.clone(),
                runtime_id: spec.runtime_id,
                kind: spec.kind,
            })
        }
        async fn start(&self, unit: &UnitRef) -> Result<(), DriverError> {
            let mut units = self.units.lock().unwrap();
            let state = units
                .get_mut(&unit.name)
                .ok_or_else(|| DriverError::NotFound(unit.name.clone()))?;
            state.state = UnitStateKind::Running;
            Ok(())
        }
        async fn stop(&self, unit: &UnitRef, _: Duration) -> Result<(), DriverError> {
            if let Some(state) = self.units.lock().unwrap().get_mut(&unit.name) {
                state.state = UnitStateKind::Stopped;
            }
            Ok(())
        }
        async fn remove(&self, unit: &UnitRef) -> Result<(), DriverError> {
            self.units.lock().unwrap().remove(&unit.name);
            Ok(())
        }
        async fn inspect(&self, unit: &UnitRef) -> Result<UnitState, DriverError> {
            Ok(self
                .units
                .lock()
                .unwrap()
                .get(&unit.name)
                .cloned()
                .unwrap_or_else(|| UnitState::of(UnitStateKind::Absent)))
        }
        async fn list(&self, _: &str) -> Result<Vec<UnitRef>, DriverError> {
            Ok(vec![])
        }
        async fn ensure_volume(&self, _: &VolumeRequest) -> Result<(), DriverError> {
            Ok(())
        }
        async fn remove_volume(&self, _: &str) -> Result<(), DriverError> {
            Ok(())
        }
    }

    fn spec() -> RuntimeSpec {
        let id = "01990000-0000-7000-8000-0199abcd1234";
        serde_json::from_value(json!({
          "runtime_id": id, "kind": "environment", "name": "orch-environment-0199abcd1234", "driver": "memory",
          "spec": {}, "resources": {}, "roles": ["environment"], "gateway_url": "wss://g", "enroll_token": "t",
          "ca_cert": "", "tools": [], "volumes": {}, "mounts": [], "env": {}, "labels": {}
        }))
        .unwrap()
    }

    #[test]
    fn a_driver_that_implements_only_what_it_has_passes_for_a_driver_and_the_rest_says_unsupported()
    {
        let driver = InMemory {
            units: Mutex::new(BTreeMap::new()),
        };
        let descriptor = driver.describe();
        assert_eq!(descriptor.check(), Ok(()));
        assert_eq!(descriptor.mounts, MountPlacement::None);
        let unit = block_on(driver.create(&spec())).unwrap();
        // Creating is idempotent by name.
        assert_eq!(block_on(driver.create(&spec())).unwrap(), unit);
        block_on(driver.start(&unit)).unwrap();
        assert_eq!(
            block_on(driver.inspect(&unit)).unwrap().state,
            UnitStateKind::Running
        );
        block_on(driver.stop(&unit, Duration::from_secs(10))).unwrap();
        block_on(driver.remove(&unit)).unwrap();
        assert_eq!(
            block_on(driver.inspect(&unit)).unwrap().state,
            UnitStateKind::Absent
        );
        // The optional operations: validation passes, an image is nothing, the rest are unsupported.
        assert_eq!(
            block_on(driver.validate(RuntimeKind::Environment, &json!({}))),
            Ok(())
        );
        assert_eq!(
            block_on(driver.ensure_image(&json!({}))),
            Ok(ImageEnsured::nothing())
        );
        assert_eq!(
            block_on(driver.stats(&unit)),
            Err(DriverError::Unsupported("stats".into()))
        );
        let import = ImportRequest {
            kind: RuntimeKind::Environment,
            format: "dockerfile".into(),
            values: json!({}),
        };
        assert_eq!(
            block_on(driver.import(&import)),
            Err(DriverError::Unsupported("import".into()))
        );
    }

    #[test]
    fn an_error_has_a_code_and_only_some_may_be_tried_again() {
        assert_eq!(DriverError::InvalidSpec("x".into()).code(), "invalid_spec");
        assert_eq!(
            DriverError::Unsupported("stats".into()).code(),
            "unsupported"
        );
        assert!(DriverError::Timeout.retryable());
        assert!(DriverError::Unavailable("daemon".into()).retryable());
        assert!(!DriverError::InvalidSpec("x".into()).retryable());
        assert!(!DriverError::Image("pull failed".into()).retryable());
    }
}
