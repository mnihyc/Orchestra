//! Durable streamed stretches, coalesced into full item rows at the configured interval.
//!
//! A piece and the link cursor acknowledging it share the caller's transaction. Live delivery
//! may precede that commit; until it commits the machine still owns the unacknowledged event.
//! After commit the database owns the piece, including across process failure. Coalescing
//! rewrites the full item once per interval instead of once per native fragment.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::ingest::{OUTPUT_INLINE_MAX_BYTES, code_points, tail_of};
use crate::unit::Unit;
use orch_db::queries::threads::{self, ItemChange};
use orch_db::{Conn, DbError};
use orch_link_contract::events::{DeltaField, ItemDelta};
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

pub(crate) fn field_name(field: DeltaField) -> &'static str {
    match field {
        DeltaField::Text => "text",
        DeltaField::Summary => "summary",
        DeltaField::Output => "output",
    }
}
pub(crate) fn field_of(field: &str) -> Result<DeltaField, EngineError> {
    match field {
        "text" => Ok(DeltaField::Text),
        "summary" => Ok(DeltaField::Summary),
        "output" => Ok(DeltaField::Output),
        _ => Err(EngineError::unreadable(
            "the streamed field is not supported",
        )),
    }
}

/// Takes a contiguous piece once. Database state, never volatile memory, decides its next offset.
pub(crate) async fn hold(
    conn: &mut Conn,
    item: Id,
    segment: Id,
    base: i64,
    delta: &ItemDelta,
) -> Result<bool, EngineError> {
    if delta.offset < 0 || delta.len <= 0 || delta.offset.checked_add(delta.len).is_none() {
        return Ok(false);
    }
    let field = field_name(delta.field);
    if delta.offset != base {
        return Ok(false);
    }
    let data = &delta.data;
    Ok(sqlx::query("INSERT INTO item_stream_parts(item_id,field,part_offset,part_len,data,source_segment) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
        .bind(item).bind(field).bind(delta.offset).bind(delta.len).bind(data.as_bytes()).bind(segment).execute(conn).await.map_err(DbError::from)?.rows_affected() > 0)
}

/// How long `field` of an item's payload is, in the field's unit: code points of its text or summary, bytes of a
/// command's output.
pub(crate) fn length_of(payload: &Json, field: DeltaField) -> i64 {
    match field {
        DeltaField::Text => code_points(
            payload
                .get("text")
                .and_then(Json::as_str)
                .unwrap_or_default(),
        ),
        DeltaField::Summary => code_points(
            payload
                .get("summary")
                .and_then(Json::as_str)
                .unwrap_or_default(),
        ),
        DeltaField::Output => payload
            .pointer("/tool/output/total_len")
            .and_then(Json::as_i64)
            .unwrap_or(0),
    }
}

/// The accepted byte tail belongs to the native output, including when it is smaller than the default.
pub(crate) fn output_inline_bytes(output: &Json) -> usize {
    usize::try_from(
        output
            .get("inline_limit")
            .and_then(Json::as_u64)
            .unwrap_or(OUTPUT_INLINE_MAX_BYTES as u64),
    )
    .unwrap_or(usize::MAX)
}

/// Adds `data`, `len` long in the field's unit, to the end of `field` of an item's payload, as a piece of it would.
pub(crate) fn append(payload: &mut Json, field: DeltaField, data: &str, len: i64) {
    match field {
        DeltaField::Text | DeltaField::Summary => {
            let member = if field == DeltaField::Text {
                "text"
            } else {
                "summary"
            };
            let grown = format!(
                "{}{data}",
                payload
                    .get(member)
                    .and_then(Json::as_str)
                    .unwrap_or_default()
            );
            payload["text_len"] = json!(code_points(&grown));
            payload[member] = json!(grown);
        }
        DeltaField::Output => {
            let output = payload
                .pointer("/tool/output")
                .cloned()
                .unwrap_or(Json::Null);
            let total = output.get("total_len").and_then(Json::as_i64).unwrap_or(0);
            let joined = format!(
                "{}{data}",
                output
                    .get("inline")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
            );
            let kept = tail_of(&joined, output_inline_bytes(&output)).to_owned();
            let new_total = total + len;
            let whole = i64::try_from(kept.len()).unwrap_or(i64::MAX) >= new_total;
            let blob = output.get("blob").cloned().unwrap_or(Json::Null);
            payload["tool"]["output"] = json!({
                "inline": kept, "total_len": new_total, "truncated": !whole, "blob": blob, "retained":output.get("retained"), "inline_limit":output.get("inline_limit"),
            });
        }
    }
}

#[derive(Default)]
struct Part {
    base: i64,
    len: i64,
    data: String,
}

impl Engine {
    /// The caller holds the thread's row, so flushing and the next accepted piece remain one order.
    pub(crate) async fn write_held_of(
        &self,
        unit: &mut Unit<'_>,
        thread: Id,
    ) -> Result<(), EngineError> {
        let items:Vec<Id> = sqlx::query_scalar("SELECT DISTINCT p.item_id FROM item_stream_parts p JOIN items i ON i.id=p.item_id WHERE i.thread_id=$1 ORDER BY p.item_id")
            .bind(thread).fetch_all(&mut *unit.conn).await.map_err(DbError::from)?;
        for item in items {
            self.write_held_item(unit, item).await?;
        }
        Ok(())
    }

    async fn write_held_item(&self, unit: &mut Unit<'_>, id: Id) -> Result<(), EngineError> {
        let Some(item) = threads::item(unit.conn, id).await? else {
            return Ok(());
        };
        let rows:Vec<(String,i64,i64,Vec<u8>,Id)> = sqlx::query_as("SELECT field,part_offset,part_len,data,source_segment FROM item_stream_parts WHERE item_id=$1 ORDER BY field,part_offset")
            .bind(id).fetch_all(&mut *unit.conn).await.map_err(DbError::from)?;
        if rows.is_empty() {
            return Ok(());
        }
        let output_limit =
            output_inline_bytes(item.payload.pointer("/tool/output").unwrap_or(&Json::Null));
        let mut fields: BTreeMap<String, Part> = BTreeMap::new();
        let mut segments = BTreeSet::new();
        for (name, offset, len, bytes, segment) in rows {
            segments.insert(segment);
            let field = field_of(&name)?;
            let data = String::from_utf8(bytes).map_err(EngineError::unreadable)?;
            let part = fields.entry(name).or_insert_with(|| Part {
                base: offset,
                ..Part::default()
            });
            if offset != part.base + part.len {
                return Err(EngineError::unreadable(
                    "the retained streamed text has a gap",
                ));
            }
            part.data.push_str(&data);
            if field == DeltaField::Output {
                part.data = tail_of(&part.data, output_limit).to_owned();
            }
            part.len += len;
        }
        let mut payload = item.payload.clone();
        for (name, part) in fields {
            let field = field_of(&name)?;
            let now = length_of(&payload, field);
            if now > part.base + part.len {
                continue;
            }
            if now < part.base {
                return Err(EngineError::unreadable(
                    "the retained streamed text does not follow its item",
                ));
            }
            let skip = now - part.base;
            if skip < part.len {
                let data = match field {
                    DeltaField::Output => tail_of(
                        &part.data,
                        usize::try_from(part.len - skip).unwrap_or(usize::MAX),
                    )
                    .to_owned(),
                    DeltaField::Text | DeltaField::Summary => part
                        .data
                        .chars()
                        .skip(usize::try_from(skip).unwrap_or(usize::MAX))
                        .collect(),
                };
                append(&mut payload, field, &data, part.len - skip);
            }
        }
        if payload != item.payload {
            let seq = unit.stamp(item.thread_id).await?;
            let at = unit.now();
            threads::change_item(
                unit.conn,
                id,
                &ItemChange {
                    status: None,
                    payload: &payload,
                    search_text: None,
                    at,
                },
                seq,
            )
            .await?;
            for segment in segments {
                if orch_db::queries::thread_history::selected_segment(
                    unit.conn,
                    item.thread_id,
                    segment,
                )
                .await?
                {
                    threads::touch(unit.conn, item.thread_id, at).await?;
                    break;
                }
            }
        }
        sqlx::query("DELETE FROM item_stream_parts WHERE item_id=$1")
            .bind(id)
            .execute(&mut *unit.conn)
            .await
            .map_err(DbError::from)?;
        Ok(())
    }

    /// Restores pending pieces on startup, then coalesces due items without a process-local cache.
    pub(crate) async fn write_held_text(&self) {
        let every = self.inner.config.stream_flush;
        let tick = if every.is_zero() {
            self.inner.config.sweep_every
        } else {
            (every / 4).max(Duration::from_nanos(1))
        };
        let mut first = true;
        loop {
            let stopping = self.is_stopped();
            let wait = if first || stopping {
                Duration::ZERO
            } else {
                every
            };
            first = false;
            let due = async {
                let mut conn = self.db().acquire().await?;
                let items:Vec<(Id,Id)> = sqlx::query_as("SELECT DISTINCT i.id,i.thread_id FROM item_stream_parts p JOIN items i ON i.id=p.item_id WHERE p.captured_at <= now() - ($1::bigint * interval '1 millisecond') UNION SELECT DISTINCT i.id,i.thread_id FROM item_redaction_holds h JOIN items i ON i.id=h.item_id WHERE i.status<>'in_progress' AND (i.turn_id IS NULL OR EXISTS(SELECT 1 FROM turns t WHERE t.id=i.turn_id AND (t.native_end_confirmed OR t.error->>'code'='session_lost'))) ORDER BY 2,1")
                    .bind(i64::try_from(wait.as_millis()).unwrap_or(i64::MAX)).fetch_all(&mut *conn).await.map_err(DbError::from)?;
                Ok::<_,EngineError>(items)
            }.await;
            match due {
                Ok(items) => {
                    for (item, thread) in items {
                        if let Err(error) = self.write_alone(item, thread).await {
                            tracing::warn!(%error, "durable streamed text could not be coalesced; it will be retried");
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "durable streamed text could not be read; it will be retried")
                }
            }
            if stopping {
                return;
            }
            tokio::select! { () = tokio::time::sleep(tick) => {}, () = self.inner.signal.notified() => {} }
        }
    }

    async fn write_alone(&self, item: Id, thread: Id) -> Result<(), EngineError> {
        let mut tx = self.db().begin().await?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        if threads::lock(&mut tx, thread).await?.is_none() {
            return Ok(());
        }
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        self.write_held_item(&mut unit, item).await?;
        if crate::redaction::has_closed(unit.conn, thread).await? {
            self.write_held_of(&mut unit, thread).await?;
            crate::redaction::finish_closed(&mut unit, thread).await?;
        }
        unit.finish().await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }
}

#[cfg(test)]
mod durable_tests;
