//! Cleanup preflights retain exact sources and execute only choices the person actually read.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::storage as api;

use orch_api_contract::storage::CleanupChoice;

use orch_api_contract::storage::CleanupItem;

use orch_db::queries::jobs;

use orch_engine::jobs::cleanup;

use orch_engine::jobs::cleanup::Target;

use orch_engine::jobs::cleanup::Work;

use std::collections::HashMap;

use crate::key_operations::native;
/// Binds one preview and one confirmation for every scoped cleanup.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(api::StorageCleanupCreate, creation::replay_binding())
        .on_with_initial_operation(
            api::StorageCleanupPreflight,
            native::effects::initial::<preview::Preview>,
            native::effects::prepare::<preview::Preview>,
            preview::run,
        )
        .on_with_initial_operation(
            api::StorageCleanupCreate,
            native::effects::initial::<creation::Create>,
            native::effects::prepare::<creation::Create>,
            creation::run,
        );
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

fn select_work(
    mut plan: api::CleanupPlan,
    current: HashMap<String, api::KeptRecord>,
    targets: &HashMap<String, Target>,
    body: &api::CleanupStart,
) -> ApiResult<(api::CleanupPlan, Vec<Work>)> {
    let mut selections = HashMap::new();
    for selection in &body.choices {
        if selections
            .insert(selection.id.clone(), selection.choice)
            .is_some()
        {
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
    Ok((plan, work))
}

#[path = "cleanup/creation.rs"]
pub mod creation;

#[path = "cleanup/preview.rs"]
pub mod preview;
