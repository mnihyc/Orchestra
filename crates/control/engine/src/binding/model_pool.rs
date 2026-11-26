//! Choose the provider and model of a thread without changing its agent or placement.
use super::*;
use crate::resource_choice::{
    ChoiceRequest, Hop, ProviderSelection,
    database::{Database, Requirements},
};
use orch_access_contract::permissions::UsagePurpose;
use orch_db::Conn;
use orch_provider_contract::pools::{Kind, Leaf, Target as ResourceTarget};

#[derive(Clone, Copy)]
pub enum Placement {
    Environment(Id),
    Template { template: Id, host: Id },
    Pool(Id),
}

pub struct Request<'a> {
    pub thread: Id,
    pub topic: Id,
    pub agent: Id,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub arrangement: Option<ArrangementType>,
    pub placement: Placement,
    pub target: &'a ResourceTarget,
}

pub struct Selected {
    pub provider: Id,
    pub model: String,
    pub pool: Option<ProviderSelection>,
}

fn offering<'a>(leaf: &'a Leaf, model: Option<&'a str>) -> Option<(Id, &'a str)> {
    match leaf {
        Leaf::Provider { id } => Some((*id, model?)),
        Leaf::Model { backend }
            if backend.route == "generation.model"
                && backend.extensions.is_empty()
                && (backend.config.is_null()
                    || backend
                        .config
                        .as_object()
                        .is_some_and(|config| config.is_empty())) =>
        {
            let offering = backend.offering.as_ref()?;
            Some((offering.provider, &offering.offering))
        }
        _ => None,
    }
}

struct Candidates<'a> {
    access: &'a dyn crate::access::ResourceAccess,
    topic: Id,
    model: Option<&'a str>,
    effort: Option<&'a str>,
    arrangement: Option<ArrangementType>,
    placement: Placement,
    agent: AgentRow,
    descriptor: AgentDescriptor,
    settings: Resolved,
}

#[async_trait::async_trait]
impl Requirements for Candidates<'_> {
    async fn model(
        &mut self,
        _: &mut Conn,
        provider: Id,
        model: &str,
    ) -> Result<Leaf, EngineError> {
        Ok(Leaf::generation_model(provider, model))
    }
    async fn reason(
        &mut self,
        conn: &mut Conn,
        principal: &Principal,
        leaf: &Leaf,
        path: &[Hop],
    ) -> Result<Option<String>, EngineError> {
        let Some((provider, model)) = offering(leaf, self.model) else {
            return Ok(Some("candidate_purpose".into()));
        };
        let uses_root = |id| path.first().is_some_and(|hop| hop.pool_id == id);
        let provider_fits = match &self.settings.locked_provider {
            None => true,
            Some(ResourceTarget::Pool {
                id,
                kind: Kind::Provider,
            }) => uses_root(*id),
            Some(target) => target.single_provider() == Some(provider),
        };
        if !provider_fits {
            return Ok(Some("provider_not_allowed".into()));
        }
        use orch_settings_contract::thread_defaults::ModelDefault;
        let model_fits = match &self.settings.locked_model {
            None => true,
            Some(ModelDefault::Target(ResourceTarget::Pool {
                id,
                kind: Kind::Model,
            })) => uses_root(*id),
            Some(ModelDefault::Target(ResourceTarget::Single { resource })) => {
                resource == leaf
                    || matches!(leaf, Leaf::Provider { .. })
                        && *resource == Leaf::generation_model(provider, model)
            }
            Some(authored) => authored.offering() == Some(model),
        };
        if !model_fits {
            return Ok(Some("model_not_allowed".into()));
        }
        if crate::provider_policy::forbidden(conn, self.topic, provider)
            .await?
            .is_some()
        {
            return Ok(Some("provider_not_allowed".into()));
        }
        let Some(account) = resources::provider(conn, provider)
            .await?
            .filter(|r| r.deleted_at.is_none() && r.status != "disabled")
        else {
            return Ok(Some("provider_unavailable".into()));
        };
        let entry = resources::model(conn, provider, model).await?;
        if crate::models::check_allowed(
            entry.as_ref(),
            self.settings.models_allowed.as_deref(),
            model,
        )
        .is_err()
        {
            return Ok(Some("model_unavailable".into()));
        }
        let effort = self.effort.or(self.settings.effort.as_deref());
        if let (Some(wanted), Some(row)) = (effort, entry.as_ref())
            && !model_parameters(row)?.efforts().iter().any(|e| e == wanted)
        {
            return Ok(Some("effort_not_listed".into()));
        }
        let pool =
            ProviderSelection::prospective(leaf, path, UsagePurpose::Turn, principal.caller());
        let binding = environment_pool::BindingDemand {
            access: self.access,
            agent: self.agent.clone(),
            descriptor: self.descriptor.clone(),
            provider: account,
            settings: self.settings.clone(),
            model: Some(model.to_owned()),
            effort: effort.map(str::to_owned),
            pool,
            arrangement: self.arrangement,
        };
        match self.placement {
            Placement::Environment(id) => {
                let Some(env) = environments::environment(conn, id).await? else {
                    return Ok(Some("environment_unavailable".into()));
                };
                binding.reason_in(conn, principal, self.topic, &env).await
            }
            Placement::Template { template, host } => {
                let Some(template) = orch_db::queries::resources::hosting::template(conn, template)
                    .await?
                    .filter(|row| {
                        row.deleted_at.is_none() && row.enabled && row.kind == "environment"
                    })
                else {
                    return Ok(Some("environment_unavailable".into()));
                };
                binding
                    .recipe_reason(conn, principal, self.topic, host, &template.spec)
                    .await
            }
            Placement::Pool(id) => Ok((!environment_pool::possible_in_pool(
                conn, principal, self.topic, id, binding,
            )
            .await?)
                .then(|| "binding_incompatible".into())),
        }
    }
}

/// The requirements the person is keeping fixed while comparing environments.
pub struct SetupBinding<'a> {
    pub agent: Id,
    pub target: &'a ResourceTarget,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub arrangement: Option<ArrangementType>,
}

pub(super) struct Setup<'a> {
    access: &'a dyn crate::access::ResourceAccess,
    binding: SetupBinding<'a>,
    agent: AgentRow,
    descriptor: AgentDescriptor,
    kind: Kind,
    topic: Id,
    settings: Resolved,
}

impl<'a> Setup<'a> {
    pub(super) async fn prepare(
        conn: &mut Conn,
        access: &'a dyn crate::access::ResourceAccess,
        principal: &Principal,
        topic: Id,
        binding: SetupBinding<'a>,
    ) -> Result<Option<Self>, EngineError> {
        let kind = match binding.target {
            ResourceTarget::Single { resource } => resource.kind(),
            ResourceTarget::Pool { kind, .. } => *kind,
        };
        if !matches!(kind, Kind::Provider | Kind::Model) || binding.target.validate(kind).is_err() {
            return Ok(None);
        }
        if !matches!(
            access
                .agent_use(conn, principal.user(), topic, binding.agent)
                .await?,
            Answer::Allowed { .. }
        ) {
            return Ok(None);
        }
        let Some(agent) = resources::agent(conn, binding.agent)
            .await?
            .filter(|row| row.deleted_at.is_none() && !row.disabled)
        else {
            return Ok(None);
        };
        let Some(descriptor) = harnesses::current_descriptor(conn, &agent).await? else {
            return Ok(None);
        };
        Ok(Some(Self {
            access,
            binding,
            kind,
            topic,
            agent,
            descriptor,
            settings: resolved::load(conn, topic).await?,
        }))
    }

    /// A read-only existential check. The native booking handles ranking and the final grant.
    pub(super) async fn on(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        placement: Placement,
    ) -> Result<Option<Vec<ObjectRef>>, EngineError> {
        use crate::resource_choice::{Event, Resolver};
        let binding = &self.binding;
        let sole = if binding.model.is_none()
            && self.settings.model.is_none()
            && let Some(provider) = binding.target.single_provider()
        {
            resources::sole_enabled_model(conn, provider).await?
        } else {
            None
        };
        let model = binding
            .model
            .or(self.settings.model.as_deref())
            .or(sole.as_deref());
        let facts = Candidates {
            access: self.access,
            topic: self.topic,
            model,
            effort: binding.effort,
            arrangement: binding.arrangement,
            placement,
            agent: self.agent.clone(),
            descriptor: self.descriptor.clone(),
            settings: self.settings.clone(),
        };
        let mut walk = Resolver::new(
            Database::new(conn, principal, facts),
            binding.target,
            self.kind,
        )
        .await?;
        while let Some(event) = walk.next().await? {
            if let Event::Candidate {
                leaf,
                path,
                reason: None,
            } = event
                && let Some((provider, _)) = offering(&leaf, model)
            {
                let mut sources = vec![
                    ObjectRef {
                        kind: ObjectKind::Agent,
                        id: self.agent.id,
                    },
                    ObjectRef {
                        kind: ObjectKind::Provider,
                        id: provider,
                    },
                ];
                sources.extend(path.iter().map(|hop| ObjectRef {
                    kind: ObjectKind::Pool,
                    id: hop.pool_id,
                }));
                return Ok(Some(sources));
            }
        }
        Ok(None)
    }
}

/// Preview or reserve only the binding that the thread's native history plan will carry.
pub(crate) async fn switch_choice(
    conn: &mut Conn,
    access: &dyn crate::access::ResourceAccess,
    principal: &Principal,
    thread: &threads::ThreadRow,
    binding: SetupBinding<'_>,
    confirmation: Option<Id>,
) -> Result<Option<(Selected, crate::resource_choice::Choice)>, EngineError> {
    let (Some(segment), Some(environment)) = (thread.current_segment_id, thread.environment_id)
    else {
        return Ok(None);
    };
    let Some(setup) = Setup::prepare(conn, access, principal, thread.node_id, binding).await?
    else {
        return Ok(None);
    };
    let model = setup.binding.model.or(setup.settings.model.as_deref());
    let facts = Candidates {
        access,
        topic: thread.node_id,
        model,
        effort: setup.binding.effort,
        arrangement: setup.binding.arrangement,
        placement: Placement::Environment(environment),
        agent: setup.agent,
        descriptor: setup.descriptor,
        settings: setup.settings.clone(),
    };
    let target = setup.binding.target;
    let source = Database::new(conn, principal, facts);
    let choice = if let Some(preflight) = confirmation {
        source
            .reserve_reviewed(
                &ChoiceRequest {
                    id: preflight,
                    target: target.clone(),
                    source: switch_source(thread.node_id, segment, preflight),
                },
                segment,
            )
            .await?
    } else {
        source.propose(target, segment).await?
    };
    let Some(leaf) = choice.leaf.as_ref() else {
        return Ok(None);
    };
    let Some((provider, model)) = offering(leaf, model) else {
        return Ok(None);
    };
    let pool =
        ProviderSelection::prospective(leaf, &choice.path, UsagePurpose::Turn, principal.caller());
    Ok(Some((
        Selected {
            provider,
            model: model.to_owned(),
            pool,
        },
        choice,
    )))
}

pub(crate) fn switch_source(thread: Id, segment: Id, preflight: Id) -> Json {
    json!({"kind":"thread_switch","node":thread,"segment":segment,"preflight":preflight,"purpose":"agent"})
}

pub async fn choose(
    conn: &mut Conn,
    engine: &Engine,
    principal: &Principal,
    request: Request<'_>,
) -> Result<Option<Selected>, CreateThreadError> {
    let target = request.target;
    let thread = request.thread;
    let topic = request.topic;
    let model = request.model;
    if target.validate(Kind::Model).is_err() && target.validate(Kind::Provider).is_err() {
        return Ok(None);
    }
    let agent = resources::agent(conn, request.agent)
        .await?
        .filter(|r| r.deleted_at.is_none() && !r.disabled)
        .ok_or(CreateThreadError::AgentUnavailable)?;
    let descriptor = harnesses::current_descriptor(conn, &agent)
        .await?
        .ok_or(CreateThreadError::AgentNotReported)?;
    let settings = resolved::load(conn, topic).await?;
    let mut facts = Candidates {
        access: engine.inner.access.as_ref(),
        topic,
        model,
        effort: request.effort,
        arrangement: request.arrangement,
        placement: request.placement,
        agent,
        descriptor,
        settings,
    };
    let (leaf, choice) = match target {
        ResourceTarget::Single { resource } => {
            if facts
                .reason(conn, principal, resource, &[])
                .await?
                .is_some()
            {
                return Ok(None);
            }
            (resource.clone(), None)
        }
        ResourceTarget::Pool { .. } => {
            let choice = Database::new(conn,principal,facts).reserve(&ChoiceRequest {
                id: new_id(), target: target.clone(), source: json!({"kind":"thread_creation","node":thread,"topic":topic,"purpose":"agent"})
            }).await?;
            let Some(leaf) = choice.leaf else {
                return Ok(None);
            };
            (leaf, Some(choice.id))
        }
    };
    let Some((provider, model)) = offering(&leaf, model) else {
        return Ok(None);
    };
    let pool = match choice {
        Some(id) => Some(
            ProviderSelection::load(
                conn,
                id,
                thread,
                provider,
                Some(model),
                UsagePurpose::Turn,
                principal.caller(),
            )
            .await?,
        ),
        None => None,
    };
    Ok(Some(Selected {
        provider,
        model: model.to_owned(),
        pool,
    }))
}

/// Link the actual initial segment only to its own accepted thread creation.
pub(super) async fn save_segment(
    conn: &mut Conn,
    selection: &ProviderSelection,
    thread: Id,
    topic: Id,
    person: Id,
    segment: Id,
) -> Result<(), CreateThreadError> {
    let choice = selection
        .choice_id()
        .ok_or(CreateThreadError::OperationScope)?;
    let source = json!({"kind":"thread_creation","node":thread,"topic":topic,"purpose":"agent"});
    let inserted = sqlx::query("INSERT INTO segment_pool_bindings(segment_id,choice_id) SELECT $1,id FROM pool_choices WHERE id=$2 AND actor_id=$3 AND source=$4 AND state='selected'")
        .bind(segment).bind(choice).bind(person).bind(source).execute(conn).await.map_err(orch_db::DbError::from)?;
    if inserted.rows_affected() != 1 {
        return Err(CreateThreadError::OperationScope);
    }
    Ok(())
}

pub(crate) async fn segment_choice(
    conn: &mut Conn,
    segment: Id,
) -> Result<Option<Id>, EngineError> {
    Ok(
        sqlx::query_scalar("SELECT choice_id FROM segment_pool_bindings WHERE segment_id=$1")
            .bind(segment)
            .fetch_optional(conn)
            .await
            .map_err(orch_db::DbError::from)?,
    )
}

pub(crate) async fn current(
    conn: &mut Conn,
    thread: &threads::ThreadRow,
    provider: Id,
    model: Option<&str>,
    caller: &Caller,
) -> Result<Option<ProviderSelection>, EngineError> {
    let Some(segment) = thread.current_segment_id else {
        return Ok(None);
    };
    let Some(choice) = segment_choice(conn, segment).await? else {
        return Ok(None);
    };
    ProviderSelection::load(
        conn,
        choice,
        thread.node_id,
        provider,
        model,
        UsagePurpose::Turn,
        caller,
    )
    .await
    .map(Some)
}
