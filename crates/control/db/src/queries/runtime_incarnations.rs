//! Original native runtime sources, separate from permissions and requested lifecycle state.

use crate::{Conn, DbError};
use orch_link_contract::native_target::RuntimeSource;
use orch_types::Id;

/// Whether this source may describe the current physical start. The caller holds its
/// provider/environment lock first; this query then serializes the runtime's source facts.
/// Seen superseded sources and closed starts cannot become current again. An untracked
/// observation cannot confirm the state of a tracked start. This grants no operation.
pub async fn observe(
    conn: &mut Conn,
    host: Id,
    runtime: Id,
    source: RuntimeSource,
    terminal: bool,
) -> Result<bool, DbError> {
    let current: Option<(Id, Option<Id>)> =
        sqlx::query_as("SELECT host_id,native_incarnation FROM runtimes WHERE id=$1 FOR UPDATE")
            .bind(runtime)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((owner, current)) = current.filter(|(owner, _)| *owner == host) else {
        return Ok(false);
    };
    let Some(incarnation) = source.incarnation else {
        return Ok(current.is_none());
    };
    if incarnation.is_nil() {
        return Ok(false);
    }
    let inserted = sqlx::query(
        "INSERT INTO runtime_incarnations(id,runtime_id,host_id,closed) VALUES($1,$2,$3,$4)
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(incarnation)
    .bind(runtime)
    .bind(owner)
    .bind(terminal)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        > 0;
    // Read ownership without taking another runtime's source lock: a substituted UUID
    // must not make two independent runtime controls wait for one another's row locks.
    let (source_runtime, source_host, closed): (Id, Id, bool) =
        sqlx::query_as("SELECT runtime_id,host_id,closed FROM runtime_incarnations WHERE id=$1")
            .bind(incarnation)
            .fetch_one(&mut *conn)
            .await?;
    if source_runtime != runtime || source_host != host {
        return Ok(false);
    }
    if !inserted && (current != Some(incarnation) || (closed && !terminal)) {
        return Ok(false);
    }
    if inserted {
        if let Some(previous) = current {
            sqlx::query("UPDATE runtime_incarnations SET closed=true WHERE id=$1 AND runtime_id=$2 AND host_id=$3")
                .bind(previous).bind(runtime).bind(host).execute(&mut *conn).await?;
        }
        sqlx::query("UPDATE runtimes SET native_incarnation=$2 WHERE id=$1")
            .bind(runtime)
            .bind(incarnation)
            .execute(&mut *conn)
            .await?;
    } else if terminal && !closed {
        sqlx::query("UPDATE runtime_incarnations SET closed=true WHERE id=$1 AND runtime_id=$2 AND host_id=$3")
            .bind(incarnation).bind(runtime).bind(host).execute(&mut *conn).await?;
    }
    Ok(true)
}

/// One accepted stop continuation. Its identifier joins receipts, not physical starts or permissions.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stop {
    pub id: Id,
    pub source: RuntimeSource,
    pub control: super::runtime_controls::Intent,
    pub revision: i64,
}

fn decode_stop(value: orch_types::Json) -> Result<Stop, DbError> {
    let stop: Stop = serde_json::from_value(value).map_err(|_| DbError::Rule {
        constraint: "runtime_native_stop".into(),
    })?;
    if stop.id.is_nil()
        || stop.revision <= 0
        || stop.source != stop.control.source()
        || stop.source.incarnation.is_some_and(|id| id.is_nil())
    {
        return Err(DbError::Rule {
            constraint: "runtime_native_stop".into(),
        });
    }
    Ok(stop)
}

/// The current recorded source; an absent runtime differs from an explicitly untracked unit.
pub async fn current(conn: &mut Conn, runtime: Id) -> Result<Option<RuntimeSource>, DbError> {
    let source: Option<Option<Id>> =
        sqlx::query_scalar("SELECT native_incarnation FROM runtimes WHERE id=$1")
            .bind(runtime)
            .fetch_optional(conn)
            .await?;
    Ok(source.map(|incarnation| RuntimeSource { incarnation }))
}

/// Starts or joins a stop of exactly this source. Hold its resource parent first.
/// A replacement cannot inherit a pending stop merely because its logical ID is unchanged.
pub async fn begin_stop(
    conn: &mut Conn,
    runtime: Id,
    control: &super::runtime_controls::Intent,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<bool, DbError> {
    if control.runtime() != runtime {
        return Ok(false);
    }
    let source = control.source();
    let row: Option<(Option<Id>, String, Option<orch_types::Json>)> = sqlx::query_as(
        "SELECT native_incarnation,state,native_stop FROM runtimes WHERE id=$1 FOR UPDATE",
    )
    .bind(runtime)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((current, state, previous)) = row else {
        return Ok(false);
    };
    if current != source.incarnation
        || !matches!(state.as_str(), "running" | "stopping")
        || !super::runtime_controls::is_current(&mut *conn, control).await?
    {
        return Ok(false);
    }
    if state == "stopping"
        && let Some(previous) = previous
    {
        let previous = decode_stop(previous)?;
        if previous.source == source && previous.control == *control {
            return Ok(true);
        }
    }
    let id = orch_types::new_id();
    let Some(revision) = super::runtime_controls::phase(
        &mut *conn,
        control,
        id,
        &serde_json::json!({"kind":"node.runtime.stop","runtime_id":runtime,"source":source}),
    )
    .await?
    else {
        return Ok(false);
    };
    let stop = Stop {
        id,
        source,
        control: control.clone(),
        revision,
    };
    let value = serde_json::to_value(stop).map_err(|_| DbError::Rule {
        constraint: "runtime_native_stop".into(),
    })?;
    sqlx::query("UPDATE runtimes SET state='stopping',state_detail=NULL,native_stop=$2,updated_at=$3 WHERE id=$1")
        .bind(runtime).bind(value).bind(at).execute(conn).await?;
    Ok(true)
}

/// Reads the original committed stop after a wait or an engine restart, without recapturing its target.
pub async fn stop(conn: &mut Conn, runtime: Id) -> Result<Option<Stop>, DbError> {
    let value: Option<orch_types::Json> = sqlx::query_scalar::<_, Option<orch_types::Json>>(
        "SELECT native_stop FROM runtimes WHERE id=$1",
    )
    .bind(runtime)
    .fetch_optional(conn)
    .await?
    .flatten();
    let stop = value.map(decode_stop).transpose()?;
    if stop
        .as_ref()
        .is_some_and(|stop| stop.control.runtime() != runtime)
    {
        return Err(DbError::Rule {
            constraint: "runtime_native_stop".into(),
        });
    }
    Ok(stop)
}
