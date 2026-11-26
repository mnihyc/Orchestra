//! Pause one real file writer at its existing structured diagnostic, without replacing its renderer.

use orch_types::Id;
use std::collections::HashMap;
use std::sync::{Mutex, Once, OnceLock, mpsc};
use tokio::sync::oneshot;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};

struct Gate {
    entered: oneshot::Sender<()>,
    release: mpsc::Receiver<()>,
}
static GATES: OnceLock<Mutex<HashMap<Id, Gate>>> = OnceLock::new();
static INSTALLED: Once = Once::new();

struct Writer;
struct Subject(Option<Id>);
impl Visit for Subject {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "thread" {
            self.0 = format!("{value:?}").trim_matches('"').parse().ok();
        }
    }
}
impl<S: Subscriber> Layer<S> for Writer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        if event.metadata().target() != "orch_engine::recovery::history_write" {
            return;
        }
        let mut subject = Subject(None);
        event.record(&mut subject);
        let Some(thread) = subject.0 else {
            return;
        };
        let gate = GATES
            .get()
            .and_then(|gates| gates.lock().unwrap().remove(&thread));
        if let Some(gate) = gate {
            let _ = gate.entered.send(());
            // Only this writer waits. Dropping the case's sender also releases it after a failed assertion.
            let _ = gate.release.recv();
        }
    }
}

pub fn pause(thread: Id) -> (oneshot::Receiver<()>, mpsc::Sender<()>) {
    INSTALLED.call_once(|| {
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(Writer))
            .expect("the history writer observer is installed once for this test program");
    });
    let (entered, waiting) = oneshot::channel();
    let (release, resume) = mpsc::channel();
    assert!(
        GATES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap()
            .insert(
                thread,
                Gate {
                    entered,
                    release: resume
                }
            )
            .is_none()
    );
    (waiting, release)
}
