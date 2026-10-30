//! A real connector core with its environment role, driven through its link and the API's stored commands.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_api_core::backplane::Report;
use orch_api_core::dispatch::cmd_of;
use orch_api_core::state::App;
use orch_config::ConnectorConfig;
use orch_connector_core::connector::{Connector, ConnectorBuilder, ConnectorParams};
use orch_connector_core::harnesses::AdapterRegistry;
use orch_db::queries::link as rows;
use orch_link::commands::Progress;
use orch_link::controller::{ControllerLink, Notice};
use orch_link_contract::commands::{Cmd, CommandBody, EnvAttach, IndexMode, SerialKey};
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_pki::enrollment::MachineIdentity;
use orch_testkit::doubles::link::{BlobServer, Hub, MachineRecord, ShapedBlobs, TestPki};
use orch_types::{Id, Json, Time, new_id};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// A busy shared machine can take this long to schedule a link or finish a command.
const PATIENCE: Duration = Duration::from_secs(90);
/// The bridge polls stored commands often enough to add no visible delay to a watch.
const POLL: Duration = Duration::from_millis(20);

/// The connector, its real workspace and the commands the system sent it.
pub struct Machine {
    pub id: Id,
    pub root: PathBuf,
    pub connector: Option<Connector>,
    pub hub: Hub,
    controller: ControllerLink,
    app: App,
    seen: Arc<Mutex<Vec<Cmd>>>,
    stop: CancellationToken,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    _data: tempfile::TempDir,
}

impl Machine {
    /// Starts the real connector, attaches the workspace and forwards its reports through the production bus.
    pub async fn start(app: App, environment: Id) -> Self {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let (id, generation) = {
            let mut conn = app.db.acquire().await.unwrap();
            let id = orch_db::queries::resources::testing::insert_machine(
                &mut conn,
                "feed-machine",
                "connected",
            )
            .await
            .unwrap();
            sqlx::query(
                "UPDATE machines SET roles = ARRAY['environment'], online = true WHERE id = $1",
            )
            .bind(id)
            .execute(&mut *conn)
            .await
            .unwrap();
            let generation: i32 = sqlx::query_scalar("UPDATE environments SET machine_id = $2, workspace_root = $3, status = 'running' WHERE id = $1 RETURNING generation")
                .bind(environment).bind(id).bind(root.to_string_lossy().as_ref()).fetch_one(&mut *conn).await.unwrap();
            (id, generation)
        };
        let pki = TestPki::new(Arc::clone(&app.clock));
        let identity = pki.machine(id);
        let hub = Hub::new(Arc::clone(&app.clock), pki.certificate());
        let (controller, mut notices) = hub.add_machine(
            id,
            vec![Role::Environment],
            BTreeMap::new(),
            MachineRecord {
                active: true,
                fingerprint: None,
            },
        );
        let stop = CancellationToken::new();
        let forwarded = controller.clone();
        let reports_app = app.clone();
        let reports_stop = stop.clone();
        let reports = tokio::spawn(async move {
            loop {
                let notice = tokio::select! { () = reports_stop.cancelled() => break, notice = notices.recv() => notice };
                match notice {
                    Some(Notice::Report(eph)) => {
                        orch_api_core::realtime::ephemeral::forward_report(
                            &reports_app.db,
                            &Report {
                                machine_id: id,
                                eph,
                            },
                        )
                        .await
                        .unwrap();
                    }
                    Some(Notice::Deliver(delivery)) => forwarded.committed(&delivery),
                    Some(Notice::Online { .. } | Notice::Offline { .. } | Notice::Reset(_)) => {}
                    None => break,
                }
            }
        });
        let mut config = ConnectorConfig::default();
        config.environment.roots = vec![data.path().to_string_lossy().into_owned()];
        config.environment.remote_write = true;
        config.environment.terminal = true;
        // Programmatic values need the same explicit provenance as operator-written keys.
        config.policy.local_keys = Some(vec![
            "environment.roots".to_owned(),
            "environment.remote_write".to_owned(),
            "environment.terminal".to_owned(),
        ]);
        let blobs = BlobServer::new();
        let connector = ConnectorBuilder::new(ConnectorParams {
            follow_system_endpoints: true,
            hosted_endpoints: None,
            version: "0.1.0".to_owned(),
            data_dir: data.path().join("connector"),
            config,
            identity: MachineIdentity {
                machine_id: id,
                key: identity.key.duplicate().unwrap(),
                certificate: identity.certificate.clone(),
                authority: pki.certificate(),
            },
            endpoints: vec![Hub::endpoint()],
            roles: vec![Role::Environment],
            managed: false,
            dialer: hub.dialer(Some(identity.certificate.clone())),
            clock: Arc::clone(&app.clock),
            platform: Platform {
                os: Os::Linux,
                arch: Arch::X86_64,
            },
            adapters: AdapterRegistry::new(),
            tools_dir: None,
            blobs: Some(Arc::new(ShapedBlobs::new(blobs, None, i64::MAX))),
        })
        .start()
        .await
        .unwrap();
        tokio::time::timeout(PATIENCE, connector.link().connected())
            .await
            .unwrap();
        let attach = Cmd::new(
            new_id(),
            &SerialKey::Environment(environment),
            Time::new(app.clock.now() + chrono::Duration::minutes(5)),
            &EnvAttach {
                resources: None,
                data_generation: Some(i64::from(generation)),
                storage: None,
                data_sources: None,
                environment_id: environment,
                workspace_root: Some(root.to_string_lossy().into_owned()),
                ignore: vec![],
                index: IndexMode::Full,
                mounts: None,
                variables: BTreeMap::new(),
                secrets: BTreeMap::new(),
            },
        )
        .unwrap();
        let mut handle = controller.commands().issue(attach);
        match tokio::time::timeout(PATIENCE, handle.wait()).await.unwrap() {
            Progress::Succeeded(facts) => assert_eq!(
                facts["generation"], generation,
                "the native attachment and API directory selectors must name the same generation"
            ),
            Progress::Failed(error) => panic!(
                "fixture env.attach failed: code={:?}, retryable={}",
                error.code, error.retryable,
            ),
            other => panic!("fixture env.attach did not succeed: {other:?}"),
        }
        let seen = Arc::new(Mutex::new(Vec::new()));
        let bridge_app = app.clone();
        let bridge_controller = controller.clone();
        let bridge_seen = seen.clone();
        let bridge_stop = stop.clone();
        let bridge = tokio::spawn(async move {
            let mut taken = HashSet::new();
            let mut handles = HashMap::new();
            loop {
                tokio::select! { () = bridge_stop.cancelled() => break, () = tokio::time::sleep(POLL) => {} }
                bridge_controller
                    .commands()
                    .tick(tokio::time::Instant::now());
                let open = {
                    let mut conn = bridge_app.db.acquire().await.unwrap();
                    rows::open_commands(&mut conn, id).await.unwrap()
                };
                for row in open {
                    if !taken.insert(row.id) {
                        continue;
                    }
                    let command = cmd_of(&row);
                    bridge_seen
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(command.clone());
                    let handle = bridge_controller.commands().issue(command);
                    handles.insert(row.id, handle);
                }
                let done: Vec<_> = handles
                    .iter()
                    .filter_map(|(id, handle)| {
                        let progress = handle.progress();
                        progress.is_final().then_some((*id, progress))
                    })
                    .collect();
                for (id, result) in done {
                    handles.remove(&id);
                    let mut conn = bridge_app.db.acquire().await.unwrap();
                    match result {
                        Progress::Succeeded(body) => {
                            rows::finish_command(
                                &mut conn,
                                id,
                                &rows::Ending::Succeeded(&body),
                                bridge_app.clock.now(),
                            )
                            .await
                            .unwrap();
                        }
                        Progress::Failed(error) => {
                            let error = serde_json::to_value(error).unwrap();
                            rows::finish_command(
                                &mut conn,
                                id,
                                &rows::Ending::Failed(&error),
                                bridge_app.clock.now(),
                            )
                            .await
                            .unwrap();
                        }
                        _ => {
                            rows::finish_command(
                                &mut conn,
                                id,
                                &rows::Ending::Expired,
                                bridge_app.clock.now(),
                            )
                            .await
                            .unwrap();
                        }
                    }
                }
            }
        });
        Self {
            id,
            root,
            connector: Some(connector),
            hub,
            controller,
            app,
            seen,
            stop,
            tasks: vec![reports, bridge],
            _data: data,
        }
    }

    /// The recorded commands of one kind, in order.
    pub fn seen(&self, kind: &str) -> Vec<Cmd> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|cmd| cmd.kind == kind)
            .cloned()
            .collect()
    }

    /// Whether every recorded command of this kind has completed successfully.
    pub async fn settled(&self, kind: &str) -> bool {
        let seen = self.seen(kind);
        if seen.is_empty() {
            return false;
        }
        let mut conn = self.app.db.acquire().await.unwrap();
        for command in seen {
            if rows::command(&mut conn, command.id)
                .await
                .unwrap()
                .is_none_or(|row| row.state != "succeeded")
            {
                return false;
            }
        }
        true
    }

    /// Sends a catalogue command through the real connector's link.
    pub async fn send<B: CommandBody>(&self, body: &B) -> Json {
        let cmd = Cmd::new(
            new_id(),
            &SerialKey::None,
            Time::new(self.app.clock.now() + chrono::Duration::minutes(5)),
            body,
        )
        .unwrap();
        let id = cmd.id;
        let mut handle = self.controller.commands().issue(cmd);
        match tokio::time::timeout(PATIENCE, handle.wait())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "native fixture command {} id={id} exceeded its original deadline",
                    B::KIND.as_str()
                )
            }) {
            Progress::Succeeded(body) => body,
            Progress::Failed(error) => panic!(
                "native fixture command {} id={id} failed: code={} retryable={}",
                B::KIND.as_str(),
                error.code,
                error.retryable
            ),
            other => panic!(
                "native fixture command {} id={id} did not succeed: {other:?}",
                B::KIND.as_str()
            ),
        }
    }

    /// Makes the connector's session disappear or return, with matching machine presence.
    pub async fn online(&self, online: bool) {
        // A transport outage lasts until the fixture explicitly restores connectivity.
        self.hub
            .faults()
            .refuse_connections(if online { 0 } else { usize::MAX });
        if !online {
            self.hub.faults().sever_now();
        }
        if online && let Some(connector) = &self.connector {
            tokio::time::timeout(PATIENCE, connector.link().connected())
                .await
                .unwrap();
            println!(
                "connector returned: {:?}",
                *connector.link().status().borrow()
            );
        }
        let mut conn = self.app.db.acquire().await.unwrap();
        orch_db::queries::resources::set_presence(
            &mut conn,
            self.id,
            online,
            None,
            self.app.clock.now(),
        )
        .await
        .unwrap();
    }

    /// Ends the connector and every bridge task this test started.
    pub async fn stop(&mut self) {
        self.stop.cancel();
        if let Some(connector) = self.connector.take() {
            connector.stop().await;
        }
        for task in self.tasks.drain(..) {
            task.await.unwrap();
        }
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.stop.cancel();
        for task in &self.tasks {
            task.abort();
        }
    }
}
