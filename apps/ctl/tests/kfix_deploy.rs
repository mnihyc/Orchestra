//! Deployment credentials are named when their file modes expose them to another account.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn doctor_names_each_exposed_password_or_secret_and_passes_after_the_owner_protects_them() {
    let directory = tempfile::tempdir().unwrap();
    let env = directory.path().join(".env");
    let secrets = directory.path().join("config/secrets");
    std::fs::create_dir_all(&secrets).unwrap();
    let secret = secrets.join("orch_master_key");
    for file in [&env, &secret] {
        std::fs::write(file, "credential-that-must-never-be-printed").unwrap();
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_orchctl"))
            .args(["doctor", "--only", "secrets", "--env-file"])
            .arg(&env)
            .arg("--secrets-dir")
            .arg(&secrets)
            .arg("--config")
            .arg(directory.path().join("absent.toml"))
            .output()
            .unwrap()
    };
    let exposed = run();
    assert_eq!(exposed.status.code(), Some(1));
    let report = String::from_utf8(exposed.stdout).unwrap();
    assert!(report.contains("FAIL  secrets"), "{report}");
    assert!(report.contains(&env.display().to_string()), "{report}");
    assert!(report.contains(&secret.display().to_string()), "{report}");
    assert!(!report.contains("credential-that-must-never-be-printed"));
    for file in [&env, &secret] {
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let private = run();
    assert!(private.status.success());
    let report = String::from_utf8(private.stdout).unwrap();
    assert!(report.contains("ok    secrets"), "{report}");
}
