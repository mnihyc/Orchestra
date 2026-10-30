//! The scheduled task of a connector.
//!
//! `Orch Connector` (or `Orch Connector <suffix>`): it starts when the person logs on, runs as them with the
//! least privilege, has no time limit, and is started again a minute after a failure, up to 999 times; the same
//! task the Windows installer registers.

use super::{Action, Service, ServiceError, manager};

/// The task's name.
#[must_use]
pub fn task_name(service: &Service) -> String {
    match &service.suffix {
        Some(suffix) => format!("Orch Connector {suffix}"),
        None => "Orch Connector".to_owned(),
    }
}

/// `text` inside an XML element.
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The task's definition, for the person `user` (`DOMAIN\name`).
#[must_use]
pub fn task_xml(service: &Service, user: &str) -> String {
    let dir = service.data_dir.to_string_lossy();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n\
         <Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n\
         <RegistrationInfo><Description>Orch connector ({dir})</Description></RegistrationInfo>\n\
         <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>\n\
         <Principals><Principal id=\"Author\"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType>\
         <RunLevel>LeastPrivilege</RunLevel></Principal></Principals>\n\
         <Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\
         <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\
         <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit>\
         <RestartOnFailure><Interval>PT1M</Interval><Count>999</Count></RestartOnFailure>\
         <StartWhenAvailable>true</StartWhenAvailable></Settings>\n\
         <Actions Context=\"Author\"><Exec><Command>{program}</Command>\
         <Arguments>--data-dir \"{dir_argument}\" run</Arguments></Exec></Actions>\n\
         </Task>\n",
        dir = xml(&dir),
        user = xml(user),
        program = xml(&service.program.to_string_lossy()),
        dir_argument = xml(&dir),
    )
}

/// `text` as UTF-16 little-endian with its byte order mark, which is how the task scheduler reads a definition.
fn utf16(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

/// Whether the task is registered.
#[must_use]
pub fn installed(service: &Service) -> bool {
    manager("schtasks", &["/Query", "/TN", &task_name(service)]).is_ok()
}

/// Does `action` with the task scheduler.
pub fn act(action: Action, service: &Service) -> Result<String, ServiceError> {
    let name = task_name(service);
    let schtasks = |args: &[&str]| manager("schtasks", args);
    match action {
        Action::Install => {
            let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
                (Ok(domain), Ok(account)) => format!("{domain}\\{account}"),
                (_, Ok(account)) => account,
                _ => {
                    return Err(ServiceError::Manager(
                        "USERNAME is not set, so the task cannot name whom it runs as".to_owned(),
                    ));
                }
            };
            let definition = service.data_dir.join("cache").join("service-task.xml");
            super::write(&definition, &utf16(&task_xml(service, &user)))?;
            let path = definition.to_string_lossy().into_owned();
            let created = schtasks(&["/Create", "/TN", &name, "/XML", &path, "/F"]);
            let _ = super::remove(&definition);
            created?;
            schtasks(&["/Run", "/TN", &name])?;
            Ok(format!(
                "The connector runs as the scheduled task '{name}', started at every logon."
            ))
        }
        Action::Uninstall => {
            if schtasks(&["/Query", "/TN", &name]).is_err() {
                return Ok(format!("No task '{name}' is installed."));
            }
            let _ = schtasks(&["/End", "/TN", &name]);
            schtasks(&["/Delete", "/TN", &name, "/F"])?;
            Ok(format!("The task '{name}' is stopped and removed."))
        }
        Action::Start => {
            schtasks(&["/Run", "/TN", &name])?;
            Ok(format!("'{name}' is started."))
        }
        Action::Stop => {
            schtasks(&["/End", "/TN", &name])?;
            Ok(format!(
                "'{name}' is stopped until it is started or the person logs on again."
            ))
        }
        Action::Status => Ok(match schtasks(&["/Query", "/TN", &name, "/FO", "LIST"]) {
            Ok(listed) => {
                let state = listed
                    .lines()
                    .find_map(|line| line.trim().strip_prefix("Status:"))
                    .map_or("installed", str::trim);
                format!("'{name}': {state}")
            }
            Err(_) => format!("No task '{name}' is installed."),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_task_starts_the_program_at_logon_and_again_after_a_failure() {
        let service = Service {
            program: r"C:\Users\a\AppData\Local\Orch\orch-connector.exe".into(),
            data_dir: r"C:\Users\a\Orch & Co".into(),
            suffix: None,
            environment: Vec::new(),
        };
        assert_eq!(task_name(&service), "Orch Connector");
        let text = task_xml(&service, r"PC\a");
        assert!(
            text.contains(r#"<Arguments>--data-dir "C:\Users\a\Orch &amp; Co" run</Arguments>"#),
            "{text}"
        );
        assert!(text.contains(r"<UserId>PC\a</UserId>"), "{text}");
        assert!(text.contains("<Count>999</Count>"), "{text}");
        let bytes = utf16("<T/>");
        assert_eq!(&bytes[..4], &[0xFF, 0xFE, b'<', 0]);
    }
}
