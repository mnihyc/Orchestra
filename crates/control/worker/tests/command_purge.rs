//! Commands to machines that ended a day ago are removed at each run, and what is still owed an answer or read back stays.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::{DateTime, Duration, Utc};
use orch_db::Db;
use orch_db::queries::link::{self, Ending, NewCommand};
use orch_db::queries::resources::testing::insert_machine;
use orch_testkit::{clock, database::TestDb};
use orch_types::{Clock, Id, Json, new_id};
use orch_worker_core::command_purge::{self, KEPT_FOR};
use serde_json::json;

/// Stores a command of `kind` made at `at` and, where an ending is given, ended at that same moment.
async fn command(
    db: &Db,
    machine: Id,
    kind: &str,
    body: Json,
    at: DateTime<Utc>,
    ending: Option<Ending<'_>>,
) -> Id {
    let mut conn = db.acquire().await.unwrap();
    let id = new_id();
    let made = NewCommand {
        id,
        machine_id: machine,
        kind,
        body: &body,
        serial_key: None,
        deadline_at: at + Duration::minutes(1),
    };
    link::insert_command(&mut conn, &made, at).await.unwrap();
    if let Some(ending) = ending {
        link::finish_command(&mut conn, id, &ending, at)
            .await
            .unwrap();
    }
    id
}

async fn kept(db: &Db, id: Id) -> bool {
    let mut conn = db.acquire().await.unwrap();
    link::command(&mut conn, id).await.unwrap().is_some()
}

#[tokio::test]
async fn commands_that_ended_a_day_ago_go_and_younger_open_recorded_and_running_jobs_commands_stay()
{
    let test = TestDb::fresh().await;
    let db = Db::from_pool(test.pool.clone());
    let clock = clock::manual();
    let now = clock.now();
    let machine = {
        let mut conn = db.acquire().await.unwrap();
        insert_machine(&mut conn, "laptop", "connected")
            .await
            .unwrap()
    };
    let listing = json!({ "entries": [], "truncated": false });
    let nothing = json!({});
    let refused =
        json!({ "code": "internal", "detail": "the harness stopped", "retryable": false });
    let over_a_day = now - KEPT_FOR - Duration::minutes(1);
    let under_a_day = now - KEPT_FOR + Duration::minutes(1);

    let listed = command(
        &db,
        machine,
        "fs.list",
        json!({}),
        over_a_day,
        Some(Ending::Succeeded(&listing)),
    )
    .await;
    let failed = command(
        &db,
        machine,
        "turn.start",
        json!({}),
        over_a_day,
        Some(Ending::Failed(&refused)),
    )
    .await;
    let lapsed = command(
        &db,
        machine,
        "fs.stat",
        json!({}),
        over_a_day,
        Some(Ending::Expired),
    )
    .await;
    let young = command(
        &db,
        machine,
        "fs.stat",
        json!({}),
        under_a_day,
        Some(Ending::Succeeded(&listing)),
    )
    .await;
    // A machine wrote this one down three days ago and still owes its answer.
    let owed = command(
        &db,
        machine,
        "exec.start",
        json!({}),
        now - Duration::days(3),
        None,
    )
    .await;
    {
        let mut conn = db.acquire().await.unwrap();
        assert!(
            link::mark_accepted(&mut conn, owed, now - Duration::days(3))
                .await
                .unwrap()
        );
    }
    // What a unit was made with is read back for as long as the unit is there.
    let made = json!({ "runtime_id": new_id() });
    let record = command(
        &db,
        machine,
        "node.runtime.create",
        made,
        over_a_day,
        Some(Ending::Succeeded(&nothing)),
    )
    .await;
    // A job that resumes reads again how its commands ended.
    let job = new_id();
    sqlx::query("INSERT INTO jobs (id, kind, state) VALUES ($1, 'snapshot', 'running')")
        .bind(job)
        .execute(&test.pool)
        .await
        .unwrap();
    let of_job = command(
        &db,
        machine,
        "fs.snapshot",
        json!({ "job_id": job }),
        over_a_day,
        Some(Ending::Succeeded(&nothing)),
    )
    .await;

    assert_eq!(command_purge::purge(&db, &clock).await.unwrap(), Some(3));
    for gone in [listed, failed, lapsed] {
        assert!(
            !kept(&db, gone).await,
            "a command that ended over a day ago is removed"
        );
    }
    assert!(
        kept(&db, young).await,
        "one that ended less than a day ago stays"
    );
    assert!(
        kept(&db, owed).await,
        "one that is not over stays, however old"
    );
    assert!(kept(&db, record).await, "what a unit was made with stays");
    assert!(
        kept(&db, of_job).await,
        "a command of a job that is not over stays"
    );

    // Once the job is over its commands go, and a day after its end so does the younger one.
    sqlx::query("UPDATE jobs SET state = 'succeeded' WHERE id = $1")
        .bind(job)
        .execute(&test.pool)
        .await
        .unwrap();
    clock.advance(Duration::minutes(2));
    assert_eq!(command_purge::purge(&db, &clock).await.unwrap(), Some(2));
    assert!(!kept(&db, of_job).await);
    assert!(!kept(&db, young).await);
    assert!(kept(&db, owed).await && kept(&db, record).await);
    assert_eq!(command_purge::purge(&db, &clock).await.unwrap(), Some(0));
}

#[tokio::test]
async fn a_run_finds_the_job_taken_while_another_holds_it_and_holds_no_transaction_open() {
    let test = TestDb::fresh().await;
    let db = Db::from_pool(test.pool.clone());
    let clock = clock::manual();
    let other = orch_db::queries::job_leases::Lease::new(
        "command_purge",
        "another worker",
        Duration::minutes(10),
    );
    assert!(
        other.hold(&db, clock.now()).await.unwrap(),
        "the other run takes the lease"
    );
    assert_eq!(command_purge::purge(&db, &clock).await.unwrap(), None);
    other.release(&db).await.unwrap();
    assert_eq!(command_purge::purge(&db, &clock).await.unwrap(), Some(0));
    let open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity
          WHERE datname = current_database() AND pid <> pg_backend_pid() AND state = 'idle in transaction'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(open, 0);
    assert!(
        other.hold(&db, clock.now()).await.unwrap(),
        "the run gave its lease up at its end"
    );
}
