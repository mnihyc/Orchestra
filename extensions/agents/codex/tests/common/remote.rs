//! What a session whose tools are carried out in another container needs beside the rig: an exec-server of the
//! harness's own, run in a container of its own, and one listener that stands for the connector's, with the
//! path of the model and the tools going to the scripted server and the path of the executor going to the
//! exec-server.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::{image, remove_containers};

/// The loopback ports the tests may listen on.
const PORTS: std::ops::Range<u16> = 41700..42000;

/// The existing peer-start budget, including retries when another test takes a selected port.
const PEER_WAIT: Duration = Duration::from_secs(30);
/// The existing interval for noticing that the peer has started or exited.
const PEER_POLL: Duration = Duration::from_millis(100);

/// A free port of the range.
async fn listen_in_range() -> TcpListener {
    for port in PORTS {
        if let Ok(listener) = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await {
            return listener;
        }
    }
    panic!("no free port in 41700..42000");
}

/// A Codex exec-server in a container, serving the workspace it is given and nothing else of the machine, and
/// requiring the token it is given.
pub struct ExecServer {
    name: String,
    port: u16,
    _token: TempDir,
}

impl ExecServer {
    /// Starts it and waits until it says where it listens.
    pub async fn start(label: &str, workspace: &Path, token: &str) -> Self {
        let directory = tempfile::Builder::new()
            .prefix("orch-codex-exec-token-")
            .tempdir()
            .unwrap();
        let file = directory.path().join("token");
        std::fs::write(&file, token).unwrap();
        let name = format!("orch-test-codex-{label}-{}-exec", orch_types::new_id());
        let mut server = Self {
            name,
            port: 0,
            _token: directory,
        };
        // Only the server's own announcement proves it bound the selected port. Another test can take that
        // port after the probe is dropped; in that case this container exits and a fresh attempt stays within
        // the same peer-start budget and allowed loopback range.
        let ready = tokio::time::timeout(PEER_WAIT, async {
            loop {
                let listener = listen_in_range().await;
                server.port = listener.local_addr().unwrap().port();
                drop(listener);
                let output = tokio::process::Command::new("docker")
                    .args([
                        "run",
                        "-d",
                        "--rm",
                        "--name",
                        &server.name,
                        "--network",
                        "host",
                    ])
                    .arg("-v")
                    .arg(format!("{0}:{0}", workspace.display()))
                    .arg("-v")
                    .arg(format!("{0}:{0}:ro", server._token.path().display()))
                    .arg(image())
                    .args(["codex", "exec-server", "--listen"])
                    .arg(format!("ws://{}", server.addr()))
                    .args(["--ws-auth", "capability-token", "--ws-token-file"])
                    .arg(&file)
                    .kill_on_drop(true)
                    .output()
                    .await
                    .unwrap();
                assert!(
                    output.status.success(),
                    "the exec-server did not start: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                loop {
                    if announced(&server.name).await == Some(server.port)
                        && TcpStream::connect(server.addr()).await.is_ok()
                    {
                        return;
                    }
                    let state = tokio::process::Command::new("docker")
                        .args(["inspect", "--format", "{{.State.Running}}", &server.name])
                        .kill_on_drop(true)
                        .output()
                        .await
                        .unwrap();
                    if !state.status.success() || state.stdout != b"true\n" {
                        remove_containers(std::slice::from_ref(&server.name));
                        // --rm can still be releasing the name after the stopped state was seen.
                        // Reuse it only after absence, within the same peer-start deadline.
                        loop {
                            let gone = tokio::process::Command::new("docker")
                                .args(["inspect", &server.name])
                                .kill_on_drop(true)
                                .output()
                                .await
                                .unwrap();
                            if !gone.status.success() {
                                break;
                            }
                            tokio::time::sleep(PEER_POLL).await;
                        }
                        break;
                    }
                    tokio::time::sleep(PEER_POLL).await;
                }
            }
        })
        .await;
        assert!(
            ready.is_ok(),
            "the exec-server {} named no address it listens on",
            server.name
        );
        server
    }

    /// Where it listens.
    pub fn addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.port))
    }
}

impl Drop for ExecServer {
    fn drop(&mut self) {
        remove_containers(std::slice::from_ref(&self.name));
    }
}

/// The port the exec-server in container `name` named on its first line, once it has: `ws://127.0.0.1:<port>`.
async fn announced(name: &str) -> Option<u16> {
    let logs = tokio::process::Command::new("docker")
        .args(["logs", name])
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    String::from_utf8_lossy(&logs.stdout)
        .lines()
        .next()?
        .trim()
        .strip_prefix("ws://127.0.0.1:")?
        .trim_end_matches('/')
        .parse::<u16>()
        .ok()
        .filter(|port| PORTS.contains(port))
}

/// One listener for the model, the tools and the executor of the sessions of a harness.
pub struct SplitProxy {
    addr: SocketAddr,
    task: JoinHandle<()>,
    exec_connections: Arc<AtomicUsize>,
}

impl SplitProxy {
    /// Listens; a connection whose first request is for `/exec/…` goes to `exec`, any other to `models`.
    pub async fn start(models: SocketAddr, exec: SocketAddr) -> Self {
        let listener = listen_in_range().await;
        let addr = listener.local_addr().unwrap();
        let exec_connections = Arc::new(AtomicUsize::new(0));
        let seen = exec_connections.clone();
        let task = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                tokio::spawn(route(client, models, exec, seen.clone()));
            }
        });
        Self {
            addr,
            task,
            exec_connections,
        }
    }

    /// The port it listens on.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Actual executor WebSocket connections routed separately from model requests.
    pub fn exec_connections(&self) -> usize {
        self.exec_connections.load(Ordering::Relaxed)
    }
}

impl Drop for SplitProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Reads the head of the first request of a connection, chooses the upstream by its path and hands the
/// connection over.
async fn route(
    mut client: TcpStream,
    models: SocketAddr,
    exec: SocketAddr,
    exec_connections: Arc<AtomicUsize>,
) {
    let mut head = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        let Ok(read) = client.read(&mut chunk).await else {
            return;
        };
        if read == 0 {
            return;
        }
        head.extend_from_slice(&chunk[..read]);
        if let Some(at) = head.windows(4).position(|window| window == b"\r\n\r\n") {
            break at + 4;
        }
        if head.len() > 64 * 1024 {
            return;
        }
    };
    let line_end = head.iter().position(|byte| *byte == b'\r').unwrap_or(end);
    let line = String::from_utf8_lossy(&head[..line_end]).into_owned();
    let mut parts = line.splitn(3, ' ');
    let (method, target, version) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    let (upstream, first) = if target.starts_with("/exec/") {
        exec_connections.fetch_add(1, Ordering::Relaxed);
        // The exec-server takes its WebSocket at `/` only: the prefix of the session's key is the listener's.
        let mut first = format!("{method} / {version}").into_bytes();
        first.extend_from_slice(&head[line_end..]);
        (exec, first)
    } else {
        (models, head)
    };
    let Ok(mut upstream) = TcpStream::connect(upstream).await else {
        return;
    };
    if upstream.write_all(&first).await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}
