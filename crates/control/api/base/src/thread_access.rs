//! What the thread engine asks about resources, answered by the access engine.
//!
//! The engine never decides whether a person may use a provider, an agent
//! profile or an environment: it asks, through a small trait of its own,
//! and this is where the question reaches the access engine. Each answer
//! is the access engine's: a use with what it is narrowed to, or a refusal
//! with everything the person lacks and who can give it. A state that
//! stops a resource for everybody (switched off, deleted, a model that is
//! not allowed) is no refusal of the person, and is left to the engine's
//! own checks, which block the thread with their own reasons.

use async_trait::async_trait;

use orch_access_contract::decisions::Decision;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::decisions::Refusal;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_agent_contract::descriptor::ArrangementType;

use orch_authz::usable;

use orch_authz::usable::Author;

use orch_authz::usable::BindingAsk;

use orch_authz::usable::BindingDecision;

use orch_authz::usable::Policies;

use orch_authz::usable::ProviderChoice;

use orch_authz::usable::TurnAsk;

use orch_authz::usable::TurnDecision;

use orch_authz::usable::Usable;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_authz::resources;

use orch_db::Conn;

use orch_db::queries::hierarchy;

use orch_db::queries::resources as rows;

use orch_engine::access::AccessFault;

use orch_engine::access::Answer;

use orch_engine::access::Limit;

use orch_engine::access::ProviderUse;

use orch_engine::access::ResourceAccess;

use orch_engine::access::RunsIn;

use orch_types::Clock;

use orch_types::Id;

use orch_types::Time;

use std::sync::Arc;
/// The access engine, as the thread engine asks it.
#[derive(Debug, Clone)]
pub struct AuthzAccess {
    clock: Arc<dyn Clock>,
}

fn fault(error: impl std::fmt::Display) -> AccessFault {
    AccessFault::Failed(error.to_string())
}

/// An arrangement type of the access catalogue, as the agent contract
/// names the same thing.
fn arrangement(of: orch_access_contract::permissions::Arrangement) -> Option<ArrangementType> {
    serde_json::from_value(serde_json::Value::String(of.as_str().to_owned())).ok()
}

fn refused(refusal: Refusal) -> Answer {
    Answer::Refused(refusal.missing)
}

impl AuthzAccess {
    /// The access engine, read at the moments `clock` shows.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self { clock }
    }

    /// Where the work an ask is about runs, for the entries to be held to it; none for work that
    /// runs in no environment.
    async fn place(
        conn: &mut Conn,
        runs_in: Option<RunsIn>,
    ) -> Result<Option<orch_access_contract::whereabouts::Whereabouts>, AccessFault> {
        Ok(match runs_in {
            Some(RunsIn::Environment(environment)) => {
                Some(usable::place_of(conn, environment).await.map_err(fault)?)
            }
            Some(RunsIn::NewOnHost(host)) => {
                Some(usable::place_of_host(conn, host).await.map_err(fault)?)
            }
            Some(RunsIn::NewOnMachine(machine)) => Some(
                usable::place_of_machine(conn, machine)
                    .await
                    .map_err(fault)?,
            ),
            None => None,
        })
    }

    fn now(&self) -> Time {
        self.clock.time()
    }

    /// The person as the access engine knows them now. They are asked about
    /// as themself, in a session: what a token of theirs was narrowed to was
    /// checked where the request came in.
    async fn principal(&self, conn: &mut Conn, person: Id) -> Result<Principal, AccessFault> {
        self.acting(conn, person, false).await
    }

    /// The person as they act: with As admin on or off, as the session they act from has it.
    async fn acting(
        &self,
        conn: &mut Conn,
        person: Id,
        as_admin: bool,
    ) -> Result<Principal, AccessFault> {
        let caller = Caller::session(person).with_admin_mode(as_admin);
        Principal::load(conn, caller, self.now())
            .await
            .map_err(fault)
    }

    /// What the settings say that decides about `resource`, resolved at its
    /// home (at the system for a resource of the system).
    pub async fn policies(conn: &mut Conn, resource: Id) -> Result<Policies, AccessFault> {
        orch_engine::access::resource_policies(conn, resource)
            .await
            .map_err(fault)
    }

    /// The answer for one resource and one permission on its own page, for
    /// a person who was found to have no use of it: everything they lack.
    async fn lacking(
        conn: &mut Conn,
        principal: &Principal,
        kind: ObjectKind,
        id: Id,
        permission: Permission,
        policies: &Policies,
        home: Option<Id>,
    ) -> Result<Answer, AccessFault> {
        let object = ObjectRef { kind, id };
        match resources::check(
            conn,
            principal,
            object,
            permission,
            policies.at(home).node_policy(),
        )
        .await
        .map_err(fault)?
        {
            Decision::Refused(refusal) => Ok(refused(refusal)),
            // The page gives it and the use does not follow (a subscription
            // kept to its owner): nobody can give more than the page has.
            Decision::Allowed { .. } => Ok(Answer::Refused(Vec::new())),
        }
    }
}

#[async_trait]
impl ResourceAccess for AuthzAccess {
    async fn provider_use(
        &self,
        conn: &mut Conn,
        ask: &ProviderUse<'_>,
    ) -> Result<Answer, AccessFault> {
        if ask.purpose != orch_access_contract::permissions::ProviderPurpose::Turn {
            return self.background_provider_use(conn, ask).await;
        }
        let principal = self.principal(conn, ask.person).await?;
        let policies = Self::policies(conn, ask.provider).await?;
        let place = Self::place(conn, ask.runs_in).await?;
        let kind = hierarchy::with_ancestors(conn, &[ask.node])
            .await
            .map_err(fault)?
            .into_iter()
            .find(|node| node.id == ask.node)
            .map(|node| node.kind);
        let arrangements = |of: &[orch_access_contract::permissions::Arrangement]| {
            Some(
                of.iter()
                    .copied()
                    .filter_map(arrangement)
                    .collect::<Vec<_>>(),
            )
        };
        match (ask.model, kind.as_deref()) {
            // A turn on a thread: the one question the access engine answers whole.
            (Some(model), Some("thread")) => {
                let turn = TurnAsk {
                    thread: ask.node,
                    provider: ask.provider,
                    profile: None,
                    model,
                    effort: ask.effort,
                    arrangement: None,
                    models_allowed: None,
                };
                match usable::check_turn_at(
                    conn,
                    Author::Person(&principal),
                    &turn,
                    &policies,
                    place.as_ref(),
                )
                .await
                {
                    Ok(TurnDecision::Allowed(grant)) => Ok(Answer::Allowed {
                        arrangements: arrangements(&grant.arrangements),
                    }),
                    Ok(TurnDecision::Blocked(_)) | Err(orch_authz::Error::Archived { .. }) => {
                        Ok(Answer::allowed())
                    }
                    Ok(TurnDecision::Refused(refusal)) => Ok(refused(*refusal)),
                    Err(orch_authz::Error::Suspended { .. }) => Ok(Answer::Refused(Vec::new())),
                    Err(error) => Err(fault(error)),
                }
            }
            // A thread that is being made in a place.
            (Some(model), Some(_)) => {
                let binding = BindingAsk {
                    node: ask.node,
                    permission: Permission::TopicStartThreads,
                    provider: Some(ProviderChoice {
                        id: ask.provider,
                        model,
                        effort: ask.effort,
                        arrangement: None,
                    }),
                    profile: None,
                    environment: None,
                    models_allowed: None,
                };
                match usable::check_binding_at(
                    conn,
                    &principal,
                    &binding,
                    &policies,
                    place.as_ref(),
                )
                .await
                {
                    Ok(BindingDecision::Allowed(grant)) => Ok(Answer::Allowed {
                        arrangements: grant
                            .provider
                            .as_ref()
                            .and_then(|provider| arrangements(&provider.arrangements)),
                    }),
                    Ok(BindingDecision::Blocked(_)) | Err(orch_authz::Error::Archived { .. }) => {
                        Ok(Answer::allowed())
                    }
                    Ok(BindingDecision::Refused(refusal)) => Ok(refused(*refusal)),
                    Ok(BindingDecision::Exceeds(_) | BindingDecision::NotYourMachine) => {
                        Err(AccessFault::Failed(
                            "a question about a provider was answered about an environment"
                                .to_owned(),
                        ))
                    }
                    Err(orch_authz::Error::Suspended { .. }) => Ok(Answer::Refused(Vec::new())),
                    Err(error) => Err(fault(error)),
                }
            }
            // The provider alone, whatever it would run.
            _ => {
                let home = rows::resource_facts(conn, &[ask.provider])
                    .await
                    .map_err(fault)?
                    .first()
                    .and_then(|facts| facts.home_id);
                match usable::usable(conn, &principal, ask.provider, Some(ask.node), &policies)
                    .await
                    .map_err(fault)?
                {
                    Usable::Yes(granted) => match usable::judge_alone(
                        conn,
                        &principal,
                        ask.provider,
                        ask.node,
                        &granted,
                        place.as_ref(),
                        &policies,
                    )
                    .await
                    .map_err(fault)?
                    {
                        usable::Alone::Allowed { arrangements: list } => Ok(Answer::Allowed {
                            arrangements: arrangements(&list),
                        }),
                        usable::Alone::Refused(refusal) => Ok(refused(*refusal)),
                    },
                    Usable::Unavailable(_) => Ok(Answer::allowed()),
                    Usable::KeptToItsOwner | Usable::No => {
                        Self::lacking(
                            conn,
                            &principal,
                            ObjectKind::Provider,
                            ask.provider,
                            Permission::ProviderUse,
                            &policies,
                            home,
                        )
                        .await
                    }
                }
            }
        }
    }

    async fn background_provider_use(
        &self,
        conn: &mut Conn,
        ask: &ProviderUse<'_>,
    ) -> Result<Answer, AccessFault> {
        let principal = self.principal(conn, ask.person).await?;
        let policies = Self::policies(conn, ask.provider).await?;
        match usable::usable(conn, &principal, ask.provider, Some(ask.node), &policies)
            .await
            .map_err(fault)?
        {
            Usable::Yes(granted) => {
                let place = Self::place(conn, ask.runs_in).await?;
                let groups = match ask.model {
                    Some(name) => rows::provider_models(conn, ask.provider)
                        .await
                        .map_err(fault)?
                        .into_iter()
                        .find(|model| model.model_name == name)
                        .map_or_else(Vec::new, |model| model.groups),
                    None => Vec::new(),
                };
                let wanted = orch_authz::options::Wanted {
                    model: ask.model.map(|name| (name, groups.as_slice())),
                    effort: ask.effort,
                    at: place.as_ref(),
                    ..orch_authz::options::Wanted::default()
                };
                // One entry has to allow the model, the effort and the place together.
                let allowing = granted.allowing_for(&wanted, ask.purpose.permission_use());
                if allowing.is_empty() {
                    return Ok(Answer::Refused(Vec::new()));
                }
                let entries: Vec<_> = allowing
                    .iter()
                    .map(|allowance| &allowance.provider)
                    .collect();
                Ok(Answer::Allowed {
                    arrangements: Some(
                        orch_authz::options::arrangements_of(&entries)
                            .into_iter()
                            .filter_map(arrangement)
                            .collect(),
                    ),
                })
            }
            // A concurrent availability change must not bypass the writer's grant.
            Usable::Unavailable(_) | Usable::KeptToItsOwner | Usable::No => {
                Ok(Answer::Refused(Vec::new()))
            }
        }
    }

    async fn agent_use(
        &self,
        conn: &mut Conn,
        person: Id,
        node: Id,
        agent: Id,
    ) -> Result<Answer, AccessFault> {
        let principal = self.principal(conn, person).await?;
        let policies = Self::policies(conn, agent).await?;
        let home = rows::resource_facts(conn, &[agent])
            .await
            .map_err(fault)?
            .first()
            .and_then(|facts| facts.home_id);
        match usable::usable(conn, &principal, agent, Some(node), &policies)
            .await
            .map_err(fault)?
        {
            Usable::Yes(_) | Usable::Unavailable(_) => Ok(Answer::allowed()),
            Usable::KeptToItsOwner | Usable::No => {
                Self::lacking(
                    conn,
                    &principal,
                    ObjectKind::Agent,
                    agent,
                    Permission::AgentUse,
                    &policies,
                    home,
                )
                .await
            }
        }
    }

    async fn environment_place(
        &self,
        conn: &mut Conn,
        person: Id,
        environment: Id,
    ) -> Result<Answer, AccessFault> {
        let principal = self.principal(conn, person).await?;
        let policies = Self::policies(conn, environment).await?;
        let home = rows::resource_facts(conn, &[environment])
            .await
            .map_err(fault)?
            .first()
            .and_then(|facts| facts.home_id);
        let object = ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        };
        match resources::check(
            conn,
            &principal,
            object,
            Permission::EnvironmentUse,
            policies.at(home).node_policy(),
        )
        .await
        .map_err(fault)?
        {
            Decision::Allowed { .. } => Ok(Answer::allowed()),
            Decision::Refused(refusal) => Ok(refused(refusal)),
        }
    }

    async fn environment_manage(
        &self,
        conn: &mut Conn,
        person: Id,
        environment: Id,
    ) -> Result<Answer, AccessFault> {
        let principal = self.principal(conn, person).await?;
        let policies = Self::policies(conn, environment).await?;
        let home = rows::resource_facts(conn, &[environment])
            .await
            .map_err(fault)?
            .first()
            .and_then(|facts| facts.home_id);
        let object = ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        };
        match resources::check(
            conn,
            &principal,
            object,
            Permission::EnvironmentManage,
            policies.at(home).node_policy(),
        )
        .await
        .map_err(fault)?
        {
            Decision::Allowed { .. } => Ok(Answer::allowed()),
            Decision::Refused(refusal) => Ok(refused(refusal)),
        }
    }

    async fn may_pass(
        &self,
        conn: &mut Conn,
        person: Id,
        as_admin: bool,
        limit: &Limit,
    ) -> Result<bool, AccessFault> {
        match *limit {
            Limit::ProviderLimit { provider } => {
                let principal = self.acting(conn, person, as_admin).await?;
                let policies = Self::policies(conn, provider).await?;
                let home = rows::resource_facts(conn, &[provider])
                    .await
                    .map_err(fault)?
                    .first()
                    .and_then(|facts| facts.home_id);
                let object = ObjectRef {
                    kind: ObjectKind::Provider,
                    id: provider,
                };
                let decision = resources::check(
                    conn,
                    &principal,
                    object,
                    Permission::ProviderBypass,
                    policies.at(home).node_policy(),
                )
                .await
                .map_err(fault)?;
                Ok(matches!(decision, Decision::Allowed { .. }))
            }
            // Who may let a turn past a cap is decided where the cap is
            // defined: by the bypass row of its node or of its provider, or by
            // a system admin for a cap of the system.
            Limit::Cap { cap } => {
                let Some(row) = orch_db::usage::caps::get(conn, cap).await.map_err(fault)? else {
                    return Ok(false);
                };
                let principal = self.acting(conn, person, as_admin).await?;
                orch_authz::caps::may_pass(
                    conn,
                    &principal,
                    orch_authz::caps::Home::of(row.node_id, row.provider_id),
                    orch_authz::Policy::default(),
                )
                .await
                .map_err(fault)
            }
        }
    }
}
