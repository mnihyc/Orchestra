//! Nested plans after a measured access statement failed; the original performance bar stays in force.

use crate::support::Statements;
use orch_authz::{Caller, Principal};
use orch_db::Conn;
use orch_types::{Id, Time};
use tracing::instrument::WithSubscriber as _;

struct Plans;
impl tracing::Subscriber for Plans {
    fn enabled(&self, meta: &tracing::Metadata<'_>) -> bool {
        meta.target() == "sqlx::query" || meta.target().starts_with("sqlx::postgres")
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        tracing::Subscriber::event(&Statements, event);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

pub async fn principal_plan(conn: &mut Conn, user: Id, at: Time, bar_ms: f64) {
    // Ten measured intervals let the failed read finish its diagnostic without accepting it
    // as passing. Disable parameter values before enabling nested plans on this fixture session.
    let setup = sqlx::raw_sql(
        "LOAD 'auto_explain';
        SET auto_explain.log_parameter_max_length=0;
        SET auto_explain.log_analyze=on;
        SET auto_explain.log_buffers=on;
        SET auto_explain.log_nested_statements=on;
        SET auto_explain.log_timing=off;
        SET auto_explain.log_format='json';
        SET auto_explain.log_level='notice';
        SET auto_explain.log_min_duration=0;",
    )
    .execute(&mut *conn)
    .await;
    if let Err(error) = setup {
        eprintln!("large hierarchy nested plan capture unavailable: {error}");
        return;
    }
    sqlx::query("SELECT set_config('statement_timeout',$1,false)")
        .bind(format!("{}ms", bar_ms * 10.0))
        .execute(&mut *conn)
        .await
        .unwrap();
    let result = Principal::load(conn, Caller::session(user), at)
        .with_subscriber(tracing::Dispatch::new(Plans))
        .await;
    eprintln!(
        "large hierarchy nested plan diagnostic completed={}",
        result.is_ok()
    );
    sqlx::query("SET auto_explain.log_min_duration=-1")
        .execute(conn)
        .await
        .unwrap();
}
