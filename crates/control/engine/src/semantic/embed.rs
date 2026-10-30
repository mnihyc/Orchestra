//! Texts made into vectors through an org's embeddings provider: the named writer's present right to use it, its state and caps, the command to its runtime, and the use counted under its own purpose.
//!
//! The request leaves from the provider's runtime on a node, as a summary's does: an endpoint only a node can
//! reach works, and so does one that runs in that runtime. Each call is checked anew against whoever named
//! the provider (the writer of `search.semantic.embeddings`), the provider's state, the model's place in its
//! list and the rules of the org, the shared caps, and its machine being online; a refusal says which, so
//! that indexing can pause with the reason. What the endpoint used is counted with the purpose `embeddings`,
//! as the org's own (its root org stands for project and topic), against the writer's caps that are shared.
//! A vector longer than the length asked is cut and made unit length again, as a model that keeps its meaning
//! in its first numbers asks: an endpoint that ignores the length gives the same vectors as one that honours it.

use super::choice::Embeddings;
use super::vectors;
use crate::access::ProviderUse;
use crate::commands::{Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::turns::{OwnPurpose, OwnUse};
use futures::{StreamExt, TryStreamExt};
use orch_db::DbError;
use orch_db::queries::resources::thread_engine as resources;
use orch_db::queries::semantic as rows;
use orch_link_contract::commands::{
    CommandError, ErrorCode, ProviderEmbed, ProviderEmbedResult, SerialKey, decode_vector,
};
use orch_model_contract::protocol::EMBEDDINGS as EMBEDDINGS_PROTOCOL;
use orch_types::{Id, Time, new_id};

#[derive(sqlx::FromRow)]
struct Pending {
    id: Id,
    node_id: Id,
    provider_id: Id,
    person_id: Option<Id>,
    model_name: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// Why texts could not be made into vectors.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    /// Something has to change first: the word names what, the sentence says it.
    #[error("{detail}")]
    Unavailable {
        /// `provider`, `provider_disabled`, `provider_removed`, `access`, `model`, `cap` or `machine`.
        reason: &'static str,
        /// What is wrong, in a sentence.
        detail: String,
    },
    /// The endpoint failed this request; a smaller or a later one may pass.
    #[error("{detail}")]
    Failed {
        /// What happened.
        detail: String,
        /// Whether it ran out of time.
        timed_out: bool,
    },
    /// The system's own fault.
    #[error(transparent)]
    Engine(#[from] EngineError),
}

impl From<DbError> for EmbedError {
    fn from(error: DbError) -> Self {
        Self::Engine(error.into())
    }
}

fn unavailable(reason: &'static str, detail: impl Into<String>) -> EmbedError {
    EmbedError::Unavailable {
        reason,
        detail: detail.into(),
    }
}

pub(super) fn pause_detail(reason: &str) -> &'static str {
    match reason {
        "provider_removed" => "Provider removed",
        "provider_disabled" => "Provider switched off",
        "model_disabled" => "Model switched off",
        _ => "Embedding request stopped",
    }
}

struct CancelOnDrop {
    engine: Engine,
    call: Option<Id>,
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(call) = self.call.take() {
            let engine = self.engine.clone();
            tokio::spawn(async move {
                let _ = engine.inner.commands.cancel(call).await;
            });
        }
    }
}

impl Engine {
    /// Stops admitted embedding commands after their provider is switched off or removed.
    pub async fn stop_embedding_requests(&self, provider: Id) -> Result<usize, EngineError> {
        let mut conn = self.db().acquire().await?;
        rows::mark_embedding_stop(&mut conn, provider).await?;
        let calls: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM summary_calls WHERE provider_id=$1 AND purpose='embeddings'
               AND cancel_reason IS NOT NULL",
        )
        .bind(provider)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
        drop(conn);
        // The existing sweep width bounds database/link work without limiting how many calls are stopped.
        let width = usize::try_from(self.inner.config.sweep_batch.max(1)).unwrap_or(1);
        futures::stream::iter(calls)
            .map(|call| async move {
                let Some(info) = self.inner.commands.status(call).await? else {
                    // Admission can commit before issue; the issuer checks the lifecycle again too.
                    return Ok::<_, EngineError>(true);
                };
                if info.outcome.is_some() {
                    return Ok(false);
                }
                Ok(self.inner.commands.cancel(call).await?)
            })
            .buffer_unordered(width)
            .try_fold(0usize, |count, stopped| async move {
                Ok(count.saturating_add(usize::from(stopped)))
            })
            .await
    }

    /// Makes vectors of `texts`, each already with its prefix, for the org `org` with the provider and model it
    /// named; one vector for each text, in their order, of the asked length or the model's own.
    pub async fn embed_texts(
        &self,
        org: Id,
        named: &Embeddings,
        texts: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, EmbedError> {
        let writer = named.writer.ok_or_else(|| {
            unavailable(
                "access",
                "nobody is recorded as naming the embeddings provider; name it again",
            )
        })?;
        let mut tx = self.db().begin().await?;
        // Serialize short admission with a lifecycle edit, never with the remote request itself.
        let present = sqlx::query("SELECT id FROM providers WHERE id=$1 FOR SHARE")
            .bind(named.provider_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DbError::from)?;
        if present.is_none() {
            return Err(unavailable("provider_removed", "Provider removed"));
        }
        if let Some(reason) = rows::provider_pause_reason(&mut tx, named.provider_id).await? {
            return Err(unavailable(reason, pause_detail(reason)));
        }
        let provider = resources::provider(&mut tx, named.provider_id)
            .await?
            .filter(|provider| provider.deleted_at.is_none())
            .ok_or_else(|| unavailable("provider_removed", "Provider removed"))?;
        if !provider
            .protocols
            .iter()
            .any(|name| name == EMBEDDINGS_PROTOCOL)
        {
            return Err(unavailable(
                "provider",
                "the embeddings provider has no endpoint of the Embeddings format",
            ));
        }
        if provider.status != "ready" {
            return Err(unavailable(
                "provider",
                format!("the embeddings provider is {}", provider.status),
            ));
        }
        let allowed = self
            .inner
            .access
            .background_provider_use(
                &mut tx,
                &ProviderUse {
                    person: writer,
                    node: org,
                    provider: provider.id,
                    model: Some(&named.model_name),
                    effort: None,
                },
            )
            .await
            .map_err(|_| unavailable("access", "the present access of whoever named the embeddings provider could not be confirmed"))?;
        if !allowed.is_allowed() {
            return Err(unavailable(
                "access",
                "whoever named the embeddings provider may not use it or its model here",
            ));
        }
        let model = resources::model(&mut tx, provider.id, &named.model_name)
            .await?
            .filter(|model| model.enabled)
            .ok_or_else(|| {
                unavailable(
                    "model",
                    "the embeddings model is not offered by its provider",
                )
            })?;
        let rules = crate::resolved::load(&mut tx, org).await?;
        if rules.models_allowed.as_ref().is_some_and(|patterns| {
            !crate::models::allowed_by(patterns, &model.model_name, &model.groups)
        }) {
            return Err(unavailable(
                "model",
                "the embeddings model is not allowed here",
            ));
        }
        if !self
            .room_for_summary(&mut tx, org, provider.id, &named.model_name)
            .await?
        {
            return Err(unavailable(
                "cap",
                "a shared usage cap has no room for embeddings",
            ));
        }
        let machine = provider.key_machine_id.ok_or_else(|| {
            unavailable(
                "machine",
                "the embeddings provider has no machine that keeps its key",
            )
        })?;
        if !orch_db::queries::resources::machine(&mut tx, machine)
            .await?
            .is_some_and(|machine| machine.online)
        {
            return Err(unavailable(
                "machine",
                "the embeddings provider's machine is offline",
            ));
        }
        let call = new_id();
        let inputs = texts.len();
        let request = ProviderEmbed {
            provider_id: provider.id,
            protocol: EMBEDDINGS_PROTOCOL.to_owned(),
            model_name: named.model_name.clone(),
            inputs: texts,
            dimensions: named.dimensions,
        };
        let admitted = self.inner.clock.now();
        let deadline = chrono::Duration::seconds(i64::from(
            orch_link_contract::commands::CommandKind::ProviderEmbed.deadline_secs(),
        ));
        let outgoing = Outgoing::new(call, machine, SerialKey::None, &request)
            .map_err(|error| EngineError::unreadable(error.to_string()))?
            .by(Time::new(admitted + deadline));
        sqlx::query("INSERT INTO summary_calls (id,node_id,topic_id,provider_id,person_id,model_name,created_at,purpose)
            VALUES ($1,$2,$2,$3,$4,$5,$6,'embeddings') ON CONFLICT(id) DO NOTHING")
            .bind(call).bind(org).bind(provider.id).bind(writer).bind(&named.model_name).bind(admitted)
            .execute(&mut *tx).await.map_err(DbError::from)?;
        // Present authority is decided when this commits; the command goes out with no transaction open.
        tx.commit().await.map_err(DbError::from)?;
        let mut cancellation = CancelOnDrop {
            engine: self.clone(),
            call: Some(call),
        };
        let pause = {
            let mut conn = self.db().acquire().await?;
            rows::embedding_pause_reason(&mut conn, call, provider.id).await?
        };
        if let Some(reason) = pause {
            cancellation.call = None;
            self.forget_embedding_call(call).await?;
            return Err(unavailable(reason, pause_detail(reason)));
        }
        self.inner
            .commands
            .issue(outgoing)
            .await
            .map_err(|error| EngineError::unreadable(error.to_string()))?;
        let stopped = {
            let mut conn = self.db().acquire().await?;
            rows::embedding_pause_reason(&mut conn, call, provider.id)
                .await?
                .is_some()
        };
        if stopped {
            self.inner
                .commands
                .cancel(call)
                .await
                .map_err(EngineError::from)?;
        }
        let outcome = self
            .inner
            .commands
            .wait(call)
            .await
            .map_err(|error| EngineError::unreadable(error.to_string()))?;
        cancellation.call = None;
        let at = self.inner.clock.now();
        let pause = {
            let mut conn = self.db().acquire().await?;
            rows::embedding_pause_reason(&mut conn, call, provider.id).await?
        };
        match outcome {
            Outcome::Succeeded(value) => {
                let answer: ProviderEmbedResult = serde_json::from_value(value)
                    .map_err(|error| EngineError::unreadable(error.to_string()))?;
                self.count_embeddings(
                    org,
                    provider.id,
                    Some(writer),
                    &named.model_name,
                    call,
                    at,
                    &answer,
                )
                .await?;
                if let Some(reason) = pause {
                    return Err(unavailable(reason, pause_detail(reason)));
                }
                let made: Option<Vec<Vec<f32>>> = answer
                    .vectors
                    .iter()
                    .map(|text| decode_vector(text))
                    .collect();
                let made =
                    made.filter(|made| made.len() == inputs)
                        .ok_or_else(|| EmbedError::Failed {
                            detail: "the endpoint did not give one vector for each text".to_owned(),
                            timed_out: false,
                        })?;
                let length = made.first().map_or(0, Vec::len);
                if length == 0
                    || made.iter().any(|vector| {
                        vector.len() != length
                            || vector.iter().any(|number| !number.is_finite())
                            || !vector.iter().any(|number| *number != 0.0)
                    })
                {
                    return Err(EmbedError::Failed {
                        detail: "the endpoint gave an empty, zero, non-finite or differently sized vector"
                            .to_owned(),
                        timed_out: false,
                    });
                }
                Ok(made
                    .into_iter()
                    .map(|vector| match named.dimensions {
                        Some(length) => vectors::shortened(&vector, length as usize),
                        None => vectors::unit(&vector),
                    })
                    .collect())
            }
            Outcome::Failed(error) => {
                self.count_failed_embeddings(
                    org,
                    provider.id,
                    Some(writer),
                    &named.model_name,
                    call,
                    at,
                    &error,
                )
                .await?;
                if let Some(reason) = pause {
                    return Err(unavailable(reason, pause_detail(reason)));
                }
                Err(match error.code.as_str() {
                    code if code == ErrorCode::LoginRequired.as_str() => unavailable(
                        "provider",
                        "the embeddings endpoint refused the provider's credential",
                    ),
                    code if code == ErrorCode::OfflinePeer.as_str() => {
                        unavailable("machine", "the embeddings provider's machine went away")
                    }
                    code if code == ErrorCode::Unsupported.as_str() => {
                        unavailable("provider", error.detail.clone())
                    }
                    code => EmbedError::Failed {
                        detail: error.detail.clone(),
                        timed_out: code == ErrorCode::Timeout.as_str(),
                    },
                })
            }
            Outcome::Expired | Outcome::Cancelled => {
                self.forget_embedding_call(call).await?;
                if let Some(reason) = pause {
                    return Err(unavailable(reason, pause_detail(reason)));
                }
                Err(unavailable(
                    "machine",
                    "the embeddings provider's machine did not take the request in time",
                ))
            }
        }
    }

    /// Counts what an answered request used, under the purpose `embeddings`.
    #[allow(
        clippy::too_many_arguments,
        reason = "the attribution of one ledger row, written in one place"
    )]
    async fn count_embeddings(
        &self,
        org: Id,
        provider: Id,
        writer: Option<Id>,
        model: &str,
        call: Id,
        at: chrono::DateTime<chrono::Utc>,
        answer: &ProviderEmbedResult,
    ) -> Result<(), EngineError> {
        let tokens = answer.usage;
        let mut tx = self.db().begin().await?;
        self.count_own_use(
            &mut tx,
            &OwnUse {
                purpose: OwnPurpose::Embeddings,
                timing: Some(&answer.timing),
                id: call,
                at: answer.timing.end.map_or(at, |end| end.get()),
                topic: org,
                thread: None,
                provider,
                entry: None,
                person: writer,
                model,
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
        sqlx::query("DELETE FROM summary_calls WHERE id = $1 AND purpose = 'embeddings'")
            .bind(call)
            .execute(&mut *tx)
            .await
            .map_err(DbError::from)?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }

    /// Counts what a failed request used, where the model client says it reached the upstream.
    #[allow(
        clippy::too_many_arguments,
        reason = "the attribution of one ledger row, written in one place"
    )]
    async fn count_failed_embeddings(
        &self,
        org: Id,
        provider: Id,
        writer: Option<Id>,
        model: &str,
        call: Id,
        at: chrono::DateTime<chrono::Utc>,
        error: &CommandError,
    ) -> Result<(), EngineError> {
        let Some(raw) = error
            .details
            .as_ref()
            .and_then(|details| details.get("model_request"))
        else {
            return self.forget_embedding_call(call).await;
        };
        let request: orch_agent_contract::requests::UsageRequest =
            serde_json::from_value(raw.clone())
                .map_err(|error| EngineError::unreadable(error.to_string()))?;
        request.check().map_err(EngineError::unreadable)?;
        self.count_embeddings(
            org,
            provider,
            writer,
            model,
            call,
            at,
            &ProviderEmbedResult {
                vectors: Vec::new(),
                usage: request.tokens.clone().usage(),
                request_ref: None,
                timing: request.measured(),
            },
        )
        .await
    }

    async fn forget_embedding_call(&self, call: Id) -> Result<(), EngineError> {
        sqlx::query("DELETE FROM summary_calls WHERE id = $1 AND purpose = 'embeddings'")
            .bind(call)
            .execute(self.db().pool())
            .await
            .map_err(DbError::from)?;
        Ok(())
    }

    /// Counts retained embedding outcomes after a lost process, including when semantic search was disabled meanwhile.
    pub(crate) async fn recover_embedding_usage(&self, limit: i64) -> Result<(), EngineError> {
        let calls: Vec<Pending> = sqlx::query_as(
            "SELECT id,node_id,provider_id,person_id,model_name,created_at
            FROM summary_calls WHERE purpose = 'embeddings' ORDER BY created_at,id LIMIT $1",
        )
        .bind(limit)
        .fetch_all(self.db().pool())
        .await
        .map_err(DbError::from)?;
        for call in calls {
            let status = self.inner.commands.status(call.id).await?;
            let absent = status.is_none();
            match status.and_then(|status| status.outcome) {
                Some(Outcome::Succeeded(value)) => {
                    let answer: ProviderEmbedResult = serde_json::from_value(value)?;
                    self.count_embeddings(
                        call.node_id,
                        call.provider_id,
                        call.person_id,
                        &call.model_name,
                        call.id,
                        call.created_at,
                        &answer,
                    )
                    .await?;
                }
                Some(Outcome::Failed(error)) => {
                    self.count_failed_embeddings(
                        call.node_id,
                        call.provider_id,
                        call.person_id,
                        &call.model_name,
                        call.id,
                        call.created_at,
                        &error,
                    )
                    .await?;
                }
                Some(Outcome::Expired | Outcome::Cancelled) => {
                    self.forget_embedding_call(call.id).await?
                }
                None => {
                    let deadline = chrono::Duration::seconds(i64::from(
                        orch_link_contract::commands::CommandKind::ProviderEmbed.deadline_secs(),
                    ));
                    if absent && self.inner.clock.now() >= call.created_at + deadline {
                        // Admission committed before the process could issue it. Its captured start deadline
                        // prevents a late delivery from spending after this unused attribution is removed.
                        self.forget_embedding_call(call.id).await?;
                    }
                }
            }
        }
        Ok(())
    }
}
