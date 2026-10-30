//! Capture only the existing job DB boundary's typed, sanitized facts.

use std::{
    fmt,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};
use tracing::{
    Event, Metadata, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};

static INSTALLED: OnceLock<Result<(), ()>> = OnceLock::new();

tokio::task_local! {
    static COUNTING: Arc<AtomicUsize>;
}

/// Runs `work` and tells how many statements it ran: sqlx tells of each one as an event while the task that makes it is
/// polled, and the sink counts those of the task that is counting, whichever thread polls it, so that the work of other
/// tasks that run side by side is not in the count.
pub(super) async fn counted<T>(work: impl std::future::Future<Output = T>) -> (T, usize) {
    let count = Arc::new(AtomicUsize::new(0));
    let done = COUNTING.scope(Arc::clone(&count), work).await;
    (done, count.load(Ordering::Relaxed))
}

struct SafeDatabase {
    next: AtomicU64,
}

impl Subscriber for SafeDatabase {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target() == "sqlx::query"
            || (metadata.target() == "orch_engine::jobs::runner"
                && *metadata.level() == tracing::Level::ERROR)
    }
    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(self.next.fetch_add(1, Ordering::Relaxed))
    }
    fn record(&self, _span: &Id, _values: &Record<'_>) {}
    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}
    fn event(&self, event: &Event<'_>) {
        if event.metadata().target() == "sqlx::query" {
            let _ = COUNTING.try_with(|count| count.fetch_add(1, Ordering::Relaxed));
            return;
        }
        if !self.enabled(event.metadata()) {
            return;
        }
        let mut fields = Fields(Vec::new());
        event.record(&mut fields);
        eprintln!("fork-database {}", fields.0.join(" "));
    }
    fn enter(&self, _span: &Id) {}
    fn exit(&self, _span: &Id) {}
}

struct Fields(Vec<String>);
impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if matches!(
            field.name(),
            "phase" | "category" | "sqlstate" | "constraint" | "column"
        ) {
            self.0.push(format!("{}={value:?}", field.name()));
        }
    }
}

pub(super) fn install() -> Result<(), &'static str> {
    if INSTALLED
        .get_or_init(|| {
            tracing::subscriber::set_global_default(SafeDatabase {
                next: AtomicU64::new(1),
            })
            .map_err(|_| ())
        })
        .is_err()
    {
        return Err("the narrow fork database diagnostic sink could not be installed");
    }
    Ok(())
}
