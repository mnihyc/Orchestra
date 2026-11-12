//! The real executable forwards local MCP choices and prints the socket's actual receipt or refusal.
//! This socket peer is a named protocol double; controller/native unlink proof lives in gateway mcp_forget.
#![cfg(unix)]
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use orch_connector_core::local_socket::{Reply, Request, SOCKET_FILE};
use std::{path::Path, process::Output, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixListener,
    process::Command,
};

// Bounds a broken CLI/protocol exchange; it does not change the command's production deadline.
const PATIENCE: Duration = Duration::from_secs(30);

async fn run(dir: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_orch-connector"));
    command
        .kill_on_drop(true)
        .env_clear()
        .env("HOME", dir)
        .env("USERPROFILE", dir)
        .env("XDG_CONFIG_HOME", dir.join("config-home"))
        .env("XDG_DATA_HOME", dir.join("data-home"))
        .current_dir(dir)
        .arg("--data-dir")
        .arg(dir)
        .arg("mcp")
        .args(args);
    tokio::time::timeout(PATIENCE, command.output())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn the_real_mcp_cli_forwards_alias_and_forget_origin_and_prints_the_actual_local_answer() {
    let data = tempfile::tempdir().unwrap();
    let listener = UnixListener::bind(data.path().join(SOCKET_FILE)).unwrap();
    let expected = [
        (
            Request::McpImport {
                name: "native:name".into(),
                from: Some("fake".into()),
                place: None,
                as_name: Some("file-tools".into()),
            },
            Reply::Done {
                said: "Imported and switched on file-tools.".into(),
            },
        ),
        (
            Request::McpForget {
                name: "file-tools".into(),
                from: Some("fake".into()),
            },
            Reply::Done {
                said: "Forgot file-tools; its copy was removed from this machine.".into(),
            },
        ),
        (
            Request::McpForget {
                name: "file-tools".into(),
                from: None,
            },
            Reply::Error {
                message: "authz.denied: the current owner cannot manage this import".into(),
            },
        ),
    ];
    let peer = tokio::spawn(async move {
        for (request, reply) in expected {
            let (socket, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = socket.into_split();
            let line = BufReader::new(reader)
                .lines()
                .next_line()
                .await
                .unwrap()
                .unwrap();
            assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), request);
            let mut bytes = serde_json::to_vec(&reply).unwrap();
            bytes.push(b'\n');
            writer.write_all(&bytes).await.unwrap();
            writer.shutdown().await.unwrap();
        }
    });
    let imported = run(
        data.path(),
        &[
            "import",
            "native:name",
            "--from",
            "fake",
            "--as",
            "file-tools",
        ],
    )
    .await;
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    assert_eq!(
        String::from_utf8(imported.stdout).unwrap().trim(),
        "Imported and switched on file-tools."
    );
    let forgotten = run(data.path(), &["forget", "file-tools", "--from", "fake"]).await;
    assert!(
        forgotten.status.success(),
        "{}",
        String::from_utf8_lossy(&forgotten.stderr)
    );
    assert_eq!(
        String::from_utf8(forgotten.stdout).unwrap().trim(),
        "Forgot file-tools; its copy was removed from this machine."
    );
    let refused = run(data.path(), &["forget", "file-tools"]).await;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("authz.denied: the current owner cannot manage this import")
    );
    assert!(refused.stdout.is_empty());
    tokio::time::timeout(PATIENCE, peer).await.unwrap().unwrap();
}
