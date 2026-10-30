//! The actual node executable previews, confirms and removes only its disposable installation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[cfg(target_os = "linux")]
fn unused_subnets(host: &str) -> [String; 2] {
    let list = Command::new("docker")
        .env("DOCKER_HOST", host)
        .args(["network", "ls", "-q"])
        .output()
        .unwrap();
    assert!(list.status.success());
    let names = String::from_utf8(list.stdout).unwrap();
    let mut occupied = Vec::new();
    if !names.trim().is_empty() {
        let read = Command::new("docker")
            .env("DOCKER_HOST", host)
            .args(["network", "inspect", "--format", "{{json .IPAM.Config}}"])
            .args(names.split_whitespace())
            .output()
            .unwrap();
        assert!(read.status.success());
        for line in String::from_utf8(read.stdout).unwrap().lines() {
            let configs: serde_json::Value = serde_json::from_str(line).unwrap();
            for config in configs.as_array().into_iter().flatten() {
                let Some((address, prefix)) =
                    config["Subnet"].as_str().and_then(|s| s.split_once('/'))
                else {
                    continue;
                };
                let Ok(address) = address.parse::<std::net::Ipv4Addr>() else {
                    continue;
                };
                let prefix: u32 = prefix.parse().unwrap();
                assert!(prefix <= 32);
                let mask = if prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - prefix)
                };
                let start = u32::from(address) & mask;
                occupied.push((start, start | !mask));
            }
        }
    }
    // Each isolated network has one unit; fourteen addresses leave room without using Docker's default pools.
    let start = u32::from(std::net::Ipv4Addr::new(10, 254, 0, 0));
    let subnets = (0..u32::from(u16::MAX))
        .step_by(16)
        .map(|offset| start + offset)
        .filter(|candidate| {
            !occupied
                .iter()
                .any(|(first, last)| *candidate <= *last && candidate + 15 >= *first)
        })
        .take(2)
        .map(|candidate| format!("{}/28", std::net::Ipv4Addr::from(candidate)))
        .collect::<Vec<_>>();
    subnets
        .try_into()
        .expect("two unused private subnets for the disposable project")
}

struct Installed {
    _root: tempfile::TempDir,
    program: PathBuf,
    data: PathBuf,
    config: PathBuf,
    borrowed: PathBuf,
}

impl Installed {
    fn new(configuration_owned: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join(if cfg!(windows) {
            "orch-node.exe"
        } else {
            "orch-node"
        });
        std::fs::copy(env!("CARGO_BIN_EXE_orch-node"), &program).unwrap();
        let data = root.path().join("node data");
        let directory = orch_connector_core::enrollment::DataDir::open(&data).unwrap();
        std::fs::write(directory.identity_dir().join(orch_connector_core::enrollment::RECORD_FILE), serde_json::to_vec(&serde_json::json!({
            "machine_id":orch_types::new_id(),"name":"Disposable node","system":{"name":"Disposable system","public_url":"http://127.0.0.1:41750"},
            "pin":"public-fixture-pin","protocol_version":1,"enrolled_at":"2026-10-26T00:00:00.000Z","managed":true
        })).unwrap()).unwrap();
        let config = root.path().join("node.toml");
        std::fs::write(&config, format!("version = 1\n[system]\nname = \"Disposable system\"\n[node]\ndata_dir = {:?}\ndrivers = []\n", data.to_str().unwrap())).unwrap();
        let borrowed = root.path().join("work");
        std::fs::create_dir(&borrowed).unwrap();
        std::fs::write(borrowed.join("keep"), b"not node data").unwrap();
        let installed = Self {
            _root: root,
            program,
            data,
            config,
            borrowed,
        };
        let mut arguments = vec!["__record-installation"];
        if configuration_owned {
            arguments.push("--configuration");
        }
        let output = installed.command(&arguments).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        installed
    }
    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(&self.program);
        command
            .arg("--config")
            .arg(&self.config)
            .arg("--data-dir")
            .arg(&self.data)
            .args(arguments);
        command
    }
    fn run(&self, arguments: &[&str]) -> Output {
        self.command(arguments).output().unwrap()
    }
}

#[test]
fn a_preview_and_a_wrong_name_change_no_owned_data_or_supplied_work() {
    let installed = Installed::new(true);
    let receipt = installed.data.join(orch_connector_core::uninstall::RECEIPT);
    let before = std::fs::read(&receipt).unwrap();
    let output = installed.run(&["uninstall", "--purge", "--plan"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Uninstall plan: Disposable node"));
    assert_eq!(std::fs::read(&receipt).unwrap(), before);
    assert!(installed.program.is_file() && installed.config.is_file());
    let mut child = installed
        .command(&["uninstall", "--purge"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"Another node\n")
        .unwrap();
    assert!(!child.wait_with_output().unwrap().status.success());
    assert_eq!(std::fs::read(&receipt).unwrap(), before);
    assert_eq!(
        std::fs::read(installed.borrowed.join("keep")).unwrap(),
        b"not node data"
    );
}

#[test]
fn purge_does_not_erase_a_supplied_configuration_kept_inside_its_data_directory() {
    let mut installed = Installed::new(false);
    let inside = installed.data.join("operator.toml");
    std::fs::rename(&installed.config, &inside).unwrap();
    installed.config = inside;
    assert!(installed.run(&["__record-installation"]).status.success());
    let before = std::fs::read(&installed.config).unwrap();
    let preview = installed.run(&["uninstall", "--purge", "--plan"]);
    assert!(preview.status.success());
    assert!(String::from_utf8_lossy(&preview.stdout).contains("move the supplied configuration"));
    let refused = installed.run(&["uninstall", "--purge", "--yes"]);
    assert!(!refused.status.success());
    assert_eq!(std::fs::read(&installed.config).unwrap(), before);
    assert!(installed.program.is_file());
    assert!(installed.data.join("identity/enrollment.json").is_file());
}

#[test]
fn uninstall_keeps_data_without_purge_and_purge_removes_it_while_supplied_configuration_stays() {
    for (configuration_owned, purge) in [(false, false), (false, true), (true, false), (true, true)]
    {
        let installed = Installed::new(configuration_owned);
        let output = if purge {
            installed.run(&["uninstall", "--purge", "--yes"])
        } else {
            installed.run(&["uninstall", "--yes"])
        };
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!installed.program.exists());
        assert_eq!(installed.data.exists(), !purge);
        assert_eq!(installed.config.is_file(), !configuration_owned);
        assert_eq!(
            std::fs::read(installed.borrowed.join("keep")).unwrap(),
            b"not node data"
        );
    }
}

#[test]
fn an_active_instance_blocks_file_purge_and_keeps_the_receipt_for_a_retry() {
    let installed = Installed::new(true);
    let _running = orch_connector_core::uninstall::InstanceLock::running(&installed.data).unwrap();
    let output = installed.run(&["uninstall", "--purge", "--yes"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("still running"));
    for path in [&installed.program, &installed.config, &installed.data] {
        assert!(Path::new(path).exists());
    }
    assert!(
        installed
            .data
            .join(orch_connector_core::uninstall::RECEIPT)
            .is_file()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_real_compose_project_proves_runtime_preservation_and_exact_purge_while_another_system_keeps_running()
 {
    for purge in [false, true] {
        let installed = Installed::new(false);
        let project = format!("orchcustody{}", orch_types::new_id().simple());
        let foreign = format!("{project}-other");
        let compose = installed._root.path().join("compose.yml");
        let socket = std::env::var("ORCH_TEST_DOCKER_SOCKET")
            .unwrap_or_else(|_| "/var/run/docker.sock".into());
        let host = if socket.contains("://") {
            socket.clone()
        } else {
            format!("unix://{socket}")
        };
        let [managed_subnet, foreign_subnet] = unused_subnets(&host);
        std::fs::write(
            &compose,
            format!(
                r#"services:
  managed:
    image: busybox:latest
    command: ["sleep", "3600"]
    labels:
      orch.system: {project}
      orch.driver: docker
    volumes: ["managed-data:/data"]
    networks: [managed-net]
  foreign:
    image: busybox:latest
    command: ["sh", "-c", "echo keep > /data/keep; exec sleep 3600"]
    labels:
      orch.system: {foreign}
      orch.driver: docker
    volumes: ["foreign-data:/data"]
    networks: [foreign-net]
volumes:
  managed-data:
    labels:
      orch.system: {project}
      orch.driver: docker
  foreign-data:
    labels:
      orch.system: {foreign}
      orch.driver: docker
networks:
  managed-net:
    ipam:
      config: [{{subnet: {managed_subnet}}}]
    labels:
      orch.system: {project}
      orch.driver: docker
  foreign-net:
    ipam:
      config: [{{subnet: {foreign_subnet}}}]
    labels:
      orch.system: {foreign}
      orch.driver: docker
"#
            ),
        )
        .unwrap();
        let docker = |arguments: &[&str]| -> Output {
            Command::new("docker")
                .env("DOCKER_HOST", &host)
                .args(arguments)
                .output()
                .unwrap()
        };
        let project_command = |arguments: &[&str]| -> Output {
            let mut command = Command::new("docker");
            command
                .env("DOCKER_HOST", &host)
                .arg("compose")
                .arg("-p")
                .arg(&project)
                .arg("-f")
                .arg(&compose)
                .args(arguments)
                .output()
                .unwrap()
        };
        struct Project<'a> {
            project: &'a str,
            file: &'a Path,
            host: &'a str,
        }
        impl Drop for Project<'_> {
            fn drop(&mut self) {
                let _ = Command::new("docker")
                    .env("DOCKER_HOST", self.host)
                    .args(["compose", "-p", self.project, "-f"])
                    .arg(self.file)
                    .args(["down", "--volumes", "--timeout", "10"])
                    .output();
            }
        }
        println!(
            "Disposable Compose project: {project} ({})",
            compose.display()
        );
        let _cleanup = Project {
            project: &project,
            file: &compose,
            host: &host,
        };
        let up = project_command(&["up", "-d"]);
        assert!(
            up.status.success(),
            "{}",
            String::from_utf8_lossy(&up.stderr)
        );
        let configuration = format!(
            "version=1\n[system]\nname={project:?}\n[node]\ndata_dir={:?}\ndrivers=[\"docker\"]\n[node.docker]\nsocket={socket:?}\nbuild_cache_gb=0\n",
            installed.data.to_str().unwrap()
        );
        std::fs::write(&installed.config, configuration).unwrap();
        let owned_filter = format!("label=orch.system={project}");
        let foreign_filter = format!("label=orch.system={foreign}");
        let units = docker(&["ps", "-aq", "--filter", &owned_filter]);
        assert!(units.status.success() && !units.stdout.is_empty());
        let volume = docker(&["volume", "ls", "-q", "--filter", &owned_filter]);
        assert!(volume.status.success() && !volume.stdout.is_empty());
        let other_before = docker(&["ps", "--filter", &foreign_filter, "--format", "{{.ID}}"]);
        assert!(!other_before.stdout.is_empty());
        let output = if purge {
            installed.run(&["uninstall", "--purge", "--yes"])
        } else {
            installed.run(&["uninstall", "--yes"])
        };
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let units_after = docker(&["ps", "-aq", "--filter", &owned_filter]);
        let volumes_after = docker(&["volume", "ls", "-q", "--filter", &owned_filter]);
        assert_eq!(units_after.stdout.is_empty(), purge);
        assert_eq!(volumes_after.stdout.is_empty(), purge);
        let other_after = docker(&["ps", "--filter", &foreign_filter, "--format", "{{.ID}}"]);
        assert_eq!(other_after.stdout, other_before.stdout);
        let other_data = project_command(&["exec", "-T", "foreign", "cat", "/data/keep"]);
        assert!(other_data.status.success());
        assert_eq!(other_data.stdout, b"keep\n");
    }
}

#[cfg(unix)]
#[test]
fn the_exact_uninstall_command_the_node_reports_runs_against_the_installed_program() {
    let installed = Installed::new(false);
    let data = orch_connector_core::enrollment::DataDir::inspect(&installed.data).unwrap();
    let installation = orch_connector_core::installation::node_with_program(
        &data,
        &installed.config,
        &installed.program,
    );
    let output = Command::new("bash")
        .arg("-c")
        .arg(format!(
            "{} --yes",
            installation.commands.uninstall.unwrap()
        ))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!installed.program.exists());
    assert!(installed.data.is_dir() && installed.config.is_file());
}

#[cfg(unix)]
#[test]
fn a_service_manager_refusal_preserves_the_program_configuration_and_identity() {
    use std::os::unix::fs::PermissionsExt;
    let installed = Installed::new(true);
    let recorded = installed.run(&["__record-installation", "--service"]);
    assert!(recorded.status.success());
    let shim = installed._root.path().join("commands");
    std::fs::create_dir(&shim).unwrap();
    let manager = shim.join("systemctl");
    std::fs::write(&manager, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&manager, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = installed
        .command(&["uninstall", "--purge", "--yes"])
        .env(
            "PATH",
            format!("{}:{}", shim.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("service manager refused"));
    assert!(installed.program.is_file() && installed.config.is_file());
    assert!(installed.data.join("identity/enrollment.json").is_file());
    assert!(
        !orch_connector_core::uninstall::inspect(&installed.data)
            .unwrap()
            .unwrap()
            .uninstalled
    );
}
