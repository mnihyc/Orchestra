//! Recent key metadata has exact concurrent count/age bounds and stable self-read pages.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use chrono::{DateTime, Duration, Utc};
use orch_db::Conn;
use orch_db::queries::{identity::testing, key_activity};
use orch_testkit::{clock, database::TestDb};
use orch_types::{Id, new_id};
use serde_json::json;

async fn key(conn: &mut Conn) -> (Id, Id) {
    let user = new_id();
    testing::insert_user(conn, user, "reader").await.unwrap();
    let token = testing::insert_api_token(conn, user, user.as_bytes())
        .await
        .unwrap();
    (token, user)
}

async fn append(
    conn: &mut Conn,
    token: Id,
    account: Id,
    id: Id,
    at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> bool {
    let facets = json!([]);
    key_activity::append(
        conn,
        &key_activity::NewCall {
            id,
            token,
            account,
            at,
            operation: "NodeGet",
            selection: Some("NodeGet"),
            facets: &facets,
            orgs: &[],
            method: "GET",
            route: "/nodes/{id}",
            status: 200,
            error: None,
            replayed: false,
            duration_ms: 3,
            request_id: "request",
            ip: None,
        },
        now,
    )
    .await
    .unwrap()
}

async fn rows(conn: &mut Conn, token: Id, now: DateTime<Utc>) -> Vec<key_activity::Row> {
    let snapshot = key_activity::snapshot(conn, token).await.unwrap();
    key_activity::page(conn, token, None, now, snapshot, None, 101)
        .await
        .unwrap()
}

#[tokio::test]
async fn recent_history_uses_server_now_for_delayed_calls_and_checks_the_real_key_owner() {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    let (token, owner) = key(&mut conn).await;
    let now = clock::start();
    let old = new_id();
    assert!(append(&mut conn, token, owner, old, now - Duration::days(8), now).await);
    let retained: i64 =
        sqlx::query_scalar("SELECT count(*) FROM api_token_activity WHERE token_id=$1")
            .bind(token)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        retained, 0,
        "a long-running call cannot move the retention clock back to its start"
    );
    let boundary = new_id();
    assert!(
        append(
            &mut conn,
            token,
            owner,
            boundary,
            now - Duration::days(7),
            now,
        )
        .await
    );
    assert_eq!(
        rows(&mut conn, token, now).await[0].id,
        boundary,
        "the exact seven-day boundary is retained until it becomes older"
    );
    for n in 0..101 {
        assert!(
            append(
                &mut conn,
                token,
                owner,
                new_id(),
                now + Duration::seconds(n),
                now + Duration::seconds(101)
            )
            .await
        );
    }
    let found = rows(&mut conn, token, now + Duration::seconds(101)).await;
    assert_eq!(found.len(), 100);
    assert!(found.iter().all(|row| row.at > now));
    assert!(
        !append(&mut conn, token, new_id(), new_id(), now, now).await,
        "an unproven key/account pair cannot attribute a call"
    );
    assert!(key_activity::owned(&mut conn, token, owner).await.unwrap());
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(token)
        .bind(now)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        key_activity::owned(&mut conn, token, owner).await.unwrap(),
        "revocation keeps retained history ownership"
    );
    drop(conn);
    db.close().await.unwrap();
}

#[tokio::test]
async fn equal_clock_read_snapshots_exclude_concurrent_self_reads_without_omitting_old_rows() {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    let (token, owner) = key(&mut conn).await;
    let now = clock::start();
    for n in [10, 20, 30, 40] {
        assert!(append(&mut conn, token, owner, Id::from_u128(n), now, now).await);
    }
    let snapshot = key_activity::snapshot(&mut conn, token).await.unwrap();
    let first = key_activity::page(&mut conn, token, None, now, snapshot, None, 2)
        .await
        .unwrap();
    assert_eq!(
        first.iter().map(|r| r.id).collect::<Vec<_>>(),
        [Id::from_u128(40), Id::from_u128(30)]
    );
    let mut sibling = db.pool.acquire().await.unwrap();
    let (a, b) = tokio::join!(
        append(&mut conn, token, owner, Id::from_u128(25), now, now),
        append(&mut sibling, token, owner, Id::from_u128(15), now, now),
    );
    assert!(a && b);
    let last = first.last().unwrap();
    let second = key_activity::page(
        &mut conn,
        token,
        None,
        now,
        snapshot,
        Some((last.at, last.id)),
        101,
    )
    .await
    .unwrap();
    assert_eq!(
        second.iter().map(|r| r.id).collect::<Vec<_>>(),
        [Id::from_u128(20), Id::from_u128(10)],
        "later self-read records with identical timestamps must not extend the traversal"
    );
    assert_eq!(
        rows(&mut conn, token, now).await.len(),
        6,
        "a fresh traversal sees both completed calls"
    );
    assert!(
        key_activity::page(
            &mut conn,
            token,
            None,
            now + Duration::days(8),
            snapshot,
            None,
            101
        )
        .await
        .unwrap()
        .is_empty(),
        "a cursor never freezes the age bound"
    );
    assert_eq!(
        key_activity::purge(&mut conn, now + Duration::days(8), 1)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        key_activity::purge(&mut conn, now + Duration::days(8), 100)
            .await
            .unwrap(),
        5
    );
    drop(sibling);
    drop(conn);
    db.close().await.unwrap();
}

#[tokio::test]
async fn simultaneous_completions_keep_at_most_one_hundred_records() {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    let (token, owner) = key(&mut conn).await;
    let now = clock::start();
    for n in 1..=99 {
        assert!(append(&mut conn, token, owner, Id::from_u128(n), now, now).await);
    }
    let mut other = db.pool.acquire().await.unwrap();
    let (a, b) = tokio::join!(
        append(&mut conn, token, owner, Id::from_u128(100), now, now),
        append(&mut other, token, owner, Id::from_u128(101), now, now),
    );
    assert!(a && b);
    let found = rows(&mut conn, token, now).await;
    assert_eq!(found.len(), 100);
    assert_eq!(found.first().unwrap().id, Id::from_u128(101));
    assert_eq!(found.last().unwrap().id, Id::from_u128(2));
    drop(other);
    drop(conn);
    db.close().await.unwrap();
}
