//! A native summary producer retains its real standing workflow or admitted resource act.
use super::*;
use orch_authz::Caller;
use orch_db::queries::hierarchy;
use orch_provider_contract::capabilities::{BackendRef, ProgramPlacement};
use orch_types::{Clock, Time};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Source {
    call: Call,
    root: Id,
    workflow: Id,
    task: String,
    request: CompletionRequest,
    resource: Option<super::super::resource::Work>,
    automatic: Option<automatic::Source>,
    deadline: Time,
}

impl Source {
    pub(super) async fn capture(
        conn: &mut Conn,
        host: &Host,
        call: Call,
        root: Id,
        configured: &(Id, String),
        request: CompletionRequest,
    ) -> Result<Self, EngineError> {
        let automatic = if host.resource.is_none() {
            Some(automatic::Source::capture(conn, host, &call, root, configured).await?)
        } else {
            None
        };
        let source = Self {
            deadline: paid::deadline(&call),
            call,
            root,
            workflow: host.workflow,
            task: host.task.into(),
            request,
            resource: host.resource.clone(),
            automatic,
        };
        if source
            .current_callers(conn, host.engine.inner.clock.as_ref())
            .await?
            .is_none()
        {
            return Err(unavailable());
        }
        Ok(source)
    }
    pub(crate) fn request_id(&self) -> Id {
        self.resource
            .as_ref()
            .map_or(self.root, super::super::resource::Work::request)
    }
    pub(crate) fn call_id(&self) -> Id {
        self.call.id
    }
    pub(crate) fn node(&self) -> Id {
        self.call.node_id
    }
    pub(crate) fn thread(&self) -> Option<Id> {
        self.call.thread_id
    }
    pub(crate) fn actor(&self) -> Result<Id, EngineError> {
        self.call.person_id.ok_or_else(unavailable)
    }
    pub(crate) fn person(&self) -> Option<Id> {
        self.call.issued_by
    }
    pub(crate) fn backend(&self) -> &BackendRef {
        &self.request.backend
    }
    pub(crate) fn input(&self) -> Json {
        json!({"system":self.request.system,"input":self.request.input,"max_output_tokens":self.request.max_output_tokens})
    }
    pub(crate) fn effort(&self) -> Result<Option<String>, EngineError> {
        self.request
            .params
            .as_ref()
            .map(orch_provider_contract::calls::requested_effort)
            .transpose()
            .map(|effort| effort.flatten().map(str::to_owned))
            .map_err(EngineError::unreadable)
    }
    async fn retained_source(conn: &mut Conn, call: Id) -> Result<Option<Self>, EngineError> {
        let stored: Option<Json> = sqlx::query_scalar("SELECT delegated_program#>'{origin,source}' FROM summary_calls WHERE id=$1 AND purpose='summary'")
            .bind(call).fetch_optional(conn).await.map_err(DbError::from)?.flatten();
        stored
            .map(serde_json::from_value)
            .transpose()
            .map_err(EngineError::from)
    }
    pub(crate) async fn retained_request(
        conn: &mut Conn,
        call: Id,
    ) -> Result<CompletionRequest, EngineError> {
        let source: Json = sqlx::query_scalar("SELECT delegated_program#>'{origin,source}' FROM summary_calls WHERE id=$1 AND purpose='summary' AND delegated_program IS NOT NULL")
            .bind(call).fetch_one(conn).await.map_err(DbError::from)?;
        let source: Source = serde_json::from_value(source)?;
        if source.call.id != call {
            return Err(unavailable());
        }
        Ok(source.request)
    }
    pub(crate) fn deadline(&self) -> Time {
        self.deadline
    }

    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        match (&self.resource, &self.automatic) {
            (Some(work), None) => work.lock_source(conn).await?,
            (None, Some(source)) => source.lock_source(conn).await?,
            _ => return Err(unavailable()),
        }
        sqlx::query("SELECT id FROM summary_calls WHERE id=$1 FOR UPDATE")
            .bind(self.call.id)
            .fetch_optional(conn)
            .await
            .map_err(DbError::from)?
            .ok_or_else(unavailable)?;
        Ok(())
    }

    /// Re-enter only the actual producer; ids and stored JSON supply no independent caller.
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<Caller>>, EngineError> {
        if clock.time() >= self.deadline {
            return Ok(None);
        }
        let kept: Option<Call> = sqlx::query_as("SELECT id,node_id,topic_id,thread_id,provider_id,person_id,issued_by,entry_id,model_name,created_at FROM summary_calls WHERE id=$1 AND purpose='summary' AND cancel_reason IS NULL")
            .bind(self.call.id).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
        if kept.as_ref() != Some(&self.call) {
            return Ok(None);
        }
        let backend: Option<Json> =
            sqlx::query_scalar("SELECT program_backend FROM summary_calls WHERE id=$1")
                .bind(self.call.id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?
                .flatten();
        if backend.as_ref() != Some(&serde_json::to_value(&self.request.backend)?) {
            return Ok(None);
        }

        let Some(offering) = self.request.backend.offering.as_ref() else {
            return Ok(None);
        };
        if offering.provider != self.call.provider_id || offering.offering != self.call.model_name {
            return Ok(None);
        }
        let callers = match (&self.resource, &self.automatic) {
            (Some(work), None) => {
                let frame = match work.frame(conn, clock).await {
                    Ok(frame) => frame,
                    Err(EngineError::BeforeEffect(_)) => return Ok(None),
                    Err(error) => return Err(error),
                };
                if frame.principal().user() != self.actor()?
                    || frame.value().config.backend != self.request.backend
                {
                    return Ok(None);
                }
                vec![frame.principal().caller().clone()]
            }
            (None, Some(source)) => {
                let Some(callers) = source
                    .current_completion(conn, &self.request, clock, true)
                    .await?
                else {
                    return Ok(None);
                };
                callers
                    .into_iter()
                    .map(|(caller, _)| caller.ok_or_else(unavailable))
                    .collect::<Result<Vec<_>, _>>()?
            }
            _ => return Ok(None),
        };
        let thread = threads::thread(conn, self.call.node_id).await?;
        let original = json!({"system":self.request.system,"input":self.request.input});
        if crate::raw_events::mask_summary(
            conn,
            self.call.node_id,
            self.call.topic_id,
            thread.and_then(|thread| thread.environment_id),
            original.clone(),
        )
        .await?
        .as_ref()
            != Some(&original)
        {
            return Ok(None);
        }
        Ok(Some(callers))
    }

    pub(crate) async fn placement(
        &self,
        conn: &mut Conn,
    ) -> Result<crate::tools::capability_service::delegation::placement::Placement, EngineError>
    {
        use crate::tools::capability_service::delegation::placement::Placement;
        if let Some(thread) = self.call.thread_id {
            let thread = threads::thread(conn, thread)
                .await?
                .ok_or_else(unavailable)?;
            return Placement::for_thread(conn, &thread).await;
        }
        let nodes = hierarchy::with_ancestors(conn, &[self.call.node_id]).await?;
        let source = nodes
            .iter()
            .find(|node| node.id == self.call.node_id)
            .ok_or_else(unavailable)?;
        let topic = nodes
            .iter()
            .rev()
            .find(|node| node.kind == "topic")
            .map(|node| node.id);
        let configured = ProgramPlacement::read(&self.request.backend.config)
            .map_err(EngineError::unreadable)?;
        Placement::configured(conn, source.org_id, topic, &configured).await
    }

    pub(crate) async fn retain(&self, engine: &Engine, value: &Json) -> Result<Json, EngineError> {
        let mut tx = engine.db().begin().await?;
        self.lock_source(&mut tx).await?;
        if self
            .current_callers(&mut tx, engine.inner.clock.as_ref())
            .await?
            .is_none()
        {
            return Err(unavailable());
        }
        let saved: Json = sqlx::query_scalar("UPDATE summary_calls SET delegated_program=COALESCE(delegated_program,$2) WHERE id=$1 AND cancel_reason IS NULL RETURNING delegated_program")
            .bind(self.call.id).bind(value).fetch_one(&mut *tx).await.map_err(DbError::from)?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(saved)
    }
    pub(crate) async fn result(&self, engine: &Engine, value: &Json) -> Result<(), EngineError> {
        let mut tx = engine.db().begin().await?;
        self.lock_source(&mut tx).await?;
        if self
            .current_callers(&mut tx, engine.inner.clock.as_ref())
            .await?
            .is_none()
        {
            return Err(unavailable());
        }
        let wrote = sqlx::query("UPDATE summary_calls SET delegated_result=COALESCE(delegated_result,$2) WHERE id=$1 AND cancel_reason IS NULL AND (delegated_result IS NULL OR delegated_result=$2)")
            .bind(self.call.id).bind(value).execute(&mut *tx).await.map_err(DbError::from)?.rows_affected();
        if wrote != 1 {
            return Err(unavailable());
        }
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }
}

fn unavailable() -> EngineError {
    EngineError::BeforeEffect(Box::new(crate::operation_scope::continuing::Error::Refused))
}

impl Host {
    pub(super) async fn complete_program(
        &self,
        node: Id,
        mut request: CompletionRequest,
    ) -> Result<CompletionAnswer, SummarizeError> {
        let writer = self.writer.ok_or_else(|| {
            SummarizeError::Skipped("No summarizer setting writer is recorded.".into())
        })?;
        request
            .backend
            .validate()
            .map_err(|reason| SummarizeError::Config(reason.into()))?;
        let original = request
            .backend
            .offering
            .as_ref()
            .ok_or_else(|| SummarizeError::Config("Choose a model offering.".into()))?;
        let configured = (original.provider, original.offering.clone());
        let mut tx = self.engine.db().begin().await.map_err(internal)?;
        orch_db::queries::transaction_guard::lock_action(&mut tx)
            .await
            .map_err(internal)?;
        let plan = crate::calls::plan_service(
            &mut tx,
            &crate::calls::ServiceCall {
                node,
                purpose: orch_provider_contract::calls::Purpose::Summary,
                root: self.call,
                workflow: Some(self.workflow),
                task: Some(self.task),
                provider: configured.0,
                model: &configured.1,
                dimensions: None,
            },
            self.engine.inner.clock.now(),
        )
        .await
        .map_err(internal)?;
        let crate::calls::ServicePlan::Ready { call, candidate } = plan else {
            return Err(SummarizeError::Skipped(
                "Call recovery needs attention or its scheduled time.".into(),
            ));
        };
        if let Some(candidate) = candidate {
            request.backend.offering = Some(orch_provider_contract::capabilities::OfferingRef {
                provider: candidate.provider_id,
                offering: candidate.model.clone(),
            });
            request.params = Some(candidate.params.clone());
        }
        if let Some(kept) = Source::retained_source(&mut tx, call)
            .await
            .map_err(internal)?
        {
            if kept.call.id != call
                || kept.root != self.call
                || kept.workflow != self.workflow
                || kept.task != self.task
                || kept.call.node_id != node
                || kept.call.person_id != self.writer
                || kept.request.backend != request.backend
                || kept.resource
                    != self
                        .resource
                        .as_ref()
                        .map(|work| work.program_candidate(&kept.request))
                || kept
                    .current_callers(&mut tx, self.engine.inner.clock.as_ref())
                    .await
                    .map_err(internal)?
                    .is_none()
            {
                return Err(SummarizeError::Skipped(
                    "The retained program request no longer matches its source.".into(),
                ));
            }
            tx.commit().await.map_err(DbError::from).map_err(internal)?;
            return self
                .engine
                .execute_program_summary(kept)
                .await
                .map_err(internal);
        }
        let offering = request
            .backend
            .offering
            .as_ref()
            .ok_or_else(|| SummarizeError::Config("Choose a model offering.".into()))?;
        crate::tools::capability_service::delegation::backend_state(
            &mut tx,
            &orch_provider_contract::builtins::BuiltinSources::default(),
            "summary",
            &request.backend,
        )
        .await
        .map_err(internal)?
        .map_err(|_| {
            SummarizeError::Skipped("The selected program model is unavailable.".into())
        })?;
        let params = request
            .params
            .as_ref()
            .cloned()
            .unwrap_or_else(|| json!({}));
        let options = params.as_object().ok_or_else(|| {
            SummarizeError::Config("Program request options must be an object.".into())
        })?;
        if options.keys().any(|key| key != "effort") {
            return Err(SummarizeError::Config(
                "This program does not implement the supplied model request options.".into(),
            ));
        }
        orch_provider_contract::calls::requested_effort(&params)
            .map_err(|_| SummarizeError::Config("The requested model effort is invalid.".into()))?;
        let thread = threads::thread(&mut tx, node).await.map_err(internal)?;
        let topic = thread.as_ref().map_or(node, |thread| thread.topic_id);
        let material = crate::raw_events::mask_summary(
            &mut tx,
            node,
            topic,
            thread.as_ref().and_then(|thread| thread.environment_id),
            json!({"system":request.system,"input":request.input}),
        )
        .await
        .map_err(internal)?
        .ok_or_else(|| SummarizeError::Skipped("Redaction is unavailable.".into()))?;
        request.system = material["system"].as_str().unwrap_or_default().to_owned();
        request.input = material["input"].as_str().unwrap_or_default().to_owned();
        let model = resources::model(&mut tx, offering.provider, &offering.offering)
            .await
            .map_err(internal)?
            .filter(|model| model.enabled)
            .ok_or_else(|| SummarizeError::Skipped("The selected model is unavailable.".into()))?;
        let parameters = orch_provider_contract::models::composed_model_parameters(
            &model.parameters,
            &model.groups,
            &model.option_layers,
            None,
        )
        .map_err(internal)?;
        let input_tokens = orch_summarizer_model::context::estimated_tokens(&request.system)
            .saturating_add(orch_summarizer_model::context::estimated_tokens(
                &request.input,
            ));
        if parameters
            .context_open()
            .or(parameters.context_window.filter(|window| *window > 0))
            .is_some_and(|window| {
                i64::from(input_tokens).saturating_add(request.max_output_tokens) > window
            })
            || self
                .resource
                .as_ref()
                .is_some_and(|work| input_tokens > work.context_tokens())
        {
            return Err(SummarizeError::Skipped(
                "The rendered summary exceeds its input allowance or model window.".into(),
            ));
        }
        if matches!(self.task, "handoff" | "goal_check") {
            let budget = super::super::budget::input_budget(
                &mut tx,
                node,
                offering.provider,
                &offering.offering,
            )
            .await
            .map_err(internal)?;
            if request.system.len().saturating_add(request.input.len()) > budget.bytes {
                return Err(SummarizeError::Skipped(
                    "The program input allowance cannot hold the rendered material.".into(),
                ));
            }
        }
        let resource = self
            .resource
            .as_ref()
            .map(|work| work.program_candidate(&request));
        let entry = match resource.as_ref() {
            Some(work) => {
                let frame = work
                    .frame(&mut tx, self.engine.inner.clock.as_ref())
                    .await
                    .map_err(internal)?;
                let entry = super::super::resource::model_entry(
                    &mut tx,
                    frame.principal(),
                    node,
                    &frame.value().config,
                )
                .await
                .map_err(internal)?;
                frame
                    .into_parts()
                    .first_effect
                    .check(self.engine.inner.clock.as_ref())
                    .map_err(internal)?;
                entry
            }
            None => None,
        };
        let current = Call {
            id: call,
            node_id: node,
            topic_id: topic,
            thread_id: thread.as_ref().map(|thread| thread.node_id),
            provider_id: offering.provider,
            person_id: Some(writer),
            issued_by: resource
                .as_ref()
                .and_then(super::super::resource::Work::issuer),
            entry_id: entry,
            model_name: offering.offering.clone(),
            created_at: self.engine.inner.clock.now(),
        };
        sqlx::query("INSERT INTO summary_calls(id,node_id,topic_id,thread_id,provider_id,person_id,model_name,created_at,issued_by,entry_id,agent_id,environment_id,program_backend) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) ON CONFLICT(id) DO NOTHING")
            .bind(current.id).bind(current.node_id).bind(current.topic_id).bind(current.thread_id).bind(current.provider_id).bind(current.person_id).bind(&current.model_name).bind(current.created_at)
            .bind(current.issued_by).bind(current.entry_id).bind(thread.as_ref().and_then(|thread|thread.agent_id)).bind(thread.as_ref().and_then(|thread|thread.environment_id))
            .bind(serde_json::to_value(&request.backend).map_err(internal)?)
            .execute(&mut *tx).await.map_err(DbError::from).map_err(internal)?;
        let kept: Call = sqlx::query_as("SELECT id,node_id,topic_id,thread_id,provider_id,person_id,issued_by,entry_id,model_name,created_at FROM summary_calls WHERE id=$1 AND purpose='summary' AND cancel_reason IS NULL")
            .bind(call).fetch_one(&mut *tx).await.map_err(DbError::from).map_err(internal)?;
        if kept.node_id != current.node_id
            || kept.provider_id != current.provider_id
            || kept.person_id != current.person_id
            || kept.issued_by != current.issued_by
            || kept.model_name != current.model_name
        {
            return Err(SummarizeError::Skipped(
                "The retained program request belongs to another choice.".into(),
            ));
        }
        let host = Host {
            engine: self.engine.clone(),
            writer: self.writer,
            call,
            workflow: self.workflow,
            task: self.task,
            resource,
        };
        let source = Source::capture(&mut tx, &host, kept, self.call, &configured, request)
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(DbError::from).map_err(internal)?;
        match self.engine.execute_program_summary(source).await {
            Ok(answer) => Ok(answer),
            Err(error) => {
                let mut tx = self.engine.db().begin().await.map_err(internal)?;
                let failure: Option<Json> = sqlx::query_scalar(
                    "SELECT delegated_result->'failure' FROM summary_calls WHERE id=$1",
                )
                .bind(call)
                .fetch_optional(&mut *tx)
                .await
                .map_err(DbError::from)
                .map_err(internal)?
                .flatten();
                let Some(failure) = failure else {
                    return Err(internal(error));
                };
                let context = json!({"root_input":self.call,"root_call":self.call,"configured_provider":configured.0,
                    "configured_model":configured.1,"provider_id":current.provider_id,"model":current.model_name,
                    "author_id":writer,"workflow_id":self.workflow,"task":self.task});
                let matched = crate::calls::reserve_service(
                    &mut tx,
                    &crate::calls::ServiceCall {
                        node,
                        purpose: orch_provider_contract::calls::Purpose::Summary,
                        root: self.call,
                        workflow: Some(self.workflow),
                        task: Some(self.task),
                        provider: configured.0,
                        model: &configured.1,
                        dimensions: None,
                    },
                    call,
                    &failure,
                    context,
                    self.engine.inner.clock.now(),
                )
                .await
                .map_err(internal)?;
                tx.commit().await.map_err(DbError::from).map_err(internal)?;
                if matched.is_none() {
                    Err(SummarizeError::Skipped(NO_RULE.into()))
                } else {
                    Err(SummarizeError::Failed {
                        message: "The program could not complete the summary.".into(),
                        retryable: true,
                    })
                }
            }
        }
    }
}

/// Only terminal producer work or an expired native act releases its source journal.
pub(super) async fn retire_finished(
    engine: &Engine,
    conn: &mut Conn,
    limit: i64,
) -> Result<(), EngineError> {
    let expired = engine.inner.clock.now()
        - chrono::Duration::seconds(i64::from(
            orch_link_contract::commands::CommandKind::ProviderComplete.deadline_secs(),
        ));
    sqlx::query("DELETE FROM summary_calls WHERE id IN (
      SELECT c.id FROM summary_calls c WHERE c.program_backend IS NOT NULL AND (
        c.created_at<=$1 OR (c.delegated_result IS NOT NULL AND (
          (c.delegated_program#>'{origin,source,resource}' IS NOT NULL AND c.delegated_program#>'{origin,source,resource}'<>'null'::jsonb
            AND NOT EXISTS(SELECT 1 FROM resource_summary_requests r WHERE r.id=(c.delegated_program#>>'{origin,source,resource,request}')::uuid AND r.state IN ('queued','running')))
          OR (c.delegated_program#>>'{origin,source,task}' IN ('title','tags')
            AND NOT EXISTS(SELECT 1 FROM node_summary_state n WHERE n.node_id=c.node_id AND n.run_id=(c.delegated_program#>>'{origin,source,workflow}')::uuid))
        ))) ORDER BY c.created_at,c.id LIMIT $2)")
        .bind(expired).bind(limit).execute(conn).await.map_err(DbError::from)?;
    Ok(())
}
