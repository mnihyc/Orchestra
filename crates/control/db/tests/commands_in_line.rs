//! What a command that waits in its machine's line says it waits behind: the oldest other command of the
//! same machine and key that the machine has taken and not answered, and nothing for any other command.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::{Duration, Utc};
use orch_db::Db;
use orch_db::queries::link::held_up_by;
use orch_db::queries::resources::testing::insert_machine;
use orch_testkit::database::TestDb;
use orch_types::{Id, new_id};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;

async fn world() -> (TestDb, Db) {
    let db = TestDb::fresh().await;
    let pool = Db::from_pool(
        PgPoolOptions::new()
            .max_connections(2)
            .connect(&db.url)
            .await
            .unwrap(),
    );
    (db, pool)
}

async fn command(
    conn: &mut orch_db::Conn,
    machine: Id,
    kind: &str,
    key: Option<&str>,
    state: &str,
    minutes_ago: i64,
) -> Id {
    let id = new_id();
    let at = Utc::now() - Duration::minutes(minutes_ago);
    sqlx::query(
        "INSERT INTO commands (id, machine_id, kind, body, serial_key, state, deadline_at, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(machine)
    .bind(kind)
    .bind(json!({}))
    .bind(key)
    .bind(state)
    .bind(at + Duration::hours(6))
    .bind(at)
    .execute(&mut *conn)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn a_command_in_line_names_the_oldest_command_before_it_that_its_machine_has_taken() {
    let (_fixture, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let machine = insert_machine(&mut conn, "laptop", "connected")
        .await
        .unwrap();
    let other = insert_machine(&mut conn, "desktop", "connected")
        .await
        .unwrap();
    let key = new_id().to_string();
    let record = command(&mut conn, machine, "fs.record", Some(&key), "accepted", 5).await;
    let clone = command(&mut conn, machine, "git.prepare", Some(&key), "accepted", 3).await;
    let made = command(&mut conn, machine, "fs.mkdir", Some(&key), "accepted", 1).await;
    // Other lines, other machines, and what is over, hold nothing up.
    command(
        &mut conn,
        machine,
        "fs.record",
        Some("another-line"),
        "accepted",
        9,
    )
    .await;
    command(&mut conn, other, "fs.record", Some(&key), "accepted", 9).await;
    command(&mut conn, machine, "fs.record", Some(&key), "succeeded", 9).await;
    let unkeyed = command(&mut conn, machine, "fs.stat", None, "accepted", 1).await;
    let unsent = command(&mut conn, machine, "fs.stat", Some(&key), "queued", 0).await;

    let (kind, _) = held_up_by(&mut conn, made).await.unwrap().unwrap();
    assert_eq!(
        kind, "fs.record",
        "the oldest one in the line is the one that holds it up"
    );
    let (kind, _) = held_up_by(&mut conn, clone).await.unwrap().unwrap();
    assert_eq!(kind, "fs.record");
    assert_eq!(
        held_up_by(&mut conn, record).await.unwrap(),
        None,
        "the first in line waits for nothing"
    );
    assert_eq!(held_up_by(&mut conn, unkeyed).await.unwrap(), None);
    assert_eq!(
        held_up_by(&mut conn, unsent).await.unwrap(),
        None,
        "a command its machine has not taken waits for the machine, not for other work"
    );
}
