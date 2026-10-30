//! A session that a person began in a harness by hand, taken in as a thread of a topic: found by asking the harness, put in the folder of theirs that holds the directory it ran in, and checked as any thread is for what the caller may use.
//!
//! The engine does the work ([`orch_engine::adoption`]); this module reads the request, and turns
//! what went wrong into the problem a client acts on: a session the harness does not hold is not
//! there, one that a thread took in already is a conflict that names the thread, and a person with no
//! environment on that machine whose folder holds the session's directory is asked to register
//! the folder first. Its preview answers what adopting would do (where the thread would stand, or the
//! folder to register first; the model it would run on; how much of the session's history comes)
//! before anything does, and writes nothing.

use super::{create_failed, one};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::{authorization, transaction_guard};
use crate::registry::{Ctx, Registry};
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{AdoptPlan, ThreadAdopt, ThreadAdoptBody, ThreadAdoptPreview};
use orch_api_contract::threads::{
    AdoptHistoryStatus, Thread, ThreadAdoptHistoryGet, ThreadAdoptRetryHistory,
};
use orch_db::Conn;
use orch_engine::adoption::{AdoptError, Adoption};
use orch_types::Id;

/// The longest reference to a session that is taken as one: what a harness names its sessions with
/// is a UUID or a path, and nothing longer.
const REFERENCE_MAX_CHARS: usize = 512;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on(ThreadAdopt, adopt);
    registry.on(ThreadAdoptPreview, preview);
    registry.on(ThreadAdoptRetryHistory, retry_history);
    registry.on(ThreadAdoptHistoryGet, history_status);
}

/// Why a session was not taken in, as the problem a client acts on.
async fn failed(conn: &mut Conn, error: AdoptError) -> ApiError {
    match error {
        AdoptError::Create(error) => create_failed(conn, error).await,
        AdoptError::Gone | AdoptError::NoSession => ApiError::not_found(),
        AdoptError::AlreadyAdopted { thread } => ApiError::new(
            ErrorCode::ConflictExists,
            "a thread has taken this session in already",
        )
        .with("existing_id", thread),
        AdoptError::NoEnvironment { directory } => ApiError::new(
            ErrorCode::EnvironmentNoneUsable,
            match &directory {
                Some(directory) => format!(
                    "none of your environments on that machine holds {directory}; register the directory first"
                ),
                None => "the session does not say where it ran, so name the environment".to_owned(),
            },
        ),
        AdoptError::Outside { root, directory } => ApiError::invalid(
            "environment_id",
            &format!("the session ran in {directory}, which is not inside the folder {root}"),
        ),
        AdoptError::Unsupported => ApiError::new(
            ErrorCode::CapabilityUnsupported,
            "this harness cannot list the sessions it holds",
        ),
        AdoptError::NoMachine => ApiError::new(
            ErrorCode::ProviderUnavailable,
            "the provider has no machine to run on",
        ),
        AdoptError::Offline => ApiError::new(
            ErrorCode::SourceOffline,
            "the machine the harness runs on is offline",
        ),
        AdoptError::Failed(why) => ApiError::new(
            ErrorCode::ProviderUnavailable,
            format!("the harness could not list its sessions: {why}"),
        ),
        AdoptError::Internal(error) => ApiError::engine(error),
    }
}

/// What the request asks to take in, read and checked.
fn adoption_of(ctx: &Ctx, body: ThreadAdoptBody) -> ApiResult<Adoption> {
    let topic = ctx.id("id")?;
    let reference = body.native_session_ref.trim();
    if reference.is_empty() || reference.chars().count() > REFERENCE_MAX_CHARS {
        return Err(ApiError::invalid(
            "native_session_ref",
            &format!(
                "is the harness's name for the session, up to {REFERENCE_MAX_CHARS} characters"
            ),
        ));
    }
    Ok(Adoption {
        topic,
        provider: body.provider_id,
        agent: body.agent_id,
        native_session_ref: reference.to_owned(),
        environment: body.environment_id,
        name: body.name,
    })
}

/// The environment named in an adoption is checked before its folder or details are read.
async fn require_environment(ctx: &Ctx, environment: Id) -> ApiResult<()> {
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    authorization::require_resource(
        &mut tx,
        guard.principal(),
        ResourceKind::Environment,
        environment,
        Permission::EnvironmentUse,
        orch_authz::Policy::default(),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// A session can name its existing thread only to someone who may read that thread.
async fn visible_thread(ctx: &Ctx, thread: Id) -> ApiResult<bool> {
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let visible = crate::routes::nodes::view::refs(
        &mut tx,
        guard.principal(),
        &[thread],
        orch_authz::Policy::default(),
    )
    .await?
    .get(&thread)
    .is_some_and(|row| !row.redacted);
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(visible)
}

/// Says what taking a session in would do, before anything does.
async fn preview(ctx: Ctx, (): (), body: ThreadAdoptBody) -> ApiResult<AdoptPlan> {
    let adoption = adoption_of(&ctx, body)?;
    let selected = adoption.environment;
    if let Some(environment) = selected {
        require_environment(&ctx, environment).await?;
    }
    let caller = authorization::caller_of(ctx.user()?)?;
    let planned = ctx.app.engine.adoption_plan(&caller, adoption).await;
    if let Some(environment) = selected {
        require_environment(&ctx, environment).await?;
    }
    match planned {
        Ok(mut plan) => {
            if let Some(environment) = plan.environment.as_ref().map(|row| row.id)
                && Some(environment) != selected
            {
                require_environment(&ctx, environment).await?;
            }
            if let Some(thread) = plan.adopted_thread_id
                && !visible_thread(&ctx, thread).await?
            {
                plan.adopted_thread_id = None;
                plan.session.adopted_thread_id = None;
            }
            Ok(plan)
        }
        Err(error) => {
            let mut conn = ctx.app.db.acquire().await?;
            Err(failed(&mut conn, error).await)
        }
    }
}

/// Takes a session in as a thread of the topic.
async fn adopt(ctx: Ctx, (): (), body: ThreadAdoptBody) -> ApiResult<Thread> {
    let adoption = adoption_of(&ctx, body)?;
    let selected = adoption.environment;
    if let Some(environment) = selected {
        require_environment(&ctx, environment).await?;
    }
    let caller = authorization::caller_of(ctx.user()?)?;
    let adopted = ctx.app.engine.adopt_thread(&caller, adoption).await;
    if adopted.is_err()
        && let Some(environment) = selected
    {
        require_environment(&ctx, environment).await?;
    }
    let mut conn = ctx.app.db.acquire().await?;
    match adopted {
        Ok(created) => one(&ctx, &mut conn, created.thread).await,
        Err(AdoptError::AlreadyAdopted { thread }) if !visible_thread(&ctx, thread).await? => {
            Err(ApiError::new(
                ErrorCode::ConflictExists,
                "a thread has taken this session in already",
            ))
        }
        Err(error) => Err(failed(&mut conn, error).await),
    }
}

async fn retry_history(ctx: Ctx, (): (), (): ()) -> ApiResult<Thread> {
    let thread = ctx.id("id")?;
    let caller = authorization::caller_of(ctx.user()?)?;
    let imported = ctx.app.engine.retry_adopted_history(&caller, thread).await;
    let mut conn = ctx.app.db.acquire().await?;
    match imported {
        Ok(()) => one(&ctx, &mut conn, thread).await,
        Err(error) => Err(failed(&mut conn, error).await),
    }
}

async fn history_status(ctx: Ctx, (): (), (): ()) -> ApiResult<Option<AdoptHistoryStatus>> {
    let thread = ctx.id("id")?;
    let mut status = ctx
        .app
        .engine
        .adoption_history_status(thread)
        .await
        .map_err(ApiError::internal)?;
    if let Some(status) = &mut status {
        let mut conn = ctx.app.db.acquire().await?;
        let principal = authorization::principal(&ctx, &mut conn).await?;
        let allowed = orch_authz::nodes::check(
            &mut conn,
            &principal,
            thread,
            orch_access_contract::permissions::Permission::ThreadManage,
            orch_authz::Act::Use,
            orch_authz::Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?;
        status.can_read_again &= matches!(
            allowed,
            orch_access_contract::decisions::Decision::Allowed { .. }
        );
    }
    Ok(status)
}
