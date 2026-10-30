//! A deleted provider's local-key purge survives an offline keeper and is retried once when it returns.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::{DateTime, Duration, Utc};
use orch_db::queries::{lifecycle, link};
use orch_db::{Conn, Db};
use orch_testkit::database::TestDb;
use orch_types::{Id, new_id};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;

async fn journal_attempt(
    db: &Db,
    previous: &link::CommandRow,
    next: Id,
    now: DateTime<Utc>,
) -> bool {
    let mut tx = db.begin().await.unwrap();
    let claimed = lifecycle::claim_provider_purge_retry(&mut tx, previous.id, next)
        .await
        .unwrap();
    if claimed {
        let seconds =
            i64::from(orch_link_contract::commands::CommandKind::ProviderStop.deadline_secs());
        assert!(
            link::insert_command(
                &mut tx,
                &link::NewCommand {
                    id: next,
                    machine_id: previous.machine_id,
                    kind: &previous.kind,
                    body: &previous.body,
                    serial_key: previous.serial_key.as_deref(),
                    deadline_at: now + Duration::seconds(seconds),
                },
                now
            )
            .await
            .unwrap()
        );
    }
    tx.commit().await.unwrap();
    claimed
}

async fn old_stop(
    conn: &mut Conn,
    machine: Id,
    provider: Id,
    purge: bool,
    state: &str,
    at: DateTime<Utc>,
) -> Id {
    let id = new_id();
    sqlx::query("INSERT INTO commands(id,machine_id,kind,body,serial_key,state,deadline_at,created_at,updated_at)
                 VALUES($1,$2,'provider.stop',$3,$4,$5,$6,$6,$6)")
        .bind(id)
        .bind(machine)
        .bind(if purge { json!({"provider_id":provider,"purge":true}) } else { json!({"provider_id":provider}) })
        .bind(provider.to_string())
        .bind(state)
        .bind(at)
        .execute(conn)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn an_unconfirmed_purge_survives_retention_and_has_one_successor_after_reconnection() {
    let fixture = TestDb::fresh().await;
    let db = Db::from_pool(
        PgPoolOptions::new()
            .max_connections(4)
            .connect(&fixture.url)
            .await
            .unwrap(),
    );
    let now = Utc::now();
    let old = now - Duration::minutes(5);
    let mut conn = db.acquire().await.unwrap();
    let machine =
        orch_db::queries::resources::testing::insert_machine(&mut conn, "Keeper", "connected")
            .await
            .unwrap();
    let provider = new_id();
    let pending = old_stop(&mut conn, machine, provider, true, "expired", old).await;
    let ordinary = old_stop(&mut conn, machine, provider, false, "expired", old).await;
    let completed = old_stop(&mut conn, machine, provider, true, "succeeded", old).await;
    link::purge_finished(&mut conn, now - Duration::minutes(1), 100)
        .await
        .unwrap();
    assert!(link::command(&mut conn, pending).await.unwrap().is_some());
    assert!(link::command(&mut conn, ordinary).await.unwrap().is_none());
    assert!(link::command(&mut conn, completed).await.unwrap().is_none());
    assert!(
        lifecycle::provider_purges_due(&mut conn, now, 10)
            .await
            .unwrap()
            .is_empty(),
        "the keeper is offline"
    );
    sqlx::query("UPDATE machines SET online=true WHERE id=$1")
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    let due = lifecycle::provider_purges_due(&mut conn, now, 10)
        .await
        .unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].id, pending);
    drop(conn);

    let mut rolled_back = db.begin().await.unwrap();
    assert!(
        lifecycle::claim_provider_purge_retry(&mut rolled_back, pending, new_id())
            .await
            .unwrap()
    );
    rolled_back.rollback().await.unwrap();
    let mut conn = db.acquire().await.unwrap();
    assert_eq!(
        lifecycle::provider_purges_due(&mut conn, now, 10)
            .await
            .unwrap()
            .len(),
        1,
        "an interrupted transaction leaves the purge eligible"
    );
    drop(conn);

    let (first, second) = (new_id(), new_id());
    let (a, b) = tokio::join!(
        journal_attempt(&db, &due[0], first, now),
        journal_attempt(&db, &due[0], second, now)
    );
    assert_ne!(a, b, "concurrent reapers admit one successor");
    let winner = if a { first } else { second };
    let mut conn = db.acquire().await.unwrap();
    let retry = link::command(&mut conn, winner).await.unwrap().unwrap();
    assert_eq!(retry.machine_id, machine);
    assert_eq!(retry.kind, "provider.stop");
    assert_eq!(retry.body, json!({"provider_id":provider,"purge":true}));
    let expected_key = provider.to_string();
    assert_eq!(retry.serial_key.as_deref(), Some(expected_key.as_str()));
    assert_eq!(retry.state, "queued");
    assert!(
        lifecycle::provider_purges_due(&mut conn, now, 10)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("UPDATE commands SET state='failed',error=$2,updated_at=$3 WHERE id=$1")
        .bind(winner)
        .bind(json!({"code":"internal","detail":"the store could not be used"}))
        .bind(old)
        .execute(&mut *conn)
        .await
        .unwrap();
    let due_again = lifecycle::provider_purges_due(&mut conn, now, 10)
        .await
        .unwrap();
    assert_eq!(due_again.len(), 1);
    assert_eq!(due_again[0].id, winner);
    drop(conn);
    assert!(
        journal_attempt(&db, &due_again[0], new_id(), now).await,
        "a failed retry can be retried again"
    );
    let mut conn = db.acquire().await.unwrap();
    let failed = link::command(&mut conn, winner).await.unwrap().unwrap();
    assert_eq!(failed.error.unwrap()["code"], "internal");
}
