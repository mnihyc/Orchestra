//! Bounded metadata witnesses for the real restarted terminal-work proof.

use crate::{realtime_support::Rig, thread_history_support};
use futures::FutureExt;
use orch_agent_contract::descriptor::AgentDescriptor;
use orch_types::Id;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};
use tracing::field::{Field, Visit};

type Store = Arc<Mutex<BTreeMap<Id, Observed>>>;
const TARGET: &str = "orch_engine::terminal_work";
// Match the Rig's existing metadata-only failure probe, never its success budget.
const EVIDENCE_WAIT: Duration = Duration::from_secs(2);

struct Phase {
    count: u64,
    last_ms: u128,
    class: &'static str,
    sqlstate: Option<String>,
}

struct Observed {
    started: Instant,
    // Keys are finite workflow/outcome enums, not unbounded event payloads.
    phases: BTreeMap<(&'static str, &'static str, bool), Phase>,
}

#[derive(Default)]
struct Metadata {
    thread: Option<Id>,
    phase: Option<&'static str>,
    outcome: Option<&'static str>,
    class: Option<&'static str>,
    sqlstate: Option<String>,
    stopped: bool,
}

fn phase(value: &str) -> Option<&'static str> {
    Some(match value {
        "startup" => "startup",
        "dispatch" => "dispatch",
        "repair_current" => "repair_current",
        "record_failure" => "record_failure",
        "begin" => "begin",
        "lock_thread" => "lock_thread",
        "load_work" => "load_work",
        "decode_work" => "decode_work",
        "load_turn" => "load_turn",
        "settle_requests" => "settle_requests",
        "account_usage" => "account_usage",
        "notifications" => "notifications",
        "context" => "context",
        "summary_due" => "summary_due",
        "retire_work" => "retire_work",
        "clear_reason" => "clear_reason",
        "unit_finish" => "unit_finish",
        "commit" => "commit",
        _ => return None,
    })
}

fn class(value: &str) -> Option<&'static str> {
    Some(match value {
        "none" => "none",
        "db_unique" => "db_unique",
        "db_reference" => "db_reference",
        "db_check" => "db_check",
        "db_rule" => "db_rule",
        "db_retry" => "db_retry",
        "db_database" => "db_database",
        "pool_timeout" => "pool_timeout",
        "row_missing" => "row_missing",
        "db_decode" => "db_decode",
        "db_other" => "db_other",
        "authz" => "authz",
        "access" => "access",
        "settings" => "settings",
        "commands" => "commands",
        "unreadable" => "unreadable",
        _ => return None,
    })
}

impl Visit for Metadata {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "thread" => self.thread = value.parse().ok(),
            "phase" => self.phase = phase(value),
            "outcome" => {
                self.outcome = match value {
                    "enter" => Some("enter"),
                    "ok" => Some("ok"),
                    "error" => Some("error"),
                    _ => None,
                }
            }
            "error_class" => self.class = class(value),
            "sqlstate"
                if value.len() == 5 && value.bytes().all(|byte| byte.is_ascii_alphanumeric()) =>
            {
                self.sqlstate = Some(value.into())
            }
            _ => {}
        }
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "stopped" {
            self.stopped = value;
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // Display-formatted native fixture UUID only. Never format an error or SQL field.
        if field.name() == "thread" {
            self.thread = format!("{value:?}").parse().ok();
        }
    }
}

struct Recorder(Store);
impl tracing::Subscriber for Recorder {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == TARGET
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() != TARGET {
            return;
        }
        let mut metadata = Metadata::default();
        event.record(&mut metadata);
        let (Some(thread), Some(phase), Some(outcome)) =
            (metadata.thread, metadata.phase, metadata.outcome)
        else {
            return;
        };
        let mut store = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(observed) = store.get_mut(&thread) else {
            return;
        };
        let key = (phase, outcome, metadata.stopped);
        let value = observed.phases.entry(key).or_insert(Phase {
            count: 0,
            last_ms: 0,
            class: "none",
            sqlstate: None,
        });
        value.count = value.count.saturating_add(1);
        value.last_ms = observed.started.elapsed().as_millis();
        value.class = metadata.class.unwrap_or("none");
        value.sqlstate = metadata.sqlstate;
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

pub struct Watch {
    thread: Id,
    store: Store,
}
impl Watch {
    pub fn new(thread: Id) -> thread_history_support::Proof<Self> {
        static STORE: OnceLock<Store> = OnceLock::new();
        static INSTALL: OnceLock<Result<(), ()>> = OnceLock::new();
        let store = STORE
            .get_or_init(|| Arc::new(Mutex::new(BTreeMap::new())))
            .clone();
        if INSTALL
            .get_or_init(|| {
                tracing::subscriber::set_global_default(Recorder(store.clone())).map_err(|_| ())
            })
            .is_err()
        {
            return Err("the terminal-work diagnostic subscriber could not be installed".into());
        }
        if !tracing::enabled!(target:TARGET,tracing::Level::DEBUG) {
            return Err("the bounded terminal-work diagnostic target must be enabled".into());
        }
        store.lock().unwrap_or_else(PoisonError::into_inner).insert(
            thread,
            Observed {
                started: Instant::now(),
                phases: BTreeMap::new(),
            },
        );
        Ok(Self { thread, store })
    }

    fn phases(&self, label: &str) {
        let phases = {
            let store = self.store.lock().unwrap_or_else(PoisonError::into_inner);
            store.get(&self.thread).map(|observed| {
                observed
                    .phases
                    .iter()
                    .map(|(key, value)| {
                        (
                            *key,
                            value.count,
                            value.last_ms,
                            value.class,
                            value.sqlstate.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
        };
        if let Some(phases) = phases {
            eprintln!("terminal-work phase={label} observations={phases:?}");
        }
    }

    pub async fn checkpoint(&self, rig: &Rig, turn: Id, frozen: &Value, label: &str) {
        let read=tokio::time::timeout(EVIDENCE_WAIT,sqlx::query_as::<_,(Value,Value,Value)>(
            "SELECT jsonb_build_object('state',t.state,'error_block',t.blocked_reason='error',
                'current_turn_present',t.current_turn_id IS NOT NULL,'turn_state',u.state,
                'native_end',u.native_end_confirmed,'lease_present',u.lease_id IS NOT NULL,
                'end_work',u.end_work IS NOT NULL,'attempted_at',u.end_work->'attempted_at',
                'same_snapshot',u.end_work->'snapshot'=$3::jsonb,
                'override_empty',a.descriptor_overrides='{}'::jsonb,
                'counted_receipts',(SELECT count(*) FROM usage_events e WHERE e.turn_id=u.id AND e.counted),
                'files_end_due',u.files_end_due,'files_work_pending',u.files_work_pending,
                'native_session_present',s.native_session_ref IS NOT NULL),a.descriptor,a.descriptor_overrides
             FROM threads t JOIN turns u ON u.thread_id=t.node_id AND u.id=$2
             JOIN segments s ON s.id=u.segment_id JOIN agents a ON a.id=s.agent_id WHERE t.node_id=$1")
            .bind(self.thread).bind(turn).bind(frozen).fetch_one(&rig.db.pool)).await;
        match read {
            Ok(Ok((mut metadata, descriptor, overrides))) => {
                let valid = serde_json::from_value::<AgentDescriptor>(descriptor)
                    .ok()
                    .is_some_and(|descriptor| descriptor.with_overrides(&overrides).is_ok());
                metadata["effective_descriptor_valid"] = json!(valid);
                eprintln!("terminal-work phase={label} metadata={metadata}");
            }
            Ok(Err(_)) => eprintln!("terminal-work phase={label} metadata=read_failed"),
            Err(_) => eprintln!("terminal-work phase={label} metadata=read_timeout"),
        }
        self.phases(label);
    }

    pub async fn accounted(&self, rig: &Rig, turn: Id, frozen: &Value) {
        // Preserve the original exact condition, 25-second deadline and panic.
        // Observation after failure cannot turn that failure into success.
        let result = AssertUnwindSafe(thread_history_support::accounted(rig, self.thread))
            .catch_unwind()
            .await;
        self.checkpoint(rig, turn, frozen, "after_accounted").await;
        if let Err(panic) = result {
            let waits=tokio::time::timeout(EVIDENCE_WAIT,sqlx::query_scalar::<_,Value>(
                "SELECT COALESCE(jsonb_agg(jsonb_build_object('pid',pid,'state',state,'wait_type',wait_event_type,
                    'wait_event',wait_event,'blockers',pg_blocking_pids(pid),
                    'transaction_ms',floor(extract(epoch FROM (clock_timestamp()-xact_start))*1000))), '[]'::jsonb)
                 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid()
                   AND backend_type='client backend'
                   AND (wait_event_type='Lock' OR cardinality(pg_blocking_pids(pid))>0 OR xact_start IS NOT NULL)")
                .fetch_one(&rig.db.pool)).await;
            match waits {
                Ok(Ok(metadata)) => eprintln!("terminal-work waits={metadata}"),
                _ => eprintln!("terminal-work waits=unavailable"),
            }
            std::panic::resume_unwind(panic);
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.phases("watch_end");
        self.store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.thread);
    }
}
