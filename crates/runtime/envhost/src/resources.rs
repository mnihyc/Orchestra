//! Resource sampling and driver limits.
//!
//! What an environment uses is sampled for the screens that show it against its limits. In a
//! unit that a driver made, the unit is the control group the connector runs in, and its own
//! counters and limits are what is read: memory in use and its ceiling, CPU time used and its
//! quota, processes and their ceiling. Where there is no such group, or the environment is only
//! the processes it started, the numbers are the sum over those processes. The disk is the file
//! system the workspace is on, and the network is what the machine's interfaces moved, as a rate
//! between two samples.
//!
//! A limit is `None` when nothing limits that resource, and a rate is zero for the first sample,
//! which has nothing to compare with.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use orch_api_contract::processes::ResourceSample;
use orch_types::Clock;
use sysinfo::Networks;

use crate::processes::{Processes, ScopeKind};

/// What a control group of version 2 says of itself and its limits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CgroupReading {
    /// Memory in use, in bytes.
    pub memory_current: Option<u64>,
    /// The most memory it may use; `None` for no limit.
    pub memory_max: Option<u64>,
    /// CPU time used, in microseconds.
    pub cpu_usage_usec: Option<u64>,
    /// The CPU it may use in thousandths of a CPU; `None` for no limit.
    pub cpu_limit_millis: Option<i64>,
    /// Processes in it.
    pub pids_current: Option<u64>,
}

/// The number a control-group file holds, or `None` for `max` and for anything else.
fn number(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

/// `cpu.max` is `max <period>` or `<quota> <period>`, in microseconds.
fn cpu_limit(text: &str) -> Option<i64> {
    let mut parts = text.split_whitespace();
    let quota: i64 = parts.next()?.parse().ok()?;
    let period: i64 = parts.next()?.parse().ok()?;
    (period > 0).then(|| quota.saturating_mul(1000) / period)
}

/// `cpu.stat` has a line `usage_usec <n>`.
fn cpu_usage(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix("usage_usec ")?.trim().parse().ok())
}

impl CgroupReading {
    /// Reads the files of the control group at `dir`. A file that is not there says nothing.
    #[must_use]
    pub fn read(dir: &Path) -> Self {
        let file = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
        Self {
            memory_current: file("memory.current").and_then(|t| number(&t)),
            memory_max: file("memory.max").and_then(|t| number(&t)),
            cpu_usage_usec: file("cpu.stat").and_then(|t| cpu_usage(&t)),
            cpu_limit_millis: file("cpu.max").and_then(|t| cpu_limit(&t)),
            pids_current: file("pids.current").and_then(|t| number(&t)),
        }
    }

    /// Whether the group reports its own use at all: the root group of a machine does not.
    #[must_use]
    pub fn reports_use(&self) -> bool {
        self.memory_current.is_some()
    }
}

#[derive(Debug, Clone, Copy)]
struct Previous {
    at: Instant,
    cpu_usec: Option<u64>,
    received: u64,
    sent: u64,
}

/// Samples what an environment uses.
pub struct Sampler {
    scope: ScopeKind,
    processes: Processes,
    clock: Arc<dyn Clock>,
    workspace: PathBuf,
    cgroup: Option<PathBuf>,
    state: Mutex<(Networks, Option<Previous>)>,
}

impl std::fmt::Debug for Sampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sampler")
            .field("scope", &self.scope)
            .field("workspace", &self.workspace)
            .finish_non_exhaustive()
    }
}

impl Sampler {
    /// A sampler for the environment whose workspace is at `workspace`. In the scope of a whole
    /// unit it reads the control group the connector is in, when there is one that reports.
    #[must_use]
    pub fn new(
        scope: ScopeKind,
        processes: Processes,
        clock: Arc<dyn Clock>,
        workspace: PathBuf,
    ) -> Self {
        let cgroup = if scope == ScopeKind::All {
            crate::platform::os::own_cgroup_dir()
        } else {
            None
        };
        Self::with_cgroup(scope, processes, clock, workspace, cgroup)
    }

    /// As [`Sampler::new`], reading the control group at `cgroup` instead of finding the one the
    /// connector is in.
    #[must_use]
    pub fn with_cgroup(
        scope: ScopeKind,
        processes: Processes,
        clock: Arc<dyn Clock>,
        workspace: PathBuf,
        cgroup: Option<PathBuf>,
    ) -> Self {
        Self {
            scope,
            processes,
            clock,
            workspace,
            cgroup,
            state: Mutex::new((Networks::new_with_refreshed_list(), None)),
        }
    }

    /// Takes a sample. It reads files and the process table, so it is for a blocking thread.
    #[must_use]
    pub fn sample(&self) -> ResourceSample {
        let now = Instant::now();
        let reading = self
            .cgroup
            .as_deref()
            .map(CgroupReading::read)
            .filter(CgroupReading::reports_use);
        let (count, process_cpu, process_memory) = self.processes.totals();

        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (networks, previous) = &mut *guard;
        networks.refresh(true);
        let (received, sent) = networks
            .iter()
            .filter(|(name, _)| name.as_str() != "lo")
            .fold((0u64, 0u64), |(rx, tx), (_, data)| {
                (
                    rx.saturating_add(data.total_received()),
                    tx.saturating_add(data.total_transmitted()),
                )
            });

        let cpu_usec = reading.and_then(|r| r.cpu_usage_usec);
        let seconds = previous
            .map(|p| now.duration_since(p.at).as_secs_f64())
            .filter(|s| *s > 0.0);
        let cpu_percent = match (
            reading,
            previous.and_then(|p| p.cpu_usec),
            cpu_usec,
            seconds,
        ) {
            (Some(_), Some(before), Some(after), Some(seconds)) => {
                (after.saturating_sub(before) as f64) / (seconds * 1_000_000.0) * 100.0
            }
            (Some(_), ..) => 0.0,
            (None, ..) => process_cpu,
        };
        let rate = |now_total: u64, before: Option<u64>| match (before, seconds) {
            (Some(before), Some(seconds)) => {
                (now_total.saturating_sub(before) as f64 / seconds) as i64
            }
            _ => 0,
        };
        let net_rx_bps = rate(received, previous.map(|p| p.received));
        let net_tx_bps = rate(sent, previous.map(|p| p.sent));
        *previous = Some(Previous {
            at: now,
            cpu_usec,
            received,
            sent,
        });
        drop(guard);

        let (disk_used_bytes, _) = disk_use(&self.workspace);
        let to_i64 = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
        ResourceSample {
            at: self.clock.time(),
            cpu_percent,
            cpu_limit_millis: reading.and_then(|r| r.cpu_limit_millis),
            memory_bytes: reading
                .and_then(|r| r.memory_current)
                .map_or(process_memory, to_i64),
            memory_limit_bytes: reading.and_then(|r| r.memory_max).map(to_i64),
            disk_used_bytes,
            disk_limit_bytes: None,
            net_rx_bps,
            net_tx_bps,
            pids: reading.and_then(|r| r.pids_current).map_or(count, to_i64),
        }
    }
}

/// What is used, and what there is, on the file system `path` is on, in bytes.
#[cfg(unix)]
fn disk_use(path: &Path) -> (Option<i64>, Option<i64>) {
    match rustix::fs::statvfs(path) {
        Ok(stat) => {
            let block = stat.f_frsize;
            let total = stat.f_blocks.saturating_mul(block);
            let free = stat.f_bfree.saturating_mul(block);
            let to_i64 = |value: u64| i64::try_from(value).ok();
            (to_i64(total.saturating_sub(free)), to_i64(total))
        }
        Err(_) => (None, None),
    }
}

/// What is used, and what there is, on the file system `path` is on, in bytes. Windows is told by
/// the portable library, which does not name the disk of a path, so nothing is said.
#[cfg(not(unix))]
fn disk_use(_path: &Path) -> (Option<i64>, Option<i64>) {
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::SystemClock;

    #[test]
    fn a_control_group_is_read_with_its_ceilings_and_without_them() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("memory.current"), "123456789\n").unwrap();
        std::fs::write(dir.path().join("memory.max"), "max\n").unwrap();
        std::fs::write(
            dir.path().join("cpu.stat"),
            "usage_usec 5000000\nuser_usec 3000000\nsystem_usec 2000000\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("cpu.max"), "max 100000\n").unwrap();
        std::fs::write(dir.path().join("pids.current"), "17\n").unwrap();
        let unlimited = CgroupReading::read(dir.path());
        assert_eq!(unlimited.memory_current, Some(123_456_789));
        assert_eq!(unlimited.memory_max, None, "`max` is no limit");
        assert_eq!(unlimited.cpu_limit_millis, None);
        assert_eq!(unlimited.cpu_usage_usec, Some(5_000_000));
        assert_eq!(unlimited.pids_current, Some(17));
        assert!(unlimited.reports_use());

        std::fs::write(dir.path().join("memory.max"), "8589934592\n").unwrap();
        std::fs::write(dir.path().join("cpu.max"), "400000 100000\n").unwrap();
        let limited = CgroupReading::read(dir.path());
        assert_eq!(limited.memory_max, Some(8 * 1024 * 1024 * 1024));
        assert_eq!(
            limited.cpu_limit_millis,
            Some(4000),
            "four CPUs in thousandths"
        );

        let empty = tempfile::tempdir().unwrap();
        assert!(
            !CgroupReading::read(empty.path()).reports_use(),
            "the root group of a machine reports nothing of its own"
        );
    }

    #[tokio::test]
    async fn a_unit_is_sampled_from_its_control_group_against_its_limits_and_cpu_is_the_change_between_samples()
     {
        let group = tempfile::tempdir().unwrap();
        let write = |name: &str, text: &str| std::fs::write(group.path().join(name), text).unwrap();
        write("memory.current", "1000000\n");
        write("memory.max", "2000000\n");
        write("cpu.stat", "usage_usec 1000000\n");
        write("cpu.max", "200000 100000\n");
        write("pids.current", "9\n");
        let workspace = tempfile::tempdir().unwrap();
        let processes = Processes::new(ScopeKind::All, Arc::new(SystemClock));
        let sampler = Sampler::with_cgroup(
            ScopeKind::All,
            processes,
            Arc::new(SystemClock),
            workspace.path().to_path_buf(),
            Some(group.path().to_path_buf()),
        );
        let started = std::time::Instant::now();
        let first = sampler.sample();
        assert_eq!(
            (first.memory_bytes, first.memory_limit_bytes),
            (1_000_000, Some(2_000_000))
        );
        assert_eq!((first.cpu_limit_millis, first.pids), (Some(2000), 9));
        assert!(
            first.cpu_percent.abs() < f64::EPSILON,
            "the first sample has nothing to compare with"
        );
        assert_eq!((first.net_rx_bps, first.net_tx_bps), (0, 0));
        assert!(first.disk_used_bytes.is_some_and(|used| used > 0));

        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        write("cpu.stat", "usage_usec 1250000\n");
        let second = sampler.sample();
        let longest = started.elapsed();
        // A quarter of a second of CPU time over the time between the two samples, which is no
        // shorter than the wait and no longer than the whole test took on a busy machine: one CPU
        // at most, and not less than the share of the longest time it could have been.
        let lowest = 100.0 * 250_000.0 / longest.as_micros() as f64;
        assert!(
            second.cpu_percent >= lowest * 0.99 && second.cpu_percent <= 100.5,
            "{} is not between {lowest} and 100",
            second.cpu_percent
        );
        assert!(second.net_rx_bps >= 0 && second.net_tx_bps >= 0);
    }

    #[test]
    fn without_a_control_group_the_processes_the_environment_started_are_what_is_summed() {
        let workspace = tempfile::tempdir().unwrap();
        let processes = Processes::new(ScopeKind::Started, Arc::new(SystemClock));
        let sampler = Sampler::with_cgroup(
            ScopeKind::Started,
            processes,
            Arc::new(SystemClock),
            workspace.path().to_path_buf(),
            None,
        );
        let sample = sampler.sample();
        assert_eq!(sample.pids, 0, "none was started");
        assert_eq!(sample.memory_bytes, 0);
        assert_eq!(sample.memory_limit_bytes, None, "nothing limits it");
        assert_eq!(sample.cpu_limit_millis, None);
    }
}
