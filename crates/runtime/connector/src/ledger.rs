//! What the connector hosts, kept on disk: the harness processes it started and the sessions that are open in them.
//!
//! A connector that stops and starts again has to know what it was running, to bring the harness
//! processes back and to say what became of the sessions in them (`reconciliation`). The record is one
//! small file, `state/hosted.json` in the data directory, written whole and put in place in one step
//! each time something is added or ends. A call of Orch's tools retains its native source and reference
//! before publication ([`RetainedCall`]); uncertain outcomes remain for the existing ten-minute interval,
//! so a retry after a restart keeps its own identity instead of borrowing another turn's answer.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use orch_link_contract::commands::AgentStart;
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};

/// The directory of the data directory that holds the record.
pub const STATE_DIR: &str = "state";
/// The record's file in it.
pub const HOSTED_FILE: &str = "hosted.json";

/// Why the record could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    /// The file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// What the system said.
        source: std::io::Error,
    },
    /// The file is not what the connector wrote.
    #[error("{path} is damaged: {message}")]
    Damaged {
        /// The file.
        path: PathBuf,
        /// What is wrong with it.
        message: String,
    },
}

/// A session that is open in a harness process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostedSession {
    /// The thread.
    pub thread_id: Id,
    /// The stretch of the thread it serves.
    pub segment_id: Id,
    /// The harness's id for it.
    pub native_session_ref: String,
    /// The turn that was running when the record was last written.
    pub running_turn: Option<Id>,
}

/// A harness process the connector started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostedHarness {
    /// What the system asked with, to start it again the same way.
    pub start: AgentStart,
    /// Its home directory as it was worked out when it was started.
    pub home: Option<String>,
    /// The sessions open in it.
    pub sessions: Vec<HostedSession>,
}

impl HostedHarness {
    /// Whether this is the process of `provider_id` and `agent_id`.
    #[must_use]
    pub fn is(&self, provider_id: Id, agent_id: Id) -> bool {
        self.start.provider_id == provider_id && self.start.agent_id == agent_id
    }
}

/// The native-source identity of an admitted own-tool call, retained before publication.
/// An uncertain retry with equal arguments in that same source keeps its reference and late answer;
/// a different turn or native lane cannot inherit it. Ordinary successful calls leave this replay set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetainedCall {
    /// The thread whose agent made it.
    pub thread_id: Id,
    /// The actual admitted native source; absent only in an older record whose source cannot be inferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ToolCallSource>,
    /// Whether a caller timed out or a restart left its original outcome unconfirmed.
    #[serde(
        default = "retained_timeout",
        skip_serializing_if = "is_retained_timeout"
    )]
    pub timed_out: bool,
    /// The tool.
    pub tool: String,
    /// The hash of its arguments, written canonically.
    pub arguments_hash: String,
    /// The reference it was made under.
    pub call_ref: String,
    /// Until when it is kept, in seconds since the epoch.
    pub until: i64,
    /// The answer that came after nobody waited for it any more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Result<Json, orch_link_contract::commands::ToolError>>,
}

fn retained_timeout() -> bool {
    true
}
fn is_retained_timeout(value: &bool) -> bool {
    *value
}

/// A logical call survives listener/process refresh, but cannot cross a native turn or lane.
/// Only the fingerprint is kept; listener credentials and session keys are never persisted here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolCallSource {
    /// The immutable recorded native stream segment.
    pub segment_id: Id,
    /// The actual turn reserved in that session.
    pub turn_id: Id,
    /// Fingerprint of the actual native conversation reference at admission.
    pub native: orch_types::Hash,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Record {
    harnesses: Vec<HostedHarness>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retained_calls: Vec<RetainedCall>,
}

/// The record of what the connector hosts.
#[derive(Debug)]
pub struct Ledger {
    path: PathBuf,
    record: Mutex<Record>,
}

impl Ledger {
    /// Opens the record in `data_dir`, or starts an empty one.
    pub fn open(data_dir: &Path) -> Result<Self, LedgerError> {
        let directory = data_dir.join(STATE_DIR);
        std::fs::create_dir_all(&directory).map_err(|source| LedgerError::Io {
            path: directory.clone(),
            source,
        })?;
        let path = directory.join(HOSTED_FILE);
        let record = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| LedgerError::Damaged {
                path: path.clone(),
                message: e.to_string(),
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Record::default(),
            Err(source) => {
                return Err(LedgerError::Io {
                    path: path.clone(),
                    source,
                });
            }
        };
        Ok(Self {
            path,
            record: Mutex::new(record),
        })
    }

    fn record(&self) -> MutexGuard<'_, Record> {
        self.record.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self, record: &Record) {
        let outcome = serde_json::to_vec_pretty(record)
            .map_err(|e| std::io::Error::other(e.to_string()))
            .and_then(|bytes| crate::policy::write_atomically(&self.path, &bytes));
        if let Err(error) = outcome {
            tracing::error!(path = %self.path.display(), %error, "what the connector hosts could not be recorded");
        }
    }

    /// What was hosted when the record was last written.
    #[must_use]
    pub fn hosted(&self) -> Vec<HostedHarness> {
        self.record().harnesses.clone()
    }

    /// Records a harness process that was started, in place of an earlier record of it. Its sessions
    /// stay.
    pub fn harness_started(&self, start: &AgentStart, home: Option<String>) {
        let mut record = self.record();
        let sessions = record
            .harnesses
            .iter()
            .position(|h| h.is(start.provider_id, start.agent_id))
            .map(|at| record.harnesses.remove(at).sessions)
            .unwrap_or_default();
        record.harnesses.push(HostedHarness {
            start: start.clone(),
            home,
            sessions,
        });
        self.write(&record);
    }

    /// Forgets a harness process and its sessions.
    pub fn harness_stopped(&self, provider_id: Id, agent_id: Id) {
        let mut record = self.record();
        record.harnesses.retain(|h| !h.is(provider_id, agent_id));
        self.write(&record);
    }

    /// Records a session that opened in a harness process.
    pub fn session_opened(&self, provider_id: Id, agent_id: Id, session: HostedSession) {
        let mut record = self.record();
        if let Some(harness) = record
            .harnesses
            .iter_mut()
            .find(|h| h.is(provider_id, agent_id))
        {
            harness
                .sessions
                .retain(|open| open.thread_id != session.thread_id);
            harness.sessions.push(session);
        }
        self.write(&record);
    }

    /// Forgets a session that ended.
    pub fn session_closed(&self, thread_id: Id) {
        let mut record = self.record();
        for harness in &mut record.harnesses {
            harness.sessions.retain(|open| open.thread_id != thread_id);
        }
        self.write(&record);
    }

    /// Records which turn of a session runs, or that none does.
    pub fn turn_running(&self, thread_id: Id, turn: Option<Id>) {
        let mut record = self.record();
        let mut changed = false;
        for session in record
            .harnesses
            .iter_mut()
            .flat_map(|harness| harness.sessions.iter_mut())
            .filter(|session| session.thread_id == thread_id)
        {
            if session.running_turn != turn {
                session.running_turn = turn;
                changed = true;
            }
        }
        if changed {
            self.write(&record);
        }
    }

    /// The calls of Orch's tools kept after their answer did not come in time.
    #[must_use]
    pub fn retained_calls(&self) -> Vec<RetainedCall> {
        self.record().retained_calls.clone()
    }

    /// Keeps these calls in place of those kept before.
    pub fn keep_retained_calls(&self, calls: &[RetainedCall]) {
        let mut record = self.record();
        if record.retained_calls != calls {
            record.retained_calls = calls.to_vec();
            self.write(&record);
        }
    }

    /// A fresh call cannot be published until its replay identity is durable.
    /// Failed writes keep the preceding record and never authorize another effect identity.
    pub fn try_keep_retained_calls(&self, calls: &[RetainedCall]) -> Result<(), LedgerError> {
        let mut record = self.record();
        if record.retained_calls == calls {
            return Ok(());
        }
        let mut next = record.clone();
        next.retained_calls = calls.to_vec();
        let bytes = serde_json::to_vec_pretty(&next).map_err(|error| LedgerError::Damaged {
            path: self.path.clone(),
            message: error.to_string(),
        })?;
        crate::policy::write_atomically(&self.path, &bytes).map_err(|source| LedgerError::Io {
            path: self.path.clone(),
            source,
        })?;
        *record = next;
        Ok(())
    }

    /// The profile an agent started with, as the system sent it.
    #[must_use]
    pub fn profile_of(&self, provider_id: Id, agent_id: Id) -> Option<Json> {
        self.record()
            .harnesses
            .iter()
            .find(|h| h.is(provider_id, agent_id))
            .and_then(|h| serde_json::to_value(&h.start.profile).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn start(provider: Id, agent: Id) -> AgentStart {
        AgentStart {
            provider_id: provider,
            agent_id: agent,
            harness_kind: "fake".into(),
            profile: serde_json::from_value(serde_json::json!({ "config": {} })).unwrap(),
            home: Some("harness/p/a".into()),
            provider_config: serde_json::json!({}),
            runtime: None,
        }
    }

    fn session(thread: Id) -> HostedSession {
        HostedSession {
            thread_id: thread,
            segment_id: new_id(),
            native_session_ref: "fake_1".into(),
            running_turn: None,
        }
    }

    #[test]
    fn what_was_hosted_is_there_when_the_connector_starts_again() {
        let dir = tempfile::tempdir().unwrap();
        let (provider, agent, thread, turn) = (new_id(), new_id(), new_id(), new_id());
        {
            let ledger = Ledger::open(dir.path()).unwrap();
            ledger.harness_started(&start(provider, agent), Some("/data/harness/p/a".into()));
            ledger.session_opened(provider, agent, session(thread));
            ledger.turn_running(thread, Some(turn));
        }
        let again = Ledger::open(dir.path()).unwrap();
        let hosted = again.hosted();
        assert_eq!(hosted.len(), 1);
        assert_eq!(hosted[0].home.as_deref(), Some("/data/harness/p/a"));
        assert_eq!(hosted[0].sessions[0].thread_id, thread);
        assert_eq!(hosted[0].sessions[0].running_turn, Some(turn));
    }

    #[test]
    fn a_session_that_ends_and_a_process_that_stops_are_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let (provider, agent) = (new_id(), new_id());
        let (one, two) = (new_id(), new_id());
        ledger.harness_started(&start(provider, agent), None);
        ledger.session_opened(provider, agent, session(one));
        ledger.session_opened(provider, agent, session(two));
        ledger.session_closed(one);
        assert_eq!(ledger.hosted()[0].sessions.len(), 1);
        ledger.harness_stopped(provider, agent);
        assert!(ledger.hosted().is_empty());
        assert!(Ledger::open(dir.path()).unwrap().hosted().is_empty());
    }

    #[test]
    fn starting_a_process_again_keeps_its_sessions_and_takes_the_new_request() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let (provider, agent, thread) = (new_id(), new_id(), new_id());
        ledger.harness_started(&start(provider, agent), None);
        ledger.session_opened(provider, agent, session(thread));
        let mut changed = start(provider, agent);
        changed.profile =
            serde_json::from_value(serde_json::json!({ "config": { "memory": "home" } })).unwrap();
        ledger.harness_started(&changed, None);
        let hosted = ledger.hosted();
        assert_eq!(hosted.len(), 1);
        assert_eq!(hosted[0].sessions.len(), 1);
        assert_eq!(
            ledger.profile_of(provider, agent),
            Some(serde_json::to_value(changed.profile).unwrap())
        );
    }

    #[test]
    fn a_damaged_record_is_reported_and_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(STATE_DIR)).unwrap();
        std::fs::write(dir.path().join(STATE_DIR).join(HOSTED_FILE), "{not json").unwrap();
        let error = Ledger::open(dir.path()).unwrap_err();
        assert!(matches!(error, LedgerError::Damaged { .. }), "{error}");
    }
}
