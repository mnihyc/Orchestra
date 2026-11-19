//! The indexer: an org's text cut into pieces and made into vectors in the background, newest first, pausing with its reason before the disk share is passed, and a new generation built beside the one that answers.
//!
//! The supervisor asks it every `interval_secs` of `search.semantic.tuning`. For each root org with search by
//! meaning on, one process claims the org's row of `search_indexes` and works until it has spent about that
//! interval: it forgets sources that are gone, cuts the sources that changed (newest first), and makes the
//! missing vectors of the generation it fills, source by source (by day of change, newest first; on one day, the
//! one with the most text first), in requests of about `batch_tokens`. Before each request it checks the disk
//! share; past it, it pauses and says so, and looks again at its next turn. A request that runs out of time is
//! tried again at half the size, down to one piece. What it does not get to waits for the next turn: nothing
//! waits for it, and its state, progress and estimate are on the admin page.
//!
//! Generations follow [`super::plan`]: the first is the one that answers from its first vector on. A change of
//! the embeddings named is checked against the one that answers: the same model elsewhere moves it, a shorter
//! length cuts it, and anything else builds a new generation beside it, which answers once every piece has its
//! vector, when the old one is removed. A new generation that the disk share cannot hold beside the old one
//! waits for an admin's choice, with both sizes and the time it takes.

use super::choice::{Choice, Embeddings, choice};
use super::chunks::{self, CHUNKING, Part};
use super::embed::EmbedError;
use super::plan::{self, Change, Made};
use super::vectors;
use crate::engine::Engine;
use crate::error::EngineError;
use orch_db::queries::semantic as rows;
use orch_db::{Conn, DbError};
use orch_settings_contract::registry::keys;
use orch_types::{Id, new_id};
use serde_json::json;
use std::time::Instant;

/// The texts whose vectors show whether two endpoints serve the same model: a few short sentences of
/// different scripts and kinds, made with the document prefix and no title.
pub const PROBE: [&str; 6] = [
    "How do I roll back a release that already reached production?",
    "如何回滚已经上线的一次发布？",
    "The session cookie is dropped when the login page redirects.",
    "数据库迁移必须保持向前兼容。",
    "Quarterly revenue grew by four percent while costs stayed flat.",
    "fn main() { println!(\"hello, world\"); }",
];

/// What the indexer says of an org when it gives its row back.
#[derive(Debug, Clone, PartialEq)]
struct Standing {
    state: &'static str,
    reason: Option<&'static str>,
    detail: Option<String>,
}

impl Standing {
    fn idle() -> Self {
        Self {
            state: "idle",
            reason: None,
            detail: None,
        }
    }

    fn because(state: &'static str, reason: &'static str, detail: impl Into<String>) -> Self {
        Self {
            state,
            reason: Some(reason),
            detail: Some(detail.into()),
        }
    }
}

/// Where indexing stands after the endpoint could not make vectors: paused with the reason, or the system's fault.
fn standing_of(error: EmbedError) -> Result<Standing, EngineError> {
    match error {
        EmbedError::Unavailable { reason, detail } => {
            Ok(Standing::because("paused", reason, detail))
        }
        EmbedError::Failed { detail, .. } => Ok(Standing::because("paused", "error", detail)),
        EmbedError::Engine(error) => Err(error),
    }
}

/// The configuration that made a generation's vectors.
pub(super) fn made_of(generation: &rows::GenerationRow) -> Made {
    Made {
        provider_id: generation.provider_id,
        model_name: generation.model_name.clone(),
        asked_dimensions: generation
            .asked_dimensions
            .and_then(|length| u32::try_from(length).ok()),
        query_prefix: generation.query_prefix.clone(),
        document_prefix: generation.document_prefix.clone(),
        chunking: generation.chunking.clone(),
    }
}

/// The complete vector configuration requested by the current settings.
pub(super) fn wanted_of(named: &Embeddings, chunk_tokens: i64) -> Made {
    Made {
        provider_id: named.provider_id,
        model_name: named.model_name.clone(),
        asked_dimensions: named.dimensions,
        query_prefix: named.query_prefix.clone(),
        document_prefix: named.document_prefix.clone(),
        chunking: format!("{CHUNKING}:{chunk_tokens}"),
    }
}

/// An endpoint edit needs probes even when the provider and model keep their names.
fn generation_change(
    generation: &rows::GenerationRow,
    wanted: &Made,
    fingerprint: &[u8],
) -> Change {
    let change = plan::change(&made_of(generation), wanted);
    if change == Change::Nothing && generation.provider_fingerprint.as_slice() != fingerprint {
        Change::Elsewhere
    } else {
        change
    }
}

/// The bytes a new generation takes for its pieces and whole items, using the new requested length or the
/// present length where none is asked, plus their source bookkeeping.
pub(crate) fn new_generation_bytes(
    named: &Embeddings,
    active: &rows::GenerationRow,
    chunks: i64,
    source_bytes: i64,
    sources: i64,
) -> i64 {
    let length = named
        .dimensions
        .map(i64::from)
        .or(active.dimensions.map(i64::from))
        .unwrap_or_default();
    rows::bytes_per_vector(length)
        .saturating_add(rows::CHUNK_ROW_BYTES)
        .saturating_mul(chunks)
        .saturating_add(rows::bytes_per_vector(length).saturating_mul(sources))
        .saturating_add(source_bytes)
}

/// The probe texts' vectors as stored with a generation.
fn probe_json(made: &[Vec<f32>]) -> serde_json::Value {
    json!(
        made.iter()
            .map(|vector| orch_link_contract::commands::encode_vector(vector))
            .collect::<Vec<_>>()
    )
}

fn probe_of(generation: &rows::GenerationRow) -> Vec<Vec<f32>> {
    generation
        .probe
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|text| {
            text.as_str()
                .and_then(orch_link_contract::commands::decode_vector)
        })
        .collect()
}

/// Whether two sets of probe vectors are of one model: each pair at least as alike as `least` thousandths.
fn alike(stored: &[Vec<f32>], fresh: &[Vec<f32>], least_milli: i64) -> bool {
    let least = least_milli as f64 / 1_000.0;
    stored.len() == fresh.len()
        && !stored.is_empty()
        && stored
            .iter()
            .zip(fresh)
            .all(|(one, other)| vectors::cosine(one, other) >= least)
}

/// The parts of a source, in reading order (a document's blocks from the top, depth first), with the bytes
/// their rows take where they are stored.
async fn parts(conn: &mut Conn, kind: &str, source: Id) -> Result<(Vec<Part>, i64), DbError> {
    let found = match kind {
        "document" => rows::document_parts(conn, source).await?,
        "conversation" => rows::conversation_parts(conn, source).await?,
        _ => rows::chat_parts(conn, source).await?,
    };
    let stored = found
        .iter()
        .fold(0_i64, |sum, row| sum.saturating_add(row.stored));
    if kind != "document" {
        return Ok((
            found
                .into_iter()
                .map(|row| Part {
                    id: row.id,
                    text: row.text,
                    heading: row.heading,
                })
                .collect(),
            stored,
        ));
    }
    // Siblings come in their order; walk from the top, each block before what lies in it.
    let mut children: std::collections::HashMap<Option<Id>, Vec<rows::PartRow>> =
        std::collections::HashMap::new();
    for row in found {
        children.entry(row.parent).or_default().push(row);
    }
    let mut ordered = Vec::new();
    let mut stack: Vec<rows::PartRow> = children.remove(&None).unwrap_or_default();
    stack.reverse();
    while let Some(row) = stack.pop() {
        if let Some(mut inside) = children.remove(&Some(row.id)) {
            inside.reverse();
            stack.extend(inside);
        }
        ordered.push(Part {
            id: row.id,
            text: row.text,
            heading: row.heading,
        });
    }
    Ok((ordered, stored))
}

/// A piece ready to send: its identifier, its text's hash, and its text as sent.
struct Ready {
    chunk: Id,
    hash: Vec<u8>,
    text: String,
    tokens: i64,
}

impl Engine {
    /// Looks once for indexing work in every root org with search by meaning on; the supervisor asks again after
    /// the interval the system's `search.semantic.tuning` says.
    pub async fn keep_indexing(&self) -> Result<(), EngineError> {
        let unreadable = |error: orch_settings::Error| EngineError::unreadable(error.to_string());
        let orgs: Vec<Id> = {
            let mut conn = self.db().acquire().await?;
            if let Some(tuning) = orch_settings::get::<super::choice::Tuning>(
                &mut conn,
                orch_settings::Place::System,
                keys::SEARCH_SEMANTIC_TUNING,
            )
            .await
            .map_err(unreadable)?
            {
                drop(conn);
                self.recover_embedding_usage(tuning.source_batch).await?;
                conn = self.db().acquire().await?;
                self.inner.index_every.store(
                    u64::try_from(tuning.interval_secs.max(1)).unwrap_or(1),
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
            let roots: Vec<Id> = rows::root_orgs(&mut conn)
                .await?
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            // One read of the switch for every root org; only those where it is on are looked at further.
            let enabled = orch_settings::resolver::get_at_nodes::<bool>(
                &mut conn,
                &roots,
                keys::SEARCH_SEMANTIC_ENABLED,
            )
            .await
            .map_err(unreadable)?;
            roots
                .into_iter()
                .filter(|org| enabled.get(org).copied().flatten() == Some(true))
                .collect()
        };
        for org in orgs {
            let chosen = {
                let mut conn = self.db().acquire().await?;
                choice(&mut conn, org).await?
            };
            if !chosen.enabled || chosen.embeddings.is_none() {
                continue;
            }
            if let Err(error) = self.index_org(&chosen).await {
                tracing::warn!(%error, %org, "indexing for search by meaning failed this turn; the next one tries again");
            }
        }
        Ok(())
    }

    /// One turn of indexing for one org, under its claim.
    async fn index_org(&self, chosen: &Choice) -> Result<(), EngineError> {
        let org = chosen.org;
        let claimant = new_id();
        let now = self.inner.clock.now();
        let budget = std::time::Duration::from_secs(
            u64::try_from(chosen.tuning.interval_secs.max(1)).unwrap_or(1),
        );
        // The claim outlasts the turn by one request's deadline, so that no other process starts the same work.
        let until = now
            + chrono::Duration::from_std(budget).unwrap_or_default()
            + chrono::Duration::seconds(i64::from(
                orch_link_contract::commands::CommandKind::ProviderEmbed.deadline_secs(),
            ));
        {
            let mut conn = self.db().acquire().await?;
            if !rows::claim(&mut conn, org, claimant, now, until).await? {
                return Ok(());
            }
        }
        let started = Instant::now();
        let mut rate: Option<f64> = None;
        let outcome = self.index_turn(chosen, started, budget, &mut rate).await;
        let standing = match outcome {
            Ok(standing) => standing,
            Err(error) => {
                tracing::warn!(%error, %org, "an indexing turn stopped");
                Standing::because(
                    "paused",
                    "error",
                    "indexing stopped on an error of the system; it tries again",
                )
            }
        };
        let mut conn = self.db().acquire().await?;
        rows::release(
            &mut conn,
            org,
            claimant,
            standing.state,
            standing.reason,
            standing.detail.as_deref(),
            rate,
            self.inner.clock.now(),
        )
        .await?;
        Ok(())
    }

    /// The work of one turn; says where the org's indexing stands after it.
    async fn index_turn(
        &self,
        chosen: &Choice,
        started: Instant,
        budget: std::time::Duration,
        rate: &mut Option<f64>,
    ) -> Result<Standing, EngineError> {
        let org = chosen.org;
        let Some(named) = chosen.embeddings.as_ref() else {
            return Ok(Standing::idle());
        };
        let mut conn = self.db().acquire().await?;
        if let Some(reason) = rows::provider_pause_reason(&mut conn, named.provider_id).await? {
            return Ok(Standing::because(
                "paused",
                reason,
                super::embed::pause_detail(reason),
            ));
        }
        if let Err(why) = rows::vectors_ready(&mut conn).await? {
            return Ok(Standing::because(
                "paused",
                "extension",
                format!(
                    "the database cannot keep vectors ({why}): it needs the pgvector extension, which Orch's database image carries"
                ),
            ));
        }
        let generations = rows::generations(&mut conn, org).await?;
        let provider_fingerprint = rows::endpoint_fingerprint(&mut conn, named.provider_id)
            .await?
            .unwrap_or_default();
        drop(conn);
        let active = generations
            .iter()
            .find(|generation| generation.state == "active")
            .cloned();
        let building = generations
            .iter()
            .find(|generation| generation.state == "building")
            .cloned();
        let wanted = wanted_of(named, chosen.tuning.chunk_tokens);
        // A generation being built as named goes on being filled: the change it is for was decided before.
        let building_as_named = building
            .as_ref()
            .filter(|building| {
                plan::change(&made_of(building), &wanted) == Change::Nothing
                    && building.provider_fingerprint == provider_fingerprint
            })
            .cloned();
        let target = if let Some(building) = building_as_named {
            building
        } else {
            match active {
                None => match self
                    .begin_generation(org, named, "active", chosen.tuning.chunk_tokens)
                    .await
                {
                    Ok(made) => made,
                    Err(error) => return standing_of(error),
                },
                Some(active) => match generation_change(&active, &wanted, &provider_fingerprint) {
                    Change::Nothing => {
                        if active.writer_id != named.writer {
                            let mut conn = self.db().acquire().await?;
                            rows::generation_writer(&mut conn, active.id, named.writer).await?;
                        }
                        if let Some(building) = building {
                            self.drop_generation(building.id).await?;
                        }
                        active
                    }
                    Change::Elsewhere => match self
                        .probe_agrees(org, named, &active, None, chosen)
                        .await
                    {
                        Ok(true) => {
                            let mut conn = self.db().acquire().await?;
                            rows::move_generation(
                                &mut conn,
                                active.id,
                                named.provider_id,
                                &named.model_name,
                                &provider_fingerprint,
                                named.writer,
                            )
                            .await?;
                            active
                        }
                        Ok(false) => match self.rebuild(chosen, named, &active, building).await? {
                            Ok(target) => target,
                            Err(standing) => return Ok(standing),
                        },
                        Err(error) => return standing_of(error),
                    },
                    Change::Shorter { to } => {
                        let shorter = active
                            .dimensions
                            .is_some_and(|length| i64::from(to) < i64::from(length));
                        match self
                            .probe_agrees(org, named, &active, Some(to), chosen)
                            .await
                        {
                            Ok(true) if shorter => {
                                let stored: Vec<Vec<f32>> = probe_of(&active)
                                    .iter()
                                    .map(|vector| vectors::shortened(vector, to as usize))
                                    .collect();
                                let mut tx = self.db().begin().await?;
                                rows::move_generation(
                                    &mut tx,
                                    active.id,
                                    named.provider_id,
                                    &named.model_name,
                                    &provider_fingerprint,
                                    named.writer,
                                )
                                .await?;
                                rows::shorten(
                                    &mut tx,
                                    active.id,
                                    i32::try_from(to).unwrap_or(i32::MAX),
                                    &probe_json(&stored),
                                )
                                .await?;
                                tx.commit().await.map_err(DbError::from)?;
                                let mut conn = self.db().acquire().await?;
                                rows::generations(&mut conn, org)
                                    .await?
                                    .into_iter()
                                    .find(|generation| generation.id == active.id)
                                    .unwrap_or(active)
                            }
                            Ok(_) => match self.rebuild(chosen, named, &active, building).await? {
                                Ok(target) => target,
                                Err(standing) => return Ok(standing),
                            },
                            Err(error) => return standing_of(error),
                        }
                    }
                    Change::Rebuild => {
                        match self.rebuild(chosen, named, &active, building).await? {
                            Ok(target) => target,
                            Err(standing) => return Ok(standing),
                        }
                    }
                },
            }
        };
        self.fill(chosen, named, &target, started, budget, rate)
            .await
    }

    /// Begins a generation made with what is named, its probe texts' vectors made first.
    async fn begin_generation(
        &self,
        org: Id,
        named: &Embeddings,
        state: &'static str,
        chunk_tokens: i64,
    ) -> Result<rows::GenerationRow, EmbedError> {
        let provider_fingerprint = {
            let mut conn = self.db().acquire().await?;
            rows::endpoint_fingerprint(&mut conn, named.provider_id)
                .await?
                .unwrap_or_default()
        };
        let probe: Vec<String> = PROBE.iter().map(|text| named.document("", text)).collect();
        let made = self.embed_texts(org, named, probe).await?;
        let length = made.first().map_or(0, Vec::len);
        let length =
            i32::try_from(length).map_err(|error| EngineError::unreadable(error.to_string()))?;
        let id = new_id();
        let mut tx = self.db().begin().await?;
        let chunking = format!("{CHUNKING}:{chunk_tokens}");
        rows::insert_generation(
            &mut tx,
            &rows::NewGeneration {
                id,
                org_id: org,
                provider_id: named.provider_id,
                provider_fingerprint: &provider_fingerprint,
                writer_id: named.writer,
                model_name: &named.model_name,
                asked_dimensions: named
                    .dimensions
                    .and_then(|length| i32::try_from(length).ok()),
                query_prefix: &named.query_prefix,
                document_prefix: &named.document_prefix,
                chunking: &chunking,
                state,
                at: self.inner.clock.now(),
            },
        )
        .await?;
        rows::set_made(&mut tx, id, length, &probe_json(&made)).await?;
        rows::make_index(&mut tx, id, length).await?;
        tx.commit().await.map_err(DbError::from)?;
        let mut conn = self.db().acquire().await?;
        Ok(rows::generations(&mut conn, org)
            .await?
            .into_iter()
            .find(|generation| generation.id == id)
            .ok_or_else(|| {
                EngineError::unreadable("a generation vanished as it was begun".to_owned())
            })?)
    }

    /// Whether the endpoint named now makes the stored probe texts' vectors again, cut to `to` where given.
    async fn probe_agrees(
        &self,
        org: Id,
        named: &Embeddings,
        stored: &rows::GenerationRow,
        to: Option<u32>,
        chosen: &Choice,
    ) -> Result<bool, EmbedError> {
        let probe: Vec<String> = PROBE.iter().map(|text| named.document("", text)).collect();
        let fresh = self.embed_texts(org, named, probe).await?;
        let kept: Vec<Vec<f32>> = probe_of(stored)
            .iter()
            .map(|vector| match to {
                Some(length) => vectors::shortened(vector, length as usize),
                None => vector.clone(),
            })
            .collect();
        Ok(alike(&kept, &fresh, chosen.tuning.same_model_cosine_milli))
    }

    /// The generation a model change fills: the one being built if it is made as named, else a new one beside
    /// the one that answers, if the disk share holds both; otherwise the admin's choice is awaited.
    async fn rebuild(
        &self,
        chosen: &Choice,
        named: &Embeddings,
        active: &rows::GenerationRow,
        building: Option<rows::GenerationRow>,
    ) -> Result<Result<rows::GenerationRow, Standing>, EngineError> {
        let wanted = wanted_of(named, chosen.tuning.chunk_tokens);
        let provider_fingerprint = {
            let mut conn = self.db().acquire().await?;
            rows::endpoint_fingerprint(&mut conn, named.provider_id)
                .await?
                .unwrap_or_default()
        };
        if let Some(building) = building {
            if plan::change(&made_of(&building), &wanted) == Change::Nothing
                && building.provider_fingerprint == provider_fingerprint
            {
                return Ok(Ok(building));
            }
            self.drop_generation(building.id).await?;
        }
        let mut conn = self.db().acquire().await?;
        let sizes = rows::sizes(
            &mut conn,
            chosen.org,
            &[(active.id, active.dimensions)],
            Some(active.id),
            true,
        )
        .await?;
        drop(conn);
        // A new generation has a vector of each piece, of the length asked or, unasked, as long as the present one's.
        let needed = new_generation_bytes(
            named,
            active,
            sizes.chunks,
            sizes.source_bytes,
            sizes.sources,
        );
        if !plan::fits(sizes.used, needed, sizes.content, chosen.max_disk_share) {
            return Ok(Err(Standing::because(
                "waiting",
                "choice",
                format!(
                    "the new model's vectors take about {needed} bytes beside the {} of the present ones, past the disk share",
                    sizes.used
                ),
            )));
        }
        match self
            .begin_generation(chosen.org, named, "building", chosen.tuning.chunk_tokens)
            .await
        {
            Ok(made) => Ok(Ok(made)),
            Err(error) => Ok(Err(standing_of(error)?)),
        }
    }

    /// Removes a generation with its vectors and index.
    pub(crate) async fn drop_generation(&self, generation: Id) -> Result<(), EngineError> {
        let mut tx = self.db().begin().await?;
        rows::drop_generation(&mut tx, generation).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }

    /// Fills `target` for a turn: cuts what changed and makes what vectors are missing, within the share.
    async fn fill(
        &self,
        chosen: &Choice,
        named: &Embeddings,
        target: &rows::GenerationRow,
        started: Instant,
        budget: std::time::Duration,
        rate: &mut Option<f64>,
    ) -> Result<Standing, EngineError> {
        let org = chosen.org;
        let tuning = chosen.tuning;
        {
            let mut tx = self.db().begin().await?;
            rows::forget_gone(&mut tx, org).await?;
            tx.commit().await.map_err(DbError::from)?;
        }
        let changed = {
            let mut conn = self.db().acquire().await?;
            rows::changed_sources(&mut conn, org, target.id, chosen.tuning.source_batch).await?
        };
        for source in &changed {
            if started.elapsed() >= budget {
                return Ok(Standing::because(
                    "indexing",
                    "counting",
                    "more text waits to be counted",
                ));
            }
            let mut tx = self.db().begin().await?;
            if let Some(reason) = rows::provider_pause_reason(&mut tx, named.provider_id).await? {
                return Ok(Standing::because(
                    "paused",
                    reason,
                    super::embed::pause_detail(reason),
                ));
            }
            let (found, stored) = parts(&mut tx, &source.kind, source.id).await?;
            let pieces: Vec<rows::CutPiece> = chunks::cut(&found, tuning.chunk_tokens)
                .into_iter()
                .map(|piece| rows::CutPiece {
                    first_part: piece.first_part,
                    text_hash: chunks::text_hash(&source.title, &piece.text)
                        .as_bytes()
                        .to_vec(),
                    tokens: i32::try_from(piece.tokens).unwrap_or(i32::MAX),
                    bytes: i32::try_from(piece.text.len()).unwrap_or(i32::MAX),
                })
                .collect();
            rows::save_cut(&mut tx, source, org, target.id, &pieces, stored).await?;
            let generations = rows::generations(&mut tx, org)
                .await?
                .iter()
                .map(|generation| (generation.id, generation.dimensions))
                .collect::<Vec<_>>();
            let projected = rows::sizes(&mut tx, org, &generations, Some(target.id), true).await?;
            if !plan::fits(projected.used, 0, projected.content, chosen.max_disk_share) {
                return Ok(Standing::because(
                    if target.state == "building" {
                        "waiting"
                    } else {
                        "paused"
                    },
                    if target.state == "building" {
                        "choice"
                    } else {
                        "disk_share"
                    },
                    "the next cut's metadata would pass the index's disk share in bytes",
                ));
            }
            tx.commit().await.map_err(DbError::from)?;
        }
        let (pending, mut sizes) = {
            let mut conn = self.db().acquire().await?;
            let pending =
                rows::sources_to_embed(&mut conn, org, target.id, chosen.tuning.source_batch)
                    .await?;
            if changed.is_empty() && pending.is_empty() && target.state == "active" {
                return Ok(Standing::idle());
            }
            let generations: Vec<(Id, Option<i32>)> = rows::generations(&mut conn, org)
                .await?
                .iter()
                .map(|generation| (generation.id, generation.dimensions))
                .collect();
            let sizes = rows::sizes(&mut conn, org, &generations, Some(target.id), true).await?;
            // Count every source before showing a percentage or a whole-index duration. The first generation
            // can still answer as soon as its first vector is made, after this inventory phase.
            if !rows::changed_sources(&mut conn, org, target.id, 1)
                .await?
                .is_empty()
            {
                return Ok(Standing::because(
                    "indexing",
                    "counting",
                    "counting the text to index",
                ));
            }
            if target.state == "building" {
                // Finish planning the replacement's own cuts before any content is re-embedded. This admits
                // its real dimensions and chunk count, including a model's unrequested native length.
                let additional = rows::additional_vectors(&mut conn, target.id).await?;
                if !plan::fits(
                    sizes.used,
                    sizes.per_vector.saturating_mul(additional),
                    sizes.content,
                    chosen.max_disk_share,
                ) {
                    return Ok(Standing::because(
                        "waiting",
                        "choice",
                        "the replacement index would pass the disk share in bytes",
                    ));
                }
            }
            (pending, sizes)
        };
        let mut limit = tuning.batch_tokens.max(1);
        for source in &pending {
            if started.elapsed() >= budget {
                return Ok(Standing::because(
                    "indexing",
                    "more",
                    "more pieces wait for their vectors",
                ));
            }
            let (ready, title) = {
                let mut conn = self.db().acquire().await?;
                let title = rows::source_title(&mut conn, &source.kind, source.source_id).await?;
                let (found, _) = parts(&mut conn, &source.kind, source.source_id).await?;
                let cut = chunks::cut(&found, tuning.chunk_tokens);
                let kept =
                    rows::chunks_of(&mut conn, &source.kind, source.source_id, target.id).await?;
                let missing = rows::missing_vectors(
                    &mut conn,
                    target.id,
                    &kept.iter().map(|chunk| chunk.id).collect::<Vec<_>>(),
                )
                .await?;
                let mut ready = Vec::new();
                for chunk in kept.iter().filter(|chunk| missing.contains(&chunk.id)) {
                    let Some(piece) = usize::try_from(chunk.ordinal)
                        .ok()
                        .and_then(|at| cut.get(at))
                    else {
                        continue;
                    };
                    let hash = chunks::text_hash(&title, &piece.text).as_bytes().to_vec();
                    // The source changed since it was cut: its next cut sends it.
                    if hash != chunk.text_hash {
                        continue;
                    }
                    ready.push(Ready {
                        chunk: chunk.id,
                        hash,
                        text: piece.text.clone(),
                        tokens: piece.tokens,
                    });
                }
                (ready, title)
            };
            let mut rest: &[Ready] = &ready;
            while !rest.is_empty() {
                let mut take = 0;
                let mut tokens = 0;
                while take < rest.len() && (take == 0 || tokens + rest[take].tokens <= limit) {
                    tokens += rest[take].tokens;
                    take += 1;
                }
                let batch = &rest[..take];
                let existing = {
                    let mut conn = self.db().acquire().await?;
                    rows::stored_vectors(
                        &mut conn,
                        target.id,
                        &batch.iter().map(|piece| piece.chunk).collect::<Vec<_>>(),
                    )
                    .await?
                };
                // Replacing a stale row does not allocate a second vector row in the logical disk budget.
                let adding = sizes.per_vector.saturating_mul(
                    i64::try_from(batch.len())
                        .unwrap_or(i64::MAX)
                        .saturating_sub(existing),
                );
                if !plan::fits(sizes.used, adding, sizes.content, chosen.max_disk_share) {
                    return Ok(Standing::because(
                        "paused",
                        "disk_share",
                        format!(
                            "the index takes {} bytes of the {} its disk share allows",
                            sizes.used,
                            plan::allowed(sizes.content, chosen.max_disk_share).unwrap_or(i64::MAX)
                        ),
                    ));
                }
                let texts: Vec<String> = batch
                    .iter()
                    .map(|piece| named.document(&title, &piece.text))
                    .collect();
                let asked = Instant::now();
                match self.embed_texts(org, named, texts).await {
                    Ok(made) => {
                        let seconds = asked.elapsed().as_secs_f64();
                        if seconds > 0.0 {
                            let measured = tokens as f64 / seconds;
                            // Each measurement counts for half: the rate follows the endpoint without jumping.
                            *rate = Some(rate.map_or(measured, |before| (before + measured) / 2.0));
                        }
                        let stored: Vec<(Id, Vec<u8>, Vec<f32>)> = batch
                            .iter()
                            .zip(made)
                            .map(|(piece, vector)| (piece.chunk, piece.hash.clone(), vector))
                            .collect();
                        let mut tx = self.db().begin().await?;
                        rows::put_vectors(&mut tx, target.id, &stored).await?;
                        tx.commit().await.map_err(DbError::from)?;
                        sizes.used = sizes.used.saturating_add(adding);
                        rest = &rest[take..];
                    }
                    Err(EmbedError::Failed {
                        timed_out: true, ..
                    }) if take > 1 => {
                        // A request too large for the endpoint's time: the same pieces in halves.
                        limit = (tokens / 2).max(1);
                    }
                    Err(error) => return standing_of(error),
                }
                if started.elapsed() >= budget && !rest.is_empty() {
                    return Ok(Standing::because(
                        "indexing",
                        "more",
                        "more pieces wait for their vectors",
                    ));
                }
            }
            let mut conn = self.db().acquire().await?;
            let kept =
                rows::chunks_of(&mut conn, &source.kind, source.source_id, target.id).await?;
            let missing = rows::missing_vectors(
                &mut conn,
                target.id,
                &kept.iter().map(|chunk| chunk.id).collect::<Vec<_>>(),
            )
            .await?;
            if missing.is_empty() {
                rows::mark_embedded(
                    &mut conn,
                    &source.kind,
                    source.source_id,
                    source.marker,
                    target.id,
                )
                .await?;
            }
        }
        if target.state == "building" {
            let mut conn = self.db().acquire().await?;
            let left = rows::sources_to_embed(&mut conn, org, target.id, 1).await?;
            let uncut = rows::changed_sources(&mut conn, org, target.id, 1).await?;
            if left.is_empty() && uncut.is_empty() {
                let mut tx = self.db().begin().await?;
                let retired =
                    rows::activate(&mut tx, org, target.id, self.inner.clock.now()).await?;
                tx.commit().await.map_err(DbError::from)?;
                for old in retired {
                    self.drop_generation(old).await?;
                }
            }
        }
        if pending.len() >= usize::try_from(chosen.tuning.source_batch).unwrap_or(usize::MAX)
            || changed.len() >= usize::try_from(chosen.tuning.source_batch).unwrap_or(usize::MAX)
        {
            return Ok(Standing::because("indexing", "more", "more text waits"));
        }
        Ok(Standing::idle())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_vectors_of_one_model_are_alike_and_of_another_are_not() {
        let stored = [vec![1.0, 0.0], vec![0.0, 1.0]];
        let same = [vec![0.999, 0.01], vec![0.01, 0.999]];
        let other = [vec![0.0, 1.0], vec![1.0, 0.0]];
        assert!(alike(&stored, &same, 990));
        assert!(!alike(&stored, &other, 990));
        assert!(!alike(&stored, &same[..1], 990), "a probe text missing");
        assert!(!alike(&[], &[], 990), "no probe says nothing");
    }

    #[test]
    fn stored_probe_vectors_read_back_as_written() {
        let made = [vec![0.5_f32, -0.25], vec![1.0, 0.0]];
        let generation = rows::GenerationRow {
            id: Id::from_u128(1),
            org_id: Id::from_u128(2),
            provider_id: Id::from_u128(3),
            provider_fingerprint: vec![1],
            writer_id: None,
            model_name: "m".into(),
            asked_dimensions: None,
            dimensions: Some(2),
            query_prefix: String::new(),
            document_prefix: String::new(),
            chunking: CHUNKING.into(),
            probe: probe_json(&made),
            state: "active".into(),
            created_at: chrono::Utc::now(),
            activated_at: None,
        };
        assert_eq!(probe_of(&generation), made);
    }
}
