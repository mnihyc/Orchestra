//! Configured run_as identity including permitted root execution.
//!
//! A node may use its own account or, when already root, the account selected by the template.
//! Account switching uses the operating system's runuser program, which initializes its groups.

use orch_driver_contract::driver::DriverError;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The local account resolved before any process or directory is made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Account name, passed as one argument to the account switcher.
    pub name: String,
    /// Numeric user identifier.
    pub uid: u32,
    /// Primary group identifier.
    pub gid: u32,
}

/// Resolves the template account, refusing a switch the node cannot perform.
pub fn resolve(name: &str) -> Result<Identity, DriverError> {
    #[cfg(unix)]
    {
        use nix::unistd::{Uid, User};
        let user = User::from_name(name)
            .map_err(|_| {
                DriverError::Unavailable("the local account directory could not be read".into())
            })?
            .ok_or_else(|| {
                DriverError::InvalidSpec("run_as must name an existing local account".into())
            })?;
        if !Uid::effective().is_root() && Uid::effective() != user.uid {
            return Err(DriverError::Unavailable("the node must already run as root to use another account; no privilege escalation is attempted".into()));
        }
        Ok(Identity {
            name: user.name,
            uid: user.uid.as_raw(),
            gid: user.gid.as_raw(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = name;
        Err(DriverError::Unsupported(
            "the host driver needs POSIX accounts".into(),
        ))
    }
}

/// Gives an owned file or directory to the unit's local account.
pub fn own(path: &Path, identity: &Identity) -> Result<(), DriverError> {
    #[cfg(unix)]
    {
        use nix::unistd::{Gid, Uid, chown};
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(path).map_err(crate::units::io_error)?;
        if metadata.file_type().is_symlink() {
            return Err(DriverError::InvalidSpec(
                "an owned directory or launch file must not be a symbolic link".into(),
            ));
        }
        if metadata.uid() != identity.uid || metadata.gid() != identity.gid {
            chown(
                path,
                Some(Uid::from_raw(identity.uid)),
                Some(Gid::from_raw(identity.gid)),
            )
            .map_err(|_| {
                DriverError::Unavailable("the node cannot assign a directory to run_as".into())
            })?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, identity);
    }
    Ok(())
}

pub(crate) fn launch(
    identity: &Identity,
    program: &Path,
    arguments: &[String],
) -> tokio::process::Command {
    #[cfg(unix)]
    if nix::unistd::Uid::effective().as_raw() != identity.uid {
        let mut command = tokio::process::Command::new("runuser");
        command
            .args(["--user", identity.name.as_str(), "--"])
            .arg(program)
            .args(arguments);
        return command;
    }
    let mut command = tokio::process::Command::new(program);
    command.args(arguments);
    command
}
