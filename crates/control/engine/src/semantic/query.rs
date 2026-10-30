//! Results by meaning for a query: the query made into a vector with an org's named embeddings, and the pieces of the generation that answers nearest to it, for the caller to check against what the reader may read and to join with results by words.

use super::choice::choice;
use super::embed::EmbedError;
use crate::engine::Engine;
use crate::error::EngineError;
use orch_db::DbError;
use orch_db::queries::semantic as rows;
use orch_types::Id;

/// A piece found near a query.
#[derive(Debug, Clone, PartialEq)]
pub struct Near {
    /// `document`, `conversation` or `chat`.
    pub kind: String,
    /// The document, the thread or the chat.
    pub source_id: Id,
    /// The block, item or message the piece begins with.
    pub first_part: Option<Id>,
    /// Its cosine distance to the query: 0 is the same direction.
    pub distance: f64,
}

/// What search by meaning gives a query in one org.
#[derive(Debug, Clone, PartialEq)]
pub enum Meaning {
    /// Semantic search is off there: results by words alone.
    Off,
    /// It is on and cannot answer now; a stable reason code says why.
    Unavailable(String),
    /// The nearest pieces, best first, with how results are to be joined.
    Found {
        /// The generation whose embedding space was searched.
        generation_id: Id,
        /// The pieces.
        near: Vec<Near>,
        /// The constant of reciprocal-rank fusion.
        fusion_k: u32,
        /// How many results by words are joined with them.
        candidates: i64,
        /// The configured provider's intentional index pause, while stored vectors still answer.
        paused: Option<String>,
    },
}

impl Engine {
    /// The live root orgs that enabled meaning or have an answering generation.
    pub async fn semantic_orgs(&self) -> Result<Vec<Id>, EngineError> {
        let mut conn = self.db().acquire().await?;
        Ok(rows::answering_orgs(&mut conn).await?)
    }

    /// The pieces of the root org `org` nearest to `query`.
    pub async fn meaning(&self, org: Id, query: &str) -> Result<Meaning, EngineError> {
        self.meaning_page(org, query, 0).await
    }

    /// Continues the same nearest-neighbour source after the number of candidates already consumed.
    pub async fn meaning_page(
        &self,
        org: Id,
        query: &str,
        offset: i64,
    ) -> Result<Meaning, EngineError> {
        self.meaning_page_in_generation(org, query, offset, None)
            .await
    }

    /// Continues a pinned generation, refusing a replacement before spending on a different embedding space.
    pub async fn meaning_page_in_generation(
        &self,
        org: Id,
        query: &str,
        offset: i64,
        expected: Option<Id>,
    ) -> Result<Meaning, EngineError> {
        self.meaning_filtered(org, query, offset, expected, None)
            .await
    }

    /// Searches the same embedding space with source filters applied inside the iterative nearest scan.
    pub async fn meaning_filtered(
        &self,
        org: Id,
        query: &str,
        offset: i64,
        expected: Option<Id>,
        mut filter: Option<orch_types::Json>,
    ) -> Result<Meaning, EngineError> {
        let mut conn = self.db().begin().await?;
        let mut metadata_cancellation =
            orch_db::queries::search::cancel_on_drop(self.db(), &mut conn).await?;
        let chosen = choice(&mut conn, org).await?;
        let budget = std::time::Duration::from_secs(
            u64::try_from(chosen.tuning.query_wait_secs.max(1)).unwrap_or(1),
        );
        // Metadata and cache reads can wait on index DDL too; they share the provider's wait budget.
        let work = async {
            if !chosen.enabled {
                return Ok(Meaning::Off);
            }
            let Some(named) = chosen.embeddings.clone() else {
                return Ok(Meaning::Unavailable("not_setup".into()));
            };
            let index_pause = rows::provider_pause_reason(&mut conn, named.provider_id).await?;
            if let Err(reason) = rows::vectors_ready(&mut conn).await? {
                return Ok(Meaning::Unavailable(
                    if reason == "not_permitted" {
                        "pgvector_not_permitted"
                    } else {
                        "pgvector_missing"
                    }
                    .into(),
                ));
            }
            let Some(active) = rows::generations(&mut conn, org)
                .await?
                .into_iter()
                .find(|generation| generation.state == "active")
            else {
                return Ok(Meaning::Unavailable(
                    index_pause.unwrap_or("not_ready").into(),
                ));
            };
            if expected.is_some_and(|id| id != active.id) {
                return Ok(Meaning::Unavailable("model_changed".into()));
            }
            let Some(dimensions) = active.dimensions else {
                return Ok(Meaning::Unavailable(
                    index_pause.unwrap_or("not_ready").into(),
                ));
            };
            let has_vectors: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM search_vectors WHERE generation_id=$1)",
            )
            .bind(active.id)
            .fetch_one(&mut *conn)
            .await
            .map_err(DbError::from)?;
            if !has_vectors {
                return Ok(Meaning::Unavailable(
                    index_pause.unwrap_or("not_ready").into(),
                ));
            }
            // The query is made with what made the vectors it is compared with; until a change is built, that is
            // the generation that answers, whatever is named now.
            let asked = super::choice::Embeddings {
                provider_id: active.provider_id,
                model_name: active.model_name.clone(),
                dimensions: active
                    .asked_dimensions
                    .and_then(|length| u32::try_from(length).ok()),
                query_prefix: active.query_prefix.clone(),
                document_prefix: active.document_prefix.clone(),
                writer: if super::plan::change(
                    &super::indexer::made_of(&active),
                    &super::indexer::wanted_of(&named, chosen.tuning.chunk_tokens),
                ) == super::plan::Change::Nothing
                {
                    named.writer
                } else {
                    // A replacement's writer need not have use of the old model. Its original writer still
                    // answers for that generation's queries, under their present access and the shared caps.
                    active.writer_id
                },
            };
            // A search waits for its query's vector only so long; the request goes on in its own task meanwhile,
            // Cancellation drops the embedding future and sends the provider command's cancellation.
            let wait = std::time::Duration::from_secs(
                u64::try_from(chosen.tuning.query_wait_secs.max(1)).unwrap_or(1),
            );
            let (cache_secs, cache_entries) = {
                let secs = orch_settings::get::<i64>(
                    &mut conn,
                    orch_settings::Place::Node(org),
                    orch_settings_contract::registry::keys::SEARCH_VECTOR_CACHE_SECS,
                )
                .await
                .map_err(EngineError::unreadable)?
                .unwrap_or(0);
                let entries = orch_settings::get::<i64>(
                    &mut conn,
                    orch_settings::Place::Node(org),
                    orch_settings_contract::registry::keys::SEARCH_VECTOR_CACHE_ENTRIES,
                )
                .await
                .map_err(EngineError::unreadable)?
                .unwrap_or(0);
                (secs, entries)
            };
            let key = orch_types::Hash::of(query.as_bytes()).to_string();
            let now = self.inner.clock.now();
            let cached: Option<Vec<f32>> = if cache_secs > 0 && cache_entries > 0 {
                sqlx::query_scalar("SELECT embedding FROM search_query_vectors WHERE org_id=$1 AND generation_id=$2 AND query_hash=$3 AND expires_at>$4")
                .bind(org).bind(active.id).bind(&key).bind(now).fetch_optional(&mut *conn).await.map_err(DbError::from)?
            } else {
                None
            };
            // An older in-flight query can fill the cache after an MRL shortening committed.
            let cached = cached.map(|vector| {
                let length = usize::try_from(dimensions).unwrap_or(0);
                if vector.len() > length {
                    super::vectors::shortened(&vector, length)
                } else {
                    vector
                }
            });
            let active_pause = if active.provider_id == named.provider_id {
                index_pause
            } else {
                rows::provider_pause_reason(&mut conn, active.provider_id).await?
            };
            if cached.is_none()
                && let Some(reason) = active_pause
            {
                return Ok(Meaning::Unavailable(reason.to_owned()));
            }
            // A stopped endpoint is not used; a live endpoint still needs its stored model-space proof.
            if active_pause.is_none()
                && rows::endpoint_fingerprint(&mut conn, active.provider_id)
                    .await?
                    .as_deref()
                    != Some(active.provider_fingerprint.as_slice())
            {
                return Ok(Meaning::Unavailable("model_changed".to_owned()));
            }
            metadata_cancellation.disarm();
            conn.commit().await.map_err(DbError::from)?;
            let texts = vec![asked.query(query)];
            let started = std::time::Instant::now();
            let vector = if let Some(vector) = cached {
                vector
            } else {
                let vector =
                    match tokio::time::timeout(wait, self.embed_texts(org, &asked, texts)).await {
                        Err(_) => return Ok(Meaning::Unavailable("query_timeout".into())),
                        Ok(Ok(mut made)) => made.pop().unwrap_or_default(),
                        Ok(Err(EmbedError::Engine(error))) => return Err(error),
                        Ok(Err(EmbedError::Unavailable { reason, .. })) => {
                            return Ok(Meaning::Unavailable(reason.into()));
                        }
                        Ok(Err(EmbedError::Failed { timed_out, .. })) => {
                            return Ok(Meaning::Unavailable(
                                if timed_out {
                                    "query_timeout"
                                } else {
                                    "provider"
                                }
                                .into(),
                            ));
                        }
                    };
                if cache_secs > 0 && cache_entries > 0 {
                    let mut cache = self.db().begin().await?;
                    let mut cancellation =
                        orch_db::queries::search::cancel_on_drop(self.db(), &mut cache).await?;
                    sqlx::query("INSERT INTO search_query_vectors (org_id,generation_id,query_hash,embedding,expires_at) VALUES ($1,$2,$3,$4,$5) ON CONFLICT(org_id,generation_id,query_hash) DO UPDATE SET embedding=EXCLUDED.embedding, expires_at=EXCLUDED.expires_at")
                    .bind(org).bind(active.id).bind(&key).bind(&vector).bind(now + chrono::Duration::seconds(cache_secs)).execute(&mut *cache).await.map_err(DbError::from)?;
                    sqlx::query("DELETE FROM search_query_vectors WHERE org_id=$1 AND (expires_at<=$2 OR (generation_id,query_hash) IN
                  (SELECT generation_id,query_hash FROM search_query_vectors WHERE org_id=$1 ORDER BY expires_at DESC,generation_id,query_hash OFFSET $3))")
                  .bind(org).bind(now).bind(cache_entries).execute(&mut *cache).await.map_err(DbError::from)?;
                    cancellation.disarm();
                    cache.commit().await.map_err(DbError::from)?;
                }
                vector
            };
            if vector.len() != usize::try_from(dimensions).unwrap_or(0) {
                return Ok(Meaning::Unavailable("model".to_owned()));
            }
            let remaining = wait.saturating_sub(started.elapsed());
            if remaining.as_millis() == 0 {
                return Ok(Meaning::Unavailable("query_timeout".to_owned()));
            }
            if let Some(filter) = &mut filter {
                filter["exact_filter_candidates"] =
                    serde_json::json!(chosen.tuning.exact_filter_candidates);
                filter["ef_search"] = serde_json::json!(chosen.tuning.ef_search);
            }
            let lookup = async {
                let mut tx = self.db().begin().await?;
                orch_db::queries::search::snapshot(&mut tx).await?;
                let mut cancellation =
                    orch_db::queries::search::cancel_on_drop(self.db(), &mut tx).await?;
                // A model change may rebuild an index. PostgreSQL must release a timed-out lookup itself,
                // so cancelled optional searches cannot fill the pool with connections waiting on its DDL.
                let timeout = format!("{}ms", remaining.as_millis());
                sqlx::query("SELECT set_config('lock_timeout', $1, true), set_config('statement_timeout', $1, true)")
                .bind(timeout).execute(&mut *tx).await.map_err(DbError::from)?;
                sqlx::query("SELECT set_config('hnsw.iterative_scan',$3,true),set_config('hnsw.max_scan_tuples',$1,true),set_config('hnsw.scan_mem_multiplier',$2,true)")
                .bind(chosen.tuning.max_scan_tuples.to_string()).bind(chosen.tuning.scan_mem_multiplier.to_string())
                .bind(chosen.tuning.iterative_scan.as_str()).execute(&mut *tx).await.map_err(DbError::from)?;
                let near = rows::nearest_filtered(
                    &mut tx,
                    active.id,
                    dimensions,
                    &vector,
                    chosen.tuning.candidates.saturating_add(offset),
                    chosen.tuning.oversample,
                    filter.as_ref(),
                )
                .await?;
                cancellation.disarm();
                tx.commit().await.map_err(DbError::from)?;
                Ok::<_, EngineError>(near)
            };
            let near = match tokio::time::timeout(remaining, lookup).await {
                Ok(Ok(near)) => near,
                Ok(Err(error)) => {
                    tracing::warn!(%org, %error, "the optional semantic lookup could not answer");
                    return Ok(Meaning::Unavailable("index".to_owned()));
                }
                Err(_) => return Ok(Meaning::Unavailable("query_timeout".to_owned())),
            };
            Ok(Meaning::Found {
                generation_id: active.id,
                near: near
                    .into_iter()
                    .skip(usize::try_from(offset).unwrap_or(usize::MAX))
                    .map(|row| Near {
                        kind: row.kind,
                        source_id: row.source_id,
                        first_part: row.first_part,
                        distance: row.distance,
                    })
                    .collect(),
                fusion_k: chosen.tuning.fusion_k,
                candidates: chosen.tuning.candidates,
                paused: index_pause.map(str::to_owned),
            })
        };
        match tokio::time::timeout(budget, Box::pin(work)).await {
            Ok(answer) => answer,
            Err(_) => Ok(Meaning::Unavailable("query_timeout".into())),
        }
    }
}
