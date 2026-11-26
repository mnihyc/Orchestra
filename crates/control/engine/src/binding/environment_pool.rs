//! Select an existing environment in the transaction that creates its thread.
use super::*;
use crate::resource_choice::{
    ChoiceRequest, Hop,
    database::{Database, Requirements},
};
use orch_access_contract::{
    permissions::Options,
    pools::{PathMember, PoolUse},
};
use orch_authz::pools::{Demand, member_choice};
use orch_db::Conn;
use orch_provider_contract::pools::{Leaf, Target as ResourceTarget};

struct EnvironmentUseDemand;
impl Demand for EnvironmentUseDemand {
    fn pool(&self, _: &PoolUse) -> bool {
        true
    }
    fn lent(&self, options: &Options) -> bool {
        matches!(options, Options::None | Options::PoolUse(_))
    }
    fn direct(&self, usage: &orch_authz::usable::Use) -> bool {
        self.lent(&usage.options)
    }
}

async fn permitted(
    conn: &mut Conn,
    principal: &Principal,
    topic: Id,
    id: Id,
    path: &[Hop],
) -> Result<bool, EngineError> {
    let path = path
        .iter()
        .map(|hop| PathMember {
            pool_id: hop.pool_id,
            member_id: hop.member_id,
            revision: hop.revision,
        })
        .collect::<Vec<_>>();
    let policies = crate::access::resource_policies_for_choice(conn, &[id], &path).await?;
    Ok(member_choice(
        conn,
        principal,
        &path,
        &Leaf::Environment { id },
        ObjectRef {
            kind: ObjectKind::Environment,
            id,
        },
        Some(topic),
        &policies,
        &EnvironmentUseDemand,
    )
    .await?
    .is_some())
}

pub(super) struct BindingDemand<'a> {
    pub(super) pool: Option<crate::resource_choice::ProviderSelection>,
    pub(super) engine: &'a Engine,
    pub(super) agent: AgentRow,
    pub(super) provider: ProviderRow,
    pub(super) descriptor: AgentDescriptor,
    pub(super) settings: Resolved,
    pub(super) model: Option<String>,
    pub(super) effort: Option<String>,
    pub(super) arrangement: Option<ArrangementType>,
}

struct Display<'a> {
    visible: &'a std::collections::HashSet<Id>,
    binding: Option<model_pool::Setup<'a>>,
    sources: &'a mut Vec<ObjectRef>,
}

struct Candidates<'a> {
    display: Option<Display<'a>>,
    topic: Id,
    binding: Option<BindingDemand<'a>>,
}

#[async_trait::async_trait]
impl Requirements for Candidates<'_> {
    async fn model(&mut self, _: &mut Conn, _: Id, _: &str) -> Result<Leaf, EngineError> {
        Err(EngineError::unreadable(
            "A thread's environment pool cannot expand a model",
        ))
    }
    async fn reason(
        &mut self,
        conn: &mut Conn,
        principal: &Principal,
        leaf: &Leaf,
        path: &[Hop],
    ) -> Result<Option<String>, EngineError> {
        let Leaf::Environment { id } = leaf else {
            return Ok(Some("environment_template".into()));
        };
        if self
            .display
            .as_ref()
            .is_some_and(|display| !display.visible.contains(id))
        {
            return Ok(Some("no_use".into()));
        }
        if !permitted(conn, principal, self.topic, *id, path).await? {
            return Ok(Some("no_use".into()));
        }
        if !orch_db::queries::loss::accepts_threads(conn, *id).await? {
            return Ok(Some("environment_unavailable".into()));
        }
        let Some(env) = environments::environment(conn, *id).await?.filter(|e| {
            e.deleted_at.is_none()
                && e.remove_at.is_none()
                && !matches!(
                    e.status.as_str(),
                    "removed" | "lost" | "disabled" | "failed"
                )
        }) else {
            return Ok(Some("environment_unavailable".into()));
        };
        if let Some(display) = &mut self.display {
            let mut sources = match &display.binding {
                Some(binding) => match binding.on(conn, principal, *id).await? {
                    Some(sources) => sources,
                    None => return Ok(Some("binding_incompatible".into())),
                },
                None => Vec::new(),
            };
            sources.extend(path.iter().map(|hop| ObjectRef {
                kind: ObjectKind::Pool,
                id: hop.pool_id,
            }));
            *display.sources = sources;
        }
        let Some(binding) = &self.binding else {
            return Ok(None);
        };
        binding.reason_in(conn, principal, self.topic, &env).await
    }
}

/// Validate the fixed profile and provider together on at least one current environment.
pub(super) async fn possible_in_pool(
    conn: &mut Conn,
    principal: &Principal,
    topic: Id,
    pool: Id,
    binding: BindingDemand<'_>,
) -> Result<bool, EngineError> {
    use crate::resource_choice::{Event, Resolver};
    use orch_provider_contract::pools::Kind;
    let target = ResourceTarget::Pool {
        id: pool,
        kind: Kind::Environment,
    };
    let source = Database::new(
        conn,
        principal,
        Candidates {
            topic,
            display: None,
            binding: Some(binding),
        },
    );
    let mut walk = Resolver::new(source, &target, Kind::Environment).await?;
    while let Some(event) = walk.next().await? {
        if matches!(event, Event::Candidate { reason: None, .. }) {
            return Ok(true);
        }
    }
    Ok(false)
}

impl BindingDemand<'_> {
    pub(super) async fn reason_in(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        topic: Id,
        env: &EnvironmentRow,
    ) -> Result<Option<String>, EngineError> {
        let Answer::Allowed { arrangements } = self
            .engine
            .inner
            .access
            .provider_use(
                conn,
                &ProviderUse {
                    pool: self.pool.as_ref(),
                    purpose: orch_access_contract::permissions::UsagePurpose::Turn,
                    person: principal.user(),
                    node: topic,
                    provider: self.provider.id,
                    model: self.model.as_deref(),
                    effort: self.effort.as_deref(),
                    runs_in: Some(RunsIn::Environment(env.id)),
                },
            )
            .await?
        else {
            return Ok(Some("not_allowed_where".into()));
        };
        let facts = Facts {
            descriptor: &self.descriptor,
            provider: &self.provider,
            environment: Some(env),
            offers: offers_of(conn, env.id).await?,
            settings: &self.settings,
            entry: arrangements,
            colocated: colocated_of(conn, env).await?,
            runtime: runtime_facts(conn, &self.agent, &self.provider, env).await?,
        };
        let place = initial_place(conn, &facts).await?;
        let machine = machine_arrangements_of(conn, &self.provider).await?;
        Ok(choose_placed(&facts, &place, machine, self.arrangement)
            .is_err()
            .then(|| "binding_incompatible".to_owned()))
    }
}

/// A read-only ground for the setup form. Only directly usable members may supply
/// displayed facts. The native booking evaluates the complete binding on every candidate.
pub async fn setup_member(
    conn: &mut Conn,
    engine: &Engine,
    principal: &Principal,
    topic: Id,
    pool: Id,
    displayable: &std::collections::HashSet<Id>,
    binding: Option<model_pool::SetupBinding<'_>>,
) -> Result<Option<(Id, Vec<ObjectRef>)>, EngineError> {
    use crate::resource_choice::{Event, Resolver};
    use orch_provider_contract::pools::Kind;
    let target = ResourceTarget::Pool {
        id: pool,
        kind: Kind::Environment,
    };
    let binding = match binding {
        Some(binding) => {
            match model_pool::Setup::prepare(conn, engine, principal, topic, binding).await? {
                Some(binding) => Some(binding),
                None => return Ok(None),
            }
        }
        None => None,
    };
    let mut sources = Vec::new();
    let source = Database::new(
        conn,
        principal,
        Candidates {
            topic,
            display: Some(Display {
                visible: displayable,
                binding,
                sources: &mut sources,
            }),
            binding: None,
        },
    );
    let mut walk = Resolver::new(source, &target, Kind::Environment).await?;
    while let Some(event) = walk.next().await? {
        if let Event::Candidate {
            leaf: Leaf::Environment { id },
            reason: None,
            ..
        } = event
        {
            drop(walk);
            return Ok(Some((id, sources)));
        }
    }
    Ok(None)
}

pub struct Request<'a> {
    pub thread: Id,
    pub topic: Id,
    pub agent: Id,
    pub provider: Id,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub arrangement: Option<ArrangementType>,
    pub provider_selection: Option<crate::resource_choice::ProviderSelection>,
    pub target: &'a ResourceTarget,
}

pub async fn choose(
    conn: &mut Conn,
    engine: &Engine,
    principal: &Principal,
    request: Request<'_>,
) -> Result<Option<Id>, CreateThreadError> {
    request
        .target
        .validate(orch_provider_contract::pools::Kind::Environment)
        .map_err(EngineError::unreadable)?;
    let agent = resources::agent(conn, request.agent)
        .await?
        .filter(|row| row.deleted_at.is_none() && !row.disabled)
        .ok_or(CreateThreadError::AgentUnavailable)?;
    let provider = resources::provider(conn, request.provider)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.status != "disabled")
        .ok_or(CreateThreadError::ProviderUnavailable)?;
    let descriptor = harnesses::current_descriptor(conn, &agent)
        .await?
        .ok_or(CreateThreadError::AgentNotReported)?;
    let settings = resolved::load(conn, request.topic).await?;
    let sole = if request.model.is_none() && settings.model.is_none() {
        resources::sole_enabled_model(conn, provider.id).await?
    } else {
        None
    };
    let (model, effort) = model_asked(
        conn,
        provider.id,
        request.model.or(sole.as_deref()),
        request.effort,
        &settings,
    )
    .await?;
    let source = json!({"kind":"thread_creation","thread":request.thread,"node":request.topic,"purpose":"environment"});
    let binding = BindingDemand {
        pool: request.provider_selection,
        engine,
        agent,
        provider,
        descriptor,
        settings,
        model,
        effort,
        arrangement: request.arrangement,
    };
    let facts = Candidates {
        topic: request.topic,
        display: None,
        binding: Some(binding),
    };
    let choice = Database::new(conn, principal, facts)
        .reserve(&ChoiceRequest {
            id: request.thread,
            target: request.target.clone(),
            source,
        })
        .await?;
    match choice.leaf {
        Some(Leaf::Environment { id }) => Ok(Some(id)),
        _ => Ok(None),
    }
}

/// Only the recorded choice of this birth may supply its environment Use.
pub(super) async fn permits_birth(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    topic: Id,
    id: Id,
) -> Result<bool, EngineError> {
    let Some(path) = birth_path(conn, principal, thread, topic, id).await? else {
        return Ok(false);
    };
    permitted(conn, principal, topic, id, &path).await
}

/// The exact source, member and path recorded inside this creation's transaction.
pub(crate) async fn birth_path(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    topic: Id,
    id: Id,
) -> Result<Option<Vec<Hop>>, EngineError> {
    let Some(row) = orch_db::queries::pool_choices::get(conn, thread).await? else {
        return Ok(None);
    };
    if row.state != "selected"
        || row.actor_id != principal.user()
        || row.source
            != json!({"kind":"thread_creation","thread":thread,"node":topic,"purpose":"environment"})
        || row.leaf != Some(json!(Leaf::Environment { id }))
    {
        return Ok(None);
    }
    let Some(path) = row.path else {
        return Ok(None);
    };
    let path: Vec<Hop> = serde_json::from_value(path)?;
    let target: ResourceTarget = serde_json::from_value(row.target)?;
    if !matches!(target, ResourceTarget::Pool {id:root,kind:orch_provider_contract::pools::Kind::Environment} if path.first().is_some_and(|hop|hop.pool_id==root))
    {
        return Ok(None);
    }
    Ok(Some(path))
}
