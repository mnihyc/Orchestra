//! One native binding-option selection, retained across its pure formatter and publication cut.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::registry::OperationPlan;

use crate::routes::access::page::NameBounds;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::binding_sources as sources;

use orch_api_contract::binding_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::engine::Graph;

use std::collections::BTreeSet;
pub(super) struct Options;

pub(super) struct Selected {
    pub context: prepared::Context,
    pub providers: ProviderCandidates,
    pub inputs: ProviderInputs,
    pub environments: EnvironmentCandidates,
    pub environment_rows: Vec<environment_queries::EnvironmentFull>,
    pub environment_hosts: Vec<(Id, Id)>,
    pub hosts: Vec<orch_engine::places::HostPlace>,
    pub machines: Vec<orch_db::queries::resources::MachineRow>,
    pub projection: ChoiceProjection,
    pub grouping: crate::routes::resources::grouping::Prepared,
    pub can_create: BindingCanCreate,
    pub query: NodeBindingOptionsGetQuery,
    pub names: BTreeSet<Id>,
    pub visible_names: BTreeSet<Id>,
}

const NAMES: &[(ObjectKind, Source)] = &[
    (ObjectKind::Org, Source::OptionsOrgs),
    (ObjectKind::Project, Source::OptionsProjects),
    (ObjectKind::Topic, Source::OptionsTopics),
    (ObjectKind::Thread, Source::OptionsThreads),
];

async fn objects(
    conn: &mut Conn,
    rows: impl IntoIterator<Item = ObjectRef>,
) -> ApiResult<Vec<(KeyTargetKind, KeyResolvedTarget)>> {
    let mut found = Vec::new();
    for object in rows {
        let target = if object.kind == ObjectKind::Machine {
            targets::machine_identities(conn, &[object.id])
                .await?
                .remove(&object.id)
                .ok_or_else(ApiError::not_found)?
        } else {
            targets::resource_identity(conn, object).await?
        };
        found.push((KeyTargetKind::Object(object.kind), target));
    }
    Ok(found)
}

impl native::Selected for Options {
    type Source = Source;
    type Query = NodeBindingOptionsGetQuery;
    type Value = Selected;
    type Reply = BindingOptions;
    const DECLARATION: &'static Declaration<Source> = &sources::OPTIONS;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a NodeBindingOptionsGetQuery,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        let ctx = ctx.clone();
        let caller = caller.clone();
        let query = query.clone();
        Box::pin(async move {
            let ctx = &ctx;
            let caller = &caller;
            let query = &query;
            let modern = native::modern(caller);
            let root = targets::node(conn, ctx.id("id")?).await?;
            if let KeyTargetKind::Object(kind) = root.object.kind {
                prepared::permission(kind.as_str())?;
            }
            let mut facts = vec![native::Facts::one(
                Source::OptionsNode,
                root.object.kind,
                root,
            )];
            let known = native::bind_known(ctx, Self::DECLARATION, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let context = prepared::context(ctx, conn, &principal, query).await?;
            facts.push(scope::collection(
                Source::OptionsContext,
                objects(conn, context.sources.iter().copied()).await?,
            ));
            let known = native::bind_known(ctx, Self::DECLARATION, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let bounds = scope::Bounds::new(ctx, caller)?;
            let bounded = modern.then_some(&bounds as &dyn choice_window::Bounds);
            let providers = provider_candidates(conn, &principal, context.node.id, bounded).await?;
            let inputs = provider_inputs(conn, &providers).await?;
            let environments = environment_candidates(
                conn,
                &principal,
                context.node.id,
                Permission::EnvironmentUse,
                bounded,
            )
            .await?;
            let environment_rows =
                environment_queries::environments(conn, &environments.ids).await?;
            let provider_resources = providers
                .seen
                .items
                .iter()
                .map(|row| &row.resource)
                .chain(providers.agents_seen.items.iter().map(|row| &row.resource))
                .collect::<Vec<_>>();
            let mut choice_facts = scope::resource_facts(conn, provider_resources).await?;
            let environment_objects = environments
                .ids
                .iter()
                .map(|&id| ObjectRef {
                    kind: ObjectKind::Environment,
                    id,
                })
                .collect::<Vec<_>>();
            choice_facts.extend(objects(conn, environment_objects).await?);
            let mut runtime = BTreeSet::new();
            if let Some((environment, _)) = &context.judged {
                for provider in inputs.providers.values() {
                    for (agent, descriptor) in &inputs.agents {
                        if descriptor.is_some()
                            && agent_offered(agent, descriptor.as_ref(), provider)
                        {
                            if let Some(machine) = orch_engine::leases::machine_of(provider) {
                                runtime.insert(machine);
                            }
                            if let Some(machine) = agent.machine_id {
                                runtime.insert(machine);
                            }
                            if let Some(machine) = environment.machine_id {
                                runtime.insert(machine);
                            }
                        }
                    }
                }
            }
            let runtime_objects = runtime
                .iter()
                .map(|&id| ObjectRef {
                    kind: ObjectKind::Machine,
                    id,
                })
                .collect::<Vec<_>>();
            choice_facts.extend(objects(conn, runtime_objects).await?);
            let can_create =
                can_create_in(conn, &principal, context.node.id, context.node.id).await?;
            let mut hosts = Vec::new();
            if can_create.allowed {
                let policies = Policies::everywhere(ResourcePolicy {
                    owners_name_outsiders: false,
                    shared_subscriptions: None,
                });
                let chosen = choice_window::select(
                    conn,
                    &choice_window::Usable {
                        principal: &principal,
                        kind: ResourceKind::Host,
                        thread: context.node.id,
                        policies: &policies,
                        manage_environments: false,
                    },
                    bounded,
                )
                .await?;
                if !modern || !chosen.items.is_empty() {
                    let request = crate::routes::environments::places::prepare_request(
                        conn,
                        context.node.id,
                        &orch_api_contract::environment::PlaceListQuery {
                            template_id: query.template_id,
                            ..Default::default()
                        },
                    )
                    .await;
                    if let Ok(request) = request {
                        let ids = chosen
                            .items
                            .iter()
                            .map(|row| row.resource.id)
                            .collect::<Vec<_>>();
                        hosts = if modern {
                            crate::routes::environments::places::prepare_hosts_among(
                                conn,
                                &principal,
                                context.node.id,
                                &request,
                                ctx.now.into(),
                                &ids,
                            )
                            .await
                        } else {
                            crate::routes::environments::places::prepare_hosts(
                                conn,
                                &principal,
                                context.node.id,
                                &request,
                                ctx.now.into(),
                            )
                            .await
                        }
                        .unwrap_or_default();
                        if !hosts.is_empty() {
                            choice_facts.extend(
                                objects(
                                    conn,
                                    [ObjectRef {
                                        kind: ObjectKind::Template,
                                        id: request.template.id,
                                    }],
                                )
                                .await?,
                            );
                        }
                    }
                }
            }
            let host_objects = hosts
                .iter()
                .map(|host| ObjectRef {
                    kind: ObjectKind::Host,
                    id: host.host.id,
                })
                .collect::<Vec<_>>();
            choice_facts.extend(objects(conn, host_objects).await?);
            let mount_objects = hosts
                .iter()
                .flat_map(|host| {
                    host.mount_sources.iter().map(|&id| ObjectRef {
                        kind: ObjectKind::Mount,
                        id,
                    })
                })
                .collect::<Vec<_>>();
            choice_facts.extend(objects(conn, mount_objects).await?);
            let mut allowed_hosts = BTreeSet::new();
            let mut allowed_machines = BTreeSet::new();
            let env_hosts = environment_choice::hosts_of(conn, &environments.ids).await?;
            for (_, id, _) in &env_hosts {
                let (allowed, target) = bounds
                    .object(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Host,
                            id: *id,
                        },
                    )
                    .await?;
                if allowed {
                    allowed_hosts.insert(*id);
                    choice_facts.push((KeyTargetKind::Object(ObjectKind::Host), target));
                }
            }
            for row in &environment_rows {
                if let Some(machine) = row.machine_id.filter(|_| row.placement == "connected") {
                    let (allowed, target) = bounds
                        .object(
                            conn,
                            ObjectRef {
                                kind: ObjectKind::Machine,
                                id: machine,
                            },
                        )
                        .await?;
                    if allowed {
                        allowed_machines.insert(machine);
                        choice_facts.push((KeyTargetKind::Object(ObjectKind::Machine), target));
                    }
                }
            }
            let own =
                orch_db::queries::resources::routes::machines_of(conn, principal.user()).await?;
            let mut machines = Vec::new();
            for machine in own {
                let (allowed, target) = bounds
                    .object(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Machine,
                            id: machine.id,
                        },
                    )
                    .await?;
                if allowed {
                    choice_facts.push((KeyTargetKind::Object(ObjectKind::Machine), target));
                    machines.push(machine);
                }
            }
            let mut grouped_subjects = providers
                .seen
                .items
                .iter()
                .chain(providers.agents_seen.items.iter())
                .map(|row| (row.resource.kind.as_str().to_owned(), row.resource.id))
                .collect::<Vec<_>>();
            grouped_subjects.extend(
                environment_rows
                    .iter()
                    .map(|row| ("environment".to_owned(), row.id)),
            );
            grouped_subjects.extend(machines.iter().map(|row| ("machine".to_owned(), row.id)));
            grouped_subjects.extend(hosts.iter().map(|row| ("host".to_owned(), row.host.id)));
            let grouping =
                crate::routes::resources::grouping::Prepared::select(conn, &grouped_subjects)
                    .await?;
            let mut reference_machines = grouping.origins();
            reference_machines.extend(environment_rows.iter().filter_map(|row| row.machine_id));
            if let Some((environment, _)) = &context.judged {
                reference_machines.extend(environment.machine_id);
            }
            for machine in reference_machines {
                // This operation's own choice facet must admit an optional machine
                // reference. Another operation's Machine Read cannot lend it.
                let subject = ObjectRef {
                    kind: ObjectKind::Machine,
                    id: machine,
                };
                let candidate = bounds.object(conn, subject).await;
                let (allowed, target) = match candidate {
                    Ok(value) => value,
                    Err(error) if error.problem.code == ErrorCode::NotFound => continue,
                    Err(error) => return Err(error),
                };
                if allowed {
                    allowed_machines.insert(machine);
                    choice_facts.push((KeyTargetKind::Object(ObjectKind::Machine), target));
                }
            }
            facts.push(scope::collection(Source::OptionsChoices, choice_facts));
            let mut ids = context.node.path.iter().copied().collect::<BTreeSet<_>>();
            ids.extend(facts.iter().flat_map(|fact| {
                fact.groups.iter().flat_map(|(_, targets)| {
                    targets.iter().flat_map(|target| {
                        target
                            .node_ancestors
                            .iter()
                            .filter_map(|ancestor| ancestor.id)
                    })
                })
            }));
            if let Some(scope) = context.policy_scope {
                ids.insert(scope);
            }
            let uses = history::environment_uses(conn, &environments.ids, context.node.id).await?;
            ids.extend(uses.iter().flat_map(|used| used.topics.iter().copied()));
            ids.extend(
                providers
                    .seen
                    .items
                    .iter()
                    .chain(providers.agents_seen.items.iter())
                    .filter_map(|row| row.resource.home),
            );
            ids.extend(environment_rows.iter().map(|row| row.home_node_id));
            ids.extend(grouping.homes());
            ids.extend(hosts.iter().filter_map(|host| host.host.home_id));
            let graph = Graph::load(conn, &ids.iter().copied().collect::<Vec<_>>())
                .await
                .map_err(ApiError::internal)?;
            let names = ids
                .iter()
                .flat_map(|id| {
                    graph
                        .get(*id)
                        .into_iter()
                        .flat_map(|node| node.path.iter().copied())
                })
                .chain(ids.iter().copied())
                .collect();
            let plan = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            let (name_facts, names) =
                native::node_name_facts(&plan, Self::DECLARATION, caller, &graph, &names, NAMES)?;
            facts.extend(name_facts);
            let complete = native::bind(ctx, Self::DECLARATION, &facts)?;
            let reader = native::preparation_principal(ctx, conn, caller, &complete).await?;
            let visible_names = node_view::reference_visibility(
                conn,
                &reader,
                &names.iter().copied().collect::<Vec<_>>(),
                Policy::default(),
            )
            .await?;
            let projection = ChoiceProjection {
                names: NameBounds {
                    limited: modern,
                    nodes: names.clone(),
                    ..Default::default()
                },
                hosts: allowed_hosts,
                machines: allowed_machines,
            };
            Ok((
                Selected {
                    context,
                    providers,
                    inputs,
                    environments,
                    environment_rows,
                    environment_hosts: env_hosts
                        .into_iter()
                        .map(|(environment, host, _)| (environment, host))
                        .collect(),
                    hosts,
                    machines,
                    projection,
                    grouping,
                    can_create,
                    query: query.clone(),
                    names,
                    visible_names,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            prepared::require(conn, principal, ctx.id("id")?, ctx.now).await?;
            Ok(())
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<BindingOptions>> {
        Box::pin(super::render_selected(ctx, conn, principal, value))
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a NodeBindingOptionsGetQuery,
        old: &'a Selected,
        fresh: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            if !old.grouping.preserves_sources(&fresh.grouping)
                || old.context.node.path != fresh.context.node.path
                || old.context.sources != fresh.context.sources
                || !old.names.is_subset(&fresh.names)
                || !old.visible_names.is_subset(&fresh.visible_names)
            {
                return Ok(false);
            }
            if old.providers.seen.items != fresh.providers.seen.items
                || old.providers.agents_seen.items != fresh.providers.agents_seen.items
            {
                return Ok(false);
            }
            match (&old.context.judged, &fresh.context.judged) {
                (Some((old, _)), Some((fresh, _)))
                    if old.id == fresh.id
                        && old.machine_id == fresh.machine_id
                        && old.generation == fresh.generation => {}
                (None, None) => {}
                _ => return Ok(false),
            }
            for old in &old.providers.may_use.items {
                if !fresh
                    .providers
                    .may_use
                    .items
                    .iter()
                    .any(|row| row.resource.id == old.resource.id && row.granted == old.granted)
                {
                    return Ok(false);
                }
            }
            for old in &old.providers.may_use_agent.items {
                if !fresh
                    .providers
                    .may_use_agent
                    .items
                    .iter()
                    .any(|row| row.resource.id == old.resource.id && row.granted == old.granted)
                {
                    return Ok(false);
                }
            }
            if old.environments.ids != fresh.environments.ids
                || old.environments.placeable != fresh.environments.placeable
            {
                return Ok(false);
            }
            for (id, provider) in &old.inputs.providers {
                let Some(current) = fresh.inputs.providers.get(id) else {
                    return Ok(false);
                };
                if provider.key_machine_id != current.key_machine_id
                    || provider.machine_id != current.machine_id
                    || provider.held_by_environment_id != current.held_by_environment_id
                {
                    return Ok(false);
                }
            }
            for (agent, _) in &old.inputs.agents {
                if !fresh.inputs.agents.iter().any(|(current, _)| {
                    current.id == agent.id && current.machine_id == agent.machine_id
                }) {
                    return Ok(false);
                }
            }
            for row in &old.environment_rows {
                if !fresh.environment_rows.iter().any(|current| {
                    current.id == row.id
                        && current.machine_id == row.machine_id
                        && current.template_id == row.template_id
                        && current.home_node_id == row.home_node_id
                }) {
                    return Ok(false);
                }
            }
            if old.environment_hosts != fresh.environment_hosts {
                return Ok(false);
            }
            for machine in &old.machines {
                if !fresh.machines.iter().any(|row| {
                    row.id == machine.id
                        && row.host_id == machine.host_id
                        && row.runtime_id == machine.runtime_id
                        && row.owner_id == machine.owner_id
                }) {
                    return Ok(false);
                }
            }
            for host in &old.hosts {
                if !fresh.hosts.iter().any(|row| {
                    row.host.id == host.host.id
                        && row.host.machine_id == host.host.machine_id
                        && row.host.home_id == host.host.home_id
                        && row.mount_sources == host.mount_sources
                        && row.usable() == host.usable()
                }) {
                    return Ok(false);
                }
            }
            Ok(true)
        })
    }
}
