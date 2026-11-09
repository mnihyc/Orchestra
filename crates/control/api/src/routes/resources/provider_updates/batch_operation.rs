//! The two existing batch producers resolve their actual collections once per guarded
//! checkpoint. Private operands never grant authority or escape into a deferred recipe.

use super::*;
use crate::key_operations::{admission::Prepared, targets};
use crate::middleware::authorization;
use crate::registry::{OperationFuture, OperationPlan};
use crate::routes::access::{
    operations::{NameRoles, projection_names},
    page::{NameBounds, NameSources},
};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::{common::ErrorCode, operations::TargetResolver};
use orch_authz::{Caller, CapturedOperation};
use orch_db::queries::jobs;
use orch_engine::updates::{Draft, Recipe};
use orch_settings::Place;
use std::collections::{BTreeSet, HashMap};

pub(super) const ORIGINS: &str = "provider_update_origins";
const PROVIDER: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Provider);
const NAMES: NameRoles = NameRoles {
    groups: None,
    nodes: [
        (ObjectKind::Org, "batch_names_orgs"),
        (ObjectKind::Project, "batch_names_projects"),
        (ObjectKind::Topic, "batch_names_topics"),
        (ObjectKind::Thread, "batch_names_threads"),
    ],
    containers: None,
};

pub(super) struct Preview {
    pub id: Id,
    pub drafts: Vec<Draft>,
    pub skipped: Vec<api::ProviderUpdateSkipReceipt>,
    pub full: Option<api::ProviderRuntimeUpdateBatchPreflightReply>,
}

pub(super) struct Confirmation {
    pub drafts: Vec<Draft>,
    pub origins: Vec<CapturedOperation>,
}

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the key does not allow this provider batch",
    )
}

fn slot(plan: &Prepared, role: &'static str) -> ApiResult<usize> {
    let mut found = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter(|(_, facet)| facet.target == TargetResolver::ResolvedCollection(role));
    let (index, _) = found
        .next()
        .ok_or_else(|| ApiError::internal("the provider batch collection is not declared"))?;
    if found.next().is_some() {
        return Err(ApiError::internal(
            "the provider batch collection is ambiguous",
        ));
    }
    Ok(index)
}

async fn object(conn: &mut Conn, kind: ObjectKind, id: Id) -> ApiResult<KeyResolvedTarget> {
    targets::page_object(conn, ObjectRef { kind, id }).await
}

fn bind(
    plan: &mut Prepared,
    role: &'static str,
    kind: ObjectKind,
    targets: &[KeyResolvedTarget],
) -> ApiResult<()> {
    let index = slot(plan, role)?;
    if targets.is_empty() {
        plan.bind_empty(index)
    } else {
        plan.bind(index, KeyTargetKind::Object(kind), targets)
    }
}

async fn read_principal(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    plan: &Prepared,
    target: &KeyResolvedTarget,
) -> ApiResult<Option<Principal>> {
    let metadata = &plan.variant().facets[slot(plan, "batch_providers")?];
    let facts = orch_engine::operation_scope::facets(
        ctx.route.name,
        metadata,
        PROVIDER,
        std::slice::from_ref(target),
    )
    .map_err(crate::key_operations::admission::binding_error)?;
    let Ok(admitted) = caller.admit_facets(&facts) else {
        return Ok(None);
    };
    let principal =
        authorization::load_principal(conn, admitted.into_caller(), Time::new(ctx.app.clock.now()))
            .await?;
    Ok(Some(
        crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await?,
    ))
}

async fn full_sources(
    conn: &mut Conn,
    caller: &Caller,
    plan: &mut Prepared,
    providers: &[Id],
    drafts: &[Draft],
    names: NameSources,
) -> ApiResult<NameBounds> {
    let mut provider_targets = Vec::new();
    for provider in providers {
        provider_targets.push(object(conn, ObjectKind::Provider, *provider).await?);
    }
    bind(
        plan,
        "batch_providers",
        ObjectKind::Provider,
        &provider_targets,
    )?;
    let mut hosts = BTreeSet::new();
    let mut templates = BTreeSet::new();
    for draft in drafts {
        // These are names the ordinary draft embeds in required strings, not arbitrary
        // stored words to scrub. A missing Read selects the entire typed receipt instead.
        if draft.recipe.host.is_some() || draft.blockers.iter().any(|b| b.code == "no_host") {
            let old = orch_db::queries::environments::hosting::runtime(conn, draft.recipe.old)
                .await?
                .ok_or_else(ApiError::not_found)?;
            hosts.insert(old.host_id);
            hosts.extend(draft.recipe.host);
        }
        if draft
            .updates
            .iter()
            .any(|update| update.what == orch_api_contract::environment::UpdateWhat::Template)
        {
            templates.insert(draft.recipe.template);
        }
    }
    for (role, kind, ids) in [
        ("batch_hosts", ObjectKind::Host, hosts),
        ("batch_templates", ObjectKind::Template, templates),
    ] {
        let mut values = Vec::new();
        for id in ids {
            values.push(object(conn, kind, id).await?);
        }
        bind(plan, role, kind, &values)?;
    }
    // Confirmation projects the newly queued row, before any active command or its live
    // machine name exists. Polling later has its own ordinary Job read producer.
    bind(plan, "batch_machines", ObjectKind::Machine, &[])?;
    projection_names(conn, caller, plan, names, &NAMES).await
}

async fn bounded_plans(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    bounds: &NameBounds,
    plans: &mut [ProviderUpdatePlan],
    drafts: &[Draft],
) -> ApiResult<()> {
    let ids: Vec<Id> = plans
        .iter()
        .flat_map(|plan| {
            plan.threads
                .iter()
                .filter_map(|thread| thread.thread.as_ref().map(|thread| thread.id))
                .chain(
                    plan.network
                        .sources
                        .iter()
                        .filter_map(|source| source.scope.as_ref().map(|scope| scope.id)),
                )
        })
        .collect();
    let refs = bounds.refs(conn, principal, &ids).await?;
    for (plan, draft) in plans.iter_mut().zip(drafts) {
        for thread in &mut plan.threads {
            thread.thread = thread
                .thread
                .as_ref()
                .and_then(|old| refs.get(&old.id).filter(|row| !row.redacted).cloned());
        }
        for source in &mut plan.network.sources {
            source.scope = source
                .scope
                .as_ref()
                .and_then(|old| refs.get(&old.id).filter(|row| !row.redacted).cloned());
        }
        plan.notify = Some(
            plans::audience_bounded(ctx, conn, principal, &draft.recipe.threads, Some(bounds))
                .await?,
        );
    }
    Ok(())
}

pub(super) fn preview<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: api::ProviderRuntimeUpdateBatchPreflightBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let mut plan = Prepared::new(ctx.route, ctx.route.name)?;
        plan.shared()
            .compatible(caller)
            .map_err(crate::key_operations::admission::binding_error)?;
        let mut ids = body.provider_ids;
        ids.sort_unstable();
        ids.dedup();
        let mut managed = Vec::new();
        let mut visible = Vec::new();
        let mut drafts = Vec::new();
        let mut skipped = Vec::new();
        let mut references = HashMap::new();
        let mut full = true;
        for id in ids {
            let target = match object(conn, ObjectKind::Provider, id).await {
                Ok(target) => target,
                // A partial preview does not turn an absent or unavailable member into
                // a differently observable target outside the key's actual collection.
                Err(error) if error.code() == ErrorCode::NotFound => continue,
                Err(error) => return Err(error),
            };
            let may_manage = plan.permits_binding(
                caller,
                slot(&plan, "affected_providers")?,
                PROVIDER,
                std::slice::from_ref(&target),
            )?;
            let reader = read_principal(ctx, conn, caller, &plan, &target).await?;
            let principal = if may_manage {
                let mut candidate = Prepared::new(ctx.route, ctx.route.name)?;
                candidate.bind(
                    slot(&candidate, "affected_providers")?,
                    PROVIDER,
                    std::slice::from_ref(&target),
                )?;
                candidate.preparation_principal(ctx, conn, caller).await?
            } else if let Some(reader) = reader.as_ref() {
                reader.clone()
            } else {
                continue;
            };
            let Some(reference) = super::reference(conn, &principal, id).await? else {
                continue;
            };
            visible.push(id);
            if reader.is_none() {
                full = false;
            }
            references.insert(id, reference);
            if !may_manage {
                skipped.push(api::ProviderUpdateSkipReceipt {
                    provider_id: id,
                    reason: api::ProviderUpdateSkipReason::NoPermission,
                });
                continue;
            }
            let draft = match ctx
                .app
                .engine
                .draft_update(
                    conn,
                    &principal,
                    Target::Provider(id),
                    UpdatePreflightReason::Update,
                    None,
                )
                .await
            {
                Ok(draft) => draft,
                Err(UpdateError::Denied) => {
                    skipped.push(api::ProviderUpdateSkipReceipt {
                        provider_id: id,
                        reason: api::ProviderUpdateSkipReason::NoPermission,
                    });
                    continue;
                }
                Err(UpdateError::Blocked(_)) | Err(UpdateError::NotFound) => {
                    managed.push(target);
                    skipped.push(api::ProviderUpdateSkipReceipt {
                        provider_id: id,
                        reason: api::ProviderUpdateSkipReason::NotManaged,
                    });
                    continue;
                }
                Err(error) => return Err(plans::refused(error)),
            };
            managed.push(target);
            if draft.updates.is_empty() {
                skipped.push(api::ProviderUpdateSkipReceipt {
                    provider_id: id,
                    reason: api::ProviderUpdateSkipReason::UpToDate,
                });
                continue;
            }
            drafts.push(draft);
        }
        bind(
            &mut plan,
            "affected_providers",
            ObjectKind::Provider,
            &managed,
        )?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        let id = new_id();
        let mut shown = Vec::new();
        let mut names = NameSources::default();
        for draft in &drafts {
            names.nodes.extend(draft.recipe.threads.iter().copied());
            if full {
                let value = plans::provider_plan(ctx, conn, &principal, id, draft).await?;
                names.nodes.extend(
                    value
                        .network
                        .sources
                        .iter()
                        .filter_map(|source| source.scope.as_ref().map(|scope| scope.id)),
                );
                shown.push(value);
            }
        }
        let bounds = full_sources(conn, caller, &mut plan, &visible, &drafts, names).await?;
        full &= plan.shared().response_coverage(caller) == Some(true);
        let full = if full {
            let reader = plan
                .shared()
                .admit(caller, true)
                .map_err(crate::key_operations::admission::binding_error)?;
            let reader =
                authorization::load_principal(conn, reader, Time::new(ctx.app.clock.now())).await?;
            let reader = crate::sign_in::second_step::hold(conn, ctx.user()?, reader).await?;
            bounded_plans(ctx, conn, &reader, &bounds, &mut shown, &drafts).await?;
            let affected: Vec<_> = drafts
                .iter()
                .filter(|draft| draft.blockers.is_empty())
                .flat_map(|draft| draft.recipe.threads.iter().copied())
                .collect();
            let parallel = orch_settings::get::<i64>(
                conn,
                Place::System,
                orch_settings_contract::registry::keys::SWITCH_BATCH_PARALLEL,
            )
            .await
            .map_err(ApiError::internal)?
            .unwrap_or(updates::DEFAULT_BATCH_PARALLEL);
            let estimate = orch_engine::jobs::estimates::together(
                &shown
                    .iter()
                    .map(|plan| plan.estimate.clone())
                    .collect::<Vec<_>>(),
                parallel,
            );
            Some(api::ProviderRuntimeUpdateBatchPreflightReply {
                notify: Some(
                    plans::audience_bounded(ctx, conn, &reader, &affected, Some(&bounds)).await?,
                ),
                preflight_id: id,
                expires_at: Time::new(updates::expires(ctx.now)),
                plans: shown,
                skipped: skipped
                    .iter()
                    .filter_map(|row| {
                        references.get(&row.provider_id).cloned().map(|provider| {
                            api::ProviderUpdateSkipped {
                                provider,
                                reason: row.reason,
                            }
                        })
                    })
                    .collect(),
                executable: drafts
                    .iter()
                    .filter(|draft| draft.blockers.is_empty())
                    .count() as i64,
                options: updates::options(conn, Place::System, None)
                    .await
                    .map_err(plans::refused)?,
                estimate,
            })
        } else {
            None
        };
        Ok(plan.with_data(Preview {
            id,
            drafts,
            skipped,
            full,
        }))
    })
}

fn origins(row: &orch_db::queries::jobs::forks::PreflightRow) -> ApiResult<Vec<CapturedOperation>> {
    let values = row.plan.get(ORIGINS).ok_or_else(|| {
        ApiError::new(
            ErrorCode::AuthzDenied,
            "this provider batch must be reviewed again",
        )
    })?;
    let origins: Vec<CapturedOperation> =
        serde_json::from_value(values.clone()).map_err(ApiError::internal)?;
    if origins.is_empty()
        || origins.iter().any(|origin| {
            origin.user() != row.created_by
                || origin.operation() != "ProviderRuntimeUpdateBatchPreflight"
        })
    {
        return Err(refused());
    }
    Ok(origins)
}

pub(super) fn confirm<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    _: orch_api_contract::forks::UpdateConfirmQuery,
    body: ForkCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let row = jobs::changes::lock_plan(conn, body.preflight_id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if row.kind != "update_batch"
            || row.node_id.is_some()
            || row.created_by != caller.user()
            || row.plan["providers"] != true
        {
            return Err(ApiError::not_found());
        }
        if row.expires_at <= ctx.now {
            return Err(plans::refused(UpdateError::Expired));
        }
        let origins = origins(&row)?;
        let recipes: Vec<Recipe> =
            serde_json::from_value(row.plan["recipes"].clone()).map_err(ApiError::internal)?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        let mut targets = Vec::new();
        let mut ids = Vec::new();
        for recipe in &recipes {
            let Target::Provider(provider) = recipe.target else {
                return Err(refused());
            };
            if recipe.by != caller.user() {
                return Err(refused());
            }
            ids.push(provider);
            targets.push(object(conn, ObjectKind::Provider, provider).await?);
        }
        bind(
            &mut plan,
            "affected_providers",
            ObjectKind::Provider,
            &targets,
        )?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        for origin in &origins {
            if ids.is_empty() {
                let current = origin
                    .restore(conn, Time::new(ctx.app.clock.now()))
                    .await
                    .map_err(ApiError::internal)?
                    .ok_or_else(refused)?;
                let mut empty =
                    orch_engine::operation_scope::Prepared::for_selection(origin.operation())
                        .map_err(crate::key_operations::admission::binding_error)?;
                let index = empty
                    .variant()
                    .facets
                    .iter()
                    .position(|facet| {
                        facet.target == TargetResolver::ResolvedCollection("affected_providers")
                    })
                    .ok_or_else(refused)?;
                empty
                    .bind_empty(index)
                    .map_err(crate::key_operations::admission::binding_error)?;
                empty
                    .admit(&current, false)
                    .map_err(crate::key_operations::admission::binding_error)?;
            }
            for recipe in &recipes {
                let provider = recipe.target.id();
                let restored = orch_engine::jobs::recreate::batch_authority::restore_origin(
                    conn,
                    origin,
                    provider,
                    Time::new(ctx.app.clock.now()),
                )
                .await
                .map_err(ApiError::engine)?
                .ok_or_else(refused)?;
                let old = orch_db::queries::environments::hosting::runtime(conn, recipe.old)
                    .await?
                    .filter(|old| old.provider_id == Some(provider))
                    .ok_or_else(|| {
                        ApiError::new(
                            ErrorCode::ConflictStaleRevision,
                            "the reviewed provider runtime changed",
                        )
                    })?;
                if !updates::template_usable(conn, &restored, recipe.template, old.template_id)
                    .await
                    .map_err(ApiError::engine)?
                {
                    return Err(refused());
                }
            }
        }
        let drafts = plans::batch_drafts(ctx, conn, &principal, None, true, &body).await?;
        let mut names = NameSources::default();
        for draft in &drafts {
            names.nodes.extend(draft.recipe.threads.iter().copied());
        }
        full_sources(conn, caller, &mut plan, &ids, &[], names).await?;
        Ok(plan.with_data(Confirmation { drafts, origins }))
    })
}
