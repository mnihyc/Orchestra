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
    source: RuntimeSource,
    accepted_revision: i64,
}
impl Intent {
    /// The addressed runtime, never a later logical lookup.
    #[must_use]
    pub const fn runtime(&self) -> Id {
        self.runtime
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
    let row: Option<(Id, Option<Id>, i64)> = sqlx::query_as(
        "SELECT runtime_id,source_incarnation,accepted_revision FROM runtime_control_intents WHERE id=$1")
        .bind(id).fetch_optional(conn).await?;
    Ok(row.map(|(runtime, incarnation, accepted_revision)| Intent {
        id,
        runtime,
        source: RuntimeSource { incarnation },
        accepted_revision,
    }))
}

/// Captures once under the caller's resource-parent lock, then the runtime lock.
/// Retrying an old ID reads its original facts and never makes it current again.
pub async fn accept(conn: &mut Conn, runtime: Id, id: Id) -> Result<Intent, DbError> {
    if !super::transaction_guard::in_transaction(conn) || id.is_nil() || runtime.is_nil() {
        return Err(invalid());
    }
    let row: Option<(Option<Id>, i64)> = sqlx::query_as(
        "SELECT native_incarnation,native_control_revision FROM runtimes WHERE id=$1 FOR UPDATE",
    )
    .bind(runtime)
    .fetch_optional(&mut *conn)
    .await?;
    let (source, order) = row.ok_or_else(invalid)?;
    if let Some(original) = read(&mut *conn, id).await? {
        if original.runtime != runtime {
            return Err(invalid());
        }
        return Ok(original);
    }
    let order = next(order)?;
    let inserted = sqlx::query(
        "INSERT INTO runtime_control_intents(id,runtime_id,source_incarnation,accepted_revision)
         VALUES($1,$2,$3,$4) ON CONFLICT(id) DO NOTHING",
    )
    .bind(id)
    .bind(runtime)
    .bind(source)
    .bind(order)
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
    let current: Option<Option<Id>> =
        sqlx::query_scalar("SELECT native_control_intent FROM runtimes WHERE id=$1")
            .bind(intent.runtime)
            .fetch_optional(conn)
            .await?;
    Ok(current == Some(Some(intent.id)))
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
        return Ok((current == Some(intent.id)).then_some(previous));
    }
    if current != Some(intent.id) {
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
    machine: Id,
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
         r.native_control_revision AS current_revision,h.machine_id AS machine
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
    Ok(Some(serde_json::to_value(target).map_err(|_| invalid())?))
}
