//! Host unit process and resource facts.
//!
//! Only the process identified by its Linux start ticks and its current descendants belong to a host unit.

use orch_driver_contract::driver::DriverError;
use orch_driver_contract::runtime::Resources;
use orch_driver_contract::units::ResourceSample;
use orch_types::Clock;
use serde::{Deserialize, Serialize};

/// A PID and its kernel start identity, which survives node restart and refuses a reused PID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    /// The group's leading connector or account-switching process.
    pub pid: u32,
    /// Start ticks from Linux procfs, never a timestamp rounded to seconds.
    pub start_ticks: u64,
}

/// The exact process identity, absent when it exited, is a zombie, or procfs cannot verify it.
#[must_use]
pub fn identity(pid: u32) -> Option<ProcessIdentity> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (_, fields) = text.rsplit_once(") ")?;
        let fields: Vec<_> = fields.split_whitespace().collect();
        if matches!(fields.first().copied(), Some("Z" | "X")) {
            return None;
        }
        // procfs field 22 is starttime; after the parenthesized command, indexing begins at field 3.
        const START_TICKS_FIELD: usize = 19;
        Some(ProcessIdentity {
            pid,
            start_ticks: fields.get(START_TICKS_FIELD)?.parse().ok()?,
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// Tests the full saved identity instead of merely asking whether its PID exists.
#[must_use]
pub fn alive(process: ProcessIdentity) -> bool {
    identity(process.pid) == Some(process)
}

pub(crate) fn tree(root: ProcessIdentity) -> Vec<ProcessIdentity> {
    if !alive(root) {
        return Vec::new();
    }
    let system = sysinfo::System::new_all();
    let mut children = std::collections::BTreeMap::<u32, Vec<u32>>::new();
    for (pid, process) in system.processes() {
        if let Some(parent) = process.parent() {
            children
                .entry(parent.as_u32())
                .or_default()
                .push(pid.as_u32());
        }
    }
    let mut queue = vec![root.pid];
    let mut result = Vec::new();
    while let Some(pid) = queue.pop() {
        if let Some(process) = identity(pid) {
            result.push(process);
        }
        if let Some(children) = children.remove(&pid) {
            queue.extend(children);
        }
    }
    result
}

/// Measures this connector's process tree and explicitly leaves unmeasured disk and network counters absent.
pub fn sample(
    process: ProcessIdentity,
    resources: &Resources,
    clock: &dyn Clock,
) -> Result<ResourceSample, DriverError> {
    if !alive(process) {
        return Err(DriverError::NotFound(
            "the host connector is not running".into(),
        ));
    }
    let identities = tree(process);
    let system = sysinfo::System::new_all();
    let mut memory = 0u64;
    let mut cpu = 0.0f64;
    for identity in &identities {
        if !alive(*identity) {
            continue;
        }
        if let Some(process) = system.process(sysinfo::Pid::from_u32(identity.pid)) {
            memory = memory.saturating_add(process.memory());
            cpu += f64::from(process.cpu_usage());
        }
    }
    const BYTES_PER_MB: i64 = 1 << 20;
    Ok(ResourceSample {
        at: clock.time(),
        cpu_percent: cpu,
        cpu_limit_millis: resources.cpu_millis,
        memory_bytes: i64::try_from(memory).unwrap_or(i64::MAX),
        memory_limit_bytes: resources
            .memory_mb
            .map(|mb| mb.saturating_mul(BYTES_PER_MB)),
        disk_used_bytes: None,
        disk_limit_bytes: None,
        net_rx_bps: 0,
        net_tx_bps: 0,
        pids: i64::try_from(identities.len()).unwrap_or(i64::MAX),
    })
}
