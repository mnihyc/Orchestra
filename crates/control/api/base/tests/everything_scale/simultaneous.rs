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
        world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&reader.token)
            .send()
            .await
            .ok()
    };
    let expected = open().await;
    let mut solo = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        let reply = open().await;
        solo.push(start.elapsed());
        assert_eq!(reply.sync, expected.sync);
    }
    solo.sort_unstable();
    let released = Instant::now();
    let results = futures::future::join_all((0..10).map(|_| async {
        let reply = open().await;
        (released.elapsed(), reply)
    }))
    .await;
    let times: Vec<_> = results.iter().map(|(time, _)| *time).collect();
    eprintln!("opening solo={solo:?} ten={times:?}");
    for (time, reply) in results {
        assert_eq!(reply.sync, expected.sync);
        assert_eq!(reply.draft, expected.draft);
        assert!(
            time <= solo[1] + solo[1] / 2,
            "solo={solo:?}, batch={times:?}"
        );
    }
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
    capture.take();
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
