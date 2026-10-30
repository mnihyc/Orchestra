//! The systemd user unit of a connector.
//!
//! `orch-connector.service` (or `orch-connector-<suffix>.service`) in `$XDG_CONFIG_HOME/systemd/user`, else
//! `~/.config/systemd/user`: it starts with the person's user manager, after the network is up, and again ten
//! seconds after a failure. It stops at logout unless the person keeps their user manager running
//! (`loginctl enable-linger`), which is suggested and never done for them.

use std::path::PathBuf;

use super::{Action, Service, ServiceError, manager};

/// The unit's name.
#[must_use]
pub fn unit_name(service: &Service) -> String {
    match &service.suffix {
        Some(suffix) => format!("orch-connector-{suffix}.service"),
        None => "orch-connector.service".to_owned(),
    }
}

/// One word of a unit's command line: quoted, with what systemd would read otherwise escaped.
fn word(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
    )
}

/// One assignment of `Environment=`: quoted, with what systemd would read otherwise escaped. Unlike a command
/// line's, `$` is plain there: systemd expands no variable in it.
fn assignment(name: &str, value: &str) -> String {
    format!(
        "\"{}\"",
        format!("{name}={value}")
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

/// The unit as it is written.
#[must_use]
pub fn unit_text(service: &Service) -> String {
    let dir = service.data_dir.to_string_lossy();
    let environment: String = service
        .environment
        .iter()
        .map(|(name, value)| format!("Environment={}\n", assignment(name, value)))
        .collect();
    format!(
        "[Unit]\nDescription=Orch connector ({})\nAfter=network-online.target\nWants=network-online.target\n\n\
         [Service]\nExecStart={} --data-dir {} run\n{environment}Restart=on-failure\nRestartSec=10\n\n\
         [Install]\nWantedBy=default.target\n",
        dir.replace('%', "%%"),
        word(&service.program.to_string_lossy()),
        word(&dir),
    )
}

/// Where user units are.
fn unit_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| super::home().join(".config"), PathBuf::from)
        .join("systemd")
        .join("user")
}

/// Whether the unit is written.
#[must_use]
pub fn installed(service: &Service) -> bool {
    unit_dir().join(unit_name(service)).is_file()
}

/// Does `action` with the person's systemd user manager.
pub fn act(action: Action, service: &Service) -> Result<String, ServiceError> {
    let name = unit_name(service);
    let path = unit_dir().join(&name);
    let systemctl = |args: &[&str]| {
        let mut all = vec!["--user"];
        all.extend_from_slice(args);
        manager("systemctl", &all)
    };
    match action {
        Action::Install => {
            // A manager that cannot be reached is said before anything is written.
            systemctl(&["show-environment"]).map_err(|_| {
                ServiceError::Manager(format!(
                    "no systemd user manager is reachable here; start the connector yourself: {} --data-dir {} run",
                    word(&service.program.to_string_lossy()),
                    word(&service.data_dir.to_string_lossy())
                ))
            })?;
            super::write(&path, unit_text(service).as_bytes())?;
            systemctl(&["daemon-reload"])?;
            systemctl(&["enable", &name])?;
            systemctl(&["restart", &name])?;
            Ok(format!(
                "The connector runs as the systemd user unit {name} ({}). It stops when you log out unless your \
                 user manager stays: loginctl enable-linger",
                path.display()
            ))
        }
        Action::Uninstall => {
            if !path.exists() {
                return Ok(format!("No unit {name} is installed."));
            }
            systemctl(&["disable", "--now", &name])?;
            if !super::remove(&path)? {
                return Ok(format!("No unit {name} is installed."));
            }
            systemctl(&["daemon-reload"])?;
            Ok(format!("The unit {name} is stopped and removed."))
        }
        Action::Start => {
            systemctl(&["start", &name])?;
            Ok(format!("{name} is started."))
        }
        Action::Stop => {
            systemctl(&["stop", &name])?;
            Ok(format!("{name} is stopped."))
        }
        Action::Status => {
            if !path.exists() {
                return Ok(format!("No unit {name} is installed."));
            }
            // `is-active` answers with a word and an exit status that is not zero for every word but active.
            let state = std::process::Command::new("systemctl")
                .args(["--user", "is-active", &name])
                .output()
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
                .unwrap_or_default();
            Ok(format!(
                "{name}: {}",
                if state.is_empty() { "unknown" } else { &state }
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_runs_the_program_on_its_directory_and_quotes_what_systemd_would_read() {
        let service = Service {
            program: "/home/a b/.local/bin/orch-connector".into(),
            data_dir: "/srv/100%/$work".into(),
            suffix: Some("3ef2a1c7".to_owned()),
            environment: Vec::new(),
        };
        assert_eq!(unit_name(&service), "orch-connector-3ef2a1c7.service");
        let text = unit_text(&service);
        assert!(
            text.contains(
                "ExecStart=\"/home/a b/.local/bin/orch-connector\" --data-dir \"/srv/100%%/$$work\" run\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("Restart=on-failure\nRestartSec=10\n"),
            "{text}"
        );
        assert!(text.contains("WantedBy=default.target"), "{text}");
        let own = Service {
            suffix: None,
            ..service
        };
        assert_eq!(unit_name(&own), "orch-connector.service");
    }

    #[test]
    fn the_unit_runs_with_the_proxy_it_was_given_quoted_as_systemd_reads_it() {
        let service = Service {
            program: "/usr/local/bin/orch-connector".into(),
            data_dir: "/var/lib/orch".into(),
            suffix: None,
            environment: vec![
                (
                    "HTTPS_PROXY".into(),
                    "socks5h://ada:p%40ss$1@proxy:1080".into(),
                ),
                ("NO_PROXY".into(), ".corp,10.0.0.0/8".into()),
            ],
        };
        let text = unit_text(&service);
        assert!(
            text.contains(
                "run\nEnvironment=\"HTTPS_PROXY=socks5h://ada:p%%40ss$1@proxy:1080\"\nEnvironment=\"NO_PROXY=.corp,10.0.0.0/8\"\nRestart="
            ),
            "{text}"
        );
    }
}
