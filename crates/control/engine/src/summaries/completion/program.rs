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
    pool_target: Option<orch_provider_contract::pools::Target>,
}

struct CheckedSource {
    source: Source,
    fence: ReadFence,
    callers: Vec<Caller>,
}

/// PostgreSQL restores this transaction-local marker when a savepoint rolls back,
/// at the same boundary that releases the source locks acquired after that savepoint.
#[derive(Clone, Copy)]
struct ReadFence(Id);

impl ReadFence {
    async fn capture(conn: &mut Conn) -> Result<Self, EngineError> {
        let marker = orch_types::new_id();
        sqlx::query("SELECT set_config('orch.summary_source_check',$1,true)")
            .bind(marker.to_string())
            .execute(conn)
            .await
            .map_err(DbError::from)?;
        Ok(Self(marker))
    }

    async fn held(self, conn: &mut Conn) -> Result<bool, EngineError> {
        Ok(sqlx::query_scalar("SELECT NULLIF(current_setting('orch.summary_source_check',true),'')::uuid IS NOT DISTINCT FROM $1")
            .bind(self.0).fetch_one(conn).await.map_err(DbError::from)?)
    }
}

tokio::task_local! {
    static CHECKED_SOURCE: std::cell::RefCell<Option<CheckedSource>>;
}

/// Repeated native creator/input checks reuse only the source fenced by their
/// actual transaction. This opaque value never leaves the awaited future.
#[inline(never)]
pub(crate) fn scope_checks<F: std::future::Future>(
    make: impl FnOnce() -> F,
) -> impl std::future::Future<Output = F::Output> {
    // Turn startup and preflight can both enter this scope. Construct their work here so
    // each caller keeps one pointer across its waits, including when a scope already exists.
    let work = Box::pin(make());
    async move {
        if CHECKED_SOURCE.try_with(|_| ()).is_ok() {
            work.await
        } else {
            CHECKED_SOURCE
                .scope(std::cell::RefCell::new(None), work)
                .await
        }
    }
}

impl Source {
    pub(super) async fn capture(
        conn: &mut Conn,
        host: &Host,
        call: Call,
        root: Id,
        configured: &(Id, String),
        request: CompletionRequest,
        pool_target: Option<orch_provider_contract::pools::Target>,
    ) -> Result<Self, EngineError> {
        let automatic = if host.resource.is_none() {
            Some(automatic::Source::capture(conn, host, &call, root, configured).await?)
        } else {
            None
        };
        let source = Self {
            pool_target,
            deadline: paid::deadline(&call),
            call,
            root,
            workflow: host.workflow,
            task: host.task.into(),
            request,
            resource: host.resource.clone(),
            automatic,
        };
        // Resource capture follows its actual admitted frame on this transaction.
        // Execution re-enters that persisted source before creating or starting a helper.
        if source.resource.is_none()
            && source
                .current_callers(conn, host.engine.inner.clock.as_ref())
                .await?
                .is_none()
        {
            return Err(unavailable());
        }
        Ok(source)
    }
    pub(crate) fn prompt_governors(
        &self,
    ) -> (
        Option<Id>,
        Option<Id>,
        Vec<orch_api_contract::summaries::SummaryTarget>,
    ) {
        use orch_api_contract::summaries::SummaryTarget;
        if let Some(work) = &self.resource {
            let (request, viewer, refs) = work.prompt_governors();
            return (Some(request), viewer, refs.to_vec());
        }
        let target = self.call.thread_id.map_or(
            SummaryTarget::Topic {
                id: self.call.node_id,
            },
            |id| SummaryTarget::Thread { id },
        );
        (None, None, vec![target])
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
    pub(crate) async fn pool_selection(
        &self,
        conn: &mut Conn,
        caller: &Caller,
    ) -> Result<Option<crate::resource_choice::ProviderSelection>, EngineError> {
        let Some(target) = &self.pool_target else {
            return Ok(None);
        };
        let selection = crate::resource_choice::ProviderSelection::load(
            conn,
            self.call.id,
            self.call.node_id,
            self.call.provider_id,
            Some(&self.call.model_name),
            orch_access_contract::permissions::UsagePurpose::Summary,
            caller,
        )
        .await?;
        if !selection.targets(target) {
            return Err(unavailable());
        }
        Ok(Some(selection))
    }
    pub(crate) fn backend(&self) -> &BackendRef {
        &self.request.backend
    }
    pub(crate) fn params(&self) -> Option<&Json> {
        self.request.params.as_ref()
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

    pub(crate) async fn lock_source(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<(), EngineError> {
        match (&self.resource, &self.automatic) {
            (Some(work), None) => {
                self.current_callers(conn, clock)
                    .await?
                    .ok_or_else(unavailable)?;
                work.lock_source(conn).await?;
            }
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
        let cacheable = self.resource.is_some()
            && CHECKED_SOURCE.try_with(|_| ()).is_ok()
            && orch_db::queries::transaction_guard::in_transaction(conn);
        if cacheable {
            let retained = CHECKED_SOURCE
                .try_with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .filter(|checked| checked.source == *self)
                        .map(|checked| (checked.fence, checked.callers.clone()))
                })
                .ok()
                .flatten();
            if let Some((fence, callers)) = retained
                && fence.held(conn).await?
            {
                return Ok((clock.time() < self.deadline
                    && callers
                        .iter()
                        .all(|caller| caller.key_deadline_allows(clock)))
                .then_some(callers));
            }
        }
        let callers = self.fresh_callers(conn, clock, false).await?;
        if cacheable && let Some(current) = &callers {
            let fence = ReadFence::capture(conn).await?;
            let _ = CHECKED_SOURCE.try_with(|slot| {
                *slot.borrow_mut() = Some(CheckedSource {
                    source: self.clone(),
                    fence,
                    callers: current.clone(),
                })
            });
        }
        Ok(callers)
    }

    pub(crate) async fn result_callers(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<Caller>>, EngineError> {
        // This read proof must never populate the paid-admission cache.
        self.fresh_callers(conn, clock, true).await
    }

    pub(crate) async fn retained_program(
        &self,
        conn: &mut Conn,
    ) -> Result<Option<Json>, EngineError> {
        Ok(sqlx::query_scalar::<_, Option<Json>>(
            "SELECT delegated_program FROM summary_calls WHERE id=$1 AND purpose='summary' AND cancel_reason IS NULL",
        )
        .bind(self.call.id)
        .fetch_optional(conn)
        .await
        .map_err(DbError::from)?
        .flatten())
    }

    pub(crate) async fn for_resource(
        conn: &mut Conn,
        work: &super::super::resource::Work,
    ) -> Result<Vec<Self>, EngineError> {
        let records: Vec<Json> = sqlx::query_scalar(
            "SELECT delegated_program#>'{origin,source}' FROM summary_calls WHERE resource_request_id=$1 AND resource_call_id=$2 AND purpose='summary' AND cancel_reason IS NULL AND delegated_program IS NOT NULL ORDER BY created_at,id",
        )
        .bind(work.request())
        .bind(work.call())
        .fetch_all(conn)
        .await
        .map_err(DbError::from)?;
        let mut sources = Vec::with_capacity(records.len());
        for value in records {
            let source: Self = serde_json::from_value(value)?;
            if source.resource.as_ref() != Some(&work.program_candidate(&source.request)) {
                return Err(unavailable());
            }
            sources.push(source);
        }
        Ok(sources)
    }

    async fn fresh_callers(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        result: bool,
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

        let association: (Option<Id>, Option<Id>) = sqlx::query_as(
            "SELECT resource_request_id,resource_call_id FROM summary_calls WHERE id=$1",
        )
        .bind(self.call.id)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
        let expected = self.resource.as_ref().map_or((None, None), |work| {
            (Some(work.request()), Some(work.call()))
        });
        if association != expected {
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
                let checked = if result {
                    work.result_frame(conn, clock).await
                } else {
                    work.frame(conn, clock).await
                };
                let frame = match checked {
                    Ok(frame) => frame,
                    Err(error) if crate::summaries::resource::source_change(&error).is_some() => {
                        return Err(error);
                    }
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

    /// This records a successful native wire cut; it supplies no caller or execution grant.
    pub(crate) async fn mark_model_admitted(
        &self,
        conn: &mut Conn,
        at: Time,
    ) -> Result<(), EngineError> {
        if at >= self.deadline {
            return Err(unavailable());
        }
        if self.pool_target.is_some()
            && !crate::resource_choice::health::admit_choice(
                conn,
                self.call.id,
                self.call.id,
                self.call.node_id,
                self.call.provider_id,
                &self.call.model_name,
                Some(&self.request.backend),
                at.get(),
            )
            .await?
        {
            return Err(unavailable());
        }
        if let Some(work) = &self.resource
            && !orch_db::queries::summaries::model_admitted(
                conn,
                work.request(),
                work.call(),
                at.get(),
            )
            .await?
        {
            return Err(unavailable());
        }
        Ok(())
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
        self.lock_source(&mut tx, engine.inner.clock.as_ref())
            .await?;
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
        #[cfg(test)]
        observation::before_result(self.call.id).await;
        let mut tx = engine.db().begin().await?;
        if let (None, Some(source)) = (&self.resource, &self.automatic) {
            source.lock_source(&mut tx).await?;
        }
        if self
            .result_callers(&mut tx, engine.inner.clock.as_ref())
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
        if self.pool_target.is_some() {
            crate::resource_choice::health::outcome(
                &mut tx,
                self.call.id,
                value.get("result").is_some(),
                value.get("failure"),
                engine.inner.clock.now(),
            )
            .await?;
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
        let purpose = orch_provider_contract::calls::Purpose::Summary;
        let crate::calls::ServicePlan::Ready { call, candidate } = plan else {
            let reason = crate::calls::read_root(&mut tx, node, purpose, self.call)
                .await
                .map_err(internal)?
                .as_ref()
                .and_then(crate::calls::administrative_pause_reason)
                .unwrap_or("Call recovery needs attention or its scheduled time.");
            tx.commit().await.map_err(DbError::from).map_err(internal)?;
            return Err(SummarizeError::Skipped(reason.into()));
        };
        let mut pool_target = None;
        let mut selected_candidate = None;
        if let Some(intent) = candidate {
            let caller = match &self.resource {
                Some(work) => work
                    .caller(&mut tx, self.engine.inner.clock.as_ref())
                    .await
                    .map_err(internal)?,
                None => Caller::session(writer).with_admin_mode(false),
            };
            let demand = super::pool_demand(
                &mut tx,
                node,
                &intent.target,
                (&request.system, &request.input),
                request.max_output_tokens,
                request.params.as_ref(),
                None,
            )
            .await?;
            let resolved = self
                .engine
                .resolve_recovery_candidate(
                    &mut tx,
                    &crate::calls::DestinationRequest {
                        node,
                        purpose,
                        root: self.call,
                        attempt: call,
                        retained_route: Some(request.backend.clone()),
                        demand,
                    },
                    std::slice::from_ref(&caller),
                    &intent,
                    &configured.1,
                )
                .await
                .map_err(internal)?;
            let Some(selected) = resolved.selected else {
                self.skip_candidate(&mut tx, node, &resolved.reason).await?;
                tx.commit().await.map_err(DbError::from).map_err(internal)?;
                return Err(SummarizeError::Skipped(
                    "The selected recovery destination is unavailable.".into(),
                ));
            };
            let Some(backend) = selected
                .route
                .as_ref()
                .filter(|backend| backend.route == request.backend.route)
            else {
                self.skip_candidate(&mut tx, node, "candidate_purpose")
                    .await?;
                tx.commit().await.map_err(DbError::from).map_err(internal)?;
                return Err(SummarizeError::Skipped(
                    "The destination cannot run this program summary.".into(),
                ));
            };
            request.backend = backend.clone();
            if selected
                .params
                .as_object()
                .is_some_and(|params| !params.is_empty())
            {
                request.params = Some(selected.params.clone());
            }
            if matches!(
                intent.target,
                orch_provider_contract::pools::Target::Pool { .. }
            ) {
                pool_target = Some(intent.target);
            }
            let saved = sqlx::query("UPDATE model_call_recovery SET context=jsonb_set(context,'{candidate}',$4) WHERE node_id=$1 AND purpose='summary' AND call_key=$2 AND state='running' AND context->>'next_call'=$3")
                .bind(node).bind(self.call).bind(call.to_string()).bind(json!(&selected))
                .execute(&mut *tx).await.map_err(DbError::from).map_err(internal)?;
            if saved.rows_affected() != 1 {
                return Err(internal(unavailable()));
            }
            selected_candidate = Some(selected);
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
                || kept.pool_target != pool_target
                || kept.request.backend != request.backend
                || kept.resource
                    != self
                        .resource
                        .as_ref()
                        .map(|work| work.program_candidate(&kept.request))
                || kept
                    .result_callers(&mut tx, self.engine.inner.clock.as_ref())
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
        let automatic = self.resource.as_ref().map_or_else(
            || matches!(self.task, "title" | "tags"),
            |work| work.issuer().is_none(),
        );
        if automatic
            && let Some(wait) =
                super::super::headroom::wait(&mut tx, offering, self.engine.inner.clock.time())
                    .await
                    .map_err(internal)?
        {
            return Err(SummarizeError::Waiting(wait));
        }
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
        params.as_object().ok_or_else(|| {
            SummarizeError::Config("Program request options must be an object.".into())
        })?;
        orch_provider_contract::calls::requested_effort(&params)
            .map_err(|_| SummarizeError::Config("The requested model effort is invalid.".into()))?;
        let thread = threads::thread(&mut tx, node).await.map_err(internal)?;
        let topic = thread.as_ref().map_or(node, |thread| thread.topic_id);
        let material = super::masked_material(
            &mut tx,
            node,
            topic,
            thread.as_ref().and_then(|thread| thread.environment_id),
            &request.system,
            &request.input,
        )
        .await?;
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
        let mut admitted_input = None;
        if matches!(self.task, "handoff" | "goal_check") {
            let budget = super::super::budget::input_budget(
                &mut tx,
                node,
                offering.provider,
                &offering.offering,
                request.max_output_tokens,
            )
            .await
            .map_err(internal)?;
            admitted_input = Some(budget.tokens);
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
        sqlx::query("INSERT INTO summary_calls(id,node_id,topic_id,thread_id,provider_id,person_id,model_name,created_at,issued_by,entry_id,agent_id,environment_id,program_backend,resource_request_id,resource_call_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) ON CONFLICT(id) DO NOTHING")
            .bind(current.id).bind(current.node_id).bind(current.topic_id).bind(current.thread_id).bind(current.provider_id).bind(current.person_id).bind(&current.model_name).bind(current.created_at)
            .bind(current.issued_by).bind(current.entry_id).bind(thread.as_ref().and_then(|thread|thread.agent_id)).bind(thread.as_ref().and_then(|thread|thread.environment_id))
            .bind(serde_json::to_value(&request.backend).map_err(internal)?)
            .bind(resource.as_ref().map(super::super::resource::Work::request)).bind(resource.as_ref().map(super::super::resource::Work::call))
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
        if let Some(capture) = self.goal_capture.as_ref() {
            super::super::facts::capture_goal(
                &mut tx,
                capture,
                super::super::facts::GoalAdmission {
                    node,
                    goal: self.workflow,
                    call,
                    owner: writer,
                    model: offering.offering.clone(),
                    model_display: if model.display_name.is_empty() {
                        offering.offering.clone()
                    } else {
                        model.display_name.clone()
                    },
                    input: admitted_input,
                    output: request.max_output_tokens,
                    already_issued: false,
                    at: self.engine.inner.clock.time(),
                },
            )
            .await
            .map_err(internal)?;
        }
        let host = Host {
            engine: self.engine.clone(),
            writer: self.writer,
            call,
            workflow: self.workflow,
            task: self.task,
            goal_capture: self.goal_capture.clone(),
            resource,
        };
        let source = Source::capture(
            &mut tx,
            &host,
            kept,
            self.call,
            &configured,
            request,
            pool_target,
        )
        .await
        .map_err(internal)?;
        tx.commit().await.map_err(DbError::from).map_err(internal)?;
        let source_backend = source.request.backend.clone();
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
                let mut context = json!({"root_input":self.call,"root_call":self.call,"configured_provider":configured.0,
                    "configured_model":configured.1,"provider_id":current.provider_id,"model":current.model_name,
                    "author_id":writer,"workflow_id":self.workflow,"task":self.task});
                context["capability_route"] = json!(source_backend);
                if let Some(candidate) = selected_candidate {
                    context["candidate"] = json!(candidate);
                }
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

#[cfg(test)]
pub(crate) mod observation {
    use futures::future::BoxFuture;
    use orch_types::Id;
    use std::{future::Future, sync::Arc};

    type Observer = Arc<dyn Fn(Id) -> BoxFuture<'static, ()> + Send + Sync>;
    tokio::task_local! { static OBSERVER: Observer; }

    pub(crate) async fn follow<T>(
        observer: impl Fn(Id) -> BoxFuture<'static, ()> + Send + Sync + 'static,
        work: impl Future<Output = T>,
    ) -> T {
        OBSERVER.scope(Arc::new(observer), work).await
    }

    pub(super) async fn before_result(call: Id) {
        if let Ok(observer) = OBSERVER.try_with(Arc::clone) {
            observer(call).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rolling_back_a_source_check_releases_only_the_proof_acquired_inside_that_savepoint()
    -> Result<(), EngineError> {
        let world = crate::test_common::World::new().await;
        let mut tx = world.pool.begin().await?;
        let outer = ReadFence::capture(&mut tx).await?;
        let mut nested = sqlx::Connection::begin(&mut *tx)
            .await
            .map_err(DbError::from)?;
        let inner = ReadFence::capture(&mut nested).await?;
        assert!(inner.held(&mut nested).await?);
        nested.rollback().await.map_err(DbError::from)?;
        assert!(!inner.held(&mut tx).await?);
        assert!(outer.held(&mut tx).await?);
        tx.commit().await.map_err(DbError::from)?;
        let mut next = world.pool.begin().await?;
        assert!(!outer.held(&mut next).await?);
        Ok(())
    }
}
