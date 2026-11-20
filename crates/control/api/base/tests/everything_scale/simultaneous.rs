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
        let trace = orch_api_core::testing::Boundaries::new();
        let reply = orch_api_core::testing::follow(trace.clone(), open()).await;
        (
            released.elapsed(),
            reply,
            trace.entered("ThreadOpeningGet", "selected.read"),
        )
    }))
    .await;
    let times: Vec<_> = results.iter().map(|(time, _, _)| *time).collect();
    eprintln!("opening solo={solo:?} ten={times:?}");
    assert_eq!(
        results.iter().filter(|(_, _, rendered)| *rendered).count(),
        1,
        "each request authenticates, and only one builds the opening"
    );
    for (time, reply, _) in results {
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

#[tokio::test]
async fn showing_more_messages_in_a_team_opening_takes_the_same_number_of_history_reads() {
    use orch_api_contract::threads::ThreadOpeningGetQuery;

    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let mut samples = Vec::new();
    for limit in [1, 40] {
        let (reply, work) = Statements::queries(
            world
                .app
                .call(ThreadOpeningGet, &[&thread])
                .token(&reader.token)
                .query(&ThreadOpeningGetQuery {
                    limit: Some(limit),
                    ..Default::default()
                })
                .send(),
        )
        .await;
        let opened = reply.ok();
        assert_eq!(opened.sync.items.len(), usize::try_from(limit).unwrap());
        let history: usize = work
            .statements
            .iter()
            .filter(|(sql, _)| sql.contains("thread_continuation_spans"))
            .map(|(_, count)| count.statements)
            .sum();
        assert!(history > 0);
        samples.push(history);
    }
    assert_eq!(
        samples[0], samples[1],
        "reading the same history must not cost a round trip per displayed message"
    );
}

#[tokio::test]
async fn a_team_members_first_screen_is_served_in_under_one_hundred_milliseconds() {
    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let mut times = Vec::new();
    for visit in ["first", "returning"] {
        let start = Instant::now();
        let response = world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&reader.token)
            .send_bytes()
            .await;
        let elapsed = start.elapsed();
        assert!(
            response.status.is_success(),
            "opening returned {}",
            response.status
        );
        let opening: orch_api_contract::threads::ThreadOpening =
            serde_json::from_slice(&response.raw).unwrap();
        assert!(!opening.sync.items.is_empty());
        eprintln!(
            "team opening visit={visit} elapsed={elapsed:?} bytes={}",
            response.raw.len()
        );
        times.push(elapsed);
    }
    if times.iter().any(|time| *time >= Duration::from_millis(100)) {
        use crate::support::failure_trace::FailureTrace;
        use tracing::instrument::WithSubscriber;
        // Follow a separate visit so recording individual SQL and native phases never taxes either timing.
        // As in the opening-scope test, keep enough events for every statement and its boundary.
        let trace = FailureTrace::with_tail("Slow team opening", Duration::from_millis(100), 1024);
        world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&reader.token)
            .send()
            .with_subscriber(trace.recorder())
            .await
            .ok();
    }
    assert!(
        times.iter().all(|time| *time < Duration::from_millis(100)),
        "first and returning server times: {times:?}"
    );
}

#[tokio::test]
async fn restoring_the_team_members_session_and_thread_takes_under_one_hundred_milliseconds() {
    use orch_api_contract::auth::{AuthSessionThreadOpen, AuthThreadOpening};
    use orch_api_contract::common::CSRF_HEADER;
    use orch_api_contract::nodes::OverviewPart;
    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let signed = world.sign_in(&reader.person).await;
    let cookie = signed.browser();
    let mut times = Vec::new();
    for visit in ["first", "returning"] {
        let start = Instant::now();
        let response = world
            .app
            .call(AuthSessionThreadOpen, &[&thread])
            .header(CSRF_HEADER, "1")
            .header("cookie", &cookie)
            .send_bytes()
            .await;
        let elapsed = start.elapsed();
        assert_eq!(response.status.as_u16(), 200);
        let opening: AuthThreadOpening = serde_json::from_slice(&response.raw).unwrap();
        assert_eq!(opening.session.user.user.id, reader.id());
        assert!(
            matches!(opening.opening, OverviewPart::Ready { data } if !data.sync.items.is_empty())
        );
        eprintln!(
            "team session and thread visit={visit} elapsed={elapsed:?} bytes={}",
            response.raw.len()
        );
        times.push(elapsed);
    }
    assert!(
        times.iter().all(|time| *time < Duration::from_millis(100)),
        "first and returning server times: {times:?}"
    );
}
