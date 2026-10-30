//! Usage cursors number each changed row at commit while retries, savepoints and cancellation preserve atomicity.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_db::{Conn, Db, Tx};
use orch_testkit::database::TestDb;
use orch_types::{Id, new_id};
use std::{future::Future, io, time::Duration};

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
/// Match the existing native epoch-lock tests; observed blockers, not elapsed time, prove ordering.
const LOCK_WAIT: Duration = Duration::from_secs(10);
const LOCK_POLL: Duration = Duration::from_millis(10);
/// An isolated two-integer advisory namespace, distinct from product bigint locks.
const LATCH_CLASS: i32 = 0x7573;
const LATCH_SLOT: i32 = 1;

#[derive(Clone, Copy)]
enum Table {
    Events,
    Running,
}

async fn fixture() -> Proof<(TestDb, Db)> {
    let test = TestDb::fresh().await;
    sqlx::raw_sql("CREATE FUNCTION usage_order_pause() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF current_setting('usage_order.pause',true)='on' THEN PERFORM pg_advisory_xact_lock(30067,1); END IF; RETURN OLD; END $$; CREATE TRIGGER usage_order_pause BEFORE DELETE ON commit_work FOR EACH ROW EXECUTE FUNCTION usage_order_pause();")
        .execute(&test.pool).await?;
    let db = Db::from_pool(test.pool.clone());
    Ok((test, db))
}

async fn insert(conn: &mut Conn, table: Table, id: Id, tokens: i64) -> Proof {
    let sql = match table {
        Table::Events => {
            "INSERT INTO usage_events(id,change_seq,at,origin,purpose,scope,counted,org_id,project_id,topic_id,provider_id,model,input_tokens,total_tokens,credits) VALUES($1,0,'2026-10-01T00:00:00Z','forwarder','turn','request',false,$1,$1,$1,$1,'cursor-model',$2,$2,0)"
        }
        Table::Running => {
            "INSERT INTO usage_running(id,change_seq,at,origin,purpose,scope,counted,org_id,project_id,topic_id,provider_id,model,input_tokens,total_tokens,credits) VALUES($1,0,'2026-10-01T00:00:00Z','forwarder','turn','request',false,$1,$1,$1,$1,'cursor-model',$2,$2,0)"
        }
    };
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(tokens)
        .execute(conn)
        .await?;
    Ok(())
}

async fn revision(conn: &mut Conn) -> Proof<i64> {
    Ok(
        sqlx::query_scalar("SELECT revision FROM usage_change_state WHERE singleton")
            .fetch_one(conn)
            .await?,
    )
}

async fn row(conn: &mut Conn, table: Table, id: Id) -> Proof<Option<(i64, i64)>> {
    let sql = match table {
        Table::Events => "SELECT change_seq,total_tokens FROM usage_events WHERE id=$1",
        Table::Running => "SELECT change_seq,total_tokens FROM usage_running WHERE id=$1",
    };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

async fn clean(conn: &mut Conn) -> Proof {
    let pending: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM usage_commit_rows),(SELECT count(*) FROM commit_work)",
    )
    .fetch_one(conn)
    .await?;
    assert_eq!(
        pending,
        (0, 0),
        "both transaction journals retire atomically"
    );
    Ok(())
}

#[tokio::test]
async fn repeated_changes_keep_first_position_and_last_data_while_immutable_replay_keeps_its_cursor()
-> Proof {
    let (test, db) = fixture().await?;
    let mut observer = db.acquire().await?;
    let baseline = revision(&mut observer).await?;
    let first = new_id();
    let next = new_id();
    let mut tx = db.begin().await?;
    insert(&mut tx, Table::Running, first, 1).await?;
    insert(&mut tx, Table::Events, next, 2).await?;
    sqlx::query("UPDATE usage_running SET input_tokens=3,total_tokens=3 WHERE id=$1")
        .bind(first)
        .execute(&mut *tx)
        .await?;
    assert!(row(&mut observer, Table::Running, first).await?.is_none());
    assert_eq!(revision(&mut observer).await?, baseline);
    tx.commit().await?;
    assert_eq!(
        row(&mut observer, Table::Running, first).await?,
        Some((baseline + 1, 3))
    );
    assert_eq!(
        row(&mut observer, Table::Events, next).await?,
        Some((baseline + 2, 2))
    );
    let mut tx = db.begin().await?;
    sqlx::query("INSERT INTO usage_events SELECT * FROM usage_events WHERE id=$1 ON CONFLICT(id) DO NOTHING").bind(next).execute(&mut *tx).await?;
    sqlx::query("UPDATE usage_events SET credits=credits+0.0000 WHERE id=$1")
        .bind(next)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    assert_eq!(
        row(&mut observer, Table::Events, next).await?,
        Some((baseline + 2, 2))
    );
    assert_eq!(revision(&mut observer).await?, baseline + 2);
    clean(&mut observer).await?;
    drop(observer);
    drop(db);
    test.close().await?;
    Ok(())
}

#[tokio::test]
async fn savepoints_deleted_observations_and_rollback_leave_no_phantom_cursors() -> Proof {
    let (test, db) = fixture().await?;
    let mut observer = db.acquire().await?;
    let baseline = revision(&mut observer).await?;
    let first = new_id();
    let discarded = new_id();
    let deleted = new_id();
    let last = new_id();
    let mut tx = db.begin().await?;
    insert(&mut tx, Table::Events, first, 1).await?;
    sqlx::query("SAVEPOINT observation")
        .execute(&mut *tx)
        .await?;
    insert(&mut tx, Table::Running, discarded, 9).await?;
    sqlx::query("UPDATE usage_events SET input_tokens=9,total_tokens=9 WHERE id=$1")
        .bind(first)
        .execute(&mut *tx)
        .await?;
    sqlx::query("ROLLBACK TO SAVEPOINT observation")
        .execute(&mut *tx)
        .await?;
    sqlx::query("RELEASE SAVEPOINT observation")
        .execute(&mut *tx)
        .await?;
    insert(&mut tx, Table::Running, deleted, 4).await?;
    sqlx::query("DELETE FROM usage_running WHERE id=$1")
        .bind(deleted)
        .execute(&mut *tx)
        .await?;
    insert(&mut tx, Table::Events, last, 2).await?;
    tx.commit().await?;
    assert_eq!(
        row(&mut observer, Table::Events, first).await?,
        Some((baseline + 1, 1))
    );
    assert_eq!(
        row(&mut observer, Table::Events, last).await?,
        Some((baseline + 2, 2))
    );
    assert!(
        row(&mut observer, Table::Running, discarded)
            .await?
            .is_none()
    );
    assert!(row(&mut observer, Table::Running, deleted).await?.is_none());
    let mut rollback = db.begin().await?;
    insert(&mut rollback, Table::Running, discarded, 8).await?;
    rollback.rollback().await?;
    assert!(
        row(&mut observer, Table::Running, discarded)
            .await?
            .is_none()
    );
    assert_eq!(revision(&mut observer).await?, baseline + 2);
    clean(&mut observer).await?;
    drop(observer);
    drop(db);
    test.close().await?;
    Ok(())
}

async fn paused(mut tx: Tx) -> Proof<(Tx, i32)> {
    sqlx::query("SELECT set_config('usage_order.pause','on',true)")
        .execute(&mut *tx)
        .await?;
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await?;
    Ok((tx, pid))
}

async fn blocker(conn: &mut Conn, waiter: i32, holder: i32) -> Proof {
    tokio::time::timeout(LOCK_WAIT, async {
        loop {
            let blocked: bool =
                sqlx::query_scalar("SELECT pg_blocking_pids($1) @> ARRAY[$2]::integer[]")
                    .bind(waiter)
                    .bind(holder)
                    .fetch_one(&mut *conn)
                    .await?;
            if blocked {
                return Ok::<_, sqlx::Error>(());
            }
            tokio::time::sleep(LOCK_POLL).await;
        }
    })
    .await
    .map_err(|_| {
        io::Error::other("the native committing backend never reached its required blocker")
    })??;
    Ok(())
}

async fn require_wait(
    pending: impl Future<Output = Result<(), sqlx::Error>>,
    conn: &mut Conn,
    waiter: i32,
    holder: i32,
) -> Proof {
    tokio::select! {
        outcome=pending=>Err(io::Error::other(format!("commit completed before observed fence: {outcome:?}")).into()),
        result=blocker(conn,waiter,holder)=>result,
    }
}

#[tokio::test]
async fn concurrent_commits_keep_unique_contiguous_ranges_in_commit_order_not_write_order() -> Proof
{
    let (test, db) = fixture().await?;
    let mut observer = db.acquire().await?;
    let baseline = revision(&mut observer).await?;
    let a = [new_id(), new_id()];
    let b = [new_id(), new_id()];
    let mut written_first = db.begin().await?;
    for id in b {
        insert(&mut written_first, Table::Events, id, 2).await?;
    }
    let mut leading = db.begin().await?;
    for id in a {
        insert(&mut leading, Table::Events, id, 1).await?;
    }
    let (leading, leading_pid) = paused(leading).await?;
    let other_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *written_first)
        .await?;
    let mut latch = test.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1,$2)")
        .bind(LATCH_CLASS)
        .bind(LATCH_SLOT)
        .execute(&mut *latch)
        .await?;
    let latch_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *latch)
        .await?;
    let leading = leading.commit();
    tokio::pin!(leading);
    require_wait(leading.as_mut(), &mut observer, leading_pid, latch_pid).await?;
    let other = written_first.commit();
    tokio::pin!(other);
    require_wait(other.as_mut(), &mut observer, other_pid, leading_pid).await?;
    assert_eq!(revision(&mut observer).await?, baseline);
    for id in a.into_iter().chain(b) {
        assert!(row(&mut observer, Table::Events, id).await?.is_none());
    }
    latch.rollback().await?;
    tokio::time::timeout(LOCK_WAIT, leading.as_mut()).await??;
    tokio::time::timeout(LOCK_WAIT, other.as_mut()).await??;
    for (index, id) in a.into_iter().chain(b).enumerate() {
        assert_eq!(
            row(&mut observer, Table::Events, id).await?.unwrap().0,
            baseline + index as i64 + 1
        );
    }
    assert_eq!(revision(&mut observer).await?, baseline + 4);
    clean(&mut observer).await?;
    drop(observer);
    drop(db);
    test.close().await?;
    Ok(())
}

#[tokio::test]
async fn cancelling_a_commit_after_numbering_rolls_back_rows_the_counter_and_its_journal() -> Proof
{
    let (test, db) = fixture().await?;
    let mut observer = db.acquire().await?;
    let baseline = revision(&mut observer).await?;
    let id = new_id();
    let mut tx = db.begin().await?;
    insert(&mut tx, Table::Events, id, 1).await?;
    let (tx, pid) = paused(tx).await?;
    let mut latch = test.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1,$2)")
        .bind(LATCH_CLASS)
        .bind(LATCH_SLOT)
        .execute(&mut *latch)
        .await?;
    let latch_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *latch)
        .await?;
    let pending = tx.commit();
    tokio::pin!(pending);
    require_wait(pending.as_mut(), &mut observer, pid, latch_pid).await?;
    let cancelled: bool = sqlx::query_scalar("SELECT pg_cancel_backend($1)")
        .bind(pid)
        .fetch_one(&mut *observer)
        .await?;
    assert!(cancelled, "only this observed fixture backend is cancelled");
    let error = tokio::time::timeout(LOCK_WAIT, pending.as_mut())
        .await?
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("57014")
    );
    latch.rollback().await?;
    assert!(row(&mut observer, Table::Events, id).await?.is_none());
    assert_eq!(revision(&mut observer).await?, baseline);
    clean(&mut observer).await?;
    drop(observer);
    drop(db);
    test.close().await?;
    Ok(())
}
