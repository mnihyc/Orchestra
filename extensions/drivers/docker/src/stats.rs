//! Container resource facts.
//!
//! What a unit uses now, in the terms of the contract: processor time as a percentage of one processor, memory
//! without what the kernel keeps as cache, processes, the traffic of its networks as a rate, and the disk its
//! own layer takes. The daemon reports counters; processor time and traffic are rates, which need two readings,
//! so the first reading of the network of a unit has a rate of nothing and later ones are measured against the
//! reading before (when that is recent enough to mean anything). The disk is the slowest thing to ask the daemon
//! and the one that changes slowest, so it is asked for at most once a minute for each unit.

use std::collections::HashMap;
use std::time::Duration;

use bollard::models::{ContainerInspectResponse, ContainerStatsResponse};
use bollard::query_parameters::{InspectContainerOptionsBuilder, StatsOptionsBuilder};
use futures::StreamExt;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::units::{ResourceSample, UnitRef};
use orch_types::Time;

use crate::client::map_error;
use crate::driver::DockerDriver;
use crate::units::LABEL_DISK_LIMIT;
use crate::volumes::BYTES_PER_GB;

/// How long a reading of the traffic stays good as the start of a rate: the system asks for a sample every few
/// seconds, so a reading older than a minute is not a reading of the interval that is asked about.
pub const RATE_WINDOW: Duration = Duration::from_secs(60);

/// How long the size of a unit's own layer is kept before the daemon is asked again.
pub const DISK_REFRESH: Duration = Duration::from_secs(60);

/// A thousandth of a processor, in the unit the daemon limits processor time in (billionths).
const NANO_CPUS_PER_MILLI: i64 = 1_000_000;

/// What is remembered of a unit between two samples.
#[derive(Debug, Clone, Default)]
pub struct Memo {
    traffic: Option<(Time, u64, u64)>,
    disk: Option<(Time, i64)>,
}

/// The memory a unit uses: what is charged to it less the cache the kernel can take back, which is how the
/// daemon's own tools count it.
#[must_use]
pub fn memory_in_use(response: &ContainerStatsResponse) -> i64 {
    let Some(memory) = response.memory_stats.as_ref() else {
        return 0;
    };
    let usage = memory.usage.unwrap_or(0);
    let cache = memory
        .stats
        .as_ref()
        .and_then(|stats| {
            stats
                .get("inactive_file")
                .or_else(|| stats.get("total_inactive_file"))
        })
        .copied()
        .unwrap_or(0);
    i64::try_from(usage.saturating_sub(cache)).unwrap_or(i64::MAX)
}

/// Processor time, in percent of one processor, between the reading before and this one.
#[must_use]
pub fn cpu_percent(response: &ContainerStatsResponse) -> f64 {
    let (Some(now), Some(before)) = (response.cpu_stats.as_ref(), response.precpu_stats.as_ref())
    else {
        return 0.0;
    };
    let used = |stats: &bollard::models::ContainerCpuStats| {
        stats.cpu_usage.as_ref().and_then(|usage| usage.total_usage)
    };
    let (Some(used_now), Some(used_before)) = (used(now), used(before)) else {
        return 0.0;
    };
    let (Some(system_now), Some(system_before)) = (now.system_cpu_usage, before.system_cpu_usage)
    else {
        return 0.0;
    };
    let used_delta = used_now.saturating_sub(used_before) as f64;
    let system_delta = system_now.saturating_sub(system_before) as f64;
    if system_delta <= 0.0 {
        return 0.0;
    }
    let processors = now
        .online_cpus
        .map(f64::from)
        .or_else(|| {
            now.cpu_usage
                .as_ref()
                .and_then(|usage| usage.percpu_usage.as_ref())
                .map(|list| list.len() as f64)
        })
        .unwrap_or(1.0);
    used_delta / system_delta * processors * 100.0
}

/// The bytes received and sent over every network of the unit since it started.
#[must_use]
pub fn traffic(response: &ContainerStatsResponse) -> (u64, u64) {
    response
        .networks
        .iter()
        .flat_map(HashMap::values)
        .fold((0, 0), |(rx, tx), network| {
            (
                rx.saturating_add(network.rx_bytes.unwrap_or(0)),
                tx.saturating_add(network.tx_bytes.unwrap_or(0)),
            )
        })
}

/// Bits per second between two readings of a byte counter.
fn bits_per_second(now: u64, before: u64, seconds: f64) -> i64 {
    if seconds <= 0.0 {
        return 0;
    }
    let bits = now.saturating_sub(before) as f64 * 8.0 / seconds;
    bits as i64
}

/// The sample of a unit, from the daemon's reading and the inspection of its container.
#[must_use]
pub fn sample(
    response: &ContainerStatsResponse,
    inspected: &ContainerInspectResponse,
    at: Time,
    memo: &mut Memo,
    host_memory_bytes: i64,
) -> ResourceSample {
    let (rx, tx) = traffic(response);
    let (rx_rate, tx_rate) = match memo.traffic {
        Some((then, rx_before, tx_before))
            if at > then && at.get() - then.get() < chrono_window() =>
        {
            let seconds = (at.unix_millis() - then.unix_millis()) as f64 / 1000.0;
            (
                bits_per_second(rx, rx_before, seconds),
                bits_per_second(tx, tx_before, seconds),
            )
        }
        _ => (0, 0),
    };
    memo.traffic = Some((at, rx, tx));
    let host = inspected.host_config.as_ref();
    let memory_limit = host
        .and_then(|host| host.memory)
        .filter(|limit| *limit > 0 && (host_memory_bytes == 0 || *limit < host_memory_bytes));
    let disk_limit = inspected
        .config
        .as_ref()
        .and_then(|config| config.labels.as_ref())
        .and_then(|labels| labels.get(LABEL_DISK_LIMIT))
        .and_then(|gb| gb.parse::<i64>().ok())
        .map(|gb| gb.saturating_mul(i64::try_from(BYTES_PER_GB).unwrap_or(i64::MAX)));
    ResourceSample {
        at,
        cpu_percent: cpu_percent(response),
        cpu_limit_millis: host
            .and_then(|host| host.nano_cpus)
            .filter(|nano| *nano > 0)
            .map(|nano| nano / NANO_CPUS_PER_MILLI),
        memory_bytes: memory_in_use(response),
        memory_limit_bytes: memory_limit,
        disk_used_bytes: inspected.size_rw,
        disk_limit_bytes: disk_limit,
        net_rx_bps: rx_rate,
        net_tx_bps: tx_rate,
        pids: response
            .pids_stats
            .as_ref()
            .and_then(|pids| pids.current)
            .map_or(0, |current| i64::try_from(current).unwrap_or(i64::MAX)),
    }
}

/// [`RATE_WINDOW`] as the time library of the clock counts.
fn chrono_window() -> chrono::Duration {
    chrono::Duration::from_std(RATE_WINDOW).unwrap_or(chrono::Duration::MAX)
}

impl DockerDriver {
    /// What a unit uses now; see `EnvironmentDriver::stats`.
    pub(crate) async fn stats_unit(&self, unit: &UnitRef) -> Result<ResourceSample, DriverError> {
        let container = self.container_of(unit).await?.ok_or_else(|| {
            DriverError::NotFound(format!("there is no unit {} of this driver", unit.name))
        })?;
        let id = container.id.clone().unwrap_or_default();
        if container.state.as_ref().and_then(|state| state.running) != Some(true) {
            return Err(DriverError::NotFound(format!(
                "the unit {} is not running",
                unit.name
            )));
        }
        // One reading that carries the one before it, which the daemon takes a second to collect.
        let options = StatsOptionsBuilder::default()
            .stream(false)
            .one_shot(false)
            .build();
        let mut stream = self.daemon.docker().stats(&id, Some(options));
        let response = stream
            .next()
            .await
            .ok_or_else(|| DriverError::Internal("the daemon gave no reading".to_owned()))?
            .map_err(map_error)?;
        let at = self.clock.time();
        // The size of the unit's own layer is asked for now and then; between times the last answer stands.
        let window = chrono::Duration::from_std(DISK_REFRESH).unwrap_or(chrono::Duration::MAX);
        let remembered = {
            let memos = self
                .memos
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            memos
                .get(&id)
                .and_then(|memo| memo.disk)
                .filter(|(then, _)| at.get() - then.get() < window)
                .map(|(_, size)| size)
        };
        let mut inspected = container;
        if remembered.is_some() {
            inspected.size_rw = remembered;
        } else if let Some(fresh) = self
            .daemon
            .docker()
            .inspect_container(
                &id,
                Some(InspectContainerOptionsBuilder::default().size(true).build()),
            )
            .await
            .ok()
            .and_then(|measured| measured.size_rw)
        {
            inspected.size_rw = Some(fresh);
        }
        let mut memos = self
            .memos
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let memo = memos.entry(id).or_default();
        if remembered.is_none()
            && let Some(size) = inspected.size_rw
        {
            memo.disk = Some((at, size));
        }
        Ok(sample(
            &response,
            &inspected,
            at,
            memo,
            self.daemon.facts().memory_total_bytes,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reading(
        total: u64,
        system: u64,
        previous_total: u64,
        previous_system: u64,
    ) -> ContainerStatsResponse {
        serde_json::from_value(json!({
            "read": "2026-10-11T10:00:01Z",
            "cpu_stats": { "cpu_usage": { "total_usage": total }, "system_cpu_usage": system, "online_cpus": 4 },
            "precpu_stats": { "cpu_usage": { "total_usage": previous_total }, "system_cpu_usage": previous_system, "online_cpus": 4 },
            "memory_stats": { "usage": 300_000_000_u64, "limit": 16_000_000_000_u64, "stats": { "inactive_file": 100_000_000_u64 } },
            "pids_stats": { "current": 12 },
            "networks": { "eth0": { "rx_bytes": 1000, "tx_bytes": 2000 }, "eth1": { "rx_bytes": 500, "tx_bytes": 0 } }
        }))
        .unwrap()
    }

    #[test]
    fn processor_time_is_the_share_of_the_hosts_time_the_unit_used_times_the_processors() {
        // The unit used a quarter of the time of four processors, which is one whole processor.
        let response = reading(1_250, 5_000, 0, 1_000);
        assert!(
            (cpu_percent(&response) - 125.0).abs() < 0.001,
            "{}",
            cpu_percent(&response)
        );
        assert_eq!(cpu_percent(&reading(5, 5, 5, 5)), 0.0, "no time passed");
        assert_eq!(cpu_percent(&ContainerStatsResponse::default()), 0.0);
    }

    #[test]
    fn memory_is_what_is_charged_less_the_cache_and_traffic_is_summed_over_the_networks() {
        let response = reading(0, 0, 0, 0);
        assert_eq!(memory_in_use(&response), 200_000_000);
        assert_eq!(traffic(&response), (1500, 2000));
    }

    #[test]
    fn a_sample_has_rates_only_from_the_second_reading_and_the_limits_of_the_container() {
        let inspected: ContainerInspectResponse = serde_json::from_value(json!({
            "Id": "abc", "SizeRw": 4096,
            "HostConfig": { "NanoCpus": 4_000_000_000_i64, "Memory": 8_589_934_592_i64 },
            "Config": { "Labels": { "orch.disk_limit_gb": "50" } }
        }))
        .unwrap();
        let mut memo = Memo::default();
        let first_at: Time = "2026-10-11T10:00:00.000Z".parse().unwrap();
        let first = sample(
            &reading(0, 0, 0, 0),
            &inspected,
            first_at,
            &mut memo,
            16_000_000_000,
        );
        assert_eq!((first.net_rx_bps, first.net_tx_bps), (0, 0));
        assert_eq!(first.cpu_limit_millis, Some(4000));
        assert_eq!(first.memory_limit_bytes, Some(8_589_934_592));
        assert_eq!(first.disk_used_bytes, Some(4096));
        assert_eq!(first.disk_limit_bytes, Some(50 * (1 << 30)));
        assert_eq!(first.pids, 12);
        let mut next = reading(0, 0, 0, 0);
        next.networks
            .as_mut()
            .unwrap()
            .get_mut("eth0")
            .unwrap()
            .rx_bytes = Some(1000 + 5_000);
        let second_at: Time = "2026-10-11T10:00:05.000Z".parse().unwrap();
        let second = sample(&next, &inspected, second_at, &mut memo, 16_000_000_000);
        assert_eq!(second.net_rx_bps, 5_000 * 8 / 5);
        assert_eq!(second.net_tx_bps, 0);
        let much_later: Time = "2026-10-11T11:00:00.000Z".parse().unwrap();
        let stale = sample(&next, &inspected, much_later, &mut memo, 16_000_000_000);
        assert_eq!(
            stale.net_rx_bps, 0,
            "a reading an hour old is not the start of a rate"
        );
    }

    #[test]
    fn a_memory_limit_as_large_as_the_host_is_no_limit() {
        let inspected: ContainerInspectResponse = serde_json::from_value(json!({
            "Id": "abc", "HostConfig": { "Memory": 16_000_000_000_i64 }
        }))
        .unwrap();
        let at: Time = "2026-10-11T10:00:00.000Z".parse().unwrap();
        let sampled = sample(
            &reading(0, 0, 0, 0),
            &inspected,
            at,
            &mut Memo::default(),
            16_000_000_000,
        );
        assert_eq!(sampled.memory_limit_bytes, None);
    }
}
