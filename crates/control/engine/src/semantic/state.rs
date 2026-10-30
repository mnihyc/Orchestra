//! Where search by meaning stands for an org, as an admin reads it: what it chose, whether the database can keep vectors, the generations, progress with an estimate, the disk, and why it waits.

use super::choice::choice;
use super::indexer::new_generation_bytes;
use super::plan;
use crate::engine::Engine;
use crate::error::EngineError;
use orch_db::queries::semantic as rows;
use orch_types::Id;

/// One generation of an org's vectors, as shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    /// Its identifier.
    pub id: Id,
    /// The provider whose endpoint makes its vectors.
    pub provider_id: Id,
    /// The model.
    pub model_name: String,
    /// The length asked of the model; none for its own.
    pub asked_dimensions: Option<i32>,
    /// The length its vectors have, once known.
    pub dimensions: Option<i32>,
    /// What was put before a query.
    pub query_prefix: String,
    /// What was put before a document.
    pub document_prefix: String,
}

/// Where search by meaning stands for one root org.
#[derive(Debug, Clone, PartialEq)]
pub struct OrgState {
    /// The root org.
    pub org: Id,
    /// Whether it is on.
    pub enabled: bool,
    /// The provider named, if any.
    pub provider_id: Option<Id>,
    /// The model named, if any.
    pub model_name: Option<String>,
    /// The length asked for, if any.
    pub dimensions: Option<u32>,
    /// `off`, `not_named`, `unavailable`, `idle`, `indexing`, `paused` or `waiting`.
    pub state: String,
    /// Why it pauses or waits, as a word.
    pub reason: Option<String>,
    /// That reason in a sentence.
    pub detail: Option<String>,
    /// The generation that answers.
    pub active: Option<Generation>,
    /// The generation being built beside it.
    pub building: Option<Generation>,
    /// Estimated tokens of all its pieces.
    pub tokens: i64,
    /// Of those, the tokens still without vectors in the generation being filled.
    pub pending_tokens: i64,
    /// The endpoint's measured tokens a second.
    pub rate: Option<f64>,
    /// The time the pending tokens take at that rate.
    pub seconds_left: Option<i64>,
    /// The bytes of text it indexes.
    pub content_bytes: i64,
    /// The bytes it takes.
    pub used_bytes: i64,
    /// The most it may take; none for no limit.
    pub allowed_bytes: Option<i64>,
    /// While a new model waits for the choice: the bytes its vectors take beside the present ones.
    pub rebuild_bytes: Option<i64>,
    /// And the time it takes to make them at the measured rate.
    pub rebuild_seconds: Option<i64>,
}

fn generation_of(row: &rows::GenerationRow) -> Generation {
    Generation {
        id: row.id,
        provider_id: row.provider_id,
        model_name: row.model_name.clone(),
        asked_dimensions: row.asked_dimensions,
        dimensions: row.dimensions,
        query_prefix: row.query_prefix.clone(),
        document_prefix: row.document_prefix.clone(),
    }
}

impl Engine {
    /// Whether the database can keep vectors; the word says why not.
    pub async fn semantic_available(&self) -> Result<Result<(), String>, EngineError> {
        let mut conn = self.db().acquire().await?;
        Ok(rows::vectors_ready(&mut conn).await?)
    }

    /// How many seconds the indexer leaves between its turns, as the system's tuning says.
    pub async fn semantic_interval(&self) -> Result<i64, EngineError> {
        let mut conn = self.db().acquire().await?;
        let tuning = orch_settings::get::<super::choice::Tuning>(
            &mut conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::SEARCH_SEMANTIC_TUNING,
        )
        .await
        .map_err(|error| EngineError::unreadable(error.to_string()))?
        .ok_or_else(|| EngineError::unreadable("search.semantic.tuning has no value"))?;
        Ok(tuning.interval_secs.max(1))
    }

    /// The root orgs an admin sees on the page of search by meaning, by name: those where it is on, and those
    /// that have an index whatever they chose since.
    pub async fn semantic_orgs_shown(&self) -> Result<Vec<(Id, String)>, EngineError> {
        let mut conn = self.db().acquire().await?;
        let roots = rows::root_orgs(&mut conn).await?;
        let ids: Vec<Id> = roots.iter().map(|(id, _)| *id).collect();
        let enabled = orch_settings::resolver::get_at_nodes::<bool>(
            &mut conn,
            &ids,
            orch_settings_contract::registry::keys::SEARCH_SEMANTIC_ENABLED,
        )
        .await
        .map_err(|error| EngineError::unreadable(error.to_string()))?;
        let indexed = rows::indexed_orgs(&mut conn).await?;
        Ok(roots
            .into_iter()
            .filter(|(id, _)| {
                enabled.get(id).copied().flatten() == Some(true) || indexed.contains(id)
            })
            .collect())
    }

    /// Where search by meaning stands for the root org `org`.
    pub async fn semantic_state(&self, org: Id) -> Result<OrgState, EngineError> {
        let mut conn = self.db().acquire().await?;
        let chosen = choice(&mut conn, org).await?;
        let ready = rows::vectors_ready(&mut conn).await?;
        let generations = rows::generations(&mut conn, org).await?;
        let active = generations.iter().find(|row| row.state == "active");
        let building = generations.iter().find(|row| row.state == "building");
        let filling = building.or(active).map(|row| row.id);
        let lengths: Vec<(Id, Option<i32>)> = generations
            .iter()
            .map(|row| (row.id, row.dimensions))
            .collect();
        let sizes = rows::sizes(&mut conn, org, &lengths, filling, ready.is_ok()).await?;
        let index = rows::index(&mut conn, org).await?;
        let rate = index.as_ref().and_then(|row| row.rate);
        let provider_pause = if let Some(named) = &chosen.embeddings {
            rows::provider_pause_reason(&mut conn, named.provider_id).await?
        } else {
            None
        };
        let (state, reason, detail) = if !chosen.enabled {
            ("off".to_owned(), None, None)
        } else if chosen.embeddings.is_none() {
            ("not_named".to_owned(), None, None)
        } else if let Some(reason) = provider_pause {
            (
                "paused".to_owned(),
                Some(reason.to_owned()),
                Some(super::embed::pause_detail(reason).to_owned()),
            )
        } else if let Err(why) = &ready {
            (
                "unavailable".to_owned(),
                Some("extension".to_owned()),
                Some(format!(
                    "the database cannot keep vectors ({why}); it needs the pgvector extension, which Orch's database image carries"
                )),
            )
        } else {
            match index {
                Some(row) if row.state == "idle" && active.is_none() && building.is_none() => {
                    ("indexing".to_owned(), Some("counting".to_owned()), None)
                }
                Some(row)
                    if row.state == "paused"
                        && matches!(
                            row.reason.as_deref(),
                            Some("provider_disabled" | "provider_removed")
                        ) =>
                {
                    (
                        "indexing".to_owned(),
                        active.is_none().then(|| "counting".to_owned()),
                        None,
                    )
                }
                Some(row) => (row.state, row.reason, row.detail),
                None => ("indexing".to_owned(), Some("counting".to_owned()), None),
            }
        };
        let (preview_chunks, preview_source_bytes, preview_sources) =
            if state == "waiting" && building.is_some() {
                let old = rows::sizes(
                    &mut conn,
                    org,
                    &lengths,
                    active.map(|generation| generation.id),
                    ready.is_ok(),
                )
                .await?;
                (
                    sizes.chunks.max(old.chunks),
                    sizes.source_bytes.max(old.source_bytes),
                    sizes.sources.max(old.sources),
                )
            } else {
                (sizes.chunks, sizes.source_bytes, sizes.sources)
            };
        let (rebuild_bytes, rebuild_seconds) = match (state.as_str(), &chosen.embeddings, active) {
            ("waiting", Some(named), Some(active)) => (
                Some(new_generation_bytes(
                    named,
                    building.unwrap_or(active),
                    preview_chunks,
                    preview_source_bytes,
                    preview_sources,
                )),
                plan::seconds_left(sizes.tokens, rate),
            ),
            _ => (None, None),
        };
        let counting = reason.as_deref() == Some("counting");
        Ok(OrgState {
            org,
            enabled: chosen.enabled,
            provider_id: chosen.embeddings.as_ref().map(|named| named.provider_id),
            model_name: chosen
                .embeddings
                .as_ref()
                .map(|named| named.model_name.clone()),
            dimensions: chosen
                .embeddings
                .as_ref()
                .and_then(|named| named.dimensions),
            state,
            reason,
            detail,
            active: active.map(generation_of),
            building: building.map(generation_of),
            tokens: sizes.tokens,
            pending_tokens: sizes.pending_tokens,
            rate,
            seconds_left: if counting {
                None
            } else {
                plan::seconds_left(sizes.pending_tokens, rate)
            },
            content_bytes: sizes.content,
            used_bytes: sizes.used,
            allowed_bytes: plan::allowed(sizes.content, chosen.max_disk_share),
            rebuild_bytes,
            rebuild_seconds,
        })
    }

    /// An admin's choice to rebuild in place: the generations there are removed, so that a new one is made
    /// where they were; results by meaning cover what is done meanwhile.
    pub async fn semantic_rebuild_in_place(&self, org: Id) -> Result<(), EngineError> {
        let mut conn = self.db().acquire().await?;
        let generations = rows::generations(&mut conn, org).await?;
        drop(conn);
        for generation in generations {
            self.drop_generation(generation.id).await?;
        }
        let mut conn = self.db().acquire().await?;
        rows::reset_index(&mut conn, org, self.inner.clock.now()).await?;
        Ok(())
    }
}
