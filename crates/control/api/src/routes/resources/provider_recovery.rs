//! A provider's existing re-create control reviews lost runtime provisioning without borrowing its owner's destination authority.

use crate::{
    middleware::{
        authorization,
        requests::{ApiError, ApiResult},
    },
    registry::Ctx,
};
use orch_api_contract::{
    forks::{ForkCreateBody, ProviderUpdatePlan, Rollback, UpdateReason, UpdateThread},
    loss::OperationNotificationPlan,
    workspace::Job,
};
use orch_authz::Principal;
use orch_db::{
    Conn, DbError,
    queries::{jobs, resources::provider_runtime as rows},
};
use orch_engine::{notifications, operation_access, provider_recovery as recovery};
use orch_types::{Id, Time, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize, Deserialize)]
struct Stored {
    recipe: recovery::Recipe,
    grants: Vec<orch_api_contract::loss::OperationAdminGrant>,
    audience: Vec<notifications::OperationRecipient>,
}
/// Whether the ordinary provider-runtime page has a declared unavailable source and no current unit.
pub(super) async fn needed(conn: &mut Conn, id: Id) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT runtime_unavailable IS NOT NULL AND NOT EXISTS(SELECT 1 FROM runtimes r WHERE r.provider_id=p.id AND r.successor_of IS NULL AND r.state NOT IN ('retired','removed')) FROM providers p WHERE p.id=$1 AND p.deleted_at IS NULL").bind(id).fetch_optional(conn).await.map_err(DbError::from)?.unwrap_or(false))
}
/// The normal runtime review dialog receives the same plan shape with truthful no-source rollback.
pub(super) async fn preflight(
    ctx: &Ctx,
    principal: &Principal,
    provider: Id,
) -> ApiResult<ProviderUpdatePlan> {
    let id = new_id();
    let mut tx = ctx.app.db.begin().await?;
    let draft = ctx
        .app
        .engine
        .draft_provider_recovery(&mut tx, principal, provider)
        .await
        .map_err(crate::routes::environments::updates::refused)?;
    let provider_ref = super::view::provider_refs(&mut tx, &[provider], &[])
        .await?
        .pop()
        .map(|(_, row)| row)
        .ok_or_else(ApiError::not_found)?;
    let ids: Vec<Id> = sqlx::query_scalar(
        "SELECT t.node_id FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.provider_id=$1 AND n.deleted_at IS NULL ORDER BY t.node_id",
    )
    .bind(provider)
    .fetch_all(&mut *tx)
    .await
    .map_err(DbError::from)?;
    let refs =
        crate::routes::nodes::view::refs(&mut tx, principal, &ids, orch_authz::Policy::default())
            .await?;
    let mut threads = Vec::new();
    for thread in ids {
        threads.push(UpdateThread {
            thread: refs.get(&thread).filter(|row| !row.redacted).cloned(),
            turn_running: orch_db::queries::threads::active_turn(&mut tx, thread)
                .await?
                .is_some(),
        });
    }
    let options =
        orch_engine::updates::options(&mut tx, orch_settings::Place::Provider(provider), None)
            .await
            .map_err(crate::routes::environments::updates::refused)?;
    let audience = notifications::operation_audience(&mut tx, principal, &[], &[provider])
        .await
        .map_err(ApiError::internal)?;
    let mut network = orch_api_contract::environment::NetworkState::open(
        if draft.recipe.definition["network"]["mode"] == "none" {
            orch_api_contract::environment::UnitNetworkMode::None
        } else {
            orch_api_contract::environment::UnitNetworkMode::Bridge
        },
    );
    if draft.recipe.definition["network"]["egress"] == "restricted" {
        network.mode = orch_api_contract::environment::EgressMode::Restricted;
        network.required_mode = orch_api_contract::environment::EgressMode::Restricted;
    }
    let plan = ProviderUpdatePlan {
        target_programs: None,
        notify: None,
        recovery: true,
        notification: Some(OperationNotificationPlan {
            preview_id: Some(id),
            people: audience.len() as i64,
            default_on: notifications::operation_default(
                &mut tx,
                orch_settings::Place::Provider(provider),
                false,
            )
            .await
            .map_err(ApiError::internal)?,
        }),
        admin_grants: draft.grants.clone(),
        preflight_id: id,
        expires_at: Time::new(orch_engine::updates::expires(ctx.now)),
        provider: provider_ref,
        reason: UpdateReason::Recreate,
        updates: vec![orch_api_contract::environment::PendingUpdate {
            what: orch_api_contract::environment::UpdateWhat::Settings,
            name: "Host".into(),
            from: None,
            to: Some(draft.host_name),
        }],
        kept: vec![],
        lost: vec![],
        rollback: Rollback::Unavailable,
        network,
        threads,
        options,
        estimate: orch_engine::updates::estimate(),
        blockers: vec![],
    };
    let stored = Stored {
        recipe: draft.recipe,
        grants: draft.grants,
        audience,
    };
    tx.rollback().await.map_err(DbError::from)?;
    let mut tx = ctx.app.db.begin().await?;
    jobs::forks::save_preflight(
        &mut tx,
        &jobs::forks::PreflightRow {
            id,
            kind: "provider_update".into(),
            node_id: None,
            environment_id: None,
            created_by: principal.user(),
            plan: json!({"provider_recovery":stored}),
            expires_at: plan.expires_at.get(),
            created_at: ctx.now,
        },
    )
    .await?;
    orch_db::queries::notifications::operations::preview(
        &mut tx,
        id,
        &stored
            .audience
            .iter()
            .map(|person| person.user)
            .collect::<Vec<_>>(),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(plan)
}
/// The retained provider id, settings and page are kept; the lost connector state is never reused.
pub(super) async fn create(ctx: &Ctx, body: ForkCreateBody) -> ApiResult<Job> {
    let provider = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    let original = jobs::forks::preflight(&mut conn, body.preflight_id)
        .await?
        .filter(|row| {
            row.kind == "provider_update"
                && row.created_by == principal.user()
                && row.expires_at > ctx.now
        })
        .ok_or_else(ApiError::not_found)?;
    drop(conn);
    let stored: Stored = serde_json::from_value(original.plan["provider_recovery"].clone())
        .map_err(ApiError::internal)?;
    if stored.recipe.provider != provider {
        return Err(ApiError::not_found());
    }
    let mut preview = ctx.app.db.begin().await?;
    let fresh = ctx
        .app
        .engine
        .draft_provider_recovery(&mut preview, &principal, provider)
        .await
        .map_err(crate::routes::environments::updates::refused)?;
    if fresh.recipe != stored.recipe
        || fresh
            .grants
            .iter()
            .any(|grant| !stored.grants.contains(grant))
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::ConflictStaleRevision,
            "The provider or its destination changed; review its recovery again",
        ));
    }
    preview.rollback().await.map_err(DbError::from)?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(ctx).await?;
    let current = jobs::changes::lock_plan(&mut tx, body.preflight_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if current.plan != original.plan || current.expires_at <= ctx.now {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::PreflightExpired,
            "Review a fresh recovery plan",
        ));
    }
    operation_access::accept(
        &mut tx,
        &principal,
        &stored.grants,
        &orch_db::audit::Origin::default(),
    )
    .await
    .map_err(crate::routes::environments::updates::refused)?;
    let audience = notifications::operation_audience(&mut tx, &principal, &[], &[provider])
        .await
        .map_err(ApiError::internal)?;
    if audience != stored.audience {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::ConflictStaleRevision,
            "The affected people changed; review its recovery again",
        ));
    }
    let state = rows::state(&mut tx, provider)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let operation = notifications::NotificationOperation {
        id: body.preflight_id,
        kind: "provider_recover".into(),
        name: state.name,
        notify: body.notify.unwrap_or(false),
        path: format!("/resources/providers/{provider}"),
        subject: Some(("provider".into(), provider)),
        recipients: audience,
    };
    let row = notifications::operation_scope(
        Some(operation.id),
        recovery::queue(
            &ctx.app.engine,
            &mut tx,
            &principal,
            &stored.recipe,
            ctx.now,
        ),
    )
    .await
    .map_err(crate::routes::environments::updates::refused)?;
    notifications::attach_operation(&mut tx, &[row.id], &operation, ctx.now)
        .await
        .map_err(ApiError::internal)?;
    notifications::notify_operation(&mut tx, &operation, "running", ctx.now)
        .await
        .map_err(ApiError::internal)?;
    let mut entry = orch_db::audit::Entry::new(
        orch_db::audit::Actor::User(principal.user()),
        orch_db::audit::name!("provider.updated"),
        "provider",
        Some(provider),
        "Requested re-creation of a lost provider runtime",
    );
    entry.history = true;
    entry.r#override = !stored.grants.is_empty();
    entry.details =
        json!({"job":row.id,"host":stored.recipe.host,"template":stored.recipe.template});
    orch_db::audit::append(&mut tx, &entry, &orch_db::audit::Origin::default(), ctx.now).await?;
    jobs::forks::forget_preflight(&mut tx, body.preflight_id).await?;
    let result = crate::routes::environments::jobs::view(&mut tx, &row).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(result)
}
