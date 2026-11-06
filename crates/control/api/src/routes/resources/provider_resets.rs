//! Explicitly confirmed provider resets, current authority, restricted receipts and audit attribution.

use crate::key_operations::{admission::Prepared, targets};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::{authorization, transaction_guard};
use crate::registry::{Ctx, OperationFuture, Registry};
use crate::routes::resources::providers::show;
use crate::routes::resources::{Home, record};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission, ResourceKind};
use orch_api_contract::common::{ErrorCode, ReceiptProjection};
use orch_api_contract::resources::{
    self as api, Provider, ProviderLastReset, ProviderResetConsumeBody,
    ProviderResetConsumeReceipt, ProviderResetConsumeReply, ProviderResetConsumeResponse,
};
use orch_authz::{Caller, Policy, Principal};
use orch_db::queries::provider_facts as rows;
use orch_db::queries::resources::thread_engine as resources;
use orch_db::{Conn, DbError};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::HashMap;

/// Binds the one confirmed reset action.
pub fn mount(registry: &mut Registry) {
    registry.on_with_operation(api::ProviderResetConsume, operation, consume);
}

fn legacy_refusal() -> ApiError {
    ApiError::new(
        ErrorCode::AuthSessionRequired,
        "this legacy key cannot confirm a provider reset",
    )
}

fn check_legacy(caller: &Caller) -> ApiResult<()> {
    if caller
        .grant()
        .is_some_and(|grant| grant.requires_legacy_compatibility())
    {
        return Err(legacy_refusal());
    }
    Ok(())
}

// Op::returning declares the required effect first and the optional Read second.
const EFFECT_FACET: usize = 0;
const RESPONSE_FACET: usize = 1;

/// The same stored Provider site supplies both effect and optional response Read facts.
async fn response_plan(ctx: &Ctx, conn: &mut Conn, id: Id) -> ApiResult<Prepared> {
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id,
        },
    )
    .await?;
    let mut plan = Prepared::new(ctx.route, ctx.route.name)?;
    plan.bind(RESPONSE_FACET, target.object.kind, &[target])?;
    Ok(plan)
}

fn operation<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    _body: ProviderResetConsumeBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        // Preserve the historical refusal before the shared compatibility check.
        check_legacy(caller)?;
        let target = targets::page_object(
            conn,
            ObjectRef {
                kind: ObjectKind::Provider,
                id: ctx.id("id")?,
            },
        )
        .await?;
        let mut plan = Prepared::new(ctx.route, ctx.route.name)?;
        plan.bind(
            EFFECT_FACET,
            target.object.kind,
            std::slice::from_ref(&target),
        )?;
        plan.bind(RESPONSE_FACET, target.object.kind, &[target])?;
        Ok(plan)
    })
}

async fn current_provider(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<Option<Provider>> {
    if ctx.key_receipt_required() {
        return Ok(None);
    }
    let plan = match response_plan(ctx, conn, id).await {
        Ok(plan) => plan,
        Err(error) if matches!(error.problem.status, 403 | 404 | 410) => {
            ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Denied);
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let projection_ctx = ctx.clone();
    match plan
        .read_response(ctx, conn, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                authorization::require_resource(
                    conn,
                    reader,
                    ResourceKind::Provider,
                    id,
                    Permission::ProviderView,
                    Policy::default(),
                )
                .await?;
                show(&projection_ctx, conn, reader, id).await
            })
        })
        .await
    {
        Ok(provider) => Ok(provider),
        Err(error) if matches!(error.problem.status, 403 | 404 | 410) => {
            ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Denied);
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Never construct a current-provider error payload without this same operation's Read.
async fn with_current(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    problem: ApiError,
) -> ApiError {
    match current_provider(ctx, conn, principal, id).await {
        Ok(Some(provider)) => problem.with("current", provider),
        Ok(None) => problem,
        Err(error) => error,
    }
}

async fn stale(ctx: &Ctx, conn: &mut Conn, principal: &Principal, id: Id) -> ApiError {
    with_current(
        ctx,
        conn,
        principal,
        id,
        ApiError::new(
            ErrorCode::ProviderResetStale,
            "The reset or the number available changed. Read the current facts and confirm again.",
        ),
    )
    .await
}

struct ResetProjection {
    full: Option<ProviderResetConsumeReply>,
    receipt: ProviderResetConsumeReceipt,
}

impl ResetProjection {
    async fn load(
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
        id: Id,
        request: Id,
        outcome: String,
    ) -> ApiResult<Self> {
        let full = current_provider(ctx, conn, principal, id)
            .await?
            .map(|provider| ProviderResetConsumeReply {
                outcome: outcome.clone(),
                provider,
            });
        Ok(Self {
            full,
            receipt: ProviderResetConsumeReceipt {
                projection: ReceiptProjection::Receipt,
                provider_id: id,
                request_id: request,
                outcome,
            },
        })
    }

    fn finish(self, ctx: &Ctx) -> ProviderResetConsumeResponse {
        match (self.full, ctx.key_receipt_required()) {
            (Some(full), false) => ProviderResetConsumeResponse::full(full),
            _ => ProviderResetConsumeResponse::Receipt(self.receipt),
        }
    }
}

async fn consume(
    mut ctx: Ctx,
    (): (),
    body: ProviderResetConsumeBody,
) -> ApiResult<ProviderResetConsumeResponse> {
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    check_legacy(guard.principal().caller())?;
    if guard.user().is_session() && guard.user().user.kind != "human" {
        return Err(legacy_refusal());
    }
    let principal = guard.principal();
    authorization::require_resource(
        &mut tx,
        principal,
        ResourceKind::Provider,
        id,
        Permission::ProviderManage,
        Policy::default(),
    )
    .await?;
    ctx.now = guard.now().get();
    if !rows::lock_provider(&mut tx, id).await? {
        return Err(ApiError::not_found());
    }
    let provider = resources::provider(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if let Some(receipt) =
        rows::reset_receipt(&mut tx, id, guard.user().id(), body.request_id).await?
    {
        if receipt.get("reset_id").and_then(Json::as_str) != Some(body.reset_id.as_str())
            || receipt.get("available_count").and_then(Json::as_i64) != Some(body.available_count)
        {
            return Err(stale(&ctx, &mut tx, principal, id).await);
        }
        let outcome = receipt
            .get("outcome")
            .and_then(Json::as_str)
            .ok_or_else(|| ApiError::internal("a reset receipt has no outcome"))?
            .to_owned();
        crate::key_operations::activity::replayed();
        let projection =
            ResetProjection::load(&ctx, &mut tx, principal, id, body.request_id, outcome).await?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok(projection.finish(&ctx));
    }
    if provider.status == "disabled" {
        return Err(ApiError::new(
            ErrorCode::ProviderResetUnavailable,
            "The provider is disabled.",
        ));
    }
    let operation =
        orch_engine::provider_fact_reads::reset_operation(id, guard.user().id(), body.request_id);
    let reserved = rows::reset_request(&mut tx, operation).await?;
    if reserved.as_ref().is_some_and(|old| {
        old.get("reset_id").and_then(Json::as_str) != Some(body.reset_id.as_str())
            || old.get("available_count").and_then(Json::as_i64) != Some(body.available_count)
            || old.get("request_id").and_then(Json::as_str)
                != Some(body.request_id.to_string().as_str())
    }) {
        return Err(stale(&ctx, &mut tx, principal, id).await);
    }
    // Confirm only what the runtime actually observed, never an override of the available count.
    let snapshot_epoch = rows::credential_epoch(&mut tx, id, Some(provider.revision))
        .await?
        .ok_or_else(ApiError::not_found)?;
    let read_started = Time::new(ctx.now);
    if reserved.is_none()
        && !rows::claim_read(&mut tx, id, true, ctx.now, chrono::Duration::zero()).await?
    {
        return Err(ApiError::new(
            ErrorCode::ProviderResetUnavailable,
            "This source's minimum read interval has not elapsed. Confirm the reset after its next read.",
        ));
    }
    if let Some(reserved) = &reserved {
        let selected = orch_engine::provider_sources::spec(
            &mut tx,
            id,
            &provider.sources,
            orch_provider_contract::SourceElement::Facts,
        )
        .await
        .map_err(ApiError::internal)?;
        let stored: Option<orch_provider_contract::SourceSpec> =
            serde_json::from_value(reserved.get("facts").cloned().unwrap_or(Json::Null))
                .map_err(ApiError::internal)?;
        let mut selected = selected.unwrap_or_default();
        let mut stored = stored.unwrap_or_default();
        // Learning a default descriptor, or renaming the provider, does not change the source that spends.
        selected.provider = None;
        stored.provider = None;
        if stored != selected {
            return Err(stale(&ctx, &mut tx, guard.principal(), id).await);
        }
    }
    let fresh = if reserved.is_some() {
        orch_link_contract::commands::ProviderRefreshResult {
            source: None,
            status: "not_told".to_owned(),
            facts: Some(
                serde_json::from_value(provider.facts.clone()).map_err(ApiError::internal)?,
            ),
        }
    } else {
        ctx.app.engine.inspect_provider_reset(id).await.map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::new(ErrorCode::ProviderResetUnavailable,"The runtime could not confirm which resets are available. Refresh before confirming again."))?
    };
    let current_authority = transaction_guard::check(&ctx, &mut tx).await?;
    authorization::require_resource(
        &mut tx,
        current_authority.principal(),
        ResourceKind::Provider,
        id,
        Permission::ProviderManage,
        Policy::default(),
    )
    .await?;
    ctx.now = current_authority.now().get();
    let principal = current_authority.principal();
    if rows::credential_epoch(&mut tx, id, None).await?.as_ref() != Some(&snapshot_epoch) {
        return Err(stale(&ctx, &mut tx, principal, id).await);
    }
    let mut recovered = ctx
        .app
        .engine
        .keep_provider_observation(&mut tx, id, &fresh, ctx.now)
        .await
        .map_err(ApiError::internal)?;
    let Some(facts) = fresh.facts.clone() else {
        let unavailable = with_current(
            &ctx,
            &mut tx,
            principal,
            id,
            ApiError::new(
                ErrorCode::ProviderResetUnavailable,
                "The source did not report its current reset credits.",
            ),
        )
        .await;
        tx.commit().await.map_err(DbError::from)?;
        if recovered {
            ctx.app
                .engine
                .reevaluate_provider(id)
                .await
                .map_err(ApiError::internal)?;
        }
        return Err(unavailable);
    };
    let current = facts.available_resets.as_ref();
    let reset_read_at = facts.field_times.get("available_resets");
    let spends_resets = ctx
        .app
        .engine
        .provider_spends_resets(&mut tx, id)
        .await
        .map_err(ApiError::internal)?;
    if reserved.is_none()
        && (!spends_resets
            || fresh.status == "not_told"
            || fresh
                .source
                .as_ref()
                .is_some_and(|source| source.error.is_some() && source.supported != Some(true))
            || current.is_none()
            || reset_read_at.is_none_or(|at| *at < read_started))
    {
        let unavailable = with_current(
            &ctx,
            &mut tx,
            principal,
            id,
            ApiError::new(
                ErrorCode::ProviderResetUnavailable,
                "This read did not confirm the provider's current reset availability.",
            ),
        )
        .await;
        tx.commit().await.map_err(DbError::from)?;
        if recovered {
            ctx.app
                .engine
                .reevaluate_provider(id)
                .await
                .map_err(ApiError::internal)?;
        }
        return Err(unavailable);
    }
    let credit = current
        .and_then(|resets| resets.credits.as_ref())
        .and_then(|credits| credits.iter().find(|credit| credit.id == body.reset_id));
    if reserved.is_none()
        && (body.available_count <= 0
            || current.is_none_or(|current| current.available_count != body.available_count)
            || credit.is_none_or(|credit| {
                credit.status != "available"
                    || credit.expires_at.is_some_and(|at| at.get() <= ctx.now)
            }))
    {
        let refused = stale(&ctx, &mut tx, principal, id).await;
        tx.commit().await.map_err(DbError::from)?;
        if recovered {
            ctx.app
                .engine
                .reevaluate_provider(id)
                .await
                .map_err(ApiError::internal)?;
        }
        return Err(refused);
    }
    // Namespace the caller's request identity here: retries by this person for this provider use the same vendor key.
    let dispatch_authority = transaction_guard::check(&ctx, &mut tx).await?;
    authorization::require_resource(
        &mut tx,
        dispatch_authority.principal(),
        ResourceKind::Provider,
        id,
        Permission::ProviderManage,
        Policy::default(),
    )
    .await?;
    let result = ctx.app.engine.consume_provider_reset(id,&body.reset_id,guard.user().id(),body.request_id,body.available_count)
        .await.map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::new(ErrorCode::ProviderResetUnavailable,"The provider could not answer this reset attempt. Retry the same request when it is available."))?;
    let now = ctx.app.clock.now();
    let after: orch_agent_contract::facts::ProviderFacts = result.facts.clone();
    let failed = result
        .source
        .as_ref()
        .is_some_and(|source| source.error.is_some() && source.supported != Some(true));
    let observed = orch_link_contract::commands::ProviderRefreshResult {
        source: result.source.clone(),
        facts: Some(after.clone()),
        status: if failed {
            "not_told"
        } else if orch_engine::provider_fact_reads::admission_exhausted(&after, Time::new(now)) {
            "exhausted"
        } else if after.ordinary_usage_allowed == Some(true) {
            "ready"
        } else {
            provider.status.as_str()
        }
        .to_owned(),
    };
    if !failed {
        let shown_until = after
            .windows
            .iter()
            .filter_map(|window| window.resets_at.map(|at| at.get()))
            .min();
        ctx.now = now;
        record(&ctx,&mut tx,orch_db::audit::action!(AuditAction::ProviderResetUsed),id,Home {node:Some(provider.home_id),org:Some(provider.org_id)},
        "A provider manager confirmed a reset",json!({"request_id":body.request_id,"reset_id":body.reset_id,"available_count":body.available_count,
            "outcome":result.outcome,"windows_before":facts.windows,"windows_after":after.windows,"shown_until":shown_until.map(Time::new)})).await?;
    }
    recovered |= ctx
        .app
        .engine
        .keep_provider_observation(&mut tx, id, &observed, now)
        .await
        .map_err(ApiError::internal)?;
    if failed {
        ctx.now = now;
        let unavailable = with_current(&ctx, &mut tx, principal, id, ApiError::new(ErrorCode::ProviderResetUnavailable,"The source could not confirm this reset attempt. Retry the same request when it is available.")).await;
        tx.commit().await.map_err(DbError::from)?;
        if recovered {
            ctx.app
                .engine
                .reevaluate_provider(id)
                .await
                .map_err(ApiError::internal)?;
        }
        return Err(unavailable);
    }
    tx.commit().await.map_err(DbError::from)?;
    let (mut reply_tx, reply_guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = reply_guard.now().get();
    authorization::require_resource(
        &mut reply_tx,
        reply_guard.principal(),
        ResourceKind::Provider,
        id,
        Permission::ProviderManage,
        Policy::default(),
    )
    .await?;
    let projection = ResetProjection::load(
        &ctx,
        &mut reply_tx,
        reply_guard.principal(),
        id,
        body.request_id,
        result.outcome,
    )
    .await?;
    reply_tx.commit().await.map_err(DbError::from)?;
    if recovered {
        ctx.app
            .engine
            .reevaluate_provider(id)
            .await
            .map_err(ApiError::internal)?;
    }
    Ok(projection.finish(&ctx))
}

/// Safe attribution for the last reset of each visible provider, until its resulting windows begin again.
pub async fn last_resets(
    conn: &mut Conn,
    providers: &[Id],
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<HashMap<Id, ProviderLastReset>> {
    let resets = rows::last_resets(conn, providers, now).await?;
    let users = authorization::refusals::user_refs(
        conn,
        &resets.iter().map(|reset| reset.user_id).collect::<Vec<_>>(),
    )
    .await?;
    Ok(resets
        .into_iter()
        .filter_map(|reset| {
            users.get(&reset.user_id).cloned().map(|user| {
                (
                    reset.provider_id,
                    ProviderLastReset {
                        user,
                        reset_id: reset.reset_id,
                        at: Time::new(reset.at),
                    },
                )
            })
        })
        .collect())
}
