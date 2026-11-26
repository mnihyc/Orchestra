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

/// Why a job cannot go on with the unit it was admitted for, in words that say what is so. The job may be the first
/// to make the unit, so none of them says that an older one changed; what is undone or kept is the undo's to say.
#[derive(Clone, Copy)]
enum Gone {
    /// The job's own record of the unit it may change is not there.
    Record,
    /// The runtime was removed.
    Runtime,
    /// The environment the runtime belongs to was removed.
    Environment,
    /// The provider the runtime belongs to was removed.
    Provider,
    /// The runtime belongs to neither.
    Owner,
    /// The host it is placed on was removed.
    Host,
    /// It was placed on another host after the job began.
    Moved,
    /// Its host's machine enrolled again after the job began.
    Enrolled,
    /// What the job was to send its host is not what it was admitted for.
    Mismatch,
    /// The unit is no longer the one this job makes or starts, and its host has not reported it down.
    Taken,
}

impl Gone {
    fn words(self) -> &'static str {
        match self {
            Self::Record => "the record of the unit this work may change is gone",
            Self::Runtime => "the runtime was removed while this work went on",
            Self::Environment => "its environment was removed while this work went on",
            Self::Provider => "its provider was removed while this work went on",
            Self::Owner => "the runtime belongs to no environment or provider",
            Self::Host => "its host was removed while this work went on",
            Self::Moved => "the runtime was placed on another host while this work went on",
            Self::Enrolled => "its host's machine was enrolled again while this work went on",
            Self::Mismatch => "what this work was to send its host is not what it was admitted for",
            Self::Taken => "a newer start, stop or removal of the unit took it over",
        }
    }

    fn stop(self) -> Stop {
        Failure::new("unavailable", self.words()).into()
    }
}

/// The unit is no longer the one this job makes or starts. When its host reported it down, that report is why, in
/// the host's own words: a unit that failed as it started leaves its job nothing to go on with.
async fn taken(conn: &mut Conn, runtime: Id) -> Result<Stop, Stop> {
    let reported = envs::runtime(conn, runtime).await?.and_then(|unit| {
        (unit.state == envs::runtime_state::FAILED)
            .then_some(unit.state_detail)
            .flatten()
            .filter(|detail| {
                !detail.trim().is_empty()
                    && ![providers::UNFINISHED, providers::UPDATE_ENDED].contains(&detail.as_str())
                    && !providers::INTENTIONAL_STOP_DETAILS.contains(&detail.as_str())
            })
    });
    Ok(match reported {
        Some(detail) => Failure::new(
            "unavailable",
            format!("its host reports the unit down: {detail}"),
        )
        .into(),
        None => Gone::Taken.stop(),
    })
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
            Err(Gone::Record.stop())
        } else {
            Ok(None)
        };
    };
    if original.host() != host {
        return Err(Gone::Moved.stop());
    }
    if original.machine() != receiver {
        return Err(Gone::Enrolled.stop());
    }
    if !controls::owns_current_source(conn, &original).await?
        && !(creating && controls::awaiting_creation(conn, &original).await?)
    {
        return Err(taken(conn, runtime).await?);
    }
    Ok(Some(original))
}

/// A resource Stop can withdraw its exact admitted provisioning child, not a broad hold.
pub(crate) async fn owned_provision_hold(
    conn: &mut Conn,
    job: &jobs::JobRow,
    environment: Id,
    accepted: Option<&Intent>,
) -> Result<bool, orch_db::DbError> {
    if job.kind != jobs::kind::ENVIRONMENT_PROVISION || job.environment_id != Some(environment) {
        return Ok(false);
    }
    let Some(runtime) = job
        .plan
        .get("runtime")
        .and_then(Json::as_str)
        .and_then(|value| value.parse::<Id>().ok())
    else {
        return Ok(false);
    };
    let Some(original) = controls::for_job(conn, job.id, runtime).await? else {
        return Ok(false);
    };
    if controls::is_current(conn, &original).await? {
        return Ok(true);
    }
    match accepted {
        Some(accepted) if accepted.runtime() == runtime => {
            Ok(controls::previous_active_job(conn, accepted).await? == Some(job.id))
        }
        _ => Ok(false),
    }
}

async fn lock_parent(conn: &mut Conn, runtime: Id) -> Result<(), Stop> {
    let unit = envs::runtime(conn, runtime)
        .await?
        .ok_or_else(|| Gone::Runtime.stop())?;
    if let Some(environment) = unit.environment_id {
        envs::lock_environment(conn, environment)
            .await?
            .ok_or_else(|| Gone::Environment.stop())?;
    } else if let Some(provider) = unit.provider_id {
        providers::lock(conn, provider)
            .await?
            .ok_or_else(|| Gone::Provider.stop())?;
    } else {
        return Err(Gone::Owner.stop());
    }
    Ok(())
}

pub(crate) async fn prepare_create(
    engine: &Engine,
    job: Id,
    runtime: Id,
    mut outgoing: Outgoing,
) -> Result<(Option<Intent>, Outgoing), Stop> {
    let mut tx = engine.db().begin().await.map_err(EngineError::from)?;
    lock_parent(&mut tx, runtime).await?;
    let unit = envs::runtime(&mut tx, runtime)
        .await?
        .ok_or_else(|| Gone::Runtime.stop())?;
    let host = orch_db::queries::resources::hosting::host(&mut tx, unit.host_id)
        .await?
        .ok_or_else(|| Gone::Host.stop())?;
    let original = placement(&mut tx, job, runtime, unit.host_id, host.machine_id, true).await?;
    if let Some(original) = &original {
        if outgoing.kind != CommandKind::NodeRuntimeCreate
            || outgoing.machine != original.machine()
            || outgoing.key != SerialKey::Runtime(runtime)
        {
            return Err(Gone::Mismatch.stop());
        }
        let Some(target) =
            controls::create_phase(&mut tx, original, outgoing.id, &outgoing.body).await?
        else {
            return Err(taken(&mut tx, runtime).await?);
        };
        outgoing = outgoing.with_native_target(NativeTarget::Lifecycle(target));
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok((original, outgoing))
}

pub(crate) async fn created(
    engine: &Engine,
    original: &Intent,
    command: Id,
    result: &Json,
) -> Result<RuntimeSource, Stop> {
    let mut tx = engine.db().begin().await.map_err(EngineError::from)?;
    lock_parent(&mut tx, original.runtime()).await?;
    let source = controls::bind_created(&mut tx, original, command, result).await?;
    let why = match source {
        Some(_) => None,
        None => Some(taken(&mut tx, original.runtime()).await?),
    };
    tx.commit().await.map_err(orch_db::DbError::from)?;
    source.ok_or_else(|| why.unwrap_or_else(|| Gone::Taken.stop()))
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
    {
        return Err(Gone::Mismatch.stop());
    }
    if !controls::owns_current_source(&mut tx, original).await? {
        return Err(taken(&mut tx, original.runtime()).await?);
    }
    if source.incarnation.is_some() {
        let Some(target) =
            controls::start_phase(&mut tx, original, outgoing.id, source, &outgoing.body).await?
        else {
            return Err(taken(&mut tx, original.runtime()).await?);
        };
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
    let result = controls::bind_started(&mut tx, original, command, result).await?;
    let why = match result {
        Some(_) => None,
        None => Some(taken(&mut tx, original.runtime()).await?),
    };
    tx.commit().await.map_err(orch_db::DbError::from)?;
    result
        .map(|_| ())
        .ok_or_else(|| why.unwrap_or_else(|| Gone::Taken.stop()))
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
