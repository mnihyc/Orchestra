//! Planned updates and recreations of provider runtimes, individually or in bounded batches.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::environments::jobs as shown_jobs;

use crate::routes::environments::updates as plans;

use orch_api_contract::environment::UpdatePreflightBody;

use orch_api_contract::environment::UpdatePreflightReason;

use orch_api_contract::forks::ForkCreateBody;

use orch_api_contract::forks::ProviderUpdatePlan;

use orch_api_contract::resources as api;

use orch_authz::CapturedOperation;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_engine::updates;

use orch_engine::updates::Target;

use orch_engine::updates::UpdateError;

use orch_types::Id;

use orch_types::Time;

use orch_types::new_id;
/// Binds provider-runtime update plans and their confirmations.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::ProviderRuntimeUpdatePreflight, preflight)
        .on(api::ProviderRuntimeUpdateCreate, create)
        .creation(
            api::ProviderRuntimeUpdateCreate,
            shown_jobs::creation::binding::<shown_jobs::creation::ProviderUpdate>(),
        )
        .on_with_operation(
            api::ProviderRuntimeUpdateBatchPreflight,
            batch_operation::preview,
            batch_preflight,
        )
        .creation(
            api::ProviderRuntimeUpdateBatchCreate,
            batch_reply::binding(),
        )
        .on_with_operation(
            api::ProviderRuntimeUpdateBatchCreate,
            batch_operation::confirm,
            batch_create,
        );
}

async fn preflight(
    mut ctx: Ctx,
    (): (),
    body: UpdatePreflightBody,
) -> ApiResult<ProviderUpdatePlan> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.into_principal();
    if super::provider_recovery::needed(&mut tx, ctx.id("id")?).await? {
        tx.rollback().await.map_err(DbError::from)?;
        return super::provider_recovery::preflight(&ctx, &principal, ctx.id("id")?).await;
    }
    let draft = ctx
        .app
        .engine
        .draft_provider_update_with_copy(
            &mut tx,
            &principal,
            ctx.id("id")?,
            body.reason.unwrap_or(UpdatePreflightReason::Update),
            body.copy_directories.unwrap_or(false),
        )
        .await
        .map_err(plans::refused)?;
    let id = new_id();
    let plan = plans::provider_plan(&ctx, &mut tx, &principal, id, &draft).await?;
    plans::keep(&ctx, &mut tx, &draft, id, "provider_update", None).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(plan)
}

async fn create(
    ctx: Ctx,
    query: orch_api_contract::forks::UpdateConfirmQuery,
    body: ForkCreateBody,
) -> ApiResult<orch_api_contract::workspace::JobCreationResponse> {
    let mut conn = ctx.app.db.acquire().await?;
    let recovery = orch_db::queries::jobs::forks::preflight(&mut conn, body.preflight_id)
        .await?
        .is_some_and(|row| row.plan.get("provider_recovery").is_some());
    drop(conn);
    let job = if recovery {
        super::provider_recovery::create(&ctx, body).await?
    } else {
        let notify = query.notify.or(body.notify).unwrap_or(true);
        let target = Target::Provider(ctx.id("id")?);
        plans::confirm_notifying(ctx.clone(), target, body, notify).await?
    };
    shown_jobs::creation::respond::<shown_jobs::creation::ProviderUpdate>(&ctx, job.id, Some(job))
        .await
}

async fn reference_access(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<bool> {
    Ok(orch_authz::resources::check(
        conn,
        principal,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ResourceKind::Provider.into(),
            id,
        },
        orch_access_contract::permissions::Permission::ProviderView,
        orch_authz::Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .is_allowed())
}

async fn reference(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<Option<api::ProviderRef>> {
    if !reference_access(conn, principal, id).await? {
        return Ok(None);
    }
    Ok(super::view::provider_refs(conn, &[id], &[])
        .await?
        .pop()
        .map(|(_, p)| p))
}

async fn batch_preflight(
    mut ctx: Ctx,
    (): (),
    _body: api::ProviderRuntimeUpdateBatchPreflightBody,
) -> ApiResult<api::ProviderRuntimeUpdateBatchPreflightResponse> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let prepared = guard.operation_data::<batch_operation::Preview>()?;
    let principal = guard.into_principal();
    let origin = CapturedOperation::capture(principal.caller()).map_err(ApiError::internal)?;
    let recipes: Vec<_> = prepared
        .drafts
        .iter()
        .filter(|draft| draft.blockers.is_empty())
        .map(|draft| draft.recipe.clone())
        .collect();
    plans::keep_provider_batch(&ctx, &mut tx, prepared.id, &recipes, &origin).await?;
    if prepared.full.is_some() {
        // The fresh guarded producer completed every actual response source. Denied
        // remains terminal; only this proven Pending projection can become Full.
        ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Full);
    }
    let reply = match &prepared.full {
        Some(full) if !ctx.key_receipt_required() => {
            api::ProviderRuntimeUpdateBatchPreflightResponse::full(full.clone())
        }
        _ => api::ProviderRuntimeUpdateBatchPreflightResponse::Receipt(
            api::ProviderRuntimeUpdateBatchPreflightReceipt {
                projection: orch_api_contract::common::ReceiptProjection::Receipt,
                preflight_id: prepared.id,
                expires_at: Time::new(updates::expires(ctx.now)),
                executable: recipes.len() as i64,
                provider_ids: recipes.iter().map(|recipe| recipe.target.id()).collect(),
                blocked: prepared
                    .drafts
                    .iter()
                    .filter(|draft| !draft.blockers.is_empty())
                    .map(|draft| draft.recipe.target.id())
                    .collect(),
                skipped: prepared.skipped.clone(),
            },
        ),
    };
    tx.commit().await.map_err(DbError::from)?;
    Ok(reply)
}

async fn batch_create(
    mut ctx: Ctx,
    query: orch_api_contract::forks::UpdateConfirmQuery,
    body: ForkCreateBody,
) -> ApiResult<api::ProviderBatchResponse> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let prepared = guard.operation_data::<batch_operation::Confirmation>()?;
    let principal = guard.principal().clone();
    let mut origins = prepared.origins.clone();
    let origin = CapturedOperation::capture(principal.caller()).map_err(ApiError::internal)?;
    if !origins.contains(&origin) {
        origins.push(origin);
    }
    let batch_id = new_id();
    let affected: Vec<_> = prepared
        .drafts
        .iter()
        .flat_map(|draft| draft.recipe.threads.iter().copied())
        .collect();
    let mut queued = Vec::new();
    for draft in &prepared.drafts {
        let row = orch_engine::jobs::recreate::queue_before(
            &mut tx,
            draft.clone(),
            Some(batch_id),
            ctx.now,
            || guard.before_effect(&ctx),
        )
        .await
        .map_err(plans::refused)?;
        orch_engine::jobs::recreate::batch_authority::retain(
            &mut tx,
            row.id,
            batch_id,
            &origins,
            Time::new(ctx.now),
        )
        .await
        .map_err(ApiError::engine)?;
        queued.push(row);
    }
    if query.notify.unwrap_or(true) {
        plans::tell_update(&ctx, &mut tx, &affected, batch_id).await?;
    }
    orch_db::queries::jobs::forks::forget_preflight(&mut tx, body.preflight_id).await?;
    batch_reply::observed(&mut tx, principal.user(), batch_id, &queued).await?;
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    batch_reply::first(&ctx, batch_id, queued).await
}

#[path = "provider_updates/batch_operation.rs"]
pub mod batch_operation;

#[path = "provider_updates/batch_reply.rs"]
pub mod batch_reply;
