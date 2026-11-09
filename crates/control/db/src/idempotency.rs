//! Request replay records.
//!
//! A creating request may carry a key. Its native identity and status are kept
//! for a time, and repeats render current content without acting again. Native
//! secret-creation exceptions retain their own non-secret replay metadata.

// One session-lock identity for claim holders and expiry cleanup.
macro_rules! creation_lock_key {
    ($account:literal,$key:literal) => {
        concat!(
            "hashtextextended('orch.creation:' || ",
            $account,
            "::uuid::text || ':' || ",
            $key,
            ",0)"
        )
    };
}
pub mod creation;

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
    /// Native replay metadata; general creations store a versioned identity,
    /// never an existing-content response or a credential.
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
        // pg_locks exposes a bigint advisory key as two 32-bit OID coordinates.
        // Observe existing holders without allocating a transaction lock per expired row.
        sqlx::query(concat!(
            "DELETE FROM idempotency_keys AS k WHERE created_at <= $1 AND NOT EXISTS (
            SELECT 1 FROM pg_locks AS l WHERE l.locktype='advisory' AND l.granted AND l.objsubid=1
            AND l.database=(SELECT oid FROM pg_database WHERE datname=current_database())
            AND ((l.classid::bigint << 32) | l.objid::bigint)=",
            creation_lock_key!("k.user_id", "k.key"),
            ")"
        ))
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
    async fn expiry_keeps_a_live_creation_claim_then_removes_it_after_the_same_session_unlocks() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let user = orch_types::new_id();
        let key = orch_types::new_id().to_string();
        let now = clock::start();
        sqlx::query("INSERT INTO users (id,username,display_name,avatar_color) VALUES($1,'claim-maker','Maker',0)")
            .bind(user).execute(&mut *conn).await.unwrap();
        keep(
            &mut conn,
            user,
            &key,
            &Replay {
                method: "POST".into(),
                path: "/nodes/example/documents".into(),
                status: 201,
                response: serde_json::json!({"native":"pending"}),
            },
            now,
        )
        .await
        .unwrap();
        let mut holding = test.pool.acquire().await.unwrap();
        assert!(creation::try_lock(&mut holding, user, &key).await.unwrap());
        let before = now + Duration::days(2);
        assert_eq!(purge(&mut conn, before).await.unwrap(), 0);
        assert!(
            find(&mut conn, user, &key, now - Duration::hours(1))
                .await
                .unwrap()
                .is_some()
        );
        assert!(creation::unlock(&mut holding, user, &key).await.unwrap());
        assert_eq!(purge(&mut conn, before).await.unwrap(), 1);
        assert!(
            find(&mut conn, user, &key, now - Duration::hours(1))
                .await
                .unwrap()
                .is_none()
        );
    }

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
