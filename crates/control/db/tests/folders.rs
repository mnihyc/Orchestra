//! Projects inside projects as the tables see them: what a project counts, how deep projects lie one inside another, which threads a fork, a switch or a move holds, and a topic's use following it when it is moved.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::{NaiveDate, TimeZone, Utc};
use orch_api_contract::usage::UsageBucket;
use orch_db::queries::hierarchy::{
    self,
    testing::{insert_node, put_thread_in_environment},
};
use orch_db::queries::identity::testing::insert_user;
use orch_db::usage::ledger::{
    self, Attribution, NewEvent, NewSummary, SummaryAttribution, Tokens, record, record_summary,
};
use orch_db::usage::rollup::{self, Filter, Read};
use orch_db::{Conn, Db};
use orch_testkit::database::TestDb;
use orch_types::{Id, Json, new_id};
use sqlx::postgres::PgPoolOptions;

/// Acme with the project Web; in it the folder Checkout, and in that the folder Payments; a topic in
/// each of the three, and a thread in the topic of Checkout.
struct Tree {
    ada: Id,
    acme: Id,
    web: Id,
    checkout: Id,
    payments: Id,
    plan: Id,
    cart: Id,
    refunds: Id,
    thread: Id,
}

async fn tree(conn: &mut Conn) -> Tree {
    let ada = new_id();
    insert_user(conn, ada, "ada").await.unwrap();
    let [acme, web, checkout, payments, plan, cart, refunds, thread] =
        std::array::from_fn(|_| new_id());
    insert_node(conn, acme, "org", None, "Acme", ada)
        .await
        .unwrap();
    insert_node(conn, web, "project", Some(acme), "Web", ada)
        .await
        .unwrap();
    insert_node(conn, checkout, "project", Some(web), "Checkout", ada)
        .await
        .unwrap();
    insert_node(conn, payments, "project", Some(checkout), "Payments", ada)
        .await
        .unwrap();
    insert_node(conn, plan, "topic", Some(web), "Plan", ada)
        .await
        .unwrap();
    insert_node(conn, cart, "topic", Some(checkout), "Cart", ada)
        .await
        .unwrap();
    insert_node(conn, refunds, "topic", Some(payments), "Refunds", ada)
        .await
        .unwrap();
    insert_node(conn, thread, "thread", Some(cart), "Fix", ada)
        .await
        .unwrap();
    Tree {
        ada,
        acme,
        web,
        checkout,
        payments,
        plan,
        cart,
        refunds,
        thread,
    }
}

#[tokio::test]
async fn a_project_counts_the_projects_directly_inside_it_and_not_the_ones_deeper() {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let t = tree(&mut conn).await;
    let mut counts =
        hierarchy::project_counts(&mut conn, &[t.web, t.checkout, t.payments, t.acme, t.plan])
            .await
            .unwrap();
    counts.sort_by_key(|(id, _)| *id);
    let mut expected = vec![(t.web, 1), (t.checkout, 1), (t.payments, 0)];
    expected.sort_by_key(|(id, _)| *id);
    assert_eq!(
        counts, expected,
        "an org and a topic are not among projects"
    );
    // A deleted one is not counted.
    sqlx::query("UPDATE nodes SET deleted_at = now() WHERE id = $1")
        .bind(t.payments)
        .execute(&mut *conn)
        .await
        .unwrap();
    let counts = hierarchy::project_counts(&mut conn, &[t.checkout])
        .await
        .unwrap();
    assert_eq!(counts, vec![(t.checkout, 0)]);
}

#[tokio::test]
async fn how_many_projects_lie_one_inside_another_is_counted_down_a_path_and_within_a_project() {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let t = tree(&mut conn).await;
    for (node, expected) in [
        (t.acme, 0),
        (t.web, 1),
        (t.checkout, 2),
        (t.payments, 3),
        (t.plan, 1),
        (t.refunds, 3),
        (t.thread, 2),
    ] {
        assert_eq!(
            hierarchy::projects_on_the_way(&mut conn, node)
                .await
                .unwrap(),
            expected,
            "projects on the way to {node}"
        );
    }
    for (project, expected) in [(t.web, 3), (t.checkout, 2), (t.payments, 1)] {
        assert_eq!(
            hierarchy::deepest_chain_of_projects(&mut conn, project)
                .await
                .unwrap(),
            expected,
            "the deepest chain within {project}"
        );
    }
    // A sibling folder beside Checkout that holds one more beside Payments changes nothing
    // until it is deeper.
    let side = new_id();
    insert_node(&mut conn, side, "project", Some(t.web), "Docs", t.ada)
        .await
        .unwrap();
    assert_eq!(
        hierarchy::deepest_chain_of_projects(&mut conn, t.web)
            .await
            .unwrap(),
        3
    );
    let deeper = new_id();
    insert_node(
        &mut conn,
        deeper,
        "project",
        Some(t.payments),
        "Cards",
        t.ada,
    )
    .await
    .unwrap();
    assert_eq!(
        hierarchy::deepest_chain_of_projects(&mut conn, t.web)
            .await
            .unwrap(),
        4
    );
    // What is deleted does not make a chain longer.
    sqlx::query("UPDATE nodes SET deleted_at = now() WHERE id = $1")
        .bind(deeper)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        hierarchy::deepest_chain_of_projects(&mut conn, t.web)
            .await
            .unwrap(),
        3
    );
}

/// The threads of a topic that a fork, a switch or a move holds.
async fn held(conn: &mut Conn, topic: Id) -> Vec<Id> {
    hierarchy::threads_held_in_topic(conn, topic)
        .await
        .unwrap()
        .into_iter()
        .map(|each| each.thread)
        .collect()
}

async fn open_job(
    conn: &mut Conn,
    kind: &str,
    node: Option<Id>,
    environment: Option<Id>,
    state: &str,
    plan: &Json,
) {
    sqlx::query(
        "INSERT INTO jobs (id, kind, node_id, environment_id, state, plan) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(new_id())
    .bind(kind)
    .bind(node)
    .bind(environment)
    .bind(state)
    .bind(plan)
    .execute(conn)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_thread_is_held_while_a_switch_a_fork_or_a_move_of_its_environment_is_open() {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let t = tree(&mut conn).await;
    assert!(held(&mut conn, t.cart).await.is_empty(), "nothing runs");

    // A switch holds the thread it replaces while it is queued or running, and not once it ended.
    let replace = serde_json::json!({ "result": "replace" });
    open_job(
        &mut conn,
        "thread_fork",
        Some(t.thread),
        None,
        "succeeded",
        &replace,
    )
    .await;
    open_job(
        &mut conn,
        "thread_fork",
        Some(t.thread),
        None,
        "failed",
        &replace,
    )
    .await;
    assert!(
        held(&mut conn, t.cart).await.is_empty(),
        "jobs that ended hold nothing"
    );
    open_job(
        &mut conn,
        "thread_fork",
        Some(t.thread),
        None,
        "queued",
        &replace,
    )
    .await;
    assert_eq!(held(&mut conn, t.cart).await, vec![t.thread]);
    assert!(
        held(&mut conn, t.plan).await.is_empty(),
        "another topic is not held by it"
    );
    sqlx::query("UPDATE jobs SET state = 'cancelled'")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(held(&mut conn, t.cart).await.is_empty());

    // A fork of a thread touches it and holds nothing of it; the thread it is making is held.
    let fork = serde_json::json!({ "result": "new_thread" });
    open_job(
        &mut conn,
        "thread_fork",
        Some(t.thread),
        None,
        "running",
        &fork,
    )
    .await;
    assert!(
        held(&mut conn, t.cart).await.is_empty(),
        "the source of a fork is not held"
    );
    let made = new_id();
    insert_node(&mut conn, made, "thread", Some(t.cart), "Copy", t.ada)
        .await
        .unwrap();
    let making = serde_json::json!({ "result": "new_thread", "new_thread": made.to_string() });
    open_job(
        &mut conn,
        "thread_fork",
        Some(t.thread),
        None,
        "running",
        &making,
    )
    .await;
    assert_eq!(held(&mut conn, t.cart).await, vec![made]);
    sqlx::query("UPDATE jobs SET state = 'succeeded'")
        .execute(&mut *conn)
        .await
        .unwrap();

    // The move of an environment holds every thread bound to it, of whatever topic.
    let environment = put_thread_in_environment(&mut conn, t.thread, "managed", t.ada, t.cart)
        .await
        .unwrap();
    open_job(
        &mut conn,
        "environment_move",
        None,
        Some(environment),
        "running",
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(held(&mut conn, t.cart).await, vec![t.thread]);
    assert!(held(&mut conn, t.refunds).await.is_empty());
}

fn place(t: &Tree, project: Id, topic: Id, thread: Option<Id>) -> Attribution {
    Attribution {
        org_id: t.acme,
        project_id: project,
        topic_id: topic,
        thread_id: thread.unwrap_or_else(new_id),
        segment_id: new_id(),
        turn_id: new_id(),
        provider_id: new_id(),
        entry_id: None,
        user_id: Some(t.ada),
        agent_id: None,
        harness_kind: Some("codex".to_owned()),
        environment_id: None,
        environment_kind: Some("managed".to_owned()),
        arrangement: None,
        turn_model: "gpt-6-sol".to_owned(),
        effort: None,
        params: Json::Null,
    }
}

fn used(tokens: i64) -> Tokens {
    Tokens {
        input: tokens,
        cached_input: 0,
        cache_write: 0,
        output: tokens / 10,
        reasoning: 0,
        total: tokens + tokens / 10,
    }
}

async fn write(conn: &mut Conn, place: &Attribution, day: u32, tokens: i64) {
    record(
        conn,
        &NewEvent {
            id: new_id(),
            at: Utc.with_ymd_and_hms(2026, 10, day, 9, 0, 0).unwrap(),
            origin: ledger::origin::FORWARDER,
            scope: ledger::scope::REQUEST,
            counted: true,
            attribution: place,
            model: "gpt-6-sol",
            tier: None,
            tokens: used(tokens),
            context_tokens: None,
            request_ref: None,
            rate: None,
            rate_source: None,
            credits: None,
        },
    )
    .await
    .unwrap();
}

/// What a topic used, in tokens, by every way the tables keep it.
async fn totals(conn: &mut Conn, topic: Id) -> (i64, i64, i64) {
    let [ledger, daily, monthly] = ["usage_events", "usage_daily", "usage_monthly"].map(|table| {
        format!("SELECT COALESCE(sum(total_tokens), 0)::bigint FROM {table} WHERE topic_id = $1")
    });
    let mut out = [0_i64; 3];
    for (slot, statement) in [ledger, daily, monthly].into_iter().enumerate() {
        out[slot] = sqlx::query_scalar(sqlx::AssertSqlSafe(statement))
            .bind(topic)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    }
    (out[0], out[1], out[2])
}

async fn projects_named(conn: &mut Conn, table: &str, topic: Id) -> Vec<Id> {
    let statement = format!("SELECT DISTINCT project_id FROM {table} WHERE topic_id = $1");
    sqlx::query_scalar(sqlx::AssertSqlSafe(statement))
        .bind(topic)
        .fetch_all(conn)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_use_of_a_topic_that_is_moved_names_the_project_it_lies_in_now_and_no_figure_changes() {
    let test = TestDb::fresh().await;
    let db = Db::from_pool(
        PgPoolOptions::new()
            .max_connections(4)
            .connect(&test.url)
            .await
            .unwrap(),
    );
    let mut conn = db.acquire().await.unwrap();
    let t = tree(&mut conn).await;
    // The topic Cart used something on two days of one month, and a summary of it was made; the
    // topic Plan of the project above used something too.
    let at_cart = place(&t, t.checkout, t.cart, Some(t.thread));
    write(&mut conn, &at_cart, 3, 1_000).await;
    write(&mut conn, &at_cart, 4, 2_000).await;
    record_summary(
        &mut conn,
        &NewSummary {
            id: new_id(),
            at: Utc.with_ymd_and_hms(2026, 10, 4, 10, 0, 0).unwrap(),
            attribution: &SummaryAttribution {
                org_id: t.acme,
                project_id: t.checkout,
                topic_id: t.cart,
                thread_id: None,
                provider_id: new_id(),
                entry_id: None,
                user_id: Some(t.ada),
            },
            model: "gpt-6-mini",
            tier: None,
            tokens: used(400),
            context_tokens: None,
            request_ref: None,
            rate: None,
            rate_source: None,
            credits: None,
        },
    )
    .await
    .unwrap();
    write(&mut conn, &place(&t, t.web, t.plan, None), 5, 500).await;
    let before = totals(&mut conn, t.cart).await;
    assert_eq!(before.0, 3_300 + 440, "two requests and a summary");
    assert_eq!(before.0, before.1);
    assert_eq!(before.1, before.2);

    // The topic is moved from Checkout to Payments.
    let mut tx = db.begin().await.unwrap();
    let rows = hierarchy::repoint_usage(&mut tx, t.cart, t.payments)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(rows.ledger, 3, "the three rows of the ledger");
    assert!(rows.daily >= 1 && rows.monthly >= 1);
    for table in ["usage_events", "usage_daily", "usage_monthly"] {
        assert_eq!(
            projects_named(&mut conn, table, t.cart).await,
            vec![t.payments],
            "{table} names the project the topic lies in"
        );
    }
    assert_eq!(
        totals(&mut conn, t.cart).await,
        before,
        "no figure of the topic changed"
    );
    assert_eq!(
        projects_named(&mut conn, "usage_events", t.plan).await,
        vec![t.web],
        "what is of another topic stays where it was"
    );
    // A second time changes nothing.
    let mut tx = db.begin().await.unwrap();
    let again = hierarchy::repoint_usage(&mut tx, t.cart, t.payments)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!((again.ledger, again.daily, again.monthly), (0, 0, 0));

    // The rollups are what a rebuild from the ledger makes of it.
    let daily: Vec<String> =
        sqlx::query_scalar("SELECT row_to_json(t)::text FROM usage_daily t ORDER BY 1")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    let monthly: Vec<String> =
        sqlx::query_scalar("SELECT row_to_json(t)::text FROM usage_monthly t ORDER BY 1")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    let mut tx = db.begin().await.unwrap();
    rollup::rebuild(&mut tx, None).await.unwrap();
    tx.commit().await.unwrap();
    let daily_after: Vec<String> =
        sqlx::query_scalar("SELECT row_to_json(t)::text FROM usage_daily t ORDER BY 1")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    let monthly_after: Vec<String> =
        sqlx::query_scalar("SELECT row_to_json(t)::text FROM usage_monthly t ORDER BY 1")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(daily, daily_after, "a rebuild gives the rows that stand");
    assert_eq!(monthly, monthly_after);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_figures_of_a_project_are_those_of_its_topics_and_of_the_folders_in_it() {
    let test = TestDb::fresh().await;
    let db = Db::from_pool(
        PgPoolOptions::new()
            .max_connections(4)
            .connect(&test.url)
            .await
            .unwrap(),
    );
    let mut conn = db.acquire().await.unwrap();
    let t = tree(&mut conn).await;
    write(&mut conn, &place(&t, t.web, t.plan, None), 3, 100).await;
    write(
        &mut conn,
        &place(&t, t.checkout, t.cart, Some(t.thread)),
        3,
        200,
    )
    .await;
    write(&mut conn, &place(&t, t.payments, t.refunds, None), 4, 400).await;
    // What each place reads, by the days and by the hour (from the ledger itself).
    let read = |place: Id, bucket: Option<UsageBucket>| {
        let db = &db;
        async move {
            let mut conn = db.acquire().await.unwrap();
            let node = rollup::place_of(&mut conn, place).await.unwrap().unwrap();
            let filter = Filter {
                node: Some(node),
                ..Filter::default()
            };
            rollup::read(
                &mut conn,
                &Read {
                    from: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                    to: NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(),
                    filter: &filter,
                    bucket,
                    by: None,
                    under: None,
                },
            )
            .await
            .unwrap()
            .iter()
            .map(|cell| cell.figures.total_tokens)
            .sum::<i64>()
        }
    };
    for bucket in [None, Some(UsageBucket::Hour)] {
        assert_eq!(
            read(t.web, bucket).await,
            110 + 220 + 440,
            "a project sums its own topics and the folders', {bucket:?}"
        );
        assert_eq!(
            read(t.checkout, bucket).await,
            220 + 440,
            "a folder sums its own and the ones inside, {bucket:?}"
        );
        assert_eq!(read(t.payments, bucket).await, 440);
        assert_eq!(
            read(t.acme, bucket).await,
            770,
            "the root org sums them all"
        );
        assert_eq!(read(t.plan, bucket).await, 110);
    }
}
