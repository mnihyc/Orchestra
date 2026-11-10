//! Stored provider reads cannot wait for a unit, a host or the release payload catalogue.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod places;
mod rig;
mod support;

use orch_api_contract::resources::{
    ProviderGet, ProviderList, ProviderListQuery, ProviderListScope,
};
use orch_engine::hosting::{Enrollment, EnrollmentAsk, HostServices, HostingError, ToolBuild};
use orch_link_contract::commands::CommandKind;
use orch_types::new_id;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use support::failure_trace::FailureTrace;
use tracing::instrument::WithSubscriber as _;

async fn timed_read<T>(
    trace: &FailureTrace,
    pool: &sqlx::PgPool,
    operation: &'static str,
    failure: &'static str,
    diagnosing: bool,
    missed: &mut Vec<&'static str>,
    future: impl std::future::Future<Output = T>,
) -> T {
    let mut pending = Box::pin(future.with_subscriber(trace.recorder()));
    let began = std::time::Instant::now();
    trace.mark(format!(
        "{operation} begins; budget=100ms; pool size={} idle={}",
        pool.size(),
        pool.num_idle()
    ));
    let answer = tokio::time::timeout(Duration::from_millis(100), &mut pending).await;
    trace.mark(format!(
        "{operation} read elapsed={:?}; budget=100ms; timed_out={}; pool size={} idle={}",
        began.elapsed(),
        answer.is_err(),
        pool.size(),
        pool.num_idle()
    ));
    match answer {
        Ok(reply) => reply,
        Err(error) if diagnosing => {
            missed.push(operation);
            let reply = tokio::time::timeout(rig::PATIENCE, pending)
                .await
                .expect(failure);
            trace.mark(format!(
                "{operation} diagnostic completion elapsed={:?}; original budget=100ms; timeout={error}",
                began.elapsed()
            ));
            reply
        }
        Err(error) => panic!(
            "{failure}: {error}; elapsed={:?}; budget=100ms",
            began.elapsed()
        ),
    }
}

struct Unanswered;
#[async_trait::async_trait]
impl HostServices for Unanswered {
    async fn enrollment(
        &self,
        _conn: &mut orch_db::Conn,
        _ask: &EnrollmentAsk,
    ) -> Result<Enrollment, HostingError> {
        std::future::pending().await
    }
    async fn tools(
        &self,
        _conn: &mut orch_db::Conn,
        _os: &str,
        _arch: &str,
        _harnesses: &[String],
    ) -> Result<Vec<ToolBuild>, HostingError> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn a_provider_whose_new_unit_never_answers_is_read_and_listed_in_under_a_hundred_milliseconds()
 {
    let diagnosing = std::env::var_os("ORCH_TEST_SQL_TIMINGS").is_some();
    let s = rig::Scene::for_requests().await;
    s.rig.engine.stop(); // Isolate the read from the ordinary background lifecycle scheduler.
    for db in [&s.rig.world.app.app.db, s.rig.engine.db()] {
        assert_eq!(
            db.pool().options().get_max_connections(),
            s.rig.world.db.pool.options().get_max_connections(),
            "the request profile retains the fixture's serving width"
        );
        let settings: (String, String, String) = sqlx::query_as(
            "SELECT current_setting('jit'), current_setting('statement_timeout'),
                    current_setting('idle_in_transaction_session_timeout')",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            settings,
            ("off".to_owned(), "15s".to_owned(), "10s".to_owned())
        );
    }
    s.rig.engine.attach_host_services(Arc::new(Unanswered));
    for kind in [
        CommandKind::AgentStart,
        CommandKind::ProviderRefresh,
        CommandKind::NodeResourcesInspect,
        CommandKind::NodeRuntimeStart,
    ] {
        s.rig.machine.handler.hold(kind);
    }
    let mut conn = s.rig.world.conn().await;
    let host = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(s.ada.id()),
        Some(s.tree.org.base.id),
        "Silent host",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE hosts SET machine_id=$2,drivers=$3 WHERE id=$1").bind(host).bind(s.rig.machine_id)
        .bind(json!({"docker":{"name":"docker","title":"Docker","lifecycle":"unit","spec_schemas":{"provider":{}},"live_changes":{}}}))
        .execute(&mut *conn).await.unwrap();
    // The system's template of providers' runtimes, as an installation's seed makes it.
    sqlx::query("INSERT INTO env_templates (id, name, kind, driver, spec) VALUES ($1, 'Provider runtime', 'provider', 'docker', '{}') ON CONFLICT (id) DO NOTHING")
        .bind(orch_db::seeds::PROVIDER_RUNTIME_TEMPLATE_ID)
        .execute(&mut *conn)
        .await
        .unwrap();
    let runtime = new_id();
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,provider_id,driver,template_id,spec_hash,state) VALUES($1,'provider',$2,$3,$4,'docker',$5,$6,'creating')")
        .bind(runtime).bind(host).bind(format!("unanswered-{runtime}")).bind(s.on.provider)
        .bind(orch_db::seeds::PROVIDER_RUNTIME_TEMPLATE_ID).bind(vec![0_u8;32]).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE providers SET machine_id=NULL,key_machine_id=NULL,status='needs_setup',facts_at=NULL,facts_requested_at=NULL WHERE id=$1")
        .bind(s.on.provider).execute(&mut *conn).await.unwrap();
    drop(conn);
    let before = s.rig.commands.issued_total();
    let mut missed_budgets = Vec::new();
    let mut trace = FailureTrace::new("provider GET/LIST phases", Duration::from_millis(100));
    let request = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&s.on.provider])
        .token(&s.ada.token);
    let read = timed_read(
        &trace,
        s.rig.world.app.app.db.pool(),
        "GET",
        "a provider GET must not wait for a native or catalogue answer",
        diagnosing,
        &mut missed_budgets,
        request.send(),
    )
    .await;
    trace.mark(format!("GET status={}", read.status));
    let read = read.ok();
    assert_eq!(read.base.id, s.on.provider);
    assert_eq!(
        read.runtime.as_ref().map(|runtime| runtime.state.as_str()),
        Some("creating")
    );
    let request = s
        .rig
        .world
        .app
        .call(ProviderList, &[])
        .token(&s.ada.token)
        .query(&ProviderListQuery {
            scope: Some(ProviderListScope::Mine),
            ..ProviderListQuery::default()
        });
    let listed = timed_read(
        &trace,
        s.rig.world.app.app.db.pool(),
        "LIST",
        "a provider list must not resolve the unit or release payload",
        diagnosing,
        &mut missed_budgets,
        request.send(),
    )
    .await;
    trace.mark(format!("LIST status={}", listed.status));
    let listed = listed.ok();
    assert!(
        listed
            .items
            .iter()
            .filter_map(orch_api_contract::common::Listed::as_visible)
            .any(|row| row.base.id == s.on.provider)
    );
    assert_eq!(
        s.rig.commands.issued_total(),
        before,
        "stored reads issue no native command"
    );
    // The serving pool is separate from the raw fixture pool.
    trace.mark("closing the fixture pool");
    tokio::time::timeout(rig::PATIENCE, s.rig.db.pool().close())
        .await
        .expect("the request-profile fixture pool closes");
    assert!(
        missed_budgets.is_empty(),
        "provider reads exceeded their original 100 ms budgets: {missed_budgets:?}"
    );
    trace.finish();
}
