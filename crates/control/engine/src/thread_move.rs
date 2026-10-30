//! Moving the same idle conversation and its side paths while preserving its native work and queue.

use crate::error::EngineError;
use crate::unit::Unit;
use orch_authz::Principal;
use orch_db::queries::{hierarchy, threads};
use orch_db::{Conn, DbError};
use orch_types::{Id, Json};
use serde_json::json;

/// The main thread and all retained side lanes, locked in stable identifier order.
pub async fn lock_lanes(conn: &mut Conn, main: Id) -> Result<Vec<threads::ThreadRow>, EngineError> {
    let ids: Vec<Id> = sqlx::query_scalar(
        "SELECT node_id FROM threads WHERE node_id=$1 OR side_of=$1 ORDER BY node_id FOR UPDATE",
    )
    .bind(main)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let mut lanes = Vec::new();
    for id in ids {
        if let Some(row) = threads::lock(conn, id).await? {
            lanes.push(row);
        }
    }
    Ok(lanes)
}

/// What prevents a thread move, without opening any native process or spending a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    /// The main or side lane whose work must finish.
    pub thread: Id,
    /// Its current work state.
    pub state: String,
    /// The job that holds it, when present.
    pub job: Option<Id>,
}

/// Starting files, live main or side work, delivering inputs and change locks all refuse a move.
pub async fn busy(
    conn: &mut Conn,
    lanes: &[threads::ThreadRow],
) -> Result<Option<Busy>, EngineError> {
    for row in lanes {
        if matches!(
            row.state.as_str(),
            "starting" | "running" | "waiting_approval"
        ) || threads::active_turn(conn, row.node_id).await?.is_some()
            || crate::background::native_busy(conn, row.node_id).await?
            || orch_db::queries::agent_history::removing(conn, row.node_id).await?
        {
            return Ok(Some(Busy {
                thread: row.node_id,
                state: row.state.clone(),
                job: None,
            }));
        }
        let delivering: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM thread_inputs WHERE thread_id=$1 AND state='delivering')",
        )
        .bind(row.node_id)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
        if delivering {
            return Ok(Some(Busy {
                thread: row.node_id,
                state: "delivering".to_owned(),
                job: None,
            }));
        }
        if let Some(job) = crate::locks::held_by(conn, row.node_id).await? {
            return Ok(Some(Busy {
                thread: row.node_id,
                state: "change_job".to_owned(),
                job: Some(job),
            }));
        }
    }
    Ok(None)
}

/// Reparents only the nodes and topic association; this is also the reversible preview trial.
pub async fn reparent(
    conn: &mut Conn,
    lanes: &[threads::ThreadRow],
    main: Id,
    target: Id,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), EngineError> {
    hierarchy::move_under(conn, main, target, now).await?;
    for row in lanes.iter().filter(|row| row.node_id != main) {
        // The logical side parent stays the main thread. Some older rows place its node directly
        // in the topic, while a nested side node remains under that same main node.
        let parent = if row.parent_id == Some(main) {
            main
        } else {
            target
        };
        hierarchy::move_under(conn, row.node_id, parent, now).await?;
    }
    let ids: Vec<_> = lanes.iter().map(|row| row.node_id).collect();
    sqlx::query("UPDATE threads SET topic_id=$2 WHERE node_id=ANY($1)")
        .bind(&ids)
        .bind(target)
        .execute(conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

/// Writes the guarded move's current scopes and history without changing a binding, file or held input.
pub async fn write(
    conn: &mut Conn,
    principal: &Principal,
    lanes: &[threads::ThreadRow],
    main: Id,
    target: Id,
    marker: Json,
    key: &str,
) -> Result<(), EngineError> {
    let now = principal.now().get();
    reparent(conn, lanes, main, target, now).await?;
    let mut unit = Unit::new(conn, now);
    for row in lanes {
        let seq = unit.stamp(row.node_id).await?;
        sqlx::query("UPDATE threads SET seq_state=$2 WHERE node_id=$1")
            .bind(row.node_id)
            .bind(seq)
            .execute(&mut *unit.conn)
            .await
            .map_err(DbError::from)?;
        // Parent/default revisions become different next-turn context. No new input or runtime
        // operation is made merely to move; only a real next turn sends its refreshed scopes.
        if let Some(segment) = row.current_segment_id {
            sqlx::query("UPDATE segments SET context_sent='{}'::jsonb WHERE id=$1")
                .bind(segment)
                .execute(&mut *unit.conn)
                .await
                .map_err(DbError::from)?;
            if row.node_id == main {
                crate::ingest::add_marker(&mut unit, main, segment, None, &marker, key).await?;
            }
        }
        hierarchy::stamp_subtree(&mut *unit.conn, row.node_id).await?;
    }
    hierarchy::stamp(&mut *unit.conn, target).await?;
    if let Some(row) = lanes.iter().find(|row| row.node_id == main) {
        hierarchy::stamp(&mut *unit.conn, row.topic_id).await?;
    }
    unit.finish().await?;
    Ok(())
}

/// The move's attributed marker uses the same facts as its two Activity entries.
pub fn marker(from: Json, to: Json, by: Json) -> Json {
    json!({"marker":"thread_moved","data":{"from":from,"to":to,"by":by}})
}
