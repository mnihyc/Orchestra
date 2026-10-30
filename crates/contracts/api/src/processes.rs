//! Process and resource views.
//!
//! Every process of an environment is listed, whichever thread started it,
//! each with the thread it belongs to when that is known. How much the
//! environment as a whole used is a series of [`ResourceSample`]s, which the
//! API keeps in memory for a few hours and does not store.

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::environment::ThroughThreadQuery;
use crate::routes::Access::Permission;

/// How many minutes of samples a request for them returns when it does not say.
pub const RESOURCE_MINUTES_DEFAULT: u32 = 60;
/// The most minutes of samples a request may ask for: six hours, which is as long as the API
/// keeps them.
pub const RESOURCE_MINUTES_MAX: u32 = 360;
/// How many seconds lie between two samples.
pub const RESOURCE_SAMPLE_SECONDS: u32 = 5;

/// One process of an environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProcessInfo {
    /// Its number.
    pub pid: u32,
    /// The number of the process that started it.
    pub ppid: u32,
    /// Its name.
    pub name: String,
    /// Its command line.
    pub command: String,
    /// Who it runs as.
    pub user: String,
    /// What it is doing, as the system reports it.
    pub state: String,
    /// How much of the CPU it uses, in percent of one CPU.
    pub cpu_percent: f64,
    /// How much memory it holds, in bytes.
    pub memory_bytes: i64,
    /// When it started.
    pub started_at: Time,
    /// The thread it belongs to, when that is known.
    pub thread_id: Option<Id>,
    /// The item of that thread whose tool call started it, when that is known.
    pub item_id: Option<Id>,
}

/// What an environment as a whole used at one moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceSample {
    /// When it was taken.
    pub at: Time,
    /// How much of the CPU was used, in percent of one CPU.
    pub cpu_percent: f64,
    /// The most CPU it may use, in thousandths of a CPU; `null` for no limit.
    pub cpu_limit_millis: Option<i64>,
    /// Memory in use, in bytes.
    pub memory_bytes: i64,
    /// The most memory it may use, in bytes; `null` for no limit.
    pub memory_limit_bytes: Option<i64>,
    /// Disk in use, in bytes, where that is known.
    pub disk_used_bytes: Option<i64>,
    /// The most disk it may use, in bytes; `null` for no limit.
    pub disk_limit_bytes: Option<i64>,
    /// Bytes a second received.
    pub net_rx_bps: i64,
    /// Bytes a second sent.
    pub net_tx_bps: i64,
    /// How many processes there were.
    pub pids: i64,
}

/// A signal that may be sent to a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum ProcessSignal {
    /// Interrupt, as the keyboard does.
    Int,
    /// Ask it to end.
    Term,
    /// End it at once.
    Kill,
}

/// Every process of an environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentProcessListReply {
    /// When the list was taken.
    pub as_of: Time,
    /// The processes.
    pub processes: Vec<ProcessInfo>,
}

/// Sends a signal to a process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentProcessSignalBody {
    /// The signal.
    pub signal: ProcessSignal,
    /// When the process started, which has to be what the list said: a process number that was
    /// reused is never signalled.
    pub started_at: Time,
}

/// How far back to read the samples.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentResourceListQuery {
    /// The thread the caller works beside. It gives nothing: what is needed is held on the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
    /// How many minutes back, at most [`RESOURCE_MINUTES_MAX`]; [`RESOURCE_MINUTES_DEFAULT`] when
    /// left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minutes: Option<u32>,
}

crate::endpoints! {
    /// Lists every process of an environment, each with the thread it belongs to when that is known.
    EnvironmentProcessList: GET "/environments/{id}/processes", Permission("terminal.view"), query ThroughThreadQuery, none, json EnvironmentProcessListReply, 200;
    /// Sends a signal to one process of an environment.
    EnvironmentProcessSignal: POST "/environments/{id}/processes/{pid}/signal", Permission("terminal.manage"), query ThroughThreadQuery, json EnvironmentProcessSignalBody, none, 204;
    /// Lists what the environment as a whole used lately, oldest first.
    EnvironmentResourceList: GET "/environments/{id}/resources", Permission("terminal.view"), query EnvironmentResourceListQuery, none, json Vec<ResourceSample>, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_signal_is_written_in_capitals_and_names_the_start_of_the_process() {
        let body: EnvironmentProcessSignalBody = serde_json::from_value(
            json!({ "signal": "TERM", "started_at": "2026-10-08T17:04:05.123Z" }),
        )
        .unwrap();
        assert_eq!(body.signal, ProcessSignal::Term);
        assert!(
            serde_json::from_value::<EnvironmentProcessSignalBody>(
                json!({ "signal": "term", "started_at": "2026-10-08T17:04:05.123Z" })
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<EnvironmentProcessSignalBody>(json!({ "signal": "KILL" }))
                .is_err(),
            "without the start time a reused number could be signalled"
        );
    }

    #[test]
    fn a_process_says_which_thread_started_it_when_that_is_known() {
        let written = json!({
            "pid": 4121, "ppid": 1, "name": "node", "command": "node server.js", "user": "agent",
            "state": "S", "cpu_percent": 12.5, "memory_bytes": 73400320,
            "started_at": "2026-10-08T17:04:05.123Z",
            "thread_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "item_id": null
        });
        let process: ProcessInfo = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(process.item_id, None);
        assert!(process.thread_id.is_some());
        assert_eq!(serde_json::to_value(&process).unwrap(), written);
    }

    #[test]
    fn a_sample_leaves_a_limit_null_when_there_is_none() {
        let sample: ResourceSample = serde_json::from_value(json!({
            "at": "2026-10-08T17:04:05.123Z", "cpu_percent": 3.0, "cpu_limit_millis": null,
            "memory_bytes": 1048576, "memory_limit_bytes": 2147483648_i64,
            "disk_used_bytes": null, "disk_limit_bytes": null,
            "net_rx_bps": 0, "net_tx_bps": 120, "pids": 14
        }))
        .unwrap();
        assert_eq!(sample.cpu_limit_millis, None);
        assert_eq!(sample.memory_limit_bytes, Some(2_147_483_648));
    }
}
