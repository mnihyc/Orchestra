//! A paid automatic field selects one eligible backend under its stable native call id.
use super::{Choice, Engine, EngineError, SummaryRequest, budget, field_name, internal};
use crate::calls::{RequestDemand, SelectedCandidate};
use crate::resource_choice::{
    ChoiceRequest, Hop, Measurements, ProviderSelection,
    database::{Database, Requirements},
};
use orch_access_contract::permissions::UsagePurpose;
use orch_authz::{Caller, Principal};
use orch_db::{Conn, DbError, queries::resources::thread_engine as resources};
use orch_provider_contract::{
    calls::{Candidate, Purpose},
    capabilities::BackendRef,
    pools::{Kind, Leaf, Target},
};
use orch_summarizer_contract::summarizer::SummarizeError;
use orch_summarizer_model::{Model, descriptor::Config};
use orch_types::{Id, Json};
use serde_json::json;

struct Candidates<'a> {
    engine: &'a Engine,
    pool: Id,
    request: &'a SummaryRequest,
    rates: Option<crate::rates::RateBook>,
}

fn backend(leaf: &Leaf) -> Option<BackendRef> {
    let Leaf::Model { backend } = leaf else {
        return None;
    };
    backend.offering.as_ref()?;
    let mut backend = backend.clone();
    match backend.route.as_str() {
        "summary.delegation" | orch_summarizer_model::capabilities::ROUTE => Some(backend),
        "generation.model"
            if backend.extensions.is_empty()
                && (backend.config.is_null()
                    || backend
                        .config
                        .as_object()
                        .is_some_and(serde_json::Map::is_empty)) =>
        {
            backend.route = orch_summarizer_model::capabilities::ROUTE.into();
            Some(backend)
        }
        _ => None,
    }
}

impl Candidates<'_> {
    fn candidate(&self, leaf: &Leaf) -> Option<SelectedCandidate> {
        let backend = backend(leaf)?;
        let offering = backend.offering.as_ref()?;
        let params = self
            .request
            .config
            .get("params")
            .cloned()
            .unwrap_or_else(|| json!({}));
        Some(SelectedCandidate {
            intent: Candidate {
                target: Target::Pool {
                    id: self.pool,
                    kind: Kind::Model,
                },
                agent_id: None,
                params: params.clone(),
                dimensions: None,
            },
            choice_id: None,
            provider_id: offering.provider,
            model: offering.offering.clone(),
            route: (backend.route == "summary.delegation").then_some(backend),
            params,
            agent_id: None,
            dimensions: None,
        })
    }

    async fn config(&self, conn: &mut Conn, leaf: &Leaf) -> Result<Config, EngineError> {
        let backend = backend(leaf).ok_or_else(|| EngineError::unreadable("No summary backend"))?;
        let mut config = Config::for_pool_member(&self.request.config, backend)
            .map_err(EngineError::unreadable)?;
        config.context_tokens = budget::context_tokens(conn, &config, self.request.field).await?;
        Ok(config)
    }
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
        let Some(candidate) = self.candidate(leaf) else {
            return Ok(Some("candidate_purpose".into()));
        };
        let Some(provider) = resources::provider(conn, candidate.provider_id)
            .await?
            .filter(|p| {
                p.deleted_at.is_none() && crate::providers::request_status_available(&p.status)
            })
        else {
            return Ok(Some("candidate_unavailable".into()));
        };
        let Some(model) = resources::model(conn, provider.id, &candidate.model)
            .await?
            .filter(|m| m.enabled)
        else {
            return Ok(Some("candidate_unavailable".into()));
        };
        if candidate.route.is_some() {
            if let Some(reason) = crate::tools::capability_service::recovery_candidate_reason(
                self.engine,
                conn,
                self.request.node.id,
                Purpose::Summary,
                std::slice::from_ref(principal.caller()),
                &candidate,
                (leaf, path),
            )
            .await?
            {
                return Ok(Some(reason));
            }
        } else if provider.protocols.is_empty()
            || super::resource::completion_protocol(&provider.protocols)?.is_none()
        {
            return Ok(Some("candidate_purpose".into()));
        }
        let selection =
            ProviderSelection::prospective(leaf, path, UsagePurpose::Summary, principal.caller());
        let Ok(effort) = orch_provider_contract::calls::requested_effort(&candidate.params) else {
            return Ok(Some("candidate_parameters".into()));
        };
        let allowed = self
            .engine
            .inner
            .access
            .background_provider_use(
                conn,
                &crate::access::ProviderUse {
                    pool: selection.as_ref(),
                    purpose: UsagePurpose::Summary,
                    person: principal.user(),
                    node: self.request.node.id,
                    provider: provider.id,
                    model: Some(&model.model_name),
                    effort,
                    runs_in: None,
                },
            )
            .await
            .map_err(EngineError::unreadable)?;
        if !allowed.is_allowed() {
            return Ok(Some("candidate_not_allowed".into()));
        }
        let config = self.config(conn, leaf).await?;
        if Model::render(&mut self.request.clone(), &config).is_err() {
            return Ok(Some("candidate_parameters".into()));
        }
        Ok(None)
    }
    async fn measurements(
        &mut self,
        conn: &mut Conn,
        _: &Principal,
        leaf: &Leaf,
        _: &[Hop],
    ) -> Result<Measurements, EngineError> {
        let Some(candidate) = self.candidate(leaf) else {
            return Ok(Measurements::default());
        };
        let config = self.config(conn, leaf).await?;
        let reasons = budget::reasons(conn, &config).await?;
        let (system, input) =
            Model::render(&mut self.request.clone(), &config).map_err(EngineError::unreadable)?;
        let protocol = if candidate.route.is_none() {
            match resources::provider(conn, candidate.provider_id).await? {
                Some(provider) => super::resource::completion_protocol(&provider.protocols)?,
                None => None,
            }
        } else {
            None
        };
        let demand = RequestDemand::summary(
            &system,
            &input,
            config.output_reserve(self.request.field, reasons),
            Some(&config.params),
            protocol.as_deref(),
        );
        crate::calls::summary_measurements(
            conn,
            self.request.node.id,
            &candidate,
            &demand,
            false,
            &mut self.rates,
        )
        .await
    }
}

pub(super) async fn automatic(
    engine: &Engine,
    choice: &Choice,
    request: &SummaryRequest,
    pool: Id,
    run: Id,
    call: Id,
) -> Result<Config, SummarizeError> {
    let refused =
        || SummarizeError::Skipped("The summary model pool has no eligible member.".into());
    let writer = choice.writer.ok_or_else(refused)?;
    let mut tx = engine.db().begin().await.map_err(internal)?;
    orch_db::queries::transaction_guard::lock_action(&mut tx)
        .await
        .map_err(internal)?;
    let node = request.node.id;
    if super::choice(&mut tx, node)
        .await
        .map_err(internal)?
        .as_ref()
        != Some(choice)
    {
        return Err(refused());
    }
    let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM node_summary_state WHERE node_id=$1 AND run_id=$2 AND claimed_until>$3)")
        .bind(node).bind(run).bind(engine.inner.clock.now()).fetch_one(&mut *tx).await.map_err(DbError::from).map_err(internal)?;
    if !active {
        return Err(refused());
    }
    let principal = Principal::load(
        &mut tx,
        Caller::session(writer).with_admin_mode(false),
        engine.inner.clock.time(),
    )
    .await
    .map_err(internal)?;
    let target = Target::Pool {
        id: pool,
        kind: Kind::Model,
    };
    let selected = Database::new(
        &mut tx,
        &principal,
        Candidates {
            engine,
            pool,
            request,
            rates: None,
        },
    )
    .reserve(&ChoiceRequest {
        id: call,
        target,
        source: source(choice, node, run, field_name(request.field)).map_err(internal)?,
    })
    .await
    .map_err(internal)?;
    let leaf = selected.leaf.as_ref().ok_or_else(refused)?;
    let config = Candidates {
        engine,
        pool,
        request,
        rates: None,
    }
    .config(&mut tx, leaf)
    .await
    .map_err(internal)?;
    tx.commit().await.map_err(DbError::from).map_err(internal)?;
    Ok(config)
}

fn source(choice: &Choice, node: Id, workflow: Id, task: &str) -> Result<Json, EngineError> {
    let setting = orch_types::Hash::of(&serde_json::to_vec(&choice.config)?);
    Ok(
        json!({"kind":"automatic_summary","purpose":"summary","node":node,"workflow":workflow,"field":task,"setting":setting}),
    )
}

/// Reopen the actual paid field and its original setting writer before a native effect.
pub(super) struct Field {
    pub node: Id,
    pub workflow: Id,
    pub task: String,
    pub call: Id,
    pub pool: Id,
}

impl Field {
    pub async fn selection(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        provider: Id,
        model: &str,
    ) -> Result<ProviderSelection, EngineError> {
        let unavailable =
            || EngineError::unreadable("The summary pool choice is no longer available.");
        let choice = super::choice(conn, self.node)
            .await?
            .filter(|choice| choice.kind == "model" && choice.writer == Some(principal.user()))
            .ok_or_else(unavailable)?;
        if Config::pool_id(&choice.config).map_err(EngineError::unreadable)? != Some(self.pool) {
            return Err(unavailable());
        }
        let kept: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pool_choices WHERE id=$1 AND actor_id=$2 AND source=$3)",
        )
        .bind(self.call)
        .bind(principal.user())
        .bind(source(&choice, self.node, self.workflow, &self.task)?)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
        if !kept {
            return Err(unavailable());
        }
        let selection = ProviderSelection::load(
            conn,
            self.call,
            self.node,
            provider,
            Some(model),
            UsagePurpose::Summary,
            principal.caller(),
        )
        .await?;
        if !selection.targets(&Target::Pool {
            kind: Kind::Model,
            id: self.pool,
        }) {
            return Err(unavailable());
        }
        Ok(selection)
    }
}

pub(super) async fn allowed(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    selection: &ProviderSelection,
    actual_backend: &BackendRef,
    params: Option<&Json>,
) -> Result<bool, EngineError> {
    let selected = selection.selection();
    if backend(selected.leaf).as_ref() != Some(actual_backend) {
        // The API native command retains the offering and route. Null and empty adapter options are equivalent.
        let Some(mut exact) = backend(selected.leaf) else {
            return Ok(false);
        };
        if exact
            .config
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
        {
            exact.config = Json::Null;
        }
        let mut actual = actual_backend.clone();
        if actual
            .config
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
        {
            actual.config = Json::Null;
        }
        if actual != exact {
            return Ok(false);
        }
    }
    let Some(offering) = actual_backend.offering.as_ref() else {
        return Ok(false);
    };
    let Some(model) = resources::model(conn, offering.provider, &offering.offering)
        .await?
        .filter(|m| m.enabled)
    else {
        return Ok(false);
    };
    let policies =
        crate::access::resource_policies_for_choice(conn, &[offering.provider], selected.path)
            .await?;
    let wanted = orch_authz::options::Wanted {
        purpose: UsagePurpose::Summary,
        model: Some((&offering.offering, &model.groups)),
        effort: params
            .map(orch_provider_contract::calls::requested_effort)
            .transpose()
            .map_err(EngineError::unreadable)?
            .flatten(),
        ..Default::default()
    };
    Ok(orch_authz::pools::provider_choice(
        conn,
        principal,
        selected.path,
        selected.leaf,
        Some(node),
        &wanted,
        orch_access_contract::permissions::ProviderUsePurpose::Work,
        &policies,
    )
    .await
    .map_err(EngineError::unreadable)?
    .is_some())
}

pub(super) struct Admitted {
    pub field: Field,
    pub config: Config,
}

impl Admitted {
    pub async fn publishable(
        &self,
        conn: &mut Conn,
        principal: &Principal,
    ) -> Result<bool, EngineError> {
        let offering = self.config.offering().map_err(EngineError::unreadable)?;
        let selection = match self
            .field
            .selection(conn, principal, offering.provider, &offering.offering)
            .await
        {
            Ok(selected) => selected,
            Err(EngineError::Unreadable(_) | EngineError::BeforeEffect(_)) => return Ok(false),
            Err(error) => return Err(error),
        };
        allowed(
            conn,
            principal,
            self.field.node,
            &selection,
            &self.config.backend,
            Some(&self.config.params),
        )
        .await
    }
}
