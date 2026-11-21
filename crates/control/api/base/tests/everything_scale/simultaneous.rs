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
    let budget = solo[1] + solo[1] / 2;
    if times.iter().any(|time| *time > budget) {
        use crate::support::failure_trace::FailureTrace;
        use tracing::instrument::WithSubscriber;

        // Repeat only a failed batch so subscriber overhead does not affect the comparison.
        // Its SQL and pool events distinguish a slow shared read from a slow fresh View check.
        let trace = FailureTrace::with_tail("Slow concurrent team opening", budget, 1024);
        futures::future::join_all((0..10).map(|_| open()))
            .with_subscriber(trace.recorder())
            .await;
    }
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
        assert!(time <= budget, "solo={solo:?}, batch={times:?}");
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
async fn a_team_opening_reads_each_visible_message_body_once_across_streams() {
    use orch_api_contract::threads::ThreadOpeningGetQuery;

    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    sqlx::query("UPDATE items SET stream='sub:opening' WHERE thread_id=$1 AND seq_created=200")
        .bind(thread)
        .execute(&mut *world.conn().await)
        .await
        .unwrap();
    for (limit, first) in [(2, 198), (200, 1)] {
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
        let sync = reply.ok().sync;
        let positions: Vec<_> = sync.items.iter().map(|item| item.seq_created).collect();
        assert_eq!(positions, (first..=200).collect::<Vec<_>>());
        assert_eq!(sync.has_older, first > 1);
        let traced: Vec<_> = work
            .statements
            .iter()
            .filter(|(sql, _)| sql.contains("AS has_older") && sql.contains("FROM items"))
            .collect();
        assert!(
            traced
                .iter()
                .all(|(sql, _)| !sql.contains("thread_item_revisions")
                    && !sql.contains("thread_continuation_spans")),
            "a first screen with no inherited spans reads current messages directly"
        );
        let decoded: u64 = traced.iter().map(|(_, count)| count.rows).sum();
        assert_eq!(
            decoded,
            sync.items.len() as u64,
            "each visible body crosses the database boundary once"
        );
        assert_eq!(
            traced
                .iter()
                .map(|(_, count)| count.statements)
                .sum::<usize>(),
            1,
            "the overlapping windows and pagination marker share one read"
        );
    }
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
        // Collect the two traces on a fresh team's first and returning visits. The original
        // measurements above exclude subscriber overhead and both use the same real route.
        drop(world);
        let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
        for name in ["Slow first team opening", "Slow returning team opening"] {
            let trace = FailureTrace::with_tail(name, Duration::from_millis(100), 1024);
            world
                .app
                .call(ThreadOpeningGet, &[&thread])
                .token(&reader.token)
                .send()
                .with_subscriber(trace.recorder())
                .await
                .ok();
        }
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
    if times[0] >= Duration::from_millis(100) {
        use crate::support::failure_trace::FailureTrace;
        use tracing::instrument::WithSubscriber;
        // Measure first, then trace the cookie route on a fresh team so its caches
        // and session renewal match the first visit. Never record the cookie.
        drop(world);
        let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
        let cookie = world.sign_in(&reader.person).await.browser();
        let trace = FailureTrace::with_tail(
            "Slow first session and thread opening",
            Duration::from_millis(100),
            1024,
        );
        let traced = world
            .app
            .call(AuthSessionThreadOpen, &[&thread])
            .header(CSRF_HEADER, "1")
            .header("cookie", &cookie)
            .send_bytes()
            .with_subscriber(trace.recorder())
            .await;
        assert_eq!(traced.status.as_u16(), 200);
    }
    assert!(
        times.iter().all(|time| *time < Duration::from_millis(100)),
        "first and returning server times: {times:?}"
    );
}

#[tokio::test]
async fn a_team_opening_can_follow_sign_in_with_only_one_free_connection() {
    use orch_api_contract::auth::AuthSessionThreadOpen;
    use orch_api_contract::common::CSRF_HEADER;
    use orch_api_contract::nodes::OverviewPart;

    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let pool = world.app.app.db.pool();
    let mut reserved = Vec::new();
    for _ in 0..pool.options().get_max_connections() {
        reserved.push(pool.acquire().await.unwrap());
    }
    reserved.pop().unwrap().return_to_pool().await;
    let signed = world.sign_in(&reader.person).await;
    assert_eq!(
        pool.num_idle(),
        1,
        "the browser can reuse its sign-in connection"
    );
    let opened = world
        .app
        .call(AuthSessionThreadOpen, &[&thread])
        .header(CSRF_HEADER, "1")
        .header("cookie", &signed.browser())
        .send()
        .await
        .ok()
        .expect("the session cookie restores its reader");
    assert_eq!(opened.session.user.user.id, reader.id());
    assert!(matches!(opened.opening, OverviewPart::Ready { data } if !data.sync.items.is_empty()));
}

#[tokio::test]
async fn a_team_opening_reads_the_thread_status_and_participation_once_for_the_first_screen() {
    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let (reply, work) = Statements::queries(
        world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&reader.token)
            .send(),
    )
    .await;
    assert!(!reply.ok().sync.items.is_empty());
    assert!(
        !work
            .statements
            .keys()
            .any(|sql| sql.contains("JOIN documents d ON d.id = n.readme_document_id")),
        "an opening without a README need not look up a document"
    );
    let lock_reads: usize = work
        .statements
        .iter()
        .filter(|(sql, _)| {
            sql.contains("restore_recovery_required") || sql.contains("thread_native_guards")
        })
        .map(|(_, count)| count.statements)
        .sum();
    assert_eq!(
        lock_reads, 1,
        "open jobs and retained preparations share one lock lookup"
    );
    let account_reads: usize = work
        .statements
        .iter()
        .filter(|(sql, _)| sql.contains("AS suspended_in") || sql.contains("d.path AS domain_path"))
        .map(|(_, count)| count.statements)
        .sum();
    assert_eq!(
        account_reads, 1,
        "the reader's account and domains are read together"
    );
    assert!(
        !work
            .statements
            .keys()
            .any(|sql| sql.contains("array_agg(g.group_id) FROM group_members")),
        "the reader's group set already comes from holder expansion"
    );
    assert!(
        !work
            .statements
            .keys()
            .any(|sql| sql.contains("SELECT t.node_id,i.payload FROM threads t")),
        "opening a conversation must not fetch the full latest message again to suggest links"
    );
    let display = work
        .statements
        .keys()
        .find(|sql| sql.contains(" recovery_has_lock"))
        .expect("the first screen reads the native thread status");
    assert!(
        !display.contains("JOIN threads selected"),
        "approval counts use the display's own selected continuation"
    );
    assert!(
        !work
            .statements
            .keys()
            .any(|sql| sql.contains("q.pending_inputs,q.held_inputs,a.pending_approvals")),
        "the opening should have read active queue and approval counts with the thread status"
    );
    for detail in [
        "FROM picked JOIN items i",
        "FROM held JOIN items m ON m.id=held.item_id",
    ] {
        assert!(
            !work.statements.keys().any(|sql| sql.contains(detail)),
            "an opening without background work should not resolve its sources: {detail}"
        );
    }
    let participation = work
        .statements
        .keys()
        .find(|sql| sql.contains("Ordinary assignment summaries and current role/follow facts"))
        .expect("the opening shows participation for the addressed thread");
    assert!(
        !participation.contains("direct_requests") && !participation.contains("blockers AS"),
        "a single node without scope restrictions needs only its descendant assignment count"
    );
    assert!(
        !participation.contains("gone.deleted_at"),
        "roles and follows use the live paths already maintained for participation"
    );
    assert!(
        !participation.contains("orch_participation_node_count"),
        "a single node's participation can stop at the first current assignment"
    );
    assert!(
        !display.contains("FROM jobs j"),
        "an idle thread need not plan the recovery job search for its status"
    );
    assert!(
        !work
            .statements
            .keys()
            .any(|sql| sql.contains("Direct participation for selected resource pages")),
        "the thread shows the caller's environment permissions without reading resource-page participation"
    );
    for fragment in [
        "Ordinary assignment summaries and current role/follow facts",
        " recovery_has_lock",
        "AS pending_notices",
        "FROM thread_goals",
    ] {
        let calls: usize = work
            .statements
            .iter()
            .filter(|(sql, _)| sql.contains(fragment))
            .map(|(_, count)| count.statements)
            .sum();
        assert_eq!(calls, 1, "opening reads {fragment}");
    }
}

#[tokio::test]
async fn a_team_opening_keeps_held_background_completions_after_the_task_ends() {
    use orch_db::queries::{source::thread_display_facts, threads};
    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let viewer = reader.id();
    let at = orch_types::Time::new(world.now());
    let mut conn = world.conn().await;
    let marker = orch_types::new_id();
    sqlx::query(r#"INSERT INTO items(id,thread_id,segment_id,continuation_id,kind,status,payload,seq_created,seq_updated)
        SELECT $2,t.node_id,s.id,t.active_continuation_id,'marker','completed','{"data":{"causes":[{},{}]}}',201,201
        FROM threads t JOIN segments s ON s.thread_id=t.node_id WHERE t.node_id=$1 ORDER BY s.ordinal DESC LIMIT 1"#)
        .bind(thread).bind(marker).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO thread_inputs(id,thread_id,continuation_id,source,content,delivery,state,state_detail,position,client_id,seq_updated)
        SELECT $2,t.node_id,t.active_continuation_id,'system','[]','queue','pending','background_stopped',1,$3,201 FROM threads t WHERE t.node_id=$1")
        .bind(thread).bind(orch_types::new_id()).bind(format!("background:{marker}")).execute(&mut *conn).await.unwrap();
    drop(conn);
    let mut tx = world.app.app.db.begin_opening_snapshot().await.unwrap();
    orch_authz::read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            let row = threads::thread(conn, thread).await.unwrap().unwrap();
            thread_display_facts(conn, &[thread], Some(viewer), at)
                .await
                .unwrap();
            let wake = orch_engine::background_wake::status(conn, &row, &[], false)
                .await
                .unwrap();
            assert!(wake.tasks.is_empty());
            assert_eq!(wake.held_completions, 2);
        })
    })
    .await;
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn a_team_opening_reads_its_connected_directory_without_planning_a_unit_host() {
    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let (reply, work) = Statements::queries(
        world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&reader.token)
            .send(),
    )
    .await;
    assert!(!reply.ok().sync.items.is_empty());
    assert!(
        work.statements
            .keys()
            .any(|sql| sql.contains("m.facts -> 'environment_descriptor'"))
    );
    assert!(
        !work
            .statements
            .keys()
            .any(|sql| sql.contains("LEFT JOIN LATERAL (SELECT target.* FROM hosts target"))
    );
}

#[tokio::test]
async fn a_team_opening_reads_the_selected_model_and_environment_defaults_once() {
    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let (reply, work) = Statements::queries(
        world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&reader.token)
            .send(),
    )
    .await;
    let sync = reply.ok().sync;
    assert!(!sync.items.is_empty());
    assert_eq!(sync.thread.model_display.as_deref(), Some("Scale model"));
    for alternatives in [
        &[
            "provider_option_layers(",
            "SELECT display_name FROM provider_models",
        ][..],
        &[
            "th.node_id,e.id,e.placement,t.spec",
            "e.id AS environment_id, e.placement, t.spec AS template_spec",
        ],
    ] {
        let calls: usize = work
            .statements
            .iter()
            .filter(|(sql, _)| alternatives.iter().any(|fragment| sql.contains(*fragment)))
            .map(|(_, count)| count.statements)
            .sum();
        assert_eq!(calls, 1, "reads per opening for {alternatives:?}");
    }
}

#[tokio::test]
async fn a_team_opening_shows_an_inherited_limit_and_skips_billing_reads_when_it_has_none() {
    let (world, reader, [_, project, _, thread, _]) = fixture(1, true).await;
    let open = || async {
        Statements::queries(
            world
                .app
                .call(ThreadOpeningGet, &[&thread])
                .token(&reader.token)
                .send(),
        )
        .await
    };
    let (empty, queries) = open().await;
    assert!(empty.ok().limits.unwrap().reached.is_empty());
    assert!(
        !queries
            .statements
            .keys()
            .any(|sql| sql.contains("WITH live AS") && sql.contains("i.kind='tool_call'")),
        "without an applicable cap the first screen need not resolve who pays for overlapping turns"
    );
    let reads: usize = queries
        .statements
        .iter()
        .filter(|(sql, _)| sql.contains("AS pending_notices") || sql.contains("usage_caps c"))
        .map(|(_, work)| work.statements)
        .sum();
    assert_eq!(
        reads, 1,
        "the selected thread includes the lookup for covering caps"
    );
    let cap = orch_types::new_id();
    sqlx::query(
        "INSERT INTO usage_caps(id,node_id,meter,amount,created_by) VALUES($1,$2,'running',0,$3)",
    )
    .bind(cap)
    .bind(project)
    .bind(reader.id())
    .execute(world.app.app.db.pool())
    .await
    .unwrap();
    let (bounded, _) = open().await;
    assert!(bounded.ok().limits.unwrap().reached.iter().any(|limit| {
        limit
            .cap
            .as_ref()
            .is_some_and(|status| status.cap.id == cap)
    }));
}

#[tokio::test]
async fn a_team_opening_reads_the_selected_environment_and_continuation_once() {
    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let mut conn = world.app.app.db.acquire().await.unwrap();
    let (view, revision): (orch_types::Id, i64) = sqlx::query_as(
        "UPDATE thread_continuations c SET queue_revision = c.queue_revision + 7
         FROM threads t WHERE t.node_id=$1 AND c.id=t.active_continuation_id
         RETURNING c.id,c.queue_revision",
    )
    .bind(thread)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let marker = orch_db::queries::thread_continuations::mark_read(
        &mut conn,
        reader.id(),
        thread,
        view,
        7,
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let (reply, work) = Statements::queries(
        world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&reader.token)
            .send(),
    )
    .await;
    let sync = reply.ok().sync;
    assert!(!sync.items.is_empty());
    assert_eq!(sync.inputs_queue_revision, Some(revision));
    assert_eq!(sync.thread.my_last_read_seq, Some(marker));
    for repeat in [
        "SELECT * FROM thread_continuations WHERE thread_id=$1 AND id=$2",
        "SELECT COALESCE(r.last_read_seq,0)::bigint FROM thread_continuations",
        "SELECT e.environment_preparation,e.generation",
    ] {
        assert!(
            !work.statements.keys().any(|sql| sql.contains(repeat)),
            "unnecessary query for the idle thread: {repeat}",
        );
    }
    for query in [
        "c.id AS viewed_continuation_id",
        "SELECT revision, id, name, owner_id, org_id, placement, home_node_id",
        "e.id AS environment_id, e.placement, t.spec AS template_spec",
        "CASE WHEN h.id IS NULL THEN 'machine' ELSE 'host' END,COALESCE(h.id,m.id)",
    ] {
        let calls: usize = work
            .statements
            .iter()
            .filter(|(sql, _)| sql.contains(query))
            .map(|(_, count)| count.statements)
            .sum();
        assert_eq!(calls, 1, "reads per opening for {query}");
    }

    let viewer = reader.id();
    let mut tx = world.app.app.db.begin_opening_snapshot().await.unwrap();
    orch_authz::read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            use orch_db::queries::{
                thread_continuations::reading::last_read, threads::activity_many,
            };
            activity_many(conn, &[thread], Some(viewer)).await.unwrap();
            let missing = orch_types::new_id();
            for (person, target, continuation, expected, queries) in [
                (viewer, thread, view, Some(marker), 0),
                (missing, thread, view, Some(0), 1),
                (viewer, thread, missing, None, 1),
                (viewer, missing, view, None, 1),
            ] {
                let (position, calls) =
                    Statements::measure(last_read(conn, person, target, continuation)).await;
                assert_eq!((position.unwrap(), calls), (expected, queries));
            }
        })
    })
    .await;
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn a_team_opening_serves_current_messages_without_reconstructing_historical_rows() {
    use orch_db::queries::thread_continuations::{projection, reading};

    let (world, _, [_, _, _, thread, _]) = fixture(1, true).await;
    let db = &world.app.app.db;
    Capture::configure(db, true).await;
    let mut conn = db.acquire().await.unwrap();
    let view = projection(&mut conn, thread, None).await.unwrap().unwrap();
    sqlx::query("SET auto_explain.log_min_duration=0")
        .execute(&mut *conn)
        .await
        .unwrap();
    let capture = Capture::default();
    let items = capture
        .run(reading::items_bounded(
            &mut conn,
            thread,
            view.viewed_continuation_id,
            None,
            false,
            Some("main"),
            40,
        ))
        .await
        .unwrap();
    assert_eq!(items.len(), 40);

    fn historical_decodes(plan: &serde_json::Value) -> u64 {
        let own = if plan["Function Name"] == "jsonb_populate_record" {
            plan["Actual Loops"].as_u64().unwrap()
        } else {
            0
        };
        own + plan["Plans"]
            .as_array()
            .into_iter()
            .flatten()
            .map(historical_decodes)
            .sum::<u64>()
    }

    let plans = capture.take();
    let page = plans
        .iter()
        .find(|plan| {
            plan.body["Query Text"].as_str().is_some_and(|sql| {
                sql.contains("thread_continuation_spans") && sql.contains("candidate_ids")
            })
        })
        .expect("PostgreSQL reports the actual plan of the selected page");
    assert_eq!(historical_decodes(&page.body["Plan"]), 0);
}

#[tokio::test]
async fn a_team_opening_reads_imported_messages_without_loading_absent_turns_or_inputs() {
    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let open = || async {
        Statements::queries(
            world
                .app
                .call(ThreadOpeningGet, &[&thread])
                .token(&reader.token)
                .send(),
        )
        .await
    };
    let (first, work) = open().await;
    let sync = first.ok().sync;
    assert!(!sync.items.is_empty());
    assert!(!sync.segments.is_empty());
    assert!(sync.turns.is_empty() && sync.inputs.is_empty());
    assert!(sync.retained_inputs.is_empty() && sync.viewed_prompt.is_none());
    for detail in [
        "position::text AS position",
        "SELECT t.id,t.ordinal FROM turns t",
        "SELECT a.lower_seq,n.upper_seq,p.id",
        "SELECT v.input_id,v.group_id,v.continuation_id,v.ordinal,i.author_id,i.created_at",
    ] {
        assert!(
            !work.statements.keys().any(|sql| sql.contains(detail)),
            "{detail}"
        );
    }
    let display = work
        .statements
        .keys()
        .find(|sql| sql.contains(" recovery_has_lock"))
        .unwrap();
    assert!(!display.contains("last_turn.state"));
    assert!(!display.contains("count(*) FILTER(WHERE NOT i.queue_held"));

    let input = orch_types::new_id();
    sqlx::query("INSERT INTO thread_inputs(id,thread_id,source,content,delivery,state,position,created_at,seq_created,seq_updated,queue_held,state_detail)
        VALUES($1,$2,'system','[]','queue','pending',1,$3,201,201,false,'model_use_required')")
        .bind(input).bind(thread).bind(world.now()).execute(world.app.app.db.pool()).await.unwrap();
    let (next, _) = open().await;
    assert!(next.ok().sync.inputs.iter().any(|row| row.id == input));
}

#[tokio::test]
async fn a_team_opening_reads_the_thread_and_its_selected_continuation_together() {
    use orch_api_contract::threads::ThreadOpeningGetQuery;

    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let active: orch_types::Id =
        sqlx::query_scalar("SELECT active_continuation_id FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(world.app.app.db.pool())
            .await
            .unwrap();
    for viewed in [None, Some(active)] {
        let query = ThreadOpeningGetQuery {
            continuation_id: viewed,
            ..Default::default()
        };
        let (reply, work) = Statements::queries(
            world
                .app
                .call(ThreadOpeningGet, &[&thread])
                .token(&reader.token)
                .query(&query)
                .send(),
        )
        .await;
        let projection = reply.ok().sync.projection.unwrap();
        assert_eq!(projection.viewed_continuation_id, active);
        assert_eq!(projection.follows_active, viewed.is_none());
        let reads: usize = work
            .statements
            .iter()
            .filter(|(sql, _)| {
                sql.contains("AS pending_notices") || sql.contains("c.id AS viewed_continuation_id")
            })
            .map(|(_, work)| work.statements)
            .sum();
        assert_eq!(reads, 1, "thread and selected continuation for {viewed:?}");
    }
}

#[tokio::test]
async fn a_team_opening_skips_recovery_detail_reads_only_when_the_thread_has_none() {
    use serde_json::json;

    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let open = || async {
        Statements::queries(
            world
                .app
                .call(ThreadOpeningGet, &[&thread])
                .token(&reader.token)
                .send(),
        )
        .await
    };
    let (reply, work) = open().await;
    assert!(reply.ok().sync.thread.display.recovery.is_none());
    let display = work
        .statements
        .keys()
        .find(|sql| sql.contains(" recovery_has_lock"))
        .unwrap();
    assert!(!display.contains("JOIN model_call_recovery"));
    assert!(
        !work
            .statements
            .keys()
            .any(|sql| sql.contains("FROM model_call_recovery WHERE node_id=$1"))
    );

    let pool = world.app.app.db.pool();
    sqlx::query("UPDATE threads SET state='blocked',blocked_reason='error',blocked_detail='call_recovery' WHERE node_id=$1")
        .bind(thread).execute(pool).await.unwrap();
    let due = world.now() + chrono::Duration::seconds(55);
    sqlx::query("INSERT INTO model_call_recovery(node_id,purpose,call_key,reference,failure,rule,cursor,action,state,due_at,context,updated_at)
        VALUES($1,'agent',$1,$2,$3,$4,'{}',$5,'scheduled',$6,$7,$8)")
        .bind(thread).bind(orch_types::new_id())
        .bind(json!({"code":"provider_busy","message":"busy","said":"busy","retryable":true}))
        .bind(json!({"id":"busy","steps":[{"type":"retry","times":3}]}))
        .bind(json!({"attempt":2,"step":{"type":"retry","times":3}}))
        .bind(due).bind(json!({"effects": false})).bind(world.now())
        .execute(pool).await.unwrap();
    let (reply, work) = open().await;
    let retry = reply.ok().sync.thread.display.recovery.unwrap();
    assert_eq!(retry.attempt, 2);
    assert_eq!(retry.due_at, Some(orch_types::Time::new(due)));
    assert!(
        work.statements
            .keys()
            .any(|sql| sql.contains("FROM model_call_recovery WHERE node_id=$1"))
    );
}

#[tokio::test]
async fn a_team_opening_counts_outstanding_harness_reviews_and_shows_only_live_requests() {
    use orch_api_contract::threads::ThreadDisplayState;

    let (world, reader, [_, _, _, thread, _]) = fixture(1, true).await;
    let approval = orch_types::new_id();
    let pool = world.app.app.db.pool();
    sqlx::query("INSERT INTO approvals(id,kind,state,thread_id,title,request,required_permission) VALUES($1,'harness','pending',$2,'Review','{}','')")
        .bind(approval).bind(thread).execute(pool).await.unwrap();
    for expired in [false, true] {
        if expired {
            sqlx::query("UPDATE approvals SET expires_at=$2 WHERE id=$1")
                .bind(approval)
                .bind(world.now() - chrono::Duration::seconds(1))
                .execute(pool)
                .await
                .unwrap();
        }
        let (reply, work) = Statements::queries(
            world
                .app
                .call(ThreadOpeningGet, &[&thread])
                .token(&reader.token)
                .send(),
        )
        .await;
        let row = reply.ok().sync.thread;
        assert_eq!(row.pending_approvals, 1);
        assert_eq!(
            row.display.state == ThreadDisplayState::WaitingApproval,
            !expired
        );
        let display = work
            .statements
            .keys()
            .find(|sql| sql.contains(" recovery_has_lock"))
            .unwrap();
        assert_eq!(display.matches("FROM approvals ").count(), 1);
    }
}

#[tokio::test]
async fn a_team_opening_counts_scheduled_inputs_once_and_keeps_personal_definitions_out_of_the_pane()
 {
    use orch_api_contract::nodes::OverviewPart;

    let (world, reader, [_, _, topic, thread, _]) = fixture(1, true).await;
    let pool = world.app.app.db.pool();
    let seq: i64 = sqlx::query_scalar("SELECT topic_seq FROM topics WHERE node_id=$1")
        .bind(topic)
        .fetch_one(pool)
        .await
        .unwrap();
    for count in [0_i64, 1] {
        if count == 1 {
            sqlx::query("INSERT INTO input_schedules(id,topic_id,author_id,name,target,content,plan,delivery,timing,state,next_run_at,topic_seq)
                VALUES($1,$2,$3,'Personal reminder',$4,'[]',false,'queue','{}','active',$5,$6)")
                .bind(orch_types::new_id()).bind(topic).bind(reader.id())
                .bind(serde_json::json!({"kind":"thread","thread_id":thread}))
                .bind(world.now()).bind(seq).execute(pool).await.unwrap();
        }
        let (reply, work) = Statements::queries(
            world
                .app
                .call(ThreadOpeningGet, &[&thread])
                .token(&reader.token)
                .send(),
        )
        .await;
        let opening = reply.ok();
        assert_eq!(opening.sync.thread.display.queued_inputs, count);
        let Some(OverviewPart::Ready { data }) = opening.pending_agent_schedules else {
            panic!("the topic reader sees pending agent work");
        };
        assert_eq!(data.seq, seq);
        assert!(data.items.is_empty());
        let reads: usize = work
            .statements
            .iter()
            .filter(|(sql, _)| sql.contains("FROM input_schedules"))
            .map(|(_, work)| work.statements)
            .sum();
        assert_eq!(
            reads,
            if count == 0 { 1 } else { 2 },
            "one count per opening, plus agent details only for a nonempty destination"
        );
    }
}
