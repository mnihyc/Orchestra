//! Authenticate the workload supervisor by kernel credentials and its container scope.

#[cfg(target_os = "linux")]
use std::os::fd::AsFd;
#[cfg(target_os = "linux")]
use std::os::fd::OwnedFd;
#[cfg(target_os = "linux")]
use tokio::net::UnixStream;

#[cfg(target_os = "linux")]
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
#[cfg(target_os = "linux")]
use nix::sys::socket::{getsockopt, sockopt::PeerPidfd};
#[cfg(target_os = "linux")]
use orch_driver_contract::driver::DriverError;

#[cfg(target_os = "linux")]
use crate::process_scope::{belongs_to_container, container_id};

/// A peer pinned to the workload supervisor's process, with its image-selected identity.
#[cfg(target_os = "linux")]
pub(crate) struct WorkloadPeer {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
    pub pidfd: OwnedFd,
}

/// Pins the socket peer itself and verifies its scope before it can be trusted.
#[cfg(target_os = "linux")]
pub(crate) fn authenticate(
    stream: &UnixStream,
    container_reference: &str,
) -> Result<WorkloadPeer, DriverError> {
    let id = container_id(container_reference)?;
    // A socket-provided pidfd refers to the actual peer, even if its numeric PID is later reused.
    let handle = getsockopt(stream, PeerPidfd).map_err(|_| {
        DriverError::Unavailable("the workload supervisor's process cannot be pinned".into())
    })?;
    let credentials = stream.peer_cred().map_err(|_| {
        DriverError::Unavailable("the workload supervisor's identity cannot be read".into())
    })?;
    let raw_pid = credentials.pid().filter(|pid| *pid > 0).ok_or_else(|| {
        DriverError::Unavailable("the workload supervisor's process is not identifiable".into())
    })?;
    let cgroup = std::fs::read_to_string(format!("/proc/{raw_pid}/cgroup")).map_err(|_| {
        DriverError::Unavailable("the workload supervisor's process scope cannot be read".into())
    })?;
    // A readable pidfd means the original peer ended; do not use proc data from a reused PID.
    let mut watched = [PollFd::new(handle.as_fd(), PollFlags::POLLIN)];
    if poll(&mut watched, PollTimeout::ZERO).map_err(|_| {
        DriverError::Unavailable("the workload supervisor's process cannot be checked".into())
    })? != 0
    {
        return Err(DriverError::Unavailable(
            "the workload supervisor's process has ended".into(),
        ));
    }
    if !belongs_to_container(&cgroup, id) {
        return Err(DriverError::Unavailable(
            "the supervisor is not in the expected workload container".into(),
        ));
    }
    Ok(WorkloadPeer {
        pid: raw_pid,
        uid: credentials.uid(),
        gid: credentials.gid(),
        pidfd: handle,
    })
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_peer_is_refused_when_the_cluster_did_not_report_a_complete_container_id() {
        let (first, _second) = UnixStream::pair().unwrap();
        let error = authenticate(&first, "containerd://short").err().unwrap();
        assert!(matches!(error, DriverError::Unsupported(_)));
    }
}
