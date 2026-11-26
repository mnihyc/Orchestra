//! Session openings for database-backed scripted harness worlds.
//!
//! A report producer opens its fixture's conversation before publishing. Its
//! destination comes from placement, and never from the machine publishing a report.

use orch_link_contract::commands::{Cmd, SessionOpen};
use orch_link_contract::events::Event;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use sqlx::{PgPool, Row};

/// The planned first open of a fixture session.
#[derive(Debug, Clone)]
pub struct Opening {
    /// The machine where this arrangement places its harness.
    pub machine: Id,
    /// The command that opens the session there.
    pub body: SessionOpen,
}

/// Plans an unopened segment and records its first command's destination once.
pub async fn prepare(
    pool: &PgPool,
    thread: Id,
    segment: Id,
) -> Result<Option<Opening>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT s.machine_id, CASE WHEN s.arrangement = 'separated' THEN p.machine_id ELSE e.machine_id END AS destination,
                s.agent_id, s.provider_id, s.environment_id, s.env_generation, s.arrangement,
                s.executor, s.tool_set, s.native_session_ref, s.raw_last_n,
                e.workspace_root, t.workspace_dir
         FROM segments s JOIN threads t ON t.node_id = s.thread_id
         JOIN providers p ON p.id = s.provider_id JOIN environments e ON e.id = s.environment_id
         WHERE s.id = $2 AND s.thread_id = $1",
    )
    .bind(thread).bind(segment).fetch_optional(pool).await?;
    let Some(row) = row else { return Ok(None) };
    if row.try_get::<Option<Id>, _>("machine_id")?.is_some() {
        return Ok(None);
    }
    let machine: Id = row
        .try_get::<Option<Id>, _>("destination")?
        .ok_or(sqlx::Error::RowNotFound)?;
    let provider: Id = row.try_get("provider_id")?;
    let root: String = row.try_get("workspace_root")?;
    let directory: String = row.try_get("workspace_dir")?;
    let directory = format!("{}/{}", root.trim_end_matches('/'), directory);
    let native: Option<String> = row.try_get("native_session_ref")?;
    let body: SessionOpen = serde_json::from_value(json!({
        "agent_id": row.try_get::<Id, _>("agent_id")?, "thread_id": thread, "segment_id": segment,
        "mode": if native.is_some() { "resume" } else { "new" }, "native_session_ref": native,
        "archive": null, "history": null, "upto_turn_ordinal": null,
        "workspace_root": root, "instruction_root_marker": false,
        "directory": directory, "cwd": directory, "mounts": [], "model": "fake-1", "effort": null,
        "params": {}, "approval_mode": "auto", "web_action_wait_secs": 120, "sandbox_mode": "workspace", "instructions": "", "memory": null,
        "arrangement": row.try_get::<String, _>("arrangement")?, "provider": {"path":"native", "provider_id":provider},
        "exec": {"executor":row.try_get::<Option<String>, _>("executor")?, "tool_set":row.try_get::<Option<String>, _>("tool_set")?,
            "environment_id":row.try_get::<Id, _>("environment_id")?, "generation":row.try_get::<i32, _>("env_generation")?, "ticket":null},
        "mcp": [], "orch_tools": [], "output_capture_bytes":67108864, "raw_events": true, "raw_after": row.try_get::<i64, _>("raw_last_n")?
    })).map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
    let assigned = sqlx::query("UPDATE segments SET machine_id = $3 WHERE id = $2 AND thread_id = $1 AND machine_id IS NULL")
        .bind(thread).bind(segment).bind(machine).execute(pool).await?.rows_affected();
    Ok((assigned != 0).then_some(Opening { machine, body }))
}

/// Records the session reference returned by the scripted harness after its open succeeds.
pub async fn opened(pool: &PgPool, segment: Id, result: &Json) -> Result<(), sqlx::Error> {
    if let Some(native) = result.get("native_session_ref").and_then(Json::as_str) {
        sqlx::query("UPDATE segments SET native_session_ref = $2 WHERE id = $1 AND native_session_ref IS NULL")
            .bind(segment).bind(native).execute(pool).await?;
    }
    Ok(())
}

/// The original native attempt of this logical turn, retained on its admitted command.
/// A later attempt must supply its own captured producer, never replace this original source.
pub async fn first_turn_opening(
    pool: &PgPool,
    thread: Id,
    segment: Id,
    turn: Id,
) -> Result<Option<Id>, sqlx::Error> {
    Ok(sqlx::query_scalar::<_,Option<Id>>(
        "SELECT (tool_session#>>'{native_target,target,opening_id}')::uuid FROM commands
          WHERE kind IN ('turn.start','turn.start.fenced')
            AND COALESCE(body#>>'{turn,turn_id}',body->>'turn_id')=$3::uuid::text
            AND tool_session#>>'{native_target,target,thread_id}'=$1::uuid::text
            AND tool_session#>>'{native_target,target,segment_id}'=$2::uuid::text
          ORDER BY (tool_session#>>'{native_target,target,opening_epoch}')::bigint NULLS FIRST,created_at,id LIMIT 1",
    ).bind(thread).bind(segment).bind(turn).fetch_optional(pool).await?.flatten())
}

/// Resolve the original report's source from its own turn/item, without reading a current slot.
pub async fn report_opening(
    pool: &PgPool,
    thread: Id,
    segment: Id,
    event: &Event,
    batch: &[Event],
) -> Result<Option<Id>, sqlx::Error> {
    match event {
        Event::SessionOpened(opened) => return Ok(Some(opened.opening_id)),
        Event::SessionLost(lost) => return Ok(Some(lost.opening_id)),
        _ => {}
    }
    let body = event
        .body()
        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
    let parse = |value: &Json| value.as_str().and_then(|value| value.parse::<Id>().ok());
    let mut turn = body.get("turn_id").and_then(parse);
    if turn.is_none()
        && let Some(reference) = body.get("native_ref").and_then(Json::as_str)
    {
        turn = batch.iter().find_map(|event| match event {
            Event::ItemStarted(started) if started.native_ref == reference => Some(started.turn_id),
            _ => None,
        });
        if turn.is_none() {
            turn = sqlx::query_scalar::<_, Option<Id>>(
                "SELECT turn_id FROM items WHERE thread_id=$1 AND segment_id=$2 AND native_ref=$3",
            )
            .bind(thread)
            .bind(segment)
            .bind(reference)
            .fetch_optional(pool)
            .await?
            .flatten();
        }
    }
    if turn.is_none()
        && let Some(reference) = body.get("request_ref").and_then(Json::as_str)
    {
        turn = sqlx::query_scalar::<_,Option<Id>>("SELECT a.turn_id FROM approvals a JOIN turns t ON t.id=a.turn_id WHERE a.thread_id=$1 AND t.segment_id=$2 AND a.native_ref=$3 ORDER BY a.created_at,a.id LIMIT 1")
            .bind(thread).bind(segment).bind(reference).fetch_optional(pool).await?.flatten();
    }
    match turn {
        Some(turn) => first_turn_opening(pool, thread, segment, turn).await,
        None => Ok(None),
    }
}
