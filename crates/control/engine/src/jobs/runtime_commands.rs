//! Original provisioning operands and canonical native outcomes, never actor authority.

use super::runner::{Failure, Stop};
use crate::{Engine, EngineError, commands::Outgoing, ids};
use orch_db::{
    Conn,
    queries::{
        environments::hosting as envs,
        jobs,
        resources::provider_runtime as providers,
        runtime_controls::{self as controls, Intent},
    },
};
use orch_link_contract::{
    commands::{CommandKind, SerialKey},
    native_target::{NativeTarget, RuntimeSource},
};
use orch_types::{Id, Json};

fn closed() -> Stop {
    Failure::new(
        "unavailable",
        "the original provisioning unit, native source or control changed; its data was kept",
    )
    .into()
}

fn required(row: &jobs::JobRow) -> bool {
    matches!(
        row.kind.as_str(),
        jobs::kind::ENVIRONMENT_PROVISION | jobs::kind::PROVIDER_PROVISION
    ) && row
        .plan
        .get("mode")
        .and_then(Json::as_str)
        .is_none_or(|mode| matches!(mode, "make" | "start" | "renew"))
}

/// Capture the job's original runtime in its actual parent-locked admission transaction.
pub(crate) async fn capture(
    conn: &mut Conn,
    job: Id,
    runtime: Id,
) -> Result<Intent, orch_db::DbError> {
    let id = ids::derived(
        "runtime.provision.control",
        &[job.as_bytes(), runtime.as_bytes()],
    );
    controls::accept_job(conn, runtime, id, job).await
}

/// Every migrated reader uses the saved receiver and source; none accepts a later one.
pub(crate) async fn placement(
    conn: &mut Conn,
    job: Id,
    runtime: Id,
    host: Id,
    receiver: Id,
    creating: bool,
) -> Result<Option<Intent>, Stop> {
    let row = jobs::job(conn, job).await?.ok_or(Stop::Lost)?;
    let original = controls::for_job(conn, job, runtime).await?;
    let Some(original) = original else {
        return if required(&row) {
            Err(closed())
        } else {
            Ok(None)
        };
    };
    if original.host() != host
        || original.machine() != receiver
        || (!controls::owns_current_source(conn, &original).await?
            && !(creating && controls::awaiting_creation(conn, &original).await?))
    {
        return Err(closed());
    }
    Ok(Some(original))
}

async fn lock_parent(conn: &mut Conn, runtime: Id) -> Result<(), Stop> {
    let unit = envs::runtime(conn, runtime).await?.ok_or_else(closed)?;
    if let Some(environment) = unit.environment_id {
        envs::lock_environment(conn, environment)
            .await?
            .ok_or_else(closed)?;
    } else if let Some(provider) = unit.provider_id {
        providers::lock(conn, provider).await?.ok_or_else(closed)?;
    } else {
        return Err(closed());
    }
    Ok(())
}

pub(crate) async fn prepare_create(
    engine: &Engine,
    job: Id,
    runtime: Id,
    outgoing: &Outgoing,
) -> Result<Option<Intent>, Stop> {
    let mut tx = engine.db().begin().await.map_err(EngineError::from)?;
    lock_parent(&mut tx, runtime).await?;
    let unit = envs::runtime(&mut tx, runtime).await?.ok_or_else(closed)?;
    let host = orch_db::queries::resources::hosting::host(&mut tx, unit.host_id)
        .await?
        .ok_or_else(closed)?;
    let original = placement(&mut tx, job, runtime, unit.host_id, host.machine_id, true).await?;
    if let Some(original) = &original {
        if outgoing.kind != CommandKind::NodeRuntimeCreate
            || outgoing.machine != original.machine()
            || outgoing.key != SerialKey::Runtime(runtime)
        {
            return Err(closed());
        }
        controls::phase(
            &mut tx,
            original,
            outgoing.id,
            &serde_json::json!({
                "kind":"node.runtime.create","source":original.source(),"body":outgoing.body,
            }),
        )
        .await?
        .ok_or_else(closed)?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(original)
}

pub(crate) async fn created(
    engine: &Engine,
    original: &Intent,
    command: Id,
    result: &Json,
) -> Result<RuntimeSource, Stop> {
    let mut tx = engine.db().begin().await.map_err(EngineError::from)?;
    lock_parent(&mut tx, original.runtime()).await?;
    let source = controls::bind_created(&mut tx, original, command, result)
        .await?
        .ok_or_else(closed)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(source)
}

/// Preparation may wait on network policy or the pool. Recheck the admitted target
/// under its real parent before publishing the requested starting state.
pub(crate) async fn starting(
    engine: &Engine,
    job: Id,
    runtime: Id,
    host: Id,
    receiver: Id,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<Intent>, Stop> {
    let mut tx = engine.db().begin().await.map_err(EngineError::from)?;
    lock_parent(&mut tx, runtime).await?;
    let original = placement(&mut tx, job, runtime, host, receiver, false).await?;
    envs::set_runtime_state(
        &mut tx,
        runtime,
        &[envs::runtime_state::STOPPED, envs::runtime_state::FAILED],
        envs::runtime_state::CREATING,
        None,
        now,
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(original)
}

pub(crate) async fn prepare_start(
    engine: &Engine,
    original: &Intent,
    source: RuntimeSource,
    mut outgoing: Outgoing,
) -> Result<Outgoing, Stop> {
    let mut tx = engine.db().begin().await.map_err(EngineError::from)?;
    lock_parent(&mut tx, original.runtime()).await?;
    if outgoing.kind != CommandKind::NodeRuntimeStart
        || outgoing.machine != original.machine()
        || outgoing.key != SerialKey::Runtime(original.runtime())
        || !controls::owns_current_source(&mut tx, original).await?
    {
        return Err(closed());
    }
    if source.incarnation.is_some() {
        let target = controls::start_phase(&mut tx, original, outgoing.id, source, &outgoing.body)
            .await?
            .ok_or_else(closed)?;
        outgoing = outgoing.with_native_target(NativeTarget::Lifecycle(target));
    }
    // An untracked native result stays untracked; this branch has not supplied an
    // absence target or claimed the physical-original migration complete.
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(outgoing)
}

pub(crate) async fn started(
    engine: &Engine,
    original: &Intent,
    source: RuntimeSource,
    command: Id,
    result: &Json,
) -> Result<(), Stop> {
    if source.incarnation.is_none() {
        return Ok(());
    }
    let mut tx = engine.db().begin().await.map_err(EngineError::from)?;
    lock_parent(&mut tx, original.runtime()).await?;
    controls::bind_started(&mut tx, original, command, result)
        .await?
        .ok_or_else(closed)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Changed or missing owned facts keep data and cannot enter another unit's cleanup.
pub(crate) async fn undo_current(engine: &Engine, job: Id, runtime: Id) -> Result<bool, Stop> {
    let mut tx = engine.db().begin().await.map_err(EngineError::from)?;
    lock_parent(&mut tx, runtime).await?;
    let row = jobs::job(&mut tx, job).await?.ok_or(Stop::Lost)?;
    let current = match controls::for_job(&mut tx, job, runtime).await? {
        Some(original) => controls::owns_current_source(&mut tx, &original).await?,
        None => !required(&row),
    };
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(current)
}
