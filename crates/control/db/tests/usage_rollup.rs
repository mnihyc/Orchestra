//! The rollups by day and by month follow the ledger row by row and are the same as a rebuild from the ledger makes of it, whatever the ledger holds; a figure read from the ledger is the one read from the days.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use orch_api_contract::usage::{UsageBucket, UsageDimension};
use orch_db::usage::ledger::{
    self, Attribution, NewEvent, NewSummary, SummaryAttribution, Tokens, record, record_summary,
};
use orch_db::usage::rollup::{self, Filter, Read};
use orch_db::{Conn, Db};
use orch_testkit::database::TestDb;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;

async fn world() -> (TestDb, Db) {
    let db = TestDb::fresh().await;
    let pool = Db::from_pool(
        PgPoolOptions::new()
            .max_connections(4)
            .connect(&db.url)
            .await
            .unwrap(),
    );
    (db, pool)
}

/// A small deterministic source of choices, so that a failing ledger can be made again.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
    }

    fn one_of<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// The places, people and providers a random ledger is made of.
struct Pool {
    topics: Vec<(Id, Id, Id)>,
    threads: Vec<Id>,
    providers: Vec<Id>,
    entries: Vec<Id>,
    users: Vec<Id>,
    agents: Vec<Id>,
}

impl Pool {
    fn new() -> Self {
        let ids = |n: usize| (0..n).map(|_| new_id()).collect::<Vec<_>>();
        Self {
            topics: (0..4).map(|_| (new_id(), new_id(), new_id())).collect(),
            threads: ids(6),
            providers: ids(3),
            entries: ids(2),
            users: ids(3),
            agents: ids(2),
        }
    }
}

impl Pool {
    /// Topics that moved: each is found under two orgs and projects, so its rows name either place.
    fn moved() -> Self {
        let mut pool = Self::new();
        pool.topics = pool
            .topics
            .iter()
            .flat_map(|&(org, project, topic)| [(org, project, topic), (new_id(), new_id(), topic)])
            .collect();
        pool
    }
}

fn attribution(pool: &Pool, dice: &mut Dice, turn: Id) -> Attribution {
    let (org, project, topic) = *dice.one_of(&pool.topics);
    Attribution {
        org_id: org,
        project_id: project,
        topic_id: topic,
        thread_id: *dice.one_of(&pool.threads),
        segment_id: new_id(),
        turn_id: turn,
        provider_id: *dice.one_of(&pool.providers),
        entry_id: dice.below(2).eq(&0).then(|| *dice.one_of(&pool.entries)),
        user_id: dice.below(4).ne(&0).then(|| *dice.one_of(&pool.users)),
        agent_id: Some(*dice.one_of(&pool.agents)),
        harness_kind: dice
            .below(5)
            .ne(&0)
            .then(|| (*dice.one_of(&["codex", "claude-code"])).to_owned()),
        environment_id: None,
        environment_kind: dice
            .below(3)
            .ne(&0)
            .then(|| (*dice.one_of(&["managed", "connected"])).to_owned()),
        arrangement: Some("key_separated".to_owned()),
        turn_model: "gpt-6-sol".to_owned(),
        effort: dice
            .below(2)
            .eq(&0)
            .then(|| (*dice.one_of(&["low", "high"])).to_owned()),
        params: Json::Null,
    }
}

fn moment(dice: &mut Dice) -> DateTime<Utc> {
    // From the last days of September to the first of December: two month boundaries, and the
    // hours around midnight, where a day is told from the next.
    let start = Utc.with_ymd_and_hms(2026, 9, 27, 0, 0, 0).unwrap();
    let seconds = i64::try_from(dice.next() % (66 * 24 * 3600)).unwrap();
    let edge = [0, 1, 23 * 3600 + 3599, 3600, 12 * 3600];
    if dice.below(4) == 0 {
        start
            + Duration::days(i64::try_from(dice.below(66)).unwrap())
            + Duration::seconds(edge[dice.below(edge.len())])
    } else {
        start + Duration::seconds(seconds)
    }
}

/// Writes a random ledger live, a row at a time.
async fn random_ledger(conn: &mut Conn, pool: &Pool, dice: &mut Dice, rows: usize) {
    let mut turns: Vec<(Id, usize)> = Vec::new();
    for _ in 0..rows {
        // A turn that already has rows gets another now and then: a turn made many requests.
        let turn = if !turns.is_empty() && dice.below(3) == 0 {
            turns[dice.below(turns.len())].0
        } else {
            let turn = new_id();
            turns.push((turn, 0));
            turn
        };
        let place = attribution(pool, dice, turn);
        let input = i64::try_from(dice.below(5000)).unwrap();
        let cached = input / 4;
        let output = i64::try_from(dice.below(900)).unwrap();
        let tokens = Tokens {
            input,
            cached_input: cached,
            cache_write: input / 8,
            output,
            reasoning: output / 3,
            total: input + output,
        };
        let model = *dice.one_of(&["gpt-6-sol", "openai/gpt-6-mini", "claude-opus-5"]);
        // A third of the rows had no rate; of the rest some were estimated by the catalogue.
        let (rate, source, credits) = match dice.below(3) {
            0 => (None, None, None),
            1 => (
                Some(json!({ "input_per_mtok": "2" })),
                Some(ledger::rate_source::CATALOGUE),
                Some(format!("{}.{:06}", dice.below(3), dice.below(1_000_000))),
            ),
            _ => (
                Some(json!({ "input_per_mtok": "3" })),
                Some(ledger::rate_source::RATE_CARD),
                Some(format!("{}.{:06}", dice.below(3), dice.below(1_000_000))),
            ),
        };
        record(
            conn,
            &NewEvent {
                id: new_id(),
                at: moment(dice),
                origin: ledger::origin::FORWARDER,
                scope: ledger::scope::REQUEST,
                counted: dice.below(5) != 0,
                attribution: &place,
                model,
                // A third of the rows ran at the speed `fast`, a third at `flex`, and the rest at the ordinary one.
                tier: *dice.one_of(&[None, Some("fast"), Some("flex")]),
                tokens,
                context_tokens: Some(input),
                request_ref: None,
                rate: rate.as_ref(),
                rate_source: source,
                credits: credits.as_deref(),
            },
        )
        .await
        .unwrap();
        // Now and then Orch asked a model for a summary of a topic.
        if dice.below(10) == 0 {
            let (org, project, topic) = *dice.one_of(&pool.topics);
            record_summary(
                conn,
                &NewSummary {
                    id: new_id(),
                    at: moment(dice),
                    attribution: &SummaryAttribution {
                        org_id: org,
                        project_id: project,
                        topic_id: topic,
                        thread_id: None,
                        provider_id: *dice.one_of(&pool.providers),
                        entry_id: None,
                        user_id: Some(*dice.one_of(&pool.users)),
                    },
                    model: "gpt-6-mini",
                    tier: None,
                    tokens: Tokens {
                        input: 400,
                        cached_input: 0,
                        cache_write: 0,
                        output: 20,
                        reasoning: 0,
                        total: 420,
                    },
                    context_tokens: Some(400),
                    request_ref: None,
                    rate: None,
                    rate_source: None,
                    credits: None,
                },
            )
            .await
            .unwrap();
        }
    }
}

/// Both rollup tables as text, in a fixed order.
async fn both(conn: &mut Conn) -> (Vec<String>, Vec<String>) {
    let daily = sqlx::query_scalar("SELECT row_to_json(t)::text FROM usage_daily t ORDER BY 1")
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    let monthly = sqlx::query_scalar("SELECT row_to_json(t)::text FROM usage_monthly t ORDER BY 1")
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    (daily, monthly)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_rollups_kept_as_rows_arrive_are_what_a_rebuild_makes_of_any_ledger() {
    let (_db, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let pool = Pool::new();
    for seed in [7_u64, 1234, 987_654_321, 42_424_242] {
        sqlx::query("TRUNCATE usage_events, usage_daily, usage_monthly")
            .execute(&mut *conn)
            .await
            .unwrap();
        let mut dice = Dice(seed);
        random_ledger(&mut conn, &pool, &mut dice, 220).await;
        let live = both(&mut conn).await;
        assert!(live.0.len() > 20, "a ledger of some size: {}", live.0.len());
        assert!(live.1.len() > 5);

        let mut tx = db.begin().await.unwrap();
        let rebuilt = rollup::rebuild(&mut tx, None).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(rebuilt.daily, live.0.len() as u64);
        assert_eq!(rebuilt.monthly, live.1.len() as u64);
        let after = both(&mut conn).await;
        assert_eq!(live.0, after.0, "daily rows, seed {seed}");
        assert_eq!(live.1, after.1, "monthly rows, seed {seed}");

        // From a day in the middle of a month, and only from there on.
        let from = NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();
        let mut tx = db.begin().await.unwrap();
        rollup::rebuild(&mut tx, Some(from)).await.unwrap();
        tx.commit().await.unwrap();
        let partial = both(&mut conn).await;
        assert_eq!(live.0, partial.0, "daily rows from a day, seed {seed}");
        assert_eq!(live.1, partial.1, "monthly rows from a day, seed {seed}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_topic_that_moved_has_its_earliest_rows_place_kept_live_as_a_rebuild_finds_it() {
    let (_db, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let pool = Pool::moved();
    for seed in [5_u64, 77_777, 31_415_926] {
        sqlx::query("TRUNCATE usage_events, usage_daily, usage_monthly")
            .execute(&mut *conn)
            .await
            .unwrap();
        // The rows arrive out of the order of their times, so a row often names an earlier place than the one kept.
        random_ledger(&mut conn, &pool, &mut Dice(seed), 220).await;
        let live = both(&mut conn).await;
        let mut tx = db.begin().await.unwrap();
        rollup::rebuild(&mut tx, None).await.unwrap();
        tx.commit().await.unwrap();
        let after = both(&mut conn).await;
        assert_eq!(live.0, after.0, "daily rows, seed {seed}");
        assert_eq!(live.1, after.1, "monthly rows, seed {seed}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_month_is_the_sum_of_its_days() {
    let (_db, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    random_ledger(&mut conn, &Pool::new(), &mut Dice(99), 150).await;
    let (days, months): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COALESCE(sum(total_tokens), 0)::bigint FROM usage_daily),
                (SELECT COALESCE(sum(total_tokens), 0)::bigint FROM usage_monthly)",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert!(days > 0);
    assert_eq!(days, months);
    let different: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (
            SELECT date_trunc('month', day)::date AS month, topic_id, sum(credits) AS credits, sum(turns) AS turns
              FROM usage_daily GROUP BY 1, 2) d
          FULL JOIN (SELECT month, topic_id, sum(credits) AS credits, sum(turns) AS turns
                       FROM usage_monthly GROUP BY 1, 2) m USING (month, topic_id)
          WHERE d.credits IS DISTINCT FROM m.credits OR d.turns IS DISTINCT FROM m.turns",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(different, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuilding_rollups_restores_cache_writes_from_the_retained_ledger() {
    let (_db, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    // Different counted rows and nullable attribution keys exercise rebuilding real retained totals.
    random_ledger(&mut conn, &Pool::new(), &mut Dice(99), 80).await;
    let written: i64 =
        sqlx::query_scalar("SELECT sum(cache_write_tokens)::bigint FROM usage_daily")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert!(written > 0);
    let before = both(&mut conn).await;
    sqlx::raw_sql(
        "UPDATE usage_daily SET cache_write_tokens=0;
         UPDATE usage_monthly SET cache_write_tokens=0;",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    rollup::rebuild(&mut tx, None).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(before, both(&mut conn).await);
}

/// A counted row of a turn of a thread, by hand.
fn row<'a>(
    id: Id,
    at: DateTime<Utc>,
    place: &'a Attribution,
    tokens: Tokens,
    credits: Option<&'a str>,
    source: Option<&'a str>,
    counted: bool,
) -> NewEvent<'a> {
    NewEvent {
        id,
        at,
        origin: ledger::origin::FORWARDER,
        scope: ledger::scope::REQUEST,
        counted,
        attribution: place,
        model: "gpt-6-sol",
        tier: None,
        tokens,
        context_tokens: None,
        request_ref: None,
        rate: None,
        rate_source: source,
        credits,
    }
}

fn used(input: i64, output: i64) -> Tokens {
    Tokens {
        input,
        cached_input: 0,
        cache_write: 0,
        output,
        reasoning: 0,
        total: input + output,
    }
}

#[tokio::test]
async fn a_million_requests_in_a_thousand_threads_keep_ten_billion_tokens_and_cache_parts() {
    let (_fixture, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let owner = new_id();
    let org = new_id();
    let project = new_id();
    let topic = new_id();
    orch_db::queries::identity::testing::insert_user(&mut conn, owner, "large-ledger")
        .await
        .unwrap();
    for (id, kind, parent, name) in [
        (org, "org", None, "Scale"),
        (project, "project", Some(org), "Project"),
        (topic, "topic", Some(project), "Topic"),
    ] {
        orch_db::queries::hierarchy::testing::insert_node(&mut conn, id, kind, parent, name, owner)
            .await
            .unwrap();
    }
    let provider = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(owner),
        Some(org),
        "Ledger provider",
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
        SELECT md5('scale-thread-'||g.n::text)::uuid,'thread',$1,$2,ARRAY[$2,$3,$1,md5('scale-thread-'||g.n::text)::uuid],
          'Thread '||g.n::text,$4,$4 FROM generate_series(0,999) AS g(n)")
        .bind(topic).bind(org).bind(project).bind(owner).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO threads(node_id,topic_id) SELECT id,$1 FROM nodes WHERE kind='thread' AND parent_id=$1")
        .bind(topic).execute(&mut *conn).await.unwrap();
    let now = Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap();
    // This fixture measures retained-ledger totals, rebuilds and upgrades at size.
    // Seed bounded ingestion transactions, retaining native commit stamps and
    // every row, rather than measuring one million-row write transaction.
    const REQUESTS: i64 = 1_000_000;
    const REQUESTS_PER_TRANSACTION: i64 = 1_000;
    for first in (0..REQUESTS).step_by(REQUESTS_PER_TRANSACTION as usize) {
        let mut tx = db.begin().await.unwrap();
        let written = sqlx::query("INSERT INTO usage_events(id,change_seq,at,origin,purpose,scope,counted,org_id,project_id,topic_id,thread_id,turn_id,
          provider_id,user_id,model,harness_kind,environment_kind,input_tokens,cached_input_tokens,cache_write_tokens,output_tokens,reasoning_tokens,total_tokens,credits,outcome)
        SELECT md5('scale-usage-'||g.n::text)::uuid,g.n+1,$1::timestamptz-(g.n%1440)*interval '1 minute','forwarder','turn','request',true,$2,$3,$4,
          md5('scale-thread-'||(CASE WHEN g.n<10000 THEN 0 ELSE 1+g.n%999 END)::text)::uuid,md5('scale-turn-'||g.n::text)::uuid,
          $5,$6,'scale-model','codex','connected',8000,2000,1000,2000,500,10000,0,'done' FROM generate_series($7::bigint,$8::bigint) AS g(n)")
            .bind(now).bind(org).bind(project).bind(topic).bind(provider).bind(owner)
            .bind(first).bind(first + REQUESTS_PER_TRANSACTION - 1)
            .execute(&mut *tx).await.unwrap();
        assert_eq!(written.rows_affected(), REQUESTS_PER_TRANSACTION as u64);
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM usage_commit_rows WHERE xact=pg_current_xact_id()",
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(pending, REQUESTS_PER_TRANSACTION);
        tx.commit().await.unwrap();
    }
    let stamped: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_events WHERE change_seq > 0")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(stamped, REQUESTS, "every native commit stamp is retained");
    let (threads,total,hottest):(i64,i64,i64)=sqlx::query_as("SELECT count(*)::bigint,sum(tokens)::bigint,max(tokens)::bigint FROM (SELECT thread_id,sum(total_tokens)::bigint AS tokens FROM usage_events GROUP BY thread_id) t")
        .fetch_one(&mut *conn).await.unwrap();
    assert_eq!(threads, 1000);
    assert_eq!(total, 10_000_000_000);
    assert_eq!(hottest, 100_000_000);
    let mut tx = db.begin().await.unwrap();
    let rebuilt = rollup::rebuild(&mut tx, None).await.unwrap();
    tx.commit().await.unwrap();
    assert!(rebuilt.daily <= 2000);
    assert_eq!(rebuilt.monthly, 1);
    let filter = Filter {
        provider: Some(provider),
        ..Default::default()
    };
    let cells = rollup::read(
        &mut conn,
        &Read {
            from: (now - Duration::days(2)).date_naive(),
            to: now.date_naive(),
            filter: &filter,
            bucket: Some(UsageBucket::Day),
            by: None,
            under: None,
        },
    )
    .await
    .unwrap();
    assert!(cells.len() <= 2);
    assert_eq!(
        cells
            .iter()
            .map(|cell| cell.figures.total_tokens)
            .sum::<i64>(),
        10_000_000_000
    );
    assert_eq!(
        cells
            .iter()
            .map(|cell| cell.figures.cache_write_tokens)
            .sum::<i64>(),
        1_000_000_000
    );
    assert_eq!(
        cells
            .iter()
            .map(|cell| cell.figures.cached_input_tokens)
            .sum::<i64>(),
        2_000_000_000
    );
    assert_eq!(
        cells
            .iter()
            .map(|cell| cell.figures.performance.requests)
            .sum::<i64>(),
        1_000_000
    );
    let before = both(&mut conn).await;
    let mut tx = db.begin().await.unwrap();
    rollup::rebuild(&mut tx, None).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        before,
        both(&mut conn).await,
        "rebuilding a retained billion-token history preserves every rollup"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_adds_its_tokens_and_credits_once_and_tokens_without_a_rate_are_counted_apart() {
    let (_db, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let pool = Pool::new();
    let mut dice = Dice(5);
    let turn = new_id();
    let place = attribution(&pool, &mut dice, turn);
    let noon = Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap();
    let first = new_id();
    for _ in 0..2 {
        // Written again, it is the same row.
        record(
            &mut conn,
            &row(
                first,
                noon,
                &place,
                used(1000, 100),
                Some("0.002500"),
                Some("rate_card"),
                true,
            ),
        )
        .await
        .unwrap();
    }
    record(
        &mut conn,
        &row(
            new_id(),
            noon + Duration::hours(1),
            &place,
            used(2000, 0),
            Some("0.001000"),
            Some("catalogue"),
            true,
        ),
    )
    .await
    .unwrap();
    record(
        &mut conn,
        &row(
            new_id(),
            noon + Duration::hours(2),
            &place,
            used(500, 50),
            None,
            None,
            true,
        ),
    )
    .await
    .unwrap();
    // Trace: kept in the ledger, in no figure.
    record(
        &mut conn,
        &row(
            new_id(),
            noon,
            &place,
            used(9_000_000, 0),
            Some("99.000000"),
            Some("rate_card"),
            false,
        ),
    )
    .await
    .unwrap();

    let day = noon.date_naive();
    let filter = Filter {
        thread: Some(place.thread_id),
        ..Filter::default()
    };
    let cells = rollup::read(
        &mut conn,
        &Read {
            from: day,
            to: day,
            filter: &filter,
            bucket: None,
            by: None,
            under: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(cells.len(), 1);
    let figures = &cells[0].figures;
    assert_eq!(figures.turns, 1, "three requests of one turn are one turn");
    assert_eq!(figures.total_tokens, 1100 + 2000 + 550);
    assert_eq!(figures.input_tokens, 3500);
    assert_eq!(figures.credits.parse::<f64>().unwrap(), 0.0035);
    assert_eq!(figures.estimated_credits.parse::<f64>().unwrap(), 0.001);
    assert_eq!(
        figures.unrated_tokens, 550,
        "a row without a rate leaves its tokens out of the credits"
    );

    // One turn can cross midnight and change its model. Its requests remain two model groups,
    // but the span's turn count still agrees with distinct turn ids in the records.
    let mut later = row(
        new_id(),
        noon + Duration::days(1),
        &place,
        used(0, 0),
        None,
        None,
        true,
    );
    later.model = "another-model";
    record(&mut conn, &later).await.unwrap();
    let span = Read {
        from: day,
        to: day.succ_opt().unwrap(),
        filter: &filter,
        bucket: None,
        by: None,
        under: None,
    };
    assert_eq!(
        rollup::read(&mut conn, &span).await.unwrap()[0]
            .figures
            .turns,
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_span_read_by_the_hour_from_the_ledger_is_the_span_read_by_the_day_from_the_rollup() {
    let (_db, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    random_ledger(&mut conn, &Pool::new(), &mut Dice(31), 200).await;
    let (from, to) = (
        NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
        NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(),
    );
    let filter = Filter::default();
    let by_hour = rollup::read(
        &mut conn,
        &Read {
            from,
            to,
            filter: &filter,
            bucket: Some(UsageBucket::Hour),
            by: None,
            under: None,
        },
    )
    .await
    .unwrap();
    let by_day = rollup::read(
        &mut conn,
        &Read {
            from,
            to,
            filter: &filter,
            bucket: Some(UsageBucket::Day),
            by: None,
            under: None,
        },
    )
    .await
    .unwrap();
    let by_month = rollup::read(
        &mut conn,
        &Read {
            from,
            to,
            filter: &filter,
            bucket: Some(UsageBucket::Month),
            by: None,
            under: None,
        },
    )
    .await
    .unwrap();
    let total = |cells: &[rollup::Cell]| -> (i64, f64, i64) {
        cells.iter().fold((0, 0.0, 0), |sum, cell| {
            (
                sum.0 + cell.figures.total_tokens,
                sum.1 + cell.figures.credits.parse::<f64>().unwrap(),
                sum.2 + cell.figures.unrated_tokens,
            )
        })
    };
    let (hours, days, month) = (total(&by_hour), total(&by_day), total(&by_month));
    assert!(hours.0 > 0);
    assert_eq!(hours.0, days.0);
    assert_eq!(hours.2, days.2);
    assert!((hours.1 - days.1).abs() < 1e-6);
    assert_eq!(days.0, month.0);
    assert_eq!(by_month.len(), 1);
    assert_eq!(
        by_month[0].bucket.unwrap(),
        Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_breakdown_by_a_dimension_adds_up_to_the_total_and_names_nobody_for_use_put_down_to_nobody()
 {
    let (_db, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    random_ledger(&mut conn, &Pool::new(), &mut Dice(77), 200).await;
    let (from, to) = (
        NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
        NaiveDate::from_ymd_opt(2026, 12, 31).unwrap(),
    );
    let filter = Filter::default();
    let all = rollup::read(
        &mut conn,
        &Read {
            from,
            to,
            filter: &filter,
            bucket: None,
            by: None,
            under: None,
        },
    )
    .await
    .unwrap();
    let total = all[0].figures.total_tokens;
    for dimension in [
        UsageDimension::Person,
        UsageDimension::Model,
        UsageDimension::Provider,
        UsageDimension::Agent,
        UsageDimension::Environment,
        UsageDimension::Effort,
        UsageDimension::Tier,
        UsageDimension::Org,
        UsageDimension::Project,
        UsageDimension::Topic,
        UsageDimension::Thread,
    ] {
        let cells = rollup::read(
            &mut conn,
            &Read {
                from,
                to,
                filter: &filter,
                bucket: None,
                by: Some(dimension),
                under: None,
            },
        )
        .await
        .unwrap();
        let sum: i64 = cells.iter().map(|cell| cell.figures.total_tokens).sum();
        assert_eq!(sum, total, "{dimension:?}");
    }
    let people = rollup::read(
        &mut conn,
        &Read {
            from,
            to,
            filter: &filter,
            bucket: None,
            by: Some(UsageDimension::Person),
            under: None,
        },
    )
    .await
    .unwrap();
    assert!(
        people.iter().any(|cell| cell.key.is_none()),
        "some use was put down to nobody"
    );
}

/// A row of the ledger at a service tier.
fn at_tier<'a>(
    id: Id,
    at: DateTime<Utc>,
    place: &'a Attribution,
    tier: Option<&'a str>,
    tokens: i64,
) -> NewEvent<'a> {
    NewEvent {
        tier,
        ..row(id, at, place, used(tokens, 0), None, None, true)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn use_at_two_speeds_is_two_rows_in_both_rollups_and_the_figures_by_speed_are_the_ledgers() {
    let (_db, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let place = attribution(&Pool::new(), &mut Dice(3), new_id());
    let noon = Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap();
    for (tier, tokens) in [
        (None, 100),
        (Some("fast"), 20),
        (Some("fast"), 5),
        (None, 1),
    ] {
        record(&mut conn, &at_tier(new_id(), noon, &place, tier, tokens))
            .await
            .unwrap();
    }
    let tiers: Vec<(Option<String>, i64)> =
        sqlx::query_as("SELECT tier, total_tokens FROM usage_daily ORDER BY tier NULLS FIRST")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        tiers,
        [(None, 101), (Some("fast".to_owned()), 25)],
        "the ordinary speed and the fast one are told apart, each once"
    );
    let months: Vec<(Option<String>, i64)> =
        sqlx::query_as("SELECT tier, total_tokens FROM usage_monthly ORDER BY tier NULLS FIRST")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(months, tiers, "a month is its days, speed by speed");

    // Read by the speed, the days give what the ledger gives.
    let day = noon.date_naive();
    let filter = Filter::default();
    let from_days = rollup::read(
        &mut conn,
        &Read {
            from: day,
            to: day,
            filter: &filter,
            bucket: None,
            by: Some(UsageDimension::Tier),
            under: None,
        },
    )
    .await
    .unwrap();
    let mut by_tier: Vec<(Option<String>, i64)> = from_days
        .iter()
        .map(|cell| (cell.key.clone(), cell.figures.total_tokens))
        .collect();
    by_tier.sort();
    assert_eq!(
        by_tier,
        [(None, 101), (Some("fast".to_owned()), 25)]
            .into_iter()
            .collect::<Vec<_>>()
    );
    let in_the_ledger: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT tier, sum(total_tokens)::bigint FROM usage_events WHERE counted GROUP BY tier ORDER BY tier NULLS FIRST",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(in_the_ledger, [(None, 101), (Some("fast".to_owned()), 25)]);

    // Made again from the ledger, the rollups keep the speed.
    let before = both(&mut conn).await;
    let mut tx = db.begin().await.unwrap();
    rollup::rebuild(&mut tx, None).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(before, both(&mut conn).await);
}
