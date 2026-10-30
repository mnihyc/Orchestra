//! Model policy and usage for the harness's own permission reviewer, never boundary approvals.

use crate::access::ProviderUse;
use crate::{engine::Engine, error::EngineError};
use orch_agent_contract::descriptor::{AgentDescriptor, ApprovalMode};
use orch_agent_contract::review::{ApprovalReviewer, REVIEW_TOOL};
use orch_db::Conn;
use orch_db::queries::threads::ThreadRow;
use orch_types::{Id, Json};

impl Engine {
    /// Native modes the thread can choose now, with the same model and confinement gates as opening it.
    pub async fn offered_approval_modes(
        &self,
        conn: &mut Conn,
        environment: Option<Id>,
        descriptor: &AgentDescriptor,
        allowed: &[ApprovalMode],
        models: &[String],
    ) -> Result<Vec<ApprovalMode>, EngineError> {
        let mut modes = Vec::new();
        for mode in &descriptor.approval_modes {
            if !allowed.contains(mode) {
                continue;
            }
            // Auto exists only where the descriptor of the build says so (it keeps the mode only for a build whose
            // check proved it) and declares the reviewer, and only for someone who may use that reviewer's model.
            if *mode == ApprovalMode::Auto
                && reviewer_allowed(*mode, Some(descriptor), models).is_err()
            {
                continue;
            }
            if *mode == ApprovalMode::Bypass {
                let Some(environment) = environment else {
                    continue;
                };
                if self
                    .bypass_environment(conn, environment, *mode)
                    .await?
                    .is_some()
                {
                    continue;
                }
            }
            if !modes.contains(mode) {
                modes.push(*mode);
            }
        }
        Ok(modes)
    }

    /// Exact offered models allowed to the current input author by both the place and provider entry.
    pub async fn review_models(
        &self,
        conn: &mut Conn,
        thread: &ThreadRow,
        provider: Id,
        person: Option<Id>,
        settings: &crate::resolved::Resolved,
    ) -> Result<Vec<String>, EngineError> {
        let mut models = Vec::new();
        for row in orch_db::queries::resources::models_of_provider(conn, provider).await? {
            if !row.enabled
                || settings.models_allowed.as_ref().is_some_and(|patterns| {
                    !crate::models::allowed_by(patterns, &row.model_name, &row.groups)
                })
            {
                continue;
            }
            if self
                .inner
                .access
                .provider_use(
                    conn,
                    &ProviderUse {
                        person: person.unwrap_or(thread.owner_id),
                        node: thread.node_id,
                        provider,
                        model: Some(&row.model_name),
                        effort: None,
                    },
                )
                .await?
                .is_allowed()
            {
                models.push(row.model_name);
            }
        }
        Ok(models)
    }

    /// Bypass needs actual confinement declared by the environment's current driver.
    pub(crate) async fn bypass_environment(
        &self,
        conn: &mut Conn,
        environment: Id,
        mode: ApprovalMode,
    ) -> Result<Option<String>, EngineError> {
        if mode != ApprovalMode::Bypass {
            return Ok(None);
        }
        let machine = orch_db::queries::environment_facts::of(conn, environment).await?;
        let isolated = machine
            .as_ref()
            .is_some_and(|machine| machine.confinement() == "isolated");
        Ok((!isolated).then(|| {
            "Bypass needs an environment whose driver declares isolation; use On request here"
                .to_owned()
        }))
    }

    /// Native reviewer facts and, only where reported apart, its model's usage.
    pub(crate) async fn record_review_item(
        &self,
        unit: &mut crate::unit::Unit<'_>,
        thread: Id,
        segment: Id,
        native_ref: &str,
        turn: Option<Id>,
        payload: &Json,
    ) -> Result<(), EngineError> {
        let Some(detail) = detail(payload) else {
            return Ok(());
        };
        let Some(model) = detail
            .get("model")
            .and_then(Json::as_str)
            .filter(|model| !model.is_empty())
        else {
            return Ok(());
        };
        let usage: Option<orch_agent_contract::events::TokenUsage> = detail
            .get("usage")
            .filter(|usage| !usage.is_null())
            .cloned()
            .map(serde_json::from_value)
            .transpose()?;
        let reviewer = ApprovalReviewer {
            model: model.to_owned(),
            reported: detail
                .get("reported")
                .and_then(Json::as_bool)
                .unwrap_or(false),
            usage_apart: usage.is_some(),
        };
        let seq = unit.stamp(thread).await?;
        sqlx::query("UPDATE segments SET approval_reviewer=$2,seq_updated=$3 WHERE id=$1")
            .bind(segment)
            .bind(serde_json::to_value(reviewer)?)
            .bind(seq)
            .execute(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
        let Some(usage) = usage else {
            return Ok(());
        };
        let turn = match turn {
            Some(turn) => orch_db::queries::threads::turn(&mut *unit.conn, turn).await?,
            None => orch_db::queries::threads::active_turn(&mut *unit.conn, thread).await?,
        };
        let Some(turn) = turn.filter(|turn| turn.segment_id == segment && turn.thread_id == thread)
        else {
            return Ok(());
        };
        use orch_db::usage::ledger;
        // A turn under a lease is attributed as its lease is. One whose harness calls its model on its own
        // sign-in (no lease) is attributed as the turn itself is: the reported review is the only meter of the
        // reviewer's use there, as the harness's own totals are of the turn's.
        let attribution = match turn.lease_id {
            Some(lease) => ledger::attribution_of_lease(&mut *unit.conn, lease).await?,
            None => {
                orch_db::queries::threads::binding::attribution_of_turn(&mut *unit.conn, turn.id)
                    .await?
            }
        };
        let Some(attribution) = attribution else {
            return Ok(());
        };
        let tokens = ledger::Tokens {
            input: usage.input_tokens,
            cached_input: usage.cached_input_tokens,
            cache_write: usage.cache_write_tokens,
            output: usage.output_tokens,
            reasoning: usage.reasoning_tokens,
            total: usage.total_tokens,
        };
        let rate = crate::caps::own_rate(&mut *unit.conn, attribution.provider_id, model).await?;
        let book = crate::rates::RateBook::load(&mut *unit.conn, thread).await?;
        let priced = book
            .price(
                &mut *unit.conn,
                rate.as_ref(),
                model,
                &tokens,
                Some(tokens.input),
                None,
            )
            .await?;
        let rate_json = priced
            .found
            .as_ref()
            .map(|found| serde_json::to_value(&found.rate))
            .transpose()?;
        let id = crate::ids::derived(
            "review.usage",
            &[segment.as_bytes().as_slice(), native_ref.as_bytes()],
        );
        let at = unit.now();
        ledger::record_review(
            &mut *unit.conn,
            &ledger::NewEvent {
                id,
                at,
                origin: ledger::origin::HARNESS,
                scope: ledger::scope::REQUEST,
                counted: true,
                attribution: &attribution,
                model,
                tier: None,
                tokens,
                context_tokens: Some(tokens.input),
                request_ref: Some(native_ref),
                rate: rate_json.as_ref(),
                rate_source: priced.found.as_ref().map(|found| found.source.word()),
                credits: priced.credits.as_deref(),
            },
        )
        .await?;
        Ok(())
    }
}

/// A non-settable native reviewer must be among the person's actual allowed models.
pub(crate) fn reviewer_allowed(
    mode: ApprovalMode,
    descriptor: Option<&AgentDescriptor>,
    allowed: &[String],
) -> Result<(), String> {
    if mode != ApprovalMode::Auto {
        return Ok(());
    }
    let Some(reviewer) =
        descriptor.and_then(|descriptor| descriptor.capabilities.approval_reviewer.as_ref())
    else {
        return Err("Auto is unavailable: this agent program declares no reviewer".to_owned());
    };
    if !reviewer.settable && !allowed.iter().any(|model| *model == reviewer.model) {
        return Err(format!(
            "Auto is unavailable: this person may not use the native reviewer model {} on this provider",
            reviewer.model
        ));
    }
    Ok(())
}

/// The current opened session's reviewer declaration/report, read under the thread's ordinary permission.
pub async fn session_reviewer(
    conn: &mut Conn,
    thread: Id,
) -> Result<Option<ApprovalReviewer>, EngineError> {
    let value: Option<Json> = sqlx::query_scalar("SELECT s.approval_reviewer FROM threads t JOIN segments s ON s.id=t.current_segment_id WHERE t.node_id=$1")
        .bind(thread).fetch_optional(conn).await.map_err(orch_db::DbError::from)?.flatten();
    Ok(value.map(serde_json::from_value).transpose()?)
}

/// Whether this payload is a native permission review, rather than a normal tool result.
pub(crate) fn detail(payload: &Json) -> Option<&Json> {
    (payload.pointer("/tool/name").and_then(Json::as_str) == Some(REVIEW_TOOL)
        && payload.pointer("/raw/name").and_then(Json::as_str) == Some(REVIEW_TOOL))
    .then(|| payload.pointer("/tool/result"))
    .flatten()
}
