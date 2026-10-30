//! The launchd agent of a connector.
//!
//! `dev.orch.connector` (or `dev.orch.connector.<suffix>`) in `~/Library/LaunchAgents`, loaded into the person's
//! GUI session: it starts at login, again ten seconds after a failure, and writes what the program says on
//! standard error to `logs/launchd.log` in its data directory. Tracing writes only the pruned daily files;
//! this launchd file is reserved for panic/startup-error reporting, not a duplicate of ordinary logs.

use std::path::PathBuf;

use super::{Action, Service, ServiceError, manager};

/// The agent's label.
#[must_use]
pub fn label(service: &Service) -> String {
    match &service.suffix {
        Some(suffix) => format!("dev.orch.connector.{suffix}"),
        None => "dev.orch.connector".to_owned(),
    }
}

/// `text` inside an XML element.
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The agent's property list as it is written.
#[must_use]
pub fn plist_text(service: &Service) -> String {
    let program = service.program.to_string_lossy();
    let dir = service.data_dir.to_string_lossy();
    let log = service.data_dir.join("logs").join("launchd.log");
    let environment: String = service
        .environment
        .iter()
        .map(|(name, value)| {
            format!(
                "\x20   <key>{}</key><string>{}</string>\n",
                xml(name),
                xml(value)
            )
        })
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \x20 <key>Label</key><string>{}</string>\n\
         \x20 <key>ProgramArguments</key>\n\x20 <array>\n\
         \x20   <string>{}</string>\n\x20   <string>--data-dir</string>\n\x20   <string>{}</string>\n\
         \x20   <string>run</string>\n\x20 </array>\n\
         \x20 <key>RunAtLoad</key><true/>\n\
         \x20 <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>\n\
         \x20 <key>ThrottleInterval</key><integer>10</integer>\n\
         \x20 <key>EnvironmentVariables</key><dict>\n\
         \x20   <key>ORCH_CONNECTOR_LOG_FILES_ONLY</key><string>1</string>\n\
         {environment}\x20 </dict>\n\
         \x20 <key>StandardErrorPath</key><string>{}</string>\n\
         </dict>\n</plist>\n",
        xml(&label(service)),
        xml(&program),
        xml(&dir),
        xml(&log.to_string_lossy()),
    )
}

/// The person's GUI session, `gui/<uid>`.
fn domain() -> Result<String, ServiceError> {
    Ok(format!("gui/{}", manager("id", &["-u"])?))
}

/// Whether the agent's property list is written.
#[must_use]
pub fn installed(service: &Service) -> bool {
    super::home()
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{}.plist", label(service)))
        .is_file()
}

/// Does `action` with launchd.
pub fn act(action: Action, service: &Service) -> Result<String, ServiceError> {
    let label = label(service);
    let path: PathBuf = super::home()
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{label}.plist"));
    let target = || domain().map(|domain| format!("{domain}/{label}"));
    let plist = path.to_string_lossy().into_owned();
    match action {
        Action::Install => {
            std::fs::create_dir_all(service.data_dir.join("logs")).map_err(|source| {
                ServiceError::Io {
                    path: service.data_dir.join("logs"),
                    source,
                }
            })?;
            super::write(&path, plist_text(service).as_bytes())?;
            // An agent loaded before is taken out first, so that the file written now is the one that runs.
            let _ = manager("launchctl", &["bootout", &target()?]);
            manager("launchctl", &["bootstrap", &domain()?, &plist])?;
            Ok(format!(
                "The connector runs as the launchd agent {label} ({plist})."
            ))
        }
        Action::Uninstall => {
            let _ = manager("launchctl", &["bootout", &target()?]);
            if super::remove(&path)? {
                Ok(format!("The agent {label} is stopped and removed."))
            } else {
                Ok(format!("No agent {label} is installed."))
            }
        }
        Action::Start => {
            if manager("launchctl", &["print", &target()?]).is_err() {
                manager("launchctl", &["bootstrap", &domain()?, &plist])?;
            }
            manager("launchctl", &["kickstart", &target()?])?;
            Ok(format!("{label} is started."))
        }
        Action::Stop => {
            // Taken out of the session: launchd would start a stopped agent that is kept alive again.
            manager("launchctl", &["bootout", &target()?])?;
            Ok(format!(
                "{label} is stopped until it is started or the person logs in again."
            ))
        }
        Action::Status => {
            if !path.exists() {
                return Ok(format!("No agent {label} is installed."));
            }
            Ok(match manager("launchctl", &["print", &target()?]) {
                Ok(printed) => {
                    let state = printed
                        .lines()
                        .find_map(|line| line.trim().strip_prefix("state = "))
                        .unwrap_or("loaded");
                    format!("{label}: {state}")
                }
                Err(_) => format!("{label}: not loaded"),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A launchd file names Unix paths; Windows joins the same names with its own separator.
    #[cfg(unix)]
    #[test]
    fn the_agent_runs_the_program_on_its_directory_with_what_xml_would_read_escaped() {
        let service = Service {
            program: "/Users/a/.local/bin/orch-connector".into(),
            data_dir: "/Users/a/R&D <work>".into(),
            suffix: Some("3ef2a1c7".to_owned()),
            environment: vec![("ALL_PROXY".into(), "socks5h://proxy&co:1080".into())],
        };
        assert_eq!(label(&service), "dev.orch.connector.3ef2a1c7");
        let text = plist_text(&service);
        assert!(
            text.contains("<string>/Users/a/R&amp;D &lt;work&gt;</string>"),
            "{text}"
        );
        assert!(
            text.contains("<string>/Users/a/R&amp;D &lt;work&gt;/logs/launchd.log</string>"),
            "{text}"
        );
        assert!(text.contains("<key>RunAtLoad</key><true/>"), "{text}");
        assert!(
            text.contains("<key>ORCH_CONNECTOR_LOG_FILES_ONLY</key><string>1</string>"),
            "{text}"
        );
        assert!(
            text.contains("<key>ALL_PROXY</key><string>socks5h://proxy&amp;co:1080</string>"),
            "{text}"
        );
    }
}
