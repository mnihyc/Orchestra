//! A paid automatic field selects one eligible backend under its stable native call id.
use super::{Choice, Engine, EngineError, SummaryRequest, budget, internal};
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
use orch_summarizer_contract::{descriptor::SummaryField, summarizer::SummarizeError};
use orch_summarizer_model::{Model, descriptor::Config};
use orch_types::{Id, Json};
use serde_json::json;

#[derive(Clone, Copy)]
enum Material<'a> {
    Field(&'a SummaryRequest),
    Goal(&'a super::goal_check::PoolInput),
    Resource {
        request: &'a SummaryRequest,
        input_budget: u32,
        issuer: Option<Id>,
    },
}

async fn fit_field(
    conn: &mut Conn,
    request: &mut SummaryRequest,
    config: &Config,
) -> Result<(), EngineError> {
    if request.field == SummaryField::Handoff {
        let room = super::handoff::member_room(conn, request.node.id, config).await?;
        super::handoff::fit_model(request, config, room)?;
    }
    Ok(())
}

struct Candidates<'a> {
    engine: &'a Engine,
    pool: Id,
    node: Id,
    authored: &'a Json,
    material: Material<'a>,
    rates: Option<crate::rates::RateBook>,
}

pub(super) fn backend(leaf: &Leaf) -> Option<BackendRef> {
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

/// The native summary adapter accepts an unconfigured generation offering. Other
/// routes keep their complete backend identity, including delegated placement.
pub(crate) fn matches_backend(leaf: &Leaf, actual: &BackendRef) -> bool {
    let Some(expected) = backend(leaf) else {
        return false;
    };
    let empty =
        |value: &Json| value.is_null() || value.as_object().is_some_and(serde_json::Map::is_empty);
    expected.route == actual.route
        && expected.offering == actual.offering
        && expected.extensions == actual.extensions
        && (expected.config == actual.config || empty(&expected.config) && empty(&actual.config))
}

impl Candidates<'_> {
    fn candidate(&self, leaf: &Leaf) -> Option<SelectedCandidate> {
        let backend = backend(leaf)?;
        let offering = backend.offering.as_ref()?;
        let params = self
            .authored
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
        let mut config =
            Config::with_pool_options(self.authored, backend).map_err(EngineError::unreadable)?;
        let field = match self.material {
            Material::Field(request) | Material::Resource { request, .. } => Some(request.field),
            Material::Goal(_) => None,
        };
        if let Material::Resource { input_budget, .. } = self.material {
            config.context_tokens = input_budget;
        }
        config.context_tokens = budget::member_context(conn, &config, field).await?;
        if let Material::Resource { input_budget, .. } = self.material
            && config.context_tokens < input_budget
        {
            return Err(EngineError::unreadable(
                "This member cannot hold the accepted source window.",
            ));
        }
        Ok(config)
    }

    async fn rendered(
        &self,
        conn: &mut Conn,
        config: &Config,
    ) -> Result<(String, String, i64), EngineError> {
        let reasons = budget::reasons(conn, config).await?;
        match self.material {
            Material::Field(request) | Material::Resource { request, .. } => {
                let mut request = request.clone();
                fit_field(conn, &mut request, config).await?;
                let (system, input) =
                    Model::render(&mut request, config).map_err(EngineError::unreadable)?;
                Ok((system, input, config.output_reserve(request.field, reasons)))
            }
            Material::Goal(goal) => {
                let output = config.goal_output_reserve(reasons);
                let input = goal.fit(conn, self.node, config, output).await?;
                Ok((goal.system.to_owned(), input.text, output))
            }
        }
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
        let node = self.node;
        if crate::provider_policy::forbidden(conn, node, provider.id)
            .await?
            .is_some()
            || crate::resolved::load(conn, node)
                .await?
                .models_allowed
                .as_ref()
                .is_some_and(|patterns| {
                    !crate::models::allowed_by(patterns, &model.model_name, &model.groups)
                })
        {
            return Ok(Some("candidate_not_allowed".into()));
        }
        let protocol = if candidate.route.is_none() {
            let Some(protocol) = super::resource::completion_protocol(&provider.protocols)? else {
                return Ok(Some("candidate_purpose".into()));
            };
            let Some(keeper) = provider.key_machine_id else {
                return Ok(Some("candidate_unavailable".into()));
            };
            if !orch_db::queries::resources::machine(conn, keeper)
                .await?
                .is_some_and(|machine| machine.online)
            {
                return Ok(Some("candidate_unavailable".into()));
            }
            Some(protocol)
        } else {
            if let Some(reason) = crate::tools::capability_service::recovery_candidate_reason(
                self.engine,
                conn,
                self.node,
                Purpose::Summary,
                std::slice::from_ref(principal.caller()),
                &candidate,
                (leaf, path),
            )
            .await?
            {
                return Ok(Some(reason));
            }
            None
        };
        let (parameters, purpose) = orch_provider_contract::models::composed_model_with_purpose(
            &model.model_name,
            &model.parameters,
            &model.groups,
            &model.option_layers,
            protocol.as_deref(),
        )?;
        if !purpose.supports_purpose(orch_provider_contract::models::ModelPurpose::Generation) {
            return Ok(Some("candidate_purpose".into()));
        }
        let (params, tier) = if let Some(protocol) = protocol {
            let defaults =
                resources::provider_request_defaults(conn, provider.id, &protocol).await?;
            let Ok(params) = orch_model_contract::parameters::resolve(
                &protocol,
                &parameters,
                &defaults,
                &candidate.params,
            ) else {
                return Ok(Some("candidate_parameters".into()));
            };
            let tier = orch_model_contract::parameters::requested_tier_for_caps(&protocol, &params);
            (params, tier)
        } else {
            let Some(agent) = provider.agent_id else {
                return Ok(Some("candidate_unavailable".into()));
            };
            let Some(agent) = resources::agent(conn, agent).await? else {
                return Ok(Some("candidate_unavailable".into()));
            };
            let Some(descriptor) = crate::harnesses::descriptor_of(&agent)? else {
                return Ok(Some("candidate_unavailable".into()));
            };
            let tier = crate::providers::service_tier(&descriptor.parameters, &candidate.params);
            (candidate.params.clone(), tier)
        };
        let issuer = match self.material {
            Material::Resource { issuer, .. } => issuer,
            _ => None,
        };
        if !self
            .engine
            .room_for_own_use(
                conn,
                &crate::caps::OwnCapAsk {
                    place: node,
                    provider: provider.id,
                    model: &candidate.model,
                    tier: tier.as_deref(),
                    purpose: "summary",
                    person: issuer,
                },
            )
            .await?
        {
            return Ok(Some("cap_no_room".into()));
        }
        let selection =
            ProviderSelection::prospective(leaf, path, UsagePurpose::Summary, principal.caller());
        let Ok(effort) = orch_provider_contract::calls::requested_effort(&params) else {
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
                    node: self.node,
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
        let config = match self.config(conn, leaf).await {
            Ok(config) => config,
            Err(EngineError::Unreadable(_)) => return Ok(Some("candidate_parameters".into())),
            Err(error) => return Err(error),
        };
        if matches!(self.material, Material::Resource { .. })
            && super::resource::judge_model_at(
                conn,
                principal,
                Some(node),
                &config,
                selection.as_ref(),
            )
            .await?
            .is_err()
        {
            return Ok(Some("candidate_not_allowed".into()));
        }
        match self.rendered(conn, &config).await {
            Ok(_) => Ok(None),
            Err(EngineError::Unreadable(_)) => Ok(Some("candidate_parameters".into())),
            Err(error) => Err(error),
        }
    }
    async fn measurements(
        &mut self,
        conn: &mut Conn,
        _: &Principal,
        leaf: &Leaf,
        _: &[Hop],
        _: bool,
    ) -> Result<Measurements, EngineError> {
        let Some(candidate) = self.candidate(leaf) else {
            return Ok(Measurements::default());
        };
        let config = self.config(conn, leaf).await?;
        let (system, input, output) = self.rendered(conn, &config).await?;
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
            output,
            Some(&config.params),
            protocol.as_deref(),
        );
        crate::calls::summary_measurements(
            conn,
            self.node,
            &candidate,
            &demand,
            false,
            &mut self.rates,
        )
        .await
    }
}

/// The fork planner may offer this producer without spending its rotation or claiming a trial.
pub(super) async fn handoff_preview(
    engine: &Engine,
    conn: &mut Conn,
    choice: &Choice,
    thread: Id,
    pool: Id,
) -> Result<Option<Config>, EngineError> {
    let Some(writer) = choice.writer else {
        return Ok(None);
    };
    let principal = Principal::load(
        conn,
        Caller::session(writer).with_admin_mode(false),
        engine.inner.clock.time(),
    )
    .await?;
    let target = orch_api_contract::summaries::SummaryTarget::Thread { id: thread };
    if !super::sources::readable(conn, &principal, target)
        .await
        .map_err(EngineError::unreadable)?
    {
        return Ok(None);
    }
    let field = SummaryField::Handoff;
    let request = SummaryRequest {
        field,
        node: orch_plugin_contract::refs::NodeRef {
            id: thread,
            kind: orch_plugin_contract::refs::NodeKind::Thread,
            name: None,
            redacted: false,
        },
        previous: String::new(),
        activity: Vec::new(),
        tag_choices: Vec::new(),
        limits: orch_summarizer_contract::summarizer::SummaryLimits::for_field(field, 0),
        config: choice.config.clone(),
    };
    let mut resolver = crate::resource_choice::Resolver::new(
        Database::new(
            conn,
            &principal,
            Candidates {
                engine,
                pool,
                node: thread,
                authored: &choice.config,
                material: Material::Field(&request),
                rates: None,
            },
        ),
        &Target::Pool {
            id: pool,
            kind: Kind::Model,
        },
        Kind::Model,
    )
    .await?;
    while let Some(event) = resolver.next().await? {
        if let crate::resource_choice::Event::Candidate {
            leaf, reason: None, ..
        } = event
        {
            // Use only the authored counts in the public estimate. The member and its rate remain undecided.
            let Some(backend) = backend(&leaf) else {
                continue;
            };
            return Config::with_pool_options(&choice.config, backend)
                .map(Some)
                .map_err(EngineError::unreadable);
        }
    }
    Ok(None)
}

pub(super) async fn reserve(
    engine: &Engine,
    choice: &Choice,
    request: &mut SummaryRequest,
    field: &Field,
    cut: Option<Id>,
) -> Result<Config, SummarizeError> {
    let mut tx = engine.db().begin().await.map_err(internal)?;
    let config = select(
        engine,
        &mut tx,
        choice,
        Material::Field(request),
        field,
        cut,
    )
    .await?;
    fit_field(&mut tx, request, &config)
        .await
        .map_err(internal)?;
    tx.commit().await.map_err(DbError::from).map_err(internal)?;
    Ok(config)
}

/// The resource request already exists in this transaction. The draw and its exact backend
/// become visible with that request, and its client-id replay never reaches this path.
pub(super) async fn reserve_resource(
    engine: &Engine,
    conn: &mut Conn,
    principal: &Principal,
    row: &orch_db::queries::summaries::Request,
    selected: &super::resource::Selection,
    pool: Id,
    authored: &Json,
    source: Json,
) -> Result<Config, EngineError> {
    let request = super::resource::request_for_selection(conn, selected).await?;
    let candidates = || Candidates {
        engine,
        pool,
        node: selected.material.identity.place(),
        authored,
        material: Material::Resource {
            request: &request,
            input_budget: selected.config.context_tokens,
            issuer: row.requested_by,
        },
        rates: None,
    };
    let choice = Database::new(conn, principal, candidates())
        .reserve(&ChoiceRequest {
            id: row.id,
            target: Target::Pool {
                id: pool,
                kind: Kind::Model,
            },
            source,
        })
        .await?;
    let leaf = choice.leaf.as_ref().ok_or_else(super::resource::refused)?;
    candidates().config(conn, leaf).await
}

pub(super) async fn reserve_goal(
    engine: &Engine,
    choice: &Choice,
    input: &super::goal_check::PoolInput,
    field: &Field,
    report: Id,
) -> Result<(Config, super::goal_check::FittedInput), SummarizeError> {
    let mut tx = engine.db().begin().await.map_err(internal)?;
    let config = select(
        engine,
        &mut tx,
        choice,
        Material::Goal(input),
        field,
        Some(report),
    )
    .await?;
    let reasons = budget::reasons(&mut tx, &config).await.map_err(internal)?;
    let fitted = input
        .fit(
            &mut tx,
            field.node,
            &config,
            config.goal_output_reserve(reasons),
        )
        .await
        .map_err(internal)?;
    tx.commit().await.map_err(DbError::from).map_err(internal)?;
    Ok((config, fitted))
}

async fn select(
    engine: &Engine,
    conn: &mut Conn,
    choice: &Choice,
    material: Material<'_>,
    field: &Field,
    source_id: Option<Id>,
) -> Result<Config, SummarizeError> {
    let pool = field.pool;
    let run = field.workflow;
    let refused =
        || SummarizeError::Skipped("The summary model pool has no eligible member.".into());
    let writer = choice.writer.ok_or_else(refused)?;
    orch_db::queries::transaction_guard::lock_action(conn)
        .await
        .map_err(internal)?;
    let node = field.node;
    if super::choice(conn, node).await.map_err(internal)?.as_ref() != Some(choice) {
        return Err(refused());
    }
    if matches!(material, Material::Field(request) if request.field != SummaryField::Handoff) {
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM node_summary_state WHERE node_id=$1 AND run_id=$2 AND claimed_until>$3)")
        .bind(node).bind(run).bind(engine.inner.clock.now()).fetch_one(&mut *conn).await.map_err(DbError::from).map_err(internal)?;
        if !active {
            return Err(refused());
        }
    }
    let principal = Principal::load(
        conn,
        Caller::session(writer).with_admin_mode(false),
        engine.inner.clock.time(),
    )
    .await
    .map_err(internal)?;
    let target = Target::Pool {
        id: pool,
        kind: Kind::Model,
    };
    let origin = field
        .source(conn, &principal, choice, source_id)
        .await
        .map_err(internal)?;
    if let Material::Goal(input) = material
        && (origin["evidence"] != json!(input.digest) || origin["history"] != json!(input.history))
    {
        return Err(refused());
    }
    let candidates = || Candidates {
        engine,
        pool,
        node,
        authored: &choice.config,
        material,
        rates: None,
    };
    let selected = Database::new(&mut *conn, &principal, candidates())
        .reserve(&ChoiceRequest {
            id: field.call,
            target,
            source: origin,
        })
        .await
        .map_err(internal)?;
    let leaf = selected.leaf.as_ref().ok_or_else(refused)?;
    candidates().config(conn, leaf).await.map_err(internal)
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
    /// Reopen the paid workflow and source under the setting writer.
    async fn source(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        choice: &Choice,
        source_id: Option<Id>,
    ) -> Result<Json, EngineError> {
        let mut origin = source(choice, self.node, self.workflow, &self.task)?;
        if matches!(self.task.as_str(), "handoff" | "goal_check") {
            let unavailable =
                || EngineError::unreadable("The summary conversation is no longer available.");
            let target = orch_api_contract::summaries::SummaryTarget::Thread { id: self.node };
            if !super::sources::readable(conn, principal, target)
                .await
                .map_err(EngineError::unreadable)?
            {
                return Err(unavailable());
            }
            let row = orch_db::queries::threads::thread(conn, self.node)
                .await?
                .ok_or_else(unavailable)?;
            if self.task == "goal_check" {
                use orch_db::queries::goals;
                let report = goals::report(conn, source_id.ok_or_else(unavailable)?)
                    .await?
                    .ok_or_else(unavailable)?;
                let goal = goals::goal(conn, self.workflow)
                    .await?
                    .ok_or_else(unavailable)?;
                if report.goal_id != goal.id
                    || report.thread_id != self.node
                    || goal.thread_id != self.node
                    || goal.status != goals::status::ACTIVE
                    || report.status != "met"
                    || report.outcome.is_some()
                    || goal
                        .checking_until
                        .is_none_or(|until| until <= principal.now().get())
                    || self.call != super::goal_check::call_id(report.id)
                {
                    return Err(unavailable());
                }
                origin["report"] = json!(report.id);
                origin["evidence"] = json!(super::goal_check::digest(&goal, &report)?);
            } else if let Some(id) = source_id {
                let cut = orch_db::queries::thread_history_cuts::cut(conn, id)
                    .await?
                    .ok_or_else(unavailable)?;
                if cut.thread_id != self.node
                    || (cut.job_id != Some(self.workflow) && cut.id != self.workflow)
                    || cut.continuation_id != row.active_continuation_id
                    || cut.execution_revision != row.execution_revision
                {
                    return Err(unavailable());
                }
                origin["cut"] = json!(cut.id);
            }
            origin["history"] = json!([row.active_continuation_id, row.execution_revision]);
        }
        Ok(origin)
    }

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
        let kept: Json = sqlx::query_scalar(
            "SELECT source FROM pool_choices WHERE id=$1 AND actor_id=$2 AND state='selected'",
        )
        .bind(self.call)
        .bind(principal.user())
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?
        .ok_or_else(unavailable)?;
        let source_id = kept
            .get(if self.task == "goal_check" {
                "report"
            } else {
                "cut"
            })
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()?;
        if kept != self.source(conn, principal, &choice, source_id).await? {
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
    Ok(
        contribution(conn, principal, node, selection, actual_backend, params)
            .await?
            .is_some(),
    )
}

pub(super) async fn contribution(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    selection: &ProviderSelection,
    actual_backend: &BackendRef,
    params: Option<&Json>,
) -> Result<Option<orch_authz::pools::Contribution>, EngineError> {
    let selected = selection.selection();
    if !matches_backend(selected.leaf, actual_backend) {
        return Ok(None);
    }
    let Some(offering) = actual_backend.offering.as_ref() else {
        return Ok(None);
    };
    let Some(model) = resources::model(conn, offering.provider, &offering.offering)
        .await?
        .filter(|m| m.enabled)
    else {
        return Ok(None);
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
    orch_authz::pools::provider_choice(
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
    .map_err(EngineError::unreadable)
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
