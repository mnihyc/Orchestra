//! Discovering the upload of a command stops after two recent matches even with many older uploads.

use chrono::{Duration, Utc};
use orch_db::queries::{resources, storage};
use orch_testkit::database::TestDb;
use orch_types::{Hash, Json, new_id};

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// Enough rows to expose a scan, with half on the queried machine but outside the time range.
const UPLOADS: i64 = 100_000;

fn bounded(node: &Json) {
    let kind = node["Node Type"].as_str().unwrap_or_default();
    assert!(
        !kind.contains("Seq Scan") && !kind.contains("Sort"),
        "{node}"
    );
    let touched = (node["Actual Rows"].as_f64().unwrap_or_default()
        + node["Rows Removed by Filter"].as_f64().unwrap_or_default())
        * node["Actual Loops"].as_f64().unwrap_or(1.0);
    assert!(
        touched <= 2.0,
        "discovery visits only its two matches: {node}"
    );
    if let Some(children) = node["Plans"].as_array() {
        for child in children {
            bounded(child);
        }
    }
}

#[tokio::test]
async fn a_jobs_upload_is_distinguished_without_scanning_older_or_other_uploads() -> Proof {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await?;
    let machine = resources::testing::insert_machine(&mut conn, "source", "connected").await?;
    let other = resources::testing::insert_machine(&mut conn, "other", "connected").await?;
    let at = Utc::now();
    let since = at - Duration::seconds(30);
    sqlx::query(
        "INSERT INTO blob_uploads(hash,size,received,machine_id,started_at,updated_at,command_id)
         SELECT decode(md5(n::text)||md5('upload'||n),'hex'),10000,1,
                CASE WHEN n%2=0 THEN $1 ELSE $2 END, $3,
                CASE WHEN n%2=0 THEN $3-interval '2 days' ELSE $3 END,
                md5('command'||n)::uuid
         FROM generate_series(1,$4::bigint) g(n)",
    )
    .bind(machine)
    .bind(other)
    .bind(at)
    .bind(UPLOADS)
    .execute(&mut *conn)
    .await?;
    sqlx::query("ANALYZE blob_uploads")
        .execute(&mut *conn)
        .await?;

    for expected in 0..=2 {
        if expected > 0 {
            let hash = Hash::from_bytes([expected; 32]);
            assert!(storage::begin_upload(&mut conn, &hash, 10000, Some(machine), at).await?);
        }
        let found = storage::uploads_of_machine(&mut conn, machine, since, 2).await?;
        assert_eq!(found.len(), usize::from(expected));
        assert!(found.iter().all(|row| row.machine_id == Some(machine)));
        let plan: Json = sqlx::query_scalar(
            "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON)
             SELECT hash,size,received,machine_id,user_id,started_at,updated_at FROM blob_uploads
             WHERE machine_id=$1 AND updated_at>=$2 ORDER BY updated_at LIMIT 2",
        )
        .bind(machine)
        .bind(since)
        .fetch_one(&mut *conn)
        .await?;
        bounded(&plan[0]["Plan"]);
        assert!(
            plan.to_string()
                .contains("blob_uploads_machine_updated_idx"),
            "the machine/time range is indexed: {plan}"
        );
    }
    let command = new_id();
    for expected in 0..=2 {
        if expected > 0 {
            let hash = Hash::from_bytes([expected; 32]);
            storage::advance_upload(&mut conn, &hash, 1, Some(command), at).await?;
        }
        let found = storage::uploads_of_command(&mut conn, machine, command, 2).await?;
        assert_eq!(found.len(), usize::from(expected));
        for row in &found {
            assert!(
                storage::upload_of_command(&mut conn, machine, command, &row.hash)
                    .await?
                    .is_some()
            );
            assert!(
                storage::upload_of_command(&mut conn, machine, new_id(), &row.hash)
                    .await?
                    .is_none()
            );
            assert!(
                storage::upload_of_command(&mut conn, other, command, &row.hash)
                    .await?
                    .is_none()
            );
        }
        let plan: Json = sqlx::query_scalar(
            "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON)
             SELECT hash,size,received,machine_id,user_id,started_at,updated_at FROM blob_uploads
             WHERE machine_id=$1 AND command_id=$2 ORDER BY hash LIMIT 2",
        )
        .bind(machine)
        .bind(command)
        .fetch_one(&mut *conn)
        .await?;
        bounded(&plan[0]["Plan"]);
        assert!(
            plan.to_string()
                .contains("blob_uploads_machine_command_idx"),
            "{plan}"
        );
    }
    Ok(())
}
