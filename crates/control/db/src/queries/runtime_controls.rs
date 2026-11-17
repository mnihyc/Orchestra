//! Immutable original lifecycle intent and sparse phase order, separate from current authority.

use crate::{Conn, DbError};
use orch_link_contract::native_target::RuntimeSource;
use orch_types::{Id, Json};

/// Original operation facts retained across its own waits and native phases. Never a grant.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    id: Id,
    runtime: Id,
    host: Id,
    machine: Id,
    source: RuntimeSource,
    accepted_revision: i64,
}
impl Intent {
    /// The addressed runtime, never a later logical lookup.
    #[must_use]
    pub const fn runtime(&self) -> Id {
        self.runtime
    }

    /// The physical Host association selected by the first acceptance.
    #[must_use]
    pub const fn host(&self) -> Id {
        self.host
    }

    /// The receiving node selected by the first acceptance, never a later enrollment lookup.
    #[must_use]
    pub const fn machine(&self) -> Id {
        self.machine
    }

    /// The physical source captured by the first acceptance, including explicit untracked.
    #[must_use]
    pub const fn source(&self) -> RuntimeSource {
        self.source
    }
}

fn invalid() -> DbError {
    DbError::Rule {
        constraint: "runtime_original_control".into(),
    }
}
fn next(order: i64) -> Result<i64, DbError> {
    order
        .checked_add(1)
        .filter(|order| *order > 0)
        .ok_or_else(invalid)
}

async fn read(conn: &mut Conn, id: Id) -> Result<Option<Intent>, DbError> {
    let row: Option<(Id, Id, Id, Option<Id>, i64)> = sqlx::query_as(
        "SELECT runtime_id,host_id,machine_id,source_incarnation,accepted_revision FROM runtime_control_intents WHERE id=$1")
        .bind(id).fetch_optional(conn).await?;
    Ok(row.map(
        |(runtime, host, machine, incarnation, accepted_revision)| Intent {
            id,
            runtime,
            host,
            machine,
            source: RuntimeSource { incarnation },
            accepted_revision,
        },
    ))
}

/// Captures once under the caller's resource-parent lock, then the runtime lock.
/// Retrying an old ID reads its original facts and never makes it current again.
pub async fn accept(conn: &mut Conn, runtime: Id, id: Id) -> Result<Intent, DbError> {
    if !super::transaction_guard::in_transaction(conn) || id.is_nil() || runtime.is_nil() {
        return Err(invalid());
    }
    let row: Option<(Option<Id>, i64,Id,Id)> = sqlx::query_as(
        "SELECT r.native_incarnation,r.native_control_revision,r.host_id,h.machine_id FROM runtimes r JOIN hosts h ON h.id=r.host_id WHERE r.id=$1 FOR UPDATE OF r",
    )
    .bind(runtime)
    .fetch_optional(&mut *conn)
    .await?;
    let (source, order, host, machine) = row.ok_or_else(invalid)?;
    if let Some(original) = read(&mut *conn, id).await? {
        if original.runtime != runtime {
            return Err(invalid());
        }
        return Ok(original);
    }
    if let Some(incarnation) = source {
        let owned:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_incarnations WHERE id=$1 AND runtime_id=$2 AND host_id=$3)")
            .bind(incarnation).bind(runtime).bind(host).fetch_one(&mut *conn).await?;
        if !owned {
            return Err(invalid());
        }
    }
    let order = next(order)?;
    let inserted = sqlx::query(
        "INSERT INTO runtime_control_intents(id,runtime_id,source_incarnation,accepted_revision,host_id,machine_id)
         VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(id) DO NOTHING",
    )
    .bind(id)
    .bind(runtime)
    .bind(source)
    .bind(order)
    .bind(host)
    .bind(machine)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        > 0;
    let original = read(&mut *conn, id).await?.ok_or_else(invalid)?;
    if original.runtime != runtime {
        return Err(invalid());
    }
    if inserted {
        sqlx::query(
            "UPDATE runtimes SET native_control_intent=$2,native_control_revision=$3 WHERE id=$1",
        )
        .bind(runtime)
        .bind(id)
        .bind(order)
        .execute(conn)
        .await?;
    }
    Ok(original)
}

/// Whether these exact original facts still name the current intent. This is not authority.
pub async fn is_current(conn: &mut Conn, intent: &Intent) -> Result<bool, DbError> {
    if read(&mut *conn, intent.id).await?.as_ref() != Some(intent) {
        return Err(invalid());
    }
    let current: Option<(Option<Id>,Id,Id)> =
        sqlx::query_as("SELECT r.native_control_intent,r.host_id,h.machine_id FROM runtimes r JOIN hosts h ON h.id=r.host_id WHERE r.id=$1")
            .bind(intent.runtime)
            .fetch_optional(conn)
            .await?;
    Ok(current == Some((Some(intent.id), intent.host, intent.machine)))
}

/// Freezes a phase's operands and order only while its original intent is current.
/// A retry retains that exact phase; an independent newer intent prevents an old continuation
/// allocating a later order. Callers still check original authority and actual physical identity
/// after every queue and join each prerequisite effect before preparing its continuation.
pub async fn phase(
    conn: &mut Conn,
    intent: &Intent,
    id: Id,
    operands: &Json,
) -> Result<Option<i64>, DbError> {
    if !super::transaction_guard::in_transaction(conn) || id.is_nil() || !operands.is_object() {
        return Err(invalid());
    }
    let row: Option<(Option<Id>, i64)> = sqlx::query_as(
        "SELECT native_control_intent,native_control_revision FROM runtimes WHERE id=$1 FOR UPDATE",
    )
    .bind(intent.runtime)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((current, order)) = row else {
        return Ok(None);
    };
    if read(&mut *conn, intent.id).await?.as_ref() != Some(intent) {
        return Err(invalid());
    }
    let association: Option<(Id,Id)>=sqlx::query_as("SELECT r.host_id,h.machine_id FROM runtimes r JOIN hosts h ON h.id=r.host_id WHERE r.id=$1")
        .bind(intent.runtime).fetch_optional(&mut *conn).await?;
    let original_receiver = association == Some((intent.host, intent.machine));
    let previous: Option<(Id, i64, Json)> = sqlx::query_as(
        "SELECT intent_id,revision,operands FROM runtime_control_phases WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((owner, previous, original)) = previous {
        if owner != intent.id || original != *operands {
            return Err(invalid());
        }
        return Ok((current == Some(intent.id) && original_receiver).then_some(previous));
    }
    if current != Some(intent.id) || !original_receiver {
        return Ok(None);
    }
    let order = next(order)?;
    let inserted = sqlx::query(
        "INSERT INTO runtime_control_phases(id,intent_id,revision,operands) VALUES($1,$2,$3,$4)
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(id)
    .bind(intent.id)
    .bind(order)
    .bind(operands)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        > 0;
    if !inserted {
        // A UUID collision on a different runtime must not acquire its row lock or borrow its order.
        let (owner, existing, original): (Id, i64, Json) = sqlx::query_as(
            "SELECT intent_id,revision,operands FROM runtime_control_phases WHERE id=$1",
        )
        .bind(id)
        .fetch_one(conn)
        .await?;
        if owner != intent.id || original != *operands {
            return Err(invalid());
        }
        return Ok(Some(existing));
    }
    sqlx::query("UPDATE runtimes SET native_control_revision=$2 WHERE id=$1")
        .bind(intent.runtime)
        .bind(order)
        .execute(conn)
        .await?;
    Ok(Some(order))
}

#[derive(sqlx::FromRow)]
struct NativeFacts {
    intent: Id,
    runtime: Id,
    original_source: Option<Id>,
    revision: i64,
    operands: Json,
    current_intent: Option<Id>,
    current_revision: i64,
    host: Id,
    machine: Id,
    original_host: Id,
    original_machine: Id,
}

/// Validates original lifecycle operands before a first queue write. These facts confer no authority;
/// the existing command issuer still checks its canonical operation and caller.
pub(crate) async fn command_facts(
    conn: &mut Conn,
    command: &super::link::NewCommand<'_>,
) -> Result<Option<Json>, DbError> {
    use orch_link_contract::native_target::{LifecycleAction, LifecycleSubject, NativeTarget};
    let Some(NativeTarget::Lifecycle(target)) = command.native_target else {
        return Ok(None);
    };
    let action = match command.kind {
        "node.runtime.start" => LifecycleAction::Start,
        "node.runtime.stop" => LifecycleAction::Stop,
        "node.runtime.remove" => LifecycleAction::Remove,
        _ => return Err(invalid()),
    };
    let LifecycleSubject::Runtime { runtime_id } = target.subject else {
        return Err(invalid());
    };
    let expected_key = orch_link_contract::commands::SerialKey::Runtime(runtime_id).wire();
    if !target.coherent_for(action)
        || command.body.get("runtime_id") != Some(&serde_json::json!(runtime_id))
        || command.serial_key != expected_key.as_deref()
    {
        return Err(invalid());
    }
    let facts: NativeFacts = sqlx::query_as(
        "SELECT i.id AS intent,i.runtime_id AS runtime,i.source_incarnation AS original_source,
         p.revision,p.operands,r.native_control_intent AS current_intent,
         r.native_control_revision AS current_revision,r.host_id AS host,h.machine_id AS machine,
         i.host_id AS original_host,i.machine_id AS original_machine
         FROM runtime_control_phases p JOIN runtime_control_intents i ON i.id=p.intent_id
         JOIN runtimes r ON r.id=i.runtime_id JOIN hosts h ON h.id=r.host_id WHERE p.id=$1",
    )
    .bind(command.id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(invalid)?;
    let source: RuntimeSource =
        serde_json::from_value(facts.operands.get("source").cloned().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
    let opening: Option<Id> = facts
        .operands
        .get("opening")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| invalid())?
        .flatten();
    if facts.runtime != runtime_id
        || facts.machine != command.machine_id
        || facts.host != facts.original_host
        || facts.machine != facts.original_machine
        || facts.current_intent != Some(facts.intent)
        || facts.current_revision != facts.revision
        || facts.revision != target.revision
        || target.observed != source.incarnation
        || target.opening != opening
        || facts.operands.get("kind").and_then(Json::as_str) != Some(command.kind)
        || facts.operands.get("body") != Some(command.body)
    {
        return Err(invalid());
    }
    if source.incarnation != facts.original_source {
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_control_phases WHERE intent_id=$1
             AND operands->>'opening'=$2)",
        )
        .bind(facts.intent)
        .bind(source.incarnation.map(|id| id.to_string()))
        .fetch_one(&mut *conn)
        .await?;
        if !owned {
            return Err(invalid());
        }
    }
    Ok(Some(
        serde_json::to_value(NativeTarget::Lifecycle(target.clone())).map_err(|_| invalid())?,
    ))
}
