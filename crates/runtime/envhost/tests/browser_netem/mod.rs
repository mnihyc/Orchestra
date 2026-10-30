//! An isolated network namespace applies the real delay, bandwidth and packet-loss profile.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

/// The shaped connection stays on loopback in its unit, in the reserved service range.
pub const PORT: u16 = 41321;
/// Building a tiny network-control image has generous room on the shared machine.
const BUILD_WAIT: Duration = Duration::from_secs(600);
/// The real browser scenario includes recovery and a still-image check.
const RUN_WAIT: Duration = Duration::from_secs(600);
/// Marker polling does not busy-loop on either the host or its unit.
const POLL: Duration = Duration::from_millis(100);

struct Unit(String);
impl Drop for Unit {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "--force", &self.0])
            .output();
    }
}

async fn tc(unit: &str, args: &[&str]) {
    let status = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--cap-add",
            "NET_ADMIN",
            "--network",
            &format!("container:{unit}"),
            "orch-browser-netem:check",
        ])
        .args(args)
        .output()
        .await
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
}

/// Runs exactly one real scenario while a short-lived privileged sidecar shapes only its test socket.
pub async fn inside(name: &str) -> bool {
    if std::env::var_os("ORCH_BROWSER_IN_UNIT").is_some() {
        return true;
    }
    let binary = std::env::var_os("ORCH_BROWSER_TEST_PROGRAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_exe().unwrap());
    if !binary
        .to_string_lossy()
        .contains("x86_64-unknown-linux-musl")
    {
        eprintln!("skipped: set ORCH_BROWSER_TEST_PROGRAM to the static fixture");
        return false;
    }
    let local = std::env::current_dir().unwrap();
    let dockerfile = if local.join("tests/browser_netem").is_dir() {
        local.join("tests/browser_netem")
    } else {
        local.join("crates/runtime/envhost/tests/browser_netem")
    };
    let built = tokio::time::timeout(
        BUILD_WAIT,
        Command::new("docker")
            .args(["build", "--tag", "orch-browser-netem:check"])
            .arg(dockerfile)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let markers = tempfile::tempdir().unwrap();
    let unit = format!("orch-browser-netem-{}", orch_types::new_id().simple());
    let status = Command::new("docker")
        .args([
            "run",
            "--detach",
            "--name",
            &unit,
            "--network",
            "none",
            "--init",
            "--ulimit",
            "core=0:-1",
            "--user",
            "1000:1000",
            "--env",
            "ORCH_BROWSER_IN_UNIT=1",
            "--env",
            "ORCH_BROWSER_SHAPE_DIR=/shape",
            "--mount",
        ])
        .arg(format!(
            "type=bind,src={},dst=/tmp/browser-tests,readonly",
            binary.display()
        ))
        .arg("--mount")
        .arg(format!(
            "type=bind,src={},dst=/shape",
            markers.path().display()
        ))
        .args([
            "orch-env-base:image-final",
            "/tmp/browser-tests",
            "--exact",
            name,
            "--nocapture",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let _cleanup = Unit(unit.clone());
    let run = async {
        while !markers.path().join("ready").exists() {
            tokio::time::sleep(POLL).await;
        }
        // Loopback normally aggregates very large packets. Ethernet-sized packets give loss the same meaning as the real network path.
        let mtu = Command::new("docker")
            .args([
                "run",
                "--rm",
                "--cap-add",
                "NET_ADMIN",
                "--network",
                &format!("container:{unit}"),
                "--entrypoint",
                "ip",
                "orch-browser-netem:check",
                "link",
                "set",
                "dev",
                "lo",
                "mtu",
                "1500",
            ])
            .output()
            .await
            .unwrap();
        assert!(mtu.status.success());
        // The unselected band leaves the browser's own local protocol and its fixture page alone.
        tc(
            &unit,
            &[
                "qdisc", "add", "dev", "lo", "root", "handle", "1:", "prio", "bands", "3",
                "priomap", "0", "0", "0", "0", "0", "0", "0", "0", "0", "0", "0", "0", "0", "0",
                "0", "0",
            ],
        )
        .await;
        tc(
            &unit,
            &[
                "qdisc", "add", "dev", "lo", "parent", "1:2", "handle", "20:", "netem", "delay",
                "150ms", "loss", "1%", "rate", "750kbit",
            ],
        )
        .await;
        tc(
            &unit,
            &[
                "qdisc", "add", "dev", "lo", "parent", "1:3", "handle", "30:", "netem", "delay",
                "150ms", "loss", "1%", "rate", "1600kbit",
            ],
        )
        .await;
        tc(
            &unit,
            &[
                "filter",
                "add",
                "dev",
                "lo",
                "protocol",
                "ip",
                "parent",
                "1:",
                "prio",
                "1",
                "u32",
                "match",
                "ip",
                "dport",
                &PORT.to_string(),
                "0xffff",
                "flowid",
                "1:2",
            ],
        )
        .await;
        tc(
            &unit,
            &[
                "filter",
                "add",
                "dev",
                "lo",
                "protocol",
                "ip",
                "parent",
                "1:",
                "prio",
                "2",
                "u32",
                "match",
                "ip",
                "sport",
                &PORT.to_string(),
                "0xffff",
                "flowid",
                "1:3",
            ],
        )
        .await;
        std::fs::write(markers.path().join("go"), "").unwrap();
        while !markers.path().join("floor").exists() {
            tokio::time::sleep(POLL).await;
        }
        tc(
            &unit,
            &[
                "qdisc", "change", "dev", "lo", "parent", "1:2", "handle", "20:", "netem", "delay",
                "150ms", "loss", "1%", "rate", "75kbit",
            ],
        )
        .await;
        tc(
            &unit,
            &[
                "qdisc", "change", "dev", "lo", "parent", "1:3", "handle", "30:", "netem", "delay",
                "150ms", "loss", "1%", "rate", "160kbit",
            ],
        )
        .await;
        while !markers.path().join("free").exists() {
            tokio::time::sleep(POLL).await;
        }
        tc(&unit, &["qdisc", "del", "dev", "lo", "root"]).await;
        let waited = Command::new("docker")
            .args(["wait", &unit])
            .output()
            .await
            .unwrap();
        let logs = Command::new("docker")
            .args(["logs", &unit])
            .output()
            .await
            .unwrap();
        eprintln!(
            "{}\n{}",
            String::from_utf8_lossy(&logs.stdout),
            String::from_utf8_lossy(&logs.stderr)
        );
        assert!(
            String::from_utf8_lossy(&logs.stdout).contains("running 1 test\n"),
            "the static fixture lacks {name}; rebuild it before running this scenario"
        );
        assert_eq!(String::from_utf8_lossy(&waited.stdout).trim(), "0");
    };
    let outcome = tokio::time::timeout(RUN_WAIT, run).await;
    let _ = Command::new("docker")
        .args(["rm", "--force", &unit])
        .output()
        .await;
    assert!(
        outcome.is_ok(),
        "the shaped browser scenario did not finish"
    );
    false
}

/// Waits until the sidecar configured the private network, then asks it to restore the fast path later.
pub async fn begin() {
    let root = PathBuf::from(std::env::var_os("ORCH_BROWSER_SHAPE_DIR").unwrap());
    std::fs::write(root.join("ready"), "").unwrap();
    while !root.join("go").exists() {
        tokio::time::sleep(POLL).await;
    }
}

/// Begins the measured profile after the browser and the shaped socket are ready.
pub fn start() {
    let root = PathBuf::from(std::env::var_os("ORCH_BROWSER_SHAPE_DIR").unwrap());
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        std::fs::write(root.join("floor"), "").unwrap();
        tokio::time::sleep(Duration::from_secs(20)).await;
        std::fs::write(root.join("free"), "").unwrap();
    });
}
