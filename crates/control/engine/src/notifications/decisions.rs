//! Own-reader annotation of already authorized history/side projections.
use crate::EngineError;
use orch_api_contract::{
    notifications::NoticeDecision,
    threads::{Item, ItemPayload, SubagentState, ToolPayload, Turn, TurnState},
};
use orch_db::{Conn, queries::notifications::decisions};
use orch_types::Id;
use std::collections::BTreeMap;
/// Called only after the native reader selected this caller's visible terminal subjects.
pub async fn project(
    conn: &mut Conn,
    user: Id,
    turns: &mut [Turn],
    items: &mut [Item],
) -> Result<(), EngineError> {
    let mut groups: BTreeMap<(Id, &str), Vec<Id>> = BTreeMap::new();
    for row in turns.iter() {
        if matches!(
            row.state,
            TurnState::Completed | TurnState::Failed | TurnState::Interrupted
        ) {
            groups
                .entry((row.thread_id, "turn"))
                .or_default()
                .push(row.id);
        }
    }
    for row in items.iter() {
        if let ItemPayload::ToolCall(call) = &row.payload
            && matches!(
                &call.tool,
                ToolPayload::Subagent {
                    state: SubagentState::Completed
                        | SubagentState::Failed
                        | SubagentState::Interrupted,
                    ..
                }
            )
        {
            groups
                .entry((row.thread_id, "item"))
                .or_default()
                .push(row.id);
        }
    }
    for ((thread, kind), ids) in groups {
        let facts = decisions::own(conn, user, thread, kind, &ids).await?;
        if kind == "turn" {
            for row in turns.iter_mut().filter(|row| row.thread_id == thread) {
                row.notice_decision = facts
                    .get(&row.id)
                    .map(|value| serde_json::from_value::<NoticeDecision>(value.clone()))
                    .transpose()?;
            }
        } else {
            for row in items.iter_mut().filter(|row| row.thread_id == thread) {
                row.notice_decision = facts
                    .get(&row.id)
                    .map(|value| serde_json::from_value::<NoticeDecision>(value.clone()))
                    .transpose()?;
            }
        }
    }
    Ok(())
}
