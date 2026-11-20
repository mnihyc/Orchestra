//! What a command that waits in its machine's line says it waits behind: the oldest other command of the
//! same machine and key that the machine has taken and not answered, and nothing for any other command.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::{Duration, Utc};
use orch_db::Db;
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::identity::testing::insert_user;
use orch_db::queries::lifecycle;
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
async fn private_command_operands_and_reply_identifiers_keep_their_exact_bytes_in_storage() {
    use orch_db::queries::link::{self, Ending, NewCommand};
    let (_fixture, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let machine = insert_machine(&mut conn, "private reply host", "connected")
        .await
        .unwrap();
    let id = new_id();
    let body = json!({"native_session_ref":"native\0id","text":"literal�value"});
    link::insert_command(
        &mut conn,
        &NewCommand {
            id,
            machine_id: machine,
            kind: "session.open",
            body: &body,
            serial_key: None,
            content_transit: false,
            deadline_at: Utc::now() + Duration::hours(1),
        },
        Utc::now(),
    )
    .await
    .unwrap();
    let reply =
        json!({"native_session_ref":"returned\0id","result":{"field\0":"one","field�":"two"}});
    link::finish_command(&mut conn, id, &Ending::Succeeded(&reply), Utc::now())
        .await
        .unwrap();
    let stored = link::command(&mut conn, id).await.unwrap().unwrap();
    assert_eq!(stored.body, body);
    assert_eq!(stored.result, Some(reply));
}

#[tokio::test]
async fn a_native_reply_keeps_every_original_member_until_its_public_projection() {
    let (_fixture, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let machine = insert_machine(&mut conn, "reply host", "connected")
        .await
        .unwrap();
    let id = command(&mut conn, machine, "fs.read", None, "accepted", 0).await;
    let answer = json!({"text":"A\0B","key\0":"retained","key�":"unchanged"});
    assert!(
        orch_db::queries::link::finish_command(
            &mut conn,
            id,
            &orch_db::queries::link::Ending::Succeeded(&answer),
            Utc::now()
        )
        .await
        .unwrap()
    );
    assert!(
        !orch_db::queries::link::finish_command(
            &mut conn,
            id,
            &orch_db::queries::link::Ending::Succeeded(&answer),
            Utc::now()
        )
        .await
        .unwrap()
    );
    let stored = orch_db::queries::link::command(&mut conn, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.result, Some(answer));
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
    let other_line = command(
        &mut conn,
        machine,
        "fs.record",
        Some("another-line"),
        "accepted",
        9,
    )
    .await;
    let other_machine = command(&mut conn, other, "fs.record", Some(&key), "accepted", 9).await;
    let finished = command(&mut conn, machine, "fs.record", Some(&key), "succeeded", 9).await;
    let unkeyed = command(&mut conn, machine, "fs.stat", None, "accepted", 1).await;
    let unsent = command(&mut conn, machine, "fs.stat", Some(&key), "queued", 0).await;

    // Acceptance alone means queued at the peer; its next receipt reports actual execution.
    sqlx::query("UPDATE commands SET key_phase=1 WHERE id=ANY($1)")
        .bind(vec![record, other_line, other_machine, finished, unkeyed])
        .execute(&mut *conn)
        .await
        .unwrap();

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

#[tokio::test]
async fn lifecycle_cleanup_keeps_the_original_dependency_and_native_work_receipt() {
    let (_fixture, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let owner = new_id();
    insert_user(&mut conn, owner, "cleanup-owner")
        .await
        .unwrap();
    let home = new_id();
    insert_node(&mut conn, home, "org", None, "Cleanup home", owner)
        .await
        .unwrap();
    let machine = insert_machine(&mut conn, "Cleanup machine", "connected")
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET owner_id=$2 WHERE id=$1")
        .bind(machine)
        .bind(owner)
        .execute(&mut *conn)
        .await
        .unwrap();
    let environment = new_id();
    sqlx::query(
        "INSERT INTO environments(id,name,owner_id,org_id,home_node_id,placement,machine_id)
         VALUES($1,'Withdrawn directory',$2,$3,$3,'connected',$4)",
    )
    .bind(environment)
    .bind(owner)
    .bind(home)
    .bind(machine)
    .execute(&mut *conn)
    .await
    .unwrap();
    let key = environment.to_string();
    let parent = command(&mut conn, machine, "machine.probe", None, "succeeded", 5).await;
    let close = command(
        &mut conn,
        machine,
        "session.close",
        Some(&key),
        "accepted",
        3,
    )
    .await;
    sqlx::query("UPDATE commands SET after_command_id=$2,work_started_at=created_at,key_phase=1 WHERE id=$1")
        .bind(close).bind(parent).execute(&mut *conn).await.unwrap();
    let original = orch_db::queries::link::command(&mut conn, close)
        .await
        .unwrap()
        .unwrap();
    let controls = lifecycle::controls(&mut conn, 10).await.unwrap();
    assert_eq!(controls, vec![original]);

    let detach = command(&mut conn, machine, "env.detach", Some(&key), "failed", 2).await;
    sqlx::query("UPDATE commands SET body=$2,error=$3,after_command_id=$4,work_started_at=created_at,key_phase=2 WHERE id=$1")
        .bind(detach).bind(json!({"environment_id":environment}))
        .bind(json!({"code":"busy","detail":"publication is pending","retryable":true}))
        .bind(parent).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE environments SET deleted_at=now() WHERE id=$1")
        .bind(environment)
        .execute(&mut *conn)
        .await
        .unwrap();
    let original = orch_db::queries::link::command(&mut conn, detach)
        .await
        .unwrap()
        .unwrap();
    let retries =
        lifecycle::busy_registration_detaches(&mut conn, original.updated_at, Utc::now(), 10)
            .await
            .unwrap();
    assert_eq!(retries, vec![original]);
}

#[tokio::test]
async fn reconnect_only_replays_a_turn_start_after_its_dispatcher_has_adopted_it() {
    use orch_db::queries::link;
    use orch_types::Clock as _;
    let (_fixture, db) = world().await;
    let clock = orch_testkit::clock::manual();
    let mut conn = db.acquire().await.unwrap();
    let machine = insert_machine(&mut conn, "Dispatch host", "connected")
        .await
        .unwrap();
    let expired = command(&mut conn, machine, "turn.start", None, "queued", 0).await;
    let accepted = command(&mut conn, machine, "turn.start", None, "queued", 0).await;
    for id in [expired, accepted] {
        link::set_start_adoption_deadline(&mut conn, id, clock.now() + Duration::seconds(2))
            .await
            .unwrap();
    }
    assert!(
        link::open_commands(&mut conn, machine)
            .await
            .unwrap()
            .is_empty()
    );
    {
        let mut tx = db.begin().await.unwrap();
        assert!(link::adopt_start(&mut tx, accepted, &clock).await.unwrap());
        tx.commit().await.unwrap();
    }
    clock.advance(Duration::seconds(3));
    let mut tx = db.begin().await.unwrap();
    assert!(!link::adopt_start(&mut tx, expired, &clock).await.unwrap());
    assert!(link::adopt_start(&mut tx, accepted, &clock).await.unwrap());
    tx.commit().await.unwrap();
    let open = link::open_commands(&mut conn, machine).await.unwrap();
    assert_eq!(
        open.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![accepted]
    );
}
