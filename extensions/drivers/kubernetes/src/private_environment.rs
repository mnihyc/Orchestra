//! Read private environment values before the supervisor starts the connector.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::process::Command;

use orch_driver_contract::driver::DriverError;
use rustix::fs::{Mode, OFlags, openat};

use crate::launch::valid_variable;
use crate::run_files::{MAX_LAUNCH_BYTES, RunDirectory};

const ENVIRONMENT_FILE: &str = "environment.json";

/// User variables may contain secrets and are never printed in diagnostics.
pub(crate) struct PrivateEnvironment(BTreeMap<String, String>);

impl std::fmt::Debug for PrivateEnvironment {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("PrivateEnvironment([secret])")
    }
}

fn invalid() -> DriverError {
    DriverError::InvalidSpec("the private environment variables are invalid".into())
}

impl PrivateEnvironment {
    /// The caller passes its verified tmpfs directory, after the guard has prepared the run.
    pub(crate) fn read(directory: &RunDirectory) -> Result<Self, DriverError> {
        let fd = openat(
            directory.as_fd(),
            ENVIRONMENT_FILE,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| invalid())?;
        let mut file = File::from(fd);
        let meta = file.metadata().map_err(|_| invalid())?;
        if !meta.is_file()
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.permissions().mode() & 0o7777 != 0o600
            || meta.nlink() != 1
            || meta.len() > u64::try_from(MAX_LAUNCH_BYTES).map_err(|_| invalid())?
        {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(u64::try_from(MAX_LAUNCH_BYTES).map_err(|_| invalid())? + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid())?;
        if bytes.len() > MAX_LAUNCH_BYTES {
            return Err(invalid());
        }
        let values: BTreeMap<String, String> =
            serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        for (name, value) in &values {
            if !valid_variable(name, value) {
                return Err(invalid());
            }
        }
        Ok(Self(values))
    }

    /// Applies values only to the child connector; do not format the resulting Command for logs.
    pub(crate) fn apply(&self, command: &mut Command) {
        command.envs(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::LaunchFiles;
    use orch_plugin_contract::Secret;
    use std::fs;
    use std::os::unix::fs::symlink;

    #[test]
    fn private_values_reach_the_connector_without_being_printed_or_replacing_reserved_names() {
        let temp = tempfile::tempdir().unwrap();
        let directory = RunDirectory::test_filesystem(temp.path()).unwrap();
        let files = LaunchFiles {
            ca: "test-ca".into(),
            enrollment: Secret::new("synthetic-once"),
            environment: Secret::new("{\"TOKEN\":\"synthetic-env-secret\"}"),
            mounts: None,
        };
        directory
            .prepare(
                &files,
                rustix::process::geteuid().as_raw(),
                rustix::process::getegid().as_raw(),
            )
            .unwrap();
        let private = PrivateEnvironment::read(&directory).unwrap();
        assert!(!format!("{private:?}").contains("synthetic-env-secret"));
        let mut command = Command::new("/unused");
        private.apply(&mut command);
        let token = command
            .get_envs()
            .find(|(key, _)| *key == "TOKEN")
            .and_then(|(_, value)| value)
            .unwrap();
        assert_eq!(token, "synthetic-env-secret");
        fs::write(
            temp.path().join(ENVIRONMENT_FILE),
            b"{\"ORCH_GATEWAY_URL\":\"override\"}",
        )
        .unwrap();
        assert!(PrivateEnvironment::read(&directory).is_err());
    }

    #[test]
    fn a_symlinked_environment_file_is_never_followed() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("unrelated");
        fs::write(&target, b"{\"TOKEN\":\"not-for-this-unit\"}").unwrap();
        symlink(&target, temp.path().join(ENVIRONMENT_FILE)).unwrap();
        let directory = RunDirectory::test_filesystem(temp.path()).unwrap();
        assert!(PrivateEnvironment::read(&directory).is_err());
        assert_eq!(
            fs::read(target).unwrap(),
            b"{\"TOKEN\":\"not-for-this-unit\"}"
        );
    }
}
