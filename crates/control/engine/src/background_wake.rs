//! Completion facts remain in the original tool timeline; continuations use ordinary input admission.
use crate::{engine::Engine, error::EngineError, ingest, unit::Unit};
use orch_db::Conn;
use orch_db::queries::threads::{self, InputRow, ItemChange, ItemRow, SegmentRow, ThreadRow};
use orch_link_contract::background::{
    BackgroundTask, BackgroundTaskKind, BackgroundTaskState, BackgroundTasks,
};
use orch_types::{Id, Json, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// An existing held-input detail; its blocked state still uses ordinary admission reasons.
pub(crate) const OFF: &str = "background_wake_off";
const PREFIX: &str = "background:";

/// Private admission facts alongside the reader-visible wake marker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Wake {
    pub input_id: Id,
    pub origin_input: Id,
    pub origin_turn: Id,
    pub native_wake: Option<String>,
    pub causes: Vec<BackgroundTask>,
    #[serde(default)]
    pub admitted: bool,
}

pub(crate) fn is_wake(input: &InputRow) -> bool {
    input.source == "system"
        && input
            .client_id
            .as_deref()
            .is_some_and(|id| id.starts_with(PREFIX))
}

/// The primary item identity makes lookups bounded without a process-inventory table.
pub(crate) async fn of_input(
    conn: &mut Conn,
    input: &InputRow,
) -> Result<Option<Wake>, EngineError> {
    if !is_wake(input) {
        return Ok(None);
    }
    let Some(id) = input
        .client_id
        .as_deref()
        .and_then(|id| id.strip_prefix(PREFIX))
        .and_then(|id| id.parse::<Id>().ok())
    else {
        return Ok(None);
    };
    let Some(item) = threads::item(conn, id).await? else {
        return Ok(None);
    };
    if item.thread_id != input.thread_id || item.payload["marker"] != "background_wake" {
        return Ok(None);
    }
    let wake: Wake = serde_json::from_value(item.payload["data"].clone())?;
    Ok((wake.input_id == input.id).then_some(wake))
}

pub(crate) fn task_key(task: &BackgroundTask) -> String {
    json!([task.id, task.run_id]).to_string()
}

/// Native adoption checks the model already executing, rather than a future-session choice.
pub(crate) async fn settings_for(
    conn: &mut Conn,
    input: &InputRow,
    settings: &crate::resolved::Resolved,
) -> Result<crate::resolved::Resolved, EngineError> {
    let mut actual = settings.clone();
    if let Some(wake) = of_input(conn, input).await?
        && wake.native_wake.is_some()
        && let Some(turn) = threads::turn(conn, wake.origin_turn).await?
    {
        actual.model = Some(turn.model);
        actual.effort = turn.effort;
        actual.params = turn.params;
    }
    Ok(actual)
}

/// The controller's text is independent of model output and names the stored source item.
pub(crate) fn completion_text(task: &BackgroundTask, item: Id) -> String {
    match task.kind {
        BackgroundTaskKind::Command => format!(
            "Background task {} finished (exit {}). Its output is in the item {item}.",
            json!(task.label),
            task.exit_code
                .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
        ),
        BackgroundTaskKind::Subagent => format!("Subagent {} finished.", json!(task.label)),
    }
}

pub(crate) async fn bind_turn(
    unit: &mut Unit<'_>,
    input: &InputRow,
    turn: Id,
) -> Result<(), EngineError> {
    if of_input(unit.conn, input).await?.is_none() {
        return Ok(());
    }
    let Some(marker) = input
        .client_id
        .as_deref()
        .and_then(|id| id.strip_prefix(PREFIX))
        .and_then(|id| id.parse::<Id>().ok())
    else {
        return Ok(());
    };
    let seq = unit.stamp(input.thread_id).await?;
    sqlx::query("UPDATE items SET turn_id=$2,seq_updated=$3,payload=jsonb_set(payload,'{data,admitted}','true'::jsonb) WHERE id=$1 AND thread_id=$4")
        .bind(marker).bind(turn).bind(seq).bind(input.thread_id)
        .execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
    Ok(())
}

async fn save_item(unit: &mut Unit<'_>, item: &ItemRow, payload: &Json) -> Result<(), EngineError> {
    let seq = unit.stamp(item.thread_id).await?;
    let at = unit.now();
    threads::change_item(
        unit.conn,
        item.id,
        &ItemChange {
            status: None,
            payload,
            search_text: None,
            at,
        },
        seq,
    )
    .await?;
    Ok(())
}

impl Engine {
    /// The authenticated session stream can describe only tasks whose actual tool/turn it owns.
    pub(crate) async fn observe_background_tasks(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        segment: &SegmentRow,
        report: &BackgroundTasks,
    ) -> Result<(), EngineError> {
        let mut eligible = Vec::new();
        for task in report
            .tasks
            .iter()
            .take(orch_link_contract::background::SNAPSHOT_COMMANDS)
        {
            let Some(item) =
                threads::item_by_native_ref(unit.conn, segment.id, &task.native_item_ref).await?
            else {
                continue;
            };
            if item.thread_id != thread.node_id
                || item.kind != "tool_call"
                || item.turn_id != Some(task.origin_turn)
            {
                continue;
            }
            let expected_kind = match task.kind {
                BackgroundTaskKind::Command => "command",
                BackgroundTaskKind::Subagent => "subagent",
            };
            if item.payload["tool"]["kind"] != expected_kind {
                continue;
            }
            let Some(turn) = threads::turn(unit.conn, task.origin_turn).await? else {
                continue;
            };
            let key = task_key(task);
            let mut payload = item.payload.clone();
            if !payload["background_tasks"].is_object() {
                payload["background_tasks"] = json!({});
            }
            let mut stored = payload["background_tasks"]
                .get(&key)
                .cloned()
                .unwrap_or_else(|| json!({}));
            // An explicitly ended task cannot become a natural completion on a racing native notification.
            let mut actual = task.clone();
            if stored["task"]["state"] == "stopped" {
                actual.state = BackgroundTaskState::Stopped;
            }
            stored["task"] = serde_json::to_value(&actual)?;
            payload["background_tasks"][&key] = stored.clone();
            if payload != item.payload {
                save_item(unit, &item, &payload).await?;
            }
            let requested = report
                .native_wake
                .as_ref()
                .is_some_and(|wake| wake.task_ids.contains(&key));
            if actual.state.wakes()
                && stored.get("wake_input").is_none()
                && (!report.self_waking || requested)
                && !turn.is_active()
            {
                eligible.push((item.id, actual, turn));
            }
        }
        if eligible.is_empty() || thread.current_segment_id != Some(segment.id) {
            return Ok(());
        }
        eligible.sort_by_key(|(_, _, turn)| turn.ordinal);
        let first = &eligible[0].2;
        let Some(origin) = threads::input(unit.conn, first.input_id).await? else {
            return Ok(());
        };
        if origin.continuation_id != thread.active_continuation_id {
            return Ok(());
        }
        let causes: Vec<_> = eligible.iter().map(|(_, task, _)| task.clone()).collect();
        let token = report.native_wake.as_ref().map(|wake| wake.token.clone());
        let reference = token.clone().unwrap_or_else(|| {
            eligible
                .iter()
                .map(|(item, task, _)| format!("{item}:{}", task_key(task)))
                .collect::<Vec<_>>()
                .join(",")
        });
        let input_id = crate::ids::derived(
            "background.input",
            &[segment.id.as_bytes(), reference.as_bytes()],
        );
        let marker_key = format!("background-wake:{input_id}");
        if threads::item_by_native_ref(unit.conn, segment.id, &marker_key)
            .await?
            .is_some()
        {
            return Ok(());
        }
        let wake = Wake {
            input_id,
            origin_input: origin.id,
            origin_turn: first.id,
            native_wake: token,
            causes,
            admitted: false,
        };
        let marker_payload = json!({"marker":"background_wake","data":wake});
        let Some(marker) = ingest::add_item(
            unit,
            &ingest::ItemSpec {
                thread: thread.node_id,
                segment: segment.id,
                turn: None,
                stream: "main",
                parent: None,
                kind: "marker",
                status: "completed",
                author: origin.author_id,
                payload: &marker_payload,
                native_ref: Some(&marker_key),
            },
        )
        .await?
        else {
            return Ok(());
        };
        let content = json!([{"type":"text","text": eligible.iter()
            .map(|(item, task, _)| completion_text(task, *item)).collect::<Vec<_>>().join("\n")}]);
        let client = format!("{PREFIX}{}", marker.id);
        let seq = unit.stamp(thread.node_id).await?;
        let at = unit.now();
        let inserted = threads::insert_input_in(
            unit.conn,
            &threads::NewInput {
                id: input_id,
                thread_id: thread.node_id,
                author_id: origin.author_id,
                submission_authority: origin.submission_authority.as_ref(),
                run_as: origin.run_as,
                run_as_authority: origin.run_as_authority.as_ref(),
                source: "system",
                content: &content,
                delivery: "queue",
                state: "pending",
                turn_id: None,
                client_id: Some(&client),
                limit_bypass: None,
                place: threads::Place::Front,
                at,
                plan: origin.plan,
            },
            Some(origin.continuation_id),
            seq,
        )
        .await?;
        if inserted.is_none() {
            return Ok(());
        }
        for (item_id, task, _) in &eligible {
            if let Some(item) = threads::item(unit.conn, *item_id).await? {
                let mut payload = item.payload.clone();
                payload["background_tasks"][task_key(task)]["wake_input"] = json!(input_id);
                save_item(unit, &item, &payload).await?;
            }
        }
        threads::set_seq_state(unit.conn, thread.node_id, seq).await?;
        unit.wake(thread.node_id);
        Ok(())
    }

    /// Refusal retracts the native follow-up without changing the completed task or its pending input.
    pub(crate) async fn refuse_background_wake(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        input: &InputRow,
    ) -> Result<(), EngineError> {
        let Some(mut wake) = of_input(unit.conn, input).await? else {
            return Ok(());
        };
        if wake.native_wake.take().is_none() {
            return Ok(());
        }
        let Some(marker_id) = input
            .client_id
            .as_deref()
            .and_then(|id| id.strip_prefix(PREFIX))
            .and_then(|id| id.parse::<Id>().ok())
        else {
            return Ok(());
        };
        let Some(marker) = threads::item(unit.conn, marker_id).await? else {
            return Ok(());
        };
        let mut payload = marker.payload.clone();
        payload["data"] = serde_json::to_value(&wake)?;
        save_item(unit, &marker, &payload).await?;
        let machine: Option<Id> = sqlx::query_scalar::<_, Option<Id>>(
            "SELECT machine_id FROM segments WHERE id=$1 AND thread_id=$2",
        )
        .bind(marker.segment_id)
        .bind(thread.node_id)
        .fetch_optional(&mut *unit.conn)
        .await
        .map_err(orch_db::DbError::from)?
        .flatten();
        let Some(machine) = machine else {
            return Ok(());
        };
        let command = crate::commands::Outgoing::new(
            crate::ids::derived("background.interrupt", &[input.id.as_bytes()]),
            machine,
            orch_link_contract::commands::SerialKey::Thread(thread.node_id),
            &orch_link_contract::commands::TurnInterrupt {
                thread_id: thread.node_id,
                turn_id: wake.origin_turn,
            },
        )?;
        let commands = std::sync::Arc::clone(&self.inner.commands);
        tokio::spawn(async move {
            if let Err(error) = commands.issue(command).await {
                tracing::warn!(%error, "a refused native background continuation could not be interrupted");
            }
        });
        Ok(())
    }
}
