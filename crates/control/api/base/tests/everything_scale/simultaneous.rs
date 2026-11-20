//! Opening contention and tuple locks taken during an idle installation minute.
use super::{
    hotspots::Capture,
    page_budgets::fixture,
    support::{Statements, World},
};
use orch_api_contract::threads::ThreadOpeningGet;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

#[tokio::test]
async fn ten_simultaneous_openings_finish_within_half_again_the_time_of_one() {
    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let open = || async {
        let response = world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&reader.token)
            .send()
            .await;
        let id = response.headers[orch_api_contract::common::REQUEST_ID_HEADER]
            .to_str()
            .unwrap()
            .to_owned();
        (response.ok(), id)
    };
    let (expected, _) = open().await;
    let mut solo = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        let (reply, _) = open().await;
        solo.push(start.elapsed());
        assert_eq!(reply.sync, expected.sync);
    }
    solo.sort_unstable();
    let released = Instant::now();
    let results = futures::future::join_all((0..10).map(|_| async {
        let trace = orch_api_core::testing::Boundaries::new();
        let reply = orch_api_core::testing::follow(trace.clone(), open()).await;
        (released.elapsed(), reply, trace)
    }))
    .await;
    let times: Vec<_> = results.iter().map(|(time, _, _)| *time).collect();
    eprintln!("opening solo={solo:?} ten={times:?}");
    assert_eq!(
        results
            .iter()
            .filter(|(_, _, trace)| trace.entered("ThreadOpeningGet", "selected.read"))
            .count(),
        1,
        "each request authenticates, and only one builds the opening"
    );
    for (_, _, trace) in &results {
        eprintln!("opening phases {:?}", trace.all(released));
    }
    // The request consumes the native HTTP reply before timing stops. The installed opening
    // has one data request and needs under 100 ms of server time at the team's figures.
    let server_budget = Duration::from_millis(100);
    assert!(
        solo.iter().all(|&time| time < server_budget),
        "individual openings: {solo:?}"
    );
    let reference = serde_json::to_value(&results[0].1.0).unwrap();
    for (time, (reply, id), _) in results {
        let mut expected_wire = reference.clone();
        for name in ["screen", "gateway", "pending_agent_schedules"] {
            if expected_wire[name]["status"] == "failed" {
                expected_wire[name]["problem"]["request_id"] = serde_json::json!(id);
            }
        }
        assert_eq!(serde_json::to_value(&reply).unwrap(), expected_wire);

        assert_eq!(reply.sync, expected.sync);
        assert_eq!(reply.draft, expected.draft);
        assert!(time < server_budget, "concurrent openings: {times:?}");
        assert!(
            time <= solo[1] + solo[1] / 2,
            "solo={solo:?}, batch={times:?}"
        );
    }
}

#[tokio::test]
async fn opening_a_bound_thread_reads_its_queue_once_for_the_header_history_and_controls() {
    use orch_api_contract::nodes::{Node, NodeGet};

    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let Node::Thread(header) = world
        .app
        .call(NodeGet, &[&thread])
        .token(&reader.token)
        .send()
        .await
        .ok()
    else {
        panic!("expected the bound thread");
    };
    let (response, work) = Statements::queries(
        world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&reader.token)
            .send(),
    )
    .await;
    let opening = response.ok();
    assert_eq!(opening.sync.thread.queue, header.queue);
    assert_eq!(opening.sync.thread.display, header.display);
    assert_eq!(opening.sync.thread.costs, header.costs);
    assert_eq!(
        opening.sync.thread.my_last_read_seq,
        header.my_last_read_seq
    );
    assert!(!opening.sync.items.is_empty());
    assert!(opening.parameters.is_some());
    let queue_reads: usize = work
        .statements
        .iter()
        .filter(|(sql, _)| sql.contains("FROM thread_queue_eligibility WHERE thread_id="))
        .map(|(_, reads)| reads.statements)
        .sum();
    assert_eq!(queue_reads, 1, "one current queue serves this opening");
    let participation_reads: usize = work
        .statements
        .iter()
        .filter(|(sql, _)| sql.contains("-- Ordinary assignment summaries"))
        .map(|(_, reads)| reads.statements)
        .sum();
    assert_eq!(
        participation_reads, 1,
        "only the node header shows participation"
    );
}

#[tokio::test]
async fn an_opening_reads_each_visible_history_item_once_plus_its_lookahead() {
    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let (reply, work) = Statements::timed_queries(
        world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&reader.token)
            .send(),
    )
    .await;
    let shown = u64::try_from(reply.ok().sync.items.len()).unwrap();
    assert!(shown > 0);
    let rows: u64 = work
        .statements
        .iter()
        .filter(|(sql, _)| {
            sql.contains("SELECT selected.id,o.frozen_at,selected.seq_created")
                && sql.contains("ORDER BY selected.seq_created DESC LIMIT $5")
        })
        .map(|(_, reads)| reads.rows)
        .sum();
    assert!(
        (shown..=shown + 1).contains(&rows),
        "history returned {rows} rows for {shown} displayed items"
    );
}

fn locks(plan: &serde_json::Value) -> u64 {
    let own = if plan["Node Type"] == "LockRows" {
        plan["Actual Rows"].as_u64().unwrap() * plan["Actual Loops"].as_u64().unwrap()
    } else {
        0
    };
    own + plan["Plans"]
        .as_array()
        .into_iter()
        .flatten()
        .map(locks)
        .sum::<u64>()
}

#[tokio::test]
async fn an_ordinary_opening_reads_its_credential_without_pinning_any_rows() {
    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let (reply, work) = Statements::queries(
        world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&reader.token)
            .send(),
    )
    .await;
    reply.ok();
    assert!(work.totals.statements > 0);
    for sql in work.statements.keys() {
        let words = sql
            .to_ascii_uppercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            ![
                "FOR UPDATE",
                "FOR NO KEY UPDATE",
                "FOR KEY SHARE",
                "FOR SHARE"
            ]
            .iter()
            .any(|clause| words.contains(clause)),
            "opening locks rows: {sql}"
        );
    }
}

#[tokio::test]
async fn a_quiet_minute_lists_its_statements_and_takes_no_row_locks() {
    let world = World::for_requests().await;
    let app = &world.app.app;
    let capture = Capture::default();
    Capture::configure(&app.db, true).await;
    capture.verify(&app.db).await;
    let mut connections = Vec::new();
    for _ in 0..app.db.pool().options().get_max_connections() {
        connections.push(app.db.pool().acquire().await.unwrap());
    }
    for conn in &mut connections {
        sqlx::query("SET auto_explain.log_min_duration=0")
            .execute(&mut **conn)
            .await
            .unwrap();
    }
    drop(connections);
    // All tasks of this isolated server run on this current-thread test runtime.
    let _subscriber = tracing::subscriber::set_default(capture.clone());
    app.link.start().await.unwrap();
    app.engine.start();
    app.realtime.start(app.engine.clone());
    tokio::time::timeout(Duration::from_secs(30), async {
        while app.engine.sweeps_run() < 2 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    drop(capture.take());
    // Prove the collector sees actual locked tuples before observing the idle minute.
    capture
        .run(
            sqlx::query("SELECT slot FROM document_notice_slots ORDER BY slot LIMIT 1 FOR UPDATE")
                .fetch_all(app.db.pool()),
        )
        .await
        .unwrap();
    assert!(capture.take().iter().any(|plan| {
        plan.body["Query Text"]
            .as_str()
            .is_some_and(|sql| sql.starts_with("SELECT slot FROM document_notice_slots"))
            && locks(&plan.body["Plan"]) == 1
    }));
    let start = Instant::now();
    let before = app.engine.sweeps_run();
    let mut statements = BTreeMap::<String, (usize, u64)>::new();
    while start.elapsed() < Duration::from_secs(60) {
        tokio::time::sleep(Duration::from_secs(1)).await;
        world.advance(chrono::Duration::seconds(1));
        for plan in capture.take() {
            let sql = plan.body["Query Text"].as_str().unwrap().to_owned();
            let count = statements.entry(sql).or_default();
            count.0 += 1;
            count.1 += locks(&plan.body["Plan"]);
        }
    }
    app.engine.stop();
    app.link.shutdown();
    app.realtime.stop().await;
    assert!(app.engine.sweeps_run() > before);
    assert!(
        statements
            .keys()
            .any(|sql| sql.contains("document_notice_slots") || sql.contains("document_notices")),
        "quiet runtime must inspect the notice queue"
    );
    for (sql, (calls, locked)) in &statements {
        eprintln!("quiet calls={calls} locked={locked} sql={sql}");
    }
    assert!(
        statements.values().all(|(_, locked)| *locked == 0),
        "an idle sweep locked a row"
    );
}
