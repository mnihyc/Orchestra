//! Every page keeps the same database round trips and result rows as unrelated teams grow.

use crate::places::{Who, person};
use crate::support::{StatementReads, Statements, World};
use orch_api_contract::{
    approvals, auth, documents, nodes, notifications, personal, search, settings, threads, topics,
};
use orch_types::Id;
use std::time::Instant;

#[derive(Debug)]
struct Observation {
    route: &'static str,
    reads: StatementReads,
}

async fn fixture(scale: i32) -> (World, Who, [Id; 5]) {
    let world = World::for_requests().await;
    let reader = person(&world, "page-reader").await;
    let mut conn = world.conn().await;
    for (key, value) in [
        ("orch.fixture_scale", scale.to_string()),
        ("orch.fixture_reader", reader.id().to_string()),
        (
            "orch.fixture_personal",
            reader.person.personal_org.to_string(),
        ),
    ] {
        sqlx::query("SELECT set_config($1,$2,false)")
            .bind(key)
            .bind(value)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    sqlx::raw_sql(include_str!("page_budgets.sql"))
        .execute(&mut *conn)
        .await
        .unwrap();
    let ids: Vec<Id> = sqlx::query_scalar(
        "SELECT pg_temp.page_id(kind,1) FROM unnest(ARRAY['org','project','topic','thread','document']) WITH ORDINALITY k(kind,n) ORDER BY n",
    ).fetch_all(&mut *conn).await.unwrap();
    let counts: (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM users),(SELECT count(*) FROM orgs WHERE org_kind='team'),
                (SELECT count(*) FROM nodes WHERE kind='project'),(SELECT count(*) FROM threads),
                (SELECT count(*) FROM items),(SELECT count(*) FROM documents),(SELECT count(*) FROM providers)",
    ).fetch_one(&mut *conn).await.unwrap();
    let scale = i64::from(scale);
    assert_eq!(
        counts,
        (
            20 * scale,
            10 * scale,
            100 * scale,
            100 * scale,
            20_000 * scale,
            1_000 * scale,
            10 * scale
        )
    );
    drop(conn);
    (world, reader, ids.try_into().unwrap())
}

async fn pages(scale: i32) -> Vec<Observation> {
    let (world, reader, [org, project, topic, thread, document]) = fixture(scale).await;
    let mut conn = world.conn().await;
    let held =
        orch_authz::holders::held(&mut conn, reader.id(), orch_types::Time::new(world.now()))
            .await
            .unwrap();
    let plan = orch_db::queries::search::explain_name_roots(
        &mut conn,
        reader.id(),
        world.now(),
        &held.held,
    )
    .await
    .unwrap();
    eprintln!("page root plan scale={scale} {plan}");
    drop(conn);
    let mut observed = Vec::new();
    macro_rules! read {
        ($endpoint:path, $ids:expr, $query:expr) => {{
            // Warm the same per-server projections that a page reuses. Measure a retained request,
            // so a connection's first preparation is not mistaken for growth with the installation.
            let warm = world
                .app
                .call($endpoint, $ids)
                .token(&reader.token)
                .raw_query($query)
                .send_bytes()
                .await;
            assert!(
                warm.status.is_success(),
                "{}: {}",
                stringify!($endpoint),
                String::from_utf8_lossy(&warm.raw)
            );
            let started = Instant::now();
            let (reply, reads) = Statements::reads(
                world
                    .app
                    .call($endpoint, $ids)
                    .token(&reader.token)
                    .raw_query($query)
                    .send_bytes(),
            )
            .await;
            assert!(
                reply.status.is_success(),
                "{}: {}",
                stringify!($endpoint),
                String::from_utf8_lossy(&reply.raw)
            );
            eprintln!(
                "page budget scale={scale} route={} statements={} rows={} bytes={} elapsed_ms={}",
                stringify!($endpoint),
                reads.statements,
                reads.rows,
                reply.raw.len(),
                started.elapsed().as_secs_f64() * 1_000.0
            );
            assert!(
                reads.statements > 0 && reads.rows > 0,
                "SQL tracing must measure the real request"
            );
            observed.push(Observation {
                route: stringify!($endpoint),
                reads,
            });
        }};
    }
    read!(threads::ThreadOpeningGet, &[&thread], "limit=100");
    read!(threads::ThreadSyncGet, &[&thread], "limit=100");
    read!(threads::ThreadOutlineGet, &[&thread], "limit=50");
    read!(settings::ThreadParameterGet, &[&thread], "");
    read!(threads::ThreadLimitsGet, &[&thread], "");
    read!(threads::ThreadDraftGet, &[&thread], "");
    read!(topics::TopicSyncGet, &[&topic], "");
    read!(documents::NodeDocumentList, &[&project], "limit=50");
    read!(documents::DocumentGet, &[&document], "");
    read!(documents::DocumentDiscussionList, &[&document], "limit=50");
    read!(documents::DocumentSyncGet, &[&document], "");
    read!(nodes::NodeGet, &[&project], "");
    read!(nodes::NodeChildList, &[&org], "kind=project&limit=50");
    read!(nodes::OrgTreeGet, &[&org], "");
    read!(nodes::TopicList, &[&project], "");
    read!(search::SearchList, &[], "q=Scale&limit=20");
    read!(search::NameSearchList, &[], "q=Scale&limit=20");
    read!(auth::MeGet, &[], "");
    read!(personal::MeStateGet, &[], "");
    read!(personal::MeFavoriteList, &[], "");
    read!(nodes::OrgList, &[], "");
    read!(notifications::NotificationList, &[], "limit=50");
    read!(notifications::NotificationSummaryGet, &[], "");
    read!(approvals::ApprovalList, &[], "");
    world.app.app.realtime.stop().await;
    observed
}

#[tokio::test]
async fn every_page_reads_the_same_bounded_rows_when_the_installation_grows_fourfold() {
    // These are the stress tool's team figures and its fourfold comparison, not product limits.
    let ordinary = pages(1).await;
    let larger = pages(4).await;
    let mut growth = Vec::new();
    for (ordinary, larger) in ordinary.iter().zip(&larger) {
        assert_eq!(ordinary.route, larger.route);
        if larger.reads.statements != ordinary.reads.statements
            || larger.reads.rows > ordinary.reads.rows
        {
            growth.push(format!(
                "{}: {:?} -> {:?}",
                ordinary.route, ordinary.reads, larger.reads
            ));
        }
    }
    assert!(
        growth.is_empty(),
        "page database budgets grew with unrelated teams:\n{}",
        growth.join("\n")
    );
}
