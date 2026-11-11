//! Current same-operation reading of the stored native review, without rerunning its writer or IO.

use super::*;

use crate::key_operations::native::reply::Prepared;

use crate::key_operations::native::reply::Projection;

use crate::routes::access::page::NameBounds;

use crate::routes::nodes::view::ReferenceDisclosure;

use std::collections::BTreeSet;
pub(super) struct Reply;

pub(super) struct Subject;

pub(in crate::routes::fleet) struct Value {
    native: environments::PreparedInventory,
    names: BTreeSet<Id>,
    templates: BTreeSet<Id>,
    disclosure: ReferenceDisclosure,
    networks: Vec<environments::network::PreparedNetwork>,
}

impl Projection for Reply {
    type Source = Source;
    type Input = Stored;
    type Subject = Subject;
    type Value = Value;
    type Reply = api::EnvironmentUpdateBatchPlan;
    const READ_SOURCE: Source = Source::Content;

    fn declaration(input: &Stored) -> &'static Declaration<Source> {
        if input.planned.selection.node.is_some() {
            &NODE_PREFLIGHT
        } else {
            &GLOBAL_PREFLIGHT
        }
    }

    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Stored,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let kept = orch_db::queries::jobs::forks::preflight(conn, input.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if kept.kind != "update_batch" || kept.node_id != input.planned.selection.node {
                return Err(ApiError::not_found());
            }
            for source in input.sources.values() {
                if !source.matches(conn).await.map_err(source_error)? {
                    return Err(ApiError::not_found());
                }
            }
            Ok((
                Subject,
                vec![
                    environment_facts(conn, Source::Environments, &input.planned.selection.managed)
                        .await?,
                    environment_facts(conn, Source::Content, &input.planned.selection.visible)
                        .await?,
                ],
            ))
        })
    }

    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Stored,
        _: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            for id in &input.planned.selection.visible {
                environments::reading::decide(conn, principal, *id).await?;
            }
            Ok(())
        })
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Stored,
        _: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            prepare_context(
                ctx,
                conn,
                principal,
                if input.planned.selection.node.is_some() {
                    &NODE_PREFLIGHT
                } else {
                    &GLOBAL_PREFLIGHT
                },
                &input.planned.selection.managed,
                &input.planned.selection.visible,
                &input.planned.drafts,
            )
            .await
        })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Stored,
        prepared: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<api::EnvironmentUpdateBatchPlan>> {
        Box::pin(async move {
            let environments =
                environments::show_inventory_prepared(ctx, conn, principal, &prepared.value.native)
                    .await?
                    .into_iter()
                    .map(|environment| (environment.base.id, environment.base))
                    .collect::<BTreeMap<_, _>>();
            let bounds = NameBounds {
                limited: native::modern(principal.caller()),
                nodes: prepared.value.names.clone(),
                ..Default::default()
            };
            let mut plans_shown = Vec::new();
            let mut threads = Vec::new();
            for draft in &input.planned.drafts {
                let environment = environments
                    .get(&draft.recipe.target.id())
                    .ok_or_else(ApiError::not_found)?;
                let mut plan = plans::environment_plan_bounded(
                    ctx,
                    conn,
                    principal,
                    input.id,
                    draft,
                    Some(plans::PlanBounds {
                        environment,
                        names: &bounds,
                        template_name: !native::modern(principal.caller())
                            || prepared.value.templates.contains(&draft.recipe.template),
                    }),
                )
                .await?;
                plan.expires_at = Time::new(updates::expires(input.at));
                plans_shown.push(plan);
                if draft.blockers.is_empty() {
                    threads.extend(draft.recipe.threads.iter().copied());
                }
            }
            let skipped = input
                .planned
                .skipped
                .iter()
                .map(|(id, reason)| {
                    Ok(api::EnvironmentUpdateSkipped {
                        environment: environments
                            .get(id)
                            .ok_or_else(ApiError::not_found)?
                            .clone(),
                        reason: *reason,
                    })
                })
                .collect::<ApiResult<Vec<_>>>()?;
            let estimate = orch_engine::jobs::estimates::together(
                &plans_shown
                    .iter()
                    .map(|plan| plan.estimate.clone())
                    .collect::<Vec<_>>(),
                input.planned.parallel,
            );
            Ok(api::EnvironmentUpdateBatchPlan {
                notify: Some(
                    plans::audience_bounded(ctx, conn, principal, &threads, Some(&bounds)).await?,
                ),
                preflight_id: input.id,
                expires_at: Time::new(updates::expires(input.at)),
                plans: plans_shown,
                skipped,
                executable: input.executable(),
                options: input.planned.options.clone(),
                estimate,
            })
        })
    }

    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Stored,
        opened: &'a Prepared<Self>,
        fresh: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.value.native.preserves_sources(&fresh.value.native)
                && opened.value.names.is_subset(&fresh.value.names)
                && opened.value.templates.is_subset(&fresh.value.templates)
                && opened
                    .value
                    .disclosure
                    .names
                    .is_subset(&fresh.value.disclosure.names)
                && opened
                    .value
                    .disclosure
                    .displays
                    .is_subset(&fresh.value.disclosure.displays)
                && opened.value.networks.len() == fresh.value.networks.len()
                && opened
                    .value
                    .networks
                    .iter()
                    .zip(&fresh.value.networks)
                    .all(|(old, fresh)| old.preserves_sources(fresh)))
        })
    }
}

/// Native environment/address/context selection shared by preview and queued-result readback.
pub(in crate::routes::fleet) async fn prepare_context(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    declaration: &'static Declaration<Source>,
    managed: &[Id],
    visible: &[Id],
    drafts: &[Draft],
) -> ApiResult<(Value, Vec<native::Facts<Source>>)> {
    let rows = orch_db::queries::environments::routes::environments(conn, visible).await?;
    if rows.len() != visible.len() {
        return Err(ApiError::not_found());
    }
    let mut native = environments::prepare_shown_rows(conn, principal, rows).await?;
    let known = native::bind_known(
        ctx,
        declaration,
        &[
            environment_facts(conn, Source::Environments, managed).await?,
            environment_facts(conn, Source::Content, visible).await?,
        ],
    )?;
    let mut wanted = orch_authz::resources::address_name_sources(conn, visible)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut networks = Vec::new();
    for draft in drafts {
        wanted.extend(draft.recipe.threads.iter().copied());
        let network = environments::network::prepare_planned(conn, &draft.recipe).await?;
        wanted.extend(network.name_sources());
        networks.push(network);
    }
    let selected = super::names::prepare(conn, principal, &known, declaration, wanted).await?;
    let names = selected.bounds.nodes;
    let disclosure = selected.disclosure;
    let mut groups = selected.groups;
    let mut templates = BTreeSet::new();
    for draft in drafts {
        if !draft
            .updates
            .iter()
            .any(|update| update.what == api::UpdateWhat::Template)
            || templates.contains(&draft.recipe.template)
        {
            continue;
        }
        let target = targets::resource_identity(
            conn,
            ObjectRef {
                kind: ObjectKind::Template,
                id: draft.recipe.template,
            },
        )
        .await?;
        let family = KeyTargetKind::Object(ObjectKind::Template);
        if native::permits_usage(
            &known,
            declaration,
            principal.caller(),
            &Source::Context,
            FacetUsage::ResponseContent,
            family,
            &target,
        )? {
            templates.insert(draft.recipe.template);
            groups.entry(family).or_insert_with(Vec::new).push(target);
        }
    }
    native.bound_names(NameBounds {
        limited: native::modern(principal.caller()),
        nodes: names.clone(),
        ..Default::default()
    });
    Ok((
        Value {
            native,
            names,
            templates,
            disclosure,
            networks,
        },
        vec![native::Facts::collection(
            Source::Context,
            groups.into_iter().collect(),
        )],
    ))
}

pub(in crate::routes::fleet) async fn references(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Value,
) -> ApiResult<BTreeMap<Id, api::EnvironmentRef>> {
    Ok(
        environments::show_inventory_prepared(ctx, conn, principal, &value.native)
            .await?
            .into_iter()
            .map(|environment| (environment.base.id, environment.base))
            .collect(),
    )
}

pub(in crate::routes::fleet) fn preserves(old: &Value, fresh: &Value) -> bool {
    old.native.preserves_sources(&fresh.native)
        && old.names.is_subset(&fresh.names)
        && old.templates.is_subset(&fresh.templates)
        && old.disclosure.names.is_subset(&fresh.disclosure.names)
        && old
            .disclosure
            .displays
            .is_subset(&fresh.disclosure.displays)
        && old.networks.len() == fresh.networks.len()
        && old
            .networks
            .iter()
            .zip(&fresh.networks)
            .all(|(old, fresh)| old.preserves_sources(fresh))
}
