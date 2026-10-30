//! Indexed lexical candidate windows whose exact access and occurrence ranks are decided before payloads are returned.

mod fields;

use super::sparse::Scope;
use super::{CandidateSet, Content, Query};
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A field's stable recency boundary, independent of its later occurrence rank.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Boundary {
    /// The last candidate's content clock.
    pub at: DateTime<Utc>,
    /// Its part identity.
    pub id: Id,
}

/// Independent field positions let a resumed window advance both common and rare terms without duplicates.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Position {
    /// The last consumed boundary of each field stream.
    pub fields: BTreeMap<String, Boundary>,
    /// Streams that were completely read.
    pub exhausted: Vec<String>,
    /// Whether the formerly combined place/document description position has been expanded.
    #[serde(default)]
    pub split_descriptions: bool,
}

/// A part awaiting its caller's exact current permission check.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Part {
    /// `name`, `document`, `item` or `chat_message`.
    pub kind: String,
    /// Its document, thread or chat.
    pub source_id: Id,
    /// The header, block, item or message it stands for.
    pub part_id: Id,
    /// Its current place.
    pub node_id: Id,
    /// A block identity, where it is a block.
    pub block_id: Option<Id>,
    /// Its field tier.
    pub tier: i32,
    /// Its content clock.
    pub at: DateTime<Utc>,
}

/// A bounded pool and its raw continuation; neither contains an access decision.
#[derive(Debug)]
pub struct Window {
    /// Parts to authorize and rank.
    pub parts: Vec<Part>,
    /// The next raw field positions, including streams with no authorized hits.
    pub next: Option<Position>,
}

/// Scope routing and per-tier overrides, inherited through the ordinary word-search settings.
#[derive(Debug, Clone, Deserialize)]
pub struct Tuning {
    /// Estimated rows at which native scoped reads replace global candidate scans; zero disables routing.
    pub scope_candidates: i64,
    /// Candidate windows per response; absence uses one window and preserves a raw continuation.
    #[serde(default)]
    pub request_windows: Option<i64>,
    /// Stream limits within title, description, heading and body tiers; absent entries use word_candidates.
    #[serde(default)]
    pub tiers: BTreeMap<String, i64>,
}

impl Tuning {
    /// Bounds request work while leaving further candidates reachable through the continuation.
    #[must_use]
    pub fn request_windows(&self) -> i64 {
        // One native window already samples every enabled field; further windows are demand-paged.
        const DEFAULT_REQUEST_WINDOWS: i64 = 1;
        self.request_windows
            .unwrap_or(DEFAULT_REQUEST_WINDOWS)
            .max(DEFAULT_REQUEST_WINDOWS)
    }

    fn wanted(&self, field: &str, fallback: i64) -> i64 {
        let tier = match field {
            "name" | "title" => "title",
            "node_description" | "description" => "description",
            "file" | "heading" => "heading",
            _ => "body",
        };
        self.tiers.get(tier).copied().unwrap_or(fallback)
    }
}

const FIELDS: &[&str] = &[
    "name",
    "title",
    "node_description",
    "description",
    "file",
    "heading",
    "block",
    "chat",
    "item",
    "revision",
];
const FILTER: &str = r#"FROM search_fields s JOIN nodes n ON n.id=s.node_id
 WHERE __CANDIDATE_ACCESS__ AND s.field=$2 AND ($3::uuid IS NULL OR n.path @> ARRAY[$3::uuid])
 AND ($19::uuid IS NULL OR n.path @> ARRAY[$19::uuid])
 AND n.deleted_at IS NULL AND NOT COALESCE((SELECT bool_or(gone.deleted_at IS NOT NULL) FROM nodes gone WHERE gone.id=ANY(n.path)),false)
 AND (($4::uuid IS NULL AND $5::uuid IS NULL) OR (s.kind='chat_message' AND s.source_id=$4) OR (s.kind='document' AND s.source_id=$5))
 AND ((s.kind='name' AND $9) OR (s.kind='document' AND $8) OR (s.kind='chat_message' AND $7) OR (s.kind='item' AND s.item_kind=ANY($6::text[])))
 AND (NOT COALESCE(($20->'advanced'->>'title_only')::boolean,false) OR s.field IN ('name','title'))
 AND (s.kind<>'name' OR $20->'advanced'->>'kinds' IS NULL OR n.kind='thread')
 AND ($10::uuid IS NULL OR s.author_id=$10)
 AND (($20->'advanced'->>'date_field') IS NOT NULL OR $11::timestamptz IS NULL OR s.at >= $11)
 AND (($20->'advanced'->>'date_field') IS NOT NULL OR $12::timestamptz IS NULL OR s.at <= $12)
 AND __ADVANCED_SOURCE__
 AND __ADVANCED_FILE__
 AND __ADVANCED_SHARED__
 AND (NOT $13 OR s.has_file)
 AND ($14::uuid IS NULL OR CASE s.kind WHEN 'chat_message' THEN s.mentions @> to_jsonb($14::text)
   ELSE jsonb_path_exists(s.mentions,'$.** ? (@.type == "mention" && @.user_id == $person)',jsonb_build_object('person',$14::text)) END)
 AND ($15::timestamptz IS NULL OR (s.at,s.part_id)<($15,$16::uuid))"#;
const TEXT_MATCH: &str = "(cardinality($1::text[])=0 OR (s.text ILIKE ($1::text[])[1] AND s.text ILIKE ALL($1::text[])))";

fn short_query(query: &Query) -> Option<String> {
    let parts: Vec<String> = query
        .terms()
        .iter()
        .filter_map(|term| {
            let length = term.chars().count();
            if !(1..=2).contains(&length) {
                return None;
            }
            let prefix = if length == 1 { 'u' } else { 'b' };
            Some(format!(
                "'{prefix}{}'",
                term.to_lowercase()
                    .replace('\\', "\\\\")
                    .replace('\'', "''")
            ))
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(" & "))
}

#[derive(Clone, Copy)]
enum ReadMode {
    Probe,
    RecentSample,
    Recency,
}

#[derive(Clone, Copy)]
struct Scan<'a> {
    mode: ReadMode,
    bigram: bool,
    scope: Option<&'a Scope>,
    deferred: bool,
}

async fn load(
    conn: &mut Conn,
    ask: &Content<'_>,
    field: &str,
    after: Option<&Boundary>,
    limit: i64,
    scan: Scan<'_>,
) -> Result<Vec<Part>, DbError> {
    let Scan {
        mode,
        bigram,
        scope,
        deferred,
    } = scan;
    let probe = matches!(mode, ReadMode::Probe);
    let query = if ask
        .grants
        .get("search_regex")
        .and_then(orch_types::Json::as_bool)
        == Some(true)
    {
        Query::literal(ask.text)
    } else {
        Query::parse(ask.text)
    };
    let short = if bigram {
        query
            .terms()
            .iter()
            .filter(|term| term.chars().count() <= 2)
            .max_by_key(|term| term.chars().count())
            .map(|term| super::substring(&term.to_lowercase()))
    } else {
        short_query(&query)
    };
    let predicate = if probe && short.is_some() {
        if bigram {
            " AND lower(s.text) LIKE $18::text"
        } else {
            " AND orch_search_grams(s.text) @@ $18::tsquery"
        }
    } else {
        " AND $18::text IS NOT NULL"
    };
    // Keep the exact check off the plain trigram expression: it cannot narrow one- or two-character
    // words and can otherwise win over the selective short-word index with a full-index scan.
    let text_match = if probe && short.is_some() {
        TEXT_MATCH.replace("s.text", "(s.text || '')")
    } else {
        TEXT_MATCH.to_owned()
    };
    let bound_short = if probe && short.is_some() {
        short
    } else {
        Some(String::new())
    };
    let order = if probe {
        ""
    } else {
        " ORDER BY s.at DESC,s.part_id DESC"
    };
    let access = super::CANDIDATE_ACCESS
        .replace("__ACCESS__", "$20")
        .replace("__PATH__", "n.path")
        .replace("__OWNER__", "n.owner_id")
        .replace("__KIND__", "s.kind")
        .replace("__SOURCE__", "s.source_id");
    // Selecting a single native branch keeps ORDER BY on its own index; the union view otherwise sorts
    // every eligible document before its LIMIT, even when only a recent sample is requested.
    let source = fields::Source::of(field);
    let selection = ask.grants.get("search_advanced");
    let filtered_source = super::advanced_source_needed(selection)
        || super::advanced_file_needed(selection)
        || ask.with_file;
    let advanced = "($20::jsonb->'advanced')";
    let source_filter = if super::advanced_source_needed(selection) {
        source.as_ref().map_or_else(
            || super::advanced_source_sql(advanced, "s.kind", "s.source_id", "s.part_id"),
            |source| source.advanced_filter(advanced, field),
        )
    } else {
        "true".to_owned()
    };
    let file_filter = if super::advanced_file_needed(selection) {
        source.as_ref().map_or_else(
            || super::advanced_file_sql(advanced, "s.kind", "s.source_id", "s.part_id", true),
            |source| source.file_filter(advanced, field),
        )
    } else {
        "true".to_owned()
    };
    let shared_filter = if super::advanced_shared_needed(selection) {
        super::advanced_shared_sql(advanced, "s.kind", "s.source_id")
    } else {
        "true".to_owned()
    };
    let render = |sql: String| {
        let sql = sql
            .replace("__ADVANCED_FILE__", &file_filter)
            .replace("__ADVANCED_SHARED__", &shared_filter);
        match source.as_ref() {
            Some(source) => source.render(
                sql.replace("__ADVANCED_SOURCE__", &source_filter),
                field,
                &access,
                scope,
                fields::ReadPlan {
                    ordered: !probe,
                    filtered_source,
                    recent_sample: matches!(mode, ReadMode::RecentSample),
                },
            ),
            None => sql
                .replace("__CANDIDATE_ACCESS__", &access)
                .replace("__ADVANCED_SOURCE__", &source_filter),
        }
    };
    let columns = "s.kind AS kind,s.source_id AS source_id,s.part_id AS part_id,s.node_id AS node_id,s.block_id AS block_id,s.tier AS tier,s.at AS at";
    let mut sql = if matches!(mode, ReadMode::RecentSample) {
        let later = deferred
            .then(|| {
                source
                    .as_ref()
                    .and_then(|source| source.deferred_text(field))
            })
            .flatten();
        if super::advanced_file_needed(selection) || ask.with_file {
            // This is only a density sample. Its raw recency bound precedes MIME evaluation, so
            // an old rare attachment cannot make the sampler walk the whole history. A sparse
            // sample still falls through to the indexed probe before any result limit is chosen.
            let filter = FILTER
                .replace("__ADVANCED_FILE__", "true")
                .replace(" AND (NOT $13 OR s.has_file)", "");
            let media = format!("({file_filter} AND (NOT $13 OR s.has_file)) AS media_matches");
            let projected_text = if later.is_some() {
                ""
            } else {
                ",s.text AS text"
            };
            let inner = render(format!(
                "SELECT {columns},{media}{projected_text} {filter} ORDER BY s.at DESC,s.part_id DESC LIMIT $17"
            ));
            let (lookup, matching) = if let Some(later) = later {
                (
                    format!("CROSS JOIN LATERAL({later})matched"),
                    text_match.replace("s.text", "matched.text"),
                )
            } else {
                (String::new(), text_match.clone())
            };
            include_str!("../search_media_sample.sql")
                .replace("__OUTER_COLUMNS__", columns)
                .replace("__INNER__", &inner)
                .replace("__TEXT_LOOKUP__", &lookup)
                .replace(
                    "__TEXT_MATCH__",
                    &format!("{matching} AND $18::text IS NOT NULL"),
                )
        } else if let Some(later) = later {
            let inner = render(format!(
                "SELECT {columns} {FILTER} ORDER BY s.at DESC,s.part_id DESC LIMIT $17"
            ));
            let matching = text_match.replace("s.text", "matched.text");
            format!(
                "SELECT {columns} FROM ({inner})s CROSS JOIN LATERAL({later})matched WHERE {matching} AND $18::text IS NOT NULL"
            )
        } else {
            let inner = render(format!(
                "SELECT {columns},s.text AS text {FILTER} ORDER BY s.at DESC,s.part_id DESC LIMIT $17"
            ));
            format!("SELECT {columns} FROM ({inner})s WHERE {text_match} AND $18::text IS NOT NULL")
        }
    } else {
        render(format!(
            "SELECT {columns} {FILTER} AND {text_match}{predicate}{order} LIMIT $17"
        ))
    };
    if scope.is_some() {
        sql = format!("WITH {} {sql}", Scope::declarations(20));
    }
    let mut readable = ask.grants.get("search_readable").cloned();
    if let (Some(readable), Some(advanced)) = (&mut readable, ask.grants.get("search_advanced")) {
        readable["advanced"] = advanced.clone();
    }
    let mut statement = sqlx::query_as::<_, Part>(sqlx::AssertSqlSafe(sql))
        .persistent(false)
        .bind(query.like_patterns())
        .bind(field)
        .bind(ask.scope)
        .bind(ask.chat)
        .bind(ask.document)
        .bind(ask.item_kinds)
        .bind(ask.chat_messages)
        .bind(ask.document_content)
        .bind(ask.names)
        .bind(ask.author)
        .bind(ask.from)
        .bind(ask.to)
        .bind(ask.with_file)
        .bind(ask.mentions_me)
        .bind(after.map(|position| position.at))
        .bind(after.map(|position| position.id))
        .bind(limit)
        .bind(bound_short)
        .bind(
            ask.grants
                .get("search_subtree")
                .and_then(orch_types::Json::as_str)
                .and_then(|value| value.parse::<Id>().ok()),
        )
        .bind(readable);
    if let Some(scope) = scope {
        for ids in scope.parameters() {
            statement = statement.bind(ids);
        }
    }
    Ok(statement.fetch_all(conn).await?)
}

/// Samples through text indexes, uses recency indexes for common terms, and leaves exact ranking to its caller.
pub async fn window(
    conn: &mut Conn,
    ask: &Content<'_>,
    position: Position,
    wanted: i64,
    probe: i64,
    recent: i64,
) -> Result<Window, DbError> {
    window_inner(conn, ask, position, wanted, probe, recent, None).await
}

/// Read native scoped candidates under configured routing and tier budgets.
pub async fn window_with_tuning(
    conn: &mut Conn,
    ask: &Content<'_>,
    position: Position,
    wanted: i64,
    probe: i64,
    recent: i64,
    tuning: &Tuning,
) -> Result<Window, DbError> {
    window_inner(conn, ask, position, wanted, probe, recent, Some(tuning)).await
}

async fn window_inner(
    conn: &mut Conn,
    ask: &Content<'_>,
    mut position: Position,
    wanted: i64,
    probe: i64,
    recent: i64,
    tuning: Option<&Tuning>,
) -> Result<Window, DbError> {
    // Older cursors bounded both kinds of description together; neither may restart on an upgrade.
    if !position.split_descriptions {
        if let Some(boundary) = position.fields.get("description").cloned() {
            position
                .fields
                .entry("node_description".into())
                .or_insert(boundary);
        }
        if position
            .exhausted
            .iter()
            .any(|field| field == "description")
            && !position
                .exhausted
                .iter()
                .any(|field| field == "node_description")
        {
            position.exhausted.push("node_description".into());
        }
        position.split_descriptions = true;
    }
    let settings: (String,String,String) = sqlx::query_as("SELECT current_setting('enable_seqscan'),current_setting('enable_indexscan'),current_setting('enable_bitmapscan')").fetch_one(&mut *conn).await?;
    let bigram:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_index i JOIN pg_class c ON c.oid=i.indexrelid JOIN pg_opclass op ON op.oid=ANY(i.indclass) WHERE c.relname='search_blocks_text_short_idx' AND op.opcname='gin_bigm_ops')").fetch_one(&mut *conn).await?;
    let within: Vec<_> = ask
        .scope
        .into_iter()
        .chain(
            ask.grants
                .get("search_subtree")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| value.parse::<Id>().ok()),
        )
        .collect();
    let scope = if let Some(tuning) = tuning
        && (ask.document.is_some() || ask.chat.is_some())
    {
        Scope::focused(conn, ask.document, ask.chat, tuning.scope_candidates, None).await?
    } else if let (Some(tuning), Some(readable)) = (tuning, ask.grants.get("search_readable")) {
        Scope::load(conn, readable, &within, tuning.scope_candidates, None).await?
    } else {
        None
    };
    let scan = Scan {
        mode: ReadMode::RecentSample,
        bigram,
        scope: scope.as_ref(),
        deferred: tuning.is_some(),
    };
    let mut parts = Vec::new();
    let mut tier = i32::MAX;
    let indexed_media =
        super::advanced_file_needed(ask.grants.get("search_advanced")) || ask.with_file;
    for field in FIELDS {
        let field_wanted = tuning.map_or(wanted, |tuning| tuning.wanted(field, wanted));
        let field_tier = match *field {
            "name" | "title" => 4,
            "node_description" | "description" => 3,
            "file" | "heading" => 2,
            _ => 0,
        };
        if ask.relevance
            && field_tier < tier
            && i64::try_from(parts.len()).unwrap_or(i64::MAX) >= wanted
        {
            break;
        }
        tier = field_tier;
        if position.exhausted.iter().any(|done| done == field) {
            continue;
        }
        if indexed_media {
            // A rare file type can enter through its GIN projection; a common type can keep the
            // ordered metadata index. Forcing either scan kind would make the other walk history.
            sqlx::query("SELECT set_config('enable_seqscan','off',true),set_config('enable_indexscan','on',true),set_config('enable_bitmapscan','on',true)").execute(&mut *conn).await?;
        } else {
            sqlx::query("SELECT set_config('enable_seqscan','off',true),set_config('enable_indexscan','on',true),set_config('enable_bitmapscan','off',true)").execute(&mut *conn).await?;
        }
        let recent_matches =
            load(conn, ask, field, position.fields.get(*field), recent, scan).await?;
        let recent_common = i64::try_from(recent_matches.len()).unwrap_or(i64::MAX) >= field_wanted;
        let sampled = if recent_common {
            Vec::new()
        } else {
            if !indexed_media {
                sqlx::query("SELECT set_config('enable_indexscan','off',true),set_config('enable_bitmapscan','on',true)").execute(&mut *conn).await?;
            }
            load(
                conn,
                ask,
                field,
                position.fields.get(*field),
                probe.saturating_add(1),
                Scan {
                    mode: ReadMode::Probe,
                    ..scan
                },
            )
            .await?
        };
        let common = recent_common || i64::try_from(sampled.len()).unwrap_or(i64::MAX) > probe;
        let mut selected = if recent_common && tuning.is_some() {
            // The first wanted matches in this ordered sample are exactly the following recency read.
            // Reuse them inside the same snapshot instead of reading and filtering them a second time.
            recent_matches
        } else if common {
            if !indexed_media {
                sqlx::query("SELECT set_config('enable_indexscan','on',true),set_config('enable_bitmapscan','off',true)").execute(&mut *conn).await?;
            }
            load(
                conn,
                ask,
                field,
                position.fields.get(*field),
                field_wanted,
                Scan {
                    mode: ReadMode::Recency,
                    ..scan
                },
            )
            .await?
        } else {
            sampled
        };
        selected.sort_by_key(|part| std::cmp::Reverse((part.at, part.part_id)));
        if !common && i64::try_from(selected.len()).unwrap_or(i64::MAX) <= field_wanted {
            position.exhausted.push((*field).to_owned());
        }
        selected.truncate(usize::try_from(field_wanted).unwrap_or(usize::MAX));
        if let Some(last) = selected.last() {
            position.fields.insert(
                (*field).to_owned(),
                Boundary {
                    at: last.at,
                    id: last.part_id,
                },
            );
        }
        parts.extend(selected);
    }
    sqlx::query("SELECT set_config('enable_seqscan',$1,true),set_config('enable_indexscan',$2,true),set_config('enable_bitmapscan',$3,true)")
        .bind(settings.0).bind(settings.1).bind(settings.2).execute(&mut *conn).await?;
    Ok(Window {
        parts,
        next: (position.exhausted.len() < FIELDS.len()).then_some(position),
    })
}

impl CandidateSet {
    /// Collects the dependencies of a raw window before its payloads are authorized.
    pub fn of(parts: &[Part]) -> Self {
        let mut set = Self::default();
        for part in parts {
            set.nodes.push(part.node_id);
            match part.kind.as_str() {
                "name" => set.names.push(part.part_id),
                "document" => {
                    set.documents.push(part.source_id);
                    if part.block_id.is_some() {
                        set.blocks.push(part.part_id);
                    } else {
                        set.headers.push(part.part_id);
                    }
                }
                "chat_message" => {
                    set.chats.push(part.source_id);
                    set.messages.push(part.part_id);
                }
                _ => set.items.push(part.part_id),
            }
        }
        for ids in [
            &mut set.nodes,
            &mut set.names,
            &mut set.documents,
            &mut set.blocks,
            &mut set.headers,
            &mut set.chats,
            &mut set.messages,
            &mut set.items,
        ] {
            ids.sort_unstable();
            ids.dedup();
        }
        set
    }
}
