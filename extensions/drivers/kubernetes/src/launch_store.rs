//! Guard-owned launch values for the life of one retained Pod.
//!
//! The real guard must be private and non-dumpable, and must call confirmation
//! only after the node has verified enrollment. This state alone proves neither.

use orch_driver_contract::driver::DriverError;
use orch_plugin_contract::Secret;

use crate::launch::LaunchFiles;
use crate::run_files::{MAX_LAUNCH_BYTES, RunDirectory};

#[derive(Default)]
pub(crate) struct LaunchStore {
    files: Option<LaunchFiles>,
    enrollment_confirmed: bool,
}

impl std::fmt::Debug for LaunchStore {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("LaunchStore")
            .field("prepared", &self.files.is_some())
            .field("enrollment_confirmed", &self.enrollment_confirmed)
            .finish()
    }
}

impl LaunchStore {
    /// Preparation happens once before untrusted work starts; it cannot replace a retained unit's values.
    pub(crate) fn prepare(&mut self, files: LaunchFiles) -> Result<(), DriverError> {
        if self.files.is_some() {
            return Err(DriverError::Busy(
                "the retained unit is already prepared".into(),
            ));
        }
        let size = files
            .ca
            .len()
            .checked_add(files.enrollment.expose().len())
            .and_then(|n| n.checked_add(files.environment.expose().len()))
            .and_then(|n| n.checked_add(files.mounts.as_ref().map_or(0, String::len)))
            .ok_or_else(|| DriverError::InvalidSpec("the launch values are too large".into()))?;
        if size > MAX_LAUNCH_BYTES || files.enrollment.expose().is_empty() {
            return Err(DriverError::InvalidSpec(
                "the launch values are missing or too large".into(),
            ));
        }
        self.files = Some(files);
        Ok(())
    }

    /// A later start obtains the same values without asking the node to resend its lost token.
    pub(crate) fn files(&self) -> Result<&LaunchFiles, DriverError> {
        self.files
            .as_ref()
            .ok_or_else(|| DriverError::Unavailable("the retained unit is not prepared".into()))
    }

    /// Forget the one-use token only after the caller has confirmed the connector's enrollment.
    pub(crate) fn confirm_enrollment(
        &mut self,
        run: &RunDirectory,
        uid: u32,
        gid: u32,
    ) -> Result<(), DriverError> {
        let files = self
            .files
            .as_mut()
            .ok_or_else(|| DriverError::Unavailable("the retained unit is not prepared".into()))?;
        run.clear_enrollment(uid, gid)?;
        files.enrollment = Secret::new("");
        self.enrollment_confirmed = true;
        Ok(())
    }

    /// Clear workload-visible files only after the guard has proved every workload process stopped.
    pub(crate) fn clear_stopped_run(
        &self,
        run: &RunDirectory,
        uid: u32,
        gid: u32,
    ) -> Result<(), DriverError> {
        self.files()?;
        run.clear_private(uid, gid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> LaunchFiles {
        LaunchFiles {
            ca: "fixture-ca".into(),
            enrollment: Secret::new("synthetic-once"),
            environment: Secret::new("{\"TOKEN\":\"synthetic-env-secret\"}"),
            mounts: Some("[]".into()),
        }
    }

    #[test]
    fn stopping_before_first_enrollment_keeps_the_token_for_the_next_start() {
        let directory = tempfile::tempdir().unwrap();
        let run = RunDirectory::test_filesystem(directory.path()).unwrap();
        let uid = rustix::process::geteuid().as_raw();
        let gid = rustix::process::getegid().as_raw();
        let mut store = LaunchStore::default();
        assert!(store.files().is_err());
        assert!(store.confirm_enrollment(&run, uid, gid).is_err());
        store.prepare(launch()).unwrap();
        assert!(store.prepare(launch()).is_err());
        run.prepare(store.files().unwrap(), uid, gid).unwrap();
        store.clear_stopped_run(&run, uid, gid).unwrap();
        assert_eq!(
            std::fs::read_to_string(directory.path().join("enroll")).unwrap(),
            ""
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("environment.json")).unwrap(),
            "{}"
        );
        assert_eq!(store.files().unwrap().enrollment.expose(), "synthetic-once");
        run.prepare(store.files().unwrap(), uid, gid).unwrap();
        assert_eq!(
            std::fs::read_to_string(directory.path().join("enroll")).unwrap(),
            "synthetic-once"
        );
        assert!(!format!("{store:?}").contains("synthetic-once"));
        assert!(!format!("{store:?}").contains("synthetic-env-secret"));
        store.confirm_enrollment(&run, uid, gid).unwrap();
        assert_eq!(
            std::fs::read_to_string(directory.path().join("enroll")).unwrap(),
            ""
        );
        assert_eq!(store.files().unwrap().enrollment.expose(), "");
        assert_eq!(
            store.files().unwrap().environment.expose(),
            "{\"TOKEN\":\"synthetic-env-secret\"}"
        );
        store.clear_stopped_run(&run, uid, gid).unwrap();
        run.prepare(store.files().unwrap(), uid, gid).unwrap();
        assert_eq!(
            std::fs::read_to_string(directory.path().join("enroll")).unwrap(),
            ""
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("environment.json")).unwrap(),
            "{\"TOKEN\":\"synthetic-env-secret\"}"
        );
    }

    #[test]
    fn a_missing_or_oversized_first_token_cannot_prepare_a_unit() {
        let mut store = LaunchStore::default();
        let mut files = launch();
        files.enrollment = Secret::new("");
        assert!(store.prepare(files).is_err());
        let mut files = launch();
        files.ca = "x".repeat(MAX_LAUNCH_BYTES);
        assert!(store.prepare(files).is_err());
        assert!(store.files().is_err());
    }
}
