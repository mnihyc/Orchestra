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
    pending: Vec<Id>,
    previous: Option<Id>,
    accepted_revision: i64,
}
impl Intent {
    /// The retained original control, for intrinsic database scope comparisons only.
    pub(crate) const fn id(&self) -> Id {
        self.id
    }

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

    /// Earlier admitted possible openings, frozen before this act's waits. Never grants.
    #[must_use]
    pub fn pending(&self) -> &[Id] {
        &self.pending
    }

    /// The prior subject control, not a current resource lookup or a cancellation grant.
    #[must_use]
    pub const fn previous(&self) -> Option<Id> {
        self.previous
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

#[derive(sqlx::FromRow)]
struct StoredIntent {
    runtime_id: Id,
    host_id: Id,
    machine_id: Id,
    source_incarnation: Option<Id>,
    pending_incarnations: Vec<Id>,
    previous_intent: Option<Id>,
    accepted_revision: i64,
}

async fn read(conn: &mut Conn, id: Id) -> Result<Option<Intent>, DbError> {
    let row: Option<StoredIntent> = sqlx::query_as(
        "SELECT runtime_id,host_id,machine_id,source_incarnation,pending_incarnations,previous_intent,accepted_revision FROM runtime_control_intents WHERE id=$1")
        .bind(id).fetch_optional(conn).await?;
    Ok(row.map(|row| Intent {
        id,
        runtime: row.runtime_id,
        host: row.host_id,
        machine: row.machine_id,
        source: RuntimeSource {
            incarnation: row.source_incarnation,
        },
        pending: row.pending_incarnations,
        previous: row.previous_intent,
        accepted_revision: row.accepted_revision,
    }))
}

/// Pending means a real canonical birth or Start was admitted, not merely that JSON contains
/// an opening. Cancellation cannot prove its native effect never happened.
async fn captured_pending(
    conn: &mut Conn,
    runtime: Id,
    host: Id,
    machine: Id,
    current: RuntimeSource,
    previous: Option<Id>,
    revision: i64,
) -> Result<Vec<Id>, DbError> {
    use orch_link_contract::{
        commands::{NodeRuntimeCreateResult, NodeRuntimeStartResult},
        native_target::{LifecycleAction, LifecycleSubject, NativeTarget},
    };
    let Some(previous) = previous else {
        return Ok(Vec::new());
    };
    let original = read(conn, previous).await?.ok_or_else(invalid)?;
    if original.runtime != runtime {
        return Err(invalid());
    }
    if original.host != host || original.machine != machine {
        return Ok(Vec::new());
    }
    let phase: Option<(Id, Json, Option<Json>)> = sqlx::query_as(
        "SELECT id,operands,outcome FROM runtime_control_phases WHERE intent_id=$1 AND revision=$2",
    )
    .bind(previous)
    .bind(revision)
    .fetch_optional(&mut *conn)
    .await?;
    let mut related = current == original.source
        || current
            .incarnation
            .is_some_and(|id| original.pending.contains(&id));
    let mut pending = original.pending;
    if let Some((command, operands, outcome)) = phase
        && matches!(
            operands.get("kind").and_then(Json::as_str),
            Some("node.runtime.start" | "node.runtime.create")
        )
        && let Some(row) = super::link::command(conn, command).await?
    {
        let creating = operands.get("kind").and_then(Json::as_str) == Some("node.runtime.create");
        let kind = if creating {
            "node.runtime.create"
        } else {
            "node.runtime.start"
        };
        let action = if creating {
            LifecycleAction::Create
        } else {
            LifecycleAction::Start
        };
        let observed: RuntimeSource =
            serde_json::from_value(operands.get("source").cloned().ok_or_else(invalid)?)
                .map_err(|_| invalid())?;
        let opening: Id =
            serde_json::from_value(operands.get("opening").cloned().ok_or_else(invalid)?)
                .map_err(|_| invalid())?;
        if opening.is_nil() || observed.incarnation == Some(opening) {
            return Err(invalid());
        }
        related |= current == observed || current.incarnation == Some(opening);
        if !related {
            return Ok(Vec::new());
        }
        let target = super::session_dependencies::command_native_target(conn, command).await?;
        let key = orch_link_contract::commands::SerialKey::Runtime(runtime).wire();
        if row.kind != kind
            || row.machine_id != machine
            || row.serial_key() != key.as_deref()
            || row.body.get("runtime_id") != Some(&serde_json::json!(runtime))
            || operands.get("body") != Some(&row.body)
            || !matches!(target, Some(NativeTarget::Lifecycle(ref target))
                    if target.coherent_for(action) && target.subject==(LifecycleSubject::Runtime {runtime_id:runtime})
                        && target.observed==observed.incarnation && target.opening==Some(opening) && target.revision==revision
                        && target.pending == if observed == original.source {pending.clone()} else {Vec::new()})
        {
            return Err(invalid());
        }
        let result = outcome.as_ref().or_else(|| {
            (row.state == "succeeded")
                .then_some(row.result.as_ref())
                .flatten()
        });
        if let Some(result) = result {
            let (actual, reused) = if creating {
                let result: NodeRuntimeCreateResult =
                    serde_json::from_value(result.clone()).map_err(|_| invalid())?;
                if result.unit.runtime_id != runtime
                    || result.unit.reference.is_empty()
                    || row.body.get("name").and_then(Json::as_str)
                        != Some(result.unit.name.as_str())
                    || row.body.get("kind")
                        != Some(&serde_json::to_value(result.unit.kind).map_err(|_| invalid())?)
                {
                    return Err(invalid());
                }
                (result.source, false)
            } else {
                let result: NodeRuntimeStartResult =
                    serde_json::from_value(result.clone()).map_err(|_| invalid())?;
                (result.source, result.reused)
            };
            if actual.incarnation.is_none()
                || if reused {
                    actual != observed
                        && !pending.contains(&actual.incarnation.ok_or_else(invalid)?)
                } else {
                    actual.incarnation != Some(opening)
                }
            {
                return Err(invalid());
            }
            if actual == current {
                // Actual native evidence resolves the possible earlier openings.
                return Ok(Vec::new());
            }
            if !reused {
                pending.push(opening);
            }
        } else {
            pending.push(opening);
        }
    }
    if !related {
        return Ok(Vec::new());
    }
    pending.retain(|id| Some(*id) != current.incarnation);
    // This is canonical set equality only, never an order of physical observations.
    pending.sort_unstable();
    pending.dedup();
    Ok(pending)
}

#[derive(sqlx::FromRow)]
struct AcceptedRuntime {
    source: Option<Id>,
    revision: i64,
    host: Id,
    machine: Id,
    previous: Option<Id>,
}

/// Captures once under the caller's resource-parent lock, then the runtime lock.
/// Retrying an old ID reads its original facts and never makes it current again.
pub async fn accept(conn: &mut Conn, runtime: Id, id: Id) -> Result<Intent, DbError> {
    if !super::transaction_guard::in_transaction(conn) || id.is_nil() || runtime.is_nil() {
        return Err(invalid());
    }
    let row: Option<AcceptedRuntime> = sqlx::query_as(
        "SELECT r.native_incarnation AS source,r.native_control_revision AS revision,r.host_id AS host,h.machine_id AS machine,r.native_control_intent AS previous FROM runtimes r JOIN hosts h ON h.id=r.host_id WHERE r.id=$1 FOR UPDATE OF r",
    )
    .bind(runtime)
    .fetch_optional(&mut *conn)
    .await?;
    let AcceptedRuntime {
        source,
        revision: order,
        host,
        machine,
        previous,
    } = row.ok_or_else(invalid)?;
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
    let pending = captured_pending(
        conn,
        runtime,
        host,
        machine,
        RuntimeSource {
            incarnation: source,
        },
        previous,
        order,
    )
    .await?;
    let order = next(order)?;
    let inserted = sqlx::query(
        "INSERT INTO runtime_control_intents(id,runtime_id,source_incarnation,accepted_revision,host_id,machine_id,previous_intent,pending_incarnations)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(id) DO NOTHING",
    )
    .bind(id)
    .bind(runtime)
    .bind(source)
    .bind(order)
    .bind(host)
    .bind(machine)
    .bind(previous)
    .bind(&pending)
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

/// Bind a job's actual target at its original runtime admission. The address does not
/// pin an expired job row, and an older unowned intent is never retrofitted or revived.
pub async fn accept_job(conn: &mut Conn, runtime: Id, id: Id, job: Id) -> Result<Intent, DbError> {
    if job.is_nil() {
        return Err(invalid());
    }
    let original = accept(conn, runtime, id).await?;
    let (owner, born): (Option<Id>, bool) = sqlx::query_as(
        "SELECT owner_job_id,xmin=((pg_current_xact_id()::text::numeric % 4294967296)::text)::xid
         FROM runtime_control_intents WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    if owner == Some(job) {
        return Ok(original);
    }
    if owner.is_some() || !born {
        return Err(invalid());
    }
    sqlx::query(
        "UPDATE runtime_control_intents SET owner_job_id=$2 WHERE id=$1 AND owner_job_id IS NULL",
    )
    .bind(id)
    .bind(job)
    .execute(conn)
    .await?;
    Ok(original)
}

/// Only this subject's exact admitted single-resource child can be withdrawn.
/// A broad parent or another resource never becomes a cancellation target here.
pub async fn previous_active_job(conn: &mut Conn, intent: &Intent) -> Result<Option<Id>, DbError> {
    if !is_current(conn, intent).await? {
        return Ok(None);
    }
    let Some(previous) = intent.previous else {
        return Ok(None);
    };
    let Some(original) = read(conn, previous).await? else {
        return Err(invalid());
    };
    if original.runtime != intent.runtime
        || original.host != intent.host
        || original.machine != intent.machine
    {
        return Ok(None);
    }
    Ok(sqlx::query_scalar("SELECT j.id FROM runtime_control_intents i JOIN jobs j ON j.id=i.owner_job_id
        JOIN runtimes r ON r.id=i.runtime_id WHERE i.id=$1 AND r.id=$2 AND j.state IN ('queued','running')
          AND j.kind IN ('environment_provision','provider_provision') AND j.plan->>'runtime'=$2::uuid::text
          AND j.environment_id IS NOT DISTINCT FROM r.environment_id AND j.provider_id IS NOT DISTINCT FROM r.provider_id")
        .bind(previous).bind(intent.runtime).fetch_optional(conn).await?)
}

/// The accepting resource operation owns this withdrawal; caller identity is supplied
/// by that actual request, while the child's original actor and source remain unchanged.
pub async fn withdraw_previous_job(
    conn: &mut Conn,
    intent: &Intent,
    by: Option<Id>,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<bool, DbError> {
    if !super::transaction_guard::in_transaction(conn) {
        return Err(invalid());
    }
    let Some(job) = previous_active_job(conn, intent).await? else {
        return Ok(false);
    };
    super::jobs::request_cancel_by(conn, job, by, at).await
}

/// Read the job's frozen target; a missing original is never recaptured from today's unit.
pub async fn for_job(conn: &mut Conn, job: Id, runtime: Id) -> Result<Option<Intent>, DbError> {
    let id: Option<Id> = sqlx::query_scalar(
        "SELECT id FROM runtime_control_intents WHERE owner_job_id=$1 AND runtime_id=$2",
    )
    .bind(job)
    .bind(runtime)
    .fetch_optional(&mut *conn)
    .await?;
    match id {
        Some(id) => read(conn, id).await,
        None => Ok(None),
    }
}

/// Current physical facts may be the original source, a validated own result, or an
/// admitted own pending Start. An unrelated report never becomes this job's operand.
pub async fn owns_current_source(conn: &mut Conn, intent: &Intent) -> Result<bool, DbError> {
    if !is_current(conn, intent).await? {
        return Ok(false);
    }
    let Some(current) = super::runtime_incarnations::current(conn, intent.runtime).await? else {
        return Ok(false);
    };
    if current == intent.source
        || current
            .incarnation
            .is_some_and(|id| intent.pending.contains(&id))
    {
        return Ok(true);
    }
    let source = serde_json::to_value(current).map_err(|_| invalid())?;
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_control_phases p JOIN runtimes r ON r.id=$4 WHERE p.intent_id=$1
        AND (p.outcome->'source'=$2 OR (p.operands->>'kind'='node.runtime.start' AND p.outcome IS NULL
            AND p.revision=r.native_control_revision AND p.operands->>'opening'=$3)))")
        .bind(intent.id).bind(source).bind(current.incarnation.map(|id|id.to_string())).bind(intent.runtime).fetch_one(conn).await?)
}

/// A native Create may report before its canonical reply is consumed. This allows
/// only continuation to that already queued command; it is not physical ownership
/// for Start or cleanup and never supplies a replacement source as operands.
pub async fn awaiting_creation(conn: &mut Conn, intent: &Intent) -> Result<bool, DbError> {
    if !is_current(conn, intent).await? {
        return Ok(false);
    }
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_control_phases p JOIN commands c ON c.id=p.id
        WHERE p.intent_id=$1 AND p.outcome IS NULL AND p.operands->>'kind'='node.runtime.create'
          AND c.kind='node.runtime.create' AND c.machine_id=$2 AND c.body=p.operands->'body'
          AND c.state IN ('queued','sent','accepted','succeeded'))",
    )
    .bind(intent.id)
    .bind(intent.machine)
    .fetch_one(conn)
    .await?)
}

#[derive(sqlx::FromRow)]
struct OutcomePhase {
    intent: Id,
    revision: i64,
    operands: Json,
    outcome: Option<Json>,
}

/// Freeze only the original successful native command's exact result, once. Later
/// archival of its public result cannot erase the phase's validated factual receipt.
async fn native_outcome(
    conn: &mut Conn,
    intent: &Intent,
    command: Id,
    kind: &str,
    supplied: &Json,
) -> Result<OutcomePhase, DbError> {
    if !super::transaction_guard::in_transaction(conn)
        || read(conn, intent.id).await?.as_ref() != Some(intent)
    {
        return Err(invalid());
    }
    let phase: OutcomePhase = sqlx::query_as(
        "SELECT intent_id AS intent,revision,operands,outcome FROM runtime_control_phases WHERE id=$1 FOR UPDATE")
        .bind(command).fetch_optional(&mut *conn).await?.ok_or_else(invalid)?;
    if phase.intent != intent.id
        || phase.operands.get("kind").and_then(Json::as_str) != Some(kind)
        || phase.operands.pointer("/body/runtime_id") != Some(&serde_json::json!(intent.runtime))
    {
        return Err(invalid());
    }
    if let Some(outcome) = &phase.outcome {
        return if outcome == supplied {
            Ok(phase)
        } else {
            Err(invalid())
        };
    }
    let row = super::link::command(conn, command)
        .await?
        .ok_or_else(invalid)?;
    let key = orch_link_contract::commands::SerialKey::Runtime(intent.runtime).wire();
    if row.kind != kind
        || row.machine_id != intent.machine
        || row.serial_key() != key.as_deref()
        || row.state != "succeeded"
        || row.result.as_ref() != Some(supplied)
        || phase.operands.get("body") != Some(&row.body)
    {
        return Err(invalid());
    }
    Ok(phase)
}

async fn freeze_outcome(conn: &mut Conn, command: Id, supplied: &Json) -> Result<(), DbError> {
    sqlx::query("UPDATE runtime_control_phases SET outcome=$2 WHERE id=$1 AND outcome IS NULL")
        .bind(command)
        .bind(supplied)
        .execute(conn)
        .await?;
    Ok(())
}

async fn owns_source(
    conn: &mut Conn,
    intent: &Intent,
    source: RuntimeSource,
    closed: bool,
) -> Result<(), DbError> {
    let Some(source) = source.incarnation else {
        return Ok(());
    };
    sqlx::query("INSERT INTO runtime_incarnations(id,runtime_id,host_id,closed) VALUES($1,$2,$3,$4) ON CONFLICT(id) DO NOTHING")
        .bind(source).bind(intent.runtime).bind(intent.host)
        .bind(closed)
        .execute(&mut *conn).await?;
    let owner: (Id, Id) =
        sqlx::query_as("SELECT runtime_id,host_id FROM runtime_incarnations WHERE id=$1")
            .bind(source)
            .fetch_one(conn)
            .await?;
    if owner != (intent.runtime, intent.host) {
        return Err(invalid());
    }
    Ok(())
}

async fn publish_source(
    conn: &mut Conn,
    intent: &Intent,
    source: RuntimeSource,
) -> Result<(), DbError> {
    let Some(next) = source.incarnation else {
        return Ok(());
    };
    sqlx::query("UPDATE runtime_incarnations SET closed=true WHERE runtime_id=$1 AND id=(SELECT native_incarnation FROM runtimes WHERE id=$1) AND id<>$2")
        .bind(intent.runtime).bind(next).execute(&mut *conn).await?;
    sqlx::query("UPDATE runtimes SET native_incarnation=$2 WHERE id=$1")
        .bind(intent.runtime)
        .bind(next)
        .execute(conn)
        .await?;
    Ok(())
}

/// The original Create result owns its unit/source; a later reported source is never
/// adopted as its replacement. Untracked remains untracked, not known absence.
pub async fn bind_created(
    conn: &mut Conn,
    intent: &Intent,
    command: Id,
    supplied: &Json,
) -> Result<Option<RuntimeSource>, DbError> {
    use orch_link_contract::commands::NodeRuntimeCreateResult;
    let phase = native_outcome(conn, intent, command, "node.runtime.create", supplied).await?;
    let result: NodeRuntimeCreateResult =
        serde_json::from_value(supplied.clone()).map_err(|_| invalid())?;
    let runtime = super::environments::hosting::runtime(conn, intent.runtime)
        .await?
        .ok_or_else(invalid)?;
    let body = phase.operands.get("body").ok_or_else(invalid)?;
    let opening: Id =
        serde_json::from_value(phase.operands.get("opening").cloned().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
    if opening.is_nil()
        || result.source.incarnation != Some(opening)
        || result.unit.runtime_id != intent.runtime
        || result.unit.reference.is_empty()
        || result.unit.name != runtime.unit_name
        || result.unit.name
            != body
                .get("name")
                .and_then(Json::as_str)
                .ok_or_else(invalid)?
        || serde_json::to_value(result.unit.kind).map_err(|_| invalid())?
            != serde_json::json!(runtime.kind)
        || body.get("kind") != Some(&serde_json::to_value(result.unit.kind).map_err(|_| invalid())?)
        || body.get("driver").and_then(Json::as_str) != Some(runtime.driver.as_str())
    {
        return Err(invalid());
    }
    freeze_outcome(conn, command, supplied).await?;
    if !is_current(conn, intent).await? {
        return Ok(None);
    }
    let current = super::runtime_incarnations::current(conn, intent.runtime)
        .await?
        .ok_or_else(invalid)?;
    if current == result.source || current.incarnation.is_none() {
        owns_source(conn, intent, result.source, false).await?;
        publish_source(conn, intent, result.source).await?;
        return Ok(Some(result.source));
    }
    owns_source(conn, intent, result.source, true).await?;
    // A replay of the same Create->Start phase may already have its own B current.
    // It still returns original A as operands, never rolls the pointer back from B.
    let own_start: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_control_phases p JOIN runtimes r ON r.id=$1
        WHERE p.intent_id=$2 AND p.revision=r.native_control_revision AND p.operands->>'kind'='node.runtime.start'
          AND p.operands->'source'=$3 AND p.operands->>'opening'=$4
          AND (p.outcome IS NULL OR p.outcome->'source'=$5))")
        .bind(intent.runtime).bind(intent.id).bind(serde_json::to_value(result.source).map_err(|_| invalid())?)
        .bind(current.incarnation.map(|id| id.to_string())).bind(serde_json::to_value(current).map_err(|_| invalid())?).fetch_one(conn).await?;
    Ok(own_start.then_some(result.source))
}

/// Choose a fresh allocation once before its canonical command is queued. A missing
/// observed source is only an expected slot; the native producer must prove it fresh.
pub async fn create_phase(
    conn: &mut Conn,
    intent: &Intent,
    command: Id,
    body: &Json,
) -> Result<Option<orch_link_contract::native_target::LifecycleTarget>, DbError> {
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget};
    if intent.source.incarnation.is_some()
        || !intent.pending.is_empty()
        || body.get("runtime_id") != Some(&serde_json::json!(intent.runtime))
    {
        return Err(invalid());
    }
    let previous: Option<Json> =
        sqlx::query_scalar("SELECT operands FROM runtime_control_phases WHERE id=$1")
            .bind(command)
            .fetch_optional(&mut *conn)
            .await?;
    let operands=previous.unwrap_or_else(||serde_json::json!({"kind":"node.runtime.create","source":intent.source,"opening":orch_types::new_id(),"body":body}));
    if operands.get("kind").and_then(Json::as_str) != Some("node.runtime.create")
        || operands.get("source")
            != Some(&serde_json::to_value(intent.source).map_err(|_| invalid())?)
        || operands.get("body") != Some(body)
    {
        return Err(invalid());
    }
    let opening: Id = serde_json::from_value(operands.get("opening").cloned().ok_or_else(invalid)?)
        .map_err(|_| invalid())?;
    if opening.is_nil() {
        return Err(invalid());
    }
    let Some(revision) = phase(conn, intent, command, &operands).await? else {
        return Ok(None);
    };
    Ok(Some(LifecycleTarget {
        subject: LifecycleSubject::Runtime {
            runtime_id: intent.runtime,
        },
        observed: None,
        opening: Some(opening),
        pending: Vec::new(),
        revision,
    }))
}

/// Choose an opening once under the original intent, before its first native effect.
pub async fn start_phase(
    conn: &mut Conn,
    intent: &Intent,
    command: Id,
    source: RuntimeSource,
    body: &Json,
) -> Result<Option<orch_link_contract::native_target::LifecycleTarget>, DbError> {
    use orch_link_contract::native_target::{LifecycleSubject, LifecycleTarget};
    if source.incarnation.is_none()
        || body.get("runtime_id") != Some(&serde_json::json!(intent.runtime))
    {
        return Err(invalid());
    }
    let previous: Option<Json> =
        sqlx::query_scalar("SELECT operands FROM runtime_control_phases WHERE id=$1")
            .bind(command)
            .fetch_optional(&mut *conn)
            .await?;
    let operands = previous.unwrap_or_else(|| {
        serde_json::json!({"kind":"node.runtime.start",
        "source":source,"opening":orch_types::new_id(),"body":body})
    });
    if operands.get("kind").and_then(Json::as_str) != Some("node.runtime.start")
        || operands.get("source") != Some(&serde_json::to_value(source).map_err(|_| invalid())?)
        || operands.get("body") != Some(body)
    {
        return Err(invalid());
    }
    let opening: Id = serde_json::from_value(operands.get("opening").cloned().ok_or_else(invalid)?)
        .map_err(|_| invalid())?;
    if opening.is_nil() || source.incarnation == Some(opening) {
        return Err(invalid());
    }
    let Some(revision) = phase(conn, intent, command, &operands).await? else {
        return Ok(None);
    };
    Ok(Some(LifecycleTarget {
        pending: if source == intent.source {
            intent.pending.clone()
        } else {
            Vec::new()
        },
        subject: LifecycleSubject::Runtime {
            runtime_id: intent.runtime,
        },
        observed: source.incarnation,
        opening: Some(opening),
        revision,
    }))
}

/// Preserve the actual new/reused result, never a source reread after native IO.
pub async fn bind_started(
    conn: &mut Conn,
    intent: &Intent,
    command: Id,
    supplied: &Json,
) -> Result<Option<orch_link_contract::commands::NodeRuntimeStartResult>, DbError> {
    use orch_link_contract::commands::NodeRuntimeStartResult;
    let phase = native_outcome(conn, intent, command, "node.runtime.start", supplied).await?;
    let result: NodeRuntimeStartResult =
        serde_json::from_value(supplied.clone()).map_err(|_| invalid())?;
    let source: RuntimeSource =
        serde_json::from_value(phase.operands.get("source").cloned().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
    let opening: Id =
        serde_json::from_value(phase.operands.get("opening").cloned().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
    let matched = if result.reused {
        result.source == source
            || (source == intent.source
                && result
                    .source
                    .incarnation
                    .is_some_and(|id| intent.pending.contains(&id)))
    } else {
        result.source.incarnation == Some(opening)
    };
    if result.source.incarnation.is_none() || !matched {
        return Err(invalid());
    }
    freeze_outcome(conn, command, supplied).await?;
    if !is_current(conn, intent).await? {
        return Ok(None);
    }
    let latest: i64 =
        sqlx::query_scalar("SELECT native_control_revision FROM runtimes WHERE id=$1 FOR UPDATE")
            .bind(intent.runtime)
            .fetch_one(&mut *conn)
            .await?;
    if latest != phase.revision {
        return Ok(None);
    }
    let current = super::runtime_incarnations::current(conn, intent.runtime)
        .await?
        .ok_or_else(invalid)?;
    if current != source
        && current != result.source
        && !current
            .incarnation
            .is_some_and(|id| intent.pending.contains(&id))
    {
        return Ok(None);
    }
    owns_source(conn, intent, result.source, false).await?;
    if super::runtime_incarnations::terminal(conn, intent.runtime, result.source).await? {
        return Ok(None);
    }
    publish_source(conn, intent, result.source).await?;
    Ok(Some(result))
}

/// Consume the exact canonical native Stop outcome for this original footprint.
/// Historical evidence may be recorded after supersession, but never published onto C.
pub async fn bind_stopped(
    conn: &mut Conn,
    intent: &Intent,
    command: Id,
    supplied: &Json,
) -> Result<Option<orch_link_contract::commands::NodeRuntimeStopResult>, DbError> {
    use orch_link_contract::commands::NodeRuntimeStopResult;
    let phase = native_outcome(conn, intent, command, "node.runtime.stop", supplied).await?;
    let result: NodeRuntimeStopResult =
        serde_json::from_value(supplied.clone()).map_err(|_| invalid())?;
    if !result.coherent() {
        return Err(invalid());
    }
    let mut originals = intent.pending.clone();
    originals.extend(intent.source.incarnation);
    originals.sort_unstable();
    originals.dedup();
    match &result {
        NodeRuntimeStopResult::Stopped { source } => {
            if !source.incarnation.is_some_and(|id| originals.contains(&id)) {
                return Err(invalid());
            }
        }
        NodeRuntimeStopResult::Absent { originals: closed } => {
            let mut closed = closed.clone();
            closed.sort_unstable();
            if closed != originals {
                return Err(invalid());
            }
        }
    }
    freeze_outcome(conn, command, supplied).await?;
    if !is_current(conn, intent).await? || !owns_current_source(conn, intent).await? {
        return Ok(None);
    }
    let latest: i64 =
        sqlx::query_scalar("SELECT native_control_revision FROM runtimes WHERE id=$1 FOR UPDATE")
            .bind(intent.runtime)
            .fetch_one(&mut *conn)
            .await?;
    if latest != phase.revision {
        return Ok(None);
    }
    match &result {
        NodeRuntimeStopResult::Stopped { source } => {
            owns_source(conn, intent, *source, true).await?;
            publish_source(conn, intent, *source).await?;
            sqlx::query("UPDATE runtime_incarnations SET closed=true WHERE runtime_id=$1 AND host_id=$2 AND id=ANY($3)")
                .bind(intent.runtime).bind(intent.host).bind(&originals).execute(conn).await?;
        }
        NodeRuntimeStopResult::Absent { .. } => {
            // Closing an admitted candidate is not evidence that it physically existed.
            sqlx::query("UPDATE runtime_incarnations SET closed=true WHERE runtime_id=$1 AND host_id=$2 AND id=ANY($3)")
                .bind(intent.runtime).bind(intent.host).bind(&originals).execute(&mut *conn).await?;
            sqlx::query("UPDATE runtimes SET loss_missing=true WHERE id=$1")
                .bind(intent.runtime)
                .execute(conn)
                .await?;
        }
    }
    Ok(Some(result))
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
    original_pending: Vec<Id>,
    revision: i64,
    operands: Json,
    current_intent: Option<Id>,
    current_revision: i64,
    host: Id,
    machine: Id,
    original_host: Id,
    original_machine: Id,
}

/// The dispatcher's factual runtime fence precedes its child-job and command rows.
/// A later Stop can then withdraw that child without a reversed runtime/job lock order.
/// Only an already admitted physical phase supplies this address; it grants nothing.
pub async fn lock_command_subject(conn: &mut Conn, command: Id) -> Result<(), DbError> {
    if !super::transaction_guard::in_transaction(conn) {
        return Err(invalid());
    }
    let _:Option<Id>=sqlx::query_scalar(
        "SELECT r.id FROM runtime_control_phases p JOIN runtime_control_intents i ON i.id=p.intent_id
         JOIN runtimes r ON r.id=i.runtime_id WHERE p.id=$1
           AND p.operands->>'kind' IN ('node.runtime.create','node.runtime.start','node.runtime.stop','node.runtime.remove')
         FOR UPDATE OF r")
        .bind(command).fetch_optional(conn).await?;
    Ok(())
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
    if !super::transaction_guard::in_transaction(conn) {
        return Err(invalid());
    }
    let action = match command.kind {
        "node.runtime.create" => LifecycleAction::Create,
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
        "SELECT i.id AS intent,i.runtime_id AS runtime,i.source_incarnation AS original_source,i.pending_incarnations AS original_pending,
         p.revision,p.operands,r.native_control_intent AS current_intent,
         r.native_control_revision AS current_revision,r.host_id AS host,h.machine_id AS machine,
         i.host_id AS original_host,i.machine_id AS original_machine
         FROM runtime_control_phases p JOIN runtime_control_intents i ON i.id=p.intent_id
         JOIN runtimes r ON r.id=i.runtime_id JOIN hosts h ON h.id=r.host_id WHERE p.id=$1
           AND (p.operands->>'kind'<>'node.runtime.stop' OR r.native_stop IS NULL
             OR r.native_stop->>'id' IS DISTINCT FROM p.id::text OR r.native_stop->'ready'='true'::jsonb)
         FOR UPDATE OF r",
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
        || target.pending
            != if source.incarnation == facts.original_source {
                facts.original_pending.clone()
            } else {
                Vec::new()
            }
        || target.opening != opening
        || facts.operands.get("kind").and_then(Json::as_str) != Some(command.kind)
        || facts.operands.get("body") != Some(command.body)
    {
        return Err(invalid());
    }
    if source.incarnation != facts.original_source {
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_control_phases WHERE intent_id=$1
             AND ((operands->>'kind'='node.runtime.start' AND operands->>'opening'=$2
                   AND (outcome IS NULL OR outcome->'source'->>'incarnation'=$2))
               OR (operands->>'kind'='node.runtime.create' AND outcome->'source'->>'incarnation'=$2)))",
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
