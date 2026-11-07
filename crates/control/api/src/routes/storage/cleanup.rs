//! Cleanup preflights retain exact sources and execute only choices the person actually read.

use crate::{
    middleware::{
        authentication::Via,
        requests::{ApiError, ApiResult},
        transaction_guard,
    },
    registry::{Ctx, Registry},
};
use orch_api_contract::{
    common::ErrorCode,
    storage::{self as api, CleanupChoice, CleanupItem},
    workspace::Job,
};
use orch_db::{DbError, queries::jobs};
use orch_engine::jobs::{
    cleanup::{self, Recipe, Target, Work},
    runner::initial_progress,
};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::collections::HashMap;

/// Binds one preview and one confirmation for every scoped cleanup.
pub fn mount(registry: &mut Registry) {
    registry
        .on_with_initial_operation(
            api::StorageCleanupPreflight,
            native::effects::initial::<preview::Preview>,
            native::effects::prepare::<preview::Preview>,
            preview::run,
        )
        .on(api::StorageCleanupCreate, start);
}

fn query(request: &api::CleanupRequest) -> api::StorageQuery {
    api::StorageQuery {
        scope: Some(request.scope.clone()),
        resource_kind: request.resource.map(|object| object.kind),
        resource_id: request.resource.map(|object| object.id),
        host: None,
        place: None,
        owner: None,
        kind: None,
        why: None,
        age: None,
        page: Default::default(),
    }
}

fn deliberate_stop(row: &mut api::KeptRecord) -> bool {
    let allowed = row.kind == api::StorageKind::Unit
        && row.can_clean
        && row.why == api::KeptWhy::InUse
        && (row.blocker.is_none() || row.blocker.as_deref() == Some("in_use"))
        && row.facts.as_ref().is_some_and(|facts| {
            facts.dependents_complete
                && facts.dependents.iter().all(|group| {
                    group.relation == orch_api_contract::resource_facts::DependentRelation::Serves
                })
        });
    if allowed {
        row.blocker = None;
    }
    allowed
}

mod preview;
use crate::key_operations::native;

/// The existing native choice/default planner consumes its prepared records and exact raw targets.
fn plan_items(
    ctx: &Ctx,
    rows: Vec<api::KeptRecord>,
    mut resolved: HashMap<String, Target>,
) -> (Vec<CleanupItem>, HashMap<String, Target>) {
    let mut targets = HashMap::new();
    let mut items = Vec::new();
    for mut row in rows {
        let stop_needs_choice = deliberate_stop(&mut row);
        let source = if row.can_clean && row.blocker.is_none() {
            resolved.remove(&row.id)
        } else {
            None
        };
        if source.is_none()
            && row.can_clean
            && row.blocker.is_none()
            && row.operator_command.is_none()
        {
            row.blocker = Some("cleanup_unavailable".into());
        }
        let mut choices = vec![CleanupChoice::Keep];
        let mut selected = CleanupChoice::Keep;
        if let Some(source) = source {
            let fate = if matches!(row.kind, api::StorageKind::Unit) {
                CleanupChoice::StopAndRemove
            } else if matches!(
                row.kind,
                api::StorageKind::EnvironmentFiles
                    | api::StorageKind::MountData
                    | api::StorageKind::Volume
                    | api::StorageKind::SkillVersion
                    | api::StorageKind::Dump
            ) {
                CleanupChoice::Purge
            } else {
                CleanupChoice::Remove
            };
            choices.push(fate);
            if !stop_needs_choice
                && (!matches!(fate, CleanupChoice::Purge)
                    || row.until.is_some_and(|until| until.get() <= ctx.now))
            {
                selected = fate;
            }
            targets.insert(row.id.clone(), source);
        }
        items.push(CleanupItem {
            record: row,
            choices,
            selected,
        });
    }
    (items, targets)
}

async fn start(mut ctx: Ctx, (): (), body: api::CleanupStart) -> ApiResult<Job> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("storage.cleanup:{}", body.plan_id))
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    if let Some(id)=sqlx::query_scalar::<_,Id>("SELECT id FROM jobs WHERE kind='storage_cleanup' AND plan->'recipe'->'plan'->>'id'=$1 AND created_by=$2").bind(body.plan_id.to_string()).bind(guard.principal().user()).fetch_optional(&mut *tx).await.map_err(DbError::from)? {
        let row=jobs::job(&mut tx,id).await?.ok_or_else(ApiError::not_found)?;
        return crate::routes::environments::jobs::view(&mut tx,&row).await;
    }
    let stored = jobs::forks::preflight(&mut tx, body.plan_id)
        .await?
        .filter(|row| {
            row.kind == cleanup::KIND
                && row.created_by == guard.principal().user()
                && row.expires_at > ctx.now
        })
        .ok_or_else(|| ApiError::new(ErrorCode::PreflightExpired, "read the cleanup plan again"))?;
    let mut plan: api::CleanupPlan =
        serde_json::from_value(stored.plan["preview"].clone()).map_err(ApiError::internal)?;
    let request: api::CleanupRequest =
        serde_json::from_value(stored.plan["request"].clone()).map_err(ApiError::internal)?;
    let targets: HashMap<String, Target> =
        serde_json::from_value(stored.plan["targets"].clone()).map_err(ApiError::internal)?;
    let (_, current, _) =
        super::records(&ctx, &mut tx, guard.principal(), &query(&request)).await?;
    let current: HashMap<_, _> = current
        .into_iter()
        .map(|mut row| {
            deliberate_stop(&mut row);
            (row.id.clone(), row)
        })
        .collect();
    let mut selections = HashMap::new();
    for selection in body.choices {
        if selections.insert(selection.id, selection.choice).is_some() {
            return Err(ApiError::invalid("choices", "an item is listed twice"));
        }
    }
    if selections
        .keys()
        .any(|id| !plan.items.iter().any(|item| item.record.id == *id))
    {
        return Err(ApiError::invalid("choices", "an item was not in this plan"));
    }
    let mut work = Vec::new();
    let mut erases = false;
    for item in &mut plan.items {
        let choice = selections
            .get(&item.record.id)
            .copied()
            .unwrap_or(item.selected);
        if !item.choices.contains(&choice) {
            return Err(ApiError::invalid("choices", "this fate was not offered"));
        }
        item.selected = choice;
        let now = current.get(&item.record.id);
        if choice != CleanupChoice::Keep
            && now.is_none_or(|row| {
                row.blocker.is_some()
                    || !row.can_clean
                    || row.resource != item.record.resource
                    || row.since != item.record.since
                    || row.until != item.record.until
            })
        {
            item.record.blocker = Some("changed".into());
        }
        erases |= matches!(choice, CleanupChoice::Purge | CleanupChoice::StopAndRemove)
            && item.record.blocker.is_none();
        work.push(Work {
            id: item.record.id.clone(),
            choice,
            target: if item.record.blocker.is_none() {
                targets.get(&item.record.id).cloned()
            } else {
                None
            },
        });
    }
    if erases && body.confirmation_name.as_deref() != Some(&plan.confirmation_name) {
        return Err(ApiError::invalid(
            "confirmation_name",
            "type the plan's name before erasing data",
        ));
    }
    plan.notify = body.notify;
    let identity = match &ctx.user()?.via {
        Via::Session { session_id } => json!({"session_id":session_id}),
        Via::Token { token_id, .. } => json!({"token_id":token_id}),
    };
    let row=jobs::insert(&mut tx,&jobs::NewJob {id:new_id(),kind:cleanup::KIND,node_id:None,environment_id:None,provider_id:None,created_by:Some(guard.principal().user()),plan:&json!({"recipe":Recipe{request:Some(request),plan,work},"identity":identity,"origin":orch_db::audit::current_origin(),"cleanup_results":[]}),progress:&serde_json::to_value(initial_progress(&cleanup::PHASES)).map_err(ApiError::internal)?},ctx.now).await?;
    sqlx::query("DELETE FROM preflights WHERE id=$1")
        .bind(stored.id)
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    let shown = crate::routes::environments::jobs::view(&mut tx, &row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}
