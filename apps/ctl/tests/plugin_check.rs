//! An operator receives a named conformance failure and a failing exit status from a broken program.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::process::Command;

#[test]
fn a_conforming_program_passes_and_a_careless_one_exits_nonzero_naming_its_rule() {
    for careless in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("plugin.py");
        orch_testkit::doubles::plugin::write_summarizer(&script, careless).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_orchctl"))
            .args(["plugin", "check", "--kind", "summarizer", "python3"])
            .arg(&script)
            .output()
            .unwrap();
        let printed = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            output.status.success(),
            !careless,
            "{printed}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if careless {
            assert!(
                printed.contains("FAILED") && printed.contains("transport.unknown_method"),
                "{printed}"
            );
        } else {
            assert!(
                printed.contains("summarizer.description") && printed.contains("0 failed"),
                "{printed}"
            );
        }
    }
}

#[test]
fn a_missing_program_cannot_be_reported_as_a_pass() {
    let output = Command::new(env!("CARGO_BIN_EXE_orchctl"))
        .args([
            "plugin",
            "check",
            "--kind",
            "driver",
            "/nonexistent/orch-plugin-conformance",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("transport.hello"));
}

#[cfg(target_os = "linux")]
#[test]
fn the_example_driver_is_checked_from_a_spec_file_and_a_wrong_reply_exits_nonzero() {
    let example = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/plugins/driver/driver.py");
    for wrong in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let spec = dir.path().join("spec.json");
        std::fs::write(&spec, r#"{"command":["sleep","600"]}"#).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_orchctl"));
        command
            .args(["plugin", "check", "--spec"])
            .arg(&spec)
            .arg("python3")
            .arg(&example)
            .arg("--base")
            .arg(dir.path().join("driver"));
        if wrong {
            command.arg("--wrong-reply");
        }
        let output = command.output().unwrap();
        let printed = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            output.status.success(),
            !wrong,
            "{printed}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(printed.contains("driver.create"), "{printed}");
    }
}
