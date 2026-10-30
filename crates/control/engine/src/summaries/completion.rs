//! Summary calls through the selected API provider with the setting writer's current authority and actual usage.

use super::internal;
use crate::access::ProviderUse;
use crate::commands::{Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::resource_facts::activity;
use crate::turns::OwnUse;
use async_trait::async_trait;
use orch_agent_contract::events::TokenUsage;
use orch_db::queries::{resources::thread_engine as resources, threads};
use orch_db::{Conn, DbError};
use orch_link_contract::commands::{ProviderComplete, ProviderCompleteResult, SerialKey};
use orch_summarizer_contract::summarizer::SummarizeError;
use orch_summarizer_model::Complete;
use orch_types::{Id, Json};
use serde_json::json;

/// A model failure with no matching policy must not fall back to an unrelated retry timer.
pub(super) const NO_RULE: &str = "calls.details.rule_unavailable";

/// A deliberate pause never enters the model-failure or legacy retry paths.
pub(super) fn stable_pause(reason: &str) -> bool {
    matches!(
        reason,
        NO_RULE
            | crate::calls::PROVIDER_SWITCHED_OFF
            | crate::calls::MODEL_SWITCHED_OFF
            | crate::calls::REQUEST_CANCELLED
            | "Provider switched off"
            | "Provider removed"
            | "Model switched off"
            | "Model removed"
            | "Agent profile switched off"
            | "Agent profile removed"
            | "Environment switched off"
            | "Environment removed"
            | "Resource switched off"
    )
}

fn effort_is_offered(params: &Json, model: &Json) -> bool {
    match orch_provider_contract::calls::requested_effort(params) {
        Ok(None) => true,
        Ok(Some(effort)) => model["efforts"]
            .as_array()
            .is_some_and(|levels| levels.iter().any(|level| level.as_str() == Some(effort))),
        Err(_) => false,
    }
}

#[derive(sqlx::FromRow)]
struct Call {
    id: Id,
    node_id: Id,
    topic_id: Id,
    thread_id: Option<Id>,
    provider_id: Id,
    person_id: Option<Id>,
    model_name: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

async fn counted(
    engine: &Engine,
    conn: &mut Conn,
    call: &Call,
    answer: &ProviderCompleteResult,
) -> Result<(), EngineError> {
    let tokens: TokenUsage = answer.usage;
    engine
        .count_own_use(
            conn,
            &OwnUse {
                purpose: crate::turns::OwnPurpose::Summary,
                timing: Some(&answer.timing),
                id: call.id,
                at: answer.timing.end.map_or(call.created_at, |at| at.get()),
                topic: call.topic_id,
                thread: call.thread_id,
                provider: call.provider_id,
                entry: None,
                person: call.person_id,
                model: &call.model_name,
                tokens: orch_db::usage::ledger::Tokens {
                    input: tokens.input_tokens,
                    cached_input: tokens.cached_input_tokens,
                    cache_write: tokens.cache_write_tokens,
                    output: tokens.output_tokens,
                    reasoning: tokens.reasoning_tokens,
                    total: tokens.total_tokens,
                },
                request_ref: answer.request_ref.as_deref(),
            },
        )
        .await?;
    sqlx::query("DELETE FROM summary_calls WHERE id=$1")
        .bind(call.id)
        .execute(conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

/// Only the model client knows whether a failed command actually made an upstream request.
async fn count_failed(
    engine: &Engine,
    conn: &mut Conn,
    call: &Call,
    error: &orch_link_contract::commands::CommandError,
) -> Result<(), EngineError> {
    let Some(raw) = error
        .details
        .as_ref()
        .and_then(|details| details.get("model_request"))
    else {
        return Ok(());
    };
    let request: orch_agent_contract::requests::UsageRequest = serde_json::from_value(raw.clone())
        .map_err(|error| EngineError::unreadable(error.to_string()))?;
    request.check().map_err(EngineError::unreadable)?;
    counted(
        engine,
        conn,
        call,
        &ProviderCompleteResult {
            text: String::new(),
            usage: request.tokens.clone().usage(),
            request_ref: None,
            timing: request.measured(),
        },
    )
    .await
}

/// Reconciles answered calls after a lost process, without making any new provider request.
pub(super) async fn recover(
    engine: &Engine,
    conn: &mut Conn,
    limit: i64,
) -> Result<(), EngineError> {
    let calls: Vec<Call> = sqlx::query_as("SELECT id,node_id,topic_id,thread_id,provider_id,person_id,model_name,created_at FROM summary_calls WHERE purpose = 'summary' ORDER BY created_at,id LIMIT $1")
        .bind(limit).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    for call in calls {
        let status = engine.inner.commands.status(call.id).await?;
        match status.and_then(|status| status.outcome) {
            Some(Outcome::Succeeded(value)) => {
                settle_recovered(engine, conn, &call, crate::model_inputs::Sent::Taken).await?;
                let answer: ProviderCompleteResult = serde_json::from_value(value)
                    .map_err(|error| EngineError::unreadable(error.to_string()))?;
                let mut tx = engine.db().begin().await?;
                counted(engine, &mut tx, &call, &answer).await?;
                tx.commit().await.map_err(DbError::from)?;
            }
            Some(outcome) => {
                if let Outcome::Failed(error) = &outcome {
                    let mut tx = engine.db().begin().await?;
                    count_failed(engine, &mut tx, &call, error).await?;
                    tx.commit().await.map_err(DbError::from)?;
                }
                let sent = if matches!(outcome, Outcome::Failed(_)) {
                    crate::model_inputs::Sent::Refused
                } else {
                    crate::model_inputs::Sent::Unknown
                };
                settle_recovered(engine, conn, &call, sent).await?;
                sqlx::query("DELETE FROM summary_calls WHERE id=$1")
                    .bind(call.id)
                    .execute(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            }
            None => {}
        }
    }
    Ok(())
}

/// A process loss cannot leave an answered auxiliary request labelled as still being sent.
async fn settle_recovered(
    engine: &Engine,
    conn: &mut Conn,
    call: &Call,
    sent: crate::model_inputs::Sent,
) -> Result<(), EngineError> {
    let record: Option<Id> = sqlx::query_scalar(
        "SELECT id FROM model_inputs WHERE node_id=$1 AND purpose='summary' AND reference=$2",
    )
    .bind(call.node_id)
    .bind(call.id.to_string())
    .fetch_optional(conn)
    .await
    .map_err(DbError::from)?;
    if let Some(record) = record {
        engine
            .settle_node_model_input(call.node_id, record, sent)
            .await;
    }
    Ok(())
}

/// The document version an auxiliary request is allowed to read.
#[derive(Clone, Copy)]
pub(super) struct DocumentContext {
    /// The document whose text was captured.
    pub(super) id: Id,
    /// The document sequence the prompt was made from.
    pub(super) seq: i64,
}

/// A single call has one persistent command and ledger identity, even after a scheduler crash.
pub(super) struct Host {
    pub(super) engine: Engine,
    pub(super) writer: Option<Id>,
    pub(super) call: Id,
    pub(super) workflow: Id,
    pub(super) task: &'static str,
    pub(super) document: Option<DocumentContext>,
}

impl Host {
    fn recovery_context(
        &self,
        call: &Call,
        root: Id,
        configured: &(Id, String),
        request: &ProviderComplete,
        writer: Id,
    ) -> orch_types::Json {
        let mut context = json!({"root_input":root,"root_call":root,"configured_provider":configured.0,"configured_model":configured.1,"provider_id":call.provider_id,"model":call.model_name,"author_id":writer,"workflow_id":self.workflow,"task":self.task});
        if request.provider_id != configured.0
            || request.model_name != configured.1
            || request.params.is_some()
        {
            context["candidate"] = json!({"provider_id":request.provider_id,"model":request.model_name,"params":request.params.clone().unwrap_or_else(||json!({}))});
        }
        if let Some(document) = self.document {
            context["document_id"] = json!(document.id);
            context["document_seq"] = json!(document.seq);
        }
        context
    }

    async fn skip_candidate(
        &self,
        conn: &mut Conn,
        node: Id,
        reason: &str,
    ) -> Result<(), SummarizeError> {
        if let Some(row) = crate::calls::read_root(
            conn,
            node,
            orch_provider_contract::calls::Purpose::Summary,
            self.call,
        )
        .await
        .map_err(internal)?
        {
            crate::calls::skip_step(conn, &row, reason, self.engine.inner.clock.now())
                .await
                .map_err(internal)?;
        }
        Ok(())
    }

    async fn complete_once(
        &self,
        node: Id,
        mut request: ProviderComplete,
        root_call: Id,
        configured: &(Id, String),
    ) -> Result<ProviderCompleteResult, SummarizeError> {
        let Some(writer) = self.writer else {
            return Err(SummarizeError::Skipped(
                "no summarizer setting writer is recorded".to_owned(),
            ));
        };
        let retained = self
            .engine
            .inner
            .commands
            .retained_body(self.call)
            .await
            .map_err(internal)?;
        let already_issued = self
            .engine
            .inner
            .commands
            .status(self.call)
            .await
            .map_err(internal)?
            .is_some();
        if let Some(body) = retained {
            let original: ProviderComplete = serde_json::from_value(body).map_err(internal)?;
            if original.provider_id != request.provider_id
                || original.model_name != request.model_name
            {
                return Err(SummarizeError::Skipped(
                    "The retained model request belongs to another choice".to_owned(),
                ));
            }
            request.protocol = original.protocol;
        }
        let mut tx = self.engine.db().begin().await.map_err(internal)?;
        // A revocation committed while this runs refuses its commit; no epoch row is held meanwhile.
        orch_db::queries::transaction_guard::lock_action(&mut tx)
            .await
            .map_err(internal)?;
        if let Some(document) = self.document {
            let current = orch_db::queries::documents::get(&mut tx, document.id)
                .await
                .map_err(internal)?;
            if current.is_none_or(|row| {
                row.deleted_at.is_some()
                    || row.description_source != "auto"
                    || row.node_id != node
                    || row.doc_seq != document.seq
            }) {
                return Err(SummarizeError::Skipped(
                    "the summary's source changed".to_owned(),
                ));
            }
            let principal = orch_authz::Principal::load(
                &mut tx,
                orch_authz::Caller::session(writer),
                orch_types::Time::new(self.engine.inner.clock.now()),
            )
            .await
            .map_err(internal)?;
            let document_view = orch_authz::documents::check(
                &mut tx,
                &principal,
                document.id,
                orch_access_contract::permissions::Permission::DocView,
                orch_authz::Policy::default(),
            )
            .await
            .map_err(internal)?
            .is_allowed();
            let node_view = orch_authz::nodes::check_named(
                &mut tx,
                &principal,
                node,
                orch_access_contract::permissions::PermissionName::OfNode(
                    orch_access_contract::permissions::NodePermission::View,
                ),
                orch_authz::Act::Use,
                orch_authz::Policy::default(),
            )
            .await
            .map_err(internal)?
            .is_allowed();
            if !document_view || !node_view {
                return Err(SummarizeError::Skipped(
                    "the summarizer cannot read its source".to_owned(),
                ));
            }
        }
        if let Some(reason) =
            crate::calls::switched_off_reason(&mut tx, request.provider_id, &request.model_name)
                .await
                .map_err(internal)?
        {
            return Err(SummarizeError::Skipped(reason.to_owned()));
        }
        let provider = resources::provider(&mut tx, request.provider_id)
            .await
            .map_err(internal)?
            .filter(|provider| {
                !provider.protocols.is_empty()
                    && provider.deleted_at.is_none()
                    && provider.status == "ready"
            })
            .ok_or_else(|| {
                SummarizeError::Skipped(
                    "the summarizer needs a ready provider with forwarded access".to_owned(),
                )
            })?;
        let allowed = self
            .engine
            .inner
            .access
            .background_provider_use(
                &mut tx,
                &ProviderUse {
                    person: writer,
                    node,
                    provider: provider.id,
                    model: Some(&request.model_name),
                    effort: request
                        .params
                        .as_ref()
                        .map(orch_provider_contract::calls::requested_effort)
                        .transpose()
                        .map_err(|_| {
                            SummarizeError::Skipped(
                                "The requested model effort is invalid".to_owned(),
                            )
                        })?
                        .flatten(),
                },
            )
            .await
            .map_err(|_| {
                SummarizeError::Skipped(
                    "the setting writer's current access could not be confirmed".to_owned(),
                )
            })?;
        if !allowed.is_allowed() {
            return Err(SummarizeError::Skipped(
                "the setting writer may not use this provider or model here".to_owned(),
            ));
        }
        let model = resources::model(&mut tx, provider.id, &request.model_name)
            .await
            .map_err(internal)?
            .filter(|model| model.enabled)
            .ok_or_else(|| {
                SummarizeError::Skipped("the selected model is not offered".to_owned())
            })?;
        if request
            .params
            .as_ref()
            .is_some_and(|params| !effort_is_offered(params, &model.parameters))
        {
            return Err(SummarizeError::Skipped(
                "The selected model does not offer the requested effort".to_owned(),
            ));
        }
        let rules = crate::resolved::load(&mut tx, node)
            .await
            .map_err(internal)?;
        if rules.models_allowed.as_ref().is_some_and(|patterns| {
            !crate::models::allowed_by(patterns, &model.model_name, &model.groups)
        }) {
            return Err(SummarizeError::Skipped(
                "the selected model is not allowed here".to_owned(),
            ));
        }
        if !already_issued
            && !self
                .engine
                .room_for_summary(&mut tx, node, provider.id, &request.model_name)
                .await
                .map_err(internal)?
        {
            return Err(SummarizeError::Skipped(
                "a shared usage cap has no room for summaries".to_owned(),
            ));
        }
        if request.protocol.is_empty() {
            request.protocol = provider.protocols.first().cloned().ok_or_else(|| {
                SummarizeError::Skipped("the API provider offers no protocol".to_owned())
            })?;
        }
        let machine = provider.key_machine_id.ok_or_else(|| {
            SummarizeError::Skipped(
                "the API provider has no credential keeper available".to_owned(),
            )
        })?;
        if !orch_db::queries::resources::machine(&mut tx, machine)
            .await
            .map_err(internal)?
            .is_some_and(|machine| machine.online)
        {
            return Err(SummarizeError::Skipped(
                "the API provider's machine is offline".to_owned(),
            ));
        }
        let thread = threads::thread(&mut tx, node).await.map_err(internal)?;
        let context: Option<(Option<Id>,Option<Id>)> = sqlx::query_as("SELECT COALESCE(s.agent_id,t.agent_id),COALESCE(s.environment_id,t.environment_id) FROM threads t LEFT JOIN segments s ON s.id=t.current_segment_id WHERE t.node_id=$1")
            .bind(node).fetch_optional(&mut *tx).await.map_err(DbError::from).map_err(internal)?;
        let (agent, environment) = context.unwrap_or_default();
        if !activity::accepts_context(
            &mut tx,
            provider.id,
            &request.model_name,
            agent,
            environment,
        )
        .await
        .map_err(internal)?
        {
            return Err(SummarizeError::Skipped("Resource switched off".into()));
        }
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
        .ok_or_else(|| SummarizeError::Skipped("redaction is unavailable".to_owned()))?;
        request.system = material["system"].as_str().unwrap_or_default().to_owned();
        request.input = material["input"].as_str().unwrap_or_default().to_owned();
        // Admission uses the final redacted bytes and current allowance. A masking replacement or a changed model
        // window cannot make a previously fitted handoff or goal check exceed the place's input share.
        if matches!(self.task, "handoff" | "goal_check") {
            let budget =
                super::budget::input_budget(&mut tx, node, provider.id, &request.model_name)
                    .await
                    .map_err(internal)?;
            if request.system.len().saturating_add(request.input.len()) > budget.bytes {
                return Err(SummarizeError::Skipped(
                    "the summary model's input allowance cannot hold the rendered material"
                        .to_owned(),
                ));
            }
        }
        let call = Call {
            id: self.call,
            node_id: node,
            topic_id: topic,
            thread_id: thread.map(|thread| thread.node_id),
            provider_id: provider.id,
            person_id: Some(writer),
            model_name: request.model_name.clone(),
            created_at: self.engine.inner.clock.now(),
        };
        sqlx::query("INSERT INTO summary_calls (id,node_id,topic_id,thread_id,provider_id,person_id,model_name,created_at,agent_id,environment_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(id) DO NOTHING")
            .bind(call.id).bind(node).bind(call.topic_id).bind(call.thread_id).bind(call.provider_id).bind(call.person_id).bind(&call.model_name).bind(call.created_at).bind(agent).bind(environment).execute(&mut *tx).await.map_err(DbError::from).map_err(internal)?;
        let outgoing =
            Outgoing::new(self.call, machine, SerialKey::None, &request).map_err(internal)?;
        // Current authority is decided when this commits; an earlier change refuses the commit.
        // Only then are the prompt recorded and the call queued, each with no transaction open, so
        // a summary neither holds a turn or a revocation back nor goes out after a change that refused it.
        tx.commit().await.map_err(DbError::from).map_err(internal)?;
        let blocks = [
            crate::model_inputs::ModelBlock::new(
                "summary_system",
                "summary",
                request.system.clone(),
            ),
            crate::model_inputs::ModelBlock::new("summary_input", "summary", request.input.clone()),
        ];
        let model_input = match self.document {
            Some(document) => {
                self.engine
                    .record_document_model_input(node, document.id, &self.call.to_string(), &blocks)
                    .await
            }
            None => {
                self.engine
                    .record_node_model_input(node, "summary", &self.call.to_string(), &blocks)
                    .await
            }
        }
        .map_err(internal)?;
        let stopped = {
            let mut conn = self.engine.db().acquire().await.map_err(internal)?;
            let reason = activity::request_stop_reason(&mut conn, self.call)
                .await
                .map_err(internal)?;
            if reason.is_some() {
                // No command has been issued by this call, so no usage receipt can be lost.
                sqlx::query("DELETE FROM summary_calls WHERE id=$1 AND cancel_reason IS NOT NULL")
                    .bind(self.call)
                    .execute(&mut *conn)
                    .await
                    .map_err(DbError::from)
                    .map_err(internal)?;
            }
            reason
        };
        if let Some(reason) = stopped {
            self.engine
                .settle_node_model_input(node, model_input, crate::model_inputs::Sent::Refused)
                .await;
            return Err(SummarizeError::Skipped(reason));
        }
        self.engine
            .inner
            .commands
            .issue(outgoing)
            .await
            .map_err(internal)?;
        let outcome = self
            .engine
            .inner
            .commands
            .wait(self.call)
            .await
            .map_err(internal)?;
        let stopped = {
            let mut conn = self.engine.db().acquire().await.map_err(internal)?;
            activity::request_stop_reason(&mut conn, self.call)
                .await
                .map_err(internal)?
        };
        let answer = match outcome {
            Outcome::Succeeded(value) => {
                self.engine
                    .settle_node_model_input(node, model_input, crate::model_inputs::Sent::Taken)
                    .await;
                serde_json::from_value::<ProviderCompleteResult>(value).map_err(internal)?
            }
            Outcome::Failed(error) => {
                let mut tx = self.engine.db().begin().await.map_err(internal)?;
                count_failed(&self.engine, &mut tx, &call, &error)
                    .await
                    .map_err(internal)?;
                if let Some(reason) = stopped.as_ref() {
                    tx.commit().await.map_err(DbError::from).map_err(internal)?;
                    self.engine
                        .settle_node_model_input(
                            node,
                            model_input,
                            crate::model_inputs::Sent::Refused,
                        )
                        .await;
                    return Err(SummarizeError::Skipped(reason.clone()));
                }
                if let Some(reason) =
                    crate::calls::switched_off_reason(&mut tx, call.provider_id, &call.model_name)
                        .await
                        .map_err(internal)?
                {
                    tx.commit().await.map_err(DbError::from).map_err(internal)?;
                    self.engine
                        .settle_node_model_input(
                            node,
                            model_input,
                            crate::model_inputs::Sent::Refused,
                        )
                        .await;
                    return Err(SummarizeError::Skipped(reason.to_owned()));
                }
                if error.code == "unsupported"
                    && error
                        .details
                        .as_ref()
                        .and_then(|details| details.get("model_request"))
                        .is_none()
                    && let Some(row) = crate::calls::read_root(
                        &mut tx,
                        node,
                        orch_provider_contract::calls::Purpose::Summary,
                        root_call,
                    )
                    .await
                    .map_err(internal)?
                    && (row.context.get("candidate").is_some()
                        || row
                            .action
                            .as_ref()
                            .and_then(|action| action.pointer("/step/type"))
                            .and_then(orch_types::Json::as_str)
                            == Some("failover"))
                {
                    crate::calls::skip_step(
                        &mut tx,
                        &row,
                        "candidate_unavailable",
                        self.engine.inner.clock.now(),
                    )
                    .await
                    .map_err(internal)?;
                    tx.commit().await.map_err(DbError::from).map_err(internal)?;
                    self.engine
                        .settle_node_model_input(
                            node,
                            model_input,
                            crate::model_inputs::Sent::Refused,
                        )
                        .await;
                    return Err(SummarizeError::Skipped(
                        "The candidate's completion capability is unavailable".to_owned(),
                    ));
                }
                let raw = error.details.as_ref();
                let mut failure=raw.and_then(|raw|raw.get("failure")).cloned().unwrap_or_else(||json!({
                    "code":raw.and_then(|raw|raw.pointer("/model_request/outcome/code")).and_then(orch_types::Json::as_str).unwrap_or("harness_error")
                }));
                failure["message"] = json!("The provider could not complete the request");
                if failure.get("retryable").is_none() {
                    failure["retryable"] = json!(matches!(
                        failure["code"].as_str(),
                        Some("provider_busy" | "provider_unreachable" | "model_refused")
                    ));
                }
                failure["said"] = raw
                    .and_then(|raw| raw.pointer("/model_request/outcome/said"))
                    .cloned()
                    .unwrap_or(orch_types::Json::Null);
                let context = self.recovery_context(&call, root_call, configured, &request, writer);
                let recovery = crate::calls::reserve(
                    &mut tx,
                    node,
                    orch_provider_contract::calls::Purpose::Summary,
                    self.call,
                    &failure,
                    context,
                    self.engine.inner.clock.now(),
                )
                .await
                .map_err(internal)?;
                tx.commit().await.map_err(DbError::from).map_err(internal)?;
                self.engine
                    .settle_node_model_input(node, model_input, crate::model_inputs::Sent::Refused)
                    .await;
                if recovery.is_none() {
                    return Err(SummarizeError::Skipped(NO_RULE.to_owned()));
                }
                return Err(SummarizeError::Failed {
                    message: "the provider could not complete the summary".to_owned(),
                    retryable: true,
                });
            }
            Outcome::Expired => {
                self.engine
                    .settle_node_model_input(node, model_input, crate::model_inputs::Sent::Unknown)
                    .await;
                if let Some(reason) = stopped {
                    return Err(SummarizeError::Skipped(reason));
                }
                let mut tx = self.engine.db().begin().await.map_err(internal)?;
                if let Some(reason) =
                    crate::calls::switched_off_reason(&mut tx, call.provider_id, &call.model_name)
                        .await
                        .map_err(internal)?
                {
                    return Err(SummarizeError::Skipped(reason.to_owned()));
                }
                let recovery=crate::calls::reserve(&mut tx,node,orch_provider_contract::calls::Purpose::Summary,self.call,
                    &json!({"code":"provider_unreachable","message":"The model request did not finish","retryable":true}),
                    self.recovery_context(&call,root_call,configured,&request,writer),self.engine.inner.clock.now()).await.map_err(internal)?;
                tx.commit().await.map_err(DbError::from).map_err(internal)?;
                if recovery.is_none() {
                    return Err(SummarizeError::Skipped(NO_RULE.to_owned()));
                }
                return Err(SummarizeError::Failed {
                    message: "The summary request did not finish".to_owned(),
                    retryable: true,
                });
            }
            Outcome::Cancelled => {
                if let Some(reason) = stopped {
                    self.engine
                        .settle_node_model_input(
                            node,
                            model_input,
                            crate::model_inputs::Sent::Unknown,
                        )
                        .await;
                    return Err(SummarizeError::Skipped(reason));
                }
                self.engine
                    .settle_node_model_input(node, model_input, crate::model_inputs::Sent::Unknown)
                    .await;
                let mut conn = self.engine.db().acquire().await.map_err(internal)?;
                let reason = crate::calls::switched_off_reason(
                    &mut conn,
                    call.provider_id,
                    &call.model_name,
                )
                .await
                .map_err(internal)?
                .unwrap_or(crate::calls::REQUEST_CANCELLED);
                return Err(SummarizeError::Skipped(reason.to_owned()));
            }
        };
        let mut tx = self.engine.db().begin().await.map_err(internal)?;
        counted(&self.engine, &mut tx, &call, &answer)
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(DbError::from).map_err(internal)?;
        if let Some(reason) = stopped {
            return Err(SummarizeError::Skipped(reason));
        }
        let mut conn = self.engine.db().acquire().await.map_err(internal)?;
        if let Some(reason) =
            crate::calls::switched_off_reason(&mut conn, call.provider_id, &call.model_name)
                .await
                .map_err(internal)?
        {
            return Err(SummarizeError::Skipped(reason.to_owned()));
        }
        Ok(answer)
    }
}

#[async_trait]
impl Complete for Host {
    async fn complete(
        &self,
        node: Id,
        mut request: ProviderComplete,
    ) -> Result<ProviderCompleteResult, SummarizeError> {
        use orch_provider_contract::calls::Purpose;
        let configured = (request.provider_id, request.model_name.clone());
        let mut tx = self.engine.db().begin().await.map_err(internal)?;
        let plan = crate::calls::plan_service(
            &mut tx,
            &crate::calls::ServiceCall {
                node,
                purpose: Purpose::Summary,
                root: self.call,
                workflow: Some(self.workflow),
                task: Some(self.task),
                provider: request.provider_id,
                model: &request.model_name,
                dimensions: None,
            },
            self.engine.inner.clock.now(),
        )
        .await
        .map_err(internal)?;
        let crate::calls::ServicePlan::Ready { call, candidate } = plan else {
            return Err(SummarizeError::Skipped(
                "Call recovery needs attention or its scheduled time".to_owned(),
            ));
        };
        if let Some(candidate) = candidate {
            if let Some(reason) =
                crate::calls::switched_off_reason(&mut tx, candidate.provider_id, &candidate.model)
                    .await
                    .map_err(internal)?
            {
                crate::calls::pause_service_in(&mut tx, node, Purpose::Summary, self.call, reason)
                    .await
                    .map_err(internal)?;
                tx.commit().await.map_err(DbError::from).map_err(internal)?;
                return Err(SummarizeError::Skipped(reason.to_owned()));
            }
            let Some(writer) = self.writer else {
                return Err(SummarizeError::Skipped(
                    "The setting writer is unavailable".to_owned(),
                ));
            };
            let target = resources::provider(&mut tx, candidate.provider_id)
                .await
                .map_err(internal)?;
            let model = resources::model(&mut tx, candidate.provider_id, &candidate.model)
                .await
                .map_err(internal)?
                .filter(|model| model.enabled);
            let reason = if !self
                .engine
                .candidate_allowed(
                    &mut tx,
                    node,
                    &orch_authz::Caller::session(writer).with_admin_mode(false),
                    &candidate,
                    Purpose::Summary,
                )
                .await
                .unwrap_or(false)
            {
                Some("candidate_not_allowed")
            } else if orch_provider_contract::calls::completion_options(&candidate.params).is_err()
            {
                Some("candidate_parameters")
            } else if target.as_ref().is_none_or(|target| {
                target.status != "ready"
                    || target.deleted_at.is_some()
                    || target.protocols.is_empty()
            }) || model.is_none()
            {
                Some("candidate_unavailable")
            } else if model
                .as_ref()
                .is_some_and(|model| !effort_is_offered(&candidate.params, &model.parameters))
            {
                Some("candidate_parameters")
            } else {
                None
            };
            if let Some(reason) = reason {
                self.skip_candidate(&mut tx, node, reason).await?;
                tx.commit().await.map_err(DbError::from).map_err(internal)?;
                return Err(SummarizeError::Skipped(
                    "The configured model candidate is unavailable".to_owned(),
                ));
            }
            let target = target.ok_or_else(|| {
                SummarizeError::Skipped("The configured provider is unavailable".to_owned())
            })?;
            let mut recovery = crate::calls::read_root(&mut tx, node, Purpose::Summary, self.call)
                .await
                .map_err(internal)?;
            let retained_protocol = recovery
                .as_ref()
                .filter(|row| row.context["next_call"] == json!(call))
                .and_then(|row| row.context["next_protocol"].as_str())
                .map(str::to_owned);
            request.protocol = retained_protocol.unwrap_or_else(|| {
                if target.protocols.contains(&request.protocol) {
                    return request.protocol.clone();
                }
                target
                    .protocols
                    .iter()
                    .find(|protocol| {
                        matches!(
                            protocol.as_str(),
                            "openai-responses" | "anthropic-messages" | "openai-chat"
                        )
                    })
                    .or_else(|| target.protocols.first())
                    .cloned()
                    .unwrap_or_default()
            });
            request.provider_id = candidate.provider_id;
            request.model_name = candidate.model.clone();
            request.params = Some(candidate.params.clone());
            if let Some(row) = recovery.as_mut() {
                row.context["next_protocol"] = json!(request.protocol);
                row.context["candidate"] = json!(candidate);
                sqlx::query("UPDATE model_call_recovery SET context=$4 WHERE node_id=$1 AND purpose='summary' AND call_key=$2 AND context->>'next_call'=$3")
                    .bind(node).bind(self.call).bind(call.to_string()).bind(&row.context).execute(&mut *tx).await.map_err(DbError::from).map_err(internal)?;
            }
        }
        tx.commit().await.map_err(DbError::from).map_err(internal)?;
        let attempt = Host {
            engine: self.engine.clone(),
            writer: self.writer,
            call,
            workflow: self.workflow,
            task: self.task,
            document: self.document,
        };
        let answer = attempt
            .complete_once(node, request, self.call, &configured)
            .await;
        {
            let mut conn = self.engine.db().acquire().await.map_err(internal)?;
            if let Err(SummarizeError::Skipped(reason)) = &answer
                && stable_pause(reason)
            {
                crate::calls::pause_service_in(
                    &mut conn,
                    node,
                    Purpose::Summary,
                    self.call,
                    reason,
                )
                .await
                .map_err(internal)?;
            } else if answer.is_err() {
                sqlx::query("UPDATE model_call_recovery SET state='waiting',due_at=NULL,context=jsonb_set(context,'{detail}',to_jsonb('service_precondition'::text)) WHERE node_id=$1 AND purpose='summary' AND call_key=$2 AND state='running'")
                    .bind(node).bind(self.call).execute(&mut *conn).await.map_err(DbError::from).map_err(internal)?;
            }
        }
        answer
    }
}
