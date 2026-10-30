//! What the tests of the connector share: a system in memory (the hub and the scripted controller), a connector on it hosting the scripted adapter, and the commands a system sends, written as a person reads them.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking; each test uses some of what is here"
)]

pub mod docker;
pub mod model;

use chrono::TimeZone;
use orch_adapter_fake::FakeAdapter;
use orch_config::ConnectorConfig;
use orch_connector_core::connector::{Connector, ConnectorBuilder, ConnectorParams};
use orch_connector_core::harnesses::{AdapterRegistration, AdapterRegistry, HomeRule};
use orch_link::commands::Progress;
use orch_link_contract::commands::{CommandBody, SerialKey};
use orch_link_contract::policy::{LocalPolicy, PolicyDefaults, PolicyKey};
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_link_contract::streams::Stream;
use orch_pki::certificates::LeafKey;
use orch_pki::enrollment::MachineIdentity;
use orch_testkit::doubles::controller::{Applied, ScriptedController};
use orch_testkit::doubles::link::TestIdentity;
use orch_testkit::doubles::link::{BlobServer, Hub, ShapedBlobs, TestPki};
use orch_types::{Clock, Id, Json, ManualClock, Time, new_id};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// How long a test waits for something that has to happen, in real time.
pub const PATIENCE: Duration = Duration::from_secs(20);

/// The registration of the scripted adapter, as a program composes it.
pub fn fake_registration() -> AdapterRegistration {
    AdapterRegistration {
        kind: "fake",
        adapter_version: 1,
        builtin: true,
        needs_binary: false,
        home_rule: HomeRule::Connector,
        factory: Arc::new(|clock| Box::new(FakeAdapter::new(clock))),
    }
}

pub fn registry() -> AdapterRegistry {
    let mut registry = AdapterRegistry::new();
    registry.register(fake_registration()).unwrap();
    registry
}

/// What a test adds to the connector it starts, once the rig has made the builder: the builder, the roles
/// the machine plays, its data directory, its key and the clock.
pub type Plug = Arc<
    dyn Fn(
        ConnectorBuilder,
        &[Role],
        &std::path::Path,
        &LeafKey,
        Arc<dyn Clock>,
    ) -> ConnectorBuilder,
>;

/// A system in memory, and one connector on it.
pub struct Rig {
    pub manual: ManualClock,
    pub clock: Arc<dyn Clock>,
    pub pki: TestPki,
    pub hub: Hub,
    pub controller: ScriptedController,
    pub data: tempfile::TempDir,
    pub machine_id: Id,
    pub blobs: Arc<BlobServer>,
    /// The connector's way to the content of the system: a test changes its shape (a line that is slow, one that
    /// recovers) while a command is under way. It lasts across starts of the connector.
    pub link: ShapedBlobs,
    pub provider: Id,
    pub agent: Id,
    pub config: ConnectorConfig,
    pub roles: Vec<Role>,
    pub connector: Option<Connector>,
    /// The key and the certificate of the machine: the same at every start of its connector.
    pub identity: TestIdentity,
    /// What is added to the connector at each start.
    pub plug: Option<Plug>,
    /// Optional source programs, applied directly without wrapping the local-only role factory.
    pub provider_sources: Option<Arc<orch_connector_core::sources::Programs>>,
    /// The adapters the connector can host.
    pub adapters: AdapterRegistry,
    /// Whether the system made the machine: it then has no local policy of its own.
    pub managed: bool,
    /// The tools directory a managed runtime is given.
    pub tools_dir: Option<std::path::PathBuf>,
    /// How long a test waits for a command or an event, in real time.
    pub patience: Duration,
}

/// A test gives the machine's configuration whole rather than as a file: what it set apart from the installed
/// cautious defaults is the owner's own value, as the keys a file names are (`policy::explicit_local_keys`).
fn owners_values(mut config: ConnectorConfig) -> ConnectorConfig {
    if config.policy.local_keys.is_none() {
        let set = orch_connector_core::policy::policy_of(&config).unwrap();
        let cautious = LocalPolicy::installed(PolicyDefaults::Cautious);
        config.policy.local_keys = Some(
            PolicyKey::ALL
                .iter()
                .filter(|key| **key != PolicyKey::PolicyDelegated)
                .filter(|key| set.get(**key) != cautious.get(**key))
                .map(|key| key.as_str().to_owned())
                .collect(),
        );
    }
    config
}

impl Rig {
    /// A system with a machine in it that plays the agent role, and a connector that is not started.
    pub fn new() -> Self {
        Self::with_roles(vec![Role::Agent])
    }

    /// A system with a machine in it that plays these roles, and a connector that is not started.
    pub fn with_roles(roles: Vec<Role>) -> Self {
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
        let pki_identity = pki.machine(machine_id);
        let controller =
            ScriptedController::attach(&hub, Arc::clone(&clock), machine_id, roles.clone());
        let blobs = BlobServer::new();
        Self {
            manual,
            clock,
            pki,
            hub,
            controller,
            data: tempfile::tempdir().unwrap(),
            machine_id,
            link: ShapedBlobs::new(Arc::clone(&blobs), None, i64::MAX),
            blobs,
            provider: new_id(),
            agent: new_id(),
            config: {
                // Offer the legacy scripted workspace and the test runner's selected scratch directory.
                let mut config = ConnectorConfig::default();
                config.environment.roots = vec!["/tmp".to_owned()];
                let scratch = std::env::temp_dir().to_string_lossy().into_owned();
                if scratch != "/tmp" {
                    config.environment.roots.push(scratch);
                }
                config
            },
            roles,
            connector: None,
            identity: pki_identity,
            plug: None,
            provider_sources: None,
            adapters: registry(),
            managed: false,
            tools_dir: None,
            patience: PATIENCE,
        }
    }

    fn params(&self) -> ConnectorParams {
        let issued = &self.identity;
        let identity = MachineIdentity {
            machine_id: self.machine_id,
            key: issued.key.duplicate().unwrap(),
            certificate: issued.certificate.clone(),
            authority: self.pki.certificate(),
        };
        ConnectorParams {
            follow_system_endpoints: true,
            hosted_endpoints: None,
            version: "0.1.0".to_owned(),
            data_dir: self.data.path().to_owned(),
            config: owners_values(self.config.clone()),
            identity,
            endpoints: vec![Hub::endpoint()],
            roles: self.roles.clone(),
            managed: self.managed,
            dialer: self.hub.dialer(Some(issued.certificate.clone())),
            clock: Arc::clone(&self.clock),
            platform: Platform {
                os: Os::Linux,
                arch: Arch::X86_64,
            },
            adapters: self.adapters.clone(),
            tools_dir: self.tools_dir.clone(),
            blobs: Some(Arc::new(self.link.clone())),
        }
    }

    /// Starts the connector and waits for its session.
    pub async fn start(&mut self) {
        let mut builder = ConnectorBuilder::new(self.params());
        if let Some(plug) = &self.plug {
            builder = plug(
                builder,
                &self.roles,
                self.data.path(),
                &self.identity.key,
                Arc::clone(&self.clock),
            );
        }
        if let Some(sources) = &self.provider_sources {
            builder = builder.provider_sources(Arc::clone(sources));
        }
        let connector = builder.start().await.unwrap();
        tokio::time::timeout(self.patience, connector.link().connected())
            .await
            .expect("the connector never got a session");
        self.connector = Some(connector);
    }

    /// Lets go of the connector as a killed process does.
    pub async fn kill(&mut self) {
        if let Some(connector) = self.connector.take() {
            connector.abandon().await;
        }
    }

    pub fn deadline(&self) -> Time {
        Time::new(self.clock.now() + chrono::Duration::minutes(5))
    }

    /// Sends a command and waits until it ended.
    pub async fn send<B: CommandBody>(&self, key: &SerialKey, body: &B) -> Progress {
        let id = self.controller.issue(key, self.deadline(), body);
        tokio::time::timeout(self.patience, self.controller.wait_command(id))
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

    /// Sends a command that has to fail, and returns the code.
    pub async fn fail<B: CommandBody>(&self, key: &SerialKey, body: &B) -> String {
        match self.send(key, body).await {
            Progress::Failed(error) => error.code,
            other => panic!("the command did not fail: {other:?}"),
        }
    }

    pub fn harness_key(&self) -> SerialKey {
        SerialKey::Harness {
            provider_id: self.provider,
            agent_id: self.agent,
        }
    }

    /// `agent.start` for the scripted adapter, with `config` as the profile's configuration.
    pub async fn agent_start(&self, config: Json) -> Json {
        let body: orch_link_contract::commands::AgentStart = serde_json::from_value(json!({
            "provider_id": self.provider, "agent_id": self.agent, "harness_kind": "fake",
            "profile": { "harness_version": null, "config": config, "env": {} },
            "home": null
        }))
        .unwrap();
        self.succeed(&self.harness_key(), &body).await
    }

    /// The body of `session.open` for a new session of a thread, in the directory `directory`, which is also the
    /// workspace root, as an environment sends it: the working directory always lies inside the root.
    pub fn open_body(&self, thread: Id, segment: Id, directory: &str) -> Json {
        json!({
            "agent_id": self.agent, "thread_id": thread, "segment_id": segment, "mode": "new",
            "native_session_ref": null, "archive": null, "history": null, "upto_turn_ordinal": null,
            "workspace_root": directory, "instruction_root_marker": false,
            "directory": directory, "cwd": directory, "mounts": [],
            "model": "fake-large", "effort": "medium", "params": {},
            "approval_mode": "on_request", "sandbox_mode": "workspace",
            "instructions": "You are working inside Orch.", "memory": null,
            "arrangement": "together",
            "provider": { "path": "native", "provider_id": self.provider },
            "exec": { "executor": null, "environment_id": new_id(), "generation": 1 },
            "mcp": [], "orch_tools": [], "raw_events": false
        })
    }

    /// Opens a session; returns what `session.open` answered.
    pub async fn open_session(&self, body: Json) -> Json {
        let thread: Id = serde_json::from_value(body["thread_id"].clone()).unwrap();
        let body: orch_link_contract::commands::SessionOpen = serde_json::from_value(body).unwrap();
        self.succeed(&SerialKey::Thread(thread), &body).await
    }

    /// `turn.start` with `text`; returns the turn.
    pub async fn start_turn(&self, thread: Id, text: &str) -> Id {
        let turn = new_id();
        let body: orch_link_contract::commands::TurnStart = serde_json::from_value(json!({
            "thread_id": thread, "turn_id": turn, "text": text, "images": [],
            "model": "fake-large", "effort": "high", "params": {},
            "approval_mode": "on_request", "sandbox_mode": "workspace"
        }))
        .unwrap();
        self.succeed(&SerialKey::Thread(thread), &body).await;
        turn
    }

    pub fn stream_of(thread: Id, segment: Id) -> String {
        Stream::Session {
            thread_id: thread,
            segment_id: segment,
        }
        .to_string()
    }

    /// What the system applied on the stream of a session.
    pub fn applied(&self, thread: Id, segment: Id) -> Vec<Applied> {
        self.controller.applied(&Self::stream_of(thread, segment))
    }

    /// The kinds of what the system applied on a stream, in order.
    pub fn kinds(&self, thread: Id, segment: Id) -> Vec<String> {
        self.applied(thread, segment)
            .into_iter()
            .map(|event| event.kind)
            .collect()
    }

    /// Waits until an event of `kind` was applied on a stream `count` times.
    pub async fn wait_for(&self, thread: Id, segment: Id, kind: &str, count: usize) {
        let name = Self::stream_of(thread, segment);
        let wanted = kind.to_owned();
        let arrived = self
            .controller
            .wait_until(self.patience, move |c| {
                c.applied(&name).iter().filter(|e| e.kind == wanted).count() >= count
            })
            .await;
        assert!(
            arrived,
            "no {count}th {kind} arrived; the stream holds {:?}",
            self.kinds(thread, segment)
        );
    }

    /// The text an agent message ended with, by the events that completed it.
    pub fn messages(&self, thread: Id, segment: Id) -> Vec<String> {
        self.applied(thread, segment)
            .into_iter()
            .filter(|e| e.kind == "item.completed" && e.body["payload"].get("text").is_some())
            .map(|e| {
                e.body["payload"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }
}
