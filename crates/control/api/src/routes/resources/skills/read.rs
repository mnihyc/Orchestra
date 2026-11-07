//! The native Skill family with once-only URL custody after the successful read snapshot.

use super::*;
use crate::middleware::transaction_guard;
use crate::registry::OperationPlan;
use crate::routes::resources::{
    collection::Collection,
    projection::{self, Rendered},
    read::{self, Cardinality, Family},
};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;

struct Skills;
pub(super) fn mount(registry: &mut Registry) {
    read::register::<Skills, _, _>(registry, api::SkillGet, api::SkillList);
}

impl Family for Skills {
    type Item = Skill;
    type ListQuery = api::SkillListQuery;
    const SPEC: &'static orch_api_contract::resource_reads::Family = &api::READS;
    const CARDINALITY: Cardinality = Cardinality::Exhaustive;
    fn require_get<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: Id,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(
            conn,
            principal,
            ResourceKind::Skill,
            id,
            Permission::SkillView,
        ))
    }
    fn prepare_get<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: Id,
    ) -> BoxFuture<'a, ApiResult<projection::Prepared>> {
        Box::pin(async move {
            let seen = single_seen(conn, principal, id).await?;
            projection::prepare(
                ctx,
                conn,
                principal,
                projection::Selection {
                    kind: ResourceKind::Skill,
                    ids: vec![id],
                    seen: &[seen],
                },
            )
            .await?
            .ok_or_else(ApiError::not_found)
        })
    }
    fn filter<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        mut rows: Vec<resources::Seen>,
        query: &'a api::SkillListQuery,
    ) -> BoxFuture<'a, ApiResult<Vec<resources::Seen>>> {
        Box::pin(async move {
            if let Some(org) = query.org_id {
                let homes: Vec<_> = rows.iter().filter_map(|row| row.resource.home).collect();
                let inside =
                    orch_db::queries::resources::routes::homes_under(conn, org, &homes).await?;
                rows.retain(|row| row.resource.home.is_some_and(|home| inside.contains(&home)));
            }
            Ok(rows)
        })
    }
    fn into_items(value: Rendered) -> ApiResult<Vec<Skill>> {
        match value {
            Rendered::Skill(rows) => Ok(rows),
            _ => Err(ApiError::internal(
                "the native Skill reader returned another family",
            )),
        }
    }
    fn after_get<'a>(ctx: &'a Ctx, value: &'a Skill) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(maintain(ctx, vec![value.id]))
    }
    fn after_list<'a>(ctx: &'a Ctx, values: &'a [Listed<Skill>]) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(maintain(
            ctx,
            values
                .iter()
                .filter_map(|value| value.as_visible().map(|skill| skill.id))
                .collect(),
        ))
    }
}

async fn maintain(ctx: &Ctx, mut ids: Vec<Id>) -> ApiResult<()> {
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(());
    }
    // Existing deployment-key IO completes before the action footprint and final lifetime cut.
    crate::routes::resources::sealing::master_key()?;
    // This is the original internal maintenance write, outside the watched read snapshot.
    let mut tx = ctx.app.db.begin().await?;
    let identity = transaction_guard::current_http_identity(
        ctx,
        &mut tx,
        transaction_guard::HttpIdentityMode::Action,
    )
    .await?;
    identity.check_deadline(&ctx.app)?;
    // Native preparation is effect-free; its completed lock footprint precedes the current guard.
    let migration = source_url::prepare_migration(&mut tx, &ids).await?;
    let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    let plan = OperationPlan::new(current.route, current.route.name)?;
    let mut targets = Collection::new(ResourceKind::Skill)
        .targets(&mut tx, &ids)
        .await?;
    let live: std::collections::HashSet<_> = rows::skills(&mut tx, &ids)
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect();
    for id in &ids {
        let target = targets.remove(id).ok_or_else(ApiError::not_found)?;
        if !live.contains(id) {
            return Err(ApiError::not_found());
        }
        if crate::key_operations::native::modern(guard.principal().caller())
            && !plan.permits_binding(
                guard.principal().caller(),
                0,
                KeyTargetKind::Object(ObjectKind::Skill),
                &[target],
            )?
        {
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "the original Skill read no longer permits its source maintenance",
            ));
        }
        require(
            &mut tx,
            guard.principal(),
            ResourceKind::Skill,
            *id,
            Permission::SkillView,
        )
        .await?;
    }
    source_url::apply_migration(&mut tx, migration, guard.now().get(), || {
        guard.check_deadline(&current)
    })
    .await?;
    guard.check_deadline(&current)?;
    tx.commit().await.map_err(DbError::from)?;
    guard.check_deadline(&current)
}
