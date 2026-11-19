//! Completion facts remain in the original tool timeline; continuations use ordinary input admission.
use crate::{engine::Engine, error::EngineError, ingest, unit::Unit};
use orch_db::Conn;
use orch_db::queries::threads::{self, InputRow, ItemChange, ItemRow, SegmentRow, ThreadRow};
use orch_link_contract::background::{
    BackgroundTask, BackgroundTaskKind, BackgroundTaskState, BackgroundTasks,
};
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// An existing held-input detail; its blocked state still uses ordinary admission reasons.
pub(crate) const OFF: &str = "background_wake_off";
const PREFIX: &str = "background:";

/// The sparse timeline index keeps idle supervision proportional to live native work.
pub(crate) async fn running(
    conn: &mut Conn,
    thread: Option<Id>,
    provider: Option<Id>,
    environment: Option<Id>,
) -> Result<bool, EngineError> {
    Ok(sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM items i JOIN segments s ON s.id=i.segment_id
        WHERE i.kind='tool_call'
          AND i.payload @? '$.background_tasks.*.task.state ? (@ == "running")'
          AND ($1::uuid IS NULL OR i.thread_id=$1)
          AND ($2::uuid IS NULL OR s.provider_id=$2)
          AND ($3::uuid IS NULL OR s.environment_id=$3))"#,
    )
    .bind(thread)
    .bind(provider)
    .bind(environment)
    .fetch_one(conn)
    .await
    .map_err(orch_db::DbError::from)?)
}

/// The existing execution actor is the account charged for an input, including explicit Run.
pub(crate) async fn other_author(
    conn: &mut Conn,
    thread: Id,
    author: Option<Id>,
) -> Result<Option<(Option<Id>, i64)>, EngineError> {
    Ok(sqlx::query_as(
        r#"SELECT COALESCE(q.run_as,q.author_id),count(*)::bigint
        FROM items i JOIN turns t ON t.id=i.turn_id JOIN thread_inputs q ON q.id=t.input_id
        CROSS JOIN LATERAL jsonb_each(i.payload->'background_tasks') entry
        WHERE i.thread_id=$1 AND i.kind='tool_call'
          AND i.payload @? '$.background_tasks.*.task.state ? (@ == "running")'
          AND entry.value->'task'->>'state'='running'
          AND COALESCE(q.run_as,q.author_id) IS DISTINCT FROM $2::uuid
        GROUP BY COALESCE(q.run_as,q.author_id) ORDER BY min(t.ordinal) LIMIT 1"#,
    )
    .bind(thread)
    .bind(author)
    .fetch_optional(conn)
    .await
    .map_err(orch_db::DbError::from)?)
}

/// A native completion may already be requesting its follow-up before its first output frame.
pub(crate) async fn awaiting_native(conn: &mut Conn, thread: Id) -> Result<bool, EngineError> {
    Ok(sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM items WHERE thread_id=$1 AND kind='tool_call'
        AND payload @? '$.background_tasks.*.awaiting_native_wake ? (@ == true)')"#,
    )
    .bind(thread)
    .fetch_one(conn)
    .await
    .map_err(orch_db::DbError::from)?)
}

pub(crate) async fn has_origin(conn: &mut Conn, turn: Id) -> Result<bool, EngineError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM items WHERE turn_id=$1 AND kind='tool_call' AND payload ? 'background_tasks')")
        .bind(turn).fetch_one(conn).await.map_err(orch_db::DbError::from)?)
}

pub(crate) async fn subagents_running(conn: &mut Conn, turn: Id) -> Result<bool, EngineError> {
    Ok(sqlx::query_scalar(r#"SELECT EXISTS(SELECT 1 FROM items WHERE turn_id=$1 AND kind='tool_call'
        AND payload @? '$.background_tasks.*.task ? (@.kind == "subagent" && @.state == "running")')"#)
        .bind(turn).fetch_one(conn).await.map_err(orch_db::DbError::from)?)
}

/// A process refresh cannot replace any lane still serving live work or a native follow-up.
pub(crate) async fn live_harness_thread(
    conn: &mut Conn,
    provider: Id,
    agent: Id,
    machine: Option<Id>,
) -> Result<Option<Id>, EngineError> {
    Ok(sqlx::query_scalar(r#"SELECT i.thread_id FROM items i JOIN segments s ON s.id=i.segment_id
        WHERE s.provider_id=$1 AND s.agent_id=$2 AND ($3::uuid IS NULL OR s.machine_id=$3) AND i.kind='tool_call'
        AND (i.payload @? '$.background_tasks.*.task.state ? (@ == "running")'
          OR i.payload @? '$.background_tasks.*.awaiting_native_wake ? (@ == true)') LIMIT 1"#)
        .bind(provider).bind(agent).bind(machine).fetch_optional(conn).await.map_err(orch_db::DbError::from)?)
}

async fn clear_awaiting(
    unit: &mut Unit<'_>,
    input: &InputRow,
    wake: &Wake,
) -> Result<(), EngineError> {
    for task in &wake.causes {
        let row: Option<Id> = sqlx::query_scalar(
            "SELECT id FROM items WHERE thread_id=$1 AND turn_id=$2 AND native_ref=$3",
        )
        .bind(input.thread_id)
        .bind(task.origin_turn)
        .bind(&task.native_item_ref)
        .fetch_optional(&mut *unit.conn)
        .await
        .map_err(orch_db::DbError::from)?;
        if let Some(id) = row
            && let Some(item) = threads::item(unit.conn, id).await?
        {
            let mut payload = item.payload.clone();
            payload["background_tasks"][task_key(task)]["awaiting_native_wake"] = json!(false);
            save_item(unit, &item, &payload).await?;
        }
    }
    Ok(())
}

/// A lost process ends its live task facts; no transcript can reactivate them.
pub(crate) async fn lost(unit: &mut Unit<'_>, thread: Id, segment: Id) -> Result<(), EngineError> {
    threads::lock(unit.conn, thread).await?;
    let rows: Vec<(Id, Json)> = sqlx::query_as(
        r#"SELECT id,payload FROM items WHERE thread_id=$1 AND segment_id=$2 AND kind='tool_call'
        AND (payload @? '$.background_tasks.*.task.state ? (@ == "running")'
             OR payload @? '$.background_tasks.*.awaiting_native_wake ? (@ == true)')"#,
    )
    .bind(thread)
    .bind(segment)
    .fetch_all(&mut *unit.conn)
    .await
    .map_err(orch_db::DbError::from)?;
    for (id, mut payload) in rows {
        if let Some(tasks) = payload["background_tasks"].as_object_mut() {
            for task in tasks.values_mut() {
                task["awaiting_native_wake"] = json!(false);
                if task["task"]["state"] == "running" {
                    task["task"]["state"] = json!("lost");
                    task["task"]["can_end"] = json!(false);
                    task["lost_at"] = json!(orch_types::Time::new(unit.now()));
                }
            }
        }
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
        threads::set_seq_state(unit.conn, thread, seq).await?;
    }
    unit.wake(thread);
    for input in threads::pending_inputs(unit.conn, thread).await? {
        if let Some(mut wake) = of_input(unit.conn, &input).await?
            && wake.native_wake.take().is_some()
            && let Some(marker_id) = input
                .client_id
                .as_deref()
                .and_then(|id| id.strip_prefix(PREFIX))
                .and_then(|id| id.parse::<Id>().ok())
            && let Some(marker) = threads::item(unit.conn, marker_id)
                .await?
                .filter(|marker| marker.segment_id == segment)
        {
            let mut payload = marker.payload.clone();
            payload["data"] = serde_json::to_value(wake)?;
            save_item(unit, &marker, &payload).await?;
        }
    }
    Ok(())
}

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
    let Some(wake) = of_input(unit.conn, input).await? else {
        return Ok(());
    };
    clear_awaiting(unit, input, &wake).await?;
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
    threads::set_seq_state(unit.conn, item.thread_id, seq).await?;
    Ok(())
}

/// Projects recorded live work after the ordinary reader scope and source masks were checked.
pub async fn status(
    conn: &mut Conn,
    row: &ThreadRow,
    native: &[(Id, String)],
    interact: bool,
) -> Result<orch_api_contract::background::BackgroundWakeStatus, EngineError> {
    use orch_api_contract::background::{BackgroundWakeStatus, BackgroundWork};
    let setting = orch_settings::get::<String>(
        conn,
        orch_settings::Place::Node(row.node_id),
        orch_settings_contract::registry::keys::THREADS_BACKGROUND_WAKE,
    )
    .await?
    .as_deref()
        != Some("off");
    let effective = if let Some(segment) = row.current_segment_id {
        sqlx::query_scalar::<_, Option<bool>>("SELECT background_wake FROM segments WHERE id=$1")
            .bind(segment)
            .fetch_optional(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?
            .flatten()
            .unwrap_or(setting)
    } else {
        setting
    };
    let rows: Vec<(Id, Id, Option<Id>, Id, Option<Id>, bool, Json)> = sqlx::query_as(
            "SELECT i.id,i.segment_id,s.environment_id,t.input_id,q.author_id,
             s.ended_at IS NULL AND s.native_session_ref IS NOT NULL,
             jsonb_build_object('background_tasks',i.payload->'background_tasks')
             FROM items i JOIN segments s ON s.id=i.segment_id
             JOIN turns t ON t.id=i.turn_id JOIN thread_inputs q ON q.id=t.input_id
             WHERE i.thread_id=$1 AND s.continuation_id=$2 AND i.kind='tool_call' AND i.payload ? 'background_tasks'
             ORDER BY i.seq_created")
            .bind(row.node_id).bind(row.active_continuation_id).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let mut tasks = Vec::new();
    let people = crate::views::People::load(
        conn,
        rows.iter().filter_map(|(_, _, _, _, author, _, _)| *author),
    )
    .await?;
    for (item, segment, environment, input_id, author, live, payload) in rows {
        let Some(entries) = payload["background_tasks"].as_object() else {
            continue;
        };
        for (key, value) in entries {
            let Ok(task) = serde_json::from_value::<BackgroundTask>(value["task"].clone()) else {
                continue;
            };
            if task.state != BackgroundTaskState::Running
                && !(task.state == BackgroundTaskState::Lost
                    && Some(segment) == row.current_segment_id)
            {
                continue;
            }
            let label =
                crate::raw_events::mask_live(conn, row, environment, json!({"label":task.label}))
                    .await?
                    .and_then(|masked| masked["label"].as_str().map(str::to_owned))
                    .unwrap_or_default();
            let can_end =
                interact && live && task.can_end && task.state == BackgroundTaskState::Running;
            let end_id = can_end
                .then(|| {
                    native
                        .iter()
                        .find(|(source, id)| {
                            *source == item
                                && (id == &format!("{segment}:{key}")
                                    || value["self_waking"] == false)
                        })
                        .map(|(_, id)| id.clone())
                })
                .flatten();
            tasks.push(BackgroundWork {
                id: format!("{segment}:{item}:{key}"),
                item_id: item,
                turn_id: task.origin_turn,
                input_id,
                author: serde_json::from_value(people.named_or_null(author))?,
                can_end,
                kind: task.kind,
                label,
                state: task.state,
                end_id,
                lost_at: serde_json::from_value(value["lost_at"].clone()).unwrap_or(None),
            });
        }
    }
    let waiting: Option<Option<Id>> = sqlx::query_scalar(
            "SELECT COALESCE(run_as,author_id) FROM thread_inputs WHERE thread_id=$1 AND continuation_id=$2 AND state='pending' AND state_detail='background_author' ORDER BY position LIMIT 1")
            .bind(row.node_id).bind(row.active_continuation_id).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let earlier = if let Some(author) = waiting {
        other_author(conn, row.node_id, author).await?
    } else {
        None
    };
    let waiting_author = if let Some((Some(author), _)) = earlier {
        Some(serde_json::from_value(
            crate::views::user_ref_json(conn, author).await?,
        )?)
    } else {
        None
    };
    let held_completions: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(jsonb_array_length(m.payload->'data'->'causes')),0)::bigint FROM thread_inputs q
             JOIN items m ON q.client_id='background:'||m.id::text
             WHERE q.thread_id=$1 AND q.continuation_id=$2 AND q.state='pending' AND q.state_detail='background_stopped'")
            .bind(row.node_id).bind(row.active_continuation_id).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let refresh_deferred = tasks
        .iter()
        .any(|task| task.state == BackgroundTaskState::Running);
    Ok(BackgroundWakeStatus {
        setting,
        effective,
        refresh_deferred,
        tasks,
        held_completions,
        waiting_author,
        waiting_running: earlier.map_or(0, |(_, count)| count),
    })
}

impl Engine {
    /// Both native histories deliver completed results with ordinary input after a refused wake.
    pub(crate) async fn report_background_with_input(
        &self,
        unit: &mut Unit<'_>,
        input: &InputRow,
        turn: Id,
    ) -> Result<(), EngineError> {
        if is_wake(input) {
            return Ok(());
        }
        for pending in threads::pending_inputs(unit.conn, input.thread_id).await? {
            if !pending.queue_held
                && matches!(
                    pending.state_detail.as_deref(),
                    Some(OFF | "model_use_required" | "author_lost_access")
                )
                && of_input(unit.conn, &pending)
                    .await?
                    .is_some_and(|wake| wake.native_wake.is_none())
            {
                let seq = unit.stamp(input.thread_id).await?;
                let at = unit.now();
                threads::change_input(
                    unit.conn,
                    pending.id,
                    &["pending"],
                    &threads::InputChange {
                        state: "delivered",
                        detail: Some("background_reported"),
                        turn_id: Some(turn),
                        delivered_at: Some(at),
                        ..Default::default()
                    },
                    seq,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Stop holds waiting completions and future wakes of this turn without ending older work.
    pub(crate) async fn hold_background_wakes(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        turn: Id,
    ) -> Result<(), EngineError> {
        sqlx::query("UPDATE turns SET background_wake_held=true WHERE id=$1 AND thread_id=$2")
            .bind(turn)
            .bind(thread.node_id)
            .execute(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
        let rows: Vec<Id> = sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND turn_id=$2 AND kind='tool_call' AND payload ? 'background_tasks'")
            .bind(thread.node_id).bind(turn).fetch_all(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        for id in rows {
            if let Some(item) = threads::item(unit.conn, id).await? {
                let mut payload = item.payload.clone();
                if let Some(tasks) = payload["background_tasks"].as_object_mut() {
                    for task in tasks.values_mut() {
                        task["wake_held"] = json!(true);
                    }
                }
                save_item(unit, &item, &payload).await?;
            }
        }
        for input in threads::pending_inputs(unit.conn, thread.node_id).await? {
            if is_wake(&input) {
                let seq = unit.stamp(thread.node_id).await?;
                sqlx::query("UPDATE thread_inputs SET state_detail='background_stopped',queue_held=false,seq_updated=$2 WHERE id=$1 AND state='pending'")
                    .bind(input.id).bind(seq).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
                self.refuse_background_wake(unit, thread, &input).await?;
            }
        }
        let seq = unit.stamp(thread.node_id).await?;
        threads::clear_empty_queue_hold(unit.conn, thread.node_id, seq).await?;
        unit.wake(thread.node_id);
        Ok(())
    }

    /// A new human input releases only the completion hold; ordinary Stop queue holds remain.
    pub(crate) async fn release_background_wakes(
        &self,
        unit: &mut Unit<'_>,
        thread: Id,
    ) -> Result<(), EngineError> {
        sqlx::query("UPDATE turns SET background_wake_held=false WHERE thread_id=$1 AND background_wake_held")
            .bind(thread).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        let rows: Vec<Id> = sqlx::query_scalar(
            r#"SELECT id FROM items WHERE thread_id=$1 AND kind='tool_call'
            AND payload @? '$.background_tasks.*.wake_held ? (@ == true)'"#,
        )
        .bind(thread)
        .fetch_all(&mut *unit.conn)
        .await
        .map_err(orch_db::DbError::from)?;
        for id in rows {
            if let Some(item) = threads::item(unit.conn, id).await? {
                let mut payload = item.payload.clone();
                if let Some(tasks) = payload["background_tasks"].as_object_mut() {
                    for task in tasks.values_mut() {
                        task["wake_held"] = json!(false);
                    }
                }
                save_item(unit, &item, &payload).await?;
            }
        }
        let seq = unit.stamp(thread).await?;
        sqlx::query("UPDATE thread_inputs SET state_detail=NULL,queue_held=false,seq_updated=$2 WHERE thread_id=$1 AND state='pending' AND state_detail='background_stopped'")
            .bind(thread).bind(seq).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        threads::clear_empty_queue_hold(unit.conn, thread, seq).await?;
        unit.wake(thread);
        Ok(())
    }

    /// Runs under the event-ingestion thread lock after exact native End is confirmed.
    pub(crate) async fn background_ended(
        &self,
        unit: &mut Unit<'_>,
        thread: Id,
        item: Option<Id>,
        command: &str,
    ) -> Result<(), EngineError> {
        let Some(item) = item else { return Ok(()) };
        let Some(item) = threads::item(unit.conn, item)
            .await?
            .filter(|item| item.thread_id == thread)
        else {
            return Ok(());
        };
        let mut payload = item.payload.clone();
        if let Some(tasks) = payload["background_tasks"].as_object_mut() {
            for (key, value) in tasks {
                // Terminal sources own one task per item; self-waking tasks name an exact run.
                let matches = key == command
                    || value["task"]["id"] == command
                    || value["self_waking"] == false;
                if matches && value["task"]["state"] == "running" {
                    value["task"]["state"] = json!("stopped");
                    value["task"]["can_end"] = json!(false);
                    value["awaiting_native_wake"] = json!(false);
                }
            }
        }
        save_item(unit, &item, &payload).await?;
        unit.wake(thread);
        Ok(())
    }

    /// A process-bound setting cannot replace a generation that still owns live work.
    pub(crate) async fn defer_background_refresh(
        &self,
        snapshot: &crate::sessions::StartSnapshot,
    ) -> Result<(), EngineError> {
        let mut tx = self.db().begin().await?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        if let Some(thread) = threads::lock(unit.conn, snapshot.thread.node_id).await?
            && thread.current_turn_id == Some(snapshot.turn.id)
        {
            self.end_turn(
                &mut unit,
                &thread,
                &snapshot.turn,
                crate::turns::TurnEnd {
                    state: "interrupted",
                    error: None,
                    input: crate::turns::InputAfter::Return,
                    then_block: None,
                },
            )
            .await?;
            let seq = unit.stamp(thread.node_id).await?;
            threads::hold_input(
                unit.conn,
                snapshot.input.id,
                Some("background_refresh"),
                seq,
            )
            .await?;
            // The retained lease still serves the original author's live work.
        }
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// The authenticated session stream can describe only tasks whose actual tool/turn it owns.
    pub(crate) async fn observe_background_tasks(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        segment: &SegmentRow,
        report: &BackgroundTasks,
    ) -> Result<(), EngineError> {
        // A lifecycle update also releases ordinary inputs waiting for a different author.
        unit.wake(thread.node_id);
        if report.native_idle {
            let rows: Vec<Id> = sqlx::query_scalar(
                r#"SELECT id FROM items WHERE thread_id=$1 AND segment_id=$2 AND kind='tool_call'
                AND payload @? '$.background_tasks.*.awaiting_native_wake ? (@ == true)'"#,
            )
            .bind(thread.node_id)
            .bind(segment.id)
            .fetch_all(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
            for id in rows {
                if let Some(item) = threads::item(unit.conn, id).await? {
                    let mut payload = item.payload.clone();
                    if let Some(tasks) = payload["background_tasks"].as_object_mut() {
                        for task in tasks.values_mut() {
                            task["awaiting_native_wake"] = json!(false);
                        }
                    }
                    save_item(unit, &item, &payload).await?;
                }
            }
        }
        let mut eligible = Vec::new();
        let mut wake_held = false;
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
            let origin_held: bool =
                sqlx::query_scalar("SELECT background_wake_held FROM turns WHERE id=$1")
                    .bind(turn.id)
                    .fetch_one(&mut *unit.conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            stored["wake_held"] = json!(origin_held || stored["wake_held"] == true);
            // An explicitly ended task cannot become a natural completion on a racing native notification.
            let mut actual = task.clone();
            actual.can_end &= actual.state == BackgroundTaskState::Running;
            if stored["task"]["state"] == "stopped" {
                actual.state = BackgroundTaskState::Stopped;
            }
            if stored["task"]["state"] == "lost" {
                actual.state = BackgroundTaskState::Lost;
                actual.can_end = false;
            }
            stored["task"] = serde_json::to_value(&actual)?;
            stored["self_waking"] = json!(report.self_waking);
            if report.native_idle {
                stored["awaiting_native_wake"] = json!(false);
            }
            if report.self_waking
                && !report.native_idle
                && actual.state.wakes()
                && stored.get("wake_input").is_none()
            {
                stored["awaiting_native_wake"] = json!(true);
            }
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
                wake_held |= stored["wake_held"] == true;
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
        // Earlier work retains its survival while an ordinary input queue is held.
        sqlx::query("UPDATE thread_inputs SET queue_held=false,state_detail=$2 WHERE id=$1")
            .bind(input_id)
            .bind(wake_held.then_some("background_stopped"))
            .execute(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
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
        clear_awaiting(unit, input, &wake).await?;
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
