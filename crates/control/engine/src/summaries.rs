//! Title/description/tag triggers, manual override races, limits and usage.
//!
//! Pending triggers survive a restart, model calls run outside the turn's transaction, and a person's
//! edit wins at commit. The selected producer sees only current redacted material; every real model
//! request is counted before its output is judged.

mod budget;
mod completion;
mod documents;
mod estimate;
pub(crate) mod goal_check;
mod handoff;

pub(crate) use budget::InputBudget;
pub(crate) use estimate::handoff_estimate;
pub use handoff::{
    Carried, IndexedTranscript, handoff, indexed_transcript, notice as handoff_notice,
};
pub(crate) use handoff::{
    FRAME_BYTES as HANDOFF_FRAME_BYTES, HEADINGS_BYTES as HANDOFF_HEADINGS_BYTES, brief_bytes,
    capture_handoff_choice, handoff_budget, handoff_captured, indexed_transcript_file,
};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::unit::Unit;
use chrono::{DateTime, Utc};
use orch_db::queries::{hierarchy, settings as stored, threads};
use orch_db::{Conn, DbError};
use orch_plugin_contract::refs::{NodeKind, NodeRef};
use orch_settings::{Place, get as setting, resolve};
use orch_settings_contract::registry::keys;
use orch_summarizer_contract::activity::{ActivityItem, ActivityKind, trim};
use orch_summarizer_contract::descriptor::{SummaryField, split_setting};
use orch_summarizer_contract::result::SummaryResult;
use orch_summarizer_contract::summarizer::{
    SummarizeError, Summarizer, SummaryLimits, SummaryRequest, next_interval,
};
use orch_summarizer_model::Model;
use orch_summarizer_model::descriptor::{INPUT_CHARS, INPUT_ITEMS};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

/// The shipped minimum interval when no scope changes the trigger setting.
const MIN_INTERVAL_SECS: i64 = 120;
/// At most three sequential field calls, with one command deadline each, fit inside this lease.
const CLAIM_GRACE_SECS: i64 = 30;
const CALLS_PER_RUN: i64 = 3;
/// Four independent summaries keep the supervisor's scan bounded without waiting for a model.
const SUMMARY_BATCH: i64 = 4;
/// The first-input title is a short list label, cut at a word boundary.
const FIRST_TITLE_CHARS: usize = 60;

/// A model answer is publishable only for the exact execution selection it read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct History {
    continuation: Id,
    revision: i64,
}

impl From<&threads::ThreadRow> for History {
    fn from(row: &threads::ThreadRow) -> Self {
        Self {
            continuation: row.active_continuation_id,
            revision: row.execution_revision,
        }
    }
}

fn stored_error(error: impl std::fmt::Display) -> EngineError {
    EngineError::unreadable(error.to_string())
}

fn seconds(value: Option<&Json>, key: &str, fallback: i64) -> i64 {
    value
        .and_then(|value| value.get(key))
        .and_then(Json::as_i64)
        .unwrap_or(fallback)
        .max(0)
}

/// Makes the selected event due in the caller's transaction; this performs no model call.
pub async fn due(
    conn: &mut Conn,
    node: Id,
    event: &str,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    let triggers: Option<Json> = setting(conn, Place::Node(node), keys::AUTO_TRIGGERS)
        .await
        .map_err(stored_error)?;
    if event != "refresh"
        && event != "rollup"
        && !triggers
            .as_ref()
            .and_then(|value| value.get("on"))
            .and_then(Json::as_array)
            .is_some_and(|events| events.iter().any(|value| value.as_str() == Some(event)))
    {
        return Ok(());
    }
    if event == "refresh" {
        sqlx::query("UPDATE model_call_recovery SET state='dismissed',due_at=NULL WHERE node_id=$1 AND purpose='summary' AND COALESCE(context->>'task','') NOT IN ('handoff','document_description') AND state IN ('waiting','stopped')")
            .bind(node).execute(&mut *conn).await.map_err(DbError::from)?;
    }
    sqlx::query("INSERT INTO node_summary_state (node_id,due_at,next_at) VALUES ($1,$2,$2) ON CONFLICT(node_id) DO UPDATE SET due_at=LEAST(COALESCE(node_summary_state.due_at,EXCLUDED.due_at),EXCLUDED.due_at)")
        .bind(node).bind(now).execute(conn).await.map_err(DbError::from)?;
    Ok(())
}

/// The last safe reason that the node's automation setting can show.
pub async fn reason(conn: &mut Conn, node: Id) -> Result<Option<String>, EngineError> {
    Ok(sqlx::query_scalar::<_, Option<String>>(
        "SELECT reason FROM (
            SELECT reason,last_at FROM node_summary_state WHERE node_id=$1 AND reason IS NOT NULL
            UNION ALL
            SELECT s.reason,s.last_at FROM document_summary_state s JOIN documents d ON d.id=s.document_id
             WHERE d.node_id=$1 AND d.deleted_at IS NULL AND s.reason IS NOT NULL
            ) reasons ORDER BY last_at DESC NULLS LAST LIMIT 1",
    )
    .bind(node)
    .fetch_optional(conn)
    .await
    .map_err(DbError::from)?
    .flatten())
}

/// Whether a helper model checks a goal's report of "met" at this thread's place: the summary producer is the model
/// kind, with a provider and a model chosen.
pub(crate) async fn goal_helper_available(conn: &mut Conn, node: Id) -> Result<bool, EngineError> {
    goal_check::available(conn, node).await
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Choice {
    kind: String,
    config: Json,
    writer: Option<Id>,
}

async fn choice(conn: &mut Conn, node: Id) -> Result<Option<Choice>, EngineError> {
    let resolved = resolve(conn, Place::Node(node), keys::AUTO_SUMMARIZER)
        .await
        .map_err(stored_error)?;
    let Some(value) = resolved.value().filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let (kind, config) = split_setting(value).map_err(stored_error)?;
    let writer = stored::row(conn, resolved.resolved.source.node, keys::AUTO_SUMMARIZER)
        .await?
        .and_then(|row| row.updated_by);
    Ok(Some(Choice {
        kind,
        config,
        writer,
    }))
}

enum ModelFailure {
    Unobserved,
    Handled,
    Unmatched,
}

impl Engine {
    /// A paid model answer that cannot supply its requested summary still belongs to call policy.
    async fn model_output_failed(
        &self,
        chosen: &Choice,
        node: Id,
        root: Id,
        workflow: Id,
        task: &str,
    ) -> Result<ModelFailure, EngineError> {
        use orch_provider_contract::calls::Purpose;
        let previous = {
            let mut conn = self.db().acquire().await?;
            crate::calls::read_root(&mut conn, node, Purpose::Summary, root).await?
        };
        if previous.as_ref().is_some_and(|row| {
            matches!(
                row.state.as_str(),
                "scheduled" | "waiting" | "stopped" | "dismissed"
            )
        }) {
            return Ok(ModelFailure::Handled);
        }
        let reference = previous
            .as_ref()
            .and_then(|row| row.context["next_call"].as_str())
            .and_then(|value| value.parse::<Id>().ok())
            .unwrap_or(root);
        if !self
            .inner
            .commands
            .status(reference)
            .await?
            .and_then(|status| status.outcome)
            .is_some_and(|outcome| matches!(outcome, crate::commands::Outcome::Succeeded(_)))
        {
            return Ok(ModelFailure::Unobserved);
        }
        let configured = chosen.config["provider_id"]
            .as_str()
            .and_then(|value| value.parse::<Id>().ok())
            .ok_or_else(|| EngineError::unreadable("the model configuration has no provider"))?;
        let model = chosen.config["model_name"]
            .as_str()
            .ok_or_else(|| EngineError::unreadable("the model configuration has no model"))?;
        let mut context = previous.map_or_else(|| json!({}), |row| row.context);
        context["author_id"] = json!(chosen.writer);
        context["provider_id"] = context
            .pointer("/candidate/provider_id")
            .cloned()
            .unwrap_or_else(|| json!(configured));
        context["model"] = context
            .pointer("/candidate/model")
            .cloned()
            .unwrap_or_else(|| json!(model));
        let mut tx = self.db().begin().await?;
        let recovery=crate::calls::reserve_service(&mut tx,&crate::calls::ServiceCall {node,purpose:Purpose::Summary,root,workflow:Some(workflow),task:Some(task),provider:configured,model,dimensions:None},reference,
            &json!({"code":"harness_error","message":"The model returned no usable summary","retryable":true}),context,self.inner.clock.now()).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(if recovery.is_some() {
            ModelFailure::Handled
        } else {
            ModelFailure::Unmatched
        })
    }

    /// Registers an in-process or plugin-backed producer by its declared kind.
    pub fn attach_summarizer(&self, producer: Arc<dyn Summarizer>) {
        let name = producer.describe().name;
        self.inner
            .summarizers
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name, producer);
    }

    /// The chosen producer's declaration, available to validate its configuration before a setting save.
    pub fn summarizer_config_schema(&self, kind: &str) -> Option<Json> {
        if kind == "model" {
            return Some(orch_summarizer_model::descriptor::descriptor().config_schema);
        }
        self.inner
            .summarizers
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(kind)
            .map(|producer| producer.describe().config_schema)
    }

    fn producer(
        &self,
        chosen: &Choice,
        call: Id,
        workflow: Id,
        field: SummaryField,
    ) -> Option<Arc<dyn Summarizer>> {
        if chosen.kind == "model" {
            return Some(Arc::new(Model::new(Arc::new(completion::Host {
                engine: self.clone(),
                writer: chosen.writer,
                call,
                workflow,
                task: field_name(field),
                document: None,
            }))));
        }
        self.inner
            .summarizers
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&chosen.kind)
            .cloned()
    }

    fn document_producer(
        &self,
        chosen: &Choice,
        call: Id,
        document: Id,
        seq: i64,
        workflow: Id,
    ) -> Option<Arc<dyn Summarizer>> {
        if chosen.kind == "model" {
            return Some(Arc::new(Model::new(Arc::new(completion::Host {
                engine: self.clone(),
                writer: chosen.writer,
                call,
                workflow,
                task: "document_description",
                document: Some(completion::DocumentContext { id: document, seq }),
            }))));
        }
        self.producer(chosen, call, workflow, SummaryField::Description)
    }

    /// Whether a chosen producer can supply a genuine summary for this conversation.
    pub async fn summary_available(&self, conn: &mut Conn, node: Id) -> Result<bool, EngineError> {
        let Some(chosen) = choice(conn, node).await? else {
            return Ok(false);
        };
        Ok(self
            .producer(&chosen, new_id(), new_id(), SummaryField::Handoff)
            .is_some_and(|producer| producer.describe().writes(SummaryField::Handoff)))
    }

    /// Looks for due summaries without awaiting any provider in the turn supervisor.
    pub(crate) async fn keep_summaries(&self) -> Result<(), EngineError> {
        if self.is_stopped() {
            return Ok(());
        }
        let now = self.inner.clock.now();
        let mut conn = self.db().acquire().await?;
        completion::recover(self, &mut conn, SUMMARY_BATCH).await?;
        // Interval candidates rotate by last attempt; a large tree is not scanned by id forever.
        let running: Vec<Id> = sqlx::query_scalar("SELECT t.node_id FROM threads t JOIN nodes n ON n.id=t.node_id LEFT JOIN node_summary_state s ON s.node_id=t.node_id WHERE t.state IN ('running','waiting_approval') AND n.deleted_at IS NULL AND n.archived_at IS NULL ORDER BY s.last_at NULLS FIRST,t.node_id LIMIT $1")
            .bind(self.config().sweep_batch).fetch_all(&mut *conn).await.map_err(DbError::from)?;
        for thread in running {
            let triggers: Option<Json> =
                setting(&mut conn, Place::Node(thread), keys::AUTO_TRIGGERS)
                    .await
                    .map_err(stored_error)?;
            let interval = seconds(triggers.as_ref(), "interval_secs", MIN_INTERVAL_SECS);
            let last: Option<DateTime<Utc>> = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
                "SELECT last_at FROM node_summary_state WHERE node_id=$1",
            )
            .bind(thread)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?
            .flatten();
            if last.is_none_or(|at| at + chrono::Duration::seconds(interval) <= now) {
                due(&mut conn, thread, "interval", now).await?;
            }
        }
        let subjects: Vec<(Id,bool)> = sqlx::query_as("SELECT subject,document FROM (
            SELECT s.node_id AS subject,false AS document,s.due_at FROM node_summary_state s JOIN nodes n ON n.id=s.node_id
             WHERE (s.due_at<=$1 OR s.run_id IS NOT NULL) AND s.next_at<=$1 AND (s.claimed_until IS NULL OR s.claimed_until<=$1) AND n.deleted_at IS NULL AND n.archived_at IS NULL
            UNION ALL
            SELECT s.document_id,true,s.due_at FROM document_summary_state s JOIN documents d ON d.id=s.document_id JOIN nodes n ON n.id=d.node_id
             WHERE (s.due_at<=$1 OR s.run_id IS NOT NULL) AND s.next_at<=$1 AND (s.claimed_until IS NULL OR s.claimed_until<=$1) AND d.deleted_at IS NULL AND n.deleted_at IS NULL AND n.archived_at IS NULL
            ) due ORDER BY due_at NULLS FIRST,subject LIMIT $2")
            .bind(now).bind(SUMMARY_BATCH).fetch_all(&mut *conn).await.map_err(DbError::from)?;
        drop(conn);
        for (subject, document) in subjects {
            if self.is_stopped() {
                return Ok(());
            }
            let engine = self.clone();
            tokio::spawn(async move {
                if engine.is_stopped() {
                    return;
                }
                let result = if document {
                    engine.summarize_document_due(subject).await
                } else {
                    engine.summarize_due(subject).await
                };
                if let Err(error) = result {
                    tracing::warn!(%error,%subject,"a summary could not be carried out; pending work is kept");
                }
            });
        }
        Ok(())
    }

    /// Carries out one due attempt; the claim makes this safe across multiple API processes.
    pub async fn summarize_due(&self, node: Id) -> Result<bool, EngineError> {
        let now = self.inner.clock.now();
        let claimant = new_id();
        let until = now
            + chrono::Duration::seconds(
                CALLS_PER_RUN
                    * i64::from(
                        orch_link_contract::commands::CommandKind::ProviderComplete.deadline_secs(),
                    )
                    + CLAIM_GRACE_SECS,
            );
        let mut conn = self.db().acquire().await?;
        let claim: Option<(Id, i64, i64)> = sqlx::query_as("UPDATE node_summary_state SET run_id=COALESCE(run_id,$2),claimed_by=$3,claimed_until=$4,due_at=NULL WHERE node_id=$1 AND (due_at <= $5 OR run_id IS NOT NULL) AND next_at <= $5 AND (claimed_until IS NULL OR claimed_until <= $5) RETURNING run_id,last_seq,failures")
            .bind(node).bind(new_id()).bind(claimant).bind(until).bind(now).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
        let Some((run, cursor, failures)) = claim else {
            return Ok(false);
        };
        let triggers: Option<Json> = setting(&mut conn, Place::Node(node), keys::AUTO_TRIGGERS)
            .await
            .map_err(stored_error)?;
        let min = seconds(triggers.as_ref(), "min_interval_secs", MIN_INTERVAL_SECS);
        let history = threads::thread(&mut conn, node)
            .await?
            .as_ref()
            .map(History::from);
        drop(conn);
        let attempted = self.generate(node, run, cursor, history).await;
        let (reason, failure, consumed) = match attempted {
            Ok(seq) => {
                metrics::counter!("orch_summary_attempts_total", "result" => "completed")
                    .increment(1);
                (None, false, Some(seq))
            }
            Err(error) => {
                let failure = error.is_failure();
                metrics::counter!("orch_summary_attempts_total", "result" => if failure { "failed" } else { "skipped" }).increment(1);
                if failure {
                    tracing::warn!(code=error.code(), %node, "the summarizer attempt failed");
                }
                (Some(safe_reason(&error)), failure, None)
            }
        };
        let unmatched = reason.as_deref().is_some_and(completion::stable_pause);
        let backoff = if failure {
            next_interval(
                Duration::from_secs(u64::try_from(min).unwrap_or(u64::MAX)),
                u32::try_from(failures + 1).unwrap_or(u32::MAX),
            )
        } else {
            Duration::from_secs(u64::try_from(min).unwrap_or(u64::MAX))
        };
        let next =
            self.inner.clock.now() + chrono::Duration::from_std(backoff).map_err(stored_error)?;
        let mut conn = self.db().acquire().await?;
        sqlx::query("WITH current_history AS (SELECT ($8::uuid IS NULL OR EXISTS(SELECT 1 FROM threads
            WHERE node_id=$1 AND active_continuation_id=$8 AND execution_revision=$9)) AS same)
          UPDATE node_summary_state SET last_at=$3,next_at=CASE WHEN h.same THEN $4 ELSE next_at END,
            failures=CASE WHEN NOT h.same THEN failures WHEN $5 THEN failures+1 ELSE 0 END,
            reason=CASE WHEN h.same THEN $6 ELSE reason END,
            last_seq=CASE WHEN h.same THEN COALESCE($7,last_seq) ELSE last_seq END,
            run_id=NULL,claimed_by=NULL,claimed_until=NULL,
            due_at=CASE WHEN h.same AND $10 THEN NULL WHEN h.same AND $5 THEN COALESCE(due_at,$4) ELSE due_at END
          FROM current_history h WHERE node_id=$1 AND claimed_by=$2")
            .bind(node).bind(claimant).bind(self.inner.clock.now()).bind(next).bind(failure).bind(reason).bind(consumed)
            .bind(history.map(|history|history.continuation)).bind(history.map(|history|history.revision)).bind(unmatched)
            .execute(&mut *conn).await.map_err(DbError::from)?;
        if let Some(recovery) = crate::calls::read_workflow(
            &mut conn,
            node,
            orch_provider_contract::calls::Purpose::Summary,
            run,
        )
        .await?
            && matches!(recovery.state.as_str(), "scheduled" | "stopped" | "waiting")
        {
            sqlx::query("UPDATE node_summary_state SET next_at=COALESCE($2,next_at),due_at=$2,run_id=CASE WHEN $2::timestamptz IS NOT NULL THEN $3 ELSE NULL END,reason=$4 WHERE node_id=$1")
                .bind(node).bind(recovery.due_at).bind(run)
                .bind(if recovery.state=="scheduled" {"calls.serviceScheduled"} else {crate::calls::administrative_pause_reason(&recovery).unwrap_or("calls.servicePaused")})
                .execute(&mut *conn).await.map_err(DbError::from)?;
        }
        Ok(true)
    }

    async fn generate(
        &self,
        node: Id,
        run: Id,
        cursor: i64,
        history: Option<History>,
    ) -> Result<i64, SummarizeError> {
        let mut conn = self.db().acquire().await.map_err(internal)?;
        let expected_epoch = orch_authz::epoch::current(&mut conn)
            .await
            .map_err(internal)?;
        let before = hierarchy::details(&mut conn, &[node], self.inner.clock.now())
            .await
            .map_err(internal)?
            .into_iter()
            .next()
            .filter(|row| row.deleted_at.is_none() && row.archived_at.is_none())
            .ok_or_else(|| SummarizeError::Skipped("the node is unavailable".to_owned()))?;
        if hierarchy::with_ancestors(&mut conn, &[node])
            .await
            .map_err(internal)?
            .iter()
            .any(|place| place.archived_at.is_some() || place.deleted_at.is_some())
        {
            return Err(SummarizeError::Skipped(
                "the place is archived or unavailable".to_owned(),
            ));
        }
        let thread = threads::thread(&mut conn, node).await.map_err(internal)?;
        if thread.as_ref().map(History::from) != history {
            return Err(SummarizeError::Skipped(
                "the selected conversation changed".to_owned(),
            ));
        }
        // Read separately: no connection is held by overlapping futures.
        let title: Option<String> = setting(&mut conn, Place::Node(node), keys::AUTO_TITLE)
            .await
            .map_err(internal)?;
        let description: Option<String> =
            setting(&mut conn, Place::Node(node), keys::AUTO_DESCRIPTION)
                .await
                .map_err(internal)?;
        let rollup: Option<String> = setting(&mut conn, Place::Node(node), keys::AUTO_TOPIC_ROLLUP)
            .await
            .map_err(internal)?;
        let tags: Option<String> = setting(&mut conn, Place::Node(node), keys::AUTO_TAGS)
            .await
            .map_err(internal)?;
        let mut fields = Vec::new();
        if before.kind == "thread"
            && before.title_source == "auto"
            && title.as_deref() == Some("summarizer")
        {
            fields.push(SummaryField::Title);
        }
        if before.description_source == "auto" {
            if before.kind == "topic" && rollup.as_deref() == Some("rollup") {
                fields.push(SummaryField::Rollup);
            } else if description.as_deref() == Some("summarizer") {
                fields.push(SummaryField::Description);
            }
        }
        if before.tags_source.as_deref() == Some("auto") && tags.as_deref() == Some("summarizer") {
            fields.push(SummaryField::Tags);
        }
        if fields.is_empty() {
            return Ok(cursor);
        }
        let chosen = choice(&mut conn, node)
            .await
            .map_err(internal)?
            .ok_or_else(|| SummarizeError::Skipped("no summarizer is configured".to_owned()))?;
        let (input_items, input_chars) = if chosen.kind == "model" {
            let config = orch_summarizer_model::descriptor::Config::read(&chosen.config)?;
            (config.input_items, config.input_chars)
        } else {
            (INPUT_ITEMS, INPUT_CHARS)
        };
        let (activity, upto) = if let Some(history) = history {
            activity(
                &mut conn,
                node,
                history.continuation,
                cursor,
                input_items,
                input_chars,
            )
            .await?
        } else {
            (
                topic_activity(
                    &mut conn,
                    node,
                    input_items,
                    input_chars,
                    self.inner.clock.now(),
                    chosen.writer,
                )
                .await?,
                cursor,
            )
        };
        let definitions = hierarchy::tags_of(&mut conn, &before.path)
            .await
            .map_err(internal)?;
        let allowed_tags: Vec<String> = definitions.iter().map(|tag| tag.name.clone()).collect();
        let old_tags = hierarchy::node_tags(&mut conn, &[node])
            .await
            .map_err(internal)?;
        let tag_limit: Option<u32> =
            setting(&mut conn, Place::Node(node), keys::TAGS_MAX_PER_PLACE)
                .await
                .map_err(internal)?;
        let (topic, environment) = thread.as_ref().map_or((node, None), |thread| {
            (thread.topic_id, thread.environment_id)
        });
        let current = crate::raw_events::mask_summary(&mut conn, node, topic, environment, json!({"name":before.name,"description":before.description,"tags":allowed_tags,"previous_tags":old_tags.iter().map(|tag| tag.name.as_str()).collect::<Vec<_>>().join(", ")})).await.map_err(internal)?
            .ok_or_else(|| SummarizeError::Skipped("redaction is unavailable".to_owned()))?;
        let title_now = current["name"].as_str().unwrap_or_default().to_owned();
        let description_now = current["description"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let allowed_tags: Vec<String> = current["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .map(str::to_owned)
            .collect();
        drop(conn);
        let mut made = Vec::new();
        let mut failure = None;
        for field in fields {
            let limits = SummaryLimits::for_field(field, tag_limit.unwrap_or_default());
            let request = SummaryRequest {
                field,
                node: NodeRef {
                    id: node,
                    kind: if before.kind == "thread" {
                        NodeKind::Thread
                    } else {
                        NodeKind::Topic
                    },
                    name: Some(title_now.clone()),
                    redacted: false,
                },
                previous: match field {
                    SummaryField::Title => title_now.clone(),
                    SummaryField::Tags => current["previous_tags"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    _ => description_now.clone(),
                },
                activity: activity.clone(),
                tag_choices: allowed_tags.clone(),
                limits,
                config: chosen.config.clone(),
            };
            // Changed material or configuration makes another real request in a recovered run.
            // Earlier answered requests are reconciled by their captured attribution.
            let body = serde_json::to_vec(&json!({"request":request,"writer":chosen.writer}))
                .map_err(internal)?;
            let digest = orch_types::Hash::of(&body);
            let call = crate::ids::derived(
                "summary.field",
                &[run.as_bytes().as_slice(), digest.as_bytes().as_slice()],
            );
            let producer = self.producer(&chosen, call, run, field).ok_or_else(|| {
                SummarizeError::Skipped("the chosen producer is not installed".to_owned())
            })?;
            if !producer.describe().writes(field) {
                measure(field, "unsupported");
                failure = Some(SummarizeError::Unsupported(field));
                continue;
            }
            let result = match producer.summarize(request).await {
                Ok(result) => result.conform(field, &limits, &allowed_tags),
                Err(error) => {
                    if matches!(&error,SummarizeError::Skipped(reason) if completion::stable_pause(reason))
                    {
                        return Err(error);
                    }
                    if chosen.kind == "model" {
                        match self
                            .model_output_failed(&chosen, node, call, run, field_name(field))
                            .await
                            .map_err(internal)?
                        {
                            ModelFailure::Handled => return Err(error),
                            ModelFailure::Unmatched => {
                                return Err(SummarizeError::Skipped(
                                    completion::NO_RULE.to_owned(),
                                ));
                            }
                            ModelFailure::Unobserved => {}
                        }
                    }
                    measure(
                        field,
                        if error.is_failure() {
                            "failed"
                        } else {
                            "skipped"
                        },
                    );
                    failure = Some(error);
                    continue;
                }
            };
            if result.discarded {
                if chosen.kind == "model" {
                    match self
                        .model_output_failed(&chosen, node, call, run, field_name(field))
                        .await
                        .map_err(internal)?
                    {
                        ModelFailure::Handled => {
                            return Err(SummarizeError::Failed {
                                message: "The model returned no usable summary".to_owned(),
                                retryable: true,
                            });
                        }
                        ModelFailure::Unmatched => {
                            return Err(SummarizeError::Skipped(completion::NO_RULE.to_owned()));
                        }
                        ModelFailure::Unobserved => {}
                    }
                }
                measure(field, "discarded");
                failure = Some(SummarizeError::Failed {
                    message: "the answer contained no usable summary".to_owned(),
                    retryable: true,
                });
                continue;
            }
            if chosen.kind == "model" {
                let mut conn = self.db().acquire().await.map_err(internal)?;
                crate::calls::finish_service(
                    &mut conn,
                    node,
                    orch_provider_contract::calls::Purpose::Summary,
                    call,
                )
                .await
                .map_err(internal)?;
            }
            made.push((field, result.result));
        }
        self.commit_summaries(
            &before,
            &chosen,
            expected_epoch,
            history,
            &made,
            &definitions,
        )
        .await
        .map_err(internal)?;
        for (field, _) in &made {
            measure(*field, "ok");
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(upto)
    }

    async fn commit_summaries(
        &self,
        before: &hierarchy::NodeDetail,
        chosen: &Choice,
        expected_epoch: i64,
        history: Option<History>,
        made: &[(SummaryField, SummaryResult)],
        definitions: &[hierarchy::TagRow],
    ) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let mut tx = self.db().begin().await?;
        // What was decided under `expected_epoch` is written only if no change of authority commits first:
        // the commit checks it, without holding the epoch's row while this runs.
        orch_db::queries::transaction_guard::expect_cache_epoch(&mut tx, expected_epoch).await?;
        if orch_authz::epoch::current(&mut tx).await? != expected_epoch {
            return Ok(());
        }
        hierarchy::lock(&mut tx, before.id).await?;
        if before.kind == "thread" {
            let current = threads::lock(&mut tx, before.id).await?;
            if current.as_ref().map(History::from) != history {
                return Ok(());
            }
        }
        let Some(current) = hierarchy::details(&mut tx, &[before.id], now)
            .await?
            .into_iter()
            .next()
        else {
            return Ok(());
        };
        // A manual edit followed by Resume is still an edit in flight: its revision wins too.
        if current.revision != before.revision
            || current.deleted_at.is_some()
            || current.archived_at.is_some()
        {
            return Ok(());
        }
        if choice(&mut tx, before.id).await?.as_ref() != Some(chosen) {
            return Ok(());
        }
        let mut unit = Unit::new(&mut tx, now);
        let mut changed = false;
        for (field, result) in made {
            let (key, wanted) = match field {
                SummaryField::Title => (keys::AUTO_TITLE, "summarizer"),
                SummaryField::Description => (keys::AUTO_DESCRIPTION, "summarizer"),
                SummaryField::Tags => (keys::AUTO_TAGS, "summarizer"),
                SummaryField::Rollup => (keys::AUTO_TOPIC_ROLLUP, "rollup"),
                SummaryField::Handoff => continue,
            };
            let mode: Option<String> = setting(&mut *unit.conn, Place::Node(before.id), key)
                .await
                .map_err(stored_error)?;
            if mode.as_deref() != Some(wanted) {
                continue;
            }
            match field {
                SummaryField::Title if current.title_source == "auto" => {
                    if let Some(title) = result
                        .text
                        .as_deref()
                        .filter(|title| *title != current.name.as_str())
                    {
                        changed |=
                            threads::rename_if_automatic(&mut *unit.conn, before.id, title, now)
                                .await?;
                        if before.kind == "thread" {
                            if let Some(row) = threads::thread(&mut *unit.conn, before.id).await?
                                && let Some(segment) = row.current_segment_id
                            {
                                let marker = json!({"marker":"renamed","data":{"from":before.name,"to":title,"by":null}});
                                crate::ingest::add_marker(
                                    &mut unit,
                                    before.id,
                                    segment,
                                    None,
                                    &marker,
                                    &format!("summary-title:{}", before.revision),
                                )
                                .await?;
                            }
                            follow_topic_title(
                                &mut *unit.conn,
                                before.id,
                                title,
                                now,
                                chosen.writer,
                            )
                            .await?;
                        }
                    }
                }
                SummaryField::Description | SummaryField::Rollup
                    if current.description_source == "auto" =>
                {
                    if let Some(text) = result
                        .text
                        .as_deref()
                        .filter(|text| *text != current.description.as_str())
                    {
                        sqlx::query("UPDATE nodes SET description=$2,auto_updated_at=$3,updated_at=$3,revision=revision+1 WHERE id=$1 AND description_source='auto'")
                            .bind(before.id).bind(text).bind(now).execute(&mut *unit.conn).await.map_err(DbError::from)?;
                        changed = true;
                    }
                }
                SummaryField::Tags if current.tags_source.as_deref() == Some("auto") => {
                    if let Some(names) = &result.tags {
                        let ids: Vec<_> = names
                            .iter()
                            .filter_map(|name| {
                                definitions
                                    .iter()
                                    .find(|tag| tag.name == *name)
                                    .map(|tag| tag.id)
                            })
                            .collect();
                        hierarchy::set_node_tags(&mut *unit.conn, before.id, &ids, now).await?;
                        sqlx::query("UPDATE nodes SET tags_source='auto' WHERE id=$1")
                            .bind(before.id)
                            .execute(&mut *unit.conn)
                            .await
                            .map_err(DbError::from)?;
                        sqlx::query("UPDATE nodes SET auto_updated_at=$2,updated_at=$2,revision=revision+1 WHERE id=$1")
                            .bind(before.id).bind(now).execute(&mut *unit.conn).await.map_err(DbError::from)?;
                        changed = true;
                    }
                }
                _ => {}
            }
        }
        if changed {
            if before.kind == "thread" {
                let seq = unit.stamp(before.id).await?;
                threads::set_seq_state(&mut *unit.conn, before.id, seq).await?;
            }
            hierarchy::stamp(&mut *unit.conn, before.id).await?;
        }
        if before.kind == "thread"
            && !made.is_empty()
            && let Some(thread) = threads::thread(&mut *unit.conn, before.id).await?
        {
            due(&mut *unit.conn, thread.topic_id, "rollup", now).await?;
        }
        unit.finish().await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }
}

fn field_name(field: SummaryField) -> &'static str {
    match field {
        SummaryField::Title => "title",
        SummaryField::Description => "description",
        SummaryField::Tags => "tags",
        SummaryField::Rollup => "rollup",
        SummaryField::Handoff => "handoff",
    }
}

fn measure(field: SummaryField, result: &'static str) {
    metrics::counter!("orch_summaries_total","field"=>field_name(field),"result"=>result)
        .increment(1);
}

fn internal(error: impl std::fmt::Display) -> SummarizeError {
    SummarizeError::Internal(error.to_string())
}

fn safe_reason(error: &SummarizeError) -> String {
    match error {
        SummarizeError::Skipped(reason) => reason.clone(),
        SummarizeError::Config(_) => "The summarizer configuration is not usable.".to_owned(),
        SummarizeError::Unsupported(_) => {
            "The chosen summarizer does not write this field.".to_owned()
        }
        SummarizeError::Failed { .. } => {
            "The summarizer returned no usable answer; it will try again after the backoff."
                .to_owned()
        }
        SummarizeError::Internal(_) => {
            "The summarizer could not complete; it will try again after the backoff.".to_owned()
        }
    }
}

async fn activity(
    conn: &mut Conn,
    node: Id,
    view: Id,
    since: i64,
    count: u32,
    chars: u32,
) -> Result<(Vec<ActivityItem>, i64), SummarizeError> {
    let row = threads::thread(conn, node)
        .await
        .map_err(internal)?
        .ok_or_else(|| SummarizeError::Skipped("the thread is unavailable".to_owned()))?;
    let items = orch_db::queries::thread_history::summary_activity(
        conn,
        node,
        view,
        since,
        i64::from(count),
    )
    .await
    .map_err(internal)?;
    let mut out = Vec::new();
    let mut upto = since;
    for (kind, payload, at, seq) in items.into_iter().rev() {
        let (kind, text) = match kind.as_str() {
            "user_message" => (
                ActivityKind::UserMessage,
                payload["parts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|part| part.get("text").and_then(Json::as_str))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            "agent_message" => (
                ActivityKind::AgentMessage,
                payload["text"].as_str().unwrap_or_default().to_owned(),
            ),
            "reasoning" => (
                ActivityKind::Reasoning,
                payload["summary"].as_str().unwrap_or_default().to_owned(),
            ),
            "tool_call" => (
                ActivityKind::ToolCall,
                payload["title"].as_str().unwrap_or_default().to_owned(),
            ),
            "plan" => (
                ActivityKind::Plan,
                payload["steps"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|step| step.get("text").and_then(Json::as_str))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => (
                ActivityKind::Error,
                payload["message"].as_str().unwrap_or_default().to_owned(),
            ),
        };
        let masked = crate::raw_events::mask_live(conn, &row, row.environment_id, json!(text))
            .await
            .map_err(internal)?
            .ok_or_else(|| SummarizeError::Skipped("redaction is unavailable".to_owned()))?;
        out.push(ActivityItem {
            kind,
            text: masked.as_str().unwrap_or_default().to_owned(),
            at: Some(Time::new(at)),
        });
        upto = upto.max(seq);
    }
    Ok((trim(out, count, chars), upto))
}

async fn topic_activity(
    conn: &mut Conn,
    topic: Id,
    count: u32,
    chars: u32,
    now: DateTime<Utc>,
    writer: Option<Id>,
) -> Result<Vec<ActivityItem>, SummarizeError> {
    let ids: Vec<Id> = sqlx::query_scalar("SELECT t.node_id FROM threads t JOIN nodes n ON n.id=t.node_id WHERE $1=ANY(n.path) AND t.side_of IS NULL AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id=ANY(n.path) AND (a.deleted_at IS NOT NULL OR a.archived_at IS NOT NULL)) ORDER BY n.updated_at DESC LIMIT $2")
        .bind(topic).bind(i64::from(count)).fetch_all(&mut *conn).await.map_err(DbError::from).map_err(internal)?;
    let mut out = Vec::new();
    let Some(writer) = writer else {
        return Err(SummarizeError::Skipped(
            "no summarizer setting writer is recorded".to_owned(),
        ));
    };
    let principal =
        orch_authz::Principal::load(conn, orch_authz::Caller::session(writer), Time::new(now))
            .await
            .map_err(internal)?;
    let private = orch_authz::nodes::restricted_below(
        conn,
        &principal,
        &ids,
        topic,
        orch_authz::Policy::default(),
    )
    .await
    .map_err(internal)?;
    for node in hierarchy::details(conn, &ids, now)
        .await
        .map_err(internal)?
    {
        if private.contains(&node.id) {
            continue;
        }
        let allowed = orch_authz::nodes::check(
            conn,
            &principal,
            node.id,
            orch_access_contract::permissions::Permission::ThreadView,
            orch_authz::Act::Use,
            orch_authz::Policy::default(),
        )
        .await
        .map_err(internal)?;
        if !allowed.is_allowed() {
            continue;
        }
        let Some(row) = threads::thread(conn, node.id).await.map_err(internal)? else {
            continue;
        };
        let text = format!("{}: {}", node.name, node.description);
        let masked = crate::raw_events::mask_live(conn, &row, row.environment_id, json!(text))
            .await
            .map_err(internal)?
            .ok_or_else(|| SummarizeError::Skipped("redaction is unavailable".to_owned()))?;
        out.push(ActivityItem {
            kind: ActivityKind::AgentMessage,
            text: masked.as_str().unwrap_or_default().to_owned(),
            at: node.auto_updated_at.map(Time::new),
        });
    }
    Ok(trim(out, count, chars))
}

async fn follow_topic_title(
    conn: &mut Conn,
    thread: Id,
    title: &str,
    now: DateTime<Utc>,
    writer: Option<Id>,
) -> Result<(), EngineError> {
    let topic: Option<Id> = sqlx::query_scalar("SELECT t.topic_id FROM threads t WHERE t.node_id=$1 AND t.side_of IS NULL AND t.node_id=(SELECT f.node_id FROM threads f JOIN nodes n ON n.id=f.node_id WHERE f.topic_id=t.topic_id AND f.side_of IS NULL AND n.deleted_at IS NULL ORDER BY n.created_at,f.node_id LIMIT 1)")
        .bind(thread).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
    if let Some(topic) = topic {
        let Some(writer) = writer else {
            return Ok(());
        };
        let Ok(principal) =
            orch_authz::Principal::load(conn, orch_authz::Caller::session(writer), Time::new(now))
                .await
        else {
            return Ok(());
        };
        if !orch_authz::nodes::restricted_below(
            conn,
            &principal,
            &[thread],
            topic,
            orch_authz::Policy::default(),
        )
        .await?
        .is_empty()
        {
            return Ok(());
        }
        if threads::rename_if_automatic(conn, topic, title, now).await? {
            hierarchy::stamp(conn, topic).await?;
        }
    }
    Ok(())
}

/// Gives an automatic title its first delivered words, leaving a harness title or any person's edit alone.
pub async fn first_input(unit: &mut Unit<'_>, thread: Id) -> Result<(), EngineError> {
    let now = unit.now();
    let mode: Option<String> = setting(&mut *unit.conn, Place::Node(thread), keys::AUTO_TITLE)
        .await
        .map_err(stored_error)?;
    if !matches!(mode.as_deref(), Some("first_input" | "harness")) {
        return Ok(());
    }
    let node = hierarchy::details(&mut *unit.conn, &[thread], now)
        .await?
        .into_iter()
        .next();
    if node
        .as_ref()
        .is_none_or(|node| node.title_source != "auto" || node.auto_updated_at.is_some())
    {
        return Ok(());
    }
    let row = threads::thread(unit.conn, thread)
        .await?
        .ok_or_else(|| EngineError::unreadable("The selected thread history is unavailable."))?;
    let payload = orch_db::queries::thread_history::first_prompt(
        unit.conn,
        thread,
        row.active_continuation_id,
    )
    .await?;
    let text = payload
        .as_ref()
        .and_then(|payload| payload["parts"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(Json::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    let first: String = text.trim().chars().take(FIRST_TITLE_CHARS).collect();
    let title = if text.trim().chars().count() > FIRST_TITLE_CHARS {
        first
            .rsplit_once(char::is_whitespace)
            .map_or(first.as_str(), |(whole, _)| whole)
    } else {
        &first
    };
    if !title.is_empty()
        && threads::rename_if_automatic(&mut *unit.conn, thread, title, now).await?
    {
        let seq = unit.stamp(thread).await?;
        threads::set_seq_state(&mut *unit.conn, thread, seq).await?;
        hierarchy::stamp(&mut *unit.conn, thread).await?;
        follow_topic_title(
            &mut *unit.conn,
            thread,
            title,
            now,
            node.as_ref().map(|node| node.owner_id),
        )
        .await?;
    }
    Ok(())
}
