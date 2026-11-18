//! Every page keeps the same database round trips and result rows as unrelated teams grow.

use crate::places::{Who, person};
use crate::support::{StatementReads, Statements, World};
use orch_api_contract::{
    approvals, auth, documents, nodes, notifications, personal, search, settings, threads, topics,
};
use orch_types::Id;
use std::time::Instant;

// Recorded on both fixture sizes for imported histories. These ceilings catch extra constant
// round trips too; a faster implementation may use fewer statements or return fewer database rows.
const IMPORTED_BUDGETS: &[(&str, usize, u64)] = &[
    ("threads::ThreadOpeningGet", 205, 959),
    ("threads::ThreadSyncGet", 137, 648),
    ("threads::ThreadOutlineGet", 51, 274),
    ("settings::ThreadParameterGet", 52, 161),
    ("threads::ThreadLimitsGet", 22, 77),
    ("threads::ThreadDraftGet", 13, 64),
    ("topics::TopicSyncGet", 248, 876),
    ("documents::NodeDocumentList", 252, 1552),
    ("documents::DocumentGet", 245, 808),
    ("documents::DocumentDiscussionList", 251, 674),
    ("documents::DocumentSyncGet", 258, 817),
    ("nodes::NodeGet", 40, 136),
    ("nodes::NodeChildList", 43, 273),
    ("nodes::OrgTreeGet", 72, 581),
    ("nodes::TopicList", 62, 245),
    ("search::SearchList", 115, 1026),
    ("search::NameSearchList", 250, 769),
    ("auth::MeGet", 12, 10),
    ("personal::MeStateGet", 8, 10),
    ("personal::MeFavoriteList", 12, 10),
    ("nodes::OrgList", 67, 293),
    ("notifications::NotificationList", 10, 9),
    ("notifications::NotificationSummaryGet", 10, 10),
    ("approvals::ApprovalList", 10, 15),
];

// Measured before sharing per-snapshot facts, with the connected threads explicitly shared.
const BOUND_BUDGETS: &[(&str, usize, u64)] = &[
    ("threads::ThreadOpeningGet", 366, 1440),
    ("threads::ThreadSyncGet", 198, 795),
    ("threads::ThreadOutlineGet", 57, 283),
    ("settings::ThreadParameterGet", 115, 398),
    ("threads::ThreadLimitsGet", 32, 88),
    ("threads::ThreadDraftGet", 15, 67),
    ("topics::TopicSyncGet", 386, 1618),
    ("documents::NodeDocumentList", 252, 1552),
    ("documents::DocumentGet", 245, 808),
    ("documents::DocumentDiscussionList", 251, 674),
    ("documents::DocumentSyncGet", 258, 817),
    ("nodes::NodeGet", 40, 136),
    ("nodes::NodeChildList", 43, 273),
    ("nodes::OrgTreeGet", 83, 712),
    ("nodes::TopicList", 87, 490),
    ("search::SearchList", 123, 1117),
    ("search::NameSearchList", 274, 941),
    ("auth::MeGet", 12, 10),
    ("personal::MeStateGet", 8, 10),
    ("personal::MeFavoriteList", 12, 10),
    ("nodes::OrgList", 67, 293),
    ("notifications::NotificationList", 10, 9),
    ("notifications::NotificationSummaryGet", 10, 10),
    ("approvals::ApprovalList", 10, 15),
];

#[derive(Debug)]
struct Observation {
    route: &'static str,
    reads: StatementReads,
}

struct Measurements {
    routes: Vec<Observation>,
    permission_rows: u64,
}

fn trace_queries(route: &str) -> bool {
    route == "nodes::OrgTreeGet"
}

fn physical_permission_rows(plan: &serde_json::Value) -> u64 {
    let count = |field: &str| plan[field].as_u64().unwrap_or_default();
    let own = if plan["Relation Name"] == "permission_entries" {
        (count("Actual Rows")
            + count("Rows Removed by Filter")
            + count("Rows Removed by Index Recheck"))
            * count("Actual Loops")
    } else {
        0
    };
    own + plan["Plans"]
        .as_array()
        .map(|plans| plans.iter().map(physical_permission_rows).sum::<u64>())
        .unwrap_or_default()
}

async fn fixture(scale: i32, bound: bool) -> (World, Who, [Id; 5]) {
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
    if bound {
        sqlx::raw_sql(include_str!("page_bound.sql"))
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE agents SET descriptor=$1")
            .bind(crate::rig::descriptor())
            .execute(&mut *conn)
            .await
            .unwrap();
        let bound_threads:i64=sqlx::query_scalar("SELECT count(*) FROM threads WHERE agent_id IS NOT NULL AND provider_id IS NOT NULL AND environment_id IS NOT NULL")
            .fetch_one(&mut *conn).await.unwrap();
        assert_eq!(bound_threads, 100 * i64::from(scale));
    }
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
            20 * scale + 1,
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

async fn pages(scale: i32, bound: bool) -> Measurements {
    let variant = if bound { "bound" } else { "imported" };
    let (world, reader, [org, project, topic, thread, document]) = fixture(scale, bound).await;
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
    eprintln!("page root plan variant={variant} scale={scale} {plan}");
    let permission_rows = physical_permission_rows(&plan[0]["Plan"]);
    eprintln!("page root permission rows variant={variant} scale={scale} rows={permission_rows}");
    let at = orch_types::Time::new(world.now());
    let principal =
        orch_authz::Principal::load(&mut conn, orch_authz::Caller::session(reader.id()), at)
            .await
            .unwrap();
    let places = [org, project, topic, thread];
    let mut pages: Vec<_> = places
        .iter()
        .map(|id| orch_db::queries::access::PageKey::new("node", *id, "live"))
        .collect();
    pages.push(orch_db::queries::access::PageKey::new(
        "document", document, "live",
    ));
    let plan = orch_db::queries::access::participation::explain_marks(
        &mut conn,
        reader.id(),
        principal.groups(),
        principal.member_of(),
        at,
        &places,
        &pages,
        &principal.memberships().roots(),
        &[],
        None,
        &[org],
        false,
    )
    .await
    .unwrap();
    eprintln!("page hotspot plan variant={variant} scale={scale} kind=participation {plan}");
    let plan = orch_db::queries::hierarchy::explain_ancestors(&mut conn, &[thread])
        .await
        .unwrap();
    eprintln!("page hotspot plan variant={variant} scale={scale} kind=ancestors {plan}");
    // Function-scan plans retain the actual function's settings and total cost; their bodies
    // remain opaque here, so they are timing/buffer evidence rather than physical-row budgets.
    let plan: serde_json::Value = sqlx::query_scalar(
        "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON)
        SELECT * FROM access_holders($1,$2,false)",
    )
    .bind(reader.id())
    .bind(world.now())
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    eprintln!("page hotspot plan variant={variant} scale={scale} kind=holders {plan}");
    let plan:serde_json::Value=sqlx::query_scalar("EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON)
        SELECT wanted,user_id FROM access_holder_people_many(ARRAY['role','members'],ARRAY[$1,$1]::uuid[],ARRAY['developer',''],$2,true)
        ORDER BY wanted,user_id")
        .bind(org).bind(world.now()).fetch_one(&mut *conn).await.unwrap();
    eprintln!("page hotspot plan variant={variant} scale={scale} kind=holder_people {plan}");
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
            let call=world
                    .app
                    .call($endpoint, $ids)
                    .token(&reader.token)
                    .raw_query($query)
                    .send_bytes();
            let (reply,reads,queries)=if trace_queries(stringify!($endpoint)) {
                let (reply,work)=Statements::queries(call).await;
                (reply,work.totals,Some(work.statements))
            } else {let (reply,reads)=Statements::reads(call).await;(reply,reads,None)};
            let elapsed=started.elapsed();
            assert!(
                reply.status.is_success(),
                "{}: {}",
                stringify!($endpoint),
                String::from_utf8_lossy(&reply.raw)
            );
            eprintln!(
                "page budget variant={variant} scale={scale} route={} statements={} rows={} bytes={} elapsed_ms={}",
                stringify!($endpoint),
                reads.statements,
                reads.rows,
                reply.raw.len(),
                elapsed.as_secs_f64() * 1_000.0
            );
            if let Some(queries)=queries {
                for (sql,work) in queries {
                    eprintln!("page query variant={variant} scale={scale} route={} statements={} rows={} sql={}",
                        stringify!($endpoint),work.statements,work.rows,sql.replace('\n'," "));
                }
            }
            assert!(
                reads.statements > 0 && reads.rows > 0,
                "SQL tracing must measure the real request"
            );
            let budgets=if bound {BOUND_BUDGETS} else {IMPORTED_BUDGETS};
            let budget = budgets
                .iter()
                .find(|budget| budget.0 == stringify!($endpoint))
                .expect("every measured page has a statement and returned-row budget");
            assert!(
                reads.statements <= budget.1 && reads.rows <= budget.2,
                "{} exceeded its {variant} budget: {:?}, ceiling {} statements / {} rows",
                stringify!($endpoint),
                reads,
                budget.1,
                budget.2
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
    Measurements {
        routes: observed,
        permission_rows,
    }
}

#[tokio::test]
async fn every_page_reads_the_same_bounded_rows_when_the_installation_grows_fourfold() {
    // These are the stress tool's team figures and its fourfold comparison, not product limits.
    same_budget(pages(1, false).await, pages(4, false).await, 53);
}

#[tokio::test]
async fn bound_threads_keep_the_same_page_work_when_unrelated_teams_grow_fourfold() {
    same_budget(pages(1, true).await, pages(4, true).await, 107);
}

fn same_budget(ordinary: Measurements, larger: Measurements, permission_budget: u64) {
    let mut growth = Vec::new();
    for (ordinary, larger) in ordinary.routes.iter().zip(&larger.routes) {
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
    assert!(
        larger.permission_rows <= ordinary.permission_rows,
        "root discovery examines unrelated permission rows: {} -> {}",
        ordinary.permission_rows,
        larger.permission_rows
    );
    assert!(
        ordinary.permission_rows <= permission_budget
            && larger.permission_rows <= permission_budget,
        "root discovery exceeds its held-identity permission budget of {permission_budget}: {} -> {}",
        ordinary.permission_rows,
        larger.permission_rows
    );
}

#[tokio::test]
async fn native_page_statements_and_their_function_bodies_stay_below_the_database_time_goal() {
    use crate::hotspots::{BAR_MS, Capture};
    use orch_api_contract::everything::EverythingList;

    let mut slow = Vec::new();
    for (bound, scale) in [(false, 1), (false, 4), (true, 1), (true, 4)] {
        let variant = if bound { "bound" } else { "imported" };
        let (world, reader, [org, project, _, thread, _]) = fixture(scale, bound).await;
        eprintln!("database hotspot fixture variant={variant} scale={scale}");
        let db = &world.app.app.db;
        Capture::configure(db, true).await;
        let capture = Capture::default();
        capture.verify(db).await;
        macro_rules! read {
            ($route:path, $ids:expr, $query:expr) => {{
                let reply = capture
                    .run(
                        world
                            .app
                            .call($route, $ids)
                            .token(&reader.token)
                            .raw_query($query)
                            .send_bytes(),
                    )
                    .await;
                assert!(
                    reply.status.is_success(),
                    "{}: {}",
                    stringify!($route),
                    String::from_utf8_lossy(&reply.raw)
                );
                for plan in capture.take() {
                    eprintln!(
                        "database hotspot scale={scale} route={} duration_ms={} plan={}",
                        stringify!($route),
                        plan.duration_ms,
                        plan.body
                    );
                    if plan.duration_ms >= BAR_MS {
                        slow.push(format!(
                            "variant={variant} scale={scale} {} {}ms {}",
                            stringify!($route),
                            plan.duration_ms,
                            plan.body["Query Text"].as_str().unwrap_or_default()
                        ));
                    }
                }
            }};
        }
        // The first opening includes cold holder preparation. Search uses the real bounded
        // item reader and current roots; navigation uses its native complete projection.
        read!(threads::ThreadOpeningGet, &[&thread], "limit=100");
        read!(search::SearchList, &[], "q=Scale&limit=20");
        read!(search::NameSearchList, &[], "q=Scale&limit=20");
        read!(nodes::OrgTreeGet, &[&org], "");
        read!(EverythingList, &[], &format!("parent={project}&limit=20"));
        Capture::configure(db, false).await;
        world.app.app.realtime.stop().await;
    }
    assert!(
        slow.is_empty(),
        "native database statements exceeded {BAR_MS}ms; exact nested plans are above:\n{}",
        slow.join("\n")
    );
}
