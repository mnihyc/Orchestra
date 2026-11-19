//! Planned updates and recreations of provider runtimes, individually or in bounded batches.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::environments::{jobs as shown_jobs, updates as plans};
use orch_api_contract::{
    environment::{UpdatePreflightBody, UpdatePreflightReason},
    forks::{ForkCreateBody, ProviderUpdatePlan},
    resources as api,
    workspace::Job,
};
use orch_authz::{CapturedOperation, Principal};
use orch_db::{Conn, DbError};
use orch_engine::updates::{self, Target, UpdateError};
use orch_types::{Id, Time, new_id};

mod batch_operation;

/// Binds provider-runtime update plans and their confirmations.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::ProviderRuntimeUpdatePreflight, preflight)
        .on(api::ProviderRuntimeUpdateCreate, create)
        .on_with_operation(
            api::ProviderRuntimeUpdateBatchPreflight,
            batch_operation::preview,
            batch_preflight,
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
) -> ApiResult<Job> {
    let mut conn = ctx.app.db.acquire().await?;
    let recovery = orch_db::queries::jobs::forks::preflight(&mut conn, body.preflight_id)
        .await?
        .is_some_and(|row| row.plan.get("provider_recovery").is_some());
    drop(conn);
    if recovery {
        return super::provider_recovery::create(&ctx, body).await;
    }
    let notify = query.notify.or(body.notify).unwrap_or(true);
    let target = Target::Provider(ctx.id("id")?);
    plans::confirm_notifying(ctx, target, body, notify).await
}

async fn reference(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<Option<api::ProviderRef>> {
    if !orch_authz::resources::check(
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
    .is_allowed()
    {
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
    let principal = guard.into_principal();
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
    let mut jobs = Vec::new();
    let mut receipts = Vec::new();
    if prepared.full {
        ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Full);
    }
    let mut full = prepared.full && !ctx.key_receipt_required();
    for draft in &prepared.drafts {
        let provider_id = draft.recipe.target.id();
        let provider = if full {
            reference(&mut tx, &principal, provider_id).await?
        } else {
            None
        };
        full &= provider.is_some();
        let row =
            orch_engine::jobs::recreate::queue(&mut tx, draft.clone(), Some(batch_id), ctx.now)
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
        receipts.push(api::ProviderBatchJobReceipt {
            provider_id,
            job_id: row.id,
        });
        if let Some(provider) = provider {
            jobs.push(api::ProviderBatchJob {
                provider,
                job: shown_jobs::view(&mut tx, &row).await?,
            });
        }
    }
    if query.notify.unwrap_or(true) {
        plans::tell_update(&ctx, &mut tx, &affected, batch_id).await?;
    }
    orch_db::queries::jobs::forks::forget_preflight(&mut tx, body.preflight_id).await?;
    let reply = if full {
        api::ProviderBatchResponse::full(api::ProviderBatch { batch_id, jobs })
    } else {
        api::ProviderBatchResponse::Receipt(api::ProviderBatchReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            batch_id,
            jobs: receipts,
        })
    };
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(reply)
}
