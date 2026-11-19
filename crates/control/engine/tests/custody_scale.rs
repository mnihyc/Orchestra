//! Resource counts stay bounded by current threads over million-row queues and retained histories.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;

use chrono::Timelike;
use common::World;
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_api_contract::resource_facts::FactTarget;
use orch_db::usage::rollup::{self, ResourceTurnDimension};
use orch_engine::resource_facts::activity;
use orch_types::{Json, Time};

// The acceptance world has one thousand threads, each with one thousand old and queued messages.
const THREADS: i64 = 1_000;
const TURNS_PER_THREAD: i64 = 1_000;
// One hundred million reported tokens per thread tests large counters without allocating model text.
const TOKENS_PER_TURN: i64 = 100_000;

fn assert_indexed_large_tables(node: &Json) {
    if let Some(table) = node["Relation Name"].as_str()
        && ["thread_inputs", "turns", "items"].contains(&table)
    {
        assert_ne!(
            node["Node Type"], "Seq Scan",
            "large-table activity access needs its selective/covering index: {node}"
        );
    }
    if let Some(children) = node["Plans"].as_array() {
        for child in children {
            assert_indexed_large_tables(child);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_million_waiting_messages_and_old_items_use_one_work_membership_per_current_thread() {
    let world = World::new().await;
    world.engine().stop();
    // PostgreSQL stores microseconds; a whole-second fixture keeps exact boundary assertions stable.
    let at = world.clock.now().with_nanosecond(0).unwrap();
    // Each transaction creates one thousand turns and usage rows. Native
    // commit journals stay bounded without disabling any trigger or constraint.
    for thread_number in 1..=THREADS {
        let mut tx = world.pool.begin().await.unwrap();
        sqlx::query("CREATE TEMP TABLE custody_scale_seed ON COMMIT DROP AS SELECT $1::uuid AS topic,$2::uuid AS person,$3::uuid AS provider,$4::uuid AS agent,$5::uuid AS environment,$6::timestamptz AS at,$7::bigint AS threads,$8::bigint AS turns_per_thread,$9::bigint AS tokens_per_turn,$10::bigint AS thread_number")
            .bind(world.topic).bind(world.ada).bind(world.provider).bind(world.agent).bind(world.environment).bind(at).bind(THREADS).bind(TURNS_PER_THREAD).bind(TOKENS_PER_TURN).bind(thread_number)
            .execute(&mut *tx).await.unwrap();
        sqlx::raw_sql(include_str!("custody_scale.sql"))
            .execute(&mut *tx)
            .await
            .unwrap();
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM usage_commit_rows WHERE xact=pg_current_xact_id()",
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(pending, TURNS_PER_THREAD);
        tx.commit().await.unwrap();
    }
    let mut conn = world.pool.acquire().await.unwrap();
    // Visibility maps let the planner measure the covering paths, rather than fresh insert churn.
    for sql in [
        "VACUUM (ANALYZE) thread_inputs",
        "VACUUM (ANALYZE) turns",
        "VACUUM (ANALYZE) items",
        "VACUUM (ANALYZE) usage_events",
        "ANALYZE threads",
        "ANALYZE segments",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let host = orch_types::new_id();
    sqlx::query("INSERT INTO hosts(id,machine_id,name,owner_id,home_id,org_id) VALUES($1,$2,'Scale host',$3,$4,$4)")
        .bind(host).bind(world.machine_id).bind(world.ada).bind(world.root).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE environments SET machine_id=$2 WHERE id=$1")
        .bind(world.environment)
        .bind(world.machine_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    let waiting_host = orch_engine::resource_facts::host_waiting(&mut conn, host, Time::new(at))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            waiting_host.environments,
            waiting_host.kept,
            waiting_host.runtimes
        ),
        (1, 0, 0)
    );
    assert!(waiting_host.until.is_none());
    assert_eq!(waiting_host.name, "Scale host");
    let target = FactTarget {
        object: ObjectRef {
            kind: ObjectKind::Provider,
            id: world.provider,
        },
        model_id: None,
    };
    let work = activity::read(&mut conn, &target).await.unwrap();
    let waiting = work
        .iter()
        .filter(|row| row.kind == "waiting")
        .collect::<Vec<_>>();
    assert_eq!(
        waiting.len() as i64,
        THREADS,
        "a million pending messages must not become a million client rows"
    );
    assert_eq!(
        waiting.iter().map(|row| row.count).sum::<i64>(),
        THREADS * TURNS_PER_THREAD
    );
    assert!(
        waiting
            .iter()
            .all(|row| row.count == TURNS_PER_THREAD && row.since == at)
    );
    let stopped = work
        .iter()
        .filter(|row| row.kind == "stopped")
        .collect::<Vec<_>>();
    assert_eq!(
        stopped.len() as i64,
        THREADS,
        "only each active continuation's latest stopped task is relevant"
    );
    assert!(stopped.iter().all(|row| row.count == 1));
    assert_eq!(
        work.len() as i64,
        2 * THREADS,
        "historical items and every queued word stay in PostgreSQL"
    );
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(world.ada),
        Time::new(at),
    )
    .await
    .unwrap();
    let facts = activity::project(&mut conn, &principal, &target, &work, Time::new(at))
        .await
        .unwrap();
    assert_eq!(
        (facts.waiting.count, facts.waiting.threads),
        (THREADS * TURNS_PER_THREAD, THREADS)
    );
    assert_eq!(
        (facts.stopped.count, facts.stopped.threads),
        (THREADS, THREADS)
    );
    for (id, dimension) in [
        (world.provider, ResourceTurnDimension::Provider),
        (world.agent, ResourceTurnDimension::Agent),
        (world.environment, ResourceTurnDimension::Environment),
    ] {
        let counted = rollup::resource_turn_counts(&mut conn, &[id], dimension, at)
            .await
            .unwrap();
        assert_eq!(
            counted.len(),
            1,
            "one SQL aggregate, not every ledger/turn row"
        );
        assert_eq!(
            (counted[0].all_time, counted[0].last_30_days),
            (THREADS * TURNS_PER_THREAD, THREADS * TURNS_PER_THREAD)
        );
        assert_eq!(
            (counted[0].people, counted[0].people_30_days),
            (Some(1), Some(1))
        );
        assert_eq!(counted[0].last_at, Some(at));
    }
    let tokens: i64 = sqlx::query_scalar(
        "SELECT sum(total_tokens)::bigint FROM usage_events WHERE provider_id=$1",
    )
    .bind(world.provider)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(tokens, THREADS * TURNS_PER_THREAD * TOKENS_PER_TURN);
    assert!(tokens > 10_000_000_000);
    let queued: i64 =
        sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE state='pending'")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        queued,
        THREADS * TURNS_PER_THREAD,
        "counting consumes no queued input"
    );
    let sql = format!(
        "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) {}",
        include_str!("../src/resource_facts/activity.sql")
    );
    let explain: Json = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind("provider")
        .bind(world.provider)
        .bind(None::<orch_types::Id>)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let plan = &explain[0]["Plan"];
    assert_eq!(plan["Actual Rows"].as_i64(), Some(2 * THREADS));
    assert_indexed_large_tables(plan);
    eprintln!(
        "custody scale: threads={THREADS}; queued={queued}; reported_tokens={tokens}; held_work_rows={}; plan={explain}",
        work.len()
    );
}
