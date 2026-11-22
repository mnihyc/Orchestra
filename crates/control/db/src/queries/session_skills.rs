//! Immutable minimal skill decisions paired with session openings, independent of command retention.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_api_contract::skills::SessionSkillsSummary;
use orch_types::{Id, Json, Time};

/// The first decision for an opening; arrays contain references and reasons, never files or archives.
pub struct NewOutcome<'a> {
    pub revision: Id,
    pub thread: Id,
    pub segment: Id,
    pub at: DateTime<Utc>,
    pub supported: bool,
    pub fingerprint: &'a str,
    pub selected: &'a Json,
    pub skipped: &'a Json,
}

/// Run in the command-creation transaction. Existing commands without a captured decision stay unreported.
pub async fn prepare(conn: &mut Conn, value: &NewOutcome<'_>) -> Result<bool, DbError> {
    let inserted = sqlx::query("INSERT INTO session_skill_outcomes(revision,thread_id,segment_id,requested_at,supported,fingerprint,selected,skipped)
      SELECT $1,$2,s.id,$4,$5,$6,$7,$8 FROM segments s WHERE s.id=$3 AND s.thread_id=$2
        AND NOT EXISTS(SELECT 1 FROM commands WHERE id=$1)
      ON CONFLICT(revision) DO NOTHING")
        .bind(value.revision).bind(value.thread).bind(value.segment).bind(value.at)
        .bind(value.supported).bind(value.fingerprint).bind(value.selected).bind(value.skipped)
        .execute(&mut *conn).await?.rows_affected()==1;
    if inserted {
        sqlx::query("UPDATE segments SET session_skills=NULL WHERE id=$1")
            .bind(value.segment)
            .execute(conn)
            .await?;
    }
    Ok(inserted)
}

/// Safe header fields without decoding the private reference arrays.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Outcome {
    pub position: i64,
    pub revision: Id,
    pub thread_id: Id,
    pub segment_id: Id,
    pub applied_at: Option<DateTime<Utc>>,
    pub applied_seq: Option<i64>,
    pub supported: bool,
    pub fingerprint: String,
    pub selected_count: i64,
    pub skipped_count: i64,
}

const HEADER: &str = "SELECT position,revision,thread_id,segment_id,applied_at,applied_seq,supported,fingerprint,
    jsonb_array_length(selected)::bigint AS selected_count,jsonb_array_length(skipped)::bigint AS skipped_count
    FROM session_skill_outcomes";

/// Only a known successful opening publishes the small session projection in that same transaction.
pub async fn accept(
    conn: &mut Conn,
    revision: Id,
    segment: Id,
    seq: i64,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE session_skill_outcomes SET applied_at=COALESCE(applied_at,$3),applied_seq=COALESCE(applied_seq,$4)
      WHERE revision=$1 AND segment_id=$2")
        .bind(revision).bind(segment).bind(at).bind(seq).execute(&mut *conn).await?;
    if let Some(row) = by_revision(conn, revision)
        .await?
        .filter(|row| row.segment_id == segment)
    {
        if let Some(applied_at) = row.applied_at {
            let summary = SessionSkillsSummary {
                session_revision: revision,
                segment_id: segment,
                applied_at: Time::new(applied_at),
                supported: row.supported,
                selected: row.selected_count,
                skipped: row.skipped_count,
            };
            sqlx::query("UPDATE segments SET session_skills=$2,seq_updated=$3 WHERE id=$1
              AND NOT EXISTS(SELECT 1 FROM session_skill_outcomes later WHERE later.segment_id=$1 AND later.position>$4)")
                .bind(segment).bind(serde_json::json!(summary)).bind(seq).bind(row.position).execute(conn).await?;
        }
    } else {
        sqlx::query("UPDATE segments SET session_skills=NULL WHERE id=$1")
            .bind(segment)
            .execute(conn)
            .await?;
    }
    Ok(())
}

/// A revision fixes every continuation page even if another session opens meanwhile.
pub async fn by_revision(conn: &mut Conn, revision: Id) -> Result<Option<Outcome>, DbError> {
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{HEADER} WHERE revision=$1")))
            .bind(revision)
            .fetch_optional(conn)
            .await?,
    )
}

/// References in the accepted immutable order, selected first then skipped, with no material bytes.
pub async fn page(
    conn: &mut Conn,
    revision: Id,
    after: i64,
    limit: i64,
) -> Result<Vec<(i64, bool, Json)>, DbError> {
    Ok(sqlx::query_as("WITH sized AS MATERIALIZED (
      SELECT selected,skipped,jsonb_array_length(selected)::bigint AS n,
        jsonb_array_length(selected)::bigint+jsonb_array_length(skipped)::bigint AS total FROM session_skill_outcomes WHERE revision=$1),
      bounded AS(SELECT *,LEAST(GREATEST($2,0),total) AS before FROM sized)
      SELECT p,p<=n,CASE WHEN p<=n THEN selected->((p-1)::int) ELSE skipped->((p-n-1)::int) END
        FROM bounded,LATERAL generate_series(before+1,before+LEAST($3,total-before)) p ORDER BY p")
        .bind(revision).bind(after).bind(limit).fetch_all(conn).await?)
}

/// Reused sessions keep their accepted versions, regardless of a subsequently requested setting.
pub async fn record_turn(conn: &mut Conn, turn: Id, segment: Id) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE turns t SET skill_versions=(SELECT COALESCE(array_agg((e.value->>'version_id')::uuid ORDER BY e.ordinality) FILTER (WHERE e.value IS NOT NULL),'{}'::uuid[])
      FROM session_skill_outcomes o LEFT JOIN LATERAL jsonb_array_elements(o.selected) WITH ORDINALITY e ON true
      WHERE o.revision=(s.session_skills->>'session_revision')::uuid)
      FROM segments s WHERE t.id=$1 AND t.segment_id=$2 AND s.id=$2 AND s.session_skills IS NOT NULL")
        .bind(turn).bind(segment).execute(conn).await?.rows_affected()==1)
}
