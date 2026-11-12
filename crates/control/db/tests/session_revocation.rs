//! The revoke producer writes exactly its materialized subjects, including retained expired rows.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use chrono::Duration;
use orch_db::queries::identity::{self, testing};
use orch_testkit::{clock, database::TestDb};
use orch_types::{Id, new_id};

#[tokio::test]
async fn a_materialized_revoke_set_keeps_new_unselected_sessions_and_their_tokens() {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    let user = new_id();
    testing::insert_user(&mut conn, user, "owner")
        .await
        .unwrap();
    let now = clock::start();
    let first = new_id();
    let expired = new_id();
    testing::insert_session(
        &mut conn,
        first,
        user,
        first.as_bytes(),
        now + Duration::days(1),
    )
    .await
    .unwrap();
    testing::insert_session(
        &mut conn,
        expired,
        user,
        expired.as_bytes(),
        now - Duration::seconds(1),
    )
    .await
    .unwrap();
    let subjects = identity::unrevoked_session_subjects(&mut conn, user)
        .await
        .unwrap();
    assert_eq!(subjects.len(), 2);
    assert!(subjects.iter().all(|(_, owner)| *owner == user));
    let selected: Vec<_> = subjects.into_iter().map(|(id, _)| id).collect();
    let newcomer = new_id();
    testing::insert_session(
        &mut conn,
        newcomer,
        user,
        newcomer.as_bytes(),
        now + Duration::days(1),
    )
    .await
    .unwrap();
    assert_eq!(
        identity::revoke_session_set(&mut conn, user, &selected, now)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        identity::unrevoked_session_subjects(&mut conn, user)
            .await
            .unwrap(),
        [(newcomer, user)]
    );
    let active: Vec<Id> = sqlx::query_scalar(
        "SELECT session_id FROM access_tokens WHERE session_id=ANY($1) ORDER BY session_id",
    )
    .bind([first, expired, newcomer].as_slice())
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(active, [newcomer]);
    assert_eq!(
        identity::revoke_session_set(&mut conn, user, &[], now)
            .await
            .unwrap(),
        0,
        "empty materialized collections must never become a broad delete"
    );
    assert_eq!(
        identity::unrevoked_session_subjects(&mut conn, user)
            .await
            .unwrap(),
        [(newcomer, user)]
    );
    drop(conn);
    db.close().await.unwrap();
}
