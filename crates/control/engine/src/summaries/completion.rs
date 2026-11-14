//! Summary calls through the selected API provider with the setting writer's current authority and actual usage.

use super::internal;
use crate::access::ProviderUse;
use crate::commands::{Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::resource_facts::activity;
use async_trait::async_trait;
use orch_db::queries::{resources::thread_engine as resources, threads};
use orch_db::{Conn, DbError};
use orch_link_contract::commands::{ProviderComplete, ProviderCompleteResult, SerialKey};
use orch_summarizer_contract::summarizer::{OUTPUT_CUT, SummarizeError};
use orch_summarizer_model::{Complete, Reasoning};
use orch_types::{Id, Json};
use serde_json::json;

pub(super) mod automatic;
mod paid;

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

/// A model's efforts from the lowest up: the names the vendors use, in their order, and any other after them in
/// the order the model lists them.
fn lowest_first(efforts: &[String]) -> Vec<&str> {
    const KNOWN: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];
    let mut ranked: Vec<(usize, &str)> = efforts
        .iter()
        .map(|effort| {
            let rank = KNOWN
                .iter()
                .position(|known| known == effort)
                .unwrap_or(KNOWN.len());
            (rank, effort.as_str())
        })
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, effort)| effort).collect()
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

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
struct Call {
    id: Id,
    node_id: Id,
    topic_id: Id,
    thread_id: Option<Id>,
    provider_id: Id,
    person_id: Option<Id>,
    issued_by: Option<Id>,
    entry_id: Option<Id>,
    model_name: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// Reconciles answered calls after a lost process, without making any new provider request.
pub(super) async fn recover(
    engine: &Engine,
    conn: &mut Conn,
    limit: i64,
) -> Result<(), EngineError> {
    let calls: Vec<Call> = sqlx::query_as("SELECT id,node_id,topic_id,thread_id,provider_id,person_id,issued_by,entry_id,model_name,created_at FROM summary_calls WHERE purpose = 'summary' ORDER BY created_at,id LIMIT $1")
        .bind(limit).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    for call in calls {
        let status = engine.inner.commands.status(call.id).await?;
        match status.and_then(|status| status.outcome) {
            Some(Outcome::Succeeded(value)) => {
                engine.reconcile_paid_service_command(call.id).await?;
                settle_recovered(engine, conn, &call, crate::model_inputs::Sent::Taken).await?;
                let _: ProviderCompleteResult = serde_json::from_value(value)
                    .map_err(|error| EngineError::unreadable(error.to_string()))?;
                paid::retire(conn, call.id).await?;
            }
            Some(outcome) => {
                engine.reconcile_paid_service_command(call.id).await?;
                let sent = if matches!(outcome, Outcome::Failed(_)) {
                    crate::model_inputs::Sent::Refused
                } else {
                    crate::model_inputs::Sent::Unknown
                };
                settle_recovered(engine, conn, &call, sent).await?;
                paid::retire(conn, call.id).await?;
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

/// A single call has one persistent command and ledger identity, even after a scheduler crash.
pub(super) struct Host {
    pub(super) engine: Engine,
    pub(super) writer: Option<Id>,
    pub(super) call: Id,
    pub(super) workflow: Id,
    pub(super) task: &'static str,
    pub(super) resource: Option<super::resource::Work>,
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
        let frozen = paid::read(&self.engine, self.call)
            .await
            .map_err(internal)?;
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
        if already_issued && frozen.is_none() {
            return Err(SummarizeError::Skipped(
                "The summary has no original prepared command".into(),
            ));
        }
        if let Some(body) = frozen
            .as_ref()
            .map(|command| command.body.clone())
            .or(retained)
        {
            let original: ProviderComplete = serde_json::from_value(body).map_err(internal)?;
            if original.provider_id != request.provider_id
                || original.model_name != request.model_name
            {
                return Err(SummarizeError::Skipped(
                    "The retained model request belongs to another choice".to_owned(),
                ));
            }
            request = original;
        }
        let mut tx = self.engine.db().begin().await.map_err(internal)?;
        // A revocation committed while this runs refuses its commit; no epoch row is held meanwhile.
        orch_db::queries::transaction_guard::lock_action(&mut tx)
            .await
            .map_err(internal)?;
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
                    && crate::providers::request_status_available(&provider.status)
            })
            .ok_or_else(|| {
                SummarizeError::Skipped(
                    "the summarizer needs an available provider with forwarded access".to_owned(),
                )
            })?;
        if request.protocol.is_empty() {
            request.protocol = super::resource::completion_protocol(&provider.protocols)
                .map_err(internal)?
                .ok_or_else(|| {
                    SummarizeError::Skipped(
                        "The provider offers no generation interface".to_owned(),
                    )
                })?;
        }
        let model = resources::model(&mut tx, provider.id, &request.model_name)
            .await
            .map_err(internal)?
            .filter(|model| model.enabled)
            .ok_or_else(|| {
                SummarizeError::Skipped("the selected model is not offered".to_owned())
            })?;
        if !already_issued && frozen.is_none() {
            let parameters: orch_provider_contract::models::ModelParameters =
                orch_provider_contract::models::composed_model_parameters(
                    &model.parameters,
                    &model.groups,
                    &model.option_layers,
                    Some(&request.protocol),
                )
                .map_err(internal)?;
            // Internal request configuration, not response content or a new provider-read door.
            let endpoint =
                resources::provider_request_defaults(&mut tx, provider.id, &request.protocol)
                    .await
                    .map_err(internal)?;
            let selected = request.params.clone().unwrap_or_else(|| json!({}));
            let params = orch_model_contract::parameters::resolve(
                &request.protocol,
                &parameters,
                &endpoint,
                &selected,
            )
            .map_err(|reason| SummarizeError::Skipped(reason.to_owned()))?;
            request.params = Some(params);
            request.model_parameters = Some(parameters);
        }
        let resource = self.resource.as_ref().map(|work| work.candidate(&request));
        let resource_frame = match resource.as_ref() {
            Some(work) => Some(
                work.frame(&mut tx, self.engine.inner.clock.as_ref())
                    .await
                    .map_err(internal)?,
            ),
            None => None,
        };
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
                    // A summary runs in no environment.
                    runs_in: None,
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
        let requested_tier = request.params.as_ref().and_then(|params| {
            orch_model_contract::parameters::requested_tier_for_caps(&request.protocol, params)
        });
        let room = if already_issued {
            true
        } else if let Some(work) = resource.as_ref() {
            work.room(&self.engine, &mut tx, &request)
                .await
                .map_err(internal)?
        } else {
            self.engine
                .room_for_summary_with_tier(
                    &mut tx,
                    node,
                    provider.id,
                    &request.model_name,
                    requested_tier.as_deref(),
                )
                .await
                .map_err(internal)?
        };
        if !room {
            return Err(SummarizeError::Skipped(
                "A usage cap has no room for summaries".to_owned(),
            ));
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
        let (agent, environment) = if resource.is_none() {
            context.unwrap_or_default()
        } else {
            (None, None)
        };
        if resource.is_none()
            && !activity::accepts_context(
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
        if !already_issued {
            let input_tokens = orch_summarizer_model::context::estimated_tokens(&request.system)
                .saturating_add(orch_summarizer_model::context::estimated_tokens(
                    &request.input,
                ));
            let current = orch_provider_contract::models::composed_model_parameters(
                &model.parameters,
                &model.groups,
                &model.option_layers,
                Some(&request.protocol),
            )
            .map_err(internal)?;
            let exceeds_window = current
                .context_open()
                .or(current.context_window.filter(|window| *window > 0))
                .is_some_and(|window| {
                    i64::from(input_tokens).saturating_add(request.max_output_tokens) > window
                });
            let exceeds_context = resource
                .as_ref()
                .is_some_and(|work| input_tokens > work.context_tokens());
            if exceeds_window || exceeds_context {
                return Err(SummarizeError::Skipped("the rendered summary exceeds its input allowance or the model window after output".into()));
            }
        }
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
        let entry_id = match resource_frame.as_ref() {
            Some(frame) => super::resource::model_entry(
                &mut tx,
                frame.principal(),
                node,
                &frame.value().config,
            )
            .await
            .map_err(internal)?,
            None => None,
        };
        let call = Call {
            id: self.call,
            node_id: node,
            topic_id: topic,
            thread_id: thread.map(|thread| thread.node_id),
            provider_id: provider.id,
            person_id: Some(writer),
            issued_by: resource.as_ref().and_then(super::resource::Work::issuer),
            entry_id,
            model_name: request.model_name.clone(),
            created_at: self.engine.inner.clock.now(),
        };
        if let Some(frame) = resource_frame {
            frame
                .into_parts()
                .first_effect
                .check(self.engine.inner.clock.as_ref())
                .map_err(internal)?;
        }
        sqlx::query("INSERT INTO summary_calls (id,node_id,topic_id,thread_id,provider_id,person_id,model_name,created_at,agent_id,environment_id,issued_by,entry_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT(id) DO NOTHING")
            .bind(call.id).bind(node).bind(call.topic_id).bind(call.thread_id).bind(call.provider_id).bind(call.person_id).bind(&call.model_name).bind(call.created_at).bind(agent).bind(environment).bind(call.issued_by).bind(call.entry_id).execute(&mut *tx).await.map_err(DbError::from).map_err(internal)?;
        let admitted: Call = sqlx::query_as("SELECT id,node_id,topic_id,thread_id,provider_id,person_id,issued_by,entry_id,model_name,created_at FROM summary_calls WHERE id=$1 AND purpose='summary' AND cancel_reason IS NULL")
            .bind(call.id).fetch_one(&mut *tx).await.map_err(DbError::from).map_err(internal)?;
        let mut expected = call;
        expected.created_at = admitted.created_at;
        if expected != admitted {
            return Err(SummarizeError::Skipped(
                "The original summary receipt belongs to another source".into(),
            ));
        }
        let call = admitted;
        let attribution = if frozen.is_none() {
            Some(paid::attribution(&mut tx, &call).await.map_err(internal)?)
        } else {
            None
        };
        let outgoing = if let Some(frozen) = frozen {
            if !paid::matches(&frozen, node, self.call, resource.as_ref()) {
                return Err(SummarizeError::Skipped(
                    "The prepared summary command belongs to another source".into(),
                ));
            }
            frozen
        } else {
            let mut outgoing = Outgoing::new(
                self.call,
                machine,
                SerialKey::Provider(provider.id),
                &request,
            )
            .map_err(internal)?
            .by(paid::deadline(&call));
            if let Some(work) = resource.as_ref() {
                let authority =
                    super::resource::command::Authority::capture(&mut tx, work.clone(), &outgoing)
                        .await
                        .map_err(internal)?;
                let native =
                    crate::commands::CommandAuthority::ResourceSummary(Box::new(authority));
                let authority = work.bind_command(&outgoing, native).map_err(internal)?;
                outgoing = outgoing.with_authority(authority);
            } else {
                let source =
                    automatic::Source::capture(&mut tx, self, &call, root_call, configured)
                        .await
                        .map_err(internal)?;
                let authority = super::resource::command::Authority::capture_automatic(
                    &mut tx, source, &outgoing,
                )
                .await
                .map_err(internal)?;
                outgoing = outgoing.with_authority(
                    crate::commands::CommandAuthority::ResourceSummary(Box::new(authority)),
                );
            }
            outgoing
        };
        // Current authority is decided when this commits; an earlier change refuses the commit.
        // Only then are the prompt recorded and the call queued, each with no transaction open, so
        // a summary neither holds a turn or a revocation back nor goes out after a change that refused it.
        tx.commit().await.map_err(DbError::from).map_err(internal)?;
        let outgoing = if let Some(attribution) = attribution {
            match paid::prepare(self, outgoing, attribution).await {
                Ok(prepared) => prepared,
                Err(error) => {
                    let mut conn = self.engine.db().acquire().await.map_err(internal)?;
                    paid::retire_unprepared(&mut conn, self.call)
                        .await
                        .map_err(internal)?;
                    return Err(internal(error));
                }
            }
        } else {
            outgoing
        };
        let blocks = [
            crate::model_inputs::ModelBlock::new(
                "summary_system",
                "summary",
                request.system.clone(),
            ),
            crate::model_inputs::ModelBlock::new("summary_input", "summary", request.input.clone()),
        ];
        let model_input = match resource.as_ref() {
            Some(work) => self
                .engine
                .record_summary_model_input(node, work.request(), &self.call.to_string(), &blocks)
                .await
                .map_err(internal)?,
            None => self
                .engine
                .record_node_model_input(node, "summary", &self.call.to_string(), &blocks)
                .await
                .map_err(internal)?,
        };
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
        let outcome = match self.engine.run_paid_service_command(outgoing).await {
            Ok(outcome) => outcome,
            Err(error) => {
                let issued = self
                    .engine
                    .inner
                    .commands
                    .status(self.call)
                    .await
                    .map_err(internal)?
                    .is_some();
                if !issued {
                    let mut conn = self.engine.db().acquire().await.map_err(internal)?;
                    paid::retire_unprepared(&mut conn, self.call)
                        .await
                        .map_err(internal)?;
                }
                self.engine
                    .settle_node_model_input(
                        node,
                        model_input,
                        if issued {
                            crate::model_inputs::Sent::Unknown
                        } else {
                            crate::model_inputs::Sent::Refused
                        },
                    )
                    .await;
                return Err(internal(error));
            }
        };
        let stopped = {
            let mut conn = self.engine.db().acquire().await.map_err(internal)?;
            activity::request_stop_reason(&mut conn, self.call)
                .await
                .map_err(internal)?
        };
        self.engine
            .reconcile_paid_service_command(self.call)
            .await
            .map_err(internal)?;
        {
            let mut conn = self.engine.db().acquire().await.map_err(internal)?;
            paid::retire(&mut conn, self.call).await.map_err(internal)?;
        }
        let answer = match outcome {
            Outcome::Succeeded(value) => {
                self.engine
                    .settle_node_model_input(node, model_input, crate::model_inputs::Sent::Taken)
                    .await;
                serde_json::from_value::<ProviderCompleteResult>(value).map_err(internal)?
            }
            Outcome::Failed(error) => {
                let mut tx = self.engine.db().begin().await.map_err(internal)?;
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
                        Some(
                            "provider_busy"
                                | "rate_limited"
                                | "overloaded"
                                | "stream_interrupted"
                                | "provider_unreachable"
                                | "model_refused"
                        )
                    ));
                }
                failure["said"] = raw
                    .and_then(|raw| raw.pointer("/model_request/outcome/said"))
                    .cloned()
                    .unwrap_or(orch_types::Json::Null);
                // The answer ran into its output allowance: a retry with the same allowance runs into it
                // again, so this is told as what it is, whatever the call rules then do.
                let cut = failure
                    .as_object_mut()
                    .and_then(|failure| failure.remove("output_limit"))
                    .and_then(|limit| limit.as_bool())
                    .unwrap_or(false);
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
                if cut {
                    return Err(SummarizeError::Failed {
                        message: OUTPUT_CUT.to_owned(),
                        retryable: recovery.is_some(),
                    });
                }
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
    /// Whether the model reasons (its parameters take efforts, or a reasoning mode is on) and the lowest effort
    /// it offers that the setting writer may use here. Nothing is written.
    async fn reasoning(
        &self,
        node: Id,
        provider_id: Id,
        model_name: &str,
    ) -> Result<Reasoning, SummarizeError> {
        let mut tx = self.engine.db().begin().await.map_err(internal)?;
        let Some(provider) = resources::provider(&mut tx, provider_id)
            .await
            .map_err(internal)?
        else {
            return Ok(Reasoning::default());
        };
        let Some(protocol) =
            super::resource::completion_protocol(&provider.protocols).map_err(internal)?
        else {
            return Ok(Reasoning::default());
        };
        let Some(model) = resources::model(&mut tx, provider_id, model_name)
            .await
            .map_err(internal)?
        else {
            return Ok(Reasoning::default());
        };
        let parameters = orch_provider_contract::models::composed_model_parameters(
            &model.parameters,
            &model.groups,
            &model.option_layers,
            Some(&protocol),
        )
        .map_err(internal)?;
        let mut reasoning = Reasoning {
            reasons: parameters.reasons(),
            lowest_effort: None,
        };
        if reasoning.reasons
            && let Some(writer) = self.writer
        {
            for effort in lowest_first(parameters.efforts()) {
                let allowed = self
                    .engine
                    .inner
                    .access
                    .background_provider_use(
                        &mut tx,
                        &ProviderUse {
                            person: writer,
                            node,
                            provider: provider_id,
                            model: Some(model_name),
                            effort: Some(effort),
                            // A summary runs in no environment.
                            runs_in: None,
                        },
                    )
                    .await;
                if allowed.is_ok_and(|allowed| allowed.is_allowed()) {
                    reasoning.lowest_effort = Some(effort.to_owned());
                    break;
                }
            }
        }
        Ok(reasoning)
    }

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
            let candidate = *candidate;
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
            let supported = match target.as_ref() {
                Some(provider) => super::resource::completion_protocol(&provider.protocols)
                    .map_err(internal)?
                    .is_some(),
                None => false,
            };
            let original;
            let caller = if let Some(work) = self.resource.as_ref() {
                original = work
                    .caller(&mut tx, self.engine.inner.clock.as_ref())
                    .await
                    .map_err(internal)?;
                &original
            } else {
                original = orch_authz::Caller::session(writer).with_admin_mode(false);
                &original
            };
            let reason = if !self
                .engine
                .candidate_allowed(&mut tx, node, caller, &candidate, Purpose::Summary)
                .await
                .unwrap_or(false)
            {
                Some("candidate_not_allowed")
            } else if orch_provider_contract::calls::completion_options(&candidate.params).is_err()
            {
                Some("candidate_parameters")
            } else if target.as_ref().is_none_or(|target| {
                !crate::providers::request_status_available(&target.status)
                    || target.deleted_at.is_some()
                    || target.protocols.is_empty()
            }) || model.is_none()
                || !supported
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
                super::resource::completion_protocol(&target.protocols)
                    .ok()
                    .flatten()
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
            resource: self.resource.clone(),
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
