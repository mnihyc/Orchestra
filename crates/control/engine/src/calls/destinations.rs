//! A call rule asks the common resource resolver; it does not dispatch or retry outside recovery.
use super::*;
use crate::resource_choice::{
    ChoiceRequest, Hop,
    database::{Database, Requirements},
};
use orch_authz::Principal;

pub(crate) struct DestinationRequest {
    pub node: Id,
    pub purpose: Purpose,
    pub root: Id,
    pub attempt: Id,
    pub retained_route: Option<BackendRef>,
    pub demand: Option<RequestDemand>,
}

/// The work a summary will submit, captured before ranking any of its destinations.
pub(crate) struct RequestDemand {
    tokens: orch_db::usage::ledger::Tokens,
    params: Option<Json>,
    protocol: Option<String>,
}

impl RequestDemand {
    pub(crate) fn summary(
        system: &str,
        input: &str,
        output: i64,
        params: Option<&Json>,
        protocol: Option<&str>,
    ) -> Self {
        use orch_summarizer_model::context::estimated_tokens;
        let input = i64::from(estimated_tokens(system)) + i64::from(estimated_tokens(input));
        Self {
            tokens: orch_db::usage::ledger::Tokens {
                input,
                output,
                total: input.saturating_add(output),
                ..Default::default()
            },
            params: params.cloned(),
            protocol: protocol.map(str::to_owned),
        }
    }

    fn fits_model(
        &self,
        model: &orch_db::queries::resources::thread_engine::ModelRow,
        protocol: Option<&str>,
    ) -> Result<bool, EngineError> {
        let parameters = orch_provider_contract::models::composed_model_parameters(
            &model.parameters,
            &model.groups,
            &model.option_layers,
            protocol,
        )?;
        let window = parameters
            .context_open()
            .or(parameters.context_window.filter(|limit| *limit > 0));
        Ok(window.is_none_or(|limit| self.tokens.total <= limit))
    }
}
pub(crate) struct CandidateResolution {
    pub selected: Option<SelectedCandidate>,
    pub reason: String,
}
impl CandidateResolution {
    fn rejected(reason: &str) -> Self {
        Self {
            selected: None,
            reason: reason.into(),
        }
    }
}

pub(super) async fn add_pool_contribution(
    conn: &mut Conn,
    policy: &mut Policy,
    purpose: Purpose,
    reference: Id,
    failure: &Json,
    context: &Json,
) -> Result<(), EngineError> {
    if policy.explicit_rules
        || context["effects"] != false
        || context["manual_stop"] == true
        || !matches!(
            failure["code"].as_str(),
            Some(
                "provider_busy"
                    | "rate_limited"
                    | "overloaded"
                    | "provider_unreachable"
                    | "provider_unauthorized"
                    | "provider_exhausted"
                    | "model_unavailable"
            )
        )
    {
        return Ok(());
    }
    // Only a terminal native receipt contributes this reviewed default. An unacknowledged
    // start has no settled capture and remains with its original command's reconciliation.
    let target: Option<Json> = sqlx::query_scalar("SELECT DISTINCT c.target FROM pool_attempt_scopes a JOIN pool_choices c ON c.id=a.choice_id WHERE a.attempt_id=$1 AND a.settled_at IS NOT NULL AND c.target->>'type'='pool'")
        .bind(reference).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
    let Some(target) = target else { return Ok(()) };
    let target: Target = serde_json::from_value(target)?;
    let Target::Pool { id, .. } = target else {
        return Ok(());
    };
    let Some(pool) = orch_db::queries::pools::get(conn, id)
        .await?
        .filter(|pool| {
            pool.deleted_at.is_none()
                && pool.definition.get("enabled").and_then(Json::as_bool) == Some(true)
        })
    else {
        return Ok(());
    };
    let attempts = pool_attempts(conn, pool.id).await?;
    if attempts == Some(0) {
        return Ok(());
    }
    policy.rules.insert(
        0,
        Rule {
            id: "pool-next-member".into(),
            when: orch_provider_contract::calls::When {
                purpose: Some(purpose),
                ..Default::default()
            },
            steps: vec![
                Step::Failover {
                    candidate: Destination {
                        target,
                        agent_id: None,
                        params: json!({}),
                        dimensions: context
                            .get("dimensions")
                            .and_then(Json::as_u64)
                            .and_then(|value| u32::try_from(value).ok()),
                    },
                },
                Step::Wait {
                    until: Until::Person,
                },
            ],
        },
    );
    Ok(())
}
async fn pool_attempts(conn: &mut Conn, pool: Id) -> Result<Option<u64>, EngineError> {
    // The nullable default resolves to no finite bound, like an explicit unbounded
    // setting. It is a valid policy rather than a missing registry default.
    Ok(orch_settings::get(conn, Place::Pool(pool), "pools.attempts").await?)
}
pub(crate) async fn policy_for_recovery(
    conn: &mut Conn,
    row: &Recovery,
) -> Result<Policy, EngineError> {
    let mut policy = Policy::load(conn, row.node_id).await?;
    let purpose: Purpose = serde_json::from_value(json!(row.purpose))?;
    add_pool_contribution(
        conn,
        &mut policy,
        purpose,
        row.reference,
        &row.failure,
        &row.context,
    )
    .await?;
    Ok(policy)
}
/// Price the actual summary request and service tier against the member's current rate.
pub(crate) async fn summary_measurements(
    conn: &mut Conn,
    node: Id,
    candidate: &SelectedCandidate,
    demand: &RequestDemand,
    inherit_params: bool,
    rates: &mut Option<crate::rates::RateBook>,
) -> Result<crate::resource_choice::Measurements, EngineError> {
    let mut facts = crate::resource_choice::Measurements::default();
    let model = candidate
        .route
        .as_ref()
        .and_then(|route| route.offering.as_ref())
        .map_or(candidate.model.as_str(), |offering| {
            offering.offering.as_str()
        });
    use orch_db::queries::resources::thread_engine as resources;
    use orch_model_contract::parameters;
    let Some(provider) = resources::provider(conn, candidate.provider_id).await? else {
        return Ok(facts);
    };
    let (rate, tier, alias) = if candidate.route.is_none() {
        let Some(protocol) = crate::summaries::resource::recovery_protocol(
            &provider.protocols,
            demand.protocol.as_deref().unwrap_or_default(),
        )?
        else {
            return Ok(facts);
        };
        let Some(entry) = resources::model(conn, provider.id, model).await? else {
            return Ok(facts);
        };
        let parameters = orch_provider_contract::models::composed_model_parameters(
            &entry.parameters,
            &entry.groups,
            &entry.option_layers,
            Some(&protocol),
        )?;
        let endpoint = resources::provider_request_defaults(conn, provider.id, &protocol).await?;
        let Ok(resolved) =
            parameters::resolve(&protocol, &parameters, &endpoint, &candidate.params)
        else {
            return Ok(facts);
        };
        if parameters::requested_capacity_tier(&protocol, &resolved)
            .is_some_and(|class| !parameters::ordinary_capacity_tier(&protocol, &class))
        {
            return Ok(facts);
        }
        let tier = parameters::requested_tier_for_caps(&protocol, &resolved);
        let alias = tier
            .as_deref()
            .and_then(|value| parameters::service_tier_price_alias(&protocol, value));
        (parameters.rate, tier, alias)
    } else {
        let Some(agent) = provider.agent_id else {
            return Ok(facts);
        };
        let Some(agent) = resources::agent(conn, agent).await? else {
            return Ok(facts);
        };
        let Some(descriptor) = crate::harnesses::descriptor_of(&agent)? else {
            return Ok(facts);
        };
        let params = if inherit_params
            && candidate
                .params
                .as_object()
                .is_some_and(|params| params.is_empty())
        {
            demand.params.as_ref()
        } else {
            Some(&candidate.params)
        };
        let tier = params
            .and_then(|params| crate::providers::service_tier(&descriptor.parameters, params));
        (
            crate::caps::own_rate(conn, provider.id, model).await?,
            tier,
            None,
        )
    };
    if rates.is_none() {
        *rates = Some(crate::rates::RateBook::load(conn, node).await?);
    }
    let Some(rates) = rates.as_ref() else {
        return Ok(facts);
    };
    let priced = rates
        .price_with_alias(
            conn,
            rate.as_ref(),
            model,
            &demand.tokens,
            Some(demand.tokens.input),
            (tier.as_deref(), alias),
        )
        .await?;
    facts.quote = crate::resource_choice::Quote::uncached_request(
        priced,
        demand.tokens.input,
        demand.tokens.output,
        tier.as_deref(),
    );
    Ok(facts)
}

struct Facts<'a> {
    engine: &'a Engine,
    request: &'a DestinationRequest,
    intent: &'a Destination,
    retained_model: &'a str,
    callers: &'a [Caller],
    chain: Id,
    failure: Json,
    first_failure: Json,
    rates: Option<crate::rates::RateBook>,
}
#[async_trait::async_trait]
impl Requirements for Facts<'_> {
    async fn measurements(
        &mut self,
        conn: &mut Conn,
        _: &Principal,
        leaf: &Leaf,
        _: &[Hop],
        _: bool,
    ) -> Result<crate::resource_choice::Measurements, EngineError> {
        let facts = crate::resource_choice::Measurements::default();
        let Some(demand) = &self.request.demand else {
            return Ok(facts);
        };
        let Some(candidate) = SelectedCandidate::from_leaf(
            self.intent,
            leaf,
            self.retained_model,
            self.request.retained_route.as_ref(),
            None,
        ) else {
            return Ok(facts);
        };
        summary_measurements(
            conn,
            self.request.node,
            &candidate,
            demand,
            self.request.retained_route.is_some(),
            &mut self.rates,
        )
        .await
    }
    async fn model(
        &mut self,
        _: &mut Conn,
        provider: Id,
        model: &str,
    ) -> Result<Leaf, EngineError> {
        let mut backend = self
            .request
            .retained_route
            .clone()
            .unwrap_or_else(|| BackendRef {
                route: "generation.model".into(),
                config: json!({}),
                ..Default::default()
            });
        backend.offering = Some(OfferingRef {
            provider,
            offering: model.into(),
        });
        Ok(Leaf::Model { backend })
    }
    async fn reason(
        &mut self,
        conn: &mut Conn,
        _: &Principal,
        leaf: &Leaf,
        path: &[Hop],
    ) -> Result<Option<String>, EngineError> {
        let Some(candidate) = SelectedCandidate::from_leaf(
            self.intent,
            leaf,
            self.retained_model,
            self.request.retained_route.as_ref(),
            None,
        ) else {
            return Ok(Some("candidate_purpose".into()));
        };
        let already: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pool_choices WHERE source->>'chain'=$1 AND state='selected' AND id<>$2 AND leaf=$3)")
            .bind(self.chain.to_string()).bind(self.request.attempt).bind(serde_json::to_value(leaf)?).fetch_one(&mut *conn).await.map_err(DbError::from)?;
        let was_tried = |observed: &Json| match leaf {
            _ if observed.get("leaf").is_some_and(|value| !value.is_null()) => {
                observed["leaf"] == json!(leaf)
            }
            Leaf::Provider { id: provider } => Some(*provider) == id(observed, "provider_id"),
            Leaf::Model { backend } => {
                observed
                    .get("capability_route")
                    .is_some_and(|route| route == &json!(backend))
                    || backend.route == "generation.model"
                        && (backend.config.is_null()
                            || backend
                                .config
                                .as_object()
                                .is_some_and(serde_json::Map::is_empty))
                        && backend.extensions.is_empty()
                        && Some(candidate.provider_id) == id(observed, "provider_id")
                        && observed["model"] == candidate.model
            }
            _ => false,
        };
        if matches!(self.intent.target, Target::Pool { .. })
            && (already || was_tried(&self.failure) || was_tried(&self.first_failure))
        {
            return Ok(Some("member_already_tried".into()));
        }
        if self.request.purpose == Purpose::Summary
            && candidate.route.as_ref().map(|route| &route.route)
                != self
                    .request
                    .retained_route
                    .as_ref()
                    .map(|route| &route.route)
        {
            return Ok(Some("candidate_purpose".into()));
        }
        if candidate.route.is_some() || self.request.retained_route.is_some() {
            let reason = crate::tools::capability_service::recovery_candidate_reason(
                self.engine,
                conn,
                self.request.node,
                self.request.purpose,
                self.callers,
                &candidate,
                (leaf, path),
            )
            .await?;
            if reason.is_none()
                && let Some(demand) = &self.request.demand
                && let Some(offering) = candidate
                    .route
                    .as_ref()
                    .and_then(|route| route.offering.as_ref())
            {
                let model = orch_db::queries::resources::thread_engine::model(
                    conn,
                    offering.provider,
                    &offering.offering,
                )
                .await?;
                let Some(model) = model.filter(|model| model.enabled) else {
                    return Ok(Some("candidate_unavailable".into()));
                };
                if !demand.fits_model(&model, None)? {
                    return Ok(Some("candidate_parameters".into()));
                }
            }
            return Ok(reason);
        }
        if self.request.purpose == Purpose::Embedding
            && (candidate.model != self.retained_model
                || candidate.dimensions != self.intent.dimensions)
        {
            return Ok(Some("embedding_identity".into()));
        }
        for caller in self.callers {
            let pool = crate::resource_choice::ProviderSelection::prospective(
                leaf,
                path,
                provider_purpose(self.request.purpose),
                caller,
            );
            if !self
                .engine
                .candidate_allowed_for(
                    conn,
                    self.request.node,
                    caller,
                    &candidate,
                    self.request.purpose,
                    pool.as_ref(),
                )
                .await?
            {
                return Ok(Some("candidate_not_allowed".into()));
            }
        }
        use orch_db::queries::resources::thread_engine as resources;
        let Some(provider) = resources::provider(conn, candidate.provider_id)
            .await?
            .filter(|provider| {
                provider.deleted_at.is_none()
                    && crate::providers::request_status_available(&provider.status)
            })
        else {
            return Ok(Some("candidate_unavailable".into()));
        };
        let Some(model) = resources::model(conn, candidate.provider_id, &candidate.model)
            .await?
            .filter(|model| model.enabled)
        else {
            return Ok(Some("candidate_unavailable".into()));
        };
        if let Some(demand) = &self.request.demand {
            let Some(protocol) = crate::summaries::resource::recovery_protocol(
                &provider.protocols,
                demand.protocol.as_deref().unwrap_or_default(),
            )?
            else {
                return Ok(Some("candidate_unavailable".into()));
            };
            if !crate::summaries::effort_is_offered(&candidate.params, &model.parameters)
                || !demand.fits_model(&model, Some(&protocol))?
            {
                return Ok(Some("candidate_parameters".into()));
            }
        }
        if self.request.purpose != Purpose::Agent
            && (provider.protocols.is_empty()
                || orch_provider_contract::calls::completion_options(&candidate.params).is_err())
        {
            return Ok(Some("candidate_parameters".into()));
        }
        if self.request.purpose == Purpose::Agent && candidate.agent_id.is_none() {
            let Some(thread) = threads::thread(conn, self.request.node).await? else {
                return Ok(Some("candidate_unavailable".into()));
            };
            if !candidate_parameters_ready(conn, &thread, &candidate).await? {
                return Ok(Some("candidate_parameters".into()));
            }
        }
        Ok(None)
    }
}

impl Engine {
    /// Actual native callers and a live recovery row bind the accepted reservation. No HTTP-supplied
    /// caller or context can stand in for the interrupted source. Replaying its attempt never redraws.
    pub(crate) async fn resolve_recovery_candidate(
        &self,
        conn: &mut Conn,
        request: &DestinationRequest,
        callers: &[Caller],
        intent: &Destination,
        retained_model: &str,
    ) -> Result<CandidateResolution, EngineError> {
        let Some(first) = callers.first() else {
            return Ok(CandidateResolution::rejected("candidate_not_allowed"));
        };
        let Some(row) = read_root(conn, request.node, request.purpose, request.root)
            .await?
            .filter(|row| matches!(row.state.as_str(), "scheduled" | "running" | "switching"))
        else {
            return Ok(CandidateResolution::rejected("candidate_not_allowed"));
        };
        if !matches!(step(&row), Some(Step::Failover { candidate }) if candidate == *intent)
            && row
                .context
                .get("candidate")
                .and_then(|value| serde_json::from_value::<SelectedCandidate>(value.clone()).ok())
                .is_none_or(|candidate| candidate.intent != *intent)
        {
            return Ok(CandidateResolution::rejected("candidate_not_allowed"));
        }
        let original = id(&row.context, "root_input").unwrap_or(request.root);
        let chain = crate::ids::derived(
            "pool.recovery.chain",
            &[original.as_bytes(), row.rule.to_string().as_bytes()],
        );
        if let Target::Pool { id, .. } = &intent.target {
            let used: i64 = sqlx::query_scalar("SELECT count(*) FROM pool_choices WHERE source->>'chain'=$1 AND state='selected' AND id<>$2").bind(chain.to_string()).bind(request.attempt).fetch_one(&mut *conn).await.map_err(DbError::from)?;
            if pool_attempts(conn, *id)
                .await?
                .is_some_and(|limit| u64::try_from(used).map_or(true, |used| used >= limit))
            {
                return Ok(CandidateResolution::rejected("pool_exhausted"));
            }
        }
        let native_leaf: Option<Json> = sqlx::query_scalar("SELECT DISTINCT c.leaf FROM pool_attempt_scopes a JOIN pool_choices c ON c.id=a.choice_id WHERE a.attempt_id=$1 AND c.leaf IS NOT NULL")
            .bind(row.reference).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
        let failed = json!({"provider_id":row.context["provider_id"],"model":row.context["model"],"capability_route":row.context["capability_route"],"leaf":native_leaf});
        let first_failure: Option<Json> = sqlx::query_scalar("SELECT source->'first_failed' FROM pool_choices WHERE source->>'chain'=$1 AND state='selected' ORDER BY created_at,id LIMIT 1")
            .bind(chain.to_string()).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
        let first_failure = first_failure.unwrap_or_else(|| failed.clone());
        let principal = Principal::load(conn, first.clone(), self.inner.clock.time()).await?;
        let choice = Database::new(conn, &principal, Facts { engine: self, request, intent, retained_model, callers, chain, failure: failed, first_failure: first_failure.clone(), rates: None }).reserve(&ChoiceRequest {
            id: request.attempt, target: intent.target.clone(), source: json!({"kind":"call_recovery","node":request.node,"purpose":request.purpose,"root":request.root,"reference":row.reference,"cursor":row.cursor,"chain":chain,"first_failed":first_failure}),
        }).await?;
        let selected = choice.leaf.as_ref().and_then(|leaf| {
            SelectedCandidate::from_leaf(
                intent,
                leaf,
                retained_model,
                request.retained_route.as_ref(),
                Some(choice.id),
            )
        });
        Ok(CandidateResolution {
            selected,
            reason: if matches!(intent.target, Target::Pool { .. }) {
                "pool_exhausted".into()
            } else {
                choice
                    .reason
                    .unwrap_or_else(|| "candidate_unavailable".into())
            },
        })
    }
}
