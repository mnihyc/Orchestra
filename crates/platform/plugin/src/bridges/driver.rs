//! External EnvironmentDriver bridge.
//!
//! [`DriverBridge`] is an [`EnvironmentDriver`] whose implementation is a program. The node treats it as it
//! treats the Docker driver: it makes units, starts, stops and removes them, keeps their volumes, and asks for
//! their state. Every method takes `&self` and may be running many times at once; the program must answer
//! requests that overlap.
//!
//! | Method | Required | Parameters | Result |
//! |--------|----------|------------|--------|
//! | `describe` | yes | none | the driver's descriptor |
//! | `validate` | no | `kind`, `spec` | none |
//! | `create` | yes | the runtime spec, which is the parameters | the unit |
//! | `start`, `remove` | yes | `unit` | none |
//! | `stop` | yes | `unit`, `grace_secs`: whole seconds, rounded up | none |
//! | `inspect` | yes | `unit` | the state of the unit |
//! | `list` | yes | `system` | `units`: the units of that system |
//! | `stats` | no | `unit` | a resource sample |
//! | `resources` | no | `system` | `resources`: installation-owned objects and dependencies |
//! | `remove_resource` | no | `system`, `resource`: kind and opaque id | none |
//! | `stop_resource` | no | `system`, `resource`: unit kind and opaque id, `grace_secs` | none |
//! | `ensure_volume` | yes | the volume request, which is the parameters | none |
//! | `remove_volume` | yes | `name` | none |
//! | `ensure_image` | no | `spec` | the digest, and what it was built from |
//! | `import` | no | the import request, which is the parameters | the proposal: a spec and its notes |
//!
//! `create` is idempotent by the unit's name. The errors a program answers with are the driver's: `unsupported`,
//! `invalid_spec`, `not_found`, `unavailable`, `image_failed`, `timeout` and `internal`; any other code is an
//! internal error that carries it. A driver that does not declare `validate`, `stats`, `ensure_image` or `import`
//! is not asked: a spec passes validation, an image needs nothing, and `stats` and `import` are unsupported.
//! `ensure_image` may report progress, which goes to the plugin's observer.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use orch_driver_contract::descriptor::DriverDescriptor;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::imports::{ImportProposal, ImportRequest};
use orch_driver_contract::runtime::{RuntimeKind, RuntimeSpec};
use orch_driver_contract::sources::{ImageEnsured, ImageProgress, ImageStep, Source};
use orch_driver_contract::units::{ResourceSample, UnitRef, UnitState};
use orch_driver_contract::volumes::{StorageInfo, VolumeRequest};
use orch_plugin_contract::PluginKind;
use orch_types::Json;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use super::{BridgeError, Described, member, params, result};
use crate::requests::CallError;
use crate::supervisor::Plugin;

const VALIDATE: &str = "validate";
const CREATE: &str = "create";
const START: &str = "start";
const STOP: &str = "stop";
const REMOVE: &str = "remove";
const INSPECT: &str = "inspect";
const LIST: &str = "list";
const STATS: &str = "stats";
const ENSURE_VOLUME: &str = "ensure_volume";
const REMOVE_VOLUME: &str = "remove_volume";
const ENSURE_IMAGE: &str = "ensure_image";
const IMPORT: &str = "import";

/// A driver that is a program.
pub struct DriverBridge {
    plugin: Plugin,
    descriptor: Described<DriverDescriptor>,
    progress: Arc<DriverProgress>,
}

#[derive(Default)]
struct DriverProgress {
    images: Mutex<BTreeMap<orch_plugin_contract::messages::RequestId, String>>,
    volumes: Mutex<BTreeMap<orch_types::Id, Arc<dyn orch_driver_contract::volumes::VolumeWork>>>,
}

#[async_trait::async_trait]
impl crate::requests::Observer for DriverProgress {
    async fn event(&self, event: orch_plugin_contract::messages::Event) {
        if event.event == orch_driver_contract::volumes::VOLUME_INPUT_READY_EVENT {
            let Ok(ready) = serde_json::from_value::<
                orch_driver_contract::volumes::DriverVolumeInputReady,
            >(event.params) else {
                return;
            };
            let work = self
                .volumes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&ready.progress_id)
                .cloned();
            if let Some(work) = work
                && !work.is_cancelled()
            {
                work.input_ready();
            }
            return;
        }
        if event.event != orch_driver_contract::volumes::VOLUME_PROGRESS_EVENT {
            return;
        }
        let Ok(report) = serde_json::from_value::<
            orch_driver_contract::volumes::DriverVolumeProgress,
        >(event.params) else {
            return;
        };
        let work = self
            .volumes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&report.progress_id)
            .cloned();
        if let Some(work) = work
            && !work.is_cancelled()
        {
            work.report(report.progress);
        }
    }
    fn progress(
        &self,
        request: orch_plugin_contract::messages::RequestId,
        method: &str,
        progress: orch_plugin_contract::messages::ProgressParams,
    ) {
        if !matches!(method, "ensure_image" | "rebuild_image") {
            return;
        }
        if let Some(message) = progress.message {
            // Only the latest glance-sized message is retained per active request.
            const LINE_CHARS: usize = 200;
            self.images
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(request, message.chars().take(LINE_CHARS).collect());
        }
    }
}

struct ProgressCall {
    observer: Arc<DriverProgress>,
    request: orch_plugin_contract::messages::RequestId,
}
impl Drop for ProgressCall {
    fn drop(&mut self) {
        self.observer
            .images
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.request);
    }
}

struct VolumeCall {
    observer: Arc<DriverProgress>,
    progress_id: orch_types::Id,
}
impl Drop for VolumeCall {
    fn drop(&mut self) {
        self.observer
            .volumes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.progress_id);
    }
}

impl DriverBridge {
    /// The driver that `plugin` is. The plugin must be of the kind and have said hello.
    pub fn new(plugin: Plugin) -> Result<Self, BridgeError> {
        let progress = Arc::new(DriverProgress::default());
        plugin.set_observer(progress.clone());
        Ok(Self {
            descriptor: Described::new(plugin.clone(), PluginKind::Driver)?,
            plugin,
            progress,
        })
    }

    /// Applies schema-checked connection options before this driver is admitted for runtime work.
    pub async fn configure(&self, configuration: &Json) -> Result<(), DriverError> {
        self.validate_configuration(configuration)?;
        if self.plugin.supports("configure") {
            self.tell("configure", &json!({"configuration":configuration}))
                .await
        } else if configuration
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
        {
            Ok(())
        } else {
            Self::unsupported("applying driver connection settings")
        }
    }

    fn volume_methods(&self) -> Result<(), DriverError> {
        const METHODS: &[&str] = &[
            "check_volume_carry",
            "measure_volume",
            "export_volume",
            "import_volume",
            "stop_volume_work",
        ];
        let missing: Vec<_> = METHODS
            .iter()
            .filter(|method| !self.plugin.supports(method))
            .copied()
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(DriverError::Unsupported(format!(
                "owned-volume transfer methods are missing: {}",
                missing.join(", ")
            )))
        }
    }

    async fn volume_call<T: DeserializeOwned>(
        &self,
        method: &'static str,
        mut sent: Json,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<T, DriverError> {
        self.volume_methods()?;
        if work.is_cancelled() {
            return Err(DriverError::Timeout);
        }
        let progress_id = orch_types::new_id();
        sent["progress_id"] = json!(progress_id);
        sent["command_id"] = json!(work.command_id());
        sent["timeout_secs"] = json!(work.timeout_secs());
        self.progress
            .volumes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(progress_id, Arc::clone(work));
        let guard = VolumeCall {
            observer: Arc::clone(&self.progress),
            progress_id,
        };
        let call = self
            .plugin
            .start_call_within(method, sent, Duration::from_secs(work.timeout_secs()))
            .map_err(error_of)?;
        let mut call = Box::pin(call);
        let outcome = tokio::select! {
            answer = &mut call => answer.map_err(error_of).and_then(|answer| result(method, answer).map_err(error_of)),
            () = work.cancelled() => {
                Err(DriverError::Timeout)
            }
        };
        drop(call);
        drop(guard);
        if matches!(outcome, Err(DriverError::Timeout))
            && let Some(command) = work.command_id()
        {
            self.stop_volume_work(command).await?;
        }
        outcome
    }

    /// Asks the program `method` with `sent`, and reads what it answers as a `T`.
    async fn ask<T: DeserializeOwned>(
        &self,
        method: &'static str,
        sent: &impl Serialize,
    ) -> Result<T, DriverError> {
        let answer = async {
            let sent = params(method, sent)?;
            result(method, self.plugin.call(method, sent).await?)
        };
        answer.await.map_err(error_of)
    }

    /// Asks the program `method` with `sent`, and wants no more than that it did.
    async fn tell(&self, method: &'static str, sent: &impl Serialize) -> Result<(), DriverError> {
        self.ask::<Json>(method, sent).await.map(|_| ())
    }

    fn unsupported<T>(method: &str) -> Result<T, DriverError> {
        Err(DriverError::Unsupported(method.to_owned()))
    }

    async fn image_reporting(
        &self,
        method: &'static str,
        spec: &Json,
        progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        let method = if method == "rebuild_image" && !self.plugin.supports(method) {
            ENSURE_IMAGE
        } else {
            method
        };
        if !self.plugin.supports(method) {
            return Ok(ImageEnsured::nothing());
        }
        let call = self
            .plugin
            .start_call(method, json!({"spec": spec}))
            .map_err(error_of)?;
        let guard = ProgressCall {
            observer: Arc::clone(&self.progress),
            request: call.id(),
        };
        let step = match Source::from_spec(spec) {
            Ok(Source::Build(_)) => ImageStep::Build,
            Ok(Source::Archive(_)) => ImageStep::Load,
            _ => ImageStep::Pull,
        };
        let mut call = std::pin::pin!(call);
        let mut interval = tokio::time::interval(crate::supervisor::DEFAULT_TICK);
        let answer = loop {
            tokio::select! {
                answer = &mut call => break answer,
                _ = interval.tick() => {
                    let message = self.progress.images.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&guard.request);
                    if let Some(message) = message { progress.line(step, &message); }
                }
            }
        };
        let message = self
            .progress
            .images
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&guard.request);
        if let Some(message) = message {
            progress.line(step, &message);
        }
        result(method, answer.map_err(error_of)?).map_err(error_of)
    }
}

/// The error of a driver for what the program answered.
fn error_of(error: CallError) -> DriverError {
    match &error {
        CallError::Failed(remote) => {
            let message = remote.message.clone();
            match remote.code.as_str() {
                "busy" => DriverError::Busy(message),
                "no_capacity" => DriverError::NoCapacity(message),
                "unsupported" => DriverError::Unsupported(message),
                "invalid_spec" => DriverError::InvalidSpec(message),
                "not_found" => DriverError::NotFound(message),
                "unavailable" => DriverError::Unavailable(message),
                "image_failed" => DriverError::Image(message),
                "timeout" => DriverError::Timeout,
                "internal" => DriverError::Internal(message),
                other => DriverError::Internal(format!("{other}: {message}")),
            }
        }
        CallError::Unavailable { .. } | CallError::Lost { .. } => {
            DriverError::Unavailable(error.to_string())
        }
        CallError::Timeout { .. } => DriverError::Timeout,
        CallError::Cancelled | CallError::Malformed { .. } => {
            DriverError::Internal(error.to_string())
        }
    }
}

/// Whole seconds, rounded up: a grace of a moment is not none.
fn whole_seconds(span: Duration) -> u64 {
    span.as_secs() + u64::from(span.subsec_nanos() > 0)
}

#[async_trait]
impl EnvironmentDriver for DriverBridge {
    async fn resources(
        &self,
        system: &str,
    ) -> Result<Vec<orch_driver_contract::resources::Resource>, DriverError> {
        if !self.plugin.supports("resources") {
            return Self::unsupported("resource inventory");
        }
        let listed: Json = self.ask("resources", &json!({"system":system})).await?;
        member("resources", listed, "resources").map_err(error_of)
    }
    async fn remove_resource(
        &self,
        system: &str,
        resource: &orch_driver_contract::resources::ResourceRef,
    ) -> Result<(), DriverError> {
        if !self.plugin.supports("remove_resource") {
            return Self::unsupported("resource removal");
        }
        self.tell(
            "remove_resource",
            &json!({"system":system,"resource":resource}),
        )
        .await
    }

    async fn stop_resource(
        &self,
        system: &str,
        resource: &orch_driver_contract::resources::ResourceRef,
        grace: Duration,
    ) -> Result<(), DriverError> {
        if resource.kind != orch_driver_contract::resources::ResourceKind::Unit {
            return Err(DriverError::InvalidSpec(
                "only a unit can be stopped".into(),
            ));
        }
        if !self.plugin.supports("stop_resource") {
            return Self::unsupported("stopping an owned resource");
        }
        self.tell(
            "stop_resource",
            &json!({"system":system,"resource":resource,"grace_secs":whole_seconds(grace)}),
        )
        .await
    }

    async fn check_volume_carry(&self) -> Result<(), DriverError> {
        self.volume_methods()?;
        self.tell("check_volume_carry", &json!({})).await
    }
    async fn measure_volume(
        &self,
        volume: &VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<orch_driver_contract::volumes::VolumeMeasure, DriverError> {
        self.volume_call(
            "measure_volume",
            json!({"volume":volume,"selector":selector}),
            work,
        )
        .await
    }
    async fn export_volume(
        &self,
        volume: &VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        into: &std::path::Path,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), DriverError> {
        self.volume_call::<Json>(
            "export_volume",
            json!({"volume":volume,"selector":selector,"into":into}),
            work,
        )
        .await
        .map(|_| ())
    }
    async fn import_volume(
        &self,
        volume: &VolumeRequest,
        selector: orch_driver_contract::volumes::OwnedVolumeSelector,
        from: &std::path::Path,
        work: &Arc<dyn orch_driver_contract::volumes::VolumeWork>,
    ) -> Result<(), DriverError> {
        self.volume_call::<Json>(
            "import_volume",
            json!({"volume":volume,"selector":selector,"from":from}),
            work,
        )
        .await
        .map(|_| ())
    }
    async fn stop_volume_work(&self, command: orch_types::Id) -> Result<(), DriverError> {
        if !self.plugin.supports("stop_volume_work") {
            return Self::unsupported("stopping durable volume helpers");
        }
        self.tell("stop_volume_work", &json!({"command_id":command}))
            .await
    }
    fn host_resources(&self) -> Result<orch_driver_contract::host::HostResources, DriverError> {
        if !self.plugin.supports("host_resources") {
            return Self::unsupported("host_resources");
        }
        let answer = self
            .plugin
            .start_call("host_resources", json!({}))
            .map_err(error_of)?
            .wait_blocking()
            .map_err(error_of)?;
        serde_json::from_value(answer).map_err(|error| DriverError::Internal(error.to_string()))
    }

    fn validate_configuration(&self, configuration: &Json) -> Result<(), DriverError> {
        let descriptor = self.describe();
        if descriptor.configuration_schema.is_none()
            && configuration
                .as_object()
                .is_some_and(serde_json::Map::is_empty)
        {
            return Ok(());
        }
        let schema = descriptor
            .configuration_schema
            .as_ref()
            .ok_or_else(|| DriverError::Unsupported("driver configuration schema".into()))?;
        crate::schema::validate(schema, configuration)
            .map_err(|errors| DriverError::InvalidSpec(format!("{errors:?}")))
    }

    async fn apply_resources(
        &self,
        unit: &UnitRef,
        resources: &orch_driver_contract::runtime::Resources,
    ) -> Result<(), DriverError> {
        if !self.plugin.supports("apply_resources") {
            return Self::unsupported("apply_resources");
        }
        self.tell(
            "apply_resources",
            &json!({"unit":unit,"resources":resources}),
        )
        .await
    }
    async fn apply_capabilities(
        &self,
        unit: &UnitRef,
        before: &orch_driver_contract::capabilities::CapabilityState,
        after: &orch_driver_contract::capabilities::CapabilityState,
    ) -> Result<bool, DriverError> {
        if before == after {
            Ok(false)
        } else if !self.plugin.supports("apply_capabilities") {
            Self::unsupported("apply_capabilities")
        } else {
            self.ask(
                "apply_capabilities",
                &json!({"unit":unit,"before":before,"after":after}),
            )
            .await
        }
    }
    async fn estimate_keep(
        &self,
        unit: &UnitRef,
    ) -> Result<orch_driver_contract::kept::KeepEstimate, DriverError> {
        if !self.plugin.supports("estimate_keep") {
            return Self::unsupported("estimate_keep");
        }
        self.ask("estimate_keep", &json!({"unit":unit})).await
    }
    async fn keep_files(
        &self,
        unit: &UnitRef,
        environment: orch_types::Id,
        generation: i64,
        capture: orch_types::Id,
    ) -> Result<orch_driver_contract::kept::KeptImage, DriverError> {
        if !self.plugin.supports("keep_files") {
            return Self::unsupported("keep_files");
        }
        self.ask("keep_files",&json!({"unit":unit,"environment":environment,"generation":generation,"capture":capture})).await
    }
    async fn kept_images(
        &self,
        system: &str,
    ) -> Result<Vec<orch_driver_contract::kept::KeptImage>, DriverError> {
        if !self.plugin.supports("kept_images") {
            return Ok(Vec::new());
        }
        self.ask("kept_images", &json!({"system":system})).await
    }
    async fn sweep_kept_metadata(
        &self,
        system: &str,
        owners: &[orch_driver_contract::kept::KeptOwner],
    ) -> Result<usize, DriverError> {
        if !self.plugin.supports("sweep_kept_metadata") {
            return Ok(0);
        }
        self.ask(
            "sweep_kept_metadata",
            &json!({"system":system,"owners":owners}),
        )
        .await
    }
    async fn remove_kept_image(
        &self,
        image: &orch_driver_contract::kept::KeptImage,
    ) -> Result<(), DriverError> {
        if !self.plugin.supports("remove_kept_image") {
            return Self::unsupported("remove_kept_image");
        }
        self.tell("remove_kept_image", &json!({"image":image}))
            .await
    }
    async fn export_kept_image(
        &self,
        image: &orch_driver_contract::kept::KeptImage,
        into: &std::path::Path,
    ) -> Result<(), DriverError> {
        if !self.plugin.supports("export_kept_image") {
            return Self::unsupported("export_kept_image");
        }
        self.tell("export_kept_image", &json!({"image":image,"into":into}))
            .await
    }
    async fn import_kept_image(
        &self,
        image: &orch_driver_contract::kept::KeptImage,
        from: &std::path::Path,
    ) -> Result<(), DriverError> {
        if !self.plugin.supports("import_kept_image") {
            return Self::unsupported("import_kept_image");
        }
        self.tell("import_kept_image", &json!({"image":image,"from":from}))
            .await
    }
    fn storages(&self) -> Option<Vec<StorageInfo>> {
        if !self.plugin.supports("storages") {
            return None;
        }
        let answer = self
            .plugin
            .start_call("storages", json!({}))
            .ok()?
            .wait_blocking()
            .ok()?;
        serde_json::from_value(answer.get("storages").cloned().unwrap_or(answer)).ok()
    }
    async fn volume_exists(&self, volume: &VolumeRequest) -> Result<bool, DriverError> {
        if !self.plugin.supports("volume_exists") {
            return Self::unsupported("volume_exists");
        }
        let answer: Json = self.ask("volume_exists", volume).await?;
        member("volume_exists", answer, "exists").map_err(error_of)
    }
    async fn suspend(&self, unit: &UnitRef) -> Result<(), DriverError> {
        if !self.plugin.supports("suspend") || !self.describe().suspend {
            return Self::unsupported("suspend");
        }
        self.tell("suspend", &json!({"unit": unit})).await
    }
    async fn resume(&self, unit: &UnitRef) -> Result<(), DriverError> {
        if !self.plugin.supports("resume") || !self.describe().suspend {
            return Self::unsupported("resume");
        }
        self.tell("resume", &json!({"unit": unit})).await
    }
    async fn registry_digest(&self, reference: &str) -> Result<Option<String>, DriverError> {
        if !self.plugin.supports("registry_digest") {
            return Ok(None);
        }
        let answer: Json = self
            .ask("registry_digest", &json!({"reference": reference}))
            .await?;
        member("registry_digest", answer, "digest").map_err(error_of)
    }
    async fn ensure_image_reporting(
        &self,
        spec: &Json,
        progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        self.image_reporting(ENSURE_IMAGE, spec, progress).await
    }
    async fn rebuild_image(
        &self,
        spec: &Json,
        progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        self.image_reporting("rebuild_image", spec, progress).await
    }
    fn describe(&self) -> DriverDescriptor {
        let mut described = (*self.descriptor.get()).clone();
        if !self.plugin.supports("export_kept_image") || !self.plugin.supports("import_kept_image")
        {
            described.kept_file_transfer =
                orch_driver_contract::descriptor::OperationSupport::available(
                    false,
                    "this driver does not implement both directions of kept-file transfer",
                );
        }
        if !self.plugin.supports("apply_resources") {
            described.live_changes.resources = false;
        }
        if !self.plugin.supports("apply_capabilities") {
            described.live_changes.capabilities.clear();
        }
        if !self.plugin.supports("keep_files")
            || !self.plugin.supports("kept_images")
            || !self.plugin.supports("remove_kept_image")
        {
            described.recreate_preserves_files = false;
        }
        described
    }

    async fn validate(&self, kind: RuntimeKind, spec: &Json) -> Result<(), DriverError> {
        if !self.plugin.supports(VALIDATE) {
            return Ok(());
        }
        self.tell(VALIDATE, &json!({ "kind": kind, "spec": spec }))
            .await
    }

    async fn create(&self, spec: &RuntimeSpec) -> Result<UnitRef, DriverError> {
        self.ask(CREATE, spec).await
    }

    async fn start(&self, unit: &UnitRef) -> Result<(), DriverError> {
        self.tell(START, &json!({ "unit": unit })).await
    }

    async fn stop(&self, unit: &UnitRef, grace: Duration) -> Result<(), DriverError> {
        self.tell(
            STOP,
            &json!({ "unit": unit, "grace_secs": whole_seconds(grace) }),
        )
        .await
    }

    async fn remove(&self, unit: &UnitRef) -> Result<(), DriverError> {
        self.tell(REMOVE, &json!({ "unit": unit })).await
    }

    async fn inspect(&self, unit: &UnitRef) -> Result<UnitState, DriverError> {
        self.ask(INSPECT, &json!({ "unit": unit })).await
    }

    async fn list(&self, system: &str) -> Result<Vec<UnitRef>, DriverError> {
        let listed: Json = self.ask(LIST, &json!({ "system": system })).await?;
        member(LIST, listed, "units").map_err(error_of)
    }

    async fn stats(&self, unit: &UnitRef) -> Result<ResourceSample, DriverError> {
        if !self.plugin.supports(STATS) {
            return Self::unsupported(STATS);
        }
        self.ask(STATS, &json!({ "unit": unit })).await
    }

    async fn apply_mounts(
        &self,
        unit: &UnitRef,
        change: &orch_driver_contract::mounts::MountChange,
    ) -> Result<Vec<orch_driver_contract::mounts::AttachedMount>, DriverError> {
        if !self.plugin.supports("apply_mounts") {
            return Self::unsupported("apply_mounts");
        }
        self.ask("apply_mounts", &json!({"unit":unit,"change":change}))
            .await
    }

    async fn ensure_volume(&self, volume: &VolumeRequest) -> Result<(), DriverError> {
        self.tell(ENSURE_VOLUME, volume).await
    }

    async fn remove_volume(&self, name: &str) -> Result<(), DriverError> {
        self.tell(REMOVE_VOLUME, &json!({ "name": name })).await
    }

    async fn ensure_image(&self, spec: &Json) -> Result<ImageEnsured, DriverError> {
        if !self.plugin.supports(ENSURE_IMAGE) {
            return Ok(ImageEnsured::nothing());
        }
        self.ask(ENSURE_IMAGE, &json!({ "spec": spec })).await
    }

    async fn import(&self, request: &ImportRequest) -> Result<ImportProposal, DriverError> {
        if !self.plugin.supports(IMPORT) {
            return Self::unsupported(IMPORT);
        }
        self.ask(IMPORT, request).await
    }
}
