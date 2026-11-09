//! The existing safe harness-descriptor preparation for a modern creator on an existing
//! Environment. This carries no partial admission into the writer or any deferred work.

use crate::key_operations::{admission::Prepared, blob_sources, targets};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationInitialFuture};
use crate::thread_access::AuthzAccess;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::CreateThread;
use orch_api_contract::operations::TargetResolver;
use orch_authz::{Act, Caller, CapturedCaller, Policy};
use orch_db::{
    Conn,
    queries::{environments::thread_engine as environments, resources::thread_engine as resources},
};
use orch_engine::access::{ProviderUse, ResourceAccess};
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Time};
use std::sync::Arc;

// Only the operands of this one discovery await. Creation source/preset settings are
// resolved again by the normal writer; this value is neither serialized nor handed off.
#[derive(PartialEq, Eq)]
struct Selection {
    agent: Id,
    provider: Id,
    environment: Id,
    generation: i32,
    arrangement: Option<orch_agent_contract::descriptor::ArrangementType>,
    default_model: Option<String>,
}

fn denied() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the chosen binding is not permitted",
    )
}

fn empty(plan: &mut Prepared, role: &'static str) -> ApiResult<()> {
    let index = plan
        .variant()
        .facets
        .iter()
        .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
        .ok_or_else(|| ApiError::internal("a creator preparation collection is not declared"))?;
    plan.bind_empty(index)
}

async fn checked(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: &CreateThread,
    after_discovery: bool,
) -> ApiResult<Option<(Selection, bool)>> {
    let topic = ctx.id("id")?;
    let target = targets::node(conn, topic).await?;
    if target.object.kind != KeyTargetKind::Object(ObjectKind::Topic) {
        return Err(ApiError::not_found());
    }
    let mut plan = Prepared::new(ctx.route, "ThreadCreate")?;
    plan.bind(0, KeyTargetKind::Object(ObjectKind::Thread), &[target])?;
    let principal = plan.preparation_principal(ctx, conn, caller).await?;
    authorization::require(
        conn,
        &principal,
        topic,
        authorization::exact(Permission::TopicStartThreads),
        Act::Use,
        Policy::default(),
    )
    .await?;
    let environment = match super::make::environment_of(
        &ctx.app,
        conn,
        topic,
        caller.user(),
        body.environment.as_ref(),
    )
    .await
    {
        Ok(environment) => environment,
        Err(error)
            if body.environment.is_none() && error.code() == ErrorCode::EnvironmentNoneUsable =>
        {
            return Ok(None);
        } // The ordinary staged writer may choose a fresh Environment.
        Err(error) => return Err(error),
    };
    let default_model = if body.model.is_some() {
        None
    } else {
        setting::<String>(conn, Place::Node(topic), keys::AGENT_MODEL)
            .await
            .map_err(ApiError::internal)?
    };
    let model = body.model.as_deref().or(default_model.as_deref());
    let Some(chosen) = super::make::binding_of(
        &ctx.app,
        conn,
        topic,
        caller.user(),
        body.binding.as_ref(),
        model,
        body.effort.as_deref(),
    )
    .await?
    else {
        return Ok(None);
    };
    let env = environments::environment(conn, environment)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !after_discovery
        && let Some(profile) = resources::agent(conn, chosen.agent).await?
        && orch_engine::harnesses::descriptor_of(&profile)
            .map_err(ApiError::engine)?
            .is_some()
    {
        return Ok(Some((
            Selection {
                agent: chosen.agent,
                provider: chosen.provider,
                environment,
                generation: env.generation,
                arrangement: chosen.arrangement,
                default_model,
            },
            false,
        )));
    }
    let (agent, provider, environment, arrangement) = match super::creation_operation::materialize(
        ctx,
        conn,
        caller,
        topic,
        body,
        Prepared::new(ctx.route, "ThreadCreate")?,
    )
    .await?
    {
        Some(full) => {
            // Reuse all presently materializable source/delegation facets and the same selected
            // operands. A descriptor read does not create a Thread or deliver the first input.
            let admitted = full.admit(caller)?;
            let creation = admitted.data.get::<super::creation_operation::Creation>()?;
            (
                creation.new.agent,
                creation.new.provider,
                creation.new.environment,
                creation.new.arrangement,
            )
        }
        None => {
            let choice = match body.agent_access {
                Some(choice) => Some(choice),
                None => setting::<orch_api_contract::nodes::AgentAccessChoice>(
                    conn,
                    Place::Node(topic),
                    keys::THREADS_AGENT_ACCESS,
                )
                .await
                .map_err(ApiError::internal)?,
            };
            if !choice.is_some_and(|choice| {
                choice.where_ == orch_api_contract::nodes::AgentAccessWhere::Thread
                    && choice.parts != Default::default()
            }) {
                return Ok(None);
            }
            // This-thread delegation needs the inserted Thread's real identity. Only that future
            // facet remains unresolved; current file/blob source reads must already pass here.
            for role in [
                "creator_new_environment_homes",
                "creator_initial_file_threads",
                "creator_initial_file_environments",
            ] {
                empty(&mut plan, role)?;
            }
            let worktree = super::make::worktree_of(body)?;
            let source = orch_engine::jobs::source::resolved(conn, topic)
                .await
                .map_err(ApiError::engine)?;
            let pending = orch_engine::jobs::source::preparation_pending(
                body.prepare_source,
                worktree.is_some(),
                source.repository_url.is_some(),
                env.status == "running",
            );
            orch_engine::operation_scope::creator_files::bind(
                conn,
                plan.shared_mut(),
                environment,
                pending,
                worktree.is_some(),
            )
            .await
            .map_err(ApiError::engine)?;
            let hashes = body
                .first_input
                .as_ref()
                .map(|first| orch_chat::following::content_blobs(&first.content))
                .unwrap_or_default();
            let sources = blob_sources::prepare(ctx, conn, caller, &plan, &hashes, &[]).await?;
            blob_sources::bind(&mut plan, &sources.bindings)?;
            let principal = plan.preparation_principal(ctx, conn, caller).await?;
            if pending {
                authorization::require_resource_projected(
                    conn,
                    &principal,
                    orch_access_contract::permissions::ResourceKind::Environment,
                    environment,
                    Permission::EnvironmentManage,
                    Policy::default(),
                    true,
                )
                .await?;
            }
            if worktree.is_some() {
                authorization::require_resource_projected(
                    conn,
                    &principal,
                    orch_access_contract::permissions::ResourceKind::Environment,
                    environment,
                    Permission::EnvironmentView,
                    Policy::default(),
                    true,
                )
                .await?;
            }
            (
                chosen.agent,
                chosen.provider,
                environment,
                chosen.arrangement,
            )
        }
    };
    let agent_row = resources::agent(conn, agent)
        .await?
        .filter(|row| row.deleted_at.is_none() && !row.disabled)
        .ok_or_else(|| ApiError::invalid("binding.agent_id", "the agent profile is unavailable"))?;
    let provider_row = resources::provider(conn, provider)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.status != "disabled")
        .ok_or_else(|| ApiError::invalid("binding.provider_id", "the provider is unavailable"))?;
    let env = environments::environment(conn, environment)
        .await?
        .filter(|row| {
            row.deleted_at.is_none()
                && row.remove_at.is_none()
                && !matches!(row.status.as_str(), "lost" | "removed")
        })
        .ok_or_else(|| ApiError::invalid("environment.use", "the environment is unavailable"))?;
    if !orch_db::queries::loss::accepts_threads(conn, environment).await? {
        return Err(denied());
    }
    let access = AuthzAccess::new(Arc::clone(&ctx.app.clock));
    if !access
        .agent_use(conn, caller.user(), topic, agent)
        .await
        .map_err(ApiError::internal)?
        .is_allowed()
        || !access
            .provider_use(
                conn,
                &ProviderUse {
                    person: caller.user(),
                    node: topic,
                    provider,
                    model: body.model.as_deref(),
                    effort: body.effort.as_deref(),
                },
            )
            .await
            .map_err(ApiError::internal)?
            .is_allowed()
        || !access
            .environment_place(conn, caller.user(), environment)
            .await
            .map_err(ApiError::internal)?
            .is_allowed()
    {
        return Err(denied());
    }
    if orch_engine::provider_policy::forbidden(conn, topic, provider)
        .await
        .map_err(ApiError::engine)?
        .is_some()
    {
        return Err(ApiError::new(
            ErrorCode::ProviderNotAllowed,
            "the provider is not allowed here",
        ));
    }
    // Match create_in: explicit model validation uses the request's model. The inherited model
    // above participates only in selecting an implicit binding, never a new stricter use check.
    if let Some(model) = body.model.as_deref() {
        let row = resources::model(conn, provider_row.id, model).await?;
        let settings = orch_engine::resolved::load(conn, topic)
            .await
            .map_err(ApiError::engine)?;
        orch_engine::models::check_allowed(row.as_ref(), settings.models_allowed.as_deref(), model)
            .map_err(|_| ApiError::new(ErrorCode::ModelNotAllowed, "the model is not allowed"))?;
        if let Some(effort) = body.effort.as_deref() {
            let row = row.ok_or_else(|| {
                ApiError::new(ErrorCode::ModelNotAllowed, "the model is not allowed")
            })?;
            let parameters =
                orch_engine::binding::model_parameters(&row).map_err(ApiError::engine)?;
            if !parameters.efforts().iter().any(|listed| listed == effort) {
                return Err(ApiError::invalid(
                    "effort",
                    "the effort is not listed for this model",
                ));
            }
        }
    }
    let cold = orch_engine::harnesses::descriptor_of(&agent_row)
        .map_err(ApiError::engine)?
        .is_none();
    Ok(Some((
        Selection {
            agent,
            provider,
            environment,
            generation: env.generation,
            arrangement,
            default_model,
        },
        cold,
    )))
}

/// The Registry restores the same accepted/current credential before and after this phase.
/// No connection or mutation transaction is held while the existing descriptor RPC is awaited.
pub(super) fn initial<'a>(
    ctx: &'a Ctx,
    caller: &'a Caller,
    (): (),
    body: CreateThread,
) -> OperationInitialFuture<'a> {
    Box::pin(async move {
        if body.program.is_some()
            || !caller
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some())
            || matches!(body.environment.as_ref(), Some(EnvironmentChoice::New(_)))
        {
            return Ok(());
        }
        let first = {
            let mut conn = ctx.app.db.acquire().await?;
            let Some((chosen, cold)) = checked(ctx, &mut conn, caller, &body, false).await? else {
                return Ok(());
            };
            if !cold {
                return Ok(());
            }
            chosen
        };
        let accepted = CapturedCaller::capture(caller);
        ctx.app
            .engine
            .learn_descriptor(first.agent, first.provider, Some(first.environment))
            .await
            .map_err(ApiError::engine)?;
        let mut conn = ctx.app.db.acquire().await?;
        let caller = accepted
            .restore(&mut conn, Time::new(ctx.app.clock.now()))
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::AuthInvalidToken,
                    "the preparation credential is no longer valid",
                )
            })?;
        let current_ctx =
            ctx.for_operation_checkpoint(ctx.user()?.as_ref().clone(), ctx.app.clock.now());
        let current = checked(&current_ctx, &mut conn, &caller, &body, true).await?;
        if current.as_ref().map(|(selection, _)| selection) != Some(&first) {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the thread's selected binding or environment changed during discovery",
            ));
        }
        Ok(())
    })
}
