//! Encrypted diagnostic fragments are joined only at their adapter-declared native end.
use super::*;
use crate::redaction::presentation;
use orch_types::Hash;
use serde::{Deserialize, Serialize};
use std::io::{Seek, SeekFrom, Write};

#[derive(Serialize, Deserialize)]
struct Part {
    pointer: String,
    text: String,
}

fn failure() -> EngineError {
    EngineError::unreadable("native trace masking is waiting for private storage")
}

fn status(value: &mut Json, state: Option<&str>) {
    if !value.is_object() {
        *value = serde_json::json!({"frame":std::mem::take(value)});
    }
    if let Some(object) = value.as_object_mut() {
        object.remove("_orch_capture");
        if let Some(state) = state {
            object.insert("_orch_capture".into(), Json::String(state.into()));
        }
    }
}

fn hidden(value: &mut Json) {
    match value {
        Json::String(text) => *text = "*".repeat(text.len()),
        Json::Array(values) => values.iter_mut().for_each(hidden),
        Json::Object(values) => values.values_mut().for_each(hidden),
        _ => {}
    }
}

pub(super) async fn prepare(
    conn: &mut Conn,
    segment: Id,
    frame: &RawFrame,
    masker: &Redactor,
) -> Result<Json, EngineError> {
    let mut value = frame.payload.clone();
    let mut captured = masker.clone();
    let mut streams = std::collections::BTreeSet::new();
    let mut ended = std::collections::BTreeSet::new();
    {
        for (index, span) in frame.text.iter().enumerate() {
            let stream = Hash::of(span.stream.as_bytes()).to_string();
            let binding = format!("native-trace-rules:{segment}:{stream}");
            let old: Option<Vec<u8>> = sqlx::query_scalar(
                "SELECT rules FROM native_trace_streams WHERE segment_id=$1 AND stream_hash=$2",
            )
            .bind(segment)
            .bind(&stream)
            .fetch_optional(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
            let mut rules = masker.clone();
            if let Some(old) = old {
                let bytes: Vec<u8> = presentation::open(&binding, &old)?;
                rules.extend_native(&Redactor::restore_native(&bytes).map_err(|_| failure())?);
            }
            if !rules.has_rules() {
                continue;
            }
            captured.extend_native(&rules);
            let sealed = presentation::seal(&binding, &rules.native_snapshot()?)?;
            sqlx::query("INSERT INTO native_trace_streams(segment_id,stream_hash,rules) VALUES($1,$2,$3) ON CONFLICT(segment_id,stream_hash) DO UPDATE SET rules=excluded.rules")
                .bind(segment).bind(&stream).bind(sealed).execute(&mut *conn).await.map_err(orch_db::DbError::from)?;
            if let Some(pointer) = &span.pointer {
                if let Some(Json::String(text)) = frame.payload.pointer(pointer) {
                    let part = i32::try_from(index).map_err(|_| failure())?;
                    let data = presentation::seal(
                        &format!("native-trace:{segment}:{}:{part}", frame.n),
                        &Part {
                            pointer: pointer.clone(),
                            text: text.clone(),
                        },
                    )?;
                    sqlx::query("INSERT INTO native_trace_parts(segment_id,stream_hash,n,part,data) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
                        .bind(segment).bind(&stream).bind(frame.n).bind(part).bind(data).execute(&mut *conn).await.map_err(orch_db::DbError::from)?;
                    if let Some(target) = value.pointer_mut(pointer) {
                        *target = Json::String("*".repeat(text.len()));
                    }
                    streams.insert(stream.clone());
                }
            }
            if span.end {
                ended.insert(stream);
            }
        }
        if captured.has_rules() && !frame.text_complete {
            hidden(&mut value);
        }
    }
    value = presentation::opaque(&value, &captured).value;
    super::mask_value(&mut value, &captured);
    status(
        &mut value,
        if !captured.has_rules() {
            None
        } else if !frame.text_complete {
            Some("unclassified")
        } else if !streams.is_empty() {
            Some("pending")
        } else {
            None
        },
    );
    // Finish after this frame is inserted; the private end marker is transaction-bound.
    for stream in ended {
        sqlx::query(
            "UPDATE native_trace_streams SET ended=true WHERE segment_id=$1 AND stream_hash=$2",
        )
        .bind(segment)
        .bind(stream)
        .execute(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
    }
    Ok(value)
}

pub(super) async fn finish(conn: &mut Conn, segment: Id) -> Result<(), EngineError> {
    // The public retention window also bounds private diagnostics. If its prefix expired,
    // remaining fragments stay hidden and say why rather than guessing at missing context.
    sqlx::query("UPDATE native_trace_streams s SET lost=true WHERE s.segment_id=$1 AND EXISTS(SELECT 1 FROM native_trace_parts p WHERE p.segment_id=s.segment_id AND p.stream_hash=s.stream_hash AND NOT EXISTS(SELECT 1 FROM raw_events r WHERE r.segment_id=p.segment_id AND r.n=p.n))")
        .bind(segment).execute(&mut *conn).await.map_err(orch_db::DbError::from)?;
    sqlx::query("DELETE FROM native_trace_parts p WHERE p.segment_id=$1 AND NOT EXISTS(SELECT 1 FROM raw_events r WHERE r.segment_id=p.segment_id AND r.n=p.n)")
        .bind(segment).execute(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let streams: Vec<(String, Vec<u8>, bool)> = sqlx::query_as("SELECT stream_hash,rules,lost FROM native_trace_streams WHERE segment_id=$1 AND ended ORDER BY stream_hash")
        .bind(segment).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
    for (stream, rules, lost) in streams {
        let rules: Vec<u8> =
            presentation::open(&format!("native-trace-rules:{segment}:{stream}"), &rules)?;
        let masker = Redactor::restore_native(&rules).map_err(|_| failure())?;
        let mut joined = tempfile::tempfile().map_err(|_| failure())?;
        let mut after = (0_i64, -1_i32);
        loop {
            let parts = parts(conn, segment, &stream, after).await?;
            if parts.is_empty() {
                break;
            }
            for (n, part, data) in parts {
                let data: Part =
                    presentation::open(&format!("native-trace:{segment}:{n}:{part}"), &data)?;
                joined
                    .write_all(data.text.as_bytes())
                    .map_err(|_| failure())?;
                after = (n, part);
            }
        }
        joined.seek(SeekFrom::Start(0)).map_err(|_| failure())?;
        let public = masker.mask_native_reader(joined).map_err(|_| failure())?;
        let mut offset = 0_usize;
        after = (0, -1);
        loop {
            let parts = parts(conn, segment, &stream, after).await?;
            if parts.is_empty() {
                break;
            }
            for (n, part, data) in parts {
                let data: Part =
                    presentation::open(&format!("native-trace:{segment}:{n}:{part}"), &data)?;
                let last = offset.checked_add(data.text.len()).ok_or_else(failure)?;
                let stored: Option<Vec<u8>> = sqlx::query_scalar(
                    "SELECT payload FROM raw_events WHERE segment_id=$1 AND n=$2",
                )
                .bind(segment)
                .bind(n)
                .fetch_optional(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
                if let Some(stored) = stored {
                    let mut value = super::unpacked(&stored)?;
                    let mut unavailable = lost;
                    if !lost {
                        if let Some(target) = value.pointer_mut(&data.pointer) {
                            // Byte masks can divide a multibyte character only where that character
                            // was replaced entirely by ASCII stars, preserving every original boundary.
                            *target = Json::String(
                                public
                                    .text()
                                    .get(offset..last)
                                    .ok_or_else(failure)?
                                    .to_owned(),
                            );
                        } else {
                            unavailable = true;
                        }
                    }
                    super::mask_value(&mut value, &masker);
                    sqlx::query(
                        "DELETE FROM native_trace_parts WHERE segment_id=$1 AND n=$2 AND part=$3",
                    )
                    .bind(segment)
                    .bind(n)
                    .bind(part)
                    .execute(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
                    let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM native_trace_parts WHERE segment_id=$1 AND n=$2)")
                        .bind(segment).bind(n).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
                    status(
                        &mut value,
                        if unavailable {
                            Some("expired")
                        } else if pending {
                            Some("pending")
                        } else {
                            None
                        },
                    );
                    let payload = super::compress(&value);
                    sqlx::query("UPDATE raw_events SET payload=$3,change_id=nextval('raw_event_changes') WHERE segment_id=$1 AND n=$2")
                        .bind(segment)
                        .bind(n)
                        .bind(payload)
                        .execute(&mut *conn)
                        .await
                        .map_err(orch_db::DbError::from)?;
                }
                offset = last;
                after = (n, part);
            }
        }
        sqlx::query("DELETE FROM native_trace_streams WHERE segment_id=$1 AND stream_hash=$2")
            .bind(segment)
            .bind(&stream)
            .execute(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
    }
    Ok(())
}

async fn parts(
    conn: &mut Conn,
    segment: Id,
    stream: &str,
    after: (i64, i32),
) -> Result<Vec<(i64, i32, Vec<u8>)>, EngineError> {
    Ok(sqlx::query_as("SELECT n,part,data FROM native_trace_parts WHERE segment_id=$1 AND stream_hash=$2 AND (n,part)>($3,$4) ORDER BY n,part LIMIT $5")
        .bind(segment).bind(stream).bind(after.0).bind(after.1).bind(i64::from(super::PAGE_DEFAULT)).fetch_all(conn).await.map_err(orch_db::DbError::from)?)
}
