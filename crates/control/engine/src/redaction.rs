//! Native presentation is masked before timeline storage or live delivery.
//!
//! Native identities and content addresses keep their meaning. Observation/capture code receives
//! original operands; the ordinary presentation receives only masked content. Cross-fragment
//! assertion context is private ciphertext in the same transaction as the stream cursor.
use crate::raw_events;
use crate::streaming;
use crate::unit::Unit;
use crate::{Engine, EngineError};
use futures::StreamExt;
use orch_chat::redaction::Redactor;
use orch_db::queries::threads::{self, ItemChange, ThreadRow};
use orch_db::{Conn, DbError};
use orch_link_contract::events::{DeltaField, ItemDelta};
use orch_types::{Hash, Id, Json};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(crate) async fn load(
    conn: &mut Conn,
    row: &ThreadRow,
    environment: Option<Id>,
) -> Result<Redactor, EngineError> {
    raw_events::live_redactor(conn, row, environment)
        .await?
        .ok_or_else(|| {
            EngineError::unreadable(
                "native text is waiting because its masking inputs could not be opened",
            )
        })
}

/// Captured protection stays in force for the item; current rules may only add protection.
pub(crate) async fn remember(
    conn: &mut Conn,
    item: Id,
    current: &Redactor,
) -> Result<Redactor, EngineError> {
    let encrypted: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT rules FROM item_capture_masks WHERE item_id=$1")
            .bind(item)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let mut merged = current.clone();
    let binding = format!("native-mask-rules:{item}");
    let mut previous = None;
    if let Some(encrypted) = encrypted {
        let bytes = raw_events::master_key()
            .map_err(EngineError::unreadable)?
            .open(&encrypted, binding.as_bytes())
            .map_err(EngineError::unreadable)?;
        let old = Redactor::restore_native(&bytes).map_err(EngineError::unreadable)?;
        merged.extend_native(&old);
        previous = Some(bytes);
    }
    if merged.has_rules() {
        let bytes = merged.native_snapshot()?;
        if previous.as_ref() != Some(&bytes) {
            let encrypted = raw_events::master_key()
                .map_err(EngineError::unreadable)?
                .seal(&bytes, binding.as_bytes())
                .map_err(EngineError::unreadable)?;
            sqlx::query("INSERT INTO item_capture_masks(item_id,rules) VALUES($1,$2) ON CONFLICT(item_id) DO UPDATE SET rules=excluded.rules")
                .bind(item).bind(encrypted).execute(conn).await.map_err(DbError::from)?;
        }
    }
    Ok(merged)
}

pub(crate) async fn has_closed(conn: &mut Conn, thread: Id) -> Result<bool, EngineError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM item_redaction_holds h JOIN items i ON i.id=h.item_id WHERE i.thread_id=$1 AND i.status<>'in_progress' AND (i.turn_id IS NULL OR EXISTS(SELECT 1 FROM turns t WHERE t.id=i.turn_id AND (t.native_end_confirmed OR t.error->>'code'='session_lost'))))")
        .bind(thread).fetch_one(conn).await.map_err(DbError::from)?)
}

// These members are protocol identities or typed discriminants, rather than presentation text.
fn identity(name: &str) -> bool {
    matches!(
        name,
        "kind"
            | "type"
            | "status"
            | "state"
            | "phase"
            | "code"
            | "marker"
            | "change"
            | "action"
            | "blob"
            | "hash"
            | "media_type"
            | "native_ref"
            | "native_item_ref"
            | "process_id"
            | "stream"
            | "id"
            | "thread_id"
            | "turn_id"
            | "segment_id"
            | "provider_id"
            | "agent_id"
            | "parent_item_id"
            | "path"
            | "cwd"
            | "directory"
            | "url"
            | "file"
            | "origin_turn"
            | "run_id"
            | "started_at"
            | "completed_at"
            | "at"
    )
}
fn strings(masker: &Redactor, value: &mut Json, key: &str, opaque: bool) {
    match value {
        Json::String(text) => {
            if !opaque && identity(key) {
                if text.contains('\0') {
                    *text = text.replace('\0', "*");
                }
            } else {
                *text = masker.mask_native(text, key == "inline");
            }
        }
        Json::Array(values) => {
            for value in values {
                strings(masker, value, key, opaque);
            }
        }
        Json::Object(values) => {
            let opaque = opaque || matches!(key, "arguments" | "result");
            if opaque {
                for (key, mut value) in std::mem::take(values) {
                    strings(masker, &mut value, &key, true);
                    let base = masker.mask_native(&key, false);
                    let mut display = base.clone();
                    let mut ordinal = 2usize;
                    while values.contains_key(&display) {
                        display = format!("{base} ({ordinal})");
                        ordinal += 1;
                    }
                    values.insert(display, value);
                }
            } else {
                for (key, value) in values {
                    strings(masker, value, key, false);
                }
            }
        }
        _ => {}
    }
}
fn fields(payload: &Json) -> Vec<(DeltaField, String, i64)> {
    let mut values = Vec::new();
    for (name, field) in [("text", DeltaField::Text), ("summary", DeltaField::Summary)] {
        if let Some(text) = payload.get(name).and_then(Json::as_str) {
            values.push((field, text.into(), crate::ingest::code_points(text)));
        }
    }
    if let Some(text) = payload
        .pointer("/tool/output/inline")
        .and_then(Json::as_str)
    {
        let total = payload
            .pointer("/tool/output/total_len")
            .and_then(Json::as_i64)
            .unwrap_or(0);
        values.push((DeltaField::Output, text.into(), total));
    }
    values
}
fn put_field(payload: &mut Json, field: DeltaField, text: String) {
    match field {
        DeltaField::Text => {
            payload["text_len"] = Json::from(crate::ingest::code_points(&text));
            payload["text"] = Json::String(text);
        }
        DeltaField::Summary => {
            payload["summary"] = Json::String(text);
        }
        DeltaField::Output => {
            payload["tool"]["output"]["inline"] = Json::String(text);
        }
    }
}
fn units(text: &str, field: DeltaField) -> i64 {
    match field {
        DeltaField::Output => i64::try_from(text.len()).unwrap_or(i64::MAX),
        _ => crate::ingest::code_points(text),
    }
}
fn skip(text: &str, field: DeltaField, count: i64) -> String {
    if field == DeltaField::Output {
        let mut after = usize::try_from(count).unwrap_or(usize::MAX).min(text.len());
        while !text.is_char_boundary(after) {
            after += 1;
        }
        text[after..].into()
    } else {
        text.chars()
            .skip(usize::try_from(count).unwrap_or(usize::MAX))
            .collect()
    }
}

impl Engine {
    pub(crate) async fn native_payload(
        &self,
        conn: &mut Conn,
        masker: &Redactor,
        original: &Json,
        growing: bool,
    ) -> Result<Json, EngineError> {
        let mut payload = original.clone();
        strings(masker, &mut payload, "", false);
        if growing && masker.has_rules() {
            for (field, text, _) in fields(original) {
                put_field(
                    &mut payload,
                    field,
                    masker.mask_native_start(&text, field == DeltaField::Output),
                );
            }
        }
        // Only declared text references are read. Opaque tool arguments never grant byte access.
        if masker.has_rules() {
            let mut paths = vec!["/raw/input", "/raw/output"];
            match original.pointer("/tool/kind").and_then(Json::as_str) {
                Some("command") => paths.push("/tool/output"),
                Some("web_fetch") => paths.push("/tool/content"),
                Some("file_read")
                    if original
                        .pointer("/tool/media_type")
                        .and_then(Json::as_str)
                        .is_none_or(|media| media.starts_with("text/")) =>
                {
                    paths.push("/tool/content")
                }
                _ => {}
            }
            let mut masked = BTreeMap::new();
            for path in paths {
                let Some(hash) = original
                    .pointer(path)
                    .and_then(|value| value.get("blob"))
                    .and_then(Json::as_str)
                    .and_then(|hash| hash.parse::<Hash>().ok())
                else {
                    continue;
                };
                let next = if let Some(hash) = masked.get(&hash) {
                    *hash
                } else {
                    let store = self.blobs().ok_or_else(|| {
                        EngineError::unreadable(
                            "native retained text is waiting for its content store",
                        )
                    })?;
                    let mut read = store
                        .get(&hash, None)
                        .await
                        .map_err(EngineError::unreadable)?;
                    let mut bytes = Vec::new();
                    while let Some(part) = read.body.next().await {
                        bytes.extend_from_slice(&part.map_err(EngineError::unreadable)?);
                    }
                    let text = String::from_utf8(bytes).map_err(EngineError::unreadable)?;
                    let text = masker.mask_native(&text, true);
                    let next = Hash::of(text.as_bytes());
                    let size = i64::try_from(text.len()).map_err(EngineError::unreadable)?;
                    store
                        .put(
                            &next,
                            Some(size),
                            orch_blob_contract::store::stream_of(vec![bytes::Bytes::from(text)]),
                        )
                        .await
                        .map_err(EngineError::unreadable)?;
                    orch_db::queries::storage::register_blob(
                        conn,
                        &next,
                        size,
                        self.inner.clock.now(),
                    )
                    .await?;
                    masked.insert(hash, next);
                    next
                };
                if let Some(value) = payload.pointer_mut(path) {
                    value["blob"] = Json::String(next.to_string());
                }
            }
        }
        Ok(payload)
    }
}

#[derive(Serialize, Deserialize)]
struct Buffered {
    data: String,
    masked: Vec<(usize, usize)>,
    shown: i64,
    whole: bool,
}
fn binding(item: Id, field: DeltaField, offset: i64, len: i64) -> Vec<u8> {
    format!(
        "native-mask:{item}:{}:{offset}:{len}",
        streaming::field_name(field)
    )
    .into_bytes()
}
fn seal(
    item: Id,
    field: DeltaField,
    offset: i64,
    len: i64,
    value: &Buffered,
) -> Result<Vec<u8>, EngineError> {
    let data = serde_json::to_vec(value)?;
    raw_events::master_key()
        .map_err(EngineError::unreadable)?
        .seal(&data, &binding(item, field, offset, len))
        .map_err(EngineError::unreadable)
}
fn open(
    item: Id,
    field: DeltaField,
    offset: i64,
    len: i64,
    data: &[u8],
) -> Result<Buffered, EngineError> {
    let data = raw_events::master_key()
        .map_err(EngineError::unreadable)?
        .open(data, &binding(item, field, offset, len))
        .map_err(EngineError::unreadable)?;
    let buffered: Buffered = serde_json::from_slice(&data)?;
    if buffered.shown < 0
        || buffered.shown > units(&buffered.data, field)
        || buffered.masked.iter().any(|(first, last)| {
            first > last
                || *last > buffered.data.len()
                || !buffered.data.is_char_boundary(*first)
                || !buffered.data.is_char_boundary(*last)
        })
    {
        return Err(EngineError::unreadable(
            "the private native masking context is invalid",
        ));
    }
    Ok(buffered)
}
async fn append(
    conn: &mut Conn,
    item: Id,
    segment: Id,
    field: DeltaField,
    offset: i64,
    len: i64,
    value: &Buffered,
) -> Result<(), EngineError> {
    let data = seal(item, field, offset, len, value)?;
    sqlx::query("INSERT INTO item_redaction_holds(item_id,field,part_offset,part_len,data,source_segment) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(item).bind(streaming::field_name(field)).bind(offset).bind(len).bind(data).bind(segment).execute(conn).await.map_err(DbError::from)?;
    Ok(())
}
pub(crate) async fn clear(conn: &mut Conn, item: Id) -> Result<(), EngineError> {
    sqlx::query("DELETE FROM item_redaction_holds WHERE item_id=$1")
        .bind(item)
        .execute(conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

/// A growing snapshot supplies assertion context even when its text was already displayed.
pub(crate) async fn seed(
    conn: &mut Conn,
    item: Id,
    segment: Id,
    payload: &Json,
    masker: &Redactor,
    changed: Option<&Json>,
) -> Result<(), EngineError> {
    if !masker.has_rules() {
        return Ok(());
    }
    for (field, text, total) in fields(payload) {
        if let Some(changed) = changed {
            let present = match field {
                DeltaField::Text => changed.get("text").is_some(),
                DeltaField::Summary => changed.get("summary").is_some(),
                DeltaField::Output => changed.pointer("/tool/output").is_some(),
            };
            if !present {
                continue;
            }
        }
        sqlx::query("DELETE FROM item_redaction_holds WHERE item_id=$1 AND field=$2")
            .bind(item)
            .bind(streaming::field_name(field))
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
        let (cut, whole) = masker.native_cut(&text);
        if cut == text.len() {
            continue;
        }
        let data = text[cut..].to_owned();
        let shown = units(&data, field);
        append(
            conn,
            item,
            segment,
            field,
            total,
            0,
            &Buffered {
                data,
                masked: Vec::new(),
                shown,
                whole,
            },
        )
        .await?;
    }
    Ok(())
}

pub(crate) async fn delta(
    conn: &mut Conn,
    item: Id,
    segment: Id,
    visible: i64,
    original: &ItemDelta,
    masker: &Redactor,
) -> Result<Option<ItemDelta>, EngineError> {
    let field = original.field;
    let expected:Option<i64> = sqlx::query_scalar("SELECT part_offset+part_len FROM item_redaction_holds WHERE item_id=$1 AND field=$2 ORDER BY id DESC LIMIT 1")
        .bind(item).bind(streaming::field_name(field)).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
    if original.offset != expected.unwrap_or(visible) {
        return Ok(None);
    }
    if masker.native_open_ended() {
        let first:Option<(i64,i64,Vec<u8>)>=sqlx::query_as("SELECT part_offset,part_len,data FROM item_redaction_holds WHERE item_id=$1 AND field=$2 ORDER BY id LIMIT 1")
            .bind(item).bind(streaming::field_name(field)).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
        if let Some((offset, len, data)) = first
            && open(item, field, offset, len, &data)?.whole
        {
            append(
                conn,
                item,
                segment,
                field,
                original.offset,
                original.len,
                &Buffered {
                    data: original.data.clone(),
                    masked: Vec::new(),
                    shown: 0,
                    whole: true,
                },
            )
            .await?;
            return Ok(None);
        }
    }
    let rows:Vec<(i64,i64,Vec<u8>)>=sqlx::query_as("SELECT part_offset,part_len,data FROM item_redaction_holds WHERE item_id=$1 AND field=$2 ORDER BY id")
        .bind(item).bind(streaming::field_name(field)).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let mut raw = String::new();
    let mut masked = Vec::new();
    let mut shown = 0;
    for (offset, len, data) in &rows {
        let buffered = open(item, field, *offset, *len, data)?;
        if raw.is_empty() {
            shown = buffered.shown;
        }
        let base = raw.len();
        raw.push_str(&buffered.data);
        masked.extend(
            buffered
                .masked
                .into_iter()
                .map(|(first, last)| (base + first, base + last)),
        );
    }
    raw.push_str(&original.data);
    let (cut, deferred) = masker.native_cut(&raw);
    let (safe, pending) = masker.native_prefix(&raw, cut, field == DeltaField::Output, &masked);
    let safe_units = units(&raw[..cut], field);
    let remaining = raw[cut..].to_owned();
    let already_shown = (shown - safe_units).max(0);
    let private_units = units(&remaining, field) - already_shown;
    let end = original.offset + original.len;
    let published = (end - private_units).max(visible);
    sqlx::query("DELETE FROM item_redaction_holds WHERE item_id=$1 AND field=$2")
        .bind(item)
        .bind(streaming::field_name(field))
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    if !remaining.is_empty() {
        append(
            conn,
            item,
            segment,
            field,
            published,
            end - published,
            &Buffered {
                data: remaining,
                masked: pending,
                shown: already_shown,
                whole: deferred,
            },
        )
        .await?;
    }
    if published == visible {
        return Ok(None);
    }
    Ok(Some(ItemDelta {
        native_ref: original.native_ref.clone(),
        field,
        offset: visible,
        len: published - visible,
        data: skip(&safe, field, shown),
    }))
}

type ClosedField = (i64, String, Vec<(usize, usize)>, i64);

/// Terminal incomplete items release their private suffix only after the field truly ended.
pub(crate) async fn finish_closed(unit: &mut Unit<'_>, thread: Id) -> Result<(), EngineError> {
    let items:Vec<Id>=sqlx::query_scalar("SELECT DISTINCT i.id FROM items i JOIN item_redaction_holds h ON h.item_id=i.id WHERE i.thread_id=$1 AND i.status<>'in_progress' AND (i.turn_id IS NULL OR EXISTS(SELECT 1 FROM turns t WHERE t.id=i.turn_id AND (t.native_end_confirmed OR t.error->>'code'='session_lost'))) ORDER BY i.id")
        .bind(thread).fetch_all(&mut *unit.conn).await.map_err(DbError::from)?;
    for id in items {
        let Some(item) = threads::item(unit.conn, id).await? else {
            continue;
        };
        let row = threads::thread(unit.conn, thread)
            .await?
            .ok_or_else(|| EngineError::unreadable("the retained native text has no thread"))?;
        let environment = threads::segment(unit.conn, item.segment_id)
            .await?
            .map(|segment| segment.environment_id);
        let current = load(unit.conn, &row, environment).await?;
        let masker = remember(unit.conn, id, &current).await?;
        let rows:Vec<(String,i64,i64,Vec<u8>)>=sqlx::query_as("SELECT field,part_offset,part_len,data FROM item_redaction_holds WHERE item_id=$1 ORDER BY field,id").bind(id).fetch_all(&mut *unit.conn).await.map_err(DbError::from)?;
        let mut fields: BTreeMap<String, ClosedField> = BTreeMap::new();
        for (name, offset, len, data) in rows {
            let field = streaming::field_of(&name)?;
            let buffered = open(id, field, offset, len, &data)?;
            let value = fields.entry(name).or_default();
            if value.1.is_empty() {
                value.3 = buffered.shown;
            }
            let base = value.1.len();
            value.1.push_str(&buffered.data);
            value.2.extend(
                buffered
                    .masked
                    .into_iter()
                    .map(|(first, last)| (first + base, last + base)),
            );
            value.0 = offset + len;
        }
        let mut payload = item.payload.clone();
        for (name, (end, data, masked, shown)) in fields {
            let field = streaming::field_of(&name)?;
            let visible = streaming::length_of(&payload, field);
            if end > visible {
                let (text, _) =
                    masker.native_prefix(&data, data.len(), field == DeltaField::Output, &masked);
                streaming::append(
                    &mut payload,
                    field,
                    &skip(&text, field, shown),
                    end - visible,
                );
            }
        }
        if payload != item.payload {
            let seq = unit.stamp(thread).await?;
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
        }
        clear(unit.conn, id).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
