//! What the tests of the node share: a system in memory (the hub and the scripted controller), a node on it with the in-memory driver, and the commands a system sends, written as a person reads them.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking; each test uses some of what is here"
)]

use async_trait::async_trait;
use chrono::TimeZone;
use orch_connector_core::connector::{Connector, ConnectorBuilder, ConnectorParams};
use orch_connector_core::harnesses::AdapterRegistry;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::runtime::{RuntimeKind, RuntimeSpec, unit_labels, unit_name};
use orch_driver_contract::sources::ImageEnsured;
use orch_driver_contract::units::{UnitRef, UnitStateKind};
use orch_driver_contract::volumes::StorageInfo;
use orch_link::commands::Progress;
use orch_link_contract::commands::{
    CommandBody, NodeRuntimeCreate, NodeRuntimeRemove, NodeRuntimeStart, NodeRuntimeStop, SerialKey,
};
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_link_contract::streams::Stream;
use orch_node_core::drivers::{DriverExtras, DriverHost, HostDriver};
use orch_node_core::images::{BlobDesk, ImageProgress, ImageStep};
use orch_node_core::role::{NodeRole, Prepared};
use orch_node_core::settings::NodeSettings;
use orch_pki::enrollment::{MachineIdentity, issue_token};
use orch_pki::sealing::{RecipientKey, info_for_command, seal};
use orch_testkit::doubles::controller::{Applied, ScriptedController};
use orch_testkit::doubles::driver::MemoryDriver;
use orch_testkit::doubles::link::{BlobServer, Hub, ShapedBlobs, TestIdentity, TestPki};
use orch_types::{Clock, Id, Json, ManualClock, Time, new_id};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// How long a test waits for something that has to happen, in real time.
pub const PATIENCE: Duration = Duration::from_secs(20);

/// The system label the runtimes of a test carry.
pub const SYSTEM: &str = "orch-test-system";

/// What the in-memory driver has beyond the contract, as the node asks for it.
pub struct MemoryExtras {
    pub driver: Arc<MemoryDriver>,
    /// How many times a build was asked to be built again.
    pub rebuilt: AtomicUsize,
    /// Registry lookups requested after a runtime was made.
    pub registry_looks: AtomicUsize,
}

#[async_trait]
impl DriverExtras for MemoryExtras {
    fn storages(&self) -> Option<Vec<StorageInfo>> {
        let storages = self.driver.storages();
        (!storages.is_empty()).then_some(storages)
    }

    async fn rebuild_image(
        &self,
        spec: &Json,
        progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        self.rebuilt.fetch_add(1, Ordering::SeqCst);
        self.ensure_image(spec, progress).await
    }

    async fn ensure_image(
        &self,
        spec: &Json,
        progress: &dyn ImageProgress,
    ) -> Result<ImageEnsured, DriverError> {
        // As the Docker driver tells it: a build's lines are the build's, a pull's the pull's.
        let step = if spec.get("build").is_some_and(|build| !build.is_null()) {
            ImageStep::Build
        } else {
            ImageStep::Pull
        };
        self.driver
            .ensure_image_reporting(spec, &mut |line| progress.line(step, line))
            .await
    }

    async fn registry_digest(&self, reference: &str) -> Result<Option<String>, DriverError> {
        self.registry_looks.fetch_add(1, Ordering::SeqCst);
        Ok(self.driver.registry_digest(reference))
    }

    async fn suspend(&self, unit: &UnitRef) -> Result<(), DriverError> {
        self.driver.suspend(unit).await
    }

    async fn resume(&self, unit: &UnitRef) -> Result<(), DriverError> {
        self.driver.resume(unit).await
    }

    async fn enrolled(&self, unit: &UnitRef) -> Option<bool> {
        self.driver.enrolled(&unit.name)
    }
}

/// The in-memory driver, given to the node with what it has beyond the contract.
#[must_use]
pub fn hosted(driver: &Arc<MemoryDriver>, extras: &Arc<MemoryExtras>) -> HostDriver {
    let contract: Arc<dyn EnvironmentDriver> = driver.clone();
    HostDriver::new(contract).with_extras(extras.clone())
}

/// A system in memory, and one node on it.
pub struct Rig {
    pub manual: ManualClock,
    pub clock: Arc<dyn Clock>,
    pub pki: TestPki,
    pub hub: Hub,
    pub controller: ScriptedController,
    pub data: tempfile::TempDir,
    pub machine_id: Id,
    pub blobs: Arc<BlobServer>,
    pub driver: Arc<MemoryDriver>,
    pub extras: Arc<MemoryExtras>,
    pub desk: Arc<BlobDesk>,
    pub settings: NodeSettings,
    pub identity: TestIdentity,
    pub node: Option<Connector>,
    /// The role the node plays, which a test can reach into.
    pub role: Arc<std::sync::Mutex<Option<Arc<NodeRole>>>>,
    /// The authority whose pin the enrollment tokens of the test carry, and whose certificate the specs hold.
    pub authority: orch_pki::ca::CertificateAuthority,
}

impl Rig {
    /// A system with a node machine in it, and a node that is not started.
    pub fn new() -> Self {
        Self::with_driver(MemoryDriver::new("memory").replacing())
    }

    /// As [`Rig::new`], with this driver.
    pub fn with_driver(driver: MemoryDriver) -> Self {
        let driver = Arc::new(driver);
        let manual = ManualClock::new(
            chrono::Utc
                .with_ymd_and_hms(2026, 10, 11, 12, 0, 0)
                .single()
                .unwrap(),
        );
        let clock: Arc<dyn Clock> = Arc::new(manual.clone());
        let pki = TestPki::new(Arc::clone(&clock));
        let hub = Hub::new(Arc::clone(&clock), pki.certificate());
        let machine_id = new_id();
        let controller =
            ScriptedController::attach(&hub, Arc::clone(&clock), machine_id, vec![Role::Node]);
        let data = tempfile::tempdir().unwrap();
        let mut settings = NodeSettings::in_dir(data.path());
        settings.drivers = vec!["memory".to_owned()];
        // Short, so that a test of what the node does on its interval does not wait.
        settings.system_name = SYSTEM.to_owned();
        let authority =
            orch_pki::ca::CertificateAuthority::create("Test", 3650, clock.as_ref()).unwrap();
        let identity = pki.machine(machine_id);
        Self {
            manual,
            clock,
            pki,
            hub,
            controller,
            data,
            machine_id,
            blobs: BlobServer::new(),
            extras: Arc::new(MemoryExtras {
                driver: Arc::clone(&driver),
                rebuilt: AtomicUsize::new(0),
                registry_looks: AtomicUsize::new(0),
            }),
            driver,
            desk: Arc::new(BlobDesk::new()),
            settings,
            identity,
            node: None,
            role: Arc::new(std::sync::Mutex::new(None)),
            authority,
        }
    }

    fn params(&self) -> ConnectorParams {
        let mut config = orch_connector_core::enrollment::DataDir::open(self.data.path())
            .unwrap()
            .config(|_| None)
            .unwrap();
        config.connector.roles = vec!["node".into()];
        config.policy.delegated = vec!["*".into()];
        let hook = &self.settings.hooks;
        config.hooks = orch_link_contract::policy::MachineHooks {
            pre_up: hook
                .pre_up
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            post_up: hook
                .post_up
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            pre_down: hook
                .pre_down
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            post_down: hook
                .post_down
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            timeout_secs: hook.timeout.as_secs(),
        };
        // The directories the machine's operator offers (`environment.roots`): a test makes its existing
        // directories and mount sources under the system's temporary directory.
        config.environment.roots = vec![std::env::temp_dir().to_string_lossy().into_owned()];
        let local = config.policy.local_keys.get_or_insert_with(Vec::new);
        for key in ["hooks", "environment.roots"] {
            if !local.iter().any(|kept| kept == key) {
                local.push(key.into());
            }
        }
        let identity = MachineIdentity {
            machine_id: self.machine_id,
            key: self.identity.key.duplicate().unwrap(),
            certificate: self.identity.certificate.clone(),
            authority: self.pki.certificate(),
        };
        ConnectorParams {
            follow_system_endpoints: true,
            hosted_endpoints: None,
            version: "0.1.0".to_owned(),
            data_dir: self.data.path().to_owned(),
            config,
            identity,
            endpoints: vec![Hub::endpoint()],
            roles: vec![Role::Node],
            managed: true,
            dialer: self.hub.dialer(Some(self.identity.certificate.clone())),
            clock: Arc::clone(&self.clock),
            platform: Platform {
                os: Os::Linux,
                arch: Arch::X86_64,
            },
            adapters: AdapterRegistry::new(),
            tools_dir: None,
            blobs: Some(Arc::new(ShapedBlobs::new(
                Arc::clone(&self.blobs),
                None,
                i64::MAX,
            ))),
        }
    }

    /// The drivers the node is given.
    pub async fn drivers(&self) -> Arc<DriverHost> {
        DriverHost::of(
            &self.settings,
            Arc::clone(&self.desk),
            vec![hosted(&self.driver, &self.extras)],
        )
    }

    /// Starts the node and waits for its session.
    pub async fn start(&mut self) {
        let drivers = self.drivers().await;
        self.start_with(drivers).await;
    }

    /// Starts the node over these drivers and waits for its session.
    pub async fn start_with(&mut self, drivers: Arc<DriverHost>) {
        let prepared = Prepared::open(&self.settings).unwrap();
        let params = self.params();
        let (slot, desk) = (Arc::clone(&self.role), Arc::clone(&self.desk));
        let factory: orch_node_core::role::RoleFactory = Box::new(move |services| {
            let role = Arc::new(NodeRole::new(services, prepared, drivers, desk));
            *slot.lock().unwrap() = Some(Arc::clone(&role));
            role
        });
        let node = ConnectorBuilder::new(params)
            .role(factory)
            .start()
            .await
            .unwrap();
        tokio::time::timeout(PATIENCE, node.link().connected())
            .await
            .expect("the node never got a session");
        self.node = Some(node);
    }

    /// Stops the node as a program that is told to stop.
    pub async fn stop(&mut self) {
        if let Some(node) = self.node.take() {
            node.stop().await;
        }
    }

    /// Lets go of the node as a killed process does.
    pub async fn kill(&mut self) {
        if let Some(node) = self.node.take() {
            node.abandon().await;
        }
    }

    /// Waits, polling, until `check` holds.
    pub async fn until(&self, what: &str, mut check: impl FnMut() -> bool) {
        let waited = tokio::time::timeout(PATIENCE, async {
            while !check() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(waited.is_ok(), "gave up waiting for {what}");
    }

    /// Waits until the driver has received a call of `operation` about `target`.
    pub async fn wait_call(
        &self,
        operation: orch_testkit::doubles::driver::Operation,
        target: &str,
    ) {
        let driver = Arc::clone(&self.driver);
        let wanted = target.to_owned();
        self.until(
            &format!("the driver to be asked {operation:?} of {target}"),
            move || driver.targets_of(operation).contains(&wanted),
        )
        .await;
    }

    /// The role the running node plays.
    pub fn role(&self) -> Arc<NodeRole> {
        self.role
            .lock()
            .unwrap()
            .clone()
            .expect("the node is not running")
    }

    pub fn deadline(&self) -> Time {
        Time::new(self.clock.now() + chrono::Duration::minutes(5))
    }

    /// Sends a command and waits until it ended.
    pub async fn send<B: CommandBody>(&self, key: &SerialKey, body: &B) -> Progress {
        let id = self.controller.issue(key, self.deadline(), body);
        tokio::time::timeout(PATIENCE, self.controller.wait_command(id))
            .await
            .expect("the command never ended")
    }

    /// Sends a command that has to succeed, and returns what it answered.
    pub async fn succeed<B: CommandBody>(&self, key: &SerialKey, body: &B) -> Json {
        match self.send(key, body).await {
            Progress::Succeeded(result) => result,
            other => panic!("the command did not succeed: {other:?}"),
        }
    }

    /// Sends a command that has to fail, and returns the error.
    pub async fn refused<B: CommandBody>(
        &self,
        key: &SerialKey,
        body: &B,
    ) -> orch_link_contract::commands::CommandError {
        match self.send(key, body).await {
            Progress::Failed(error) => error,
            other => panic!("the command did not fail: {other:?}"),
        }
    }

    // ------------------------------------------------------------------------------------ requests

    /// A token of the test's authority, as the system makes one for a unit.
    pub fn token(&self) -> String {
        issue_token(
            &self.authority,
            Duration::from_secs(900),
            self.clock.as_ref(),
        )
        .token
        .to_string()
    }

    /// The request to make a unit of the pool `kind`, named after `label`, with a token of its own.
    pub fn spec(&self, kind: RuntimeKind, label: &str) -> RuntimeSpec {
        let id = new_id();
        let name = format!("{}-{label}", unit_name(kind, &id));
        let mut request = json!({
            "runtime_id": id, "kind": kind.as_str(), "name": name, "driver": "memory",
            "spec": { "image": "busybox:latest" }, "resources": { "memory_mb": 512 },
            "roles": [ match kind { RuntimeKind::Environment => "environment", RuntimeKind::Provider => "provider" } ],
            "gateway_url": "wss://gateway.orch.internal:9443",
            "enroll_token": self.token(), "ca_cert": self.authority.certificate().pem(),
            "tools": [],
            "volumes": { "state": { "name": format!("orch-test-{label}-state"), "storage": null } },
            "mounts": [], "env": {},
            "labels": unit_labels(SYSTEM, kind, &id)
        });
        if kind == RuntimeKind::Environment {
            request["volumes"]["workspace"] =
                json!({ "name": format!("orch-test-{label}-ws"), "storage": null });
            request["volumes"]["home"] =
                json!({ "name": format!("orch-test-{label}-home"), "storage": null });
        }
        serde_json::from_value(request).unwrap()
    }

    /// Puts a connector in the node's tool cache the way `node.tool.ensure` leaves one.
    pub fn have_connector(&self) {
        let versions = self.settings.tools_dir().join("orch-connector");
        let version = versions.join("0.1.0");
        std::fs::create_dir_all(&version).unwrap();
        std::fs::write(version.join("orch-connector"), "#!/bin/sh\n").unwrap();
        std::fs::write(version.join(".complete"), "x\n").unwrap();
        let link = versions.join("current");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink("0.1.0", link).unwrap();
    }

    /// The body of `node.runtime.create` for `spec`, its token sealed to the node for the command `command`.
    pub fn create_body(&self, spec: &RuntimeSpec, command: Id) -> NodeRuntimeCreate {
        let mut body = serde_json::to_value(spec).unwrap();
        let sealed = seal(
            &RecipientKey::from_certificate(&self.identity.certificate),
            &info_for_command(command),
            spec.enroll_token.expose().as_bytes(),
        )
        .unwrap();
        body["enroll_token"] = json!({ "$sealed": sealed });
        NodeRuntimeCreate(serde_json::from_value(body).unwrap())
    }

    /// Sends `node.runtime.create` for `spec` and waits until it ended.
    pub async fn create(&self, spec: &RuntimeSpec) -> Progress {
        let id = new_id();
        self.controller.issue_with_id(
            id,
            &SerialKey::Runtime(spec.runtime_id),
            self.deadline(),
            &self.create_body(spec, id),
        );
        tokio::time::timeout(PATIENCE, self.controller.wait_command(id))
            .await
            .expect("the command never ended")
    }

    /// `node.runtime.create` that has to succeed; answers the unit.
    pub async fn made(&self, spec: &RuntimeSpec) -> UnitRef {
        match self.create(spec).await {
            Progress::Succeeded(result) => serde_json::from_value(result["unit"].clone()).unwrap(),
            other => panic!("the unit was not made: {other:?}"),
        }
    }

    pub async fn start_runtime(&self, runtime_id: Id) -> Progress {
        self.send(
            &SerialKey::Runtime(runtime_id),
            &NodeRuntimeStart { runtime_id },
        )
        .await
    }

    pub async fn stop_runtime(&self, runtime_id: Id) -> Progress {
        self.send(
            &SerialKey::Runtime(runtime_id),
            &NodeRuntimeStop {
                runtime_id,
                reason: None,
            },
        )
        .await
    }

    pub async fn remove_runtime(&self, runtime_id: Id) -> Progress {
        self.send(
            &SerialKey::Runtime(runtime_id),
            &NodeRuntimeRemove {
                runtime_id,
                keep_volumes: true,
                volume_refs: Vec::new(),
            },
        )
        .await
    }

    // ------------------------------------------------------------------------------------ what the node told

    /// What the system applied on the node's stream.
    pub fn applied(&self) -> Vec<Applied> {
        self.controller.applied(&Stream::Node.to_string())
    }

    /// The `node.runtime` events about one runtime, as `(state, detail)` in order.
    pub fn states_of(&self, runtime_id: Id) -> Vec<(String, Option<String>)> {
        self.applied()
            .into_iter()
            .filter(|event| event.kind == "node.runtime")
            .filter(|event| event.body["runtime_id"] == json!(runtime_id))
            .map(|event| {
                (
                    event.body["state"].as_str().unwrap().to_owned(),
                    event.body["detail"].as_str().map(str::to_owned),
                )
            })
            .collect()
    }

    /// Waits until the node told something that `check` accepts.
    pub async fn wait_told(&self, what: &str, mut check: impl FnMut(&[Applied]) -> bool) {
        let stream = Stream::Node.to_string();
        let arrived = self
            .controller
            .wait_until(PATIENCE, move |c| check(&c.applied(&stream)))
            .await;
        assert!(arrived, "the node never told {what}: {:?}", self.applied());
    }

    /// Waits until the node told the state of a runtime.
    pub async fn wait_state(&self, runtime_id: Id, state: &str) {
        let wanted = state.to_owned();
        self.wait_told(&format!("{state} of {runtime_id}"), move |events| {
            events.iter().any(|event| {
                event.kind == "node.runtime"
                    && event.body["runtime_id"] == json!(runtime_id)
                    && event.body["state"] == wanted.as_str()
            })
        })
        .await;
    }
}

/// The state a unit is in, as the in-memory driver holds it.
pub fn state_in(driver: &MemoryDriver, spec: &RuntimeSpec) -> UnitStateKind {
    driver.state_of(&spec.name)
}
