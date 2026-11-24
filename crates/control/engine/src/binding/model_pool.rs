//! Choose the provider and model of a thread without changing its agent or placement.
use super::*;
use crate::resource_choice::{
    ChoiceRequest, Hop, ProviderSelection,
    database::{Database, Requirements},
};
use orch_access_contract::permissions::UsagePurpose;
use orch_db::Conn;
use orch_provider_contract::{
    capabilities::{BackendRef, OfferingRef},
    pools::{Kind, Leaf, Target as ResourceTarget},
};

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
    request: Request<'a>,
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
        Ok(Leaf::Model {
            backend: BackendRef {
                route: "generation.model".into(),
                offering: Some(OfferingRef {
                    provider,
                    offering: model.to_owned(),
                }),
                config: json!({}),
                ..Default::default()
            },
        })
    }
    async fn reason(
        &mut self,
        conn: &mut Conn,
        principal: &Principal,
        leaf: &Leaf,
        path: &[Hop],
    ) -> Result<Option<String>, EngineError> {
        let request = &self.request;
        let Some((provider, model)) = offering(leaf, request.model) else {
            return Ok(Some("candidate_purpose".into()));
        };
        if crate::provider_policy::forbidden(conn, request.topic, provider)
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
        let effort = request.effort.or(self.settings.effort.as_deref());
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
            arrangement: request.arrangement,
        };
        match request.placement {
            Placement::Environment(id) => {
                let Some(env) = environments::environment(conn, id).await? else {
                    return Ok(Some("environment_unavailable".into()));
                };
                binding
                    .reason_in(conn, principal, request.topic, &env)
                    .await
            }
            Placement::Pool(id) => Ok((!environment_pool::possible_in_pool(
                conn,
                principal,
                request.topic,
                id,
                binding,
            )
            .await?)
                .then(|| "binding_incompatible".into())),
        }
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
        request,
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
