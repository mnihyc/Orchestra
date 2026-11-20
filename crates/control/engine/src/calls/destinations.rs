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
struct Facts<'a> {
    engine: &'a Engine,
    request: &'a DestinationRequest,
    intent: &'a Destination,
    retained_model: &'a str,
    callers: &'a [Caller],
    chain: Id,
    failure: Json,
    first_failure: Json,
}
#[async_trait::async_trait]
impl Requirements for Facts<'_> {
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
        if candidate.route.is_some() || self.request.retained_route.is_some() {
            return crate::tools::capability_service::recovery_candidate_reason(
                self.engine,
                conn,
                self.request.node,
                self.request.purpose,
                self.callers,
                &candidate,
                (leaf, path),
            )
            .await;
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
        if resources::model(conn, candidate.provider_id, &candidate.model)
            .await?
            .is_none_or(|model| !model.enabled)
        {
            return Ok(Some("candidate_unavailable".into()));
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
        let choice = Database::new(conn, &principal, Facts { engine: self, request, intent, retained_model, callers, chain, failure: failed, first_failure: first_failure.clone() }).reserve(&ChoiceRequest {
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
