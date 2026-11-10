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
const KEPT_ENTRIES: usize = 64;

const ENTRY_CHARS: usize = 512;

type Entries = VecDeque<(Duration, String)>;

#[derive(Default)]
struct State {
    phases: Entries,
    events: Entries,
}

/// A request-local recorder can follow a future without changing other test tasks.
#[derive(Clone)]
pub struct Recorder {
    started: Instant,
    state: Arc<Mutex<State>>,
}

impl Recorder {
    pub fn mark(&self, message: impl AsRef<str>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        keep(&mut state.phases, self.started.elapsed(), message.as_ref());
    }
}

fn keep(entries: &mut Entries, elapsed: Duration, message: &str) {
    if entries.len() == KEPT_ENTRIES {
        entries.pop_front();
    }
    entries.push_back((elapsed, message.chars().take(ENTRY_CHARS).collect()));
}

impl tracing::Subscriber for Recorder {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        matches!(
            metadata.target(),
            "sqlx::query" | "orch_api::native_operations"
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
        struct Fields(Vec<String>);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
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
                ) {
                    self.0.push(format!("{}={value:?}", field.name()));
                }
            }
        }
        let mut fields = Fields(Vec::new());
        event.record(&mut fields);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        keep(
            &mut state.events,
            self.started.elapsed(),
            &format!("{} {}", event.metadata().target(), fields.0.join(" ")),
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
        Self {
            name,
            budget,
            recorder: Recorder {
                started: Instant::now(),
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
        for (elapsed, phase) in &state.phases {
            eprintln!("  phase +{elapsed:?}: {phase}");
        }
        for (elapsed, event) in &state.events {
            eprintln!("  event +{elapsed:?}: {event}");
        }
    }
}
