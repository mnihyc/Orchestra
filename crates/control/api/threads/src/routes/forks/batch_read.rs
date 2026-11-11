//! Scope precedes the native batch aggregation; no job or batch UUID is a scope root.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::registry::OperationPlan;

use crate::routes::access::page::NameBounds;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::fork_sources as sources;

use orch_api_contract::fork_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_authz::Caller;

use std::collections::BTreeMap;

use std::collections::BTreeSet;

use std::collections::HashMap;
const MACHINE: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Machine);

const NAMES: &[(ObjectKind, Source)] = &[
    (ObjectKind::Org, Source::NamesOrgs),
    (ObjectKind::Project, Source::NamesProjects),
    (ObjectKind::Topic, Source::NamesTopics),
    (ObjectKind::Thread, Source::NamesThreads),
];

pub(super) struct Batch;

pub(super) struct Selected {
    batch: Id,
    rows: Vec<JobRow>,
    visible: BTreeSet<Id>,
    paths: BTreeMap<Id, Vec<Id>>,
    names: NameBounds,
    visible_names: BTreeSet<Id>,
    machines: Option<HashMap<Id, String>>,
    waiting: BTreeMap<Id, Option<Id>>,
}

fn collection(
    source: Source,
    targets: impl IntoIterator<Item = (KeyTargetKind, KeyResolvedTarget)>,
) -> native::Facts<Source> {
    let mut groups = BTreeMap::<_, BTreeMap<_, _>>::new();
    for (family, target) in targets {
        groups
            .entry(family)
            .or_default()
            .insert(target.object, target);
    }
    native::Facts::collection(
        source,
        groups
            .into_iter()
            .map(|(family, targets)| (family, targets.into_values().collect()))
            .collect(),
    )
}

async fn rows(conn: &mut Conn, batch: Id) -> ApiResult<Vec<JobRow>> {
    let rows = orch_db::queries::jobs::forks::of_batch(conn, batch).await?;
    if rows.is_empty() {
        return Err(ApiError::not_found());
    }
    Ok(rows)
}

/// The native member gate, shared with the unbounded batch view below.
pub(super) async fn member(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    names: &NameBounds,
) -> ApiResult<Option<orch_api_contract::nodes::NodeRef>> {
    let may_view = matches!(
        nodes::check(
            conn,
            principal,
            thread,
            Permission::ThreadView,
            Act::Use,
            Policy::default()
        )
        .await,
        Ok(orch_access_contract::decisions::Decision::Allowed { .. })
            | Err(orch_authz::Error::Archived { .. })
    );
    if !may_view {
        return Ok(None);
    }
    Ok(names
        .refs(conn, principal, &[thread])
        .await?
        .remove(&thread))
}

/// The existing member projection and aggregate, with optional factual name bounds.
pub(super) async fn view(
    conn: &mut Conn,
    principal: &Principal,
    batch: Id,
    rows: &[JobRow],
    names: &NameBounds,
    machines: Option<&HashMap<Id, String>>,
) -> ApiResult<ForkBatch> {
    let mut jobs = Vec::new();
    let mut hidden = 0;
    for row in rows {
        let Some(id) = row.node_id else { continue };
        let Some(thread) = member(conn, principal, id, names).await? else {
            hidden += 1;
            continue;
        };
        jobs.push(ForkBatchJob {
            thread,
            job: job_view::view_with_machine_sources(conn, row, machines).await?,
        });
    }
    Ok(ForkBatch {
        batch_id: batch,
        jobs,
        percent: percent_of_batch(rows),
        hidden,
        counts: counts_of(rows),
    })
}

impl native::Selected for Batch {
    type Source = Source;
    type Query = ();
    type Value = Selected;
    type Reply = ForkBatch;
    const DECLARATION: &'static Declaration<Source> = &sources::BATCH_GET;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): &'a (),
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let batch = ctx.id("batch_id")?;
            let candidates = rows(conn, batch).await?;
            let limited = native::modern(caller);
            let plan = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            let mut selected = Vec::new();
            let mut subjects = Vec::new();
            let governors = if limited {
                governors(conn, &candidates).await?
            } else {
                HashMap::new()
            };
            for row in &candidates {
                if limited {
                    // A fork's job belongs to its existing source thread; a maker with no
                    // thread belongs to its actual environment. Never infer a created ID.
                    let (family, target) = governors
                        .get(&row.id)
                        .cloned()
                        .ok_or_else(ApiError::not_found)?;
                    if !native::permits(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &Source::BatchJobs,
                        family,
                        &target,
                    )? {
                        continue;
                    }
                    subjects.push((family, target));
                }
                selected.push(row.clone());
            }
            // The actual selector has now established even an empty collection.
            let mut facts = vec![collection(Source::BatchJobs, subjects)];
            let known = native::bind_known(ctx, Self::DECLARATION, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            may_read_batch(conn, &principal, &candidates).await?;
            let (value, additional) =
                prepare_view(ctx, conn, &principal, batch, selected, Self::DECLARATION).await?;
            facts.extend(additional);
            Ok((value, if limited { facts } else { Vec::new() }))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let rows = rows(conn, ctx.id("batch_id")?).await?;
            may_read_batch(conn, principal, &rows).await
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<ForkBatch>> {
        Box::pin(view(
            conn,
            principal,
            value.batch,
            &value.rows,
            &value.names,
            value.machines.as_ref(),
        ))
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        (): &'a (),
        old: &'a Selected,
        fresh: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            if !old.visible.is_subset(&fresh.visible)
                || !old.names.nodes.is_subset(&fresh.names.nodes)
                || !old.visible_names.is_subset(&fresh.visible_names)
            {
                return Ok(false);
            }
            for row in &old.rows {
                let Some(new) = fresh.rows.iter().find(|new| new.id == row.id) else {
                    return Ok(false);
                };
                if row.kind != new.kind
                    || row.node_id != new.node_id
                    || row.environment_id != new.environment_id
                    || row.provider_id != new.provider_id
                    || row.created_by != new.created_by
                    || row.created_at != new.created_at
                    || row.batch_id != new.batch_id
                    || row
                        .node_id
                        .is_some_and(|id| old.paths.get(&id) != fresh.paths.get(&id))
                    || old.waiting.get(&row.id) != fresh.waiting.get(&row.id)
                {
                    return Ok(false);
                }
            }
            Ok(true)
        })
    }
}

/// One native field/name preparer after the consuming operation's actual primary Read.
pub(super) async fn prepare_view(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    batch: Id,
    selected: Vec<JobRow>,
    declaration: &'static Declaration<Source>,
) -> ApiResult<(Selected, Vec<native::Facts<Source>>)> {
    let caller = principal.caller();
    let limited = native::modern(caller);
    let plan = OperationPlan::new(ctx.route, declaration.operation)?;
    let mut facts = Vec::new();
    let mut names = NameBounds::default();
    let mut paths = BTreeMap::new();
    {
        let ids = selected
            .iter()
            .filter_map(|row| row.node_id)
            .collect::<Vec<_>>();
        let graph = orch_authz::engine::Graph::load(conn, &ids)
            .await
            .map_err(ApiError::internal)?;
        for id in &ids {
            if let Some(node) = graph.get(*id) {
                paths.insert(*id, node.path.clone());
            }
        }
        let all = paths.values().flatten().copied().chain(ids).collect();
        let (name_facts, allowed) =
            native::node_name_facts(&plan, declaration, caller, &graph, &all, NAMES)?;
        facts.extend(name_facts);
        names.limited = limited;
        names.nodes = allowed;
    }
    let name_refs = names
        .refs(
            conn,
            principal,
            &names.nodes.iter().copied().collect::<Vec<_>>(),
        )
        .await?;
    let visible_names = name_refs
        .into_iter()
        .filter_map(|(id, named)| (!named.redacted && named.name.is_some()).then_some(id))
        .collect();
    let mut visible = BTreeSet::new();
    for row in &selected {
        if let Some(id) = row.node_id
            && member(conn, principal, id, &names).await?.is_some()
        {
            visible.insert(row.id);
        }
    }
    let mut waiting = BTreeMap::new();
    let machines;
    {
        let shown = selected
            .iter()
            .filter(|row| visible.contains(&row.id))
            .cloned()
            .collect::<Vec<_>>();
        let mut labels = if limited {
            job_view::machine_name_sources(conn, &shown).await?
        } else {
            HashMap::new()
        };
        let machine_ids = shown
            .iter()
            .filter_map(job_view::waiting_machine)
            .collect::<Vec<_>>();
        let identities = targets::machine_identities(conn, &machine_ids).await?;
        let mut kept = Vec::new();
        let mut allowed = BTreeSet::new();
        for (id, target) in identities {
            if !limited
                || native::permits_usage(
                    &plan,
                    declaration,
                    caller,
                    &Source::WaitingMachines,
                    FacetUsage::ResponseContent,
                    MACHINE,
                    &target,
                )?
            {
                allowed.insert(id);
                kept.push((MACHINE, target));
            }
        }
        labels.retain(|id, _| allowed.contains(id));
        facts.push(collection(Source::WaitingMachines, kept));
        // An undisclosed waiting label is not a source of the rendered body.
        for row in &shown {
            let disclosed =
                job_view::waiting_machine(row).filter(|id| !limited || labels.contains_key(id));
            waiting.insert(row.id, disclosed);
        }
        machines = limited.then_some(labels);
    }
    Ok((
        Selected {
            batch,
            rows: selected,
            visible,
            paths,
            names,
            visible_names,
            machines,
            waiting,
        },
        facts,
    ))
}

/// Actual stored governors for an explicit collection, before any public formatting.
pub(super) async fn subjects(
    conn: &mut Conn,
    source: Source,
    rows: &[JobRow],
) -> ApiResult<native::Facts<Source>> {
    Ok(collection(
        source,
        governors(conn, rows).await?.into_values(),
    ))
}

async fn governors(
    conn: &mut Conn,
    rows: &[JobRow],
) -> ApiResult<HashMap<Id, (KeyTargetKind, KeyResolvedTarget)>> {
    let nodes = rows
        .iter()
        .filter_map(|row| row.node_id)
        .collect::<Vec<_>>();
    let graph = orch_authz::engine::Graph::load(conn, &nodes)
        .await
        .map_err(ApiError::internal)?;
    let mut environments = BTreeMap::<Id, KeyResolvedTarget>::new();
    let mut subjects = HashMap::new();
    for row in rows {
        let object = match (row.node_id, row.environment_id) {
            (Some(id), _) => ObjectRef {
                kind: ObjectKind::Thread,
                id,
            },
            (None, Some(id)) => ObjectRef {
                kind: ObjectKind::Environment,
                id,
            },
            (None, None) => return Err(ApiError::internal("a batch job has no native subject")),
        };
        let target = if object.kind == ObjectKind::Thread {
            orch_engine::operation_scope::targets::from_node_identity(&graph, object.id)
                .ok_or_else(ApiError::not_found)?
        } else if let Some(target) = environments.get(&object.id) {
            target.clone()
        } else {
            let target = targets::resource_identity(conn, object).await?;
            environments.insert(object.id, target.clone());
            target
        };
        subjects.insert(row.id, (KeyTargetKind::Object(object.kind), target));
    }
    Ok(subjects)
}
