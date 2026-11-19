//! Leases on jobs: one process at a time runs a job, or holds a role such as the realtime coordinator, by name, with no transaction held open.
//!
//! A lock that lives in a transaction holds the transaction open for as long as it is held, and an open
//! transaction holds back what the database may clean up everywhere, and a connection of the pool with it. A
//! job whose run commits in several transactions, or a role a process holds for as long as it runs, holds a
//! lease instead: a row of `job_leases` with its holder and the moment it is free again. It is taken where it
//! is free (no row, or one whose moment has passed) or already the taker's, renewed by taking it again before
//! that moment, and released when the work is done or the process stops. A holder that dies leaves it to lapse.
//! Every statement here stands alone and commits at once.

use crate::pool::Db;
use crate::{Conn, DbError};
use chrono::{DateTime, Duration, Utc};

/// Takes the lease `name` for `holder` until `until`, where it is free at `now` or already `holder`'s; a
/// holder's take renews it. Says whether `holder` holds it now.
pub async fn take(
    conn: &mut Conn,
    name: &str,
    holder: &str,
    now: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<bool, DbError> {
    let taken: Option<String> = sqlx::query_scalar(
        "INSERT INTO job_leases (name, holder, until) VALUES ($1, $2, $3)
         ON CONFLICT (name) DO UPDATE SET holder = EXCLUDED.holder, until = EXCLUDED.until
          WHERE job_leases.holder = EXCLUDED.holder OR job_leases.until <= $4
         RETURNING holder",
    )
    .bind(name)
    .bind(holder)
    .bind(until)
    .bind(now)
    .fetch_optional(conn)
    .await?;
    Ok(taken.is_some())
}

/// Gives the lease `name` up, where `holder` holds it. Says whether it did.
pub async fn release(conn: &mut Conn, name: &str, holder: &str) -> Result<bool, DbError> {
    let released = sqlx::query("DELETE FROM job_leases WHERE name = $1 AND holder = $2")
        .bind(name)
        .bind(holder)
        .execute(conn)
        .await?
        .rows_affected();
    Ok(released > 0)
}

/// Who holds the lease `name` at `now`, if anybody does.
pub async fn holder(
    conn: &mut Conn,
    name: &str,
    now: DateTime<Utc>,
) -> Result<Option<String>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT holder FROM job_leases WHERE name = $1 AND until > $2")
            .bind(name)
            .bind(now)
            .fetch_optional(conn)
            .await?,
    )
}

/// A lease as one holder keeps it: its name, who holds it, and for how long each take or renewal holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    name: String,
    holder: String,
    length: Duration,
}

impl Lease {
    /// The lease `name` as `holder` keeps it, each take holding it for `length`.
    #[must_use]
    pub fn new(name: impl Into<String>, holder: impl Into<String>, length: Duration) -> Self {
        Self {
            name: name.into(),
            holder: holder.into(),
            length,
        }
    }

    /// The lease `name` for one run of a job: its holder is that run alone.
    #[must_use]
    pub fn for_run(name: impl Into<String>, length: Duration) -> Self {
        Self::new(name, orch_types::new_id().simple().to_string(), length)
    }

    /// Its name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Takes it, or renews it, at `now`. Says whether this holder holds it now.
    pub async fn hold(&self, db: &Db, now: DateTime<Utc>) -> Result<bool, DbError> {
        let mut conn = db.acquire().await?;
        take(&mut conn, &self.name, &self.holder, now, now + self.length).await
    }

    /// Gives it up, where this holder holds it.
    pub async fn release(&self, db: &Db) -> Result<(), DbError> {
        let mut conn = db.acquire().await?;
        release(&mut conn, &self.name, &self.holder).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_testkit::{clock, database::TestDb};

    #[tokio::test]
    async fn a_lease_is_held_by_one_renewed_by_it_free_after_it_lapses_and_given_up_on_release() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let now = clock::start();
        let length = Duration::seconds(30);
        let first = Lease::new("retention", "first", length);
        let second = Lease::new("retention", "second", length);
        assert!(first.hold(&db, now).await.unwrap(), "a free lease is taken");
        assert!(
            !second.hold(&db, now).await.unwrap(),
            "another holder waits"
        );
        let later = now + Duration::seconds(20);
        assert!(
            first.hold(&db, later).await.unwrap(),
            "its holder renews it"
        );
        assert!(
            !second.hold(&db, now + Duration::seconds(40)).await.unwrap(),
            "the renewal moved its end: 20 + 30 seconds"
        );
        let mut conn = db.acquire().await.unwrap();
        assert_eq!(
            holder(&mut conn, "retention", later)
                .await
                .unwrap()
                .as_deref(),
            Some("first")
        );
        drop(conn);
        let lapsed = later + length;
        assert!(
            second.hold(&db, lapsed).await.unwrap(),
            "a lease whose holder stopped renewing lapses"
        );
        assert!(
            !first.hold(&db, lapsed).await.unwrap(),
            "and its former holder now waits"
        );
        second.release(&db).await.unwrap();
        assert!(
            first.hold(&db, lapsed).await.unwrap(),
            "a released lease is free at once"
        );
        first.release(&db).await.unwrap();
        let mut conn = db.acquire().await.unwrap();
        assert_eq!(holder(&mut conn, "retention", lapsed).await.unwrap(), None);
        assert!(
            !release(&mut conn, "retention", "first").await.unwrap(),
            "nothing is left to give up"
        );
    }

    #[tokio::test]
    async fn taking_a_lease_leaves_no_transaction_open() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let lease = Lease::for_run("document_purge", Duration::minutes(10));
        assert!(lease.hold(&db, clock::start()).await.unwrap());
        let mut conn = db.acquire().await.unwrap();
        let open: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
              WHERE datname = current_database() AND pid <> pg_backend_pid() AND state = 'idle in transaction'",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(open, 0, "the lease is a row, not a transaction kept open");
    }
}
