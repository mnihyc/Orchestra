//! Canonical wire facts in one job's existing private command journal.
//!
//! These helpers establish identity only. Their callers retain job ownership,
//! cancellation, phase and authority checks, and hold the existing write fences
//! before remembering a command.

use crate::EngineError;
use crate::commands::{Outgoing, RetainedCommand};
use orch_db::Conn;
use orch_db::queries::jobs::{self, JobRow};
use orch_types::{Hash, Id, Json, Time};
use serde_json::json;

/// Hash the same JSON body independently of object insertion order.
pub(crate) fn hash(value: &Json) -> Result<Hash, EngineError> {
    let mut value = value.clone();
    value.sort_all_objects();
    Ok(Hash::of(&serde_json::to_vec(&value)?))
}

/// The immutable recipient, serial lane, kind and body admitted under one ID.
pub(crate) fn canonical(
    id: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
) -> Result<Json, EngineError> {
    Ok(json!({"id":id,"machine":machine,"key":key,"kind":kind,"body_hash":hash(body)?.to_string()}))
}

/// Match both the job's active command and its privately recorded wire facts.
/// The business facade decides which jobs, phases and journal fields it owns.
pub(crate) fn recorded_matches(
    row: &JobRow,
    field: &'static str,
    id: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
) -> Result<bool, EngineError> {
    if row.plan.pointer("/active_command/id") != Some(&json!(id))
        || row.plan.pointer("/active_command/machine") != Some(&json!(machine))
        || row
            .plan
            .pointer("/active_command/kind")
            .and_then(Json::as_str)
            != Some(kind)
    {
        return Ok(false);
    }
    Ok(row
        .plan
        .get(field)
        .and_then(|values| values.get(id.to_string()))
        == Some(&canonical(id, machine, key, kind, body)?))
}

/// Match the issuer's actual retained frame, rather than a later retry's body.
/// A retry still has to name that frame's actual recipient, kind and serial lane.
pub(crate) fn retained_matches(
    row: &JobRow,
    field: &'static str,
    command: &Outgoing,
    retained: &RetainedCommand,
) -> Result<bool, EngineError> {
    if retained.machine != command.machine
        || retained.kind != command.kind.as_str()
        || retained.key != command.key.wire()
    {
        return Ok(false);
    }
    recorded_matches(
        row,
        field,
        command.id,
        retained.machine,
        retained.key.as_deref(),
        &retained.kind,
        &retained.body,
    )
}

/// Add one identity to the current journal without replacing another admitted frame.
/// The caller already holds the job's existing write fences and has admitted it.
pub(crate) async fn remember(
    conn: &mut Conn,
    row: &JobRow,
    field: &'static str,
    command: &Outgoing,
    now: Time,
) -> Result<(), EngineError> {
    let native = canonical(
        command.id,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
    )?;
    let current = jobs::job(conn, row.id)
        .await?
        .ok_or_else(|| EngineError::unreadable("The provider update journal disappeared."))?;
    let mut values = current
        .plan
        .get(field)
        .cloned()
        .unwrap_or_else(|| json!({}));
    let entries = values.as_object_mut().ok_or_else(|| {
        EngineError::unreadable("The provider update command journal is invalid.")
    })?;
    if entries
        .get(&command.id.to_string())
        .is_some_and(|old| old != &native)
    {
        return Err(EngineError::unreadable(
            "The provider update command identity changed.",
        ));
    }
    entries.insert(command.id.to_string(), native);
    jobs::merge_plan(conn, row.id, &json!({field:values}), now.get()).await?;
    Ok(())
}
