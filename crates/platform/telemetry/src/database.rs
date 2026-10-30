//! Counts SQLx's slow-statement and slow-acquisition warnings without retaining SQL or bindings.

use tracing::{Event, Metadata, Subscriber};
use tracing_subscriber::{Layer, filter::filter_fn, layer::Context, registry::LookupSpan};

/// The slow-statement counter exported by each database-using service.
pub const SLOW_STATEMENTS: &str = "orch_db_slow_statements_total";
/// The slow successful pool-acquisition counter exported by each database-using service.
pub const SLOW_ACQUIRES: &str = "orch_db_slow_acquires_total";

/// Whether an event may be one of SQLx's threshold warnings: its target and level only. SQLx asks
/// `tracing::enabled!` for the target and level before it writes a slow statement's warning, with a callsite that
/// has no fields; a filter that read the fields would refuse that question, and the refusal, kept for the event that
/// follows it, would hide the warning itself from this layer. The fields are read when the event arrives.
fn sqlx_warning(metadata: &Metadata<'_>) -> bool {
    *metadata.level() == tracing::Level::WARN
        && matches!(metadata.target(), "sqlx::query" | "sqlx::pool::acquire")
}

struct Watch;

impl<S: Subscriber> Layer<S> for Watch {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let fields = event.metadata().fields();
        match event.metadata().target() {
            "sqlx::query" if fields.field("slow_threshold").is_some() => {
                metrics::counter!(SLOW_STATEMENTS).increment(1);
            }
            "sqlx::pool::acquire" if fields.field("slow_acquire_threshold_secs").is_some() => {
                metrics::counter!(SLOW_ACQUIRES).increment(1);
            }
            _ => {}
        }
    }
}

/// Counts only SQLx's threshold warnings, independently of the operator's log-output filter.
/// No statement, parameter, connection string or event field becomes a metric label.
pub fn layer<S>() -> impl Layer<S>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    Watch.with_filter(filter_fn(sqlx_warning))
}

/// Registers zero totals so a collector sees the watch before the first slow operation.
pub fn register() {
    metrics::describe_counter!(
        SLOW_STATEMENTS,
        "Statements that reached the configured slow threshold"
    );
    metrics::describe_counter!(
        SLOW_ACQUIRES,
        "Successful pooled acquisitions exceeding the configured slow threshold"
    );
    metrics::counter!(SLOW_STATEMENTS).increment(0);
    metrics::counter!(SLOW_ACQUIRES).increment(0);
}
