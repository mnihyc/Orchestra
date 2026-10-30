//! Which definition a unit was made with is asked of the commands by the unit's id. The commands of a system are never
//! few, so the question is answered from an index: with a table of a million commands the newest creation of
//! one unit is read through the index and nothing else of the table.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_db::queries::{jobs, resources};
use orch_testkit::database::TestDb;
use orch_types::new_id;

/// How many commands the table holds besides the unit's own: made by one statement, of three kinds and in the shapes a
/// real system has (a creation carries the spec, a removal and a resource change the unit's id only).
const OTHERS: i64 = 1_000_000;

#[tokio::test]
async fn the_command_that_made_a_unit_is_found_through_an_index_among_a_million_commands() {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    let machine = resources::testing::insert_machine(&mut conn, "host", "connected")
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO commands (id, machine_id, kind, body, state, deadline_at, created_at)
         SELECT gen_random_uuid(), $1,
                CASE n % 3 WHEN 0 THEN 'node.runtime.create' WHEN 1 THEN 'node.resources.apply' ELSE 'node.runtime.remove' END,
                jsonb_build_object('runtime_id', gen_random_uuid()::text, 'spec', jsonb_build_object('runtime', 'runc', 'image', 'orch-env-base:1')),
                'succeeded', now() + interval '1 hour', now() - make_interval(secs => n)
         FROM generate_series(1, $2::bigint) AS n",
    )
    .bind(machine)
    .bind(OTHERS)
    .execute(&mut *conn)
    .await
    .unwrap();
    let unit = new_id();
    // Two creations of the unit (it was made again), the newer naming another runtime.
    for (secs, runtime) in [(5_i32, "runc"), (1_i32, "kata")] {
        sqlx::query(
            "INSERT INTO commands (id, machine_id, kind, body, state, deadline_at, created_at)
             VALUES ($1, $2, 'node.runtime.create', jsonb_build_object('runtime_id', $3::text, 'spec', jsonb_build_object('runtime', $4::text)),
                     'succeeded', now() + interval '1 hour', now() - make_interval(secs => $5))",
        )
        .bind(new_id())
        .bind(machine)
        .bind(unit)
        .bind(runtime)
        .bind(f64::from(secs))
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    sqlx::query("ANALYZE commands")
        .execute(&mut *conn)
        .await
        .unwrap();

    let definition = jobs::changes::definition(&mut conn, unit)
        .await
        .unwrap()
        .expect("the unit was made");
    assert_eq!(
        definition["spec"]["runtime"], "kata",
        "the newest creation answers"
    );

    // The question the engine asks, shown to the planner: the index and no pass over the table.
    let plan: Vec<String> = sqlx::query_scalar(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT TEXT)
         SELECT body #>> '{spec,runtime}' FROM commands
          WHERE kind = 'node.runtime.create' AND body ->> 'runtime_id' = $1
            AND state IN ('queued','sent','accepted','succeeded')
          ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(unit.to_string())
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let plan = plan.join("\n");
    assert!(plan.contains("commands_runtime_create_idx"), "{plan}");
    assert!(!plan.contains("Seq Scan"), "{plan}");
    // Rows the index handed over: the unit's own two, not a share of the table.
    let handed: i64 = plan
        .lines()
        .find(|line| line.contains("commands_runtime_create_idx"))
        .and_then(|line| line.split("rows=").nth(2))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|rows| rows.parse().ok())
        .unwrap_or(i64::MAX);
    assert!(handed <= 2, "{plan}");
}
