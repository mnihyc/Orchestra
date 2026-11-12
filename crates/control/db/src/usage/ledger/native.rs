//! Native aggregate/detail overlap is allocated once in the canonical ledger, without rewriting billing.
use super::*;
use std::collections::BTreeMap;

/// Quantity ownership and known billed fees remain independent when an aggregate had no quote.
#[derive(Debug, Default)]
pub struct Coverage {
    /// Units already represented by the native aggregate, whether priced or unpriced.
    pub quantities: BTreeMap<String, i64>,
    /// Of those units, the aggregate's frozen quote already supplied a known fee.
    pub priced: BTreeMap<String, i64>,
}

/// Both native publication orders use this transaction lock before measuring their unallocated units.
pub async fn lock(conn: &mut Conn, turn: Id, provider: Id) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("native_usage:{turn}:{provider}"))
        .execute(conn)
        .await?;
    Ok(())
}

/// A later real request consumes only the still-unallocated units of this exact source/model's aggregate.
/// Coverage is stored on the new request by the same writer; original observations and quotes stay intact.
pub async fn covered(
    conn: &mut Conn,
    turn: Id,
    provider: Id,
    model: &str,
    event: Id,
    requested: &BTreeMap<String, i64>,
) -> Result<Coverage, DbError> {
    if requested.is_empty() {
        return Ok(Coverage::default());
    }
    lock(conn, turn, provider).await?;
    let rows:Vec<(String,i64,bool)>=sqlx::query_as(
        "WITH available AS (
          SELECT s.key AS capability,(s.value#>>'{}')::numeric AS requested,
            COALESCE(sum(CASE WHEN e.scope='turn'
               AND e.measurement_states->'declared_counters'->s.key->>'state'='told'
               THEN (e.measurement_states->'declared_counters'->s.key->>'value')::numeric ELSE 0 END),0)
            - COALESCE(sum(CASE WHEN e.scope='request'
               THEN (e.measurement_states->'_native_counter_covered'->>s.key)::numeric ELSE 0 END),0) AS remaining,
            bool_and(CASE WHEN e.scope='turn'
               AND e.measurement_states->'declared_counters'->s.key->>'state'='told'
               THEN e.credits IS NOT NULL ELSE true END) AS priced
          FROM jsonb_each($5::jsonb) s LEFT JOIN usage_events e
            ON e.turn_id=$1 AND e.provider_id=$2 AND e.model=$3 AND e.id<>$4
             AND e.origin='harness' AND e.purpose=usage_turn_purpose($1) AND e.counted
          GROUP BY s.key,s.value
        ) SELECT capability,LEAST(requested,GREATEST(remaining,0))::bigint,priced FROM available
          WHERE requested>0 AND remaining>0")
        .bind(turn).bind(provider).bind(model).bind(event).bind(serde_json::json!(requested))
        .fetch_all(conn).await?;
    let mut result = Coverage::default();
    for (capability, count, priced) in rows {
        result.quantities.insert(capability.clone(), count);
        if priced {
            result.priced.insert(capability, count);
        }
    }
    Ok(result)
}
