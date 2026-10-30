//! The settings of the system: every key that is set at system scope with where its value comes from, written (and locked for every node below, or not), and taken back to what Orch ships.
//!
//! The settings engine does all of it, with the system as the place: the registry says which keys
//! there are and what each accepts, a lock above the system does not exist, and a value written as
//! it stands changes nothing and leaves no line. A system admin holds what writing and locking
//! any of them takes, so the one question asked here is whether the caller is one.

use super::{admin_of, invalid};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::access::page::same;
use crate::sign_in::origin;
use orch_api_contract::admin::{
    AdminSettingDelete, AdminSettingDeleteQuery, AdminSettingList, AdminSettingReachGet,
    AdminSettingSet, AdminSettingSetBody,
};
use orch_api_contract::common::{ErrorCode, FieldFault, Problem};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::settings::ResolvedSetting;
use orch_db::DbError;
use orch_db::audit::Actor;
use orch_settings::locks::Refusal;
use orch_settings::write::{Clear, Write, WriteError, clear, write};
use orch_settings::{Place, SettingView, Target, resolve_all};
use orch_settings_contract::registry::Invalid;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(AdminSettingList, list)
        .on(AdminSettingReachGet, reach)
        .on(AdminSettingSet, set)
        .on(AdminSettingDelete, delete);
}

/// A setting as the API shows it. Nothing at system scope is a node, so no node is named; the
/// closure is the answer for a view that names one all the same.
fn shown(view: SettingView) -> ApiResult<ResolvedSetting> {
    let setting = view.into_setting(true).map_node(|id| NodeRef {
        thread_display: None,
        id,
        kind: NodeKind::Org,
        name: None,
        redacted: true,
    });
    same(&setting).ok_or_else(|| ApiError::internal("a setting does not read as the API's"))
}

/// What the answer to a write that was refused is.
fn refused(error: WriteError) -> ApiError {
    match error {
        WriteError::Db(error) => error.into(),
        WriteError::NotFound | WriteError::Invalid(Invalid::UnknownKey(_)) => ApiError::not_found(),
        WriteError::Invalid(Invalid::WrongScope { key, scope }) => {
            invalid("key", &format!("{key} is not set at the scope {scope}"))
        }
        WriteError::Invalid(Invalid::Value { faults, .. }) => Problem::invalid(
            faults
                .into_iter()
                .map(|fault| FieldFault {
                    path: if fault.path.is_empty() {
                        "value".to_owned()
                    } else {
                        format!("value.{}", fault.path)
                    },
                    message: fault.message,
                })
                .collect(),
        )
        .into(),
        WriteError::Lock(Refusal::NotLockable) => invalid(
            "locked",
            "this setting is not one that can be locked here: nothing below the system could be told otherwise",
        ),
        WriteError::Lock(Refusal::NotPermitted) => ApiError::new(
            ErrorCode::AuthzDenied,
            "only a system admin sets or lifts a lock of the system",
        ),
        WriteError::Stale { current } => match shown(*current) {
            Ok(setting) => ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the setting was changed since it was read",
            )
            .with("current", setting),
            Err(error) => error,
        },
        // The system has no place above it and is no thread.
        other @ (WriteError::SideThread
        | WriteError::Locked { .. }
        | WriteError::Unreadable { .. }) => ApiError::internal(other),
    }
}

/// Every key that is set at system scope, as it holds there.
async fn list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<ResolvedSetting>> {
    admin_of(&ctx)?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = crate::middleware::authorization::principal(&ctx, &mut conn).await?;
    let mut rows = resolve_all(&mut conn, Place::System)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(shown)
        .collect::<ApiResult<Vec<_>>>()?;
    crate::routes::settings::add_mode_choices(
        &ctx,
        &mut conn,
        &principal,
        Place::System,
        &mut rows,
    )
    .await?;
    Ok(rows)
}

/// The same read gate as the system's settings list.
async fn reach(
    ctx: Ctx,
    query: orch_api_contract::common::PageQuery,
    (): (),
) -> ApiResult<orch_api_contract::settings::SettingReach> {
    admin_of(&ctx)?;
    crate::routes::settings::reach::read(&ctx, None, &query).await
}

/// Writes a key at system scope.
async fn set(mut ctx: Ctx, (): (), body: AdminSettingSetBody) -> ApiResult<ResolvedSetting> {
    admin_of(&ctx)?;
    let key = ctx.param("key")?.to_owned();
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    crate::routes::resources::validation::setting(&ctx.app.engine, &key, &body.value)?;
    let written = write(
        &mut tx,
        &Write {
            actor: Actor::User(admin),
            target: Target::System,
            key: &key,
            value: body.value,
            locked: body.locked,
            expected_revision: body.expected_revision,
            may_lock: true,
            origin: &origin(&ctx),
        },
        ctx.now,
    )
    .await
    .map_err(refused)?;
    let mut reply = shown(written.view)?;
    crate::routes::settings::add_mode_choices(
        &ctx,
        &mut tx,
        guard.principal(),
        Place::System,
        std::slice::from_mut(&mut reply),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    if key == orch_settings_contract::registry::keys::AUTH_SECOND_STEP {
        crate::routes::settings::second_step_policy_changed(&ctx).await;
    }
    Ok(reply)
}

/// Takes a key at system scope back to what Orch ships.
async fn delete(
    mut ctx: Ctx,
    query: AdminSettingDeleteQuery,
    (): (),
) -> ApiResult<ResolvedSetting> {
    admin_of(&ctx)?;
    let key = ctx.param("key")?.to_owned();
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    let cleared = clear(
        &mut tx,
        &Clear {
            actor: Actor::User(admin),
            target: Target::System,
            key: &key,
            expected_revision: query.expected_revision,
            may_lock: true,
            origin: &origin(&ctx),
        },
        ctx.now,
    )
    .await
    .map_err(refused)?;
    let mut reply = shown(cleared.view)?;
    crate::routes::settings::add_mode_choices(
        &ctx,
        &mut tx,
        guard.principal(),
        Place::System,
        std::slice::from_mut(&mut reply),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    if key == orch_settings_contract::registry::keys::AUTH_SECOND_STEP {
        crate::routes::settings::second_step_policy_changed(&ctx).await;
    }
    Ok(reply)
}
