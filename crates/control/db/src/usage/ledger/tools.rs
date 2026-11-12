//! Derived per-request tool facts from explicit native item associations, never overlapping time windows.
use super::*;

/// Retains this adapter's actual native tool-item references and refreshes only its canonical row.
pub async fn record(
    conn: &mut Conn,
    turn: Id,
    provider: Id,
    adapter: &str,
    upstream: Option<&str>,
    refs: &[String],
) -> Result<(), DbError> {
    let refs = serde_json::to_value(refs).unwrap_or_default();
    for table in ["usage_events", "usage_running"] {
        let sql = format!(
            "UPDATE {table} SET measurement_states=jsonb_set(measurement_states,'{{_tool_refs}}',$5) WHERE turn_id=$1 AND provider_id=$2 AND scope='request' AND (request_ref=$3 OR measurement_states->>'_adapter_request_ref'=$3 OR provider_request_id=$4) AND measurement_states->'_tool_refs' IS DISTINCT FROM $5 RETURNING id"
        );
        let ids: Vec<Id> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(turn)
            .bind(provider)
            .bind(adapter)
            .bind(upstream)
            .bind(&refs)
            .fetch_all(&mut *conn)
            .await?;
        refresh(conn, &ids).await?;
    }
    Ok(())
}

/// Replaces only derived facts whose actual associated item changed; the normal usage cursor carries them.
pub async fn item_changed(
    conn: &mut Conn,
    turn: Id,
    segment: Id,
    reference: &str,
) -> Result<(), DbError> {
    let ids:Vec<Id>=sqlx::query_scalar("SELECT id FROM usage_events WHERE turn_id=$1 AND segment_id=$2 AND measurement_states->'_tool_refs' ? $3 UNION ALL SELECT id FROM usage_running WHERE turn_id=$1 AND segment_id=$2 AND measurement_states->'_tool_refs' ? $3")
        .bind(turn).bind(segment).bind(reference).fetch_all(&mut *conn).await?;
    refresh(conn, &ids).await
}

pub(super) async fn refresh(conn: &mut Conn, ids: &[Id]) -> Result<(), DbError> {
    if ids.is_empty() {
        return Ok(());
    }
    for table in ["usage_events", "usage_running"] {
        let sql=format!("WITH facts AS (
            SELECT e.id,
                CASE WHEN count(r.reference)=count(i.id) THEN count(i.id)::bigint END AS tool_calls,
                CASE WHEN count(r.reference)=count(i.id)
                    AND count(i.id)=count(i.id) FILTER (WHERE jsonb_typeof(i.payload->'duration_ms')='number' AND (i.payload->>'duration_ms')::bigint>=0)
                    THEN COALESCE(sum((i.payload->>'duration_ms')::bigint),0)::bigint END AS tool_ms
            FROM {table} e
            LEFT JOIN LATERAL jsonb_array_elements_text(e.measurement_states->'_tool_refs') r(reference) ON true
            LEFT JOIN items i ON i.turn_id=e.turn_id AND i.segment_id=e.segment_id AND i.native_ref=r.reference AND i.kind='tool_call'
            WHERE e.id=ANY($1) AND jsonb_typeof(e.measurement_states->'_tool_refs')='array'
            GROUP BY e.id)
            UPDATE {table} e SET tool_calls=facts.tool_calls,tool_ms=facts.tool_ms FROM facts WHERE e.id=facts.id AND (e.tool_calls,e.tool_ms) IS DISTINCT FROM (facts.tool_calls,facts.tool_ms)");
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(ids)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}
