//! Nested plans after a measured access statement failed; the original performance bar stays in force.

use crate::support::Statements;
use orch_authz::{Caller, Principal};
use orch_db::Conn;
use orch_types::{Id, Json, Time};
use sqlx::{Connection as _, Row as _};
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

async fn begin(conn: &mut Conn, bar_ms: f64) -> bool {
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
        return false;
    }
    sqlx::query("SELECT set_config('statement_timeout',$1,false)")
        .bind(format!("{}ms", bar_ms * 10.0))
        .execute(&mut *conn)
        .await
        .unwrap();
    true
}

async fn finish(conn: &mut Conn) {
    sqlx::query("SET auto_explain.log_min_duration=-1")
        .execute(conn)
        .await
        .unwrap();
}

async fn holder_estimate(conn: &mut Conn) {
    let source: String = sqlx::query_scalar(
        "SELECT prosrc FROM pg_proc
          WHERE oid='public.access_holders(uuid,timestamptz,boolean)'::regprocedure",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let Some(body) = source
        .trim()
        .rsplit_once("RETURN QUERY")
        .map(|(_, walk)| walk)
        .and_then(|sql| sql.trim().strip_suffix("END"))
        .and_then(|sql| sql.trim().strip_suffix(';'))
    else {
        eprintln!("large hierarchy generic holder plan needs the stored recursive return query");
        return;
    };
    // Inspect the installed function's actual statement without executing it or binding
    // fixture values. A second timed-out execution otherwise leaves no nested plan.
    let explain = sqlx::AssertSqlSafe(format!(
        "EXPLAIN (GENERIC_PLAN,FORMAT JSON)
         SELECT walked.* FROM (SELECT $1::uuid AS person,$2::timestamptz AS sampled_at,
                                      $3::boolean AS include_admin,$4::uuid[] AS blocked_orgs,
                                      $5::boolean AS has_deleted,$6::boolean AS check_scopes,
                                      $7::boolean AS has_creation,$8::boolean AS has_named,
                                      $9::boolean AS has_links,$10::boolean AS has_local_entries,
                                      $11::boolean AS has_local_cuts) inputs
         CROSS JOIN LATERAL ({body}) walked"
    ));
    let mut tx = conn.begin().await.unwrap();
    let result: Result<Json, sqlx::Error> = async {
        sqlx::raw_sql(
            "SET TRANSACTION READ ONLY;
             SET LOCAL jit=off;
             SET LOCAL join_collapse_limit=1;
             SET LOCAL from_collapse_limit=1;",
        )
        .execute(&mut *tx)
        .await?;
        let plans = sqlx::raw_sql(explain).fetch_all(&mut *tx).await?;
        plans[0].try_get(0)
    }
    .await;
    tx.rollback().await.unwrap();
    match result {
        Ok(plan) => eprintln!("large hierarchy generic holder plan {plan}"),
        Err(error) => eprintln!("large hierarchy generic holder plan unavailable: {error}"),
    }
}

pub async fn principal_plan(conn: &mut Conn, user: Id, at: Time, bar_ms: f64) {
    holder_estimate(conn).await;
    if !begin(conn, bar_ms).await {
        return;
    }
    let result = Principal::load(conn, Caller::session(user), at)
        .with_subscriber(tracing::Dispatch::new(Plans))
        .await;
    eprintln!(
        "large hierarchy nested plan diagnostic completed={}",
        result.is_ok()
    );
    finish(conn).await;
}

pub async fn projection_plan(conn: &mut Conn, principal: &Principal, bar_ms: f64) {
    if !begin(conn, bar_ms).await {
        return;
    }
    let result = orch_authz::everything::project(conn, principal, None)
        .with_subscriber(tracing::Dispatch::new(Plans))
        .await;
    eprintln!(
        "large hierarchy projection nested diagnostic completed={}",
        result.is_ok()
    );
    finish(conn).await;
}
