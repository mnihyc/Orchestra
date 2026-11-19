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
async fn private_command_operands_and_reply_identifiers_keep_their_exact_bytes_in_storage() {
    use orch_db::queries::link::{self, Ending, NewCommand};
    let (_fixture, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let machine = insert_machine(&mut conn, "private reply host", "connected")
        .await
        .unwrap();
    use orch_db::queries::{
        hierarchy::testing::insert_node,
        resources::{access_fixtures::insert_resource, testing::insert_user},
    };
    let owner = insert_user(&mut conn, "operator").await.unwrap();
    let (org, project, topic, thread, segment) = (new_id(), new_id(), new_id(), new_id(), new_id());
    for (id, kind, parent) in [
        (org, "org", None),
        (project, "project", Some(org)),
        (topic, "topic", Some(project)),
        (thread, "thread", Some(topic)),
    ] {
        insert_node(&mut conn, id, kind, parent, kind, owner)
            .await
            .unwrap();
    }
    let agent = insert_resource(&mut conn, "agent", Some(owner), Some(topic), "Agent")
        .await
        .unwrap();
    let provider = insert_resource(&mut conn, "provider", Some(owner), Some(topic), "Provider")
        .await
        .unwrap();
    let environment = orch_db::queries::hierarchy::testing::put_thread_in_environment(
        &mut conn,
        thread,
        "connected",
        owner,
        topic,
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO segments(id,thread_id,continuation_id,ordinal,agent_id,provider_id,environment_id,machine_id,env_generation,arrangement,credential_path,seq_updated)
        SELECT $2,node_id,active_continuation_id,1,$3,$4,$5,$6,1,'together','native',0 FROM threads WHERE node_id=$1")
        .bind(thread).bind(segment).bind(agent).bind(provider).bind(environment).bind(machine)
        .execute(&mut *conn).await.unwrap();
    let id = new_id();
    let body = json!({"thread_id":thread,"segment_id":segment,"native_session_ref":"native\0id","text":"literal�value"});
    link::insert_command(
        &mut conn,
        &NewCommand {
            native_target: None,
            id,
            machine_id: machine,
            kind: "session.open",
            body: &body,
            serial_key: None,
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
