//! Observed installation and exact instance commands, shared by connector and node reports.

use crate::{
    enrollment::{DataDir, default_data_dir},
    service::{self, Service},
};
use orch_api_contract::machine_programs::{MachineInstallation, MachineInstallationCommands};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn quote(value: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn executable(path: &Path) -> String {
    let word = quote(&path.to_string_lossy());
    if cfg!(target_os = "windows") {
        format!("& {word}")
    } else {
        word
    }
}

/// The physical runner's selected executable, retained when an installer replaces its file in place.
#[must_use]
pub fn program_path() -> Option<PathBuf> {
    static EXECUTABLE: OnceLock<Option<PathBuf>> = OnceLock::new();
    EXECUTABLE
        .get_or_init(|| std::env::current_exe().ok())
        .clone()
}

/// A connector's observed user installation, including the real named-instance service if present.
#[must_use]
pub fn connector(data: &DataDir) -> MachineInstallation {
    connector_with_program(
        data,
        &program_path().unwrap_or_else(|| PathBuf::from("orch-connector")),
    )
}

/// A node's actual operational file and identity directory; no instance is inferred from placement.
#[must_use]
pub fn node(data: &DataDir, config_file: &Path) -> MachineInstallation {
    node_with_program(
        data,
        config_file,
        &program_path().unwrap_or_else(|| PathBuf::from("orch-node")),
    )
}

/// Uses the same observed command formatter for a known installed connector executable.
#[must_use]
pub fn connector_with_program(data: &DataDir, program: &Path) -> MachineInstallation {
    observe(data, None, program)
}

/// Uses the same observed command formatter for a known installed node executable.
#[must_use]
pub fn node_with_program(
    data: &DataDir,
    config_file: &Path,
    program: &Path,
) -> MachineInstallation {
    observe(data, Some(config_file), program)
}

fn observe(data: &DataDir, node_config: Option<&Path>, path: &Path) -> MachineInstallation {
    let node = node_config.is_some();
    let program = if node { "orch-node" } else { "orch-connector" };
    let path = path.to_owned();
    let root = std::path::absolute(data.root()).unwrap_or_else(|_| data.root().to_owned());
    let configured = node_config.map_or_else(|| data.config_path(), Path::to_owned);
    let config_file = std::path::absolute(&configured).unwrap_or(configured);
    let mut prefix = format!(
        "{} --data-dir {}",
        executable(&path),
        quote(&root.to_string_lossy())
    );
    if node {
        prefix.push_str(&format!(
            " --config {}",
            quote(&config_file.to_string_lossy())
        ));
    }
    let user = {
        #[cfg(unix)]
        {
            if rustix::process::geteuid().as_raw() == 0 {
                Some("root".into())
            } else {
                std::env::var("USER").ok()
            }
        }
        #[cfg(not(unix))]
        {
            std::env::var("USERNAME").ok()
        }
    };
    let named = Service::new(path.clone(), root.clone(), true);
    let plain = Service::new(path.clone(), root.clone(), false);
    let default = default_data_dir(|name| std::env::var(name).ok());
    let default = std::path::absolute(&default).unwrap_or(default);
    let installed = if node {
        None
    } else if service::installed(&named) {
        Some(named)
    } else if root == default && service::installed(&plain) {
        Some(plain)
    } else {
        None
    };
    let service = if node {
        Path::new("/etc/systemd/system/orch-node.service")
            .is_file()
            .then(|| "orch-node.service".into())
    } else {
        installed.as_ref().map(service::name)
    };
    let (start, stop) = if node && service.is_some() {
        (
            "systemctl start orch-node.service".into(),
            "systemctl stop orch-node.service".into(),
        )
    } else if let Some(installed) = installed {
        // Explicit --data-dir selects a hashed service. The default service stays unsuffixed.
        let service_prefix = if installed.suffix.is_some() {
            prefix.clone()
        } else {
            executable(&path)
        };
        (
            format!("{service_prefix} service start"),
            format!("{service_prefix} service stop"),
        )
    } else {
        let stop = if cfg!(target_os = "windows") {
            format!("Stop-Process -Id {}", std::process::id())
        } else {
            format!("kill -TERM {}", std::process::id())
        };
        (format!("{prefix} run"), stop)
    };
    let standalone = crate::uninstall::available(data.root(), &path)
        .unwrap_or(false)
        .then(|| format!("{prefix} uninstall"));
    let uninstall = if node {
        std::env::var_os("ORCH_INSTALLATION_ROOT")
            .filter(|root| !root.is_empty())
            .map(|root| {
                format!(
                    "{} uninstall",
                    executable(&PathBuf::from(root).join("update.sh"))
                )
            })
            .or(standalone)
    } else {
        standalone
    };
    MachineInstallation {
        program: program.into(),
        executable: path.display().to_string(),
        data_dir: root.display().to_string(),
        config_file: config_file.display().to_string(),
        system: user.as_deref() == Some("root"),
        user,
        service,
        commands: MachineInstallationCommands {
            policy: format!("{prefix} policy"),
            roles: (!node).then(|| format!("{prefix} roles")),
            start,
            stop,
            update: format!("{prefix} update"),
            uninstall,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_instance_commands_keep_the_identity_directory_and_the_nodes_real_config()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().join("a named instance");
        let data = DataDir::open(&root)?;
        let connector = connector(&data);
        assert_eq!(connector.program, "orch-connector");
        assert_eq!(connector.data_dir, root.display().to_string());
        let selector = format!("--data-dir {}", quote(&root.to_string_lossy()));
        assert!(connector.commands.policy.contains(&selector));
        assert!(connector.commands.policy.ends_with(" policy"));
        assert!(
            connector
                .commands
                .roles
                .as_ref()
                .is_some_and(|command| command.contains(&selector))
        );
        assert!(connector.commands.start.ends_with(" run"));
        assert_eq!(
            connector.commands.uninstall, None,
            "an unknown installer record never invents an uninstall command"
        );
        let config = temporary.path().join("custom node.toml");
        let node = node(&data, &config);
        assert_eq!(node.program, "orch-node");
        assert_eq!(node.config_file, config.display().to_string());
        assert!(node.commands.roles.is_none());
        assert!(
            node.commands
                .policy
                .contains(&format!("--config {}", quote(&config.to_string_lossy())))
        );
        assert!(node.commands.update.contains(&selector));
        let relative = Path::new("ops/node.toml");
        let node = super::node(&data, relative);
        assert_eq!(
            node.config_file,
            std::path::absolute(relative)?.display().to_string()
        );
        Ok(())
    }
}
