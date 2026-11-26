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
    engine: &'a Engine,
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
            engine: self.engine,
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
    engine: &'a Engine,
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
        engine: &'a Engine,
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
            engine
                .inner
                .access
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
            engine,
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
        environment: Id,
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
            engine: self.engine,
            topic: self.topic,
            model,
            effort: binding.effort,
            arrangement: binding.arrangement,
            placement: Placement::Environment(environment),
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
        engine,
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
