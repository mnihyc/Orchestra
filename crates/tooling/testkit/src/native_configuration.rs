//! Guarded native session-configuration proofs inside an already-owned Linux container.

use std::ffi::OsStr;
use std::path::Path;

fn admitted(value: Option<&OsStr>, module: &str, container: bool) -> Result<bool, &'static str> {
    let Some(value) = value else { return Ok(false) };
    if value != OsStr::new("1") {
        return Err("ORCH_SESSION_SETTINGS_PROOF_IN_CONTAINER accepts only 1 when set");
    }
    if module.split("::").next() != Some("session_configuration") {
        return Err(
            "the session settings container mode is restricted to its native integration target",
        );
    }
    if !container {
        return Err("the native settings proof requires the actual Linux Docker container marker");
    }
    Ok(true)
}

/// Only the named native integration binary may use production direct launching in its container.
/// The ordinary adapter startup checks its exact CLI pin before any session or model input.
#[must_use]
pub fn container_proof_mode(module: &str) -> bool {
    let value = std::env::var_os("ORCH_SESSION_SETTINGS_PROOF_IN_CONTAINER");
    let marker = cfg!(target_os = "linux") && Path::new("/.dockerenv").is_file();
    admitted(value.as_deref(), module, marker).unwrap_or_else(|reason| panic!("{reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_native_settings_mode_cannot_turn_an_unrelated_or_host_test_into_a_direct_harness_run() {
        assert_eq!(admitted(None, "ordinary::common", false), Ok(false));
        assert_eq!(
            admitted(Some(OsStr::new("1")), "session_configuration::common", true),
            Ok(true)
        );
        assert!(admitted(Some(OsStr::new("1")), "ordinary::common", true).is_err());
        assert!(
            admitted(
                Some(OsStr::new("1")),
                "session_configuration::common",
                false
            )
            .is_err()
        );
        assert!(admitted(Some(OsStr::new("0")), "session_configuration::common", true).is_err());
    }
}
