//! Inspect actual slow native statements, including SQL-function bodies, on isolated fixture pools.

use orch_db::Db;
use serde_json::Value;
use std::sync::{Arc, Mutex, PoisonError};
use tracing::instrument::WithSubscriber as _;

/// The per-statement database goal, independent of the HTTP/pool timing bar.
pub const BAR_MS: f64 = 20.0;

#[derive(Clone)]
pub struct Plan {
    pub duration_ms: f64,
    pub body: Value,
}

fn parse_notice(message: &str) -> Option<Plan> {
    let (_, message) = message.split_once("duration: ")?;
    let (duration, message) = message.split_once(" ms")?;
    let (_, body) = message.split_once("plan:")?;
    Some(Plan {
        duration_ms: duration.trim().parse().ok()?,
        body: serde_json::from_str(body.trim()).ok()?,
    })
}

#[derive(Clone, Default)]
pub struct Capture(Arc<Mutex<Vec<Plan>>>);

impl Capture {
    pub fn take(&self) -> Vec<Plan> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub async fn run<F: std::future::Future>(&self, future: F) -> F::Output {
        future
            .with_subscriber(tracing::Dispatch::new(self.clone()))
            .await
    }

    /// Configure every serving connection before the native request chooses one. This pool
    /// belongs only to this fresh fixture; no application configuration or global logger changes.
    pub async fn configure(db: &Db, enabled: bool) {
        let mut connections = Vec::new();
        for _ in 0..db.pool().options().get_max_connections() {
            connections.push(db.pool().acquire().await.unwrap());
        }
        for conn in &mut connections {
            if enabled {
                sqlx::raw_sql(
                    "LOAD 'auto_explain';
                    SET auto_explain.log_parameter_max_length=0;
                    SET auto_explain.log_analyze=on;
                    SET auto_explain.log_buffers=on;
                    SET auto_explain.log_nested_statements=on;
                    SET auto_explain.log_timing=off;
                    SET auto_explain.log_format='json';
                    SET auto_explain.log_level='notice';
                    SET client_min_messages='notice';",
                )
                .execute(&mut **conn)
                .await
                .unwrap();
            }
            sqlx::query("SELECT set_config('auto_explain.log_min_duration',$1,false)")
                .bind(if enabled {
                    format!("{BAR_MS}ms")
                } else {
                    "-1".to_owned()
                })
                .execute(&mut **conn)
                .await
                .unwrap();
        }
    }

    /// A separately discarded probe proves PostgreSQL notices reach this subscriber. An empty
    /// measured result can then mean every real statement stayed below the goal.
    pub async fn verify(&self, db: &Db) {
        let mut conn = db.acquire().await.unwrap();
        self.run(
            sqlx::query("SELECT pg_sleep($1)")
                .bind(2.0 * BAR_MS / 1_000.0)
                .execute(&mut *conn),
        )
        .await
        .unwrap();
        assert!(
            self.take().iter().any(|plan| {
                plan.body["Query Text"]
                    .as_str()
                    .is_some_and(|sql| sql.contains("pg_sleep"))
                    && plan.duration_ms >= BAR_MS
            }),
            "the isolated PostgreSQL plan probe must be observed before judging real requests"
        );
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, meta: &tracing::Metadata<'_>) -> bool {
        meta.target().starts_with("sqlx::postgres")
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        struct Notice(Vec<Plan>);
        impl tracing::field::Visit for Notice {
            fn record_str(&mut self, _: &tracing::field::Field, value: &str) {
                if let Some(plan) = parse_notice(value) {
                    self.0.push(plan);
                }
            }
            fn record_debug(&mut self, _: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                let text = format!("{value:?}");
                let decoded = serde_json::from_str::<String>(&text).unwrap_or(text);
                if let Some(plan) = parse_notice(&decoded) {
                    self.0.push(plan);
                }
            }
        }
        let mut notice = Notice(Vec::new());
        event.record(&mut notice);
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(notice.0);
    }
}
