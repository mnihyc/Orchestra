//! Capture only the continuation boundary's sanitized internal phase facts in native fixture output.

use std::{
    fmt,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};
use tracing::{
    Event, Metadata, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;
static INSTALLED: OnceLock<Result<(), ()>> = OnceLock::new();

struct SafePhases {
    next: AtomicU64,
}

impl Subscriber for SafePhases {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target() == "orch_api_core::routes::threads::continuations"
            && *metadata.level() == tracing::Level::ERROR
    }
    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(self.next.fetch_add(1, Ordering::Relaxed))
    }
    fn record(&self, _span: &Id, _values: &Record<'_>) {}
    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}
    fn event(&self, event: &Event<'_>) {
        if !self.enabled(event.metadata()) {
            return;
        }
        let mut fields = Fields(Vec::new());
        event.record(&mut fields);
        eprintln!("continuation-internal {}", fields.0.join(" "));
    }
    fn enter(&self, _span: &Id) {}
    fn exit(&self, _span: &Id) {}
}

struct Fields(Vec<String>);
impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if matches!(
            field.name(),
            "phase" | "category" | "sqlstate" | "constraint"
        ) {
            self.0.push(format!("{}={value:?}", field.name()));
        }
    }
}

/// A missing diagnostic sink is explicit; no broad error/SQL/body subscriber is substituted.
pub fn install() -> Proof {
    if INSTALLED
        .get_or_init(|| {
            tracing::subscriber::set_global_default(SafePhases {
                next: AtomicU64::new(1),
            })
            .map_err(|_| ())
        })
        .is_err()
    {
        return Err("the bounded continuation diagnostic sink could not be installed".into());
    }
    Ok(())
}
