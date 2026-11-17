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

/// The pending Stop's native producer, independent of current permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopProducer {
    /// A native workflow that has not supplied a retained direct HTTP command.
    Unbound,
    /// The exact logical Provider Stop, including its original private command carrier.
    ProviderStop,
    /// Its existing issuer refused further work; a fresh request must own a new intent.
    ProviderStopRefused,
}

/// One accepted stop continuation. Its identifier joins receipts, not physical starts or permissions.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stop {
    pub id: Id,
    pub source: RuntimeSource,
    pub control: super::runtime_controls::Intent,
    pub revision: i64,
    pub producer: StopProducer,
    /// The existing private Outgoing command carrier, never runtime report or HTTP content.
    pub command: Option<orch_types::Json>,
}

impl std::fmt::Debug for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stop")
            .field("id", &self.id)
            .field("source", &self.source)
            .field("control", &self.control)
            .field("revision", &self.revision)
            .field("producer", &self.producer)
            .field("command", &self.command.as_ref().map(|_| "[private]"))
            .finish()
    }
}

fn decode_stop(value: orch_types::Json) -> Result<Stop, DbError> {
    let stop: Stop = serde_json::from_value(value).map_err(|_| DbError::Rule {
        constraint: "runtime_native_stop".into(),
    })?;
    if stop.id.is_nil()
        || stop.revision <= 0
        || stop.source != stop.control.source()
        || stop.source.incarnation.is_some_and(|id| id.is_nil())
        || match stop.producer {
            StopProducer::Unbound => stop.command.is_some(),
            StopProducer::ProviderStop | StopProducer::ProviderStopRefused => !stop
                .command
                .as_ref()
                .is_some_and(orch_types::Json::is_object),
        }
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

/// Whether this exact owned source has already supplied terminal native evidence. Not authority.
pub async fn terminal(
    conn: &mut Conn,
    runtime: Id,
    source: RuntimeSource,
) -> Result<bool, DbError> {
    let Some(incarnation) = source.incarnation else {
        return Ok(false);
    };
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_incarnations WHERE id=$1 AND runtime_id=$2 AND closed)")
        .bind(incarnation).bind(runtime).fetch_one(conn).await?)
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
        &serde_json::json!({"kind":"node.runtime.stop","runtime_id":runtime,"source":source,
            "body":{"runtime_id":runtime}}),
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
        producer: StopProducer::Unbound,
        command: None,
    };
    let value = serde_json::to_value(stop).map_err(|_| DbError::Rule {
        constraint: "runtime_native_stop".into(),
    })?;
    sqlx::query("UPDATE runtimes SET state='stopping',state_detail=NULL,native_stop=$2,updated_at=$3 WHERE id=$1")
        .bind(runtime).bind(value).bind(at).execute(conn).await?;
    Ok(true)
}

/// Retains the producer's original command atomically with its pending Stop. This is the
/// existing command carrier for crash continuation, not a second authority evaluator.
/// A retry cannot replace an earlier command, credential, deadline or physical operand.
pub async fn bind_provider_command(
    conn: &mut Conn,
    expected: &Stop,
    command: &orch_types::Json,
) -> Result<bool, DbError> {
    let invalid = || DbError::Rule {
        constraint: "runtime_native_stop".into(),
    };
    if !super::transaction_guard::in_transaction(conn) || !command.is_object() {
        return Err(invalid());
    }
    if expected.producer != StopProducer::Unbound {
        return if expected.command.as_ref() == Some(command) {
            Ok(true)
        } else {
            Err(invalid())
        };
    }
    if expected.command.is_some() {
        return Err(invalid());
    }
    if !super::runtime_controls::is_current(conn, &expected.control).await?
        || current(conn, expected.control.runtime()).await? != Some(expected.source)
    {
        return Ok(false);
    }
    let mut retained = expected.clone();
    retained.producer = StopProducer::ProviderStop;
    retained.command = Some(command.clone());
    let original = serde_json::to_value(expected).map_err(|_| invalid())?;
    let retained = serde_json::to_value(retained).map_err(|_| invalid())?;
    Ok(sqlx::query(
        "UPDATE runtimes SET native_stop=$2 WHERE id=$1 AND state='stopping' AND native_stop=$3",
    )
    .bind(expected.control.runtime())
    .bind(retained)
    .bind(original)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Record the existing issuer's refusal without confirming any physical outcome or changing
/// the original source/control. A later authorized request can accept a fresh intent.
pub async fn refuse_provider_command(conn: &mut Conn, expected: &Stop) -> Result<bool, DbError> {
    if expected.producer != StopProducer::ProviderStop
        || !super::transaction_guard::in_transaction(conn)
        || !super::runtime_controls::is_current(conn, &expected.control).await?
        || current(conn, expected.control.runtime()).await? != Some(expected.source)
    {
        return Ok(false);
    }
    let original = serde_json::to_value(expected).map_err(|_| DbError::Rule {
        constraint: "runtime_native_stop".into(),
    })?;
    let mut refused = expected.clone();
    refused.producer = StopProducer::ProviderStopRefused;
    let refused = serde_json::to_value(refused).map_err(|_| DbError::Rule {
        constraint: "runtime_native_stop".into(),
    })?;
    Ok(sqlx::query("UPDATE runtimes SET native_stop=$2,state_detail=$4 WHERE id=$1 AND state='stopping' AND native_stop=$3")
        .bind(expected.control.runtime()).bind(refused).bind(original)
        .bind("The original Stop can no longer issue native work; retry Stop.")
        .execute(conn).await?.rows_affected() > 0)
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
