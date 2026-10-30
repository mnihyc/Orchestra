//! Pause, enable and delete previews read the shared dependency and work service with native resource authority.

use crate::{
    middleware::{
        authorization,
        requests::{ApiError, ApiResult},
    },
    registry::{Ctx, Registry},
};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::resource_facts::{self as api, FactTarget, ResourceImpact};
use orch_db::DbError;
use orch_engine::resource_facts::{self, activity};
use orch_types::Time;

/// Binds the shared current-consequence read.
pub fn mount(registry: &mut Registry) {
    registry.on(api::ResourceImpactRead, read);
}

/// Pause notes remain a single visible line and an empty field means no note.
pub(crate) fn note(value: Option<&str>) -> ApiResult<Option<&str>> {
    if value.is_some_and(|value| value.contains(['\n', '\r', '\u{2028}', '\u{2029}'])) {
        return Err(ApiError::invalid("note", "must be one line"));
    }
    Ok(value.map(str::trim).filter(|value| !value.is_empty()))
}

async fn read(ctx: Ctx, (): (), target: FactTarget) -> ApiResult<ResourceImpact> {
    if !matches!(
        target.object.kind,
        ObjectKind::Provider | ObjectKind::Agent | ObjectKind::Environment
    ) || target.model_id.is_some() && target.object.kind != ObjectKind::Provider
    {
        return Err(ApiError::invalid(
            "object",
            "must be a provider, profile, environment or a model of its provider",
        ));
    }
    let mut tx = ctx.app.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let source = resource_facts::authorities(&mut tx, &principal, &[target.object.id])
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .find(|row| row.resource.object() == target.object && (row.readable || row.manageable))
        .ok_or_else(ApiError::not_found)?;
    let mut name = source.resource.name;
    if let Some(model) = target.model_id {
        name=sqlx::query_scalar("SELECT COALESCE(NULLIF(display_name,''),model_name) FROM provider_models WHERE provider_id=$1 AND id=$2").bind(target.object.id).bind(model).fetch_optional(&mut *tx).await.map_err(DbError::from)?.ok_or_else(ApiError::not_found)?;
    }
    let at = Time::new(ctx.now);
    let raw = resource_facts::target_dependents(&mut tx, &target, at)
        .await
        .map_err(ApiError::internal)?;
    let facts = resource_facts::project(&mut tx, &principal, Default::default(), &[raw])
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("the impact facts were not projected"))?;
    let work = activity::read(&mut tx, &target)
        .await
        .map_err(ApiError::internal)?;
    let activity = activity::project(&mut tx, &principal, &target, &work, at)
        .await
        .map_err(ApiError::internal)?;
    let stopping = activity::stopping(&mut tx, &target)
        .await
        .map_err(ApiError::internal)?;
    let threads: Vec<_> = facts
        .dependents
        .iter()
        .filter(|group| group.relation == api::DependentRelation::Binding)
        .flat_map(|group| &group.visible)
        .filter_map(|row| row.object.as_ref())
        .filter(|object| object.kind == ObjectKind::Thread)
        .map(|object| object.id)
        .collect();
    let thread_displays =
        crate::routes::nodes::view::refs(&mut tx, &principal, &threads, Default::default())
            .await?
            .into_iter()
            .filter_map(|(id, row)| row.thread_display.map(|display| (id, *display)))
            .collect();
    Ok(ResourceImpact {
        target,
        name,
        facts,
        activity,
        stopping,
        thread_displays,
    })
}
