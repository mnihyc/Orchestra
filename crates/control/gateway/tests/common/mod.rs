//! A whole system on loopback: a database, the secrets of a deployment, the API's listeners, a gateway, and machines.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    dead_code,
    reason = "a test fails by panicking, and each test file uses some of the helpers"
)]

#[path = "../content_support/mod.rs"]
mod content_support;

use async_trait::async_trait;
use axum::Router;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use orch_api_core::backplane::{ApplyError, EventApplier, Link, LinkParts};
use orch_api_core::certificates::{TokenFor, TokenRequest};
use orch_api_core::server;
use orch_api_core::state::{App, AppState};
use orch_build_info::BuildInfo;
use orch_config::ServerConfig;
use orch_db::queries::resources::testing as rows;
use orch_db::secrets::MasterKey;
use orch_db::{Conn, Db, DbError};
use orch_gateway_core::{Gateway, GatewayParts, Settings};
use orch_link::commands::CommandHandler;
use orch_link::machine::{MachineConfig, MachineLink};
use orch_link::spool::SpoolOptions;
use orch_link::streams::Delivery;
use orch_link::tunnels::channel::TunnelOptions;
use orch_link::websocket::TlsDialer;
use orch_link_contract::enrollment::{
    ENROLL_PATH, Endpoint, EnrollReply, EnrollRequest, EnrollmentToken, Trust,
};
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_pki::ca::{CA_CERT_FILE, CA_KEY_FILE, CaFiles, CertificateAuthority, Holder};
use orch_pki::certificates::{Certificate, LeafKey, ServiceName, write_file, write_private_file};
use orch_pki::enrollment::{MachineIdentity, PendingEnrollment};
use orch_pki::verification::{
    ServerTrust, TlsIdentity, pinned_client_config, public_client_config, root_store,
};
use orch_testkit::database::TestDb;
use orch_testkit::doubles::machine::ScriptedHandler;
use orch_types::{Clock, Id, SystemClock};
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;

/// The names the service certificates of a test deployment are good for.
const NAMES: [&str; 6] = [
    "api",
    "gateway",
    "worker",
    "gateway.orch.internal",
    "localhost",
    "127.0.0.1",
];

/// How long a test waits for something that has to happen.
pub const PATIENCE: Duration = Duration::from_secs(30);

/// Waits until `check` holds, for `PATIENCE`.
pub async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let waited = tokio::time::timeout(PATIENCE, async {
        loop {
            if check().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "gave up waiting for {what}");
}

/// The secrets of a deployment, laid out in a directory as the program that
/// composes them lays them out.
pub struct Secrets {
    pub dir: tempfile::TempDir,
}

impl Secrets {
    pub fn compose(clock: &dyn Clock) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let files = CaFiles::in_dir(dir.path());
        let (authority, _) = files.create_if_missing("Test", 3_650, clock).unwrap();
        for service in [ServiceName::Api, ServiceName::Gateway, ServiceName::Worker] {
            let (key, certificate) = authority
                .issue_with_new_key(
                    &Holder::Service {
                        service,
                        names: NAMES.iter().map(|name| (*name).to_owned()).collect(),
                    },
                    365,
                    clock,
                )
                .unwrap();
            write_private_file(
                &dir.path().join(format!("service_{service}_key.pem")),
                &key.to_pem(),
            )
            .unwrap();
            write_file(
                &dir.path().join(format!("service_{service}_cert.pem")),
                &certificate.pem(),
            )
            .unwrap();
        }
        Self { dir }
    }

    pub fn authority(&self) -> CertificateAuthority {
        CaFiles::in_dir(self.dir.path()).load().unwrap()
    }

    pub fn ca_certificate(&self) -> Certificate {
        Certificate::from_pem(&std::fs::read_to_string(self.dir.path().join(CA_CERT_FILE)).unwrap())
            .unwrap()
    }

    pub fn identity(&self, service: ServiceName) -> TlsIdentity {
        let read = |name: String| std::fs::read_to_string(self.dir.path().join(name)).unwrap();
        TlsIdentity {
            chain: vec![
                Certificate::from_pem(&read(format!("service_{service}_cert.pem"))).unwrap(),
            ],
            key: LeafKey::from_pem(&read(format!("service_{service}_key.pem"))).unwrap(),
        }
    }

    /// A certificate authority that is not this deployment's, and a service
    /// certificate it issued for the gateway: what a stranger shows.
    pub fn stranger(clock: &dyn Clock) -> TlsIdentity {
        let other = CertificateAuthority::create("Other", 3_650, clock).unwrap();
        let (key, certificate) = other
            .issue_with_new_key(
                &Holder::Service {
                    service: ServiceName::Gateway,
                    names: NAMES.iter().map(|name| (*name).to_owned()).collect(),
                },
                365,
                clock,
            )
            .unwrap();
        TlsIdentity {
            chain: vec![certificate],
            key,
        }
    }

    pub fn ca_key_path(&self) -> PathBuf {
        self.dir.path().join(CA_KEY_FILE)
    }
}

/// Applies events under unique request keys in the initial fixture user's canonical ledger, in the transaction that moves
/// the cursor, so that what was applied can be read back and a second
/// application of anything is refused by the unique ledger key.
pub struct RecordingApplier {
    pub refused: AtomicUsize,
    pub applied: AtomicUsize,
}

#[async_trait]
impl EventApplier for RecordingApplier {
    async fn apply(
        &self,
        tx: &mut Conn,
        machine: Id,
        delivery: &Delivery,
    ) -> Result<(), ApplyError> {
        for event in &delivery.events {
            let result = sqlx::query(
                "INSERT INTO idempotency_keys (user_id,key,method,path,status,response) SELECT id,$1::uuid::text || '/' || $2::text || '/' || $3::bigint::text,'LINK_EVENT',$2,200,jsonb_build_object('machine_id',$1::uuid,'seq',$3::bigint,'body',$4::jsonb) FROM users ORDER BY created_at,id LIMIT 1",
            )
            .bind(machine)
            .bind(delivery.stream.to_string())
            .bind(event.seq)
            .bind(&event.body)
            .execute(&mut *tx)
            .await;
            if let Err(error) = result {
                self.refused.fetch_add(1, Ordering::SeqCst);
                return Err(ApplyError::Db(DbError::from(error)));
            }
            self.applied.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// The API's two listeners over a link, as the program composes them: only
/// the internal one, which is all a gateway reaches.
pub struct Api {
    pub link: Link,
    pub app: App,
    pub port: u16,
    pub applier: Arc<RecordingApplier>,
    stop: CancellationToken,
    task: Option<JoinHandle<std::io::Result<()>>>,
}

impl Api {
    pub async fn stop(&mut self) {
        self.stop.cancel();
        self.link.shutdown();
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(15), task).await;
        }
    }
}

/// A gateway on two ports.
pub struct Running {
    pub gateway: Gateway,
    pub pinned: SocketAddr,
    pub edge: SocketAddr,
    task: Option<JoinHandle<()>>,
}

impl Running {
    pub async fn stop(&mut self) {
        self.gateway.shutdown();
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(15), task).await;
        }
    }
}

/// Everything a test stands on.
pub struct World {
    pub db: Db,
    pub test_db: TestDb,
    pub clock: Arc<dyn Clock>,
    pub secrets: Secrets,
    pub config: Arc<ServerConfig>,
    pub api: Api,
    pub gateway: Running,
    pub user: Id,
    pub machines_dir: tempfile::TempDir,
    pub content: Router<App>,
    /// The key that stored secrets are sealed under.
    pub master: [u8; 32],
    /// How many times events were refused by an API that has since stopped.
    pub refused_before: usize,
    /// How many events an API that has since stopped applied.
    pub applied_before: usize,
}

fn loopback(port: u16) -> String {
    format!("127.0.0.1:{port}")
}

async fn start_api(
    db: &Db,
    clock: &Arc<dyn Clock>,
    secrets: &Secrets,
    config: &Arc<ServerConfig>,
    content: Router<App>,
    master: [u8; 32],
    port: Option<u16>,
) -> Api {
    let link = Link::new(LinkParts {
        db: db.clone(),
        clock: Arc::clone(clock),
        config: Arc::clone(config),
        release_version: "0.1.0".to_owned(),
        authority: Arc::new(secrets.authority()),
        master_key: MasterKey::new(master),
    });
    let applier = Arc::new(RecordingApplier {
        refused: AtomicUsize::new(0),
        applied: AtomicUsize::new(0),
    });
    link.set_applier(applier.clone());
    link.start().await.unwrap();
    let app: App = Arc::new(AppState {
        host_ports: Default::default(),
        builtin_sources: Default::default(),
        db: db.clone(),
        config: Arc::clone(config),
        clock: Arc::clone(clock),
        link: link.clone(),
        engine: orch_api_core::thread_engine::detached(db, clock, config, &link),
        realtime: orch_api_core::realtime::Realtime::new(db, clock, config),
        build: BuildInfo::new("api", "0.0.0-test", "", "1"),
        blobs: orch_api_core::blobs::Blobs::in_memory(),
        cache: orch_api_core::cache::Cache::detached(),
    });
    let router = server::internal_router(Arc::clone(&app), content);
    let tls = server::internal_tls_config(
        &secrets.identity(ServiceName::Api),
        &secrets.ca_certificate(),
        Arc::clone(clock),
    )
    .unwrap();
    // A restart takes the same port back, as the program would: the gateway
    // dials an address it was given.
    let bind = loopback(port.unwrap_or(0));
    let listener = bind_with_retry(&bind).await;
    let port = listener.local_addr().unwrap().port();
    let stop = CancellationToken::new();
    let task = tokio::spawn(server::serve_tls(listener, tls, router, stop.clone()));
    Api {
        link,
        app,
        port,
        applier,
        stop,
        task: Some(task),
    }
}

/// Binds an address, trying again for a few seconds while the last holder of
/// it lets go.
pub async fn bind_with_retry(address: &str) -> TcpListener {
    for _ in 0..100 {
        if let Ok(listener) = TcpListener::bind(address).await {
            return listener;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{address} could not be bound");
}

async fn start_gateway(
    world_clock: &Arc<dyn Clock>,
    secrets: &Secrets,
    api_port: u16,
    pinned: Option<SocketAddr>,
    edge: Option<SocketAddr>,
    identity: Option<TlsIdentity>,
) -> Running {
    let settings = Settings {
        gateway_id: "test-gateway".to_owned(),
        version: "0.0.0-test".to_owned(),
        backplane_url: format!("wss://127.0.0.1:{api_port}/internal/backplane"),
        heartbeat_secs: 20,
        limits: orch_link_contract::version::Limits::default(),
        tunnel_resume_grace: Duration::from_secs(60),
        expired_renew_grace_days: 90,
        trusted_proxy_cidrs: Vec::new(),
    };
    let gateway = Gateway::new(GatewayParts {
        settings,
        identity: identity.unwrap_or_else(|| secrets.identity(ServiceName::Gateway)),
        authority: secrets.ca_certificate(),
        clock: Arc::clone(world_clock),
    })
    .unwrap();
    let pinned = bind_with_retry(&pinned.map_or_else(|| loopback(0), |a| a.to_string())).await;
    let edge = bind_with_retry(&edge.map_or_else(|| loopback(0), |a| a.to_string())).await;
    let (pinned_addr, edge_addr) = (pinned.local_addr().unwrap(), edge.local_addr().unwrap());
    let running = gateway.clone();
    let task = tokio::spawn(async move { running.run(pinned, edge).await });
    Running {
        gateway,
        pinned: pinned_addr,
        edge: edge_addr,
        task: Some(task),
    }
}

impl World {
    /// A gateway/API fixture with content explicitly disabled.
    pub async fn start() -> Self {
        Self::start_with(content_support::router()).await
    }

    /// As [`World::start`], with something that answers content requests.
    pub async fn start_with(content: Router<App>) -> Self {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let secrets = Secrets::compose(clock.as_ref());
        let test_db = TestDb::fresh().await;
        let db = Db::from_pool(test_db.pool.clone());
        let user = {
            let mut conn = db.acquire().await.unwrap();
            rows::insert_person_with_home(&mut conn, "ada", clock.now())
                .await
                .unwrap()
        };
        // The ports of the gateway are known before the API is, because the
        // API tells machines about them when they enroll.
        let pinned_port = reserve_port();
        let edge_port = reserve_port();
        let mut config = ServerConfig::default();
        // Renewal must issue the same deployment names that the fixture's original services use.
        config.pki.service_dns_names = NAMES.iter().map(|name| (*name).to_owned()).collect();
        config.gateway.endpoints = vec![
            orch_config::server::GatewayEndpoint {
                url: format!("wss://127.0.0.1:{pinned_port}"),
                trust: "pinned".to_owned(),
            },
            orch_config::server::GatewayEndpoint {
                url: format!("wss://127.0.0.1:{edge_port}"),
                trust: "public".to_owned(),
            },
        ];
        let config = Arc::new(config);
        let mut master = [0u8; 32];
        rand::fill(&mut master[..]);
        let api = start_api(
            &db,
            &clock,
            &secrets,
            &config,
            content.clone(),
            master,
            None,
        )
        .await;
        let gateway = start_gateway(
            &clock,
            &secrets,
            api.port,
            Some(loopback(pinned_port).parse().unwrap()),
            Some(loopback(edge_port).parse().unwrap()),
            None,
        )
        .await;
        gateway.gateway.wait_for_backplane().await;
        Self {
            db,
            test_db,
            clock,
            secrets,
            config,
            api,
            gateway,
            user,
            machines_dir: tempfile::tempdir().unwrap(),
            content,
            master,
            refused_before: 0,
            applied_before: 0,
        }
    }

    /// The API stops and starts again over the same database, on the same
    /// port, with a link made anew from what the database holds.
    pub async fn restart_api(&mut self) {
        let port = self.api.port;
        self.api.stop().await;
        self.refused_before += self.api.applier.refused.load(Ordering::SeqCst);
        self.applied_before += self.api.applier.applied.load(Ordering::SeqCst);
        self.api = start_api(
            &self.db,
            &self.clock,
            &self.secrets,
            &self.config,
            self.content.clone(),
            self.master,
            Some(port),
        )
        .await;
    }

    /// The API goes away, and the gateway is left holding its sessions.
    pub async fn api_goes_away(&mut self) {
        self.api.stop().await;
        self.refused_before += self.api.applier.refused.load(Ordering::SeqCst);
        self.applied_before += self.api.applier.applied.load(Ordering::SeqCst);
    }

    /// The API comes back, on the same port, over the same database.
    pub async fn api_comes_back(&mut self) {
        let port = self.api.port;
        self.api = start_api(
            &self.db,
            &self.clock,
            &self.secrets,
            &self.config,
            self.content.clone(),
            self.master,
            Some(port),
        )
        .await;
    }

    /// The gateway stops and starts again, on the same ports.
    pub async fn restart_gateway(&mut self) {
        let (pinned, edge) = (self.gateway.pinned, self.gateway.edge);
        self.gateway.stop().await;
        self.gateway = start_gateway(
            &self.clock,
            &self.secrets,
            self.api.port,
            Some(pinned),
            Some(edge),
            None,
        )
        .await;
        self.gateway.gateway.wait_for_backplane().await;
    }

    /// The gateway stops and starts again, on the same ports, showing `identity`.
    pub async fn restart_gateway_with(&mut self, identity: TlsIdentity) {
        let (pinned, edge) = (self.gateway.pinned, self.gateway.edge);
        self.gateway.stop().await;
        self.gateway = start_gateway(
            &self.clock,
            &self.secrets,
            self.api.port,
            Some(pinned),
            Some(edge),
            Some(identity),
        )
        .await;
        self.gateway.gateway.wait_for_backplane().await;
    }

    /// A gateway certificate of this deployment's authority issued `days_ago` days ago for a year.
    pub fn gateway_identity_issued(&self, days_ago: i64) -> TlsIdentity {
        let then =
            orch_types::ManualClock::new(chrono::Utc::now() - chrono::Duration::days(days_ago));
        let (key, certificate) = self
            .secrets
            .authority()
            .issue_with_new_key(
                &Holder::Service {
                    service: ServiceName::Gateway,
                    names: NAMES.iter().map(|name| (*name).to_owned()).collect(),
                },
                365,
                &then,
            )
            .unwrap();
        TlsIdentity {
            chain: vec![certificate],
            key,
        }
    }

    /// A token for one machine, made as the system makes them.
    pub async fn token(&self) -> String {
        self.api
            .link
            .certificates()
            .issue_token(&TokenRequest {
                created_by: self.user,
                purpose: TokenFor::Machine,
                name_hint: None,
            })
            .await
            .unwrap()
            .token
    }

    /// A machine enrolls through the pinned port, trusting only the pin its
    /// token carries.
    pub async fn enroll(&self, name: &str) -> Enrolled {
        let token_text = self.token().await;
        self.enroll_with(&token_text, name).await.unwrap()
    }

    pub async fn enroll_with(
        &self,
        token_text: &str,
        name: &str,
    ) -> Result<Enrolled, (u16, String)> {
        let token: EnrollmentToken = token_text.parse().unwrap();
        let pending = PendingEnrollment::start().unwrap();
        let request = EnrollRequest {
            machine_id: None,
            token: token_text.to_owned(),
            csr_pem: pending.csr_pem().to_owned(),
            name: name.to_owned(),
            platform: Platform {
                os: Os::Linux,
                arch: Arch::X86_64,
            },
            connector_version: "0.1.0".to_owned(),
            protocol_versions: vec![1],
            roles: vec![Role::Agent, Role::Environment],
        };
        let tls = pinned_client_config(
            ServerTrust::Pin(*token.pin()),
            Some(ServiceName::Gateway),
            None,
            Arc::clone(&self.clock),
        )
        .unwrap();
        let (status, body) = post_json(
            self.gateway.pinned,
            tls,
            ENROLL_PATH,
            &serde_json::to_string(&request).unwrap(),
            &[],
        )
        .await;
        if status != 201 {
            return Err((status, String::from_utf8_lossy(&body).into_owned()));
        }
        let reply: EnrollReply = serde_json::from_slice(&body).unwrap();
        let identity = pending
            .complete(&token, &reply, self.clock.as_ref())
            .unwrap();
        Ok(Enrolled { identity, reply })
    }

    /// The client configuration with which an enrolled machine reaches the
    /// pinned port.
    pub fn pinned_config(&self, enrolled: &Enrolled) -> Arc<ClientConfig> {
        pinned_client_config(
            ServerTrust::Authority(Box::new(self.secrets.ca_certificate())),
            Some(ServiceName::Gateway),
            Some(&enrolled.identity.tls().unwrap()),
            Arc::clone(&self.clock),
        )
        .unwrap()
    }

    /// An enrolled machine starts its link, trying the endpoints it was given
    /// in their order.
    pub fn start_machine(&self, enrolled: &Enrolled, endpoints: Vec<Endpoint>) -> Machine {
        self.start_machine_as(enrolled, endpoints, vec![Role::Agent, Role::Environment])
    }

    /// As [`World::start_machine`], with the roles the machine switches on.
    pub fn start_machine_as(
        &self,
        enrolled: &Enrolled,
        endpoints: Vec<Endpoint>,
        roles: Vec<Role>,
    ) -> Machine {
        let dir = self.machines_dir.path().join(format!(
            "{}-{}",
            enrolled.identity.machine_id,
            orch_types::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        self.start_machine_in(enrolled, endpoints, roles, &dir)
    }

    pub fn start_machine_in(
        &self,
        enrolled: &Enrolled,
        endpoints: Vec<Endpoint>,
        roles: Vec<Role>,
        dir: &Path,
    ) -> Machine {
        let public = public_client_config(
            root_store(&[self.secrets.ca_certificate()]).unwrap(),
            Arc::clone(&self.clock),
        )
        .unwrap();
        let dialer = TlsDialer::new(Some(self.pinned_config(enrolled)), Some(public));
        let link = MachineLink::start(
            MachineConfig {
                hosted_endpoints: None,
                follow_system_endpoints: false,
                machine_id: enrolled.identity.machine_id,
                connector_version: "0.1.0".to_owned(),
                platform: Platform {
                    os: Os::Linux,
                    arch: Arch::X86_64,
                },
                roles,
                proof: Some((
                    enrolled.identity.key.duplicate().unwrap(),
                    enrolled.identity.certificate.clone(),
                )),
                endpoints,
                data_dir: dir.to_owned(),
                spool: SpoolOptions {
                    max_bytes: 256 * 1024 * 1024,
                    max_age: Duration::from_secs(7 * 86_400),
                    segment_bytes: 8 * 1024 * 1024,
                },
                journal_keep: Duration::from_secs(24 * 3_600),
                connected: true,
                tunnels: TunnelOptions::default(),
                tunnel_gauge: None,
            },
            Arc::new(dialer),
            Arc::clone(&self.clock),
        )
        .unwrap();
        let handler = Arc::new(ScriptedHandler::default());
        link.serve(handler.clone() as Arc<dyn CommandHandler>);
        Machine {
            id: enrolled.identity.machine_id,
            link,
            handler,
        }
    }

    /// Stops the gateway and the API, and closes the pool. The database is
    /// dropped when the world is, on a thread that blocks this one; a
    /// connection that was being opened by a task of the API at that moment
    /// would stay half made, and the server would wait for it for as long as
    /// it gives a connection to authenticate, a minute, before it let the
    /// database go.
    pub async fn stop(&mut self) {
        self.gateway.stop().await;
        self.api.stop().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.db.pool().close().await;
    }
}

/// A machine that has enrolled.
pub struct Enrolled {
    pub identity: MachineIdentity,
    pub reply: EnrollReply,
}

impl Enrolled {
    pub fn endpoints(&self, trust: Trust) -> Vec<Endpoint> {
        self.reply
            .endpoints
            .iter()
            .filter(|endpoint| endpoint.trust == trust)
            .cloned()
            .collect()
    }
}

/// A machine's link, with what answers its commands.
pub struct Machine {
    pub id: Id,
    pub link: MachineLink,
    pub handler: Arc<ScriptedHandler>,
}

/// A port that nothing holds right now: found by binding and letting go.
pub fn reserve_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// One HTTPS request with a JSON body, over a connection of its own.
pub async fn post_json(
    address: SocketAddr,
    tls: Arc<ClientConfig>,
    path: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> (u16, Vec<u8>) {
    request(address, tls, "POST", path, Some(body), headers).await
}

pub async fn request(
    address: SocketAddr,
    tls: Arc<ClientConfig>,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> (u16, Vec<u8>) {
    try_request(address, tls, method, path, body, headers)
        .await
        .unwrap()
}

/// As [`request`], saying why when the request did not get an answer: the
/// connection was refused, or ended before one came.
pub async fn try_request(
    address: SocketAddr,
    tls: Arc<ClientConfig>,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> Result<(u16, Vec<u8>), String> {
    let socket = TcpStream::connect(address)
        .await
        .map_err(|e| e.to_string())?;
    let stream = TlsConnector::from(tls)
        .connect(ServerName::try_from("127.0.0.1").unwrap(), socket)
        .await
        .map_err(|e| e.to_string())?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(stream))
            .await
            .map_err(|e| e.to_string())?;
    tokio::spawn(connection);
    let mut builder = hyper::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("127.0.0.1:{}", address.port()));
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Full::new(Bytes::from(body.unwrap_or_default().to_owned())))
        .unwrap();
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?
        .to_bytes();
    Ok((status, bytes.to_vec()))
}
