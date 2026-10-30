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
use orch_authz::Principal;
use orch_db::{Conn, DbError};
use orch_engine::updates::{self, Target, UpdateError};
use orch_settings::Place;
use orch_types::{Id, Time, new_id};

/// Binds provider-runtime update plans and their confirmations.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::ProviderRuntimeUpdatePreflight, preflight)
        .on(api::ProviderRuntimeUpdateCreate, create)
        .on(api::ProviderRuntimeUpdateBatchPreflight, batch_preflight)
        .on(api::ProviderRuntimeUpdateBatchCreate, batch_create);
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
    body: api::ProviderRuntimeUpdateBatchPreflightBody,
) -> ApiResult<api::ProviderRuntimeUpdateBatchPreflightReply> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.into_principal();
    let mut ids = body.provider_ids;
    ids.sort_unstable();
    ids.dedup();
    let id = new_id();
    let mut recipes = Vec::new();
    let mut plans_shown = Vec::new();
    let mut skipped = Vec::new();
    for provider in ids {
        let Some(reference) = reference(&mut tx, &principal, provider).await? else {
            continue;
        };
        let draft = match ctx
            .app
            .engine
            .draft_update(
                &mut tx,
                &principal,
                Target::Provider(provider),
                UpdatePreflightReason::Update,
                None,
            )
            .await
        {
            Ok(draft) => draft,
            Err(UpdateError::Denied) => {
                skipped.push(api::ProviderUpdateSkipped {
                    provider: reference,
                    reason: api::ProviderUpdateSkipReason::NoPermission,
                });
                continue;
            }
            Err(UpdateError::Blocked(_)) | Err(UpdateError::NotFound) => {
                skipped.push(api::ProviderUpdateSkipped {
                    provider: reference,
                    reason: api::ProviderUpdateSkipReason::NotManaged,
                });
                continue;
            }
            Err(error) => return Err(plans::refused(error)),
        };
        if draft.updates.is_empty() {
            skipped.push(api::ProviderUpdateSkipped {
                provider: reference,
                reason: api::ProviderUpdateSkipReason::UpToDate,
            });
            continue;
        }
        plans_shown.push(plans::provider_plan(&ctx, &mut tx, &principal, id, &draft).await?);
        if draft.blockers.is_empty() {
            recipes.push(draft.recipe);
        }
    }
    let options = updates::options(&mut tx, Place::System, None)
        .await
        .map_err(plans::refused)?;
    let parallel = orch_settings::get::<i64>(
        &mut tx,
        Place::System,
        orch_settings_contract::registry::keys::SWITCH_BATCH_PARALLEL,
    )
    .await
    .map_err(ApiError::internal)?
    .unwrap_or(updates::DEFAULT_BATCH_PARALLEL);
    let estimate = orch_engine::jobs::estimates::together(
        &plans_shown
            .iter()
            .map(|p| p.estimate.clone())
            .collect::<Vec<_>>(),
        parallel,
    );
    plans::keep_batch(&ctx, &mut tx, id, None, &recipes, true).await?;
    let executable = recipes.len() as i64;
    let affected: Vec<_> = recipes
        .iter()
        .flat_map(|recipe| recipe.threads.iter().copied())
        .collect();
    let notify = Some(plans::audience(&ctx, &mut tx, &principal, &affected).await?);
    tx.commit().await.map_err(DbError::from)?;
    Ok(api::ProviderRuntimeUpdateBatchPreflightReply {
        notify,
        preflight_id: id,
        expires_at: Time::new(updates::expires(ctx.now)),
        plans: plans_shown,
        skipped,
        executable,
        options,
        estimate,
    })
}

async fn batch_create(
    mut ctx: Ctx,
    query: orch_api_contract::forks::UpdateConfirmQuery,
    body: ForkCreateBody,
) -> ApiResult<api::ProviderBatch> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.into_principal();
    let drafts = plans::batch_drafts(&ctx, &mut tx, &principal, None, true, &body).await?;
    let batch_id = new_id();
    let affected: Vec<_> = drafts
        .iter()
        .flat_map(|draft| draft.recipe.threads.iter().copied())
        .collect();
    let mut jobs = Vec::new();
    for draft in drafts {
        let provider = super::view::provider_refs(&mut tx, &[draft.recipe.target.id()], &[])
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?
            .1;
        let row = orch_engine::jobs::recreate::queue(&mut tx, draft, Some(batch_id), ctx.now)
            .await
            .map_err(plans::refused)?;
        jobs.push(api::ProviderBatchJob {
            provider,
            job: shown_jobs::view(&mut tx, &row).await?,
        });
    }
    if query.notify.unwrap_or(true) {
        plans::tell_update(&ctx, &mut tx, &affected, batch_id).await?;
    }
    orch_db::queries::jobs::forks::forget_preflight(&mut tx, body.preflight_id).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(api::ProviderBatch { batch_id, jobs })
}
