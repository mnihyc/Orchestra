//! Automatic document descriptions share the configured producer, current access, pacing and usage records.

use super::{
    CLAIM_GRACE_SECS, Choice, INPUT_CHARS, INPUT_ITEMS, MIN_INTERVAL_SECS, choice, internal,
    safe_reason, seconds, stored_error,
};
use crate::engine::Engine;
use crate::error::EngineError;
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName};
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::DbError;
use orch_db::queries::{documents as rows, hierarchy, threads};
use orch_plugin_contract::refs::{NodeKind, NodeRef};
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_summarizer_contract::activity::{ActivityItem, ActivityKind, trim};
use orch_summarizer_contract::descriptor::SummaryField;
use orch_summarizer_contract::summarizer::{
    SummarizeError, SummaryLimits, SummaryRequest, next_interval,
};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

fn unavailable(reason: &str) -> SummarizeError {
    SummarizeError::Skipped(reason.to_owned())
}

fn page_activity(blocks: &[rows::BlockRow], limit: u32) -> Vec<ActivityItem> {
    let mut siblings: HashMap<Option<Id>, Vec<&rows::BlockRow>> = HashMap::new();
    // The database already orders siblings by position and identity.
    for block in blocks {
        siblings.entry(block.parent_id).or_default().push(block);
    }
    let mut stack: Vec<_> = siblings
        .remove(&None)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .collect();
    let mut seen = HashSet::new();
    let mut activity = Vec::new();
    while let Some(block) = stack.pop() {
        if !seen.insert(block.id) {
            continue;
        }
        if !block.text.trim().is_empty() {
            activity.push(ActivityItem {
                kind: ActivityKind::Document,
                text: block.text.clone(),
                at: Some(Time::new(block.updated_at)),
            });
            if activity.len() >= usize::try_from(limit).unwrap_or(usize::MAX) {
                break;
            }
        }
        stack.extend(
            siblings
                .remove(&Some(block.id))
                .unwrap_or_default()
                .into_iter()
                .rev(),
        );
    }
    activity
}

impl Engine {
    /// Runs one due automatic document description, leaving model I/O outside its transaction.
    pub async fn summarize_document_due(&self, document: Id) -> Result<bool, EngineError> {
        let now = self.inner.clock.now();
        let claimant = new_id();
        let until = now
            + chrono::Duration::seconds(
                i64::from(
                    orch_link_contract::commands::CommandKind::ProviderComplete.deadline_secs(),
                ) + CLAIM_GRACE_SECS,
            );
        let mut conn = self.db().acquire().await?;
        let claim: Option<(Id, i64)> = sqlx::query_as("UPDATE document_summary_state s SET run_id=COALESCE(run_id,$2),claimed_by=$3,claimed_until=$4,due_at=NULL FROM documents d WHERE s.document_id=$1 AND d.id=s.document_id AND d.deleted_at IS NULL AND (s.due_at<=$5 OR s.run_id IS NOT NULL) AND s.next_at<=$5 AND (s.claimed_until IS NULL OR s.claimed_until<=$5) RETURNING s.run_id,s.failures")
            .bind(document).bind(new_id()).bind(claimant).bind(until).bind(now).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
        let Some((run, failures)) = claim else {
            return Ok(false);
        };
        let doc = rows::get(&mut conn, document).await?;
        let scope = doc.as_ref().map(|doc| doc.node_id);
        let triggers = match doc.as_ref() {
            Some(doc) => setting::<Json>(&mut conn, Place::Node(doc.node_id), keys::AUTO_TRIGGERS)
                .await
                .map_err(stored_error)?,
            None => None,
        };
        let min = seconds(triggers.as_ref(), "min_interval_secs", MIN_INTERVAL_SECS);
        drop(conn);
        let attempted = self.generate_document_description(document, run).await;
        let (reason, failure, retry) = match attempted {
            Ok(_) => (None, false, false),
            Err(error) => {
                if error.is_failure() {
                    tracing::warn!(code=error.code(), %document, "a document description could not be written");
                }
                (Some(safe_reason(&error)), error.is_failure(), true)
            }
        };
        let unmatched = reason
            .as_deref()
            .is_some_and(super::completion::stable_pause);
        let interval = Duration::from_secs(u64::try_from(min).unwrap_or(u64::MAX));
        let backoff = if failure {
            next_interval(
                interval,
                u32::try_from(failures.saturating_add(1)).unwrap_or(u32::MAX),
            )
        } else {
            interval
        };
        let next =
            self.inner.clock.now() + chrono::Duration::from_std(backoff).map_err(stored_error)?;
        let mut conn = self.db().acquire().await?;
        sqlx::query("UPDATE document_summary_state SET last_at=$3,next_at=$4,failures=CASE WHEN $5 THEN failures+1 ELSE 0 END,reason=$6,run_id=NULL,claimed_by=NULL,claimed_until=NULL,due_at=CASE WHEN $8 THEN NULL WHEN $7 THEN COALESCE(due_at,$4) ELSE due_at END WHERE document_id=$1 AND claimed_by=$2")
            .bind(document).bind(claimant).bind(self.inner.clock.now()).bind(next).bind(failure).bind(reason).bind(retry).bind(unmatched)
            .execute(&mut *conn).await.map_err(DbError::from)?;
        if let Some(node) = scope
            && let Some(recovery) = crate::calls::read_workflow(
                &mut conn,
                node,
                orch_provider_contract::calls::Purpose::Summary,
                run,
            )
            .await?
            && matches!(recovery.state.as_str(), "scheduled" | "waiting" | "stopped")
        {
            sqlx::query("UPDATE document_summary_state SET next_at=COALESCE($2,next_at),due_at=$2,run_id=CASE WHEN $2::timestamptz IS NOT NULL THEN $3 ELSE NULL END,reason=$4 WHERE document_id=$1")
                .bind(document).bind(recovery.due_at).bind(run).bind(if recovery.state=="scheduled" {"calls.serviceScheduled"} else {crate::calls::administrative_pause_reason(&recovery).unwrap_or("calls.servicePaused")}).execute(&mut *conn).await.map_err(DbError::from)?;
        }
        Ok(true)
    }

    async fn generate_document_description(
        &self,
        document: Id,
        run: Id,
    ) -> Result<bool, SummarizeError> {
        let mut conn = self.db().acquire().await.map_err(internal)?;
        let epoch = orch_authz::epoch::current(&mut conn)
            .await
            .map_err(internal)?;
        let Some(before) = rows::get(&mut conn, document)
            .await
            .map_err(internal)?
            .filter(|doc| doc.deleted_at.is_none() && doc.description_source == "auto")
        else {
            return Ok(false);
        };
        let Some(chosen) = choice(&mut conn, before.node_id).await.map_err(internal)? else {
            return Ok(false);
        };
        let writer = chosen
            .writer
            .ok_or_else(|| unavailable("the summarizer setting has no current writer"))?;
        let principal = Principal::load(
            &mut conn,
            Caller::session(writer),
            Time::new(self.inner.clock.now()),
        )
        .await
        .map_err(internal)?;
        if !orch_authz::documents::check(
            &mut conn,
            &principal,
            document,
            Permission::DocView,
            Policy::default(),
        )
        .await
        .map_err(internal)?
        .is_allowed()
        {
            return Err(unavailable("the summarizer cannot read its source"));
        }
        if !orch_authz::nodes::check_named(
            &mut conn,
            &principal,
            before.node_id,
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            Policy::default(),
        )
        .await
        .map_err(internal)?
        .is_allowed()
        {
            return Err(unavailable("the summarizer cannot read its place"));
        }
        let node = hierarchy::details(&mut conn, &[before.node_id], self.inner.clock.now())
            .await
            .map_err(internal)?
            .into_iter()
            .next()
            .ok_or_else(|| unavailable("the summary's place is unavailable"))?;
        if hierarchy::with_ancestors(&mut conn, &[before.node_id])
            .await
            .map_err(internal)?
            .iter()
            .any(|node| node.deleted_at.is_some() || node.archived_at.is_some())
        {
            return Err(unavailable("the summary's place is unavailable"));
        }
        let (items, chars) = if chosen.kind == "model" {
            let config = orch_summarizer_model::descriptor::Config::read(&chosen.config)?;
            (config.input_items, config.input_chars)
        } else {
            (INPUT_ITEMS, INPUT_CHARS)
        };
        let limits = SummaryLimits {
            max_chars: setting::<u32>(
                &mut conn,
                Place::Node(before.node_id),
                keys::DOCS_DESCRIPTION_CHARS,
            )
            .await
            .map_err(internal)?
            .ok_or_else(|| {
                SummarizeError::Config(
                    "the document description length has no resolved value".to_owned(),
                )
            })?,
            max_tags: 0,
        };
        let live = rows::live_blocks(&mut conn, document)
            .await
            .map_err(internal)?;
        let mut activity = page_activity(&live, items);
        if let Some(first) = activity.first_mut() {
            first.text = format!("Document: {}\n\n{}", before.title, first.text);
        } else {
            activity.push(ActivityItem {
                kind: ActivityKind::Document,
                text: format!("Document: {}\n\nThe document has no text.", before.title),
                at: Some(Time::new(before.updated_at)),
            });
        }
        let thread = threads::thread(&mut conn, before.node_id)
            .await
            .map_err(internal)?;
        let (topic, environment) = thread.as_ref().map_or((before.node_id, None), |thread| {
            (thread.topic_id, thread.environment_id)
        });
        let material = crate::raw_events::mask_summary(&mut conn, before.node_id, topic, environment,
            json!({"activity":activity,"previous":before.description,"title":before.title,"place":node.name})).await.map_err(internal)?
            .ok_or_else(|| unavailable("redaction is unavailable"))?;
        let activity: Vec<ActivityItem> =
            serde_json::from_value(material["activity"].clone()).map_err(internal)?;
        // A document is read from its opening, in page order; the configured input budget remains the same.
        let activity = trim(
            activity
                .into_iter()
                .take(usize::try_from(items).unwrap_or(usize::MAX))
                .collect(),
            items,
            chars,
        );
        let request = SummaryRequest {
            field: SummaryField::Description,
            node: NodeRef {
                id: before.node_id,
                kind: match node.kind.as_str() {
                    "org" => NodeKind::Org,
                    "project" => NodeKind::Project,
                    "thread" => NodeKind::Thread,
                    _ => NodeKind::Topic,
                },
                name: material["place"].as_str().map(str::to_owned),
                redacted: false,
            },
            previous: material["previous"].as_str().unwrap_or_default().to_owned(),
            activity,
            tag_choices: Vec::new(),
            limits,
            config: chosen.config.clone(),
        };
        let fingerprint = serde_json::to_vec(
            &json!({"document":document,"request":request,"writer":chosen.writer}),
        )
        .map_err(internal)?;
        let digest = orch_types::Hash::of(&fingerprint);
        let call = crate::ids::derived(
            "summary.document",
            &[run.as_bytes().as_slice(), digest.as_bytes().as_slice()],
        );
        if orch_authz::epoch::current(&mut conn)
            .await
            .map_err(internal)?
            != epoch
        {
            return Err(unavailable("the summarizer's source access changed"));
        }
        drop(conn);
        let producer = self
            .document_producer(&chosen, call, document, before.doc_seq, run)
            .ok_or_else(|| unavailable("the chosen producer is not installed"))?;
        if !producer.describe().writes(SummaryField::Description) {
            return Err(SummarizeError::Unsupported(SummaryField::Description));
        }
        let made = producer.summarize(request).await;
        let result = match made {
            Ok(result) => result.conform(SummaryField::Description, &limits, &[]),
            Err(error) => {
                if matches!(&error, SummarizeError::Skipped(reason) if super::completion::stable_pause(reason))
                {
                    return Err(error);
                }
                if chosen.kind == "model"
                    && matches!(
                        self.model_output_failed(
                            &chosen,
                            before.node_id,
                            call,
                            run,
                            "document_description"
                        )
                        .await
                        .map_err(internal)?,
                        super::ModelFailure::Unmatched
                    )
                {
                    return Err(SummarizeError::Skipped(
                        super::completion::NO_RULE.to_owned(),
                    ));
                }
                return Err(error);
            }
        };
        if result.discarded {
            if chosen.kind == "model"
                && matches!(
                    self.model_output_failed(
                        &chosen,
                        before.node_id,
                        call,
                        run,
                        "document_description"
                    )
                    .await
                    .map_err(internal)?,
                    super::ModelFailure::Unmatched
                )
            {
                return Err(SummarizeError::Skipped(
                    super::completion::NO_RULE.to_owned(),
                ));
            }
            return Err(SummarizeError::Failed {
                message: "the answer contained no usable description".to_owned(),
                retryable: true,
            });
        }
        let description = result
            .result
            .text
            .ok_or_else(|| unavailable("the producer wrote no description"))?;
        if chosen.kind == "model" {
            let mut conn = self.db().acquire().await.map_err(internal)?;
            crate::calls::finish_service(
                &mut conn,
                before.node_id,
                orch_provider_contract::calls::Purpose::Summary,
                call,
            )
            .await
            .map_err(internal)?;
        }
        self.commit_document_description(&before, &chosen, epoch, &description)
            .await
            .map_err(internal)?
            .ok_or_else(|| unavailable("the summary's source changed"))
    }

    async fn commit_document_description(
        &self,
        before: &rows::DocumentRow,
        chosen: &Choice,
        epoch: i64,
        description: &str,
    ) -> Result<Option<bool>, EngineError> {
        let mut tx = self.db().begin().await?;
        orch_db::queries::transaction_guard::expect_cache_epoch(&mut tx, epoch).await?;
        if orch_authz::epoch::current(&mut tx).await? != epoch {
            return Ok(None);
        }
        let Some(current) = rows::lock(&mut tx, before.id).await? else {
            return Ok(Some(false));
        };
        if current.deleted_at.is_some() || current.description_source != "auto" {
            return Ok(Some(false));
        }
        if current.doc_seq != before.doc_seq
            || current.node_id != before.node_id
            || choice(&mut tx, current.node_id).await?.as_ref() != Some(chosen)
        {
            return Ok(None);
        }
        let changed = rows::describe_automatically(
            &mut tx,
            current.id,
            description,
            current.doc_seq,
            self.inner.clock.now(),
        )
        .await?
        .is_some();
        tx.commit().await.map_err(DbError::from)?;
        Ok(Some(changed))
    }
}
