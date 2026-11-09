//! A process-independent sweep supplier; effects and progress commit under the actual job lease.
use super::job_leases::Lease;
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Clock, Id};

/// The current frozen sweep and its next document/file supplier.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Progress {
    /// Native phase, documents followed by oldest-first unnamed files.
    pub phase: String,
    /// Last fully committed document.
    pub after_document: Option<Id>,
    /// Last fully committed file's added time.
    pub after_file_at: Option<DateTime<Utc>>,
    /// Last fully committed file at that time.
    pub after_file: Option<Id>,
    /// Fixed removed-block cutoff for this whole sweep.
    pub blocks_before: DateTime<Utc>,
    /// Fixed accepted-batch cutoff.
    pub batches_before: DateTime<Utc>,
    /// Fixed upload grace cutoff.
    pub files_before: DateTime<Utc>,
}
/// Open or resume the same sweep after pinning the real lease on this page transaction.
pub async fn open(
    conn: &mut Conn,
    lease: &Lease,
    clock: &dyn Clock,
    blocks: DateTime<Utc>,
    batches: DateTime<Utc>,
    files: DateTime<Utc>,
) -> Result<Option<Progress>, DbError> {
    if lease.name() != "document_purge" || lease.fence(conn, clock).await?.is_none() {
        return Ok(None);
    }
    sqlx::query("INSERT INTO document_purge_progress(job,phase,blocks_before,batches_before,files_before) VALUES($1,'documents',$2,$3,$4) ON CONFLICT(job) DO NOTHING")
        .bind(lease.name()).bind(blocks).bind(batches).bind(files).execute(&mut *conn).await?;
    Ok(sqlx::query_as("SELECT phase,after_document,after_file_at,after_file,blocks_before,batches_before,files_before FROM document_purge_progress WHERE job=$1 FOR UPDATE")
        .bind(lease.name()).fetch_optional(conn).await?)
}
/// Advance only the exact still-live holder; the caller rolls back all page effects on false.
pub async fn save(
    conn: &mut Conn,
    lease: &Lease,
    clock: &dyn Clock,
    progress: &Progress,
) -> Result<bool, DbError> {
    if lease.fence(conn, clock).await?.is_none() {
        return Ok(false);
    }
    let changed=sqlx::query("UPDATE document_purge_progress SET phase=$2,after_document=$3,after_file_at=$4,after_file=$5 WHERE job=$1")
        .bind(lease.name()).bind(&progress.phase).bind(progress.after_document).bind(progress.after_file_at).bind(progress.after_file)
        .execute(&mut *conn).await?.rows_affected();
    Ok(changed == 1)
}
/// Finish only after both suppliers are exhausted; a crash before commit leaves the old supplier.
pub async fn finish(conn: &mut Conn, lease: &Lease, clock: &dyn Clock) -> Result<bool, DbError> {
    if lease.fence(conn, clock).await?.is_none() {
        return Ok(false);
    }
    Ok(
        sqlx::query("DELETE FROM document_purge_progress WHERE job=$1")
            .bind(lease.name())
            .execute(conn)
            .await?
            .rows_affected()
            == 1,
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a test fails by panicking")]
    use super::*;
    use crate::Db;
    use chrono::Duration;
    use orch_testkit::{clock, database::TestDb};

    #[tokio::test]
    async fn a_new_holder_resumes_the_committed_supplier_and_old_holder_cannot_advance_it() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let clock = clock::manual();
        let first = Lease::new("document_purge", "old-holder", Duration::minutes(1));
        let second = Lease::new("document_purge", "new-holder", Duration::minutes(1));
        assert!(first.hold(&db, clock.now()).await.unwrap());
        let cutoff = clock.now() - Duration::days(30);
        let mut tx = db.begin().await.unwrap();
        let mut source = open(&mut tx, &first, &clock, cutoff, cutoff, cutoff)
            .await
            .unwrap()
            .unwrap();
        let saved = orch_types::new_id();
        source.after_document = Some(saved);
        assert!(save(&mut tx, &first, &clock, &source).await.unwrap());
        tx.commit().await.unwrap();
        clock.advance(Duration::minutes(2));
        assert!(second.hold(&db, clock.now()).await.unwrap());
        let mut tx = db.begin().await.unwrap();
        assert!(
            open(
                &mut tx,
                &first,
                &clock,
                clock.now(),
                clock.now(),
                clock.now()
            )
            .await
            .unwrap()
            .is_none()
        );
        let resumed = open(
            &mut tx,
            &second,
            &clock,
            clock.now(),
            clock.now(),
            clock.now(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(resumed.after_document, Some(saved));
        assert_eq!(resumed.blocks_before, cutoff);
        source.after_document = Some(orch_types::new_id());
        assert!(!save(&mut tx, &first, &clock, &source).await.unwrap());
        assert!(!finish(&mut tx, &first, &clock).await.unwrap());
        assert!(finish(&mut tx, &second, &clock).await.unwrap());
        tx.commit().await.unwrap();
    }

    #[tokio::test]
    async fn expiry_during_a_page_rolls_back_the_supplier_with_that_pages_effects() {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let clock = clock::manual();
        let lease = Lease::new("document_purge", "page-holder", Duration::minutes(1));
        assert!(lease.hold(&db, clock.now()).await.unwrap());
        let mut tx = db.begin().await.unwrap();
        let mut source = open(
            &mut tx,
            &lease,
            &clock,
            clock.now(),
            clock.now(),
            clock.now(),
        )
        .await
        .unwrap()
        .unwrap();
        source.phase = "files".into();
        source.after_file = Some(orch_types::new_id());
        source.after_file_at = Some(clock.now());
        clock.advance(Duration::minutes(2));
        assert!(!save(&mut tx, &lease, &clock, &source).await.unwrap());
        tx.rollback().await.unwrap();
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM document_purge_progress")
            .fetch_one(&test.pool)
            .await
            .unwrap();
        assert_eq!(rows, 0);
    }
}
