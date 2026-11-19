//! A unit leaves no core file when a process crashes and a shell may raise its own core limit.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::Lab;
use serde_json::json;

#[tokio::test]
async fn a_crash_leaves_no_memory_file_and_a_debugging_shell_can_raise_its_limit() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let spec = lab.request("environment", "core", |request| {
        request["env"] = json!({"LAB_RUN": "mkdir -p /workspace/core-check; (cd /workspace/core-check; sh -c 'kill -ABRT $$') 2>/dev/null; printf 'soft=%s\\n' \"$(ulimit -c)\"; printf 'files=%s\\n' \"$(find /workspace/core-check -type f | wc -l)\"; ulimit -c unlimited; printf 'raised=%s\\n' \"$(ulimit -c)\""});
    });
    let unit = lab.run(&spec).await;
    let result = lab.read(&unit, "/var/lib/orch/run.txt").await.unwrap();
    assert!(result.contains("soft=0\n"), "{result}");
    assert!(result.contains("files=0\n"), "{result}");
    assert!(result.contains("raised=unlimited\n"), "{result}");
}
