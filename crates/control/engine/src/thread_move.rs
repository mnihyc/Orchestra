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
    lane_rows(conn, main, true).await
}

/// The same retained membership for effect-free operation preparation, without a writer lock.
pub async fn lanes(conn: &mut Conn, main: Id) -> Result<Vec<threads::ThreadRow>, EngineError> {
    lane_rows(conn, main, false).await
}

async fn lane_rows(
    conn: &mut Conn,
    main: Id,
    locked: bool,
) -> Result<Vec<threads::ThreadRow>, EngineError> {
    let sql = if locked {
        "SELECT node_id FROM threads WHERE node_id=$1 OR side_of=$1 ORDER BY node_id FOR UPDATE"
    } else {
        "SELECT node_id FROM threads WHERE node_id=$1 OR side_of=$1 ORDER BY node_id"
    };
    let ids: Vec<Id> = sqlx::query_scalar(sql)
        .bind(main)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let mut lanes = Vec::new();
    for id in ids {
        let row = if locked {
            threads::lock(conn, id).await?
        } else {
            threads::thread(conn, id).await?
        };
        if let Some(row) = row {
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
    reparent_before(conn, lanes, main, target, now, || Ok(())).await
}

/// The native trial/act's first mutation, after its owner acquired the complete tree/lane footprint.
pub async fn reparent_before<F>(
    conn: &mut Conn,
    lanes: &[threads::ThreadRow],
    main: Id,
    target: Id,
    now: chrono::DateTime<chrono::Utc>,
    before: F,
) -> Result<(), EngineError>
where
    F: FnOnce() -> Result<(), EngineError> + Send,
{
    before()?;
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
    write_before(conn, principal, lanes, main, target, marker, key, || Ok(())).await
}

/// The same native write and bookkeeping, with the owner's check at the first real mutation.
#[allow(
    clippy::too_many_arguments,
    reason = "preserve the native move operands and its first-mutation callback"
)]
pub async fn write_before<F>(
    conn: &mut Conn,
    principal: &Principal,
    lanes: &[threads::ThreadRow],
    main: Id,
    target: Id,
    marker: Json,
    key: &str,
    before: F,
) -> Result<(), EngineError>
where
    F: FnOnce() -> Result<(), EngineError> + Send,
{
    let now = principal.now().get();
    reparent_before(conn, lanes, main, target, now, before).await?;
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
