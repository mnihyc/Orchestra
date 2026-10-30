//! Unit identity, lifecycle facts and succession.
//!
//! A **unit** is whatever a driver creates for one runtime: a container, a microVM, a process group. Every
//! unit carries the identifiers `orch.system`, `orch.runtime_id` and `orch.kind` (labels, or the driver's
//! equivalent), and the node never touches a unit without its system's identifier.
//!
//! A driver is asked for the state of a unit ([`UnitState`]), and every thirty seconds and at start for all
//! the units of its system, so that what is real can be reconciled with what is recorded
//! ([`reconcile`]). A recreate, an update and a move keep the unit they replace until its successor has
//! passed its checks; the successor is a runtime of its own with a unit of its own, on the same host it uses
//! the same volumes, and the two never run at once ([`successor_may_start`]).

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::runtime::RuntimeKind;

/// How often, and at start, the node asks each driver for its system's units: thirty seconds, which finds a
/// unit that died within the minute and costs one cheap listing.
pub const RECONCILE_INTERVAL_SECS: u64 = 30;

/// One unit, as a driver names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct UnitRef {
    /// The driver's own reference for what it made.
    pub reference: String,
    /// The unit's name (`orch-<kind>-<12 hex digits>`).
    pub name: String,
    /// The runtime it is for, from its `orch.runtime_id`.
    pub runtime_id: Id,
    /// Its pool, from its `orch.kind`.
    pub kind: RuntimeKind,
}

/// Where a unit stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnitStateKind {
    /// It is running.
    Running,
    /// It exists and is not running.
    Stopped,
    /// There is no such unit.
    Absent,
    /// It ended with a failure.
    Failed,
}

/// The state of a unit, with detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnitState {
    /// Where it stands.
    pub state: UnitStateKind,
    /// What the driver can say of it, such as why it failed.
    pub detail: Option<String>,
    /// How its main process ended, when it did.
    pub exit_code: Option<i64>,
    /// When it last started.
    pub started_at: Option<Time>,
    /// When it last ended.
    pub finished_at: Option<Time>,
}

impl UnitState {
    /// A unit in `state` and nothing more is known.
    #[must_use]
    pub fn of(state: UnitStateKind) -> Self {
        Self {
            state,
            detail: None,
            exit_code: None,
            started_at: None,
            finished_at: None,
        }
    }
}

/// What a unit used, at one moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceSample {
    /// When it was taken.
    pub at: Time,
    /// Processor use, in percent of one processor.
    pub cpu_percent: f64,
    /// The processor limit, in thousandths of a processor.
    pub cpu_limit_millis: Option<i64>,
    /// Memory in use, in bytes.
    pub memory_bytes: i64,
    /// The memory limit, in bytes.
    pub memory_limit_bytes: Option<i64>,
    /// Disk in use, in bytes.
    pub disk_used_bytes: Option<i64>,
    /// The disk limit, in bytes.
    pub disk_limit_bytes: Option<i64>,
    /// Bits per second received.
    pub net_rx_bps: i64,
    /// Bits per second sent.
    pub net_tx_bps: i64,
    /// Processes and threads.
    pub pids: i64,
}

/// What the API recorded of a runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Recorded {
    /// The row says it runs.
    Running,
    /// The row says it is stopped.
    Stopped,
    /// The row says it was removed.
    Removed,
}

/// What the API does about a difference between a row and a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reconciliation {
    /// Nothing: row and unit agree.
    Nothing,
    /// Mark the runtime failed, with the unit's exit status. An environment goes `failed`; a provider's
    /// runtime is created again by the next pre-flight that needs it.
    MarkFailed,
    /// Stop the unit.
    Stop,
    /// Remove the unit, never its volumes.
    Remove,
}

/// What to do when a runtime's row says `recorded` and its unit, listed by the driver, is in `found`
/// (`None` when the driver lists no such unit). A unit that has a system's identifier and no row at all is
/// removed and logged ([`reconcile_orphan`]).
#[must_use]
pub fn reconcile(recorded: Recorded, found: Option<UnitStateKind>) -> Reconciliation {
    use UnitStateKind::{Absent, Failed, Running, Stopped};
    match (recorded, found) {
        (Recorded::Running, None | Some(Absent | Stopped | Failed)) => Reconciliation::MarkFailed,
        (Recorded::Stopped, Some(Running)) => Reconciliation::Stop,
        (Recorded::Removed, Some(Running | Stopped | Failed)) => Reconciliation::Remove,
        _ => Reconciliation::Nothing,
    }
}

/// What to do with a unit that carries the system's identifier and has no row: remove it, never its
/// volumes.
#[must_use]
pub const fn reconcile_orphan() -> Reconciliation {
    Reconciliation::Remove
}

/// Whether the unit that succeeds another may start. On the same host the two use the same volumes and
/// never run at once, so the predecessor must have stopped; on another host nothing is shared.
#[must_use]
pub const fn successor_may_start(shares_volumes: bool, predecessor: UnitStateKind) -> bool {
    !shares_volumes || !matches!(predecessor, UnitStateKind::Running)
}

#[cfg(test)]
mod tests {
    use super::*;
    use UnitStateKind::{Absent, Failed, Running, Stopped};
    use serde_json::json;

    #[test]
    fn a_runtime_that_is_recorded_as_running_and_has_no_running_unit_is_marked_failed() {
        for found in [None, Some(Absent), Some(Stopped), Some(Failed)] {
            assert_eq!(
                reconcile(Recorded::Running, found),
                Reconciliation::MarkFailed
            );
        }
        assert_eq!(
            reconcile(Recorded::Running, Some(Running)),
            Reconciliation::Nothing
        );
    }

    #[test]
    fn a_unit_that_runs_against_a_row_that_says_it_does_not_is_stopped_or_removed() {
        assert_eq!(
            reconcile(Recorded::Stopped, Some(Running)),
            Reconciliation::Stop
        );
        assert_eq!(
            reconcile(Recorded::Stopped, Some(Stopped)),
            Reconciliation::Nothing
        );
        assert_eq!(
            reconcile(Recorded::Removed, Some(Running)),
            Reconciliation::Remove
        );
        assert_eq!(
            reconcile(Recorded::Removed, Some(Stopped)),
            Reconciliation::Remove
        );
        assert_eq!(reconcile(Recorded::Removed, None), Reconciliation::Nothing);
        assert_eq!(reconcile_orphan(), Reconciliation::Remove);
    }

    #[test]
    fn two_units_on_the_same_volumes_never_run_at_once() {
        assert!(!successor_may_start(true, Running));
        assert!(successor_may_start(true, Stopped));
        assert!(successor_may_start(true, Absent));
        assert!(
            successor_may_start(false, Running),
            "another host shares nothing"
        );
    }

    #[test]
    fn a_unit_and_its_state_read_as_the_driver_reports_them() {
        let unit: UnitRef = serde_json::from_value(json!({
            "reference": "3f9a1c", "name": "orch-environment-0199abcd1234",
            "runtime_id": "01990000-0000-7000-8000-0199abcd1234", "kind": "environment"
        }))
        .unwrap();
        assert_eq!(unit.kind, RuntimeKind::Environment);
        let state: UnitState = serde_json::from_value(json!({
            "state": "failed", "detail": "out of memory", "exit_code": 137,
            "started_at": "2026-10-11T10:00:00.000Z", "finished_at": "2026-10-11T10:05:00.000Z"
        }))
        .unwrap();
        assert_eq!(state.state, UnitStateKind::Failed);
        assert_eq!(UnitState::of(Absent).exit_code, None);
    }

    #[test]
    fn a_sample_of_use_reads_as_the_api_shows_it() {
        let sample: ResourceSample = serde_json::from_value(json!({
            "at": "2026-10-11T10:00:00.000Z", "cpu_percent": 12.5, "cpu_limit_millis": 4000,
            "memory_bytes": 1_000_000, "memory_limit_bytes": null, "disk_used_bytes": null, "disk_limit_bytes": null,
            "net_rx_bps": 10, "net_tx_bps": 20, "pids": 31
        }))
        .unwrap();
        assert_eq!(sample.pids, 31);
        assert_eq!(
            serde_json::to_value(&sample).unwrap()["cpu_limit_millis"],
            json!(4000)
        );
    }
}
