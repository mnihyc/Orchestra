//! The installed connector's real command preserves shared programs and borrowed work across purge choices.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn program(root: &Path) -> PathBuf {
    let path = root.join(if cfg!(windows) {
        "orch-connector.exe"
    } else {
        "orch-connector"
    });
    std::fs::copy(env!("CARGO_BIN_EXE_orch-connector"), &path).unwrap();
    path
}

fn run(program: &Path, data: &Path, arguments: &[&str]) -> Output {
    Command::new(program)
        .arg("--data-dir")
        .arg(data)
        .args(arguments)
        .output()
        .unwrap()
}

fn instance(program: &Path, data: &Path, name: &str) {
    let directory = orch_connector_core::enrollment::DataDir::open(data).unwrap();
    std::fs::write(directory.identity_dir().join(orch_connector_core::enrollment::RECORD_FILE), serde_json::to_vec(&serde_json::json!({
        "machine_id":orch_types::new_id(),"name":name,"system":{"name":"Disposable system","public_url":"http://127.0.0.1:41751"},
        "pin":"public-fixture-pin","protocol_version":1,"enrolled_at":"2026-10-26T00:00:00.000Z","managed":false
    })).unwrap()).unwrap();
    let output = run(program, data, &["__record-installation", "--named"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn only_the_last_real_instance_removes_the_shared_program_and_each_data_choice_is_respected() {
    let root = tempfile::tempdir().unwrap();
    let program = program(root.path());
    let first = root.path().join("first instance");
    let second = root.path().join("second instance");
    let work = root.path().join("borrowed");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(work.join("keep"), b"borrowed work").unwrap();
    instance(&program, &first, "First machine");
    instance(&program, &second, "Second machine");
    let preview = run(&program, &first, &["uninstall", "--purge", "--plan"]);
    assert!(preview.status.success());
    assert!(program.exists() && first.exists() && second.exists());
    let output = run(&program, &first, &["uninstall", "--yes"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(program.exists() && first.exists() && second.exists());
    assert!(
        orch_connector_core::uninstall::inspect(&first)
            .unwrap()
            .unwrap()
            .uninstalled
    );
    let output = run(&program, &second, &["uninstall", "--purge", "--yes"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!program.exists() && !second.exists());
    assert!(first.exists());
    assert_eq!(std::fs::read(work.join("keep")).unwrap(), b"borrowed work");
}

#[cfg(target_os = "linux")]
#[test]
fn a_purge_removes_only_factory_owned_folders_and_a_replaced_folder_blocks_before_anything_is_removed()
 {
    use orch_link_contract::policy::LocalPolicy;
    for replaced in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let program = program(root.path());
        let data = root.path().join("connector data");
        let work = root.path().join("work");
        std::fs::create_dir(&work).unwrap();
        std::fs::write(work.join("borrowed"), b"keep").unwrap();
        instance(&program, &data, "Owned directory machine");
        std::fs::write(
            data.join("config.toml"),
            format!("[environment]\nroots = [{:?}]\n", work.to_str().unwrap()),
        )
        .unwrap();
        let policy = LocalPolicy {
            environment_roots: vec![work.to_string_lossy().into_owned()],
            ..Default::default()
        };
        let owned = orch_connector_core::storages::directory(
            &data,
            &Default::default(),
            &policy,
            orch_types::new_id(),
            "workspace",
            None,
        )
        .unwrap();
        std::fs::write(owned.join("owned"), b"remove").unwrap();
        if replaced {
            let moved = root.path().join("original");
            std::fs::rename(&owned, moved).unwrap();
            std::fs::create_dir(&owned).unwrap();
            std::fs::write(owned.join("replacement"), b"keep replaced data").unwrap();
        }
        let output = run(&program, &data, &["uninstall", "--purge", "--yes"]);
        assert_eq!(
            output.status.success(),
            !replaced,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(program.exists(), replaced);
        assert_eq!(data.exists(), replaced);
        assert_eq!(owned.exists(), replaced);
        assert_eq!(std::fs::read(work.join("borrowed")).unwrap(), b"keep");
        if replaced {
            assert_eq!(
                std::fs::read(owned.join("replacement")).unwrap(),
                b"keep replaced data"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn the_exact_uninstall_command_the_connector_reports_runs_with_its_named_directory() {
    let root = tempfile::tempdir().unwrap();
    let program = program(root.path());
    let data = root.path().join("a named instance's data");
    instance(&program, &data, "Named machine");
    let directory = orch_connector_core::enrollment::DataDir::inspect(&data).unwrap();
    let installation =
        orch_connector_core::installation::connector_with_program(&directory, &program);
    let output = Command::new("bash")
        .arg("-c")
        .arg(format!(
            "{} --purge --yes",
            installation.commands.uninstall.unwrap()
        ))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!program.exists() && !data.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn a_refused_named_service_stop_leaves_its_data_and_program_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let program = program(root.path());
    let data = root.path().join("service data");
    instance(&program, &data, "Service machine");
    assert!(
        run(
            &program,
            &data,
            &["__record-installation", "--named", "--service"]
        )
        .status
        .success()
    );
    let configuration = root.path().join("service configuration");
    let units = configuration.join("systemd/user");
    std::fs::create_dir_all(&units).unwrap();
    let service = orch_connector_core::service::Service::new(program.clone(), data.clone(), true);
    let unit = units.join(orch_connector_core::service::linux::unit_name(&service));
    std::fs::write(
        &unit,
        orch_connector_core::service::linux::unit_text(&service),
    )
    .unwrap();
    let commands = root.path().join("commands");
    std::fs::create_dir(&commands).unwrap();
    let manager = commands.join("systemctl");
    std::fs::write(&manager, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&manager, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(&program)
        .arg("--data-dir")
        .arg(&data)
        .args(["uninstall", "--purge", "--yes"])
        .env("XDG_CONFIG_HOME", &configuration)
        .env(
            "PATH",
            format!("{}:{}", commands.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(program.is_file() && unit.is_file());
    assert!(data.join("identity/enrollment.json").is_file());
    assert!(
        !orch_connector_core::uninstall::inspect(&data)
            .unwrap()
            .unwrap()
            .uninstalled
    );
}
