//! Identify a workload's processes without matching command lines.

use orch_driver_contract::driver::DriverError;

/// The containerd IDs used by this process scope are full 256-bit hexadecimal identifiers.
const CONTAINER_ID_HEX_DIGITS: usize = 64;

fn valid_id(id: &str) -> bool {
    id.len() == CONTAINER_ID_HEX_DIGITS
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Refuses a proc mount that hides processes or cannot be identified unambiguously.
pub(crate) fn visible_proc(mountinfo: &str) -> bool {
    let mut proc_mounts = 0;
    for line in mountinfo.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        let mut fields = before.split_whitespace();
        if fields.nth(4) != Some("/proc") {
            continue;
        }
        let Some(mount_options) = fields.next() else {
            return false;
        };
        let mut filesystem = after.split_whitespace();
        if filesystem.next() != Some("proc") || filesystem.next().is_none() {
            return false;
        }
        let Some(super_options) = filesystem.next() else {
            return false;
        };
        proc_mounts += 1;
        if mount_options
            .split(',')
            .chain(super_options.split(','))
            .filter_map(|option| option.strip_prefix("hidepid="))
            .any(|value| value != "0")
        {
            return false;
        }
    }
    proc_mounts == 1
}

/// Reads a complete supported container ID from the cluster's container status.
pub(crate) fn container_id(reference: &str) -> Result<&str, DriverError> {
    let (runtime, id) = reference.split_once("://").ok_or_else(|| {
        DriverError::Unsupported("the container runtime did not report an identifiable unit".into())
    })?;
    if runtime != "containerd" || !valid_id(id) {
        return Err(DriverError::Unsupported(
            "the container runtime did not report an identifiable unit".into(),
        ));
    }
    Ok(id)
}

/// Whether a proc cgroup record puts this process in the exact workload container or its children.
pub(crate) fn belongs_to_container(cgroup: &str, id: &str) -> bool {
    if !valid_id(id) {
        return false;
    }
    let scope = format!("cri-containerd-{id}.scope");
    cgroup.lines().any(|line| {
        let Some(path) = line.strip_prefix("0::") else {
            return false;
        };
        path.split('/').any(|part| part == id || part == scope)
    })
}

/// Lists candidate workload PIDs while keeping the authenticated supervisor alive.
#[cfg(target_os = "linux")]
pub(crate) fn members(
    proc_root: &std::path::Path,
    id: &str,
    supervisor_pid: i32,
    deadline: std::time::Instant,
) -> Result<Vec<i32>, DriverError> {
    if std::time::Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    if !valid_id(id) || supervisor_pid <= 0 {
        return Err(DriverError::Unsupported(
            "the workload process scope is not identifiable".into(),
        ));
    }
    let supervisor = std::fs::read_to_string(proc_root.join(format!("{supervisor_pid}/cgroup")))
        .map_err(|_| {
            DriverError::Unavailable("the workload supervisor's scope cannot be read".into())
        })?;
    if !belongs_to_container(&supervisor, id) {
        return Err(DriverError::Unavailable(
            "the workload supervisor left its expected container".into(),
        ));
    }
    let entries = std::fs::read_dir(proc_root)
        .map_err(|_| DriverError::Unavailable("the workload processes cannot be listed".into()))?;
    let mut found = Vec::new();
    for entry in entries {
        if std::time::Instant::now() >= deadline {
            return Err(DriverError::Timeout);
        }
        let entry =
            entry.map_err(|_| DriverError::Unavailable("a process entry cannot be read".into()))?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        if pid <= 0 || pid == supervisor_pid {
            continue;
        }
        let cgroup = match std::fs::read_to_string(entry.path().join("cgroup")) {
            Ok(cgroup) => cgroup,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                return Err(DriverError::Unavailable(
                    "a process scope cannot be read".into(),
                ));
            }
        };
        if belongs_to_container(&cgroup, id) {
            found.push(pid);
        }
    }
    if std::time::Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    found.sort_unstable();
    Ok(found)
}

/// What became of one exact process during a stop.
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub(crate) enum SignalResult {
    /// The pidfd received the signal and pins the exact process incarnation.
    Sent(std::os::fd::OwnedFd),
    /// The process had already ended.
    Gone,
    /// It does not belong to the selected container or is the retained supervisor.
    Skipped,
}

/// Signals only a process still in the selected cgroup, using a pidfd to fence PID reuse.
#[cfg(target_os = "linux")]
pub(crate) fn signal_member(
    raw_pid: i32,
    supervisor_pid: i32,
    id: &str,
    signal: rustix::process::Signal,
    deadline: std::time::Instant,
) -> Result<SignalResult, DriverError> {
    use rustix::io::Errno;
    use rustix::process::{Pid, PidfdFlags, pidfd_open, pidfd_send_signal};

    if std::time::Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    if !valid_id(id) {
        return Err(DriverError::Unsupported(
            "the container runtime did not report an identifiable unit".into(),
        ));
    }
    if raw_pid <= 0 || supervisor_pid <= 0 {
        return Err(DriverError::InvalidSpec(
            "a process identifier must be positive".into(),
        ));
    }
    if raw_pid == supervisor_pid {
        return Ok(SignalResult::Skipped);
    }
    let pid = Pid::from_raw(raw_pid)
        .ok_or_else(|| DriverError::InvalidSpec("a process identifier must be positive".into()))?;
    let handle = match pidfd_open(pid, PidfdFlags::empty()) {
        Ok(handle) => handle,
        Err(Errno::SRCH) => return Ok(SignalResult::Gone),
        Err(_) => {
            return Err(DriverError::Unavailable(
                "the workload process could not be identified safely".into(),
            ));
        }
    };
    let path = format!("/proc/{raw_pid}/cgroup");
    let cgroup = match std::fs::read_to_string(path) {
        Ok(cgroup) => cgroup,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SignalResult::Gone);
        }
        Err(_) => {
            return Err(DriverError::Unavailable(
                "the workload process scope could not be read".into(),
            ));
        }
    };
    if !belongs_to_container(&cgroup, id) {
        return Ok(SignalResult::Skipped);
    }
    if std::time::Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    let result = match pidfd_send_signal(&handle, signal) {
        Ok(()) => Ok(SignalResult::Sent(handle)),
        Err(Errno::SRCH) => Ok(SignalResult::Gone),
        Err(_) => Err(DriverError::Unavailable(
            "the workload process could not be stopped".into(),
        )),
    };
    if std::time::Instant::now() >= deadline {
        return Err(DriverError::Timeout);
    }
    result
}

/// An exited pidfd becomes readable; a live pidfd still pins its PID against reuse.
#[cfg(target_os = "linux")]
pub(crate) fn process_is_live(handle: &std::os::fd::OwnedFd) -> Result<bool, DriverError> {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use std::os::fd::AsFd;

    let mut descriptors = [PollFd::new(handle.as_fd(), PollFlags::POLLIN)];
    let count = poll(&mut descriptors, PollTimeout::ZERO).map_err(|_| {
        DriverError::Unavailable("the workload process identity cannot be checked".into())
    })?;
    if count == 0 {
        return Ok(true);
    }
    let flags = descriptors[0].revents().unwrap_or_else(PollFlags::empty);
    if flags.intersects(PollFlags::POLLIN | PollFlags::POLLHUP) {
        Ok(false)
    } else {
        Err(DriverError::Unavailable(
            "the workload process identity cannot be checked".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_full_runtime_reported_container_identity_is_accepted() {
        let id = "a".repeat(CONTAINER_ID_HEX_DIGITS);
        assert_eq!(container_id(&format!("containerd://{id}")), Ok(id.as_str()));
        assert!(container_id("containerd://aabb").is_err());
        assert!(container_id(&format!("unrecognized://{id}")).is_err());
        assert!(
            container_id(&format!(
                "containerd://{}",
                "A".repeat(CONTAINER_ID_HEX_DIGITS)
            ))
            .is_err()
        );
    }

    #[test]
    fn only_the_exact_container_cgroup_or_its_subgroups_match() {
        let id = "a".repeat(CONTAINER_ID_HEX_DIGITS);
        let other = "b".repeat(CONTAINER_ID_HEX_DIGITS);
        assert!(belongs_to_container(&format!("0::/../{id}\n"), &id));
        assert!(belongs_to_container(
            &format!("0::/kubepods/cri-containerd-{id}.scope/child\n"),
            &id
        ));
        assert!(!belongs_to_container(&format!("0::/../{other}\n"), &id));
        assert!(!belongs_to_container(&format!("0::/../prefix{id}\n"), &id));
        assert!(!belongs_to_container(&format!("2:cpu:/{id}\n"), &id));
        assert!(!belongs_to_container("0::/\n", &id));
    }

    #[test]
    fn a_process_scan_requires_a_visible_unambiguous_proc_mount() {
        let visible = "42 1 0:7 / /proc rw,nosuid - proc proc rw\n";
        let hidden = "42 1 0:7 / /proc rw,nosuid - proc proc rw,hidepid=2\n";
        assert!(visible_proc(visible));
        assert!(!visible_proc(hidden));
        assert!(!visible_proc(""));
        assert!(!visible_proc(&format!("{visible}{visible}")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_unidentified_container_and_the_retained_supervisor_are_never_signalled() {
        let id = "a".repeat(CONTAINER_ID_HEX_DIGITS);
        assert!(matches!(
            signal_member(
                1,
                1,
                &id,
                rustix::process::Signal::TERM,
                std::time::Instant::now() + std::time::Duration::from_secs(5)
            ),
            Ok(SignalResult::Skipped)
        ));
        assert!(
            signal_member(
                1,
                2,
                "short",
                rustix::process::Signal::TERM,
                std::time::Instant::now() + std::time::Duration::from_secs(5)
            )
            .is_err()
        );
        assert!(matches!(
            signal_member(
                1,
                1,
                &id,
                rustix::process::Signal::TERM,
                std::time::Instant::now()
            ),
            Err(DriverError::Timeout)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_pidfd_distinguishes_a_live_child_from_its_reaped_incarnation() {
        use rustix::process::{Pid, PidfdFlags, pidfd_open};
        let mut child = std::process::Command::new("/usr/bin/sleep")
            .arg("10")
            .spawn()
            .unwrap();
        let pid = Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
        let handle = pidfd_open(pid, PidfdFlags::empty()).unwrap();
        assert!(process_is_live(&handle).unwrap());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!process_is_live(&handle).unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_scan_keeps_the_supervisor_and_ignores_processes_of_other_containers() {
        let root = tempfile::tempdir().unwrap();
        let id = "a".repeat(CONTAINER_ID_HEX_DIGITS);
        let other = "b".repeat(CONTAINER_ID_HEX_DIGITS);
        for (pid, scope) in [(7, &id), (8, &id), (9, &other), (10, &id)] {
            let dir = root.path().join(pid.to_string());
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join("cgroup"), format!("0::/../{scope}\n")).unwrap();
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        assert_eq!(members(root.path(), &id, 7, deadline).unwrap(), [8, 10]);
        assert!(members(root.path(), &other, 7, deadline).is_err());
        assert!(members(root.path(), "short", 7, deadline).is_err());
        assert!(matches!(
            members(root.path(), &id, 7, std::time::Instant::now()),
            Err(DriverError::Timeout)
        ));
    }
}
