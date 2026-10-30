//! Search everything in an agent's reach by a word or a bounded pattern, narrowed by kind, place or name: what the database holds first, then inside files, within bounds that are said when they are reached.
//!
//! Literal queries use the same words-in-any-order and quoted-phrase grammar as people's search. With `regex`
//! the query is a regular expression in the
//! syntax of the Rust `regex` crate, which has no look-around and no back-references and is matched in time linear
//! in what it reads, and which ignores case unless it says `(?-i)`. A pattern longer than a thousand characters, or
//! one that does not compile within the crate's default size limit, is refused with the reason.
//!
//! **What is looked at first.** The names of places, threads and documents, and the text the database holds
//! (documents, chats, conversations), through the indexes that people's search uses; then the names of files. A
//! regular expression is matched by the engine itself over the candidate rows, which the literal parts of it of
//! three characters or more narrow first; a pattern without any is the slow case that the bounds are for. Only what
//! the agent may read is looked at, each call anew.
//!
//! **Bounds.** One call reads at most `sharing.snapshot_max_bytes` of text (as resolved for the calling thread) and
//! takes at most twenty seconds; when either ends the search, `truncated` is true and `searched` says how far it
//! got. Continuations finish each bounded scan window before advancing to the next; partial counts and positions
//! describe that window. Each entry appears once per answer, with snippets only on request.
//!
//! **Order.** Windows advance from the newest text to the oldest. Within one, entries are ranked as people's search
//! ranks its best matches ([`orch_db::queries::search::Rank`]): by the most telling field that matched (a name or a
//! title, a description, a heading, the rest), then by how many of its rows matched, then the newest, then by
//! identifier.
//!
//! **By meaning.** Where the agent's org searches by meaning, the first window of a search by words is joined with
//! the entries whose pieces are nearest the query by meaning ([`crate::semantic::query`]): one order by
//! reciprocal-rank fusion, each entry once, and only entries the agent may read now (the tree it may read is
//! where their paths come from). Each hit then says `found_by`: `words`, `meaning` or both; an entry found by
//! meaning alone has no match count and no snippet. The cursor continues both sources, pinning the embedding generation.

mod indexed;
mod referenced;
mod related;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::tools::definitions::{CONTEXT_MAX, Code, QUERY_MAX_CHARS, RESULTS_MAX, Refusal};
use crate::tools::shared::audit_by_agent;
use crate::tools::tree::{FoundEntry, Pattern};
use crate::tools::{Answer, CallContext};
use base64::Engine as _;
use orch_access_contract::audit::AuditAction;
use orch_api_contract::share::SharedEntryKind;
use orch_db::Conn;
use orch_db::queries::search::{Field, Query, Rank};
use orch_db::queries::shares::{self as rows, TextRow, like_escaped};
use orch_db::queries::threads::ThreadRow;
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Hash, Id, Json};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, Instant};

/// How many hits a search returns when it is not told.
pub const HITS_DEFAULT: i64 = 50;

/// How many lines around a match a snippet carries when it is not told.
pub const CONTEXT_DEFAULT: i64 = 1;

/// The longest snippet, in characters.
pub const SNIPPET_CHARS: usize = 400;

/// How long one search may take.
pub const TIME_LIMIT: Duration = Duration::from_secs(20);

/// The total prefetch budget for the merged text streams, before checking each consumed row's bounds.
pub const BATCH_ROWS: i64 = 500;
/// Three text streams share the previous batch budget rather than tripling buffered body text.
const STREAM_BATCH_ROWS: i64 = BATCH_ROWS / 3;

/// The most files named in `skipped`.
pub const SKIPPED_MAX: usize = 20;

/// The shortest run of a pattern's own characters that narrows the rows the database gives.
pub const LITERAL_MIN_CHARS: usize = 3;

/// The default bytes one search reads, if the setting cannot be read: the shipped value.
const SNAPSHOT_DEFAULT: i64 = 67_108_864;
/// Five native positions/samples keep an entry's summary compact; read_shared supplies the complete range.
const FIRST_POSITIONS: usize = 5;

struct MeaningTask(
    Option<tokio::task::JoinHandle<Result<crate::semantic::query::Meaning, EngineError>>>,
);
impl Drop for MeaningTask {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

/// The arguments of `search_shared`, once the schema has let them through.
#[derive(Debug, Deserialize)]
struct Arguments {
    #[serde(default)]
    query: String,
    related_to: Option<String>,
    referenced_by: Option<String>,
    regex: Option<bool>,
    path: Option<String>,
    kinds: Option<Vec<String>>,
    names: Option<String>,
    max_results: Option<i64>,
    context: Option<i64>,
    snippets: Option<bool>,
    cursor: Option<String>,
}

/// What is looked for: the matches of one expression.
struct Matcher {
    expression: Regex,
    literal: Option<Query>,
}

impl Matcher {
    /// Literal terms in any order, with quoted phrases and the shared case-insensitive matcher.
    fn plain(query: &str) -> Result<Self, String> {
        let literal = Query::parse(query);
        if literal.terms().is_empty() {
            return Err("Give at least one word or quoted phrase.".to_owned());
        }
        let pattern = literal
            .terms()
            .iter()
            .map(|term| regex::escape(term))
            .collect::<Vec<_>>()
            .join("|");
        let mut matcher = Self::expression(&pattern, true)?;
        matcher.literal = Some(literal);
        Ok(matcher)
    }

    fn expression(pattern: &str, ignore_case: bool) -> Result<Self, String> {
        RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .build()
            .map(|expression| Self {
                expression,
                literal: None,
            })
            .map_err(|error| error.to_string())
    }

    /// The byte ranges of the matches in a text.
    fn find(&self, text: &str) -> Vec<(usize, usize)> {
        if self
            .literal
            .as_ref()
            .is_some_and(|query| !query.matches(text))
        {
            return Vec::new();
        }
        self.ranges(text)
    }

    /// Summaries need only presence, so a long body need not allocate every highlight range.
    fn matches(&self, text: &str) -> bool {
        self.literal
            .as_ref()
            .is_none_or(|query| query.matches(text))
            && self.expression.is_match(text)
    }

    fn ranges(&self, text: &str) -> Vec<(usize, usize)> {
        self.expression
            .find_iter(text)
            .map(|found| (found.start(), found.end()))
            .collect()
    }
}

/// Compiles the same case-insensitive, linear-time expression used by agents' search.
pub fn search_expression(pattern: &str) -> Result<Regex, String> {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .map_err(|error| error.to_string())
}

/// An indexed literal that every match of the expression must contain, when one is known.
#[must_use]
pub fn expression_literal(pattern: &str) -> Option<String> {
    literal_of(pattern)
}

/// The longest run of a pattern's own characters that any match must contain, if it has one of three
/// characters or more: what narrows the rows the database gives before the expression is matched.
fn literal_of(pattern: &str) -> Option<String> {
    if pattern.contains('|') {
        return None;
    }
    let mut best = String::new();
    let mut run = String::new();
    let mut depth = 0_u32;
    let mut in_class = false;
    let mut chars = pattern.chars().peekable();
    let end_run = |run: &mut String, best: &mut String| {
        if run.chars().count() > best.chars().count() {
            std::mem::swap(run, best);
        }
        run.clear();
    };
    while let Some(c) = chars.next() {
        if in_class {
            if c == ']' {
                in_class = false;
            }
            continue;
        }
        match c {
            '\\' => {
                chars.next();
                end_run(&mut run, &mut best);
            }
            '[' => {
                in_class = true;
                end_run(&mut run, &mut best);
            }
            '(' => {
                depth += 1;
                end_run(&mut run, &mut best);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                end_run(&mut run, &mut best);
            }
            '.' | '^' | '$' | '+' | '?' | '*' | '{' | '}' => {
                // A quantifier makes the character before it optional or repeated.
                if matches!(c, '?' | '*' | '{') {
                    run.pop();
                }
                end_run(&mut run, &mut best);
            }
            other if depth == 0 => run.push(other),
            _ => {}
        }
    }
    end_run(&mut run, &mut best);
    (best.chars().count() >= LITERAL_MIN_CHARS).then_some(best)
}

/// A snippet of a text around its first match: the lines around the line it is on, cut to [`SNIPPET_CHARS`]
/// characters, with where in it the matches are, counted in characters.
fn snippet(text: &str, matcher: &Matcher, context: usize) -> Option<(String, Vec<(usize, usize)>)> {
    let found = matcher.find(text);
    let (start, _) = *found.first()?;
    let lines: Vec<&str> = text.split('\n').collect();
    let mut offset = 0;
    let mut at = 0;
    for (index, line) in lines.iter().enumerate() {
        if start < offset + line.len() + 1 {
            at = index;
            break;
        }
        offset += line.len() + 1;
    }
    let first = at.saturating_sub(context);
    let last = (at + context).min(lines.len().saturating_sub(1));
    let mut shown = lines[first..=last].join("\n");
    if shown.chars().count() > SNIPPET_CHARS {
        // Around the match, not its beginning.
        let before: usize = lines[first..at].iter().map(|line| line.len() + 1).sum();
        let from = (before + (start - offset)).saturating_sub(SNIPPET_CHARS / 4);
        let mut begin = from.min(shown.len());
        while begin > 0 && !shown.is_char_boundary(begin) {
            begin -= 1;
        }
        shown = shown[begin..].chars().take(SNIPPET_CHARS).collect();
    }
    let matches: Vec<(usize, usize)> = matcher
        .ranges(&shown)
        .into_iter()
        .map(|(from, to)| (shown[..from].chars().count(), shown[..to].chars().count()))
        .collect();
    Some((shown, matches))
}

/// One hit, as it is given.
struct Hit {
    key: SearchKey,
    field: Field,
    owner: Id,
    path: String,
    kind: SharedEntryKind,
    position: Option<i64>,
    snippet: String,
    matches: Vec<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct SearchKey {
    at: chrono::DateTime<chrono::Utc>,
    id: Id,
    path: String,
}

/// Where an entry stands among the others of its window, the greatest first: the shared rank of literal matches,
/// then its path, which tells apart two entries of one owner (a document and a file it holds).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct EntryRank {
    rank: Rank,
    path: String,
}

/// Three newest-first text streams and one name stream advance independently without skipping a buffered row.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Scan {
    before: [Option<SearchKey>; 4],
    done: [bool; 4],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SemanticPosition {
    offset: i64,
    done: bool,
    generation: Option<Id>,
    reason: Option<String>,
}

/// Result pages stay within one bounded scan window; the next window starts only after its last entry is shown.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SearchCursor {
    fingerprint: Hash,
    start: Scan,
    end: Option<Scan>,
    before_entry: Option<EntryRank>,
    /// In a first window joined with results by meaning: how many of the joined entries were shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fused: Option<usize>,
    /// Entries already returned from a fused window, including when later pages fall back to words.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    fused_shown: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    meaning: Option<SemanticPosition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    indexed: Option<indexed::Position>,
}

impl SearchCursor {
    fn from_token(text: &str) -> Option<Self> {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(text)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }

    fn token(&self) -> Result<String, serde_json::Error> {
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(self)?))
    }

    fn valid_for(&self, fingerprint: &Hash) -> bool {
        if &self.fingerprint != fingerprint
            || ((self.before_entry.is_some() || self.fused.is_some()) && self.end.is_none())
        {
            return false;
        }
        self.end.as_ref().is_none_or(|end| {
            // A joined first window pages by its place in the joined order, even when no word matched.
            (end != &self.start || self.fused.is_some())
                && (0..4).all(|source| {
                    let forward = match (&self.start.before[source], &end.before[source]) {
                        (Some(start), Some(end)) => end <= start,
                        (None, _) => true,
                        (Some(_), None) => false,
                    };
                    forward
                        && (!self.start.done[source]
                            || (end.done[source]
                                && end.before[source] == self.start.before[source]))
                })
        })
    }
}

struct Summary {
    key: SearchKey,
    /// Found by meaning, alone or beside its words.
    by_meaning: bool,
    field: Field,
    path: String,
    kind: SharedEntryKind,
    owner: Id,
    count: i64,
    positions: BTreeSet<i64>,
    snippets: Vec<Json>,
}

impl Summary {
    fn rank(&self) -> EntryRank {
        EntryRank {
            rank: Rank {
                field: self.field,
                count: self.count,
                at: self.key.at,
                id: self.key.id,
            },
            path: self.key.path.clone(),
        }
    }
}

/// How far a search got.
#[derive(Default)]
struct Reached {
    entries: i64,
    files: i64,
    bytes: i64,
    skipped: Vec<Json>,
}

impl Engine {
    /// `search_shared`: the call of an agent.
    #[allow(
        clippy::too_many_lines,
        reason = "a search is the same walk over four kinds of text; they are told apart where it matters"
    )]
    pub(crate) async fn search_shared_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
    ) -> Result<Answer, EngineError> {
        let arguments: Arguments = match serde_json::from_value(call.arguments.clone()) {
            Ok(arguments) => arguments,
            Err(error) => return Ok(Refusal::invalid(error.to_string()).into()),
        };
        if arguments.related_to.is_some() && arguments.referenced_by.is_some() {
            return Ok(Refusal::invalid(
                "Give one of query, related_to or referenced_by, not several.",
            )
            .into());
        }
        if let Some(source) = arguments.referenced_by.as_deref() {
            return match tokio::time::timeout(
                TIME_LIMIT,
                self.referenced_by_call(thread, &arguments, source),
            )
            .await
            {
                Ok(answer) => answer,
                Err(_) => Ok(Refusal::new(
                    Code::Limit,
                    "The referenced_by lookup timed out. Try again or narrow its path, kinds or names.",
                )
                .into()),
            };
        }
        if let Some(source) = arguments.related_to.as_deref() {
            return match tokio::time::timeout(
                TIME_LIMIT,
                self.related_shared_call(thread, &arguments, source),
            )
            .await
            {
                Ok(answer) => answer,
                Err(_) => Ok(Refusal::new(
                    Code::Limit,
                    "The related lookup timed out. Try again or narrow its path, kinds or names.",
                )
                .into()),
            };
        }
        if arguments.query.trim().is_empty() || arguments.query.chars().count() > QUERY_MAX_CHARS {
            return Ok(Refusal::invalid(format!(
                "\"query\" is from 1 to {QUERY_MAX_CHARS} characters."
            ))
            .into());
        }
        let regex = arguments.regex == Some(true);
        let matcher = match if regex {
            Matcher::expression(&arguments.query, true)
        } else {
            Matcher::plain(&arguments.query)
        } {
            Ok(matcher) => matcher,
            Err(reason) => {
                return Ok(
                    Refusal::invalid(format!("\"query\" cannot be matched: {reason}")).into(),
                );
            }
        };
        let patterns: Vec<String> = if regex {
            literal_of(&arguments.query)
                .map(|literal| like_escaped(&literal))
                .into_iter()
                .collect()
        } else {
            Query::parse(&arguments.query).like_patterns()
        };
        let pattern = match arguments.names.as_deref().map(Pattern::new) {
            Some(None) => {
                return Ok(
                    Refusal::invalid("\"names\" takes *, ? and [...] as a shell pattern.").into(),
                );
            }
            Some(Some(pattern)) => Some(pattern),
            None => None,
        };
        let wanted = |kind: &str| {
            arguments
                .kinds
                .as_ref()
                .is_none_or(|kinds| kinds.iter().any(|word| word == kind))
        };
        let limit = usize::try_from(
            arguments
                .max_results
                .unwrap_or(HITS_DEFAULT)
                .clamp(1, RESULTS_MAX),
        )
        .unwrap_or(1);
        let context = usize::try_from(
            arguments
                .context
                .unwrap_or(CONTEXT_DEFAULT)
                .clamp(0, CONTEXT_MAX),
        )
        .unwrap_or(1);
        let snippets = arguments.snippets == Some(true);
        let mut conn = self.inner.db.acquire().await?;
        let asked = arguments.path.as_deref().unwrap_or("/");
        let root_fingerprint=Hash::of(&serde_json::to_vec(&json!({
            "thread":thread.node_id,"query":arguments.query,"regex":regex,"scope":"/",
            "entry":format!("{:?}",FoundEntry::Directory(crate::tools::tree::Where::Root)),
            "kinds":arguments.kinds,"names":arguments.names,"snippets":snippets,"context":context,
        })).map_err(EngineError::unreadable)?);
        let early_cursor = arguments
            .cursor
            .as_deref()
            .and_then(SearchCursor::from_token);
        let indexed_root = !regex
            && asked == "/"
            && (arguments.cursor.is_none()
                || early_cursor
                    .as_ref()
                    .is_some_and(|cursor| cursor.indexed.is_some()));
        if indexed_root
            && early_cursor
                .as_ref()
                .is_some_and(|cursor| !cursor.valid_for(&root_fingerprint))
        {
            return Ok(Refusal::invalid("This cursor belongs to another search. Keep the query, path and filters, or restart without cursor.").into());
        }
        let phase = Instant::now();
        let mut prepared = if indexed_root {
            Some(indexed::prepare(self, thread.node_id, &arguments, early_cursor.as_ref()).await?)
        } else {
            None
        };
        tracing::debug!(target: "orch::search::timing", phase="agent_prepare", elapsed_ms=phase.elapsed().as_secs_f64()*1000.);
        let mut prefetched_meaning = prepared
            .as_mut()
            .and_then(|prepared| prepared.meaning.take());
        let indexed_position = prepared.as_ref().map(|prepared| prepared.position.clone());
        let phase = Instant::now();
        let located = if let Some(prepared) = &prepared {
            let selected = orch_db::queries::search::CandidateSet {
                documents: prepared.documents.clone(),
                nodes: prepared.conversations.iter().copied().collect(),
                chats: prepared.chats.iter().copied().collect(),
                ..Default::default()
            };
            self.locate_shared_search(&mut conn, thread.node_id, asked, &selected)
                .await?
        } else {
            self.locate_shared(&mut conn, thread.node_id, asked, false, false)
                .await?
        };
        tracing::debug!(target: "orch::search::timing", phase="agent_tree", elapsed_ms=phase.elapsed().as_secs_f64()*1000.);
        let crate::tools::shared::LocatedPath {
            asking,
            tree,
            found,
            also_named_so,
            ..
        } = match located {
            Ok(located) => located,
            Err(refusal) => return Ok(refusal.into()),
        };
        let directory = matches!(found.entry, FoundEntry::Directory(_));
        let scope = found.path.clone();
        let inside = |path: &str| {
            if directory {
                path.starts_with(&scope) || path == scope.trim_end_matches('/')
            } else {
                path == scope
            }
        };
        let name_ok = |path: &str| {
            pattern.as_ref().is_none_or(|pattern| {
                pattern.matches(
                    path.trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .unwrap_or(path),
                )
            })
        };
        let fingerprint = Hash::of(&serde_json::to_vec(&json!({
            "thread": thread.node_id, "query": arguments.query, "regex": regex, "scope": scope,
            "entry": format!("{:?}", found.entry), "kinds": arguments.kinds, "names": arguments.names,
            "snippets": snippets, "context": context,
        })).map_err(|error| EngineError::unreadable(error.to_string()))?);
        let cursor: Option<SearchCursor> = match arguments.cursor.as_deref() {
            Some(text) => match SearchCursor::from_token(text) {
                Some(cursor) if cursor.valid_for(&fingerprint) => Some(cursor),
                _ => return Ok(Refusal::invalid("This cursor belongs to another search. Keep the query, path and filters, or restart without cursor.").into()),
            },
            None => None,
        };
        let start = cursor
            .as_ref()
            .map(|cursor| cursor.start.clone())
            .unwrap_or_default();
        let end = cursor.as_ref().and_then(|cursor| cursor.end.clone());
        let before_entry = cursor
            .as_ref()
            .and_then(|cursor| cursor.before_entry.clone());
        let previously_shown = cursor
            .as_ref()
            .map(|cursor| cursor.fused_shown.clone())
            .unwrap_or_default();
        let mut meaning_position = cursor
            .as_ref()
            .and_then(|cursor| cursor.meaning.clone())
            .unwrap_or(SemanticPosition {
                offset: 0,
                done: cursor.as_ref().is_some_and(|cursor| cursor.fused.is_none()),
                generation: None,
                reason: None,
            });
        let mut meaning_work = if prefetched_meaning.is_some()
            || regex
            || arguments.query.contains('"')
            || meaning_position.done
            || (!wanted("documents") && !wanted("conversations") && !wanted("chats"))
        {
            MeaningTask(None)
        } else if let Some(org) =
            orch_db::queries::semantic::org_of(&mut conn, thread.node_id).await?
        {
            let engine = self.clone();
            let query = arguments.query.clone();
            let position = meaning_position.clone();
            MeaningTask(Some(tokio::spawn(async move {
                engine
                    .meaning_page_in_generation(org, &query, position.offset, position.generation)
                    .await
            })))
        } else {
            MeaningTask(None)
        };
        let mut meaning_consumed = 0_i64;
        let mut meaning_more = false;
        let bytes_limit = orch_settings::get::<i64>(
            &mut conn,
            Place::Node(asking.agent.topic),
            keys::SHARING_SNAPSHOT_MAX_BYTES,
        )
        .await?
        .unwrap_or(SNAPSHOT_DEFAULT)
        .max(1);
        let started = Instant::now();
        let mut reached = Reached::default();
        let conversations: Vec<_> = if wanted("conversations") {
            tree.conversations()
                .into_iter()
                .map(|(id, prefix, _)| (id, format!("{prefix}conversation.md")))
                .filter(|(id, path)| {
                    inside(path)
                        && name_ok(path)
                        && prepared
                            .as_ref()
                            .is_none_or(|selected| selected.conversations.contains(id))
                })
                .collect()
        } else {
            Vec::new()
        };
        let chats: Vec<_> = if wanted("chats") {
            tree.chats()
                .into_iter()
                .map(|(id, path, _)| (id, path))
                .filter(|(id, path)| {
                    inside(path)
                        && name_ok(path)
                        && prepared
                            .as_ref()
                            .is_none_or(|selected| selected.chats.contains(id))
                })
                .collect()
        } else {
            Vec::new()
        };
        let documents: Vec<_> = if wanted("documents") || wanted("files") {
            tree.documents()
                .into_iter()
                .filter(|(_, path)| inside(path) && name_ok(path))
                .map(|(doc, path)| {
                    (
                        doc.row.id,
                        path,
                        doc.row.title.clone(),
                        doc.row.description.clone(),
                        doc.row.updated_at,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let owners = [
            conversations.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            chats.iter().map(|(id, _)| *id).collect(),
            if wanted("documents") {
                documents.iter().map(|(id, _, _, _, _)| *id).collect()
            } else {
                Vec::new()
            },
        ];
        let paths: HashMap<Id, String> = conversations
            .iter()
            .chain(&chats)
            .map(|(id, path)| (*id, path.clone()))
            .chain(
                documents
                    .iter()
                    .map(|(id, path, _, _, _)| (*id, path.clone())),
            )
            .collect();
        reached.entries = i64::try_from(paths.len()).unwrap_or(i64::MAX);
        let mut names = Vec::new();
        let name_hit = |owner: Id,
                        path: String,
                        kind: SharedEntryKind,
                        at: chrono::DateTime<chrono::Utc>,
                        text: &str,
                        field: Field|
         -> Option<Hit> {
            if !name_ok(&path) || !matcher.matches(text) {
                return None;
            }
            let (shown, matches) = if snippets {
                snippet(text, &matcher, context).unwrap_or_default()
            } else {
                (String::new(), Vec::new())
            };
            Some(Hit {
                key: SearchKey {
                    at,
                    id: owner,
                    path: path.clone(),
                },
                field,
                owner,
                path,
                kind,
                position: None,
                snippet: shown,
                matches,
            })
        };
        for (id, path, name, kind) in tree.places() {
            let Some(place) = tree.place(id) else {
                continue;
            };
            if !inside(&path) {
                continue;
            }
            // The most telling field that matches stands for the place: its name, else its description.
            if let Some(hit) = name_hit(
                id,
                path.clone(),
                kind,
                place.updated_at,
                &name,
                Field::Title,
            )
            .or_else(|| {
                name_hit(
                    id,
                    path,
                    kind,
                    place.updated_at,
                    &place.description,
                    Field::Description,
                )
            }) {
                names.push(hit);
            }
        }
        if wanted("documents") {
            for (id, path, title, description, at) in &documents {
                if let Some(hit) = name_hit(
                    *id,
                    path.clone(),
                    SharedEntryKind::Document,
                    *at,
                    title,
                    Field::Title,
                )
                .or_else(|| {
                    name_hit(
                        *id,
                        path.clone(),
                        SharedEntryKind::Document,
                        *at,
                        description,
                        Field::Description,
                    )
                }) {
                    names.push(hit);
                }
            }
        }
        if wanted("files") && !documents.is_empty() {
            let ids: Vec<_> = documents.iter().map(|(id, _, _, _, _)| *id).collect();
            let times: HashMap<_, _> = documents
                .iter()
                .map(|(id, _, _, _, at)| (*id, *at))
                .collect();
            for (document, name, size, is_text) in
                rows::document_file_names(&mut conn, &ids).await?
            {
                reached.files += 1;
                let Some(path) = paths.get(&document) else {
                    continue;
                };
                let path = format!("{}.files/{name}", path.trim_end_matches(".md"));
                if let Some(at) = times.get(&document)
                    && let Some(hit) = name_hit(
                        document,
                        path.clone(),
                        SharedEntryKind::File,
                        *at,
                        &name,
                        Field::Title,
                    )
                {
                    names.push(hit);
                }
                if reached.skipped.len() < SKIPPED_MAX {
                    reached.skipped.push(json!({"path":path,"reason":if is_text {"unavailable"} else {"not_text"},"size":size}));
                }
            }
        }
        names.sort_by(|one, other| other.key.cmp(&one.key));
        let mut names: VecDeque<_> = names
            .into_iter()
            .filter(|hit| {
                start.before[3]
                    .as_ref()
                    .is_none_or(|before| hit.key < *before)
            })
            .collect();
        let mut state = start.clone();
        let mut batches: [VecDeque<TextRow>; 3] = std::array::from_fn(|_| VecDeque::new());
        let mut more = [true; 3];
        let mut gathered = Vec::new();
        let phase = Instant::now();
        loop {
            if end.as_ref().is_some_and(|end| state.before == end.before) {
                if let Some(end) = &end {
                    state.done = end.done;
                }
                break;
            }
            for kind in 0..3 {
                if state.done[kind]
                    || end
                        .as_ref()
                        .is_some_and(|end| state.before[kind] == end.before[kind])
                {
                    continue;
                }
                if batches[kind].is_empty() {
                    let selected = prepared.as_ref().map(|selected| match kind {
                        0 => selected.text.items.as_slice(),
                        1 => selected.text.messages.as_slice(),
                        _ => selected.text.blocks.as_slice(),
                    });
                    if !more[kind]
                        || owners[kind].is_empty()
                        || selected.is_some_and(<[Id]>::is_empty)
                    {
                        state.done[kind] = true;
                        continue;
                    }
                    let before = state.before[kind].as_ref().map(|key| (key.at, key.id));
                    let batch = match kind {
                        0 => {
                            rows::search_items(
                                &mut conn,
                                &owners[kind],
                                &patterns,
                                before,
                                STREAM_BATCH_ROWS,
                                selected,
                            )
                            .await?
                        }
                        1 => {
                            rows::search_messages(
                                &mut conn,
                                &owners[kind],
                                &patterns,
                                before,
                                STREAM_BATCH_ROWS,
                                selected,
                            )
                            .await?
                        }
                        _ => {
                            rows::search_blocks(
                                &mut conn,
                                &owners[kind],
                                &patterns,
                                before,
                                STREAM_BATCH_ROWS,
                                selected,
                            )
                            .await?
                        }
                    };
                    more[kind] = i64::try_from(batch.len()).unwrap_or(0) == STREAM_BATCH_ROWS;
                    batches[kind] = batch.into();
                    if batches[kind].is_empty() {
                        state.done[kind] = true;
                    }
                }
            }
            if names.is_empty() {
                state.done[3] = true;
            }
            // A removed/unreadable boundary row still bounds the window. Never require seeing it again,
            // and never consume an older row merely because that exact boundary disappeared.
            if let Some(end) = &end {
                for (source, head) in batches
                    .iter()
                    .map(|batch| {
                        batch.front().map(|row| SearchKey {
                            at: row.at,
                            id: row.id,
                            path: String::new(),
                        })
                    })
                    .chain(std::iter::once(names.front().map(|hit| hit.key.clone())))
                    .enumerate()
                {
                    if state.before[source] == end.before[source] {
                        continue;
                    }
                    if let Some(bound) = &end.before[source]
                        && (state.done[source] || head.as_ref().is_some_and(|head| head < bound))
                    {
                        state.before[source] = Some(bound.clone());
                        state.done[source] = end.done[source];
                    }
                }
                if state.before == end.before {
                    state.done = end.done;
                    break;
                }
            }
            let mut heads: Vec<(SearchKey, usize)> = (0..3)
                .filter_map(|kind| {
                    if state.done[kind]
                        || end
                            .as_ref()
                            .is_some_and(|end| state.before[kind] == end.before[kind])
                    {
                        return None;
                    }
                    batches[kind].front().map(|row| {
                        (
                            SearchKey {
                                at: row.at,
                                id: row.id,
                                path: String::new(),
                            },
                            kind,
                        )
                    })
                })
                .collect();
            if !state.done[3]
                && !end
                    .as_ref()
                    .is_some_and(|end| state.before[3] == end.before[3])
                && let Some(hit) = names.front()
            {
                heads.push((hit.key.clone(), 3));
            }
            let Some((key, source)) = heads.into_iter().max() else {
                break;
            };
            state.before[source] = Some(key.clone());
            if source == 3 {
                if let Some(hit) = names.pop_front() {
                    reached.bytes = reached
                        .bytes
                        .saturating_add(i64::try_from(hit.path.len()).unwrap_or(i64::MAX));
                    gathered.push(hit);
                }
                if names.is_empty() {
                    state.done[3] = true;
                }
            } else if let Some(row) = batches[source].pop_front() {
                reached.bytes = reached
                    .bytes
                    .saturating_add(i64::try_from(row.text.len()).unwrap_or(i64::MAX));
                if matcher.matches(&row.text)
                    && let Some(path) = paths.get(&row.owner)
                {
                    let (shown, matches) = if snippets {
                        snippet(&row.text, &matcher, context).unwrap_or_default()
                    } else {
                        (String::new(), Vec::new())
                    };
                    gathered.push(Hit {
                        key,
                        field: if row.heading {
                            Field::Heading
                        } else {
                            Field::Body
                        },
                        owner: row.owner,
                        path: path.clone(),
                        kind: match source {
                            0 => SharedEntryKind::Conversation,
                            1 => SharedEntryKind::Chat,
                            _ => SharedEntryKind::Document,
                        },
                        position: row.position,
                        snippet: shown,
                        matches,
                    });
                }
                if batches[source].is_empty() && !more[source] {
                    state.done[source] = true;
                }
            }
            if reached.bytes >= bytes_limit || started.elapsed() >= TIME_LIMIT {
                if end.is_some() && !end.as_ref().is_some_and(|end| state.before == end.before) {
                    return Ok(Refusal::new(Code::Limit, "This search page could not be reread within its bound. Restart without cursor and narrow path, kinds or names.").into());
                }
                break;
            }
        }
        tracing::debug!(target: "orch::search::timing", phase="agent_rows", elapsed_ms=phase.elapsed().as_secs_f64()*1000., rows=gathered.len());
        // Native chat positions are counted only for the few positions each printed entry can show.
        let mut chat_parts: HashMap<Id, BTreeMap<i64, Id>> = HashMap::new();
        for hit in &gathered {
            if hit.kind == SharedEntryKind::Chat
                && let Some(sequence) = hit.position
            {
                chat_parts
                    .entry(hit.owner)
                    .or_default()
                    .insert(sequence, hit.key.id);
            }
        }
        let chat_ids: Vec<_> = chat_parts
            .values()
            .flat_map(|parts| parts.values().take(FIRST_POSITIONS).copied())
            .collect();
        let numbers = rows::search_chat_numbers(&mut conn, &chat_ids).await?;
        let doc_ids: BTreeSet<_> = gathered
            .iter()
            .filter(|hit| hit.kind == SharedEntryKind::Document && hit.key.path.is_empty())
            .map(|hit| hit.owner)
            .collect();
        let mut blocks: HashMap<Id, (Option<Id>, i64)> = HashMap::new();
        if !doc_ids.is_empty() {
            for (_, block, parent, position) in
                rows::block_places(&mut conn, &doc_ids.into_iter().collect::<Vec<_>>()).await?
            {
                blocks.insert(block, (parent, position));
            }
        }
        let top_of = |mut block: Id| -> Option<i64> {
            for _ in 0..=orch_documents::blocks::nesting_limit() {
                let (parent, number) = blocks.get(&block)?;
                match parent {
                    None => return Some(*number),
                    Some(parent) => block = *parent,
                }
            }
            None
        };
        let mut grouped: HashMap<String, Summary> = HashMap::new();
        for mut hit in gathered {
            if hit.kind == SharedEntryKind::Chat {
                hit.position = numbers.get(&hit.key.id).copied();
            }
            if hit.kind == SharedEntryKind::Document && hit.key.path.is_empty() {
                hit.position = top_of(hit.key.id);
            }
            let key = SearchKey {
                at: hit.key.at,
                id: hit.key.id,
                path: hit.path.clone(),
            };
            let entry = grouped.entry(hit.path.clone()).or_insert_with(|| Summary {
                key: key.clone(),
                by_meaning: false,
                field: hit.field,
                path: hit.path.clone(),
                kind: hit.kind,
                owner: hit.owner,
                count: 0,
                positions: BTreeSet::new(),
                snippets: Vec::new(),
            });
            entry.count = entry.count.saturating_add(1);
            entry.key = entry.key.clone().max(key);
            entry.field = entry.field.max(hit.field);
            if let Some(position) = hit.position {
                entry.positions.insert(position);
            }
            if snippets
                && !hit.matches.is_empty()
                && (hit.kind != SharedEntryKind::Chat || hit.position.is_some())
            {
                entry.snippets.push(
                    json!({"position":hit.position,"snippet":hit.snippet,"matches":hit.matches}),
                );
            }
        }
        let counts_partial = indexed_position
            .as_ref()
            .is_some_and(|position| position.next.is_some())
            || start != Scan::default()
            || !state.done.iter().all(|done| *done);
        let (meaning, meaning_state) = if prefetched_meaning.is_some() || meaning_work.0.is_some() {
            let answer = if let Some(answer) = prefetched_meaning.take() {
                answer
            } else {
                let work = meaning_work
                    .0
                    .as_mut()
                    .ok_or_else(|| EngineError::unreadable("meaning work vanished"))?;
                match work.await {
                    Ok(Ok(answer)) => answer,
                    Ok(Err(error)) => {
                        tracing::warn!(%error, "optional meaning retrieval failed");
                        crate::semantic::query::Meaning::Unavailable("error".into())
                    }
                    Err(error) => {
                        tracing::warn!(%error, "optional meaning retrieval stopped");
                        crate::semantic::query::Meaning::Unavailable("error".into())
                    }
                }
            };
            meaning_work.0 = None;
            match answer {
                crate::semantic::query::Meaning::Found {
                    near,
                    fusion_k,
                    candidates,
                    generation_id,
                    paused,
                } => {
                    meaning_position.generation = Some(generation_id);
                    meaning_consumed = i64::try_from(near.len()).unwrap_or(i64::MAX);
                    meaning_more = meaning_consumed >= candidates;
                    let state = paused.map_or_else(
                        || json!({"state":"done"}),
                        |reason| json!({"state":"done","reason":reason}),
                    );
                    (Some((near, fusion_k)), state)
                }
                crate::semantic::query::Meaning::Unavailable(reason) => {
                    meaning_position.done = true;
                    meaning_position.reason = Some(reason.clone());
                    (None, json!({"state":"unavailable", "reason":reason}))
                }
                crate::semantic::query::Meaning::Off => {
                    meaning_position.done = true;
                    (None, json!({"state":"none", "reason":"off"}))
                }
            }
        } else {
            (
                None,
                if let Some(reason) = &meaning_position.reason {
                    json!({"state":"unavailable","reason":reason})
                } else {
                    json!({"state":"none", "reason":if regex {"regex"} else if arguments.query.contains('"') {"phrase"} else {"continuation"}})
                },
            )
        };
        let fused_offset = cursor.as_ref().and_then(|cursor| cursor.fused).unwrap_or(0);
        let mut hits: Vec<_> = grouped
            .into_values()
            .filter(|hit| {
                meaning.is_some()
                    || before_entry
                        .as_ref()
                        .is_none_or(|before| hit.rank() < *before)
            })
            .collect();
        hits.sort_by_cached_key(|hit| std::cmp::Reverse(hit.rank()));
        if let Some((near, fusion_k)) = &meaning {
            let words: Vec<String> = hits.iter().map(|hit| hit.path.clone()).collect();
            let mut by_meaning: Vec<String> = Vec::new();
            for found in near {
                if !match found.kind.as_str() {
                    "document" => wanted("documents"),
                    "conversation" => wanted("conversations"),
                    "chat" => wanted("chats"),
                    _ => false,
                } {
                    continue;
                }
                let Some(found_path) = paths.get(&found.source_id) else {
                    continue;
                };
                if by_meaning.contains(found_path) {
                    continue;
                }
                by_meaning.push(found_path.clone());
                if let Some(hit) = hits.iter_mut().find(|hit| hit.path == *found_path) {
                    hit.by_meaning = true;
                    continue;
                }
                let kind = match found.kind.as_str() {
                    "conversation" => SharedEntryKind::Conversation,
                    "chat" => SharedEntryKind::Chat,
                    _ => SharedEntryKind::Document,
                };
                let at = documents
                    .iter()
                    .find(|(id, _, _, _, _)| *id == found.source_id)
                    .map(|(_, _, _, _, at)| *at)
                    .or_else(|| tree.place(found.source_id).map(|place| place.updated_at))
                    .unwrap_or_default();
                hits.push(Summary {
                    key: SearchKey {
                        at,
                        id: found.source_id,
                        path: found_path.clone(),
                    },
                    by_meaning: true,
                    field: Field::Body,
                    path: found_path.clone(),
                    kind,
                    owner: found.source_id,
                    count: 0,
                    positions: BTreeSet::new(),
                    snippets: Vec::new(),
                });
            }
            let order: Vec<String> = crate::semantic::fusion::fuse(&[words, by_meaning], *fusion_k)
                .into_iter()
                .map(|(found_path, _)| found_path)
                .collect();
            hits.sort_by_key(|hit| {
                order
                    .iter()
                    .position(|found_path| *found_path == hit.path)
                    .unwrap_or(usize::MAX)
            });
            if previously_shown.is_empty() {
                hits = hits.split_off(fused_offset.min(hits.len()));
            }
        }
        hits.retain(|hit| !previously_shown.contains(&hit.path));
        let more_entries = hits.len() > limit;
        hits.truncate(limit);
        let page_paths: Vec<String> = hits.iter().map(|hit| hit.path.clone()).collect();
        let mut seen_after = previously_shown.clone();
        if indexed_root || meaning.is_some() || !previously_shown.is_empty() {
            seen_after.extend(page_paths.iter().cloned());
            seen_after.sort_unstable();
            seen_after.dedup();
        }
        if !more_entries && !meaning_more {
            meaning_position.done = true;
        }
        let next = if more_entries && meaning.is_some() {
            Some(SearchCursor {
                fingerprint,
                meaning: Some(meaning_position.clone()),
                indexed: indexed_position.clone(),
                start: start.clone(),
                end: Some(state.clone()),
                before_entry: None,
                fused: Some(fused_offset + limit),
                fused_shown: seen_after.clone(),
            })
        } else if more_entries {
            hits.last().map(|hit| SearchCursor {
                fingerprint,
                meaning: Some(meaning_position.clone()),
                indexed: indexed_position.clone(),
                start: start.clone(),
                end: Some(state.clone()),
                before_entry: Some(hit.rank()),
                fused: None,
                fused_shown: seen_after.clone(),
            })
        } else if meaning_more && !meaning_position.done {
            let mut continued = meaning_position.clone();
            continued.offset = continued.offset.saturating_add(meaning_consumed);
            Some(SearchCursor {
                fingerprint,
                meaning: Some(continued),
                indexed: indexed_position.as_ref().map(|position| indexed::Position {
                    start: position
                        .next
                        .clone()
                        .unwrap_or_else(|| position.start.clone()),
                    next: None,
                    done: position.next.is_none(),
                }),
                start: Scan::default(),
                end: None,
                before_entry: None,
                fused: Some(0),
                fused_shown: seen_after.clone(),
            })
        } else if !state.done.iter().all(|done| *done) {
            Some(SearchCursor {
                fingerprint,
                meaning: Some(meaning_position.clone()),
                indexed: indexed_position.clone(),
                start: state.clone(),
                end: None,
                before_entry: None,
                fused: None,
                fused_shown: seen_after.clone(),
            })
        } else if let Some(position) = indexed_position
            .as_ref()
            .filter(|position| position.next.is_some())
        {
            Some(SearchCursor {
                fingerprint,
                meaning: Some(meaning_position.clone()),
                indexed: Some(indexed::Position {
                    start: position
                        .next
                        .clone()
                        .ok_or_else(|| EngineError::unreadable("indexed continuation vanished"))?,
                    next: None,
                    done: false,
                }),
                start: Scan::default(),
                end: None,
                before_entry: None,
                fused: None,
                fused_shown: seen_after.clone(),
            })
        } else {
            None
        };
        let next_cursor = next
            .as_ref()
            .map(SearchCursor::token)
            .transpose()
            .map_err(|error| EngineError::unreadable(error.to_string()))?;
        let partial = next_cursor.is_some();
        self.audit_search(&mut conn, &asking, &arguments.query, &scope, hits.len())
            .await?;
        let joined = meaning.is_some();
        let hits: Vec<_> = hits
            .into_iter()
            .enumerate()
            .map(|(shown, mut hit)| {
                let row_cursor = SearchCursor {
                    fingerprint,
                    meaning: Some(if joined {
                        SemanticPosition {
                            done: false,
                            ..meaning_position.clone()
                        }
                    } else {
                        meaning_position.clone()
                    }),
                    indexed: indexed_position.clone(),
                    start: start.clone(),
                    end: Some(state.clone()),
                    before_entry: (!joined).then(|| hit.rank()),
                    fused: joined.then_some(fused_offset + shown + 1),
                    fused_shown: if joined {
                        let mut seen = previously_shown.clone();
                        seen.extend(page_paths.iter().take(shown + 1).cloned());
                        seen.sort_unstable();
                        seen.dedup();
                        seen
                    } else {
                        previously_shown.clone()
                    },
                }
                .token()
                .map_err(|error| EngineError::unreadable(error.to_string()))?;
                let positions: Vec<_> = hit.positions.into_iter().take(FIRST_POSITIONS).collect();
                let mut value = json!({"path":hit.path,"kind":hit.kind,"updated_at":hit.key.at,
                "match_count":hit.count,"match_count_partial":counts_partial,"positions":positions,
                "position":positions.first().copied(),"cursor":row_cursor});
                if hit.kind != SharedEntryKind::File {
                    value["id"] = json!(hit.owner);
                }
                if joined {
                    value["found_by"] = match (hit.count > 0, hit.by_meaning) {
                        (true, true) => json!(["words", "meaning"]),
                        (false, true) => json!(["meaning"]),
                        _ => json!(["words"]),
                    };
                }
                if snippets {
                    hit.snippets
                        .sort_by_key(|sample| sample["position"].as_i64());
                    hit.snippets.truncate(FIRST_POSITIONS);
                    if let Some(first) = hit.snippets.first() {
                        value["snippet"] = first["snippet"].clone();
                        value["matches"] = first["matches"].clone();
                    }
                    value["snippets"] = json!(hit.snippets);
                }
                Ok(value)
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        let mut answer = json!({"hits":hits,"truncated":partial,"partial":partial,"next_cursor":next_cursor,
            "counts_complete":!counts_partial,"searched":{"entries":reached.entries,"files":reached.files,"bytes":reached.bytes},"skipped":reached.skipped});
        answer["meaning"] = meaning_state;
        if let Some(notice) = also_named_so {
            answer["also_named_so"] = json!(notice);
        }
        Ok(Answer::Done(answer))
    }

    /// Writes down that an agent searched, with what it looked for and where.
    async fn audit_search(
        &self,
        conn: &mut Conn,
        asking: &crate::tools::shared::Asking,
        query: &str,
        scope: &str,
        hits: usize,
    ) -> Result<(), EngineError> {
        audit_by_agent(
            conn,
            asking,
            orch_db::audit::action!(AuditAction::ShareSearchedByAgent),
            "thread",
            asking.agent.thread,
            Some(asking.agent.thread),
            "An agent searched what was shared with it.",
            json!({ "query": query, "path": scope, "hits": hits }),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_is_narrowed_by_the_longest_run_that_every_match_must_hold() {
        assert_eq!(
            literal_of("login failure"),
            Some("login failure".to_owned())
        );
        assert_eq!(
            literal_of("(?i)session.*cookie"),
            Some("session".to_owned())
        );
        assert_eq!(literal_of("colou?r"), Some("colo".to_owned()));
        assert_eq!(
            literal_of("a.c"),
            None,
            "no run of three characters to narrow by"
        );
        assert_eq!(literal_of("foo|bar"), None);
        assert_eq!(literal_of("a[bc]d"), None);
        assert_eq!(literal_of("^abc\\d+$"), Some("abc".to_owned()));
        assert_eq!(
            literal_of("(optional)?mandatory"),
            Some("mandatory".to_owned())
        );
    }

    #[test]
    fn a_snippet_carries_the_lines_around_the_first_match_and_says_where_the_matches_are() {
        let matcher = Matcher::plain("cookie").unwrap();
        let text = "one\ntwo\nthe cookie is dropped\nfour\nfive";
        let (shown, matches) = snippet(text, &matcher, 1).unwrap();
        assert_eq!(shown, "two\nthe cookie is dropped\nfour");
        assert_eq!(matches, vec![(8, 14)]);
        let (alone, _) = snippet(text, &matcher, 0).unwrap();
        assert_eq!(alone, "the cookie is dropped");
        assert!(snippet("nothing here", &matcher, 1).is_none());
    }

    #[test]
    fn a_long_line_is_cut_around_the_match() {
        let matcher = Matcher::plain("needle").unwrap();
        let text = format!("{} needle {}", "a".repeat(2_000), "b".repeat(2_000));
        let (shown, matches) = snippet(&text, &matcher, 0).unwrap();
        assert!(shown.chars().count() <= SNIPPET_CHARS);
        assert_eq!(matches.len(), 1);
        let chars: Vec<char> = shown.chars().collect();
        let found: String = chars[matches[0].0..matches[0].1].iter().collect();
        assert_eq!(found, "needle");
    }

    #[test]
    fn a_word_is_found_whatever_its_case_and_a_pattern_unless_it_says_otherwise() {
        let plain = Matcher::plain("a.b").unwrap();
        assert_eq!(
            plain.find("A.B and axb").len(),
            1,
            "a dot in a word is a dot"
        );
        let pattern = Matcher::expression("(?-i)Cookie", true).unwrap();
        assert_eq!(pattern.find("Cookie cookie").len(), 1);
        assert!(
            Matcher::expression("(?=x)y", true).is_err(),
            "no look-around"
        );
        assert!(
            Matcher::expression("(a)\\1", true).is_err(),
            "no back-references"
        );
    }
}
