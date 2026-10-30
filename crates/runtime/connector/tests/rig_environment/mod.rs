//! What the tests of the environment role share: a system in memory (the hub and the scripted controller), a connector on it that plays the environment role over a real temporary directory, and the commands a system sends, written as a person reads them.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking; each test uses some of what is here"
)]

pub mod stand_in;
pub mod stand_in_executors;
pub mod tunnels;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use chrono::TimeZone;
use orch_adapter_fake::FakeAdapter;
use orch_config::ConnectorConfig;
use orch_connector_core::connector::{Connector, ConnectorBuilder, ConnectorParams};
use orch_connector_core::harnesses::{AdapterRegistration, AdapterRegistry, HomeRule};
use orch_connector_core::roles::environment::EnvironmentParts;
use orch_link::commands::Progress;
use orch_link_contract::commands::{CommandBody, EnvAttach, IndexMode, KeyKind, SerialKey};
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_link_contract::streams::Stream;
use orch_link_contract::tunnels::{Service, TicketSource};
use orch_pki::enrollment::MachineIdentity;
use orch_pki::sealing::{RecipientKey, info_for_command, seal};
use orch_pki::tickets::{TicketRequest, TicketSigner};
use orch_testkit::doubles::controller::{Applied, ScriptedController};
use orch_testkit::doubles::link::{BlobServer, Hub, LinkShape, ShapedBlobs, TestPki};
use orch_types::{Clock, Hash, Id, Json, ManualClock, Time, new_id};

/// How long a test waits for something that has to happen, in real time.
pub const PATIENCE: Duration = Duration::from_secs(30);

/// A system in memory, and one connector on it that plays the environment role.
pub struct EnvRig {
    pub manual: ManualClock,
    pub clock: Arc<dyn Clock>,
    pub pki: TestPki,
    pub hub: Hub,
    pub controller: ScriptedController,
    pub signer: TicketSigner,
    pub data: tempfile::TempDir,
    /// The directory the machine's owner offers: environments of the tests are inside it.
    pub home: tempfile::TempDir,
    pub machine_id: Id,
    pub blobs: Arc<BlobServer>,
    pub config: ConnectorConfig,
    pub parts: EnvironmentParts,
    pub roles: Vec<Role>,
    pub connector: Option<Connector>,
    /// The environment the commands of the rig are about.
    pub environment: Id,
    /// The key the machine opens what is sealed to it with, once it is started.
    pub recipient: Option<RecipientKey>,
    /// The harnesses the connector can host.
    pub adapters: AdapterRegistry,
    /// The provider and the agent profile of the harness process the rig starts.
    pub provider: Id,
    pub agent: Id,
    /// How slow the line to the system's content store is, if it is.
    pub content_line: Option<LinkShape>,
    /// A controlled content endpoint for command-order and cancellation proofs.
    pub blob_endpoint: Option<Arc<dyn orch_link::content::BlobEndpoint>>,
    /// The tools directory of a runtime the system made; the machine is then one, and has no policy of its own.
    pub tools_dir: Option<PathBuf>,
    identity: Option<MachineIdentity>,
}

impl EnvRig {
    /// A system with a machine in it, whose owner offers a directory of its own and allows what a test
    /// needs; a connector that is not started.
    pub fn new() -> Self {
        Self::with_roles(vec![Role::Environment])
    }

    /// The same, for a machine that plays these roles.
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
        let signer = TicketSigner::generate();
        hub.set_ticket_verifier(signer.verifier());
        let machine_id = new_id();
        let controller =
            ScriptedController::attach(&hub, Arc::clone(&clock), machine_id, roles.clone());
        let home = tempfile::tempdir().unwrap();
        let mut config = ConnectorConfig::default();
        config.environment.roots = vec![home.path().display().to_string()];
        config.environment.remote_write = true;
        Self {
            manual,
            clock,
            pki,
            hub,
            controller,
            signer,
            data: tempfile::tempdir().unwrap(),
            home,
            machine_id,
            blobs: BlobServer::new(),
            config,
            parts: EnvironmentParts::default(),
            roles,
            connector: None,
            environment: new_id(),
            recipient: None,
            adapters: AdapterRegistry::new(),
            provider: new_id(),
            agent: new_id(),
            content_line: None,
            blob_endpoint: None,
            tools_dir: None,
            identity: None,
        }
    }

    /// Lets the connector host the scripted harness.
    pub fn with_the_scripted_harness(&mut self) {
        self.adapters
            .register(AdapterRegistration {
                kind: "fake",
                adapter_version: 1,
                builtin: true,
                needs_binary: false,
                home_rule: HomeRule::Connector,
                factory: Arc::new(|clock| Box::new(FakeAdapter::new(clock))),
            })
            .unwrap();
    }

    fn params(&mut self) -> ConnectorParams {
        let kept = self.identity.get_or_insert_with(|| {
            let issued = self.pki.machine(self.machine_id);
            MachineIdentity {
                machine_id: self.machine_id,
                key: issued.key,
                certificate: issued.certificate,
                authority: self.pki.certificate(),
            }
        });
        self.recipient = RecipientKey::from_point(&kept.key.public_point()).ok();
        let certificate = kept.certificate.clone();
        let identity = MachineIdentity {
            machine_id: kept.machine_id,
            key: kept.key.duplicate().unwrap(),
            certificate: certificate.clone(),
            authority: kept.authority.clone(),
        };
        ConnectorParams {
            follow_system_endpoints: true,
            hosted_endpoints: None,
            version: "0.1.0".to_owned(),
            data_dir: self.data.path().to_owned(),
            config: self.config.clone(),
            identity,
            endpoints: vec![Hub::endpoint()],
            roles: self.roles.clone(),
            managed: self.tools_dir.is_some(),
            dialer: self.hub.dialer(Some(certificate)),
            clock: Arc::clone(&self.clock),
            platform: Platform {
                os: Os::Linux,
                arch: Arch::X86_64,
            },
            adapters: self.adapters.clone(),
            tools_dir: self.tools_dir.clone(),
            blobs: Some(self.blob_endpoint.clone().unwrap_or_else(|| {
                Arc::new(ShapedBlobs::new(
                    Arc::clone(&self.blobs),
                    self.content_line,
                    i64::MAX,
                ))
            })),
        }
    }

    /// Starts the connector and waits for its session.
    pub async fn start(&mut self) {
        // This rig supplies a complete configuration directly, rather than loading sparse TOML.
        // Its fields are explicit local values; production DataDir carries its persisted manifest.
        if self.config.policy.local_keys.is_none() {
            self.config.policy.local_keys = Some(
                orch_link_contract::policy::PolicyKey::ALL
                    .iter()
                    .filter(|key| **key != orch_link_contract::policy::PolicyKey::PolicyDelegated)
                    .map(|key| key.as_str().to_owned())
                    .collect(),
            );
        }
        let params = self.params();
        let connector = ConnectorBuilder::new(params)
            .environment(self.parts.clone())
            .start()
            .await
            .unwrap();
        tokio::time::timeout(PATIENCE, connector.link().connected())
            .await
            .expect("the connector never got a session");
        self.connector = Some(connector);
    }

    /// Stops the connector as a program that is asked to stop does.
    pub async fn stop(&mut self) {
        if let Some(connector) = self.connector.take() {
            connector.stop().await;
        }
    }

    /// A directory of the machine's owner, made now.
    pub fn directory(&self, name: &str) -> PathBuf {
        let path = self.home.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    pub fn deadline(&self) -> Time {
        Time::new(self.clock.now() + chrono::Duration::minutes(5))
    }

    /// The serial key a command of this kind is sent under, for this rig's environment.
    pub fn key_of<B: CommandBody>(&self) -> SerialKey {
        match B::KIND.key() {
            KeyKind::Environment => SerialKey::Environment(self.environment),
            KeyKind::Machine => SerialKey::Machine,
            KeyKind::Runtime => SerialKey::Runtime(self.environment),
            KeyKind::Harness => SerialKey::Harness {
                provider_id: self.provider,
                agent_id: self.agent,
            },
            _ => SerialKey::None,
        }
    }

    /// Seals a value to the machine for the command `command`, as the system does a secret.
    pub fn sealed(&self, command: Id, value: &str) -> String {
        let recipient = self.recipient.as_ref().expect("the connector is started");
        seal(recipient, &info_for_command(command), value.as_bytes()).unwrap()
    }

    /// Sends a command and waits until it ended. The system's store has the command open for content.
    pub async fn send<B: CommandBody>(&self, body: &B) -> Progress {
        self.send_as(new_id(), body).await
    }

    /// Sends a command under an identifier the caller chose, which a secret of its body was sealed for.
    pub async fn send_as<B: CommandBody>(&self, id: Id, body: &B) -> Progress {
        self.blobs.open_command(id);
        self.controller
            .issue_with_id(id, &self.key_of::<B>(), self.deadline(), body);
        tokio::time::timeout(PATIENCE, self.controller.wait_command(id))
            .await
            .expect("the command never ended")
    }

    /// Sends a command that has to succeed, and returns what it answered.
    pub async fn succeed<B: CommandBody>(&self, body: &B) -> Json {
        match self.send(body).await {
            Progress::Succeeded(result) => result,
            other => panic!("the command did not succeed: {other:?}"),
        }
    }

    /// Sends a command that has to fail, and returns the code.
    pub async fn fail<B: CommandBody>(&self, body: &B) -> String {
        self.failure(body).await.0
    }

    /// Sends a command that has to fail, and returns the code and the detail.
    pub async fn failure<B: CommandBody>(&self, body: &B) -> (String, String) {
        match self.send(body).await {
            Progress::Failed(error) => (error.code, error.detail),
            other => panic!("the command did not fail: {other:?}"),
        }
    }

    /// `env.attach` for a directory, which the machine's owner offers; returns the facts.
    pub async fn attach(&self, directory: &Path) -> Json {
        let body = EnvAttach {
            resources: None,
            data_generation: None,
            storage: None,
            data_sources: None,
            environment_id: self.environment,
            workspace_root: Some(directory.display().to_string()),
            ignore: Vec::new(),
            index: IndexMode::Full,
            mounts: Some(Vec::new()),
            variables: std::collections::BTreeMap::new(),
            secrets: std::collections::BTreeMap::new(),
        };
        self.succeed(&body).await
    }

    /// Puts content in the system's store as if it had been uploaded, and returns its hash.
    pub fn stored(&self, content: &[u8]) -> Hash {
        self.blobs.insert(Bytes::copy_from_slice(content))
    }

    /// The stream of the rig's environment.
    pub fn stream(&self) -> String {
        Stream::Env {
            environment_id: self.environment,
        }
        .to_string()
    }

    /// What the system applied on the stream of the rig's environment.
    pub fn applied(&self) -> Vec<Applied> {
        self.controller.applied(&self.stream())
    }

    /// Waits until an event of `kind` was applied on the environment's stream `count` times.
    pub async fn wait_for(&self, kind: &str, count: usize) {
        let name = self.stream();
        let wanted = kind.to_owned();
        let arrived = self
            .controller
            .wait_until(PATIENCE, move |c| {
                c.applied(&name).iter().filter(|e| e.kind == wanted).count() >= count
            })
            .await;
        assert!(
            arrived,
            "no {count}th {kind} arrived; the stream holds {:?}",
            self.applied().iter().map(|e| &e.kind).collect::<Vec<_>>()
        );
    }

    /// A ticket the system signed for `service` on this machine, for a presenter.
    pub fn ticket(
        &self,
        src: TicketSource,
        service: Service,
        thread: Option<Id>,
        tool_set: Option<&str>,
    ) -> String {
        let tool = matches!(service, Service::Exec { .. } | Service::Mcp { .. });
        let request = TicketRequest {
            src,
            dst: self.machine_id,
            service,
            thread_id: thread,
            tool_set: tool_set.map(str::to_owned),
            expires_at: Time::new(self.clock.now() + chrono::Duration::hours(1)),
        };
        if tool {
            let generation = self
                .connector
                .as_ref()
                .and_then(|connector| connector.environment_role())
                .and_then(|role| role.host().environment(self.environment).ok())
                .map(|environment| environment.generation())
                .unwrap_or(0);
            self.signer
                .issue_for_tool(&request, self.environment, generation)
        } else {
            self.signer.issue(&request)
        }
        .unwrap()
    }
}
