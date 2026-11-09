//! A closed native tool call can pause its service; resuming never replays that closed request.
use super::*;
use crate::tools::capability_service::selection::Selection;

/// Model-readable pause state. It describes no completed request or measured quantity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPause {
    pub service: String,
    pub reason: String,
    pub cause: String,
    pub until: Option<Time>,
    pub needs_person: bool,
}
impl ToolPause {
    pub fn result(&self) -> Json {
        json!({"state":"paused","service":self.service,"reason":self.reason,"cause":self.cause,
            "until":self.until,"needs_person":self.needs_person})
    }
}

fn pause(row: &Recovery) -> Option<ToolPause> {
    if row.context["native_tool_paused"] != true
        || !matches!(row.state.as_str(), "waiting" | "scheduled" | "stopped")
    {
        return None;
    }
    Some(ToolPause {
        service: row.purpose.clone(),
        reason: row.context["native_pause_reason"]
            .as_str()
            .unwrap_or("person")
            .into(),
        cause: row.failure["code"]
            .as_str()
            .unwrap_or("harness_error")
            .into(),
        until: row.due_at.map(Time::new),
        needs_person: row.due_at.is_none(),
    })
}
async fn current(conn: &mut Conn, row: &Recovery, now: DateTime<Utc>) -> Result<bool, EngineError> {
    if pause(row).is_none() || row.due_at.is_some_and(|until| until <= now) {
        return Ok(false);
    }
    let Some(value) = row.context.get("native_backend") else {
        return Ok(false);
    };
    let selected: Selection = serde_json::from_value(value.clone())?;
    Ok(selected.is_current(conn, row.node_id).await?
        && Policy::load(conn, row.node_id)
            .await?
            .rules
            .iter()
            .any(|rule| json!(rule) == row.rule))
}

/// Current service pauses across logical calls, restricted to the same explicit backend opt-in.
/// This is a status lookup; the calling tool still restores its actual Source and callers.
pub async fn active(
    conn: &mut Conn,
    node: Id,
    purpose: Purpose,
    selected: &Selection,
    now: DateTime<Utc>,
) -> Result<Option<ToolPause>, EngineError> {
    let rows:Vec<Recovery>=sqlx::query_as("SELECT node_id,purpose,call_key,reference,failure,rule,cursor,action,state,due_at,context FROM model_call_recovery WHERE node_id=$1 AND purpose=$2 AND context->>'native_tool_paused'='true' AND state IN ('waiting','scheduled','stopped') ORDER BY updated_at DESC,reference DESC")
        .bind(node).bind(purpose.as_str()).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    for row in rows {
        if row.context.get("native_backend") == Some(&serde_json::to_value(selected)?)
            && current(conn, &row, now).await?
        {
            return Ok(pause(&row));
        }
    }
    Ok(None)
}

/// Retain the actual rule decision after ending a tool call. It schedules no request and no usage.
/// The producer supplies its already admitted native recovery row on its guarded transaction.
pub async fn retain(
    conn: &mut Conn,
    row: &Recovery,
    selected: &Selection,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<ToolPause, EngineError> {
    let purpose: Purpose = serde_json::from_value(json!(row.purpose))?;
    if purpose.capability().is_none() {
        return Err(EngineError::StaleToolSource);
    }
    if !matches!(row.state.as_str(), "waiting" | "scheduled" | "stopped")
        || !selected.is_current(conn, row.node_id).await?
    {
        return Err(EngineError::StaleToolSource);
    }
    let wrote=sqlx::query("UPDATE model_call_recovery SET context=jsonb_set(jsonb_set(jsonb_set(context,'{native_tool_paused}','true'),'{native_backend}',$4),'{native_pause_reason}',to_jsonb($5::text)),updated_at=$6 WHERE node_id=$1 AND purpose=$2 AND call_key=$3 AND reference=$7 AND rule=$8 AND cursor=$9 AND state=$10")
        .bind(row.node_id).bind(&row.purpose).bind(row.call_key).bind(serde_json::to_value(selected)?)
        .bind(reason).bind(now).bind(row.reference).bind(&row.rule).bind(&row.cursor).bind(&row.state)
        .execute(conn).await.map_err(DbError::from)?.rows_affected();
    if wrote != 1 {
        return Err(EngineError::StaleToolSource);
    }
    Ok(ToolPause {
        service: row.purpose.clone(),
        reason: reason.into(),
        cause: row.failure["code"]
            .as_str()
            .unwrap_or("harness_error")
            .into(),
        until: row.due_at.map(Time::new),
        needs_person: row.due_at.is_none(),
    })
}

pub(super) async fn selected(
    conn: &mut Conn,
    node: Id,
    now: DateTime<Utc>,
) -> Result<Option<Recovery>, EngineError> {
    if let Some(agent) = read(conn, node, Purpose::Agent)
        .await?
        .filter(|row| !matches!(row.state.as_str(), "completed" | "dismissed"))
    {
        return Ok(Some(agent));
    }
    let rows:Vec<Recovery>=sqlx::query_as("SELECT node_id,purpose,call_key,reference,failure,rule,cursor,action,state,due_at,context FROM model_call_recovery WHERE node_id=$1 AND context->>'native_tool_paused'='true' AND state IN ('waiting','scheduled','stopped') ORDER BY updated_at DESC,reference DESC")
        .bind(node).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    for row in rows {
        if current(conn, &row, now).await? {
            return Ok(Some(row));
        }
    }
    Ok(None)
}

pub(super) fn service(row: &Recovery) -> Option<String> {
    pause(row).map(|pause| pause.service)
}

pub(super) async fn control(
    conn: &mut Conn,
    row: &Recovery,
    action: orch_api_contract::calls::CallRecoveryAction,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    let stop = action == orch_api_contract::calls::CallRecoveryAction::Stop;
    sqlx::query("UPDATE model_call_recovery SET state=$4,due_at=NULL,updated_at=$5,context=jsonb_set(context,'{native_pause_reason}',to_jsonb($6::text)) WHERE node_id=$1 AND purpose=$2 AND call_key=$3 AND context->>'native_tool_paused'='true'")
        .bind(row.node_id).bind(&row.purpose).bind(row.call_key).bind(if stop {"stopped"} else {"dismissed"})
        .bind(now).bind(if stop {"stopped_by_person"} else {"resumed_by_person"}).execute(conn).await.map_err(DbError::from)?;
    Ok(())
}
