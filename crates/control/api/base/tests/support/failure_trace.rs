//! Bounded fixture diagnostics, emitted only when a checked operation fails.
#![allow(dead_code, reason = "each fixture uses part of the shared diagnostics")]

use std::collections::VecDeque;

use std::sync::Arc;

use std::sync::Mutex;

use std::sync::PoisonError;

use std::time::Duration;

use std::time::Instant;
// A tail of 64 short entries keeps both useful boundaries and recent query
// completions without retaining an unbounded transcript or statement bindings.
// A read that runs more statements than that asks for a longer tail of its own.
const KEPT_ENTRIES: usize = 64;

const ENTRY_CHARS: usize = 512;

type Entries = VecDeque<(Duration, String)>;

#[derive(Default)]
struct Timing {
    count: u64,
    seconds: f64,
    first_start: Option<Duration>,
    last_finish: Option<Duration>,
}

impl Timing {
    fn record(&mut self, at: Duration, seconds: f64) {
        self.count += 1;
        self.seconds += seconds;
        self.first_start
            .get_or_insert(at.saturating_sub(Duration::from_secs_f64(seconds)));
        self.last_finish = Some(at);
    }
}

#[derive(Default)]
struct State {
    phases: Entries,
    events: Entries,
    sql: Timing,
    pool: Timing,
    // Every native boundary the followed request has entered, as (operation, phase), kept whole: the tail of events
    // above forgets the early ones when a request runs many statements.
    entered: Vec<(String, String)>,
    // How many events of each target the followed request produced, whatever the tail kept: a target that is missing
    // here was never heard, which is not the same as a request that did nothing.
    heard: std::collections::BTreeMap<String, u64>,
}

/// A request-local recorder can follow a future without changing other test tasks.
#[derive(Clone)]
pub struct Recorder {
    started: Instant,
    tail: usize,
    state: Arc<Mutex<State>>,
}

impl Recorder {
    pub fn mark(&self, message: impl AsRef<str>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        keep(
            &mut state.phases,
            self.started.elapsed(),
            message.as_ref(),
            self.tail,
        );
    }

    /// Every native boundary the followed request has entered so far, as (operation, phase), in order.
    pub fn boundaries(&self) -> Vec<(String, String)> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entered
            .clone()
    }

    /// How many events of each target the followed request has produced, on one line.
    pub fn heard(&self) -> String {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.heard.is_empty() {
            return String::from("no event of the followed request was heard");
        }
        state
            .heard
            .iter()
            .map(|(target, count)| format!("{target}={count}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Whether the followed request has entered `phase` of `operation`'s native boundary.
    pub fn entered(&self, operation: &str, phase: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entered
            .iter()
            .any(|(entered, at)| entered == operation && at == phase)
    }
}

fn keep(entries: &mut Entries, elapsed: Duration, message: &str, tail: usize) {
    if entries.len() >= tail {
        entries.pop_front();
    }
    entries.push_back((elapsed, message.chars().take(ENTRY_CHARS).collect()));
}

impl tracing::Subscriber for Recorder {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        matches!(
            metadata.target(),
            "sqlx::query" | "orch_api::native_operations" | "orch_db::pool"
        ) || metadata.target().contains("::realtime")
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if !self.enabled(event.metadata()) {
            return;
        }
        #[derive(Default)]
        struct Fields {
            values: Vec<String>,
            elapsed_seconds: Option<f64>,
            message: Option<String>,
            operation: Option<String>,
            phase: Option<String>,
            statement: Option<String>,
        }
        impl tracing::field::Visit for Fields {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                match field.name() {
                    "operation" => self.operation = Some(value.to_owned()),
                    "phase" => self.phase = Some(value.to_owned()),
                    "db.statement" => {
                        self.statement =
                            Some(value.split_whitespace().collect::<Vec<_>>().join(" "));
                        return;
                    }
                    _ => {}
                }
                self.record_debug(field, &value);
            }

            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                match field.name() {
                    "message" => self.message = Some(format!("{value:?}")),
                    // The whole statement, on one line and last, so that a long one never hides the timings before it.
                    "db.statement" => {
                        self.statement = Some(
                            format!("{value:?}")
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" "),
                        );
                        return;
                    }
                    _ => {}
                }
                if matches!(
                    field.name(),
                    "message"
                        | "summary"
                        | "elapsed"
                        | "elapsed_secs"
                        | "operation"
                        | "phase"
                        | "disclosure"
                        | "code"
                        | "status"
                        | "error"
                        | "reason"
                        | "pool_size"
                        | "pool_idle"
                ) {
                    self.values.push(format!("{}={value:?}", field.name()));
                }
            }

            fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
                if field.name() == "elapsed_secs" && value.is_finite() && value >= 0.0 {
                    self.elapsed_seconds = Some(value);
                }
                self.record_debug(field, &value);
            }
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let at = self.started.elapsed();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        *state
            .heard
            .entry(event.metadata().target().to_owned())
            .or_default() += 1;
        if event.metadata().target() == "orch_api::native_operations"
            && fields.message.as_deref() == Some("native boundary entered")
            && let (Some(operation), Some(phase)) = (fields.operation.take(), fields.phase.take())
        {
            state.entered.push((operation, phase));
        }
        if let Some(seconds) = fields.elapsed_seconds {
            match event.metadata().target() {
                "sqlx::query" => state.sql.record(at, seconds),
                "orch_db::pool" => state.pool.record(at, seconds),
                _ => {}
            }
        }
        let statement = fields
            .statement
            .as_deref()
            .map_or_else(String::new, |text| format!(" statement={text}"));
        keep(
            &mut state.events,
            at,
            &format!(
                "{} {}{statement}",
                event.metadata().target(),
                fields.values.join(" ")
            ),
            self.tail,
        );
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Explicit success suppresses output. Panic, timeout and early Result errors
/// retain the phases reached and the same original observation budget.
pub struct FailureTrace {
    name: &'static str,
    budget: Duration,
    recorder: Recorder,
    finished: bool,
}

impl FailureTrace {
    pub fn new(name: &'static str, budget: Duration) -> Self {
        Self::with_tail(name, budget, KEPT_ENTRIES)
    }
    /// A trace that keeps the last `tail` phases and events instead of the usual few.
    pub fn with_tail(name: &'static str, budget: Duration, tail: usize) -> Self {
        Self {
            name,
            budget,
            recorder: Recorder {
                started: Instant::now(),
                tail,
                state: Arc::default(),
            },
            finished: false,
        }
    }
    pub fn recorder(&self) -> Recorder {
        self.recorder.clone()
    }
    pub fn mark(&self, message: impl AsRef<str>) {
        self.recorder.mark(message);
    }
    pub fn finish(&mut self) {
        self.finished = true;
    }
}

impl Drop for FailureTrace {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let state = self
            .recorder
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        eprintln!(
            "{} failed: elapsed={:?} original_budget={:?}",
            self.name,
            self.recorder.started.elapsed(),
            self.budget
        );
        for (name, timing) in [("sql", &state.sql), ("pool", &state.pool)] {
            if timing.count > 0 {
                eprintln!(
                    "  {name} count={} elapsed={:.6}s first_start={:?} last_finish={:?}",
                    timing.count, timing.seconds, timing.first_start, timing.last_finish
                );
            }
        }
        for (elapsed, phase) in &state.phases {
            eprintln!("  phase +{elapsed:?}: {phase}");
        }
        for (elapsed, event) in &state.events {
            eprintln!("  event +{elapsed:?}: {event}");
        }
    }
}
