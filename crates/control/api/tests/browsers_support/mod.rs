//! Browser tests use a real shipped-image peer, a fresh database and the existing in-memory machine hub.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(
    dead_code,
    reason = "each browser test uses some of this shared fixture"
)]

use crate::places::Who;
use crate::rig::Scene;
use futures::{SinkExt, StreamExt};
use orch_api_contract::browsers::*;
use orch_api_core::registry::Registry;
use orch_api_core::state::App;
use orch_link_contract::commands::{CommandError, CommandKind, CommandResult, ErrorCode};
use orch_link_contract::tunnels::{OpenErrorCode, Service};
use orch_types::{Id, Json};
use serde_json::json;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

pub mod fake;

/// Browser setup and the shared machine get generous bounded waits.
pub const WAIT: Duration = Duration::from_secs(60);

/// A private environment peer in a disposable shipped-image unit.
pub struct Peer {
    /// The kept mount used to prove cookie persistence across unit lifetimes.
    pub dir: tempfile::TempDir,
    /// Its private command and tunnel socket.
    pub path: PathBuf,
    name: String,
}

impl Peer {
    async fn start(id: Id) -> Option<Arc<Self>> {
        if std::env::var_os("ORCH_BROWSER_PEER_PROGRAM").is_none() {
            eprintln!(
                "skipped: build the static browser_peer fixture and set ORCH_BROWSER_PEER_PROGRAM"
            );
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        let peer = Arc::new(Self {
            path: dir.path().join("peer.sock"),
            dir,
            name: format!("orch-browser-peer-{}", orch_types::new_id().simple()),
        });
        peer.launch(id, 1).await;
        Some(peer)
    }

    async fn launch(&self, id: Id, generation: i64) {
        let binary = std::env::var_os("ORCH_BROWSER_PEER_PROGRAM").unwrap();
        let output = tokio::process::Command::new("docker")
            .args([
                "run",
                "--detach",
                "--name",
                &self.name,
                "--network",
                "none",
                "--init",
                "--ulimit",
                "core=0:-1",
                "--user",
                "1000:1000",
                "--mount",
            ])
            .arg(format!(
                "type=bind,src={},dst=/peer",
                self.dir.path().display()
            ))
            .arg("--mount")
            .arg(format!(
                "type=bind,src={},dst=/tmp/browser-peer,readonly",
                PathBuf::from(binary).display()
            ))
            .args([
                "orch-env-base:image-final",
                "/tmp/browser-peer",
                "/peer/peer.sock",
                &id.to_string(),
                &generation.to_string(),
            ])
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        tokio::time::timeout(WAIT, async {
            loop {
                if std::os::unix::net::UnixStream::connect(&self.path).is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
    }

    /// Starts the same unit again, or replaces it while keeping its home and workspace mount.
    pub async fn cycle(&self, id: Id, generation: i64, recreate: bool) {
        let status = tokio::process::Command::new("docker")
            .args(["stop", &self.name])
            .output()
            .await
            .unwrap();
        assert!(status.status.success());
        if recreate {
            let removed = tokio::process::Command::new("docker")
                .args(["rm", &self.name])
                .output()
                .await
                .unwrap();
            assert!(removed.status.success());
            self.launch(id, generation).await;
        } else {
            let started = tokio::process::Command::new("docker")
                .args(["start", &self.name])
                .output()
                .await
                .unwrap();
            assert!(started.status.success());
            tokio::time::timeout(WAIT, async {
                while std::os::unix::net::UnixStream::connect(&self.path).is_err() {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await
            .unwrap();
        }
    }

    /// Asks the native test peer for a command result or an observation.
    pub fn rpc(&self, value: Json) -> Json {
        let mut socket = std::os::unix::net::UnixStream::connect(&self.path).unwrap();
        socket.set_read_timeout(Some(WAIT)).unwrap();
        socket.write_all(format!("{value}\n").as_bytes()).unwrap();
        let mut line = String::new();
        std::io::BufReader::new(socket)
            .read_line(&mut line)
            .unwrap();
        serde_json::from_str(&line).unwrap()
    }
    /// Reads the fixture page to prove input reached it or was refused.
    pub fn observe(&self, browser: Id, expression: &str) -> Json {
        self.rpc(json!({"observe":browser,"expression":expression}))["ok"].clone()
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
    }
}

/// The API, database, scripted machine link and real environment used by browser scenarios.
pub struct Floor {
    /// People, resources and their access pages.
    pub scene: Scene,
    /// The actual browser process host.
    pub peer: Arc<Peer>,
    /// The environment under test.
    pub environment: Id,
    /// The composed application.
    pub app: App,
    server: Mutex<Option<std::net::SocketAddr>>,
}

impl Floor {
    /// Connects one fresh database to one real browser environment.
    pub async fn new() -> Option<Self> {
        let scene = Scene::new().await;
        let environment = scene.on.environment;
        let peer = Peer::start(environment).await?;
        let looked =
            peer.rpc(json!({"command":{"k":"screen.look","b":{"environment_id":environment}}}));
        let mut conn = scene.rig.db.acquire().await.unwrap();
        sqlx::query("UPDATE environments SET browser=true,vnc=true,terminal=true,status='running',placement='managed',facts=jsonb_set(jsonb_set(facts,'{browser}',$2),'{vnc}',$3) WHERE id=$1").bind(environment).bind(looked["ok"]["b"]["browser"].clone()).bind(looked["ok"]["b"]["vnc"].clone()).execute(&mut *conn).await.unwrap();
        drop(conn);
        for kind in [
            CommandKind::BrowserOpen,
            CommandKind::BrowserAdopt,
            CommandKind::BrowserClose,
            CommandKind::BrowserProfile,
            CommandKind::ScreenLook,
            CommandKind::DisplayOpen,
            CommandKind::DisplayAdopt,
            CommandKind::DisplayClose,
        ] {
            let peer = Arc::clone(&peer);
            scene.rig.machine.handler.on(kind, move |command| {
                let result = peer.rpc(json!({"command":command}));
                if let Some(error) = result.get("error") {
                    return Err(CommandError {
                        code: error["code"].as_str().unwrap().to_owned(),
                        detail: error["detail"].as_str().unwrap().to_owned(),
                        retryable: false,
                        details: error.get("details").cloned(),
                    });
                }
                serde_json::from_value::<CommandResult>(result["ok"].clone()).map_err(|_| {
                    CommandError::new(ErrorCode::Internal, "the peer reply was invalid")
                })
            });
        }
        let app = Arc::clone(&scene.rig.world.app.app);
        scene
            .rig
            .hub
            .set_ticket_verifier(app.link.tickets().verifier());
        app.realtime.browsers().through(scene.rig.hub.terminals());
        app.realtime.displays().through(scene.rig.hub.terminals());
        let mut incoming = scene.rig.machine.link.incoming_tunnels();
        let peer_for_tunnels = Arc::clone(&peer);
        tokio::spawn(async move {
            while let Some(incoming) = incoming.next().await {
                let peer = Arc::clone(&peer_for_tunnels);
                tokio::spawn(async move {
                    let request = match incoming.service.parse::<Service>() {
                        Ok(Service::Browser { browser_id }) => json!({"tunnel":browser_id}),
                        Ok(Service::Rfb { display_id }) => json!({"rfb":display_id}),
                        _ => {
                            incoming.refuse(OpenErrorCode::NoSuchService, "no such service");
                            return;
                        }
                    };
                    let Ok(mut local) = tokio::net::UnixStream::connect(&peer.path).await else {
                        return;
                    };
                    local
                        .write_all(format!("{request}\n").as_bytes())
                        .await
                        .unwrap();
                    let Ok(mut tunnel) = incoming.accept() else {
                        return;
                    };
                    let _ = tokio::io::copy_bidirectional(&mut local, &mut tunnel).await;
                });
            }
        });
        Some(Self {
            scene,
            peer,
            environment,
            app,
            server: Mutex::new(None),
        })
    }
    /// Opens or joins the named profile through HTTP.
    pub async fn open(&self, by: &Who, profile: &str) -> Browser {
        self.scene
            .rig
            .world
            .app
            .call(EnvironmentBrowserCreate, &[&self.environment])
            .token(&by.token)
            .body(&EnvironmentBrowserCreateBody {
                profile: profile.to_owned(),
                create: Some(true),
                preset: None,
                url: Some("http://127.0.0.1:41820/".to_owned()),
            })
            .send()
            .await
            .ok()
    }
    /// Mints a viewing or controlling ticket as the given person.
    pub async fn ticket(&self, by: &Who, browser: Id, control: bool) -> BrowserTicketCreateReply {
        self.scene
            .rig
            .world
            .app
            .call(BrowserTicketCreate, &[&browser])
            .token(&by.token)
            .body(&BrowserTicketCreateBody {
                control: Some(control),
            })
            .send()
            .await
            .ok()
    }
    /// Serves the composed routes on one reserved loopback port.
    pub async fn serve(&self) -> std::net::SocketAddr {
        if let Some(addr) = *self.server.lock().unwrap() {
            return addr;
        }
        self.app.realtime.start(self.scene.rig.engine.clone());
        self.app.realtime.listening().await;
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let router = orch_api_core::server::router(self.app.clone(), registry);
        let listener = loop {
            let port = rand::random_range(41000..=41999);
            if let Ok(l) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                break l;
            }
        };
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ =
                orch_api_core::server::serve_plain(listener, router, std::future::pending()).await;
        });
        *self.server.lock().unwrap() = Some(addr);
        addr
    }
    /// Attaches a real WebSocket client with its single-use ticket.
    pub async fn screen(&self, browser: Id, ticket: &str) -> Screen {
        let addr = self.serve().await;
        let mut request = format!("ws://{addr}/api/v1/ws/browsers/{browser}?ticket={ticket}")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("sec-websocket-protocol", "orch.browser.v1".parse().unwrap());
        let (socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        Screen { socket }
    }
    /// Verifies a reused ticket is rejected before an upgrade.
    pub async fn a_spent_ticket_is_refused(&self, browser: Id, ticket: &str) {
        let addr = self.serve().await;
        let mut request = format!("ws://{addr}/api/v1/ws/browsers/{browser}?ticket={ticket}")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("sec-websocket-protocol", "orch.browser.v1".parse().unwrap());
        match tokio_tungstenite::connect_async(request).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                assert_eq!(response.status(), 410)
            }
            result => panic!("a spent ticket was not refused: {result:?}"),
        }
    }
    /// Takes a permission entry back while a socket is alive.
    pub async fn removes(&self, entry: Id) {
        let mut conn = self.scene.rig.db.acquire().await.unwrap();
        orch_db::queries::access::remove_entry(
            &mut conn,
            entry,
            self.scene.ada.id(),
            self.scene.rig.world.now(),
        )
        .await
        .unwrap();
    }
    /// Names a person in one environment door and returns the entry for later revocation.
    pub async fn names(&self, who: &Who, permission: &str) -> Id {
        let mut conn = self.scene.rig.db.acquire().await.unwrap();
        orch_db::queries::access::testing::add_entry(
            &mut conn,
            &orch_db::queries::access::PageKey::new("environment", self.environment, "live"),
            "any",
            permission,
            ("user", Some(who.id()), None),
            None,
            self.scene.rig.world.now(),
        )
        .await
        .unwrap()
    }
}

impl Drop for Floor {
    fn drop(&mut self) {
        self.scene.rig.engine.stop();
        let realtime = self.app.realtime.clone();
        tokio::spawn(async move {
            realtime.stop().await;
        });
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.peer.name])
            .output();
    }
}

/// A real client whose picture acknowledgements are explicit.
pub struct Screen {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}
impl Screen {
    /// Sends one frame exactly as a browser client would.
    pub async fn send(&mut self, value: Json) {
        self.socket
            .send(Message::Text(value.to_string().into()))
            .await
            .unwrap();
    }
    /// Takes the requested metadata while acknowledging intervening pictures.
    pub async fn text(&mut self, op: &str) -> Json {
        tokio::time::timeout(WAIT, async {
            loop {
                match self.socket.next().await.unwrap().unwrap() {
                    Message::Text(text) => {
                        let value: Json = serde_json::from_str(&text).unwrap();
                        if value["op"] == "error" && op != "error" {
                            panic!("unexpected browser refusal: {value}");
                        }
                        if value["op"] == op {
                            return value;
                        }
                    }
                    Message::Binary(bytes) => {
                        let n = orch_realtime_contract::browser::PictureHeader::decode(&bytes)
                            .unwrap()
                            .n;
                        self.send(json!({"op":"ack","n":n})).await;
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap()
    }
    /// Takes one picture without acknowledging it, allowing tests to emulate a slow viewer.
    pub async fn picture(&mut self) -> bytes::Bytes {
        tokio::time::timeout(WAIT, async {
            loop {
                if let Message::Binary(bytes) = self.socket.next().await.unwrap().unwrap() {
                    return bytes;
                }
            }
        })
        .await
        .unwrap()
    }
    /// Acknowledges the supplied picture as drawn.
    pub async fn drawn(&mut self, bytes: &[u8]) {
        let n = orch_realtime_contract::browser::PictureHeader::decode(bytes)
            .unwrap()
            .n;
        self.send(json!({"op":"ack","n":n})).await;
    }
}
