//! Host maintenance settings use the shared registry, resolver, locks and revision guards.

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::access::page::same;
use crate::routes::nodes::invalid;
use crate::routes::resources::{Home, begin_mutation, record, require};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::settings as api;
use orch_authz::{Policy, resources};
use orch_db::{Conn, DbError};
use orch_settings::{Place, resolve, resolve_all};
use orch_settings_contract::registry::{self, Scope, keys};
use orch_types::Id;
use serde_json::json;

/// Binds host maintenance policy reads and guarded changes.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::HostSettingList, list)
        .on(api::HostSettingSet, set)
        .on(api::HostSettingDelete, clear);
}

async fn projected(
    conn: &mut Conn,
    id: Id,
    permitted: bool,
) -> ApiResult<Vec<api::ResolvedSetting>> {
    resolve_all(conn, Place::Host(id))
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|view| {
            same(&view.into_setting(permitted))
                .ok_or_else(|| ApiError::internal("a host setting cannot be projected"))
        })
        .collect()
}

async fn list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<api::ResolvedSetting>> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    require(
        &mut conn,
        &principal,
        ResourceKind::Host,
        id,
        Permission::HostView,
    )
    .await?;
    let permitted = resources::check(
        &mut conn,
        &principal,
        ObjectRef {
            kind: ResourceKind::Host.into(),
            id,
        },
        Permission::HostManage,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .is_allowed();
    projected(&mut conn, id, permitted).await
}

async fn set(ctx: Ctx, (): (), body: api::NodeSettingSetBody) -> ApiResult<api::ResolvedSetting> {
    if body.locked == Some(true) {
        return Err(invalid("locked", "a host's own value cannot carry a lock"));
    }
    let key = ctx.param("key")?;
    registry::validate_at(key, Scope::Host, &body.value)
        .map_err(|error| invalid("value", &error.to_string()))?;
    let days = body
        .value
        .as_i64()
        .ok_or_else(|| invalid("value", "must be a nonnegative whole number"))?;
    change(ctx, Some(days), body.expected_revision).await
}

async fn clear(
    ctx: Ctx,
    query: api::NodeSettingDeleteQuery,
    (): (),
) -> ApiResult<api::ResolvedSetting> {
    change(ctx, None, query.expected_revision).await
}

async fn change(
    ctx: Ctx,
    days: Option<i64>,
    expected: Option<i64>,
) -> ApiResult<api::ResolvedSetting> {
    let id = ctx.id("id")?;
    if ctx.param("key")? != keys::CLEANUP_UNUSED_KEEP_DAYS {
        return Err(ApiError::not_found());
    }
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    require(
        &mut tx,
        &principal,
        ResourceKind::Host,
        id,
        Permission::HostManage,
    )
    .await?;
    let (before, revision, name, home, org): (Option<i64>, i64, String, Option<Id>, Option<Id>) = sqlx::query_as("SELECT cleanup_keep_days,revision,name,home_id,org_id FROM hosts WHERE id=$1 FOR UPDATE")
        .bind(id).fetch_optional(&mut *tx).await.map_err(DbError::from)?.ok_or_else(ApiError::not_found)?;
    let view = resolve(&mut tx, Place::Host(id), keys::CLEANUP_UNUSED_KEEP_DAYS)
        .await
        .map_err(ApiError::internal)?;
    if !view.open_to_write() {
        return Err(ApiError::new(
            ErrorCode::AccessLocked,
            "the system locks automatic cleanup",
        ));
    }
    let own_revision = before.map_or(0, |_| revision);
    if expected.is_some_and(|expected| expected != own_revision) {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the host setting changed meanwhile",
        ));
    }
    if before != days {
        sqlx::query(
            "UPDATE hosts SET cleanup_keep_days=$2,revision=revision+1,updated_at=$3 WHERE id=$1",
        )
        .bind(id)
        .bind(days)
        .bind(ctx.now)
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
        record(
            &ctx,
            &mut tx,
            AuditAction::HostUpdated,
            id,
            Home { node: home, org },
            &format!("Automatic cleanup changed on {name}"),
            json!({"host":id,"key":keys::CLEANUP_UNUSED_KEEP_DAYS,"before":before,"value":days}),
        )
        .await?;
    }
    let setting = projected(&mut tx, id, true)
        .await?
        .into_iter()
        .find(|setting| setting.key == keys::CLEANUP_UNUSED_KEEP_DAYS)
        .ok_or_else(|| ApiError::internal("the changed host setting is absent"))?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(setting)
}
