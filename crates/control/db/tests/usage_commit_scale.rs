//! A million ledger rows commit as one bounded-memory cursor range without changing their token totals.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_testkit::database::TestDb;

/// The acceptance corpus represents one million measured requests and ten billion tokens.
const REQUESTS: i64 = 1_000_000;
/// A corpus one order smaller detects growth before the release-sized transaction.
const SMALL_REQUESTS: i64 = 100_000;
const TOKENS_PER_REQUEST: i64 = 10_000;

#[tokio::test]
async fn a_million_usage_rows_in_one_transaction_get_unique_gap_free_commit_cursors() {
    for (requests, cached_input_tokens, cache_write_tokens) in [
        (SMALL_REQUESTS, 200_000_000_i64, 100_000_000_i64),
        (REQUESTS, 2_000_000_000, 1_000_000_000),
    ] {
        let test = TestDb::fresh().await;
        let before: i64 =
            sqlx::query_scalar("SELECT revision FROM usage_change_state WHERE singleton")
                .fetch_one(&test.pool)
                .await
                .unwrap();
        let mut tx = test.pool.begin().await.unwrap();
        let wrote=sqlx::query("INSERT INTO usage_events(id,change_seq,at,origin,purpose,scope,counted,org_id,project_id,topic_id,thread_id,provider_id,model,input_tokens,cached_input_tokens,cache_write_tokens,output_tokens,total_tokens,credits)
          SELECT md5('commit-scale-request-'||n)::uuid,0,'2026-10-01T00:00:00Z','forwarder','turn','request',true,
            md5('commit-scale-org')::uuid,md5('commit-scale-project')::uuid,md5('commit-scale-topic')::uuid,
            md5('commit-scale-thread-'||(n%1000))::uuid,md5('commit-scale-provider')::uuid,'scale-model',8000,2000,1000,2000,$2,0
          FROM generate_series(1,$1::bigint) AS g(n)")
            .bind(requests).bind(TOKENS_PER_REQUEST).execute(&mut *tx).await.unwrap();
        assert_eq!(wrote.rows_affected(), requests as u64);
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM usage_commit_rows WHERE xact=pg_current_xact_id()",
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(
            pending, requests,
            "the database journal, not a client history vector, holds pending identities"
        );
        tx.commit().await.unwrap();
        let facts:(i64,i64,i64,i64,i64,i64,i64,i64)=sqlx::query_as("SELECT count(*),count(DISTINCT change_seq),min(change_seq),max(change_seq),sum(total_tokens)::bigint,sum(cached_input_tokens)::bigint,sum(cache_write_tokens)::bigint,count(DISTINCT thread_id) FROM usage_events")
            .fetch_one(&test.pool).await.unwrap();
        assert_eq!(
            facts,
            (
                requests,
                requests,
                before + 1,
                before + requests,
                requests * TOKENS_PER_REQUEST,
                cached_input_tokens,
                cache_write_tokens,
                1000
            )
        );
        let revision: i64 =
            sqlx::query_scalar("SELECT revision FROM usage_change_state WHERE singleton")
                .fetch_one(&test.pool)
                .await
                .unwrap();
        assert_eq!(revision, before + requests);
        let journals: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM usage_commit_rows),(SELECT count(*) FROM commit_work)",
        )
        .fetch_one(&test.pool)
        .await
        .unwrap();
        assert_eq!(journals, (0, 0));
        test.close().await.unwrap();
    }
}
