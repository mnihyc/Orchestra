//! Process control and resource samples of an environment.
//!
//! Every process of an environment is listed to whoever may see its processes, whichever thread
//! started it, each with the thread it belongs to when that is known and the caller's own thread's
//! first: a list kept to one thread's processes would suggest a separation that is not there. A
//! signal names the process by its number and by the moment it started, so that a number that was
//! reused is never signalled. How much the environment as a whole used lately is a series of
//! measurements that the API keeps in memory for six hours; they are not stored.

use super::workspace::door;
use super::workspace::live;
use super::workspace::problems::Doing;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::sign_in::origin;
use chrono::Duration;
use orch_access_contract::audit::AuditAction;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::ThroughThreadQuery;
use orch_api_contract::processes::{
    EnvironmentProcessList, EnvironmentProcessListReply, EnvironmentProcessSignal,
    EnvironmentProcessSignalBody, EnvironmentResourceList, EnvironmentResourceListQuery,
    ProcessSignal, RESOURCE_MINUTES_DEFAULT, RESOURCE_MINUTES_MAX, ResourceSample,
};
use orch_db::audit::{self, Actor, Entry};
use orch_link_contract::commands::{ProcList, ProcSignal, SerialKey, Signal};
use orch_settings::Place;
use orch_types::Time;

/// The key of the setting that says whether shells and signals are allowed where a thread is.
const TERMINAL_ENABLED: &str = "terminal.enabled";

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(EnvironmentProcessList, list)
        .on(EnvironmentProcessSignal, signal)
        .on(EnvironmentResourceList, resources);
}

/// Every process of the environment, the thread the caller came in by first.
async fn list(
    ctx: Ctx,
    query: ThroughThreadQuery,
    (): (),
) -> ApiResult<EnvironmentProcessListReply> {
    let mut conn = ctx.app.db.acquire().await?;
    let door = door::open(&ctx, &mut conn, query.thread).await?;
    let machine = live::require_live(&ctx, &mut conn, &door.env).await?;
    drop(conn);
    let found = live::run(
        &ctx,
        machine,
        SerialKey::None,
        &ProcList {
            environment_id: door.env.id,
        },
    )
    .await
    .map_err(|failure| failure.problem(Doing::Processes))?;
    let mut processes = found.processes;
    let own = door.thread;
    processes.sort_by_key(|process| (own.is_none() || process.thread_id != own, process.pid));
    Ok(EnvironmentProcessListReply {
        as_of: found.as_of,
        processes,
    })
}

/// Sends a signal to one process.
async fn signal(
    ctx: Ctx,
    query: ThroughThreadQuery,
    body: EnvironmentProcessSignalBody,
) -> ApiResult<()> {
    let pid: u32 = ctx
        .param("pid")?
        .parse()
        .map_err(|_| ApiError::not_found())?;
    let mut conn = ctx.app.db.acquire().await?;
    let door = door::open(&ctx, &mut conn, query.thread).await?;
    // The environment's own switch for what is done at a shell, and the setting of the place.
    let place = door
        .thread
        .map_or(Place::Environment(door.env.id), Place::Node);
    let enabled = orch_settings::get::<bool>(&mut conn, place, TERMINAL_ENABLED)
        .await
        .ok()
        .flatten()
        .unwrap_or(true);
    if !door.env.terminal || !enabled {
        return Err(ApiError::new(
            ErrorCode::TerminalNotAllowed,
            "processes are not signalled here",
        ));
    }
    let machine = live::require_live(&ctx, &mut conn, &door.env).await?;
    drop(conn);
    live::run(
        &ctx,
        machine,
        SerialKey::Environment(door.env.id),
        &ProcSignal {
            environment_id: door.env.id,
            pid: i64::from(pid),
            started_at: body.started_at,
            signal: match body.signal {
                ProcessSignal::Int => Signal::Int,
                ProcessSignal::Term => Signal::Term,
                ProcessSignal::Kill => Signal::Kill,
            },
        },
    )
    .await
    .map_err(|failure| failure.problem(Doing::Processes))?;
    let mut tx = ctx.app.db.begin().await?;
    let summary = format!(
        "{} sent {:?} to the process {pid}",
        crate::routes::nodes::actor_name(&ctx)?,
        body.signal
    );
    let mut entry = Entry::new(
        Actor::User(ctx.user()?.id()),
        orch_db::audit::action!(AuditAction::ProcessSignalled).as_str(),
        "environment",
        Some(door.env.id),
        &summary,
    );
    entry.node_id = Some(door.env.home_node_id);
    entry.org_id = Some(door.env.org_id);
    entry.environment_id = Some(door.env.id);
    entry.details = serde_json::json!({
        "pid": pid, "signal": body.signal, "started_at": body.started_at, "thread": door.thread
    });
    audit::append(&mut tx, &entry, &origin(&ctx), ctx.now).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// What the environment as a whole used lately, oldest first.
async fn resources(
    ctx: Ctx,
    query: EnvironmentResourceListQuery,
    (): (),
) -> ApiResult<Vec<ResourceSample>> {
    let minutes = match query.minutes {
        None => RESOURCE_MINUTES_DEFAULT,
        Some(0) => return Err(ApiError::invalid("minutes", "must be at least 1")),
        Some(minutes) => minutes.min(RESOURCE_MINUTES_MAX),
    };
    let mut conn = ctx.app.db.acquire().await?;
    let door = door::open(&ctx, &mut conn, query.thread).await?;
    // Reading history starts no background work; the visible view owns its live stats subscription.
    Ok(ctx
        .app
        .cache
        .samples()
        .since(door.env.id, ctx.now - Duration::minutes(i64::from(minutes))))
}

/// The moment of a sample, for tests that read the series.
#[must_use]
pub fn taken_at(sample: &ResourceSample) -> Time {
    sample.at
}
