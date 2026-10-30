//! Request replay records.
//!
//! A creating request may carry a key. Its first answer is kept for a
//! time and returned to a repeat, which then does nothing again; the same
//! key on another method or path is a mistake of the client and is told.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// What a key was first used for, and what was answered.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Replay {
    /// The method of the first request.
    pub method: String,
    /// Its path.
    pub path: String,
    /// The status that was answered.
    pub status: i32,
    /// The body that was answered.
    pub response: Json,
}

/// The answer kept under `key` for `user`, where one was kept after `since`.
pub async fn find(
    conn: &mut Conn,
    user: Id,
    key: &str,
    since: DateTime<Utc>,
) -> Result<Option<Replay>, DbError> {
    Ok(sqlx::query_as("SELECT method, path, status, response FROM idempotency_keys WHERE user_id = $1 AND key = $2 AND created_at > $3")
        .bind(user)
        .bind(key)
        .bind(since)
        .fetch_optional(conn)
        .await?)
}

/// Keeps the answer of a request under `key`, in the transaction that
/// made the change. A key that was kept before `now` by an answer that has
/// run out is replaced.
pub async fn keep(
    conn: &mut Conn,
    user: Id,
    key: &str,
    replay: &Replay,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO idempotency_keys (user_id, key, method, path, status, response, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (user_id, key) DO UPDATE
            SET method = EXCLUDED.method, path = EXCLUDED.path, status = EXCLUDED.status,
                response = EXCLUDED.response, created_at = EXCLUDED.created_at",
    )
    .bind(user)
    .bind(key)
    .bind(&replay.method)
    .bind(&replay.path)
    .bind(replay.status)
    .bind(&replay.response)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}

/// Forgets every answer kept before `before`. Returns how many went.
pub async fn purge(conn: &mut Conn, before: DateTime<Utc>) -> Result<u64, DbError> {
    Ok(
        sqlx::query("DELETE FROM idempotency_keys WHERE created_at <= $1")
            .bind(before)
            .execute(conn)
            .await?
            .rows_affected(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use orch_testkit::{clock, database::TestDb};

    #[tokio::test]
    async fn an_answer_is_replayed_for_a_day_and_then_forgotten() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let user = orch_types::new_id();
        sqlx::query("INSERT INTO users (id, username, display_name, avatar_color) VALUES ($1, 'ada', 'Ada', 0)")
            .bind(user)
            .execute(&mut *conn)
            .await
            .unwrap();
        let now = clock::start();
        let day = Duration::hours(24);
        let replay = Replay {
            method: "POST".into(),
            path: "/nodes".into(),
            status: 201,
            response: serde_json::json!({ "id": 1 }),
        };
        keep(&mut conn, user, "k1", &replay, now).await.unwrap();

        assert_eq!(
            find(&mut conn, user, "k1", now - day).await.unwrap(),
            Some(replay.clone())
        );
        assert_eq!(find(&mut conn, user, "k2", now - day).await.unwrap(), None);
        // A day later the key is free again.
        let later = now + day + Duration::seconds(1);
        assert_eq!(
            find(&mut conn, user, "k1", later - day).await.unwrap(),
            None
        );
        assert_eq!(purge(&mut conn, later - day).await.unwrap(), 1);
    }
}
