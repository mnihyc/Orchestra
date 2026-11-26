//! Durable context contributions advance from changed source identities, never from a replay of history.

use super::{EngineError, Family, SentBlock, TextKind, exchange_most_bytes};
use orch_db::{Conn, DbError};
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Totals {
    pub inputs: BTreeMap<String, i64>,
    pub said: i64,
    pub tools: BTreeMap<String, (i64, i64, i64)>,
    pub images: i64,
}

fn changed(was: i64, value: i64, add: bool) -> Result<i64, EngineError> {
    if add {
        was.checked_add(value)
    } else {
        was.checked_sub(value)
    }
    .ok_or_else(|| {
        EngineError::unreadable("a context contribution exceeds its integer representation")
    })
}

impl Totals {
    fn apply(&mut self, value: &Self, add: bool) -> Result<(), EngineError> {
        self.said = changed(self.said, value.said, add)?;
        self.images = changed(self.images, value.images, add)?;
        for (key, tokens) in &value.inputs {
            let entry = self.inputs.entry(key.clone()).or_default();
            *entry = changed(*entry, *tokens, add)?;
        }
        for (name, (items, call, result)) in &value.tools {
            let entry = self.tools.entry(name.clone()).or_default();
            entry.0 = changed(entry.0, *items, add)?;
            entry.1 = changed(entry.1, *call, add)?;
            entry.2 = changed(entry.2, *result, add)?;
        }
        Ok(())
    }

    fn prune(&mut self) {
        self.inputs.retain(|_, tokens| *tokens != 0);
        self.tools.retain(|_, value| *value != (0, 0, 0));
    }
}

#[derive(sqlx::FromRow)]
struct Item {
    id: Id,
    kind: String,
    name: String,
    tool: String,
    said: i64,
    call_bytes: i64,
    result_bytes: i64,
    images: i64,
}

/// The caller's savepoint contains every source read, contribution replacement and dirty-queue removal.
pub(super) async fn read(
    conn: &mut Conn,
    segment: Id,
    since: i64,
    family: Family,
) -> Result<Totals, EngineError> {
    sqlx::query(
        "INSERT INTO context_estimates(segment_id,totals) VALUES($1,$2) ON CONFLICT DO NOTHING",
    )
    .bind(segment)
    .bind(serde_json::to_value(Totals::default())?)
    .execute(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let (held_family, boundary, value): (Option<Json>, i64, Json) = sqlx::query_as(
        "SELECT family,since,totals FROM context_estimates WHERE segment_id=$1 FOR UPDATE",
    )
    .bind(segment)
    .fetch_one(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let measured = serde_json::to_value(family)?;
    let pending: Vec<(String, Id, i64)> =
        sqlx::query_as("SELECT source,id,revision FROM context_estimate_dirty WHERE segment_id=$1")
            .bind(segment)
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let rebuild = held_family.as_ref() != Some(&measured)
        || boundary != since
        || pending.iter().any(|(source, _, _)| source == "refresh");
    let mut totals = if rebuild {
        Totals::default()
    } else {
        serde_json::from_value(value)?
    };
    let dirty: Vec<(String, Id)> = if rebuild {
        sqlx::query("DELETE FROM context_estimate_contributions WHERE segment_id=$1")
            .bind(segment)
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
        sqlx::query_as("SELECT 'input'::text,id FROM model_inputs WHERE segment_id=$1 AND purpose='input' AND state<>'failed' AND seq_created>$2 UNION ALL SELECT 'item'::text,id FROM items WHERE segment_id=$1 AND stream='main' AND NOT before_orch AND seq_created>$2 AND kind IN ('agent_message','tool_call','user_message')")
            .bind(segment).bind(since).fetch_all(&mut *conn).await.map_err(DbError::from)?
    } else {
        pending
            .iter()
            .map(|(source, id, _)| (source.clone(), *id))
            .collect()
    };
    if dirty.is_empty() && !rebuild {
        return Ok(totals);
    }
    let inputs = dirty
        .iter()
        .filter(|(source, _)| source == "input")
        .map(|(_, id)| *id)
        .collect::<Vec<_>>();
    let items = dirty
        .iter()
        .filter(|(source, _)| source == "item")
        .map(|(_, id)| *id)
        .collect::<Vec<_>>();
    if !rebuild {
        let previous: Vec<Json> = sqlx::query_scalar("DELETE FROM context_estimate_contributions WHERE segment_id=$1 AND (source='input' AND id=ANY($2) OR source='item' AND id=ANY($3)) RETURNING value")
            .bind(segment).bind(&inputs).bind(&items).fetch_all(&mut *conn).await.map_err(DbError::from)?;
        for value in previous {
            totals.apply(&serde_json::from_value(value)?, false)?;
        }
    }
    let mut next = Vec::new();
    if !inputs.is_empty() {
        let rows: Vec<(Id, Json)> = sqlx::query_as("SELECT id,blocks FROM model_inputs WHERE id=ANY($1) AND segment_id=$2 AND purpose='input' AND state<>'failed' AND seq_created>$3")
            .bind(inputs).bind(segment).bind(since).fetch_all(&mut *conn).await.map_err(DbError::from)?;
        for (id, blocks) in rows {
            let mut contribution = Totals::default();
            for block in serde_json::from_value::<Vec<SentBlock>>(blocks).unwrap_or_default() {
                let tokens = block.tokens(family, TextKind::Prose);
                let entry = contribution.inputs.entry(block.key).or_default();
                *entry = changed(*entry, tokens, true)?;
            }
            totals.apply(&contribution, true)?;
            next.push(serde_json::json!({"id":id,"source":"input","value":contribution}));
        }
    }
    if !items.is_empty() {
        let rows: Vec<Item> = sqlx::query_as(
            "SELECT id,kind,COALESCE(payload->'raw'->>'name','') AS name,COALESCE(payload->'tool'->>'kind','') AS tool,
                CASE WHEN kind='agent_message' THEN octet_length(COALESCE(payload->>'text',''))::bigint ELSE 0::bigint END AS said,
                CASE WHEN kind='tool_call' AND jsonb_typeof(payload->'raw'->'input'->'total_len')='number' THEN LEAST((payload->'raw'->'input'->>'total_len')::bigint,$4) ELSE 0::bigint END AS call_bytes,
                CASE WHEN kind='tool_call' AND jsonb_typeof(payload->'raw'->'output'->'total_len')='number' THEN LEAST((payload->'raw'->'output'->>'total_len')::bigint,$4) ELSE 0::bigint END AS result_bytes,
                CASE WHEN kind='user_message' AND jsonb_typeof(payload->'parts')='array' THEN (SELECT count(*) FROM jsonb_array_elements(payload->'parts') AS part WHERE part->>'type'='image') ELSE 0::bigint END AS images
             FROM items WHERE id=ANY($1) AND segment_id=$2 AND stream='main' AND NOT before_orch AND seq_created>$3 AND kind IN ('agent_message','tool_call','user_message')",
        ).bind(items).bind(segment).bind(since).bind(exchange_most_bytes(family)).fetch_all(&mut *conn).await.map_err(DbError::from)?;
        for row in rows {
            let mut contribution = Totals {
                said: row.said,
                images: row.images,
                ..Totals::default()
            };
            if row.kind == "tool_call" {
                if row.tool == "image" {
                    contribution.images = changed(contribution.images, 1, true)?;
                } else {
                    contribution
                        .tools
                        .insert(row.name, (1, row.call_bytes, row.result_bytes));
                }
            }
            totals.apply(&contribution, true)?;
            next.push(serde_json::json!({"id":row.id,"source":"item","value":contribution}));
        }
    }
    totals.prune();
    if !next.is_empty() {
        sqlx::query("INSERT INTO context_estimate_contributions(segment_id,source,id,value) SELECT $1,entry.source,entry.id,entry.value FROM jsonb_to_recordset($2) AS entry(source text,id uuid,value jsonb)")
            .bind(segment).bind(Json::Array(next)).execute(&mut *conn).await.map_err(DbError::from)?;
    }
    sqlx::query("UPDATE context_estimates SET family=$2,since=$3,totals=$4 WHERE segment_id=$1")
        .bind(segment)
        .bind(measured)
        .bind(since)
        .bind(serde_json::to_value(&totals)?)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    // A later source write increments its revision instead of waiting for this estimate's scan.
    // Remove only the revisions observed before reading; concurrent replacements stay pending.
    if !pending.is_empty() {
        let observed = pending.into_iter().map(|(source,id,revision)|serde_json::json!({"source":source,"id":id,"revision":revision})).collect::<Vec<_>>();
        sqlx::query("DELETE FROM context_estimate_dirty d USING jsonb_to_recordset($2) AS observed(source text,id uuid,revision bigint) WHERE d.segment_id=$1 AND d.source=observed.source AND d.id=observed.id AND d.revision=observed.revision")
            .bind(segment).bind(Json::Array(observed)).execute(&mut *conn).await.map_err(DbError::from)?;
    }
    Ok(totals)
}
