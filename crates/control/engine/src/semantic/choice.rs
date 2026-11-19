//! What an org chose for search by meaning: whether it is on, which provider and model make its vectors and who named them, the disk it may take, and how it cuts, finds and joins.
//!
//! The index of an org tree is one, so its keys are read at the root org: `search.semantic.enabled` (off as
//! shipped), `search.semantic.embeddings` (unset as shipped), `search.semantic.max_disk_share` (100 % as
//! shipped, empty for no limit) and `search.semantic.tuning`. Whoever last wrote the embeddings key where it
//! resolves answers for the use of what it names, as the summarizer's writer does.

use crate::error::EngineError;
use orch_db::Conn;
use orch_db::queries::settings as stored;
use orch_settings::{Place, get, resolve};
use orch_settings_contract::registry::keys;
use orch_types::Id;
use serde::Deserialize;

/// The ordering mode of a broad iterative graph scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IterativeScan {
    /// Preserve exact distance ordering while the graph continues.
    #[default]
    StrictOrder,
    /// Permit relaxed graph order and sort the selected results before returning them.
    RelaxedOrder,
    /// Use only the initial graph breadth.
    Off,
}

impl IterativeScan {
    /// The pgvector setting value used only inside this lookup's transaction.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StrictOrder => "strict_order",
            Self::RelaxedOrder => "relaxed_order",
            Self::Off => "off",
        }
    }
}

/// How search by meaning cuts, sends, finds and joins, as `search.semantic.tuning` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Tuning {
    /// The estimated tokens of one piece.
    pub chunk_tokens: i64,
    /// The estimated tokens of one request to the endpoint.
    pub batch_tokens: i64,
    /// How many results by meaning are joined with those by words.
    pub candidates: i64,
    /// How many candidates the index looks at for each result kept.
    pub oversample: i64,
    /// The constant of reciprocal-rank fusion.
    pub fusion_k: u32,
    /// The least likeness, in thousandths, at which another endpoint serves the same model.
    pub same_model_cosine_milli: i64,
    /// How often the indexer looks for work, in seconds.
    pub interval_secs: i64,
    /// Small filtered sets searched exactly; zero keeps graph retrieval for every query.
    #[serde(default = "default_exact_filter")]
    pub exact_filter_candidates: i64,
    /// Sources cut or embedded in one background batch.
    #[serde(default = "default_source_batch")]
    pub source_batch: i64,
    /// The iterative graph scan budget; pgvector ships twenty thousand visits as its starting point.
    #[serde(default = "default_max_scan")]
    pub max_scan_tuples: i64,
    /// Extra scan memory relative to PostgreSQL work_mem.
    #[serde(default = "default_scan_memory")]
    pub scan_mem_multiplier: i64,
    /// Explicit initial breadth; absence preserves candidates multiplied by oversample.
    #[serde(default)]
    pub ef_search: Option<i64>,
    /// Broad graph ordering; older saved tuning objects keep strict ordering.
    #[serde(default)]
    pub iterative_scan: IterativeScan,
    /// How long a search waits for its query's vector before it answers by words alone, in seconds.
    pub query_wait_secs: i64,
}

const fn default_exact_filter() -> i64 {
    20_000
}

const fn default_source_batch() -> i64 {
    64
}

// Keep older saved tuning objects valid while exposing the defaults in the setting.
const fn default_max_scan() -> i64 {
    20_000
}
const fn default_scan_memory() -> i64 {
    2
}

/// The provider and model named to make an org's vectors.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Embeddings {
    /// The API provider.
    pub provider_id: Id,
    /// The model, by the name that is sent.
    pub model_name: String,
    /// The length asked of a model that can shorten its vectors; none for its own.
    #[serde(default)]
    pub dimensions: Option<u32>,
    /// What is put before a query.
    #[serde(default)]
    pub query_prefix: String,
    /// What is put before a document; `{title}` stands for its title, or `none`.
    #[serde(default)]
    pub document_prefix: String,
    /// Whoever last wrote the key where it resolves, who answers for its use.
    #[serde(skip)]
    pub writer: Option<Id>,
}

impl Embeddings {
    /// A document's text as it is sent: the document prefix with its title in place, then the text.
    #[must_use]
    pub fn document(&self, title: &str, text: &str) -> String {
        let title = if title.trim().is_empty() {
            "none"
        } else {
            title.trim()
        };
        format!("{}{text}", self.document_prefix.replace("{title}", title))
    }

    /// A query as it is sent.
    #[must_use]
    pub fn query(&self, text: &str) -> String {
        format!("{}{text}", self.query_prefix)
    }
}

/// What an org chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// The root org.
    pub org: Id,
    /// Whether search by meaning is on.
    pub enabled: bool,
    /// The provider and model named for it, if any.
    pub embeddings: Option<Embeddings>,
    /// The disk it may take, in percent of what it indexes; none for no limit.
    pub max_disk_share: Option<i64>,
    /// How it cuts, finds and joins.
    pub tuning: Tuning,
}

fn stored_error(error: impl std::fmt::Display) -> EngineError {
    EngineError::unreadable(error.to_string())
}

/// What the org tree rooted at `org` chose.
pub async fn choice(conn: &mut Conn, org: Id) -> Result<Choice, EngineError> {
    let place = Place::Node(org);
    let enabled = get::<bool>(conn, place, keys::SEARCH_SEMANTIC_ENABLED)
        .await
        .map_err(stored_error)?
        .unwrap_or(false);
    let resolved = resolve(conn, place, keys::SEARCH_SEMANTIC_EMBEDDINGS)
        .await
        .map_err(stored_error)?;
    let embeddings = match resolved
        .resolved
        .value
        .clone()
        .filter(|value| !value.is_null())
    {
        Some(value) => {
            let mut named: Embeddings = serde_json::from_value(value).map_err(stored_error)?;
            named.writer = stored::row(
                conn,
                resolved.resolved.source.node,
                keys::SEARCH_SEMANTIC_EMBEDDINGS,
            )
            .await?
            .and_then(|row| row.updated_by);
            Some(named)
        }
        None => None,
    };
    let max_disk_share = get::<Option<i64>>(conn, place, keys::SEARCH_SEMANTIC_MAX_DISK_SHARE)
        .await
        .map_err(stored_error)?
        .flatten();
    let tuning = get::<Tuning>(conn, place, keys::SEARCH_SEMANTIC_TUNING)
        .await
        .map_err(stored_error)?
        .ok_or_else(|| stored_error("search.semantic.tuning has no value"))?;
    Ok(Choice {
        org,
        enabled,
        embeddings,
        max_disk_share,
        tuning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_is_sent_with_its_title_in_the_prefix_and_a_query_with_its_own() {
        let named = Embeddings {
            provider_id: Id::from_u128(1),
            model_name: "embeddinggemma-2".into(),
            dimensions: Some(256),
            query_prefix: "task: search result | query: ".into(),
            document_prefix: "title: {title} | text: ".into(),
            writer: None,
        };
        assert_eq!(
            named.document("发布流程", "先构建。"),
            "title: 发布流程 | text: 先构建。"
        );
        assert_eq!(named.document("  ", "text"), "title: none | text: text");
        assert_eq!(
            named.query("rollout"),
            "task: search result | query: rollout"
        );
        let plain = Embeddings {
            query_prefix: String::new(),
            document_prefix: String::new(),
            ..named
        };
        assert_eq!(plain.document("Title", "text"), "text");
    }
}
