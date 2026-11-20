//! Measured room for the machine and host references admitted by a placement read.
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_api_contract::environment::PlaceRoom;
use orch_types::{Id, Json, Time};
use std::collections::HashMap;

/// The latest machine reports for this set of connected directory machines.
async fn machines(conn: &mut Conn, ids: &[Id]) -> Result<HashMap<Id, PlaceRoom>, DbError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Id, Json, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT id,facts->'room',session_started_at FROM machines
         WHERE id=ANY($1) AND online AND status='active' AND deleted_at IS NULL
           AND lost_at IS NULL AND facts ? 'room'",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, value, session)| {
            let room: PlaceRoom = serde_json::from_value(value).ok()?;
            session
                .is_none_or(|start| room.observed_at >= Time::new(start))
                .then_some((id, room))
        })
        .collect())
}

fn millis(value: f64) -> Option<u64> {
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "nonnegative measured processor time is rounded to whole millicores"
    )]
    Some(value.round() as u64)
}

fn host_room(capacity: &Json, load: &Json, at: DateTime<Utc>) -> PlaceRoom {
    let cpu = capacity["cpu_millis"]
        .as_u64()
        .or_else(|| millis(capacity["cpus"].as_f64()? * 1000.0));
    let used = load["cpu_percent"]
        .as_f64()
        .and_then(|percent| millis(percent * 10.0));
    let memory = capacity["memory_mb"].as_u64().filter(|n| *n > 0);
    PlaceRoom {
        cpu_free_millis: cpu
            .filter(|n| *n > 0)
            .zip(used)
            .map(|(total, used)| total.saturating_sub(used)),
        memory_free_bytes: memory
            .zip(load["memory_used_mb"].as_u64())
            .map(|(total, used)| total.saturating_sub(used).saturating_mul(1024 * 1024)),
        observed_at: Time::new(at),
    }
}

/// Samples from the latest online session of each admitted host.
pub async fn hosts(conn: &mut Conn, ids: &[Id]) -> Result<HashMap<Id, PlaceRoom>, DbError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Id, Json, Json, DateTime<Utc>)> = sqlx::query_as(
        "SELECT h.id,h.capacity,h.load,h.load_at FROM hosts h JOIN machines m ON m.id=h.machine_id
         WHERE h.id=ANY($1) AND m.status='active' AND m.online AND m.deleted_at IS NULL
           AND m.lost_at IS NULL AND h.load_at IS NOT NULL
           AND (m.session_started_at IS NULL OR h.load_at >= m.session_started_at)",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, capacity, load, at)| (id, host_room(&capacity, &load, at)))
        .collect())
}

/// The caller supplies only physical references admitted on the environment choices it will show.
pub async fn for_environments(
    conn: &mut Conn,
    sources: &[(Id, Option<Id>, Option<Id>)],
) -> Result<HashMap<Id, PlaceRoom>, DbError> {
    let hosts = hosts(
        conn,
        &sources
            .iter()
            .filter_map(|(_, host, _)| *host)
            .collect::<Vec<_>>(),
    )
    .await?;
    let machines = machines(
        conn,
        &sources
            .iter()
            .filter_map(|(_, _, machine)| *machine)
            .collect::<Vec<_>>(),
    )
    .await?;
    Ok(sources
        .iter()
        .filter_map(|(environment, host, machine)| {
            let room = match host {
                Some(host) => hosts.get(host),
                None => machines.get(&(*machine)?),
            }?;
            Some((*environment, *room))
        })
        .collect())
}

/// Counts primary running threads once per current environment of these host choices.
pub async fn running_threads(conn: &mut Conn, ids: &[Id]) -> Result<HashMap<Id, i64>, DbError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Id,i64)> = sqlx::query_as(
        "WITH units AS (
             SELECT DISTINCT host_id,environment_id FROM runtimes
             WHERE host_id=ANY($1) AND kind='environment' AND successor_of IS NULL
               AND state NOT IN ('removed','retired')
         )
         SELECT u.host_id,count(*) FROM units u
         JOIN threads t ON t.environment_id=u.environment_id AND t.state='running' AND t.side_of IS NULL
         JOIN nodes n ON n.id=t.node_id AND n.deleted_at IS NULL GROUP BY u.host_id",
    ).bind(ids).fetch_all(conn).await?;
    Ok(rows.into_iter().collect())
}
