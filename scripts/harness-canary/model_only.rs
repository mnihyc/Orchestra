//! A harness in a networkless container reaches only the explicitly supplied model through a Unix socket relay.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "each native test uses part of this helper and fails by panicking"
)]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use orch_types::new_id;
use serde_json::{Value, json};
use tokio::net::{TcpStream, UnixListener};
use tokio::process::Command;

/// A Unix relay reserved for this check and the containers started through it.
pub struct ModelOnly {
    directory: tempfile::TempDir,
    socket: PathBuf,
    address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
    names: Mutex<Vec<String>>,
}

impl ModelOnly {
    /// Gives networkless containers access to this one controlled service.
    pub async fn new(address: SocketAddr) -> Arc<Self> {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let socket = directory.path().join("model.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o666)).unwrap();
        let task = tokio::spawn(async move {
            while let Ok((mut unix, _)) = listener.accept().await {
                tokio::spawn(async move {
                    if let Ok(mut tcp) = TcpStream::connect(address).await {
                        let _ = tokio::io::copy_bidirectional(&mut unix, &mut tcp).await;
                    }
                });
            }
        });
        Arc::new(Self {
            directory,
            socket,
            address,
            task,
            names: Mutex::new(Vec::new()),
        })
    }

    /// A command that starts only this test's container; environment values do not become command arguments.
    pub fn command(
        &self,
        image: &str,
        program: &str,
        arguments: &[String],
        environment: &BTreeMap<String, String>,
        mounts: &[PathBuf],
        cwd: Option<&Path>,
    ) -> (Command, String) {
        let name = format!("orch-conformance-{}", new_id().simple());
        self.names.lock().unwrap().push(name.clone());
        let mut command = Command::new("docker");
        command.args(["run", "-i", "--rm", "--name", &name, "--network", "none"]);
        command
            .arg("-v")
            .arg(format!("{0}:{0}", self.directory.path().display()));
        for path in mounts {
            command.arg("-v").arg(format!("{0}:{0}", path.display()));
        }
        if let Some(cwd) = cwd {
            command.arg("-w").arg(cwd);
        }
        for (key, value) in environment {
            // Host-local login/search paths cannot replace the container's own.
            if ![
                "PATH",
                "HOME",
                "USER",
                "LOGNAME",
                "SHELL",
                "TMPDIR",
                "XDG_RUNTIME_DIR",
                "TERM",
            ]
            .contains(&key.as_str())
            {
                command.arg("-e").arg(key);
                command.env(key, value);
            }
        }
        command
            .arg(image)
            .args(["node", "-e", RELAY, "--"])
            .arg(&self.socket)
            .arg(self.address.port().to_string())
            .arg(program)
            .args(arguments);
        (command, name)
    }

    /// Removes only a container created by this helper.
    pub fn kill(&self, name: &str) {
        if self.names.lock().unwrap().iter().any(|owned| owned == name) {
            let _ = std::process::Command::new("docker")
                .args(["rm", "-f", name])
                .output();
        }
    }
}

impl Drop for ModelOnly {
    fn drop(&mut self) {
        self.task.abort();
        for name in self.names.lock().unwrap().iter() {
            let _ = std::process::Command::new("docker")
                .args(["rm", "-f", name])
                .output();
        }
    }
}

// The helper creates no Internet interface. The only TCP listener is in the container's own loopback;
// every accepted connection goes to the one mounted Unix socket, whose peer is the controlled service.
const RELAY: &str = r#"const net=require('net'),cp=require('child_process');
const [socket,port,program,...args]=process.argv.slice(1);
const server=net.createServer(local=>{
  const peer=net.createConnection(socket); peer.on('error',()=>local.destroy());
  local.on('error',()=>peer.destroy()); local.pipe(peer).pipe(local);
});
server.listen(Number(port),'127.0.0.1',()=>{
  const child=cp.spawn(program,args,{stdio:'inherit',env:process.env});
  child.on('error',e=>{console.error(e.message);process.exit(1)});
  child.on('exit',code=>process.exit(code===null?1:code));
  process.on('SIGTERM',()=>child.kill('SIGTERM'));
});"#;

/// Records safe native evidence and prints one machine-readable line for the canary.
pub fn record(adapter: &str, case: &str, status: &str, version: &str, details: Value) {
    let evidence = json!({"adapter":adapter,"case":case,"status":status,"harness_version":version,
        "network":"none; controlled service through a Unix socket", "details":details});
    if let Ok(directory) = std::env::var("ORCH_CONFORMANCE_RECORD") {
        let path = PathBuf::from(directory);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join(format!("{adapter}-{case}.json")),
            serde_json::to_vec_pretty(&evidence).unwrap(),
        )
        .unwrap();
    }
    println!("CONFORMANCE {status} {adapter} {case} {version}");
}
