//! Bounded request-local query fingerprints and internal-failure classes; never data or SQL values.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use tracing::field::{Field, Visit};

const FIELD_BYTES: usize = 4096;
const SIGNATURES: usize = 128;
const RECENT: usize = 8;
// Only the unmatched counted statement needs a stack; other SQL events retain cheap fingerprints.
const UNMATCHED_STATEMENT: &str =
    "3e6ec1650bce940f45f6e58c8d0d06c7f7a4644adad52bdffac38c2bbd8441b4";

#[derive(Default)]
struct Text(String);
impl std::fmt::Write for Text {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        let mut count = value.len().min(FIELD_BYTES.saturating_sub(self.0.len()));
        while !value.is_char_boundary(count) {
            count -= 1;
        }
        self.0.push_str(&value[..count]);
        Ok(())
    }
}

#[derive(Default)]
struct Fields {
    statement: Text,
    summary: Text,
    cause: Text,
}
impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "db.statement" | "sql" | "statement" => {
                let _ = self.statement.write_str(value);
            }
            "summary" => {
                let _ = self.summary.write_str(value);
            }
            "cause" => {
                let _ = self.cause.write_str(value);
            }
            _ => {}
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "db.statement" | "sql" | "statement" => {
                let _ = write!(&mut self.statement, "{value:?}");
            }
            "summary" => {
                let _ = write!(&mut self.summary, "{value:?}");
            }
            "cause" => {
                let _ = write!(&mut self.cause, "{value:?}");
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct Observed {
    count: usize,
    signatures: BTreeMap<String, usize>,
    omitted: usize,
    recent: VecDeque<String>,
    unmatched_callers: Option<Vec<String>>,
    unmatched_sqlx_callers: Option<Vec<String>>,
}
struct Recorder {
    label: &'static str,
    observed: Arc<Mutex<Observed>>,
}

fn project_callers() -> Vec<String> {
    // Function symbols only: no absolute paths, arguments, SQL, errors or account data.
    std::backtrace::Backtrace::force_capture()
        .to_string()
        .lines()
        .filter(|line| {
            [
                "orch_api_core::",
                "orch_db::",
                "orch_authz::",
                "orch_settings::",
            ]
            .iter()
            .any(|name| line.contains(*name))
        })
        .take(12)
        .map(|line| {
            line.trim()
                .split_once(": ")
                .map_or(line.trim(), |(_, symbol)| symbol)
                .chars()
                .take(256)
                .collect()
        })
        .collect()
}

fn sqlx_callers() -> Vec<String> {
    // A first prepared/type-discovery query must be distinguishable from an application query.
    std::backtrace::Backtrace::force_capture()
        .to_string()
        .lines()
        .filter(|line| line.contains("sqlx_core::") || line.contains("sqlx_postgres::"))
        .take(8)
        .map(|line| {
            line.trim()
                .split_once(": ")
                .map_or(line.trim(), |(_, symbol)| symbol)
                .chars()
                .take(256)
                .collect()
        })
        .collect()
}

fn hints(text: &str) -> Vec<&'static str> {
    let text = text.to_ascii_lowercase();
    [
        "select",
        "insert",
        "update",
        "delete",
        "begin",
        "commit",
        "rollback",
        "decode",
        "null",
        "not found",
        "does not exist",
        "syntax",
        "permission",
        "unknown",
        "invalid",
        "boolean",
        "uuid",
        "json",
        "timestamp",
        "nodes",
        "topics",
        "users",
        "documents",
        "containers",
        "owners",
        "holder",
        "settings",
        "authz",
        "epoch",
        "scope",
        "cache",
        "transaction",
        "column",
        "relation",
        "variant",
        "integer",
        "text",
        "row",
        "owner_id",
        "node_id",
        "created_at",
        "updated_at",
        "name",
        "kind",
        "role",
        "limit",
        "cursor",
    ]
    .into_iter()
    .filter(|word| text.contains(*word))
    .collect()
}

fn column(text: &str) -> Option<String> {
    let text = text.replace("\\\"", "\"");
    let (_, suffix) = text.split_once("column ")?;
    let name = suffix.strip_prefix('"')?.split('"').next()?;
    // A schema identifier, never an error's value, row, SQL, user name or free-form sentence.
    (!name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
    .then(|| name.to_owned())
}

impl tracing::Subscriber for Recorder {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "sqlx::query" || *metadata.level() == tracing::Level::ERROR
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        if event.metadata().target() == "sqlx::query" {
            // Preserve the existing thread counter; this subscriber adds evidence, not a new cost metric.
            tracing::Subscriber::event(&crate::support::Statements, event);
            let fingerprint = orch_types::Hash::of(fields.statement.0.as_bytes()).to_string();
            let unmatched_callers = (fingerprint == UNMATCHED_STATEMENT).then(project_callers);
            let unmatched_sqlx_callers = (fingerprint == UNMATCHED_STATEMENT).then(sqlx_callers);
            let signature = serde_json::json!({"prefix_blake3": fingerprint,
                "summary_prefix_blake3": orch_types::Hash::of(fields.summary.0.as_bytes()).to_string(),
                "hints": hints(if fields.statement.0.is_empty() { &fields.summary.0 } else { &fields.statement.0 })}).to_string();
            let mut observed = self.observed.lock().unwrap_or_else(PoisonError::into_inner);
            observed.count += 1;
            if unmatched_callers.is_some() && observed.unmatched_callers.is_none() {
                observed.unmatched_callers = unmatched_callers;
            }
            if unmatched_sqlx_callers.is_some() && observed.unmatched_sqlx_callers.is_none() {
                observed.unmatched_sqlx_callers = unmatched_sqlx_callers;
            }
            if observed.signatures.contains_key(&signature)
                || observed.signatures.len() < SIGNATURES
            {
                *observed.signatures.entry(signature.clone()).or_default() += 1;
            } else {
                observed.omitted += 1;
            }
            if observed.recent.len() == RECENT {
                observed.recent.pop_front();
            }
            observed.recent.push_back(signature);
        } else if !fields.cause.0.is_empty() {
            let recent = self
                .observed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .recent
                .clone();
            eprintln!(
                "reader_failure {}",
                serde_json::json!({"phase":self.label,"target":event.metadata().target(),
                "cause_prefix_blake3":orch_types::Hash::of(fields.cause.0.as_bytes()).to_string(),
                "hints":hints(&fields.cause.0),"column":column(&fields.cause.0),"recent_sql":recent,"callers":project_callers()})
            );
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Applies the diagnostic subscriber at each poll, without replacing a binary's global counter.
pub async fn trace<T>(
    label: &'static str,
    future: impl Future<Output = T>,
) -> (T, serde_json::Value) {
    let observed = Arc::new(Mutex::new(Observed::default()));
    let dispatch = tracing::Dispatch::new(Recorder {
        label,
        observed: Arc::clone(&observed),
    });
    let mut future = std::pin::pin!(future);
    let answer = std::future::poll_fn(|cx| {
        tracing::dispatcher::with_default(&dispatch, || future.as_mut().poll(cx))
    })
    .await;
    let observed = observed.lock().unwrap_or_else(PoisonError::into_inner);
    let evidence = serde_json::json!({"statements":observed.count,"signatures":observed.signatures,"omitted":observed.omitted,
        "unmatched_callers":observed.unmatched_callers,"unmatched_sqlx_callers":observed.unmatched_sqlx_callers});
    (answer, evidence)
}
