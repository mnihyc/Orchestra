//! Commit-time authorization locks include equal and physically absent zero epochs.

use orch_db::{
    Conn, Db, DbError, Tx,
    queries::{
        access,
        transaction_guard::{self, Scope},
    },
};
use orch_testkit::database::TestDb;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::{future::Future, io, time::Duration};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

// Match the existing database row-lock tests' scheduling headroom. A positive
// pg_blocking_pids relationship, not the passage of this time, proves the wait.
const LOCK_WAIT_LIMIT: Duration = Duration::from_secs(10);
const LOCK_POLL: Duration = Duration::from_millis(10);
// The fixture's two-integer advisory namespace is separate from production's
// single-bigint named locks. Each isolated database runs one interleaving at a time.
const LATCH_CLASS: i32 = 0x6570;
const LATCH_SLOT: i32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Birth,
    Change,
    AdvanceOnly,
}

struct Decision {
    tx: Tx,
    pid: i32,
    effect: Id,
}

fn failure(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(io::Error::other(message.into()))
}

async fn fixture() -> TestResult<(TestDb, Db)> {
    let test = TestDb::fresh().await;
    // The four fixture connections are sufficient: the latch, committing
    // decision, competing decision, and observer. No detached tasks own a pool.
    sqlx::raw_sql(r#"
        CREATE TABLE commit_epoch_probe_effects(id uuid PRIMARY KEY,
            xact xid8 NOT NULL DEFAULT pg_current_xact_id(), checked_epochs jsonb);
        CREATE FUNCTION commit_epoch_probe_pause() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
            UPDATE commit_epoch_probe_effects SET checked_epochs=(
                SELECT COALESCE(jsonb_object_agg(e.org_id::text,e.epoch),'{}') FROM authz_epochs e
                 WHERE e.org_id IN (SELECT k::uuid FROM jsonb_object_keys(orch_setting_json('orch.authz_seen','{}')) k)
                    OR e.org_id IN (SELECT a::uuid FROM jsonb_array_elements_text(orch_setting_json('orch.authz_advance','[]')) a)
            ) WHERE xact=OLD.xact;
            IF COALESCE(current_setting('commit_epoch_probe.pause',true),'')='on' THEN
                PERFORM pg_advisory_xact_lock(25968,1);
            END IF;
            RETURN OLD;
        END $$;
        CREATE TRIGGER commit_epoch_probe_pause BEFORE DELETE ON commit_work
            FOR EACH ROW EXECUTE FUNCTION commit_epoch_probe_pause();
    "#).execute(&test.pool).await?;
    // Only this observer is added: the deployed deferred trigger and its
    // epoch/cache statements remain intact. DELETE is after their real checks.
    let db = Db::from_pool(test.pool.clone());
    Ok((test, db))
}

async fn decision(db: &Db, kind: Kind, watched: &[Id], advanced: &[Id]) -> TestResult<Decision> {
    let scope = match kind {
        Kind::Birth => Scope::Creation,
        Kind::Change => Scope::Authority,
        Kind::AdvanceOnly => Scope::Action,
    };
    let mut tx = transaction_guard::request_scope(scope, db.begin()).await?;
    let expected_seen = if kind == Kind::AdvanceOnly {
        assert!(watched.is_empty());
        json!({})
    } else {
        let accepted: bool =
            sqlx::query_scalar("SELECT orch_authz_saw(orch_authz_epochs($1::uuid[]))")
                .bind(watched)
                .fetch_one(&mut *tx)
                .await?;
        assert!(accepted);
        sqlx::query_scalar::<_, Json>("SELECT orch_authz_epochs($1::uuid[])")
            .bind(watched)
            .fetch_one(&mut *tx)
            .await?
    };
    if kind == Kind::Birth {
        assert!(advanced.is_empty());
    } else {
        assert!(!advanced.is_empty());
        let _: String = sqlx::query_scalar(
            "SELECT set_config('orch.authz_scope',to_jsonb($1::uuid[])::text,true)",
        )
        .bind(advanced)
        .fetch_one(&mut *tx)
        .await?;
    }
    access::bump_epoch(&mut tx).await?;
    let (seen, advance, mode, isolation): (Json, Json, String, String) = sqlx::query_as(
        "SELECT orch_setting_json('orch.authz_seen','{}'),
                orch_setting_json('orch.authz_advance','[]'), current_setting('orch.guard_epoch'),
                current_setting('transaction_isolation')",
    )
    .fetch_one(&mut *tx)
    .await?;
    assert_eq!(seen, expected_seen);
    assert_eq!(advance, json!(advanced));
    assert_eq!(
        mode,
        if kind == Kind::Birth {
            "birth"
        } else {
            "write"
        }
    );
    assert_eq!(
        isolation, "read committed",
        "these interleavings exercise the deployed default isolation"
    );
    let effect = new_id();
    sqlx::query("INSERT INTO commit_epoch_probe_effects(id) VALUES($1)")
        .bind(effect)
        .execute(&mut *tx)
        .await?;
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await?;
    Ok(Decision { tx, pid, effect })
}

async fn pause_at_commit(tx: &mut Conn) -> TestResult {
    let _: String = sqlx::query_scalar("SELECT set_config('commit_epoch_probe.pause','on',true)")
        .fetch_one(tx)
        .await?;
    Ok(())
}

async fn hold(
    pool: &sqlx::PgPool,
) -> TestResult<(sqlx::Transaction<'static, sqlx::Postgres>, i32)> {
    // This raw transaction is only the external observer's latch. It neither
    // reads authority nor arms commit work of its own.
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1,$2)")
        .bind(LATCH_CLASS)
        .bind(LATCH_SLOT)
        .execute(&mut *tx)
        .await?;
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await?;
    Ok((tx, pid))
}

async fn wait_for_blocker(
    observer: &mut Conn,
    waiter: i32,
    blocker: i32,
    phase: &str,
) -> TestResult {
    tokio::time::timeout(LOCK_WAIT_LIMIT, async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT pg_blocking_pids($1) @> ARRAY[$2]::integer[]",
            ).bind(waiter).bind(blocker).fetch_one(&mut *observer).await?;
            if waiting { return Ok::<_, sqlx::Error>(()); }
            tokio::time::sleep(LOCK_POLL).await;
        }
    }).await.map_err(|_| failure(format!(
        "{phase}: backend {waiter} never waited on exact backend {blocker} within {LOCK_WAIT_LIMIT:?}",
    )))??;
    Ok(())
}

async fn require_blocked(
    pending: impl Future<Output = Result<(), sqlx::Error>>,
    observer: &mut Conn,
    waiter: i32,
    blocker: i32,
    phase: &str,
) -> TestResult {
    tokio::select! {
        result = pending => Err(failure(format!("{phase}: commit completed before its required wait: {result:?}"))),
        result = wait_for_blocker(observer, waiter, blocker, phase) => result,
    }
}

async fn require_epoch_wait(observer: &mut Conn, pid: i32) -> TestResult {
    let (epoch_write, cache_write): (bool, bool) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation='authz_epochs'::regclass
                        AND mode='RowExclusiveLock' AND granted),
                EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation='authz_state'::regclass
                        AND mode='RowExclusiveLock' AND granted)",
    )
    .bind(pid)
    .fetch_one(observer)
    .await?;
    assert!(
        epoch_write,
        "the competitor must reach native epoch initialization"
    );
    assert!(
        !cache_write,
        "a later cache-counter wait is not evidence of an epoch fence"
    );
    Ok(())
}

async fn require_checked_epochs(
    observer: &mut Conn,
    effect: Id,
    epochs: &[(Id, i64)],
) -> TestResult {
    let actual: Json =
        sqlx::query_scalar("SELECT checked_epochs FROM commit_epoch_probe_effects WHERE id=$1")
            .bind(effect)
            .fetch_one(observer)
            .await?;
    let expected: serde_json::Map<String, Json> = epochs
        .iter()
        .map(|(id, epoch)| (id.to_string(), json!(epoch)))
        .collect();
    assert_eq!(
        actual,
        Json::Object(expected),
        "the real post-check transaction must contain every physical zero or advanced epoch"
    );
    Ok(())
}

fn require_retry(result: Result<(), sqlx::Error>) -> TestResult {
    let error = match result {
        Err(error) => error,
        Ok(()) => return Err(failure("a stale authorization decision committed")),
    };
    let database = error
        .as_database_error()
        .ok_or_else(|| failure(format!("expected database refusal: {error}")))?;
    assert_eq!(database.code().as_deref(), Some("40001"));
    assert_eq!(database.constraint(), Some("authz_epoch_moved"));
    assert!(matches!(DbError::from(error), DbError::Retry));
    Ok(())
}

async fn require_locked(pool: &sqlx::PgPool, place: Id) -> TestResult {
    let mut probe = pool.begin().await?;
    let result = sqlx::query_scalar::<_, i64>(
        "SELECT epoch FROM authz_epochs WHERE org_id=$1 FOR NO KEY UPDATE NOWAIT",
    )
    .bind(place)
    .fetch_optional(&mut *probe)
    .await;
    probe.rollback().await?;
    let error = match result {
        Err(error) => error,
        Ok(value) => {
            return Err(failure(format!(
                "equal epoch {place} was not locked: {value:?}"
            )));
        }
    };
    let database = error
        .as_database_error()
        .ok_or_else(|| failure(format!("expected native lock conflict: {error}")))?;
    assert_eq!(
        database.code().as_deref(),
        Some("55P03"),
        "the row lock itself must conflict"
    );
    Ok(())
}

async fn require_final(
    observer: &mut Conn,
    epochs: &[(Id, i64)],
    present: &[Id],
    absent: &[Id],
    cache: i64,
) -> TestResult {
    let ids: Vec<_> = epochs.iter().map(|(id, _)| *id).collect();
    let actual: Vec<(Id, i64)> = sqlx::query_as(
        "SELECT org_id,epoch FROM authz_epochs WHERE org_id=ANY($1) ORDER BY org_id",
    )
    .bind(&ids)
    .fetch_all(&mut *observer)
    .await?;
    let mut expected = epochs.to_vec();
    expected.sort_unstable();
    assert_eq!(
        actual, expected,
        "physical epochs, including zero rows, must match exactly"
    );
    let effects: Vec<_> = present.iter().chain(absent).copied().collect();
    let actual: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM commit_epoch_probe_effects WHERE id=ANY($1) ORDER BY id",
    )
    .bind(&effects)
    .fetch_all(&mut *observer)
    .await?;
    let mut expected = present.to_vec();
    expected.sort_unstable();
    assert_eq!(
        actual, expected,
        "a refused commit must roll back its exact effect"
    );
    assert_eq!(
        access::epoch(observer).await?,
        cache,
        "only successful commits move the cache counter"
    );
    let outstanding: i64 = sqlx::query_scalar("SELECT count(*) FROM commit_work")
        .fetch_one(observer)
        .await?;
    assert_eq!(
        outstanding, 0,
        "successful and refused commits both retire their work"
    );
    Ok(())
}

#[tokio::test]
async fn a_watched_absent_zero_fences_the_first_competing_epoch_insertion() -> TestResult {
    let (test, db) = fixture().await?;
    let mut observer = test.pool.acquire().await?;
    for kind in [Kind::Birth, Kind::Change] {
        eprintln!("absent zero, original decision first: {kind:?}");
        let root = new_id();
        let physical: Option<i64> =
            sqlx::query_scalar("SELECT epoch FROM authz_epochs WHERE org_id=$1")
                .bind(root)
                .fetch_optional(&mut *observer)
                .await?;
        assert_eq!(physical, None);
        let cache = access::epoch(&mut observer).await?;
        let advanced = if kind == Kind::Birth {
            Vec::new()
        } else {
            vec![root]
        };
        let Decision {
            mut tx,
            pid,
            effect,
        } = decision(&db, kind, &[root], &advanced).await?;
        let seen: Json = sqlx::query_scalar("SELECT orch_setting_json('orch.authz_seen','{}')")
            .fetch_one(&mut *tx)
            .await?;
        assert_eq!(
            seen.get(root.to_string().as_str()).and_then(Json::as_i64),
            Some(0)
        );
        pause_at_commit(&mut tx).await?;
        let (latch, latch_pid) = hold(&test.pool).await?;
        let leading = tx.commit();
        tokio::pin!(leading);
        require_blocked(
            leading.as_mut(),
            &mut observer,
            pid,
            latch_pid,
            "original commit after epoch work",
        )
        .await?;
        let physical: Option<i64> =
            sqlx::query_scalar("SELECT epoch FROM authz_epochs WHERE org_id=$1")
                .bind(root)
                .fetch_optional(&mut *observer)
                .await?;
        assert_eq!(
            physical, None,
            "the first physical epoch is still uncommitted"
        );

        let other = decision(&db, Kind::Change, &[root], &[root]).await?;
        let other_effect = other.effect;
        let competing = other.tx.commit();
        tokio::pin!(competing);
        require_blocked(
            competing.as_mut(),
            &mut observer,
            other.pid,
            pid,
            "concurrent first epoch insertion",
        )
        .await?;
        require_epoch_wait(&mut observer, other.pid).await?;
        latch.rollback().await?;
        tokio::time::timeout(LOCK_WAIT_LIMIT, leading.as_mut()).await??;
        require_checked_epochs(
            &mut observer,
            effect,
            &[
                (transaction_guard::SYSTEM, 0),
                (root, if kind == Kind::Birth { 0 } else { 1 }),
            ],
        )
        .await?;
        let result = tokio::time::timeout(LOCK_WAIT_LIMIT, competing.as_mut()).await?;
        let (present, absent, moved) = if kind == Kind::Birth {
            result?;
            (vec![effect, other_effect], Vec::new(), 2)
        } else {
            require_retry(result)?;
            (vec![effect], vec![other_effect], 1)
        };
        require_final(
            &mut observer,
            &[(transaction_guard::SYSTEM, 0), (root, 1)],
            &present,
            &absent,
            cache + moved,
        )
        .await?;
    }
    drop(observer);
    drop(db);
    test.close().await?;
    Ok(())
}

#[tokio::test]
async fn a_first_committed_epoch_refuses_an_older_absent_zero_decision_atomically() -> TestResult {
    let (test, db) = fixture().await?;
    let mut observer = test.pool.acquire().await?;
    for kind in [Kind::Birth, Kind::Change] {
        eprintln!("absent zero, first authority change first: {kind:?}");
        let root = new_id();
        let physical: Option<i64> =
            sqlx::query_scalar("SELECT epoch FROM authz_epochs WHERE org_id=$1")
                .bind(root)
                .fetch_optional(&mut *observer)
                .await?;
        assert_eq!(physical, None);
        let cache = access::epoch(&mut observer).await?;
        let advanced = if kind == Kind::Birth {
            Vec::new()
        } else {
            vec![root]
        };
        let older = decision(&db, kind, &[root], &advanced).await?;
        let first = decision(&db, Kind::Change, &[root], &[root]).await?;
        tokio::time::timeout(LOCK_WAIT_LIMIT, first.tx.commit()).await??;
        require_checked_epochs(
            &mut observer,
            first.effect,
            &[(transaction_guard::SYSTEM, 0), (root, 1)],
        )
        .await?;
        let refusal = tokio::time::timeout(LOCK_WAIT_LIMIT, older.tx.commit()).await?;
        require_retry(refusal)?;
        require_final(
            &mut observer,
            &[(transaction_guard::SYSTEM, 0), (root, 1)],
            &[first.effect],
            &[older.effect],
            cache + 1,
        )
        .await?;
    }
    drop(observer);
    drop(db);
    test.close().await?;
    Ok(())
}

#[tokio::test]
async fn every_equal_watched_or_advanced_root_is_locked_until_the_real_commit_finishes()
-> TestResult {
    let (test, db) = fixture().await?;
    let mut observer = test.pool.acquire().await?;
    for kind in [Kind::Birth, Kind::Change, Kind::AdvanceOnly] {
        eprintln!("complete first/middle/last epoch lock set: {kind:?}");
        let mut roots: [Id; 3] = std::array::from_fn(|_| new_id());
        roots.sort_unstable();
        sqlx::query("INSERT INTO authz_epochs(org_id) SELECT unnest($1::uuid[]) ORDER BY 1")
            .bind(roots.as_slice())
            .execute(&mut *observer)
            .await?;
        let cache = access::epoch(&mut observer).await?;
        // Present roots begin equal to every value in seen. Reverse the input
        // order so serialization is the schema's order, not the caller's order.
        let watched: Vec<_> = if kind == Kind::AdvanceOnly {
            Vec::new()
        } else {
            roots.iter().rev().copied().collect()
        };
        let advanced = match kind {
            Kind::Birth => Vec::new(),
            Kind::Change => vec![roots[0]],
            Kind::AdvanceOnly => roots.iter().rev().copied().collect(),
        };
        let Decision {
            mut tx,
            pid,
            effect,
        } = decision(&db, kind, &watched, &advanced).await?;
        pause_at_commit(&mut tx).await?;
        let (latch, latch_pid) = hold(&test.pool).await?;
        let leading = tx.commit();
        tokio::pin!(leading);
        require_blocked(
            leading.as_mut(),
            &mut observer,
            pid,
            latch_pid,
            "complete epoch set after production checks",
        )
        .await?;
        if kind != Kind::AdvanceOnly {
            require_locked(&test.pool, transaction_guard::SYSTEM).await?;
        }
        for root in roots {
            require_locked(&test.pool, root).await?;
        }

        let other = decision(&db, Kind::Change, &[roots[2]], &[roots[2]]).await?;
        let other_effect = other.effect;
        let competing = other.tx.commit();
        tokio::pin!(competing);
        require_blocked(
            competing.as_mut(),
            &mut observer,
            other.pid,
            pid,
            "last-root authority commit",
        )
        .await?;
        require_epoch_wait(&mut observer, other.pid).await?;
        latch.rollback().await?;
        tokio::time::timeout(LOCK_WAIT_LIMIT, leading.as_mut()).await??;
        let checked: Vec<_> = match kind {
            Kind::Birth => [
                (transaction_guard::SYSTEM, 0),
                (roots[0], 0),
                (roots[1], 0),
                (roots[2], 0),
            ]
            .to_vec(),
            Kind::Change => [
                (transaction_guard::SYSTEM, 0),
                (roots[0], 1),
                (roots[1], 0),
                (roots[2], 0),
            ]
            .to_vec(),
            Kind::AdvanceOnly => roots.iter().map(|root| (*root, 1)).collect(),
        };
        require_checked_epochs(&mut observer, effect, &checked).await?;
        let result = tokio::time::timeout(LOCK_WAIT_LIMIT, competing.as_mut()).await?;
        let (present, absent, epochs, moved) = match kind {
            Kind::Birth => {
                result?;
                (vec![effect, other_effect], Vec::new(), [0, 0, 1], 2)
            }
            Kind::Change => {
                result?;
                (vec![effect, other_effect], Vec::new(), [1, 0, 1], 2)
            }
            Kind::AdvanceOnly => {
                require_retry(result)?;
                (vec![effect], vec![other_effect], [1, 1, 1], 1)
            }
        };
        let expected = [
            (transaction_guard::SYSTEM, 0),
            (roots[0], epochs[0]),
            (roots[1], epochs[1]),
            (roots[2], epochs[2]),
        ];
        require_final(&mut observer, &expected, &present, &absent, cache + moved).await?;
    }
    drop(observer);
    drop(db);
    test.close().await?;
    Ok(())
}

#[tokio::test]
async fn a_current_resource_root_change_retires_only_that_root_and_restores_the_callers_scope()
-> TestResult {
    let (_test, db) = fixture().await?;
    let root = new_id();
    let other = new_id();
    let roots = vec![root, other];
    let before: Json = sqlx::query_scalar("SELECT orch_authz_epochs($1::uuid[])")
        .bind(&roots)
        .fetch_one(db.pool())
        .await?;
    let mut tx = transaction_guard::request_scope(Scope::Authority, db.begin()).await?;
    let _: bool = sqlx::query_scalar("SELECT orch_authz_saw(orch_authz_epochs($1::uuid[]))")
        .bind(&roots)
        .fetch_one(&mut *tx)
        .await?;
    let scope = json!([other]).to_string();
    let _: String = sqlx::query_scalar("SELECT set_config('orch.authz_scope',$1,true)")
        .bind(&scope)
        .fetch_one(&mut *tx)
        .await?;
    access::bump_epoch_in_root(&mut tx, root).await?;
    let (restored, advanced): (String, Json) = sqlx::query_as(
        "SELECT current_setting('orch.authz_scope'),orch_setting_json('orch.authz_advance','[]')",
    )
    .fetch_one(&mut *tx)
    .await?;
    assert_eq!(restored, scope);
    assert_eq!(advanced, json!([root]));
    tx.commit().await?;
    let after: Json = sqlx::query_scalar("SELECT orch_authz_epochs($1::uuid[])")
        .bind(&roots)
        .fetch_one(db.pool())
        .await?;
    assert_eq!(after[other.to_string()], before[other.to_string()]);
    assert_eq!(after[Id::nil().to_string()], before[Id::nil().to_string()]);
    assert_eq!(after[root.to_string()], json!(1));
    Ok(())
}

#[tokio::test]
async fn a_commit_with_usage_and_an_authority_change_refuses_a_moved_epoch_atomically() -> TestResult
{
    let (test, db) = fixture().await?;
    let mut observer = test.pool.acquire().await?;
    let root = new_id();
    let cache_before = access::epoch(&mut observer).await?;
    let usage_before: i64 =
        sqlx::query_scalar("SELECT revision FROM usage_change_state WHERE singleton")
            .fetch_one(&mut *observer)
            .await?;
    let mut older = decision(&db, Kind::Change, &[root], &[root]).await?;
    let usage = new_id();
    sqlx::query("INSERT INTO usage_events(id,change_seq,at,origin,purpose,scope,counted,org_id,project_id,topic_id,provider_id,model,total_tokens) VALUES($1,0,'2026-10-01T00:00:00Z','forwarder','turn','request',false,$2,$2,$2,$2,'epoch-model',7)")
        .bind(usage).bind(root).execute(&mut *older.tx).await?;
    let hidden: Option<i64> = sqlx::query_scalar("SELECT change_seq FROM usage_events WHERE id=$1")
        .bind(usage)
        .fetch_optional(&mut *observer)
        .await?;
    assert_eq!(hidden, None, "uncommitted usage has no published cursor");
    let newer = decision(&db, Kind::Change, &[root], &[root]).await?;
    let committed_effect = newer.effect;
    tokio::time::timeout(LOCK_WAIT_LIMIT, newer.tx.commit()).await??;
    let refusal = tokio::time::timeout(LOCK_WAIT_LIMIT, older.tx.commit()).await?;
    require_retry(refusal)?;
    require_final(
        &mut observer,
        &[(transaction_guard::SYSTEM, 0), (root, 1)],
        &[committed_effect],
        &[older.effect],
        cache_before + 1,
    )
    .await?;
    let hidden: Option<i64> = sqlx::query_scalar("SELECT change_seq FROM usage_events WHERE id=$1")
        .bind(usage)
        .fetch_optional(&mut *observer)
        .await?;
    assert_eq!(
        hidden, None,
        "the moved epoch rolls back the actual usage row"
    );
    let usage_after: i64 =
        sqlx::query_scalar("SELECT revision FROM usage_change_state WHERE singleton")
            .fetch_one(&mut *observer)
            .await?;
    assert_eq!(
        usage_after, usage_before,
        "refused authority cannot reserve a usage range"
    );
    let journal: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_commit_rows")
        .fetch_one(&mut *observer)
        .await?;
    assert_eq!(
        journal, 0,
        "refused authority also rolls back journal identities"
    );
    drop(observer);
    drop(db);
    test.close().await?;
    Ok(())
}
