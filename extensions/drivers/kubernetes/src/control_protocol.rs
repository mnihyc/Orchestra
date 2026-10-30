//! Bounded message types for a unit's trusted guard.
//!
//! Serialize locally and send only through the encrypted transport. The guard
//! must authenticate its workload peer separately and verify the Pod UID.

use std::io::{self, Write};

use orch_driver_contract::driver::DriverError;
use orch_plugin_contract::Secret;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

/// The serialized message includes a CA and a mount list; cap memory for damaged or hostile peers.
pub(crate) const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Commands never contain the enrollment token in a process argument or a Pod environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum GuardRequest {
    /// Bind the original Pod and its workload container to the authenticated socket peer.
    Attach {
        pod_uid: String,
        container_id: String,
    },
    /// Give the guard the first launch's private values while the unit is stopped.
    Prepare {
        pod_uid: String,
        ca: String,
        enrollment: Secret,
        environment: Secret,
        mounts: Option<String>,
    },
    /// Start using the values already held privately by the guard.
    Start { pod_uid: String },
    /// Clear the one-use token after the node confirms connector enrollment.
    EnrollmentConfirmed { pod_uid: String },
    /// Stop every workload process while keeping the trusted supervisor.
    Stop { pod_uid: String, grace_millis: u64 },
    /// Observe the pinned helper and workload without starting either.
    Inspect { pod_uid: String },
}

impl GuardRequest {
    /// Every command must name the guard's own immutable Pod incarnation.
    pub(crate) fn pod_uid(&self) -> &str {
        match self {
            Self::Attach { pod_uid, .. }
            | Self::Prepare { pod_uid, .. }
            | Self::Start { pod_uid, .. }
            | Self::EnrollmentConfirmed { pod_uid, .. }
            | Self::Stop { pod_uid, .. }
            | Self::Inspect { pod_uid } => pod_uid,
        }
    }
}

/// What the guard has actually established about the original unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GuardPhase {
    /// No authenticated workload has been prepared.
    Unprepared,
    /// The guard has proved the workload cannot write before returning this state.
    Stopped,
    /// The original supervised workload is running.
    Running,
    /// The supervisor observed a failure; this alone does not prove every descendant stopped.
    Failed,
}

/// A fixed refusal code; submitted values and peer-supplied text are never echoed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GuardRefusal {
    WrongIncarnation,
    Unavailable,
    Invalid,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum GuardReply {
    /// A verified observation or a completed operation.
    State { phase: GuardPhase },
    /// No success or state claim accompanies a refusal.
    Refused { reason: GuardRefusal },
}

struct BoundedBytes(Vec<u8>);

impl Write for BoundedBytes {
    fn write(&mut self, chunk: &[u8]) -> io::Result<usize> {
        let next = self
            .0
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| io::Error::other("the helper message is too large"))?;
        if next > MAX_FRAME_BYTES {
            return Err(io::Error::other("the helper message is too large"));
        }
        self.0.extend_from_slice(chunk);
        Ok(chunk.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serialize locally under the caller's deadline; this buffer must never be logged.
pub(crate) fn encode_request(
    request: &GuardRequest,
    deadline: Instant,
) -> Result<Vec<u8>, DriverError> {
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    let mut bytes = BoundedBytes(Vec::new());
    serde_json::to_writer(&mut bytes, request).map_err(|_| {
        DriverError::InvalidSpec("the unit helper message is too large or invalid".into())
    })?;
    if Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    Ok(bytes.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn an_enrollment_round_trip_is_redacted_in_debug_output() {
        let request = GuardRequest::Prepare {
            pod_uid: "uid-1".into(),
            ca: "test-ca".into(),
            enrollment: Secret::new("synthetic-once"),
            environment: Secret::new("{\"TOKEN\":\"synthetic-env-secret\"}"),
            mounts: Some("[]".into()),
        };
        assert!(!format!("{request:?}").contains("synthetic-once"));
        assert!(!format!("{request:?}").contains("synthetic-env-secret"));
        let encoded = encode_request(&request, Instant::now() + Duration::from_secs(5)).unwrap();
        let decoded: GuardRequest = serde_json::from_slice(&encoded).unwrap();
        let GuardRequest::Prepare {
            enrollment,
            pod_uid,
            ..
        } = decoded
        else {
            panic!("wrong operation")
        };
        assert_eq!(pod_uid, "uid-1");
        assert_eq!(enrollment.expose(), "synthetic-once");
    }

    #[test]
    fn an_oversized_serialized_request_and_expired_deadline_are_refused() {
        let request = GuardRequest::Prepare {
            pod_uid: "uid-1".into(),
            ca: "x".repeat(MAX_FRAME_BYTES),
            enrollment: Secret::new("synthetic-once"),
            environment: Secret::new("{}"),
            mounts: None,
        };
        assert!(matches!(
            encode_request(&request, Instant::now() + Duration::from_secs(5)),
            Err(DriverError::InvalidSpec(_))
        ));
        assert!(matches!(
            encode_request(&request, Instant::now()),
            Err(DriverError::Timeout)
        ));
    }

    #[test]
    fn unexpected_fields_are_not_accepted_as_control_authority() {
        let bytes = br#"{"operation":"inspect","pod_uid":"uid-1","unexpected":true}"#;
        assert!(serde_json::from_slice::<GuardRequest>(bytes).is_err());
    }
}
