//! One cancellable stream of search parts, source progress and authoritative reciprocal-rank orders.
//!
//! Retrieval futures belong to the response body. Dropping it cancels their SQL and any pending embedding
//! command; no detached retrieval can outlive a reader. Continuations retain source positions and seen part
//! identities, never payloads or permissions, and every resumed source authorizes its new batch again.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::nodes::invalid;
use axum::{body::Body, response::Response};
use bytes::Bytes;
use orch_api_contract::search::{
    SearchFound, SearchFoundBy, SearchHit, SearchListQuery, SearchMatchField, SearchOrderEntry,
    SearchSort, SearchStreamEvent as Event, SearchStreamListQuery,
};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::Id;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// The published reciprocal-rank constant, also the indexer's shipped setting.
pub(super) const DEFAULT_FUSION_K: u32 = 60;

#[derive(serde::Serialize, serde::Deserialize)]
struct Cursor {
    session: Id,
    revision: i64,
    generation: String,
    selection: SearchListQuery,
    user: Id,
    words: Option<String>,
    words_done: bool,
    meaning_offset: i64,
    #[serde(default)]
    meaning_positions: BTreeMap<Id, super::meaning::Position>,
    meaning_done: bool,
    /// The parts already delivered, sorted, so that each is looked up by halving.
    seen: Vec<String>,
    #[serde(default)]
    unavailable: [Option<String>; 2],
}

fn selection(query: &SearchListQuery) -> SearchListQuery {
    SearchListQuery {
        cursor: None,
        limit: None,
        immediate: None,
        typing_age_ms: None,
        ..query.clone()
    }
}

/// Stable identity of the part, independent of the source that found it.
pub(super) fn identity(hit: &SearchHit) -> String {
    match &hit.found {
        SearchFound::Name { node, .. } => format!("name:{}", node.id),
        SearchFound::Item { item, .. } => format!("item:{}", item.id),
        SearchFound::ChatMessage { message, .. } => format!("chat:{}", message.id),
        SearchFound::Document {
            document, block_id, ..
        } => format!(
            "doc:{}:{}",
            document.id,
            block_id.map_or_else(|| "title".to_owned(), |id| id.to_string())
        ),
    }
}

struct Batch {
    source: usize,
    hits: Vec<SearchHit>,
    ranks: Vec<Vec<String>>,
    reason: Option<String>,
    paused: Option<String>,
    next: Option<String>,
    more: bool,
    meaning_positions: BTreeMap<Id, super::meaning::Position>,
    fusion_k: u32,
}
struct Streaming {
    pending: tokio::task::JoinSet<(usize, ApiResult<Batch>)>,
    tasks: HashMap<tokio::task::Id, usize>,
    events: VecDeque<Event>,
    cursor: Cursor,
    lists: Vec<Vec<String>>,
    sent: HashSet<String>,
    fields: HashMap<String, SearchMatchField>,
    fusion_k: u32,
    finished: bool,
    started: bool,
    ctx: Ctx,
    ttl_secs: i64,
    cursor_entries: i64,
}

impl Streaming {
    async fn next(&mut self) -> ApiResult<Option<Event>> {
        if !self.started {
            if let Some(event) = self.events.pop_front() {
                return Ok(Some(event));
            }
            self.started = true;
        }
        let completed = if self.events.is_empty() {
            self.pending.join_next().await
        } else {
            self.pending.try_join_next()
        };
        if let Some(answer) = completed {
            let (source, answer) = match answer {
                Ok(answer) => answer,
                Err(error) => (
                    self.tasks.remove(&error.id()).unwrap_or(1),
                    Err(ApiError::internal(error)),
                ),
            };
            let previous = std::mem::take(&mut self.events);
            let name = if source == 0 { "words" } else { "meaning" };
            match answer {
                Err(error) => {
                    tracing::warn!(%error, source=name, "search source failed");
                    self.cursor.unavailable[source] = Some("error".into());
                    if source == 0 {
                        self.cursor.words_done = true;
                    } else {
                        self.cursor.meaning_done = true;
                    }
                    self.events.push_back(Event::Source {
                        source: name.into(),
                        state: "unavailable".into(),
                        reason: Some("error".into()),
                        received: 0,
                    });
                }
                Ok(batch) => {
                    self.cursor.unavailable[source] = batch.reason.clone();
                    if batch.source == 1 {
                        self.fusion_k = batch.fusion_k;
                    }
                    if batch.source == 0 {
                        self.cursor.words = batch.next;
                        self.cursor.words_done = !batch.more;
                    } else {
                        self.cursor.meaning_positions = batch.meaning_positions;
                        self.cursor.meaning_offset = 0;
                        self.cursor.meaning_done = !batch.more;
                    }
                    if source == 1 {
                        self.lists.truncate(1);
                        self.lists.extend(batch.ranks.into_iter().map(|list| {
                            list.into_iter()
                                .filter(|id| self.cursor.seen.binary_search(id).is_err())
                                .collect()
                        }));
                    }
                    for hit in batch.hits {
                        let id = identity(&hit);
                        if self.cursor.seen.binary_search(&id).is_ok() {
                            continue;
                        }
                        if let Some(field) = hit.match_field {
                            if source == 0 {
                                self.fields.insert(id.clone(), field);
                            } else {
                                self.fields.entry(id.clone()).or_insert(field);
                            }
                        }
                        if source == 0 && !self.lists[0].contains(&id) {
                            self.lists[0].push(id.clone());
                        }
                        if self.sent.insert(id.clone()) {
                            self.events.push_back(Event::Hit {
                                id,
                                hit: Box::new(hit),
                                found_by: if source == 0 {
                                    SearchFoundBy::Words
                                } else {
                                    SearchFoundBy::Meaning
                                },
                            });
                        }
                    }
                    let items = orch_engine::semantic::fusion::fuse(&self.lists, self.fusion_k)
                        .into_iter()
                        .map(|(id, rank)| {
                            let found_by = match (
                                self.lists[0].contains(&id),
                                self.lists[1..].iter().any(|list| list.contains(&id)),
                            ) {
                                (true, true) => SearchFoundBy::Both,
                                (true, false) => SearchFoundBy::Words,
                                _ => SearchFoundBy::Meaning,
                            };
                            let match_field = self.fields.get(&id).copied();
                            SearchOrderEntry {
                                id,
                                rank,
                                found_by,
                                match_field,
                            }
                        })
                        .collect();
                    self.events.push_back(Event::Order {
                        items,
                        received: i64::try_from(self.sent.len()).unwrap_or(i64::MAX),
                    });
                    self.events.push_back(Event::Source {
                        source: name.into(),
                        state: if batch.reason.is_some() {
                            "unavailable"
                        } else {
                            "done"
                        }
                        .into(),
                        reason: batch.reason.or(batch.paused),
                        received: i64::try_from(if source == 0 {
                            self.lists[0].len()
                        } else {
                            self.lists[1..]
                                .iter()
                                .flatten()
                                .collect::<HashSet<_>>()
                                .len()
                        })
                        .unwrap_or(i64::MAX),
                    });
                }
            }
            let has_order = self
                .events
                .iter()
                .position(|event| matches!(event, Event::Order { .. }));
            if let Some(index) = has_order
                && let Some(order) = self.events.remove(index)
            {
                self.events.push_front(order);
            }
            self.events.extend(
                previous
                    .into_iter()
                    .filter(|event| has_order.is_none() || !matches!(event, Event::Order { .. })),
            );
            return Ok(self.events.pop_front());
        }
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        if self.finished {
            return Ok(None);
        }
        self.finished = true;
        self.cursor.seen.extend(self.sent.iter().cloned());
        self.cursor.seen.sort_unstable();
        self.cursor.seen.dedup();
        let cursor = if self.cursor.words_done && self.cursor.meaning_done {
            if self.cursor.revision > 0 {
                let consumed = sqlx::query(
                    "DELETE FROM search_sessions WHERE id=$1 AND user_id=$2 AND revision=$3",
                )
                .bind(self.cursor.session)
                .bind(self.cursor.user)
                .bind(self.cursor.revision)
                .execute(self.ctx.app.db.pool())
                .await
                .map_err(orch_db::DbError::from)?;
                if consumed.rows_affected() != 1 {
                    return Err(invalid(
                        "cursor",
                        "was already continued; use its newest continuation",
                    ));
                }
            }
            None
        } else {
            let old = self.cursor.revision;
            self.cursor.revision += 1;
            let now = self.ctx.app.clock.now();
            let state = serde_json::to_value(&self.cursor).map_err(ApiError::internal)?;
            let changed = sqlx::query("INSERT INTO search_sessions (id,user_id,revision,state,expires_at) VALUES ($1,$2,$3,$4,$5)
              ON CONFLICT(id) DO UPDATE SET revision=EXCLUDED.revision,state=EXCLUDED.state,expires_at=EXCLUDED.expires_at
              WHERE search_sessions.user_id=EXCLUDED.user_id AND search_sessions.revision=$6")
              .bind(self.cursor.session).bind(self.cursor.user).bind(self.cursor.revision).bind(state)
              .bind(now + chrono::Duration::seconds(self.ttl_secs)).bind(old)
              .execute(self.ctx.app.db.pool()).await.map_err(orch_db::DbError::from)?;
            if changed.rows_affected() != 1 {
                return Err(invalid(
                    "cursor",
                    "was already continued; use its newest continuation",
                ));
            }
            // Bound cleanup to this reader's configured allowance; expiration uses the same small indexed cut.
            sqlx::query("DELETE FROM search_sessions WHERE id IN (
                SELECT id FROM search_sessions WHERE expires_at<=$1 ORDER BY expires_at,id LIMIT $3)
                OR id IN (SELECT id FROM search_sessions WHERE user_id=$2 ORDER BY expires_at DESC,id OFFSET $3)")
                .bind(now).bind(self.cursor.user).bind(self.cursor_entries)
                .execute(self.ctx.app.db.pool())
                .await
                .map_err(orch_db::DbError::from)?;
            Some(format!("{}:{}", self.cursor.session, self.cursor.revision))
        };
        Ok(Some(Event::Cursor { cursor }))
    }
}

pub(super) async fn search(
    ctx: Ctx,
    query: SearchStreamListQuery,
    _body: Body,
) -> ApiResult<Response> {
    let user = ctx.user()?.id();
    let select = selection(&query);
    if select.q.trim().is_empty() {
        return Err(invalid("q", "give a word or phrase"));
    }
    if select.regex == Some(true) {
        orch_engine::tools::search::search_expression(&select.q)
            .map_err(|reason| invalid("q", &reason))?;
        if select.continuation_id.is_some() || select.projection_revision.is_some() {
            return Err(invalid(
                "regex",
                "select the active history for an expression",
            ));
        }
    }
    let mut conn = ctx.app.db.acquire().await?;
    let generation: String = sqlx::query_scalar("SELECT COALESCE(string_agg(id::text||':'||COALESCE(dimensions,0)::text,',' ORDER BY id),'') FROM search_generations WHERE state='active'")
        .fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let generation = orch_types::Hash::of(generation.as_bytes()).to_string();
    let mut cursor = if let Some(raw) = &query.cursor {
        let bad = || invalid("cursor", "is expired or belongs to another search");
        let (id, revision) = raw.split_once(':').ok_or_else(bad)?;
        let id = id.parse::<Id>().map_err(|_| bad())?;
        let revision = revision.parse::<i64>().map_err(|_| bad())?;
        let state: Option<orch_types::Json> = sqlx::query_scalar("SELECT state FROM search_sessions WHERE id=$1 AND user_id=$2 AND revision=$3 AND expires_at>$4")
            .bind(id).bind(user).bind(revision).bind(ctx.now).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
        let cursor: Cursor = serde_json::from_value(state.ok_or_else(bad)?).map_err(|_| bad())?;
        if cursor.selection != select || cursor.user != user {
            return Err(bad());
        }
        cursor
    } else {
        Cursor {
            session: orch_types::new_id(),
            revision: 0,
            generation: generation.clone(),
            selection: select.clone(),
            user,
            words: None,
            words_done: false,
            meaning_offset: 0,
            meaning_positions: BTreeMap::new(),
            meaning_done: false,
            seen: Vec::new(),
            unavailable: [None, None],
        }
    };
    let place = select.scope.map_or(Place::System, Place::Node);
    let mut options = HashMap::new();
    for key in [
        keys::SEARCH_PAGE_SIZE,
        keys::SEARCH_REORDER_WINDOW_SECS,
        keys::SEARCH_MEANING_PAUSE_MS,
        keys::SEARCH_CURSOR_TTL_SECS,
        keys::SEARCH_CURSOR_ENTRIES,
    ] {
        options.insert(
            key,
            get::<i64>(&mut conn, place, key)
                .await
                .map_err(ApiError::internal)?
                .unwrap_or_default(),
        );
    }
    drop(conn);
    let number = |key| u32::try_from(options[&key]).unwrap_or_default();
    let page_size = number(keys::SEARCH_PAGE_SIZE);
    let pause = number(keys::SEARCH_MEANING_PAUSE_MS);
    let mut events = VecDeque::from([Event::Options {
        page_size,
        reorder_window_secs: number(keys::SEARCH_REORDER_WINDOW_SECS),
        meaning_pause_ms: pause,
    }]);
    let mut pending = tokio::task::JoinSet::new();
    let mut tasks = HashMap::new();
    if !cursor.meaning_done && cursor.generation != generation {
        cursor.meaning_done = true;
        cursor.unavailable[1] = Some("model_changed".into());
    }
    for (index, name, done) in [
        (0, "words", cursor.words_done),
        (1, "meaning", cursor.meaning_done),
    ] {
        if done {
            let paused = if index == 1 {
                cursor
                    .meaning_positions
                    .values()
                    .find_map(|position| position.paused.clone())
            } else {
                None
            };
            events.push_back(Event::Source {
                source: name.into(),
                state: if cursor.unavailable[index].is_some() {
                    "unavailable"
                } else {
                    "done"
                }
                .into(),
                reason: cursor.unavailable[index].clone().or(paused),
                received: 0,
            });
        }
    }
    if !cursor.words_done {
        let ctx = ctx.clone();
        let mut words = select.clone();
        words.cursor = cursor.words.clone();
        words.limit = Some(page_size);
        events.push_back(Event::Source {
            source: "words".into(),
            state: "running".into(),
            reason: None,
            received: 0,
        });
        let shown = cursor.seen.clone();
        let task = pending.spawn(async move {
            (
                0,
                super::words::retrieve(ctx, words, shown)
                    .await
                    .map(|page| Batch {
                        source: 0,
                        hits: page.items,
                        ranks: Vec::new(),
                        reason: None,
                        paused: None,
                        more: page.next_cursor.is_some(),
                        next: page.next_cursor,
                        meaning_positions: BTreeMap::new(),
                        fusion_k: DEFAULT_FUSION_K,
                    }),
            )
        });
        tasks.insert(task.id(), 0);
    }
    let reason = if select.title_only == Some(true) {
        Some("title_only")
    } else if select.sort.unwrap_or_default() == SearchSort::Newest {
        Some("newest")
    } else if select.regex == Some(true) {
        Some("regex")
    } else if select.q.contains('"') {
        Some("phrase")
    } else if select.continuation_id.is_some() {
        Some("history")
    } else {
        None
    };
    if let Some(reason) = reason {
        cursor.meaning_done = true;
        events.push_back(Event::Source {
            source: "meaning".into(),
            state: "done".into(),
            reason: Some(reason.into()),
            received: 0,
        });
    } else if !cursor.meaning_done {
        let ctx = ctx.clone();
        let offset = cursor.meaning_offset;
        let positions = cursor.meaning_positions.clone();
        let meaning = select.clone();
        let immediate = query.immediate.unwrap_or(false);
        let delay = pause.saturating_sub(query.typing_age_ms.unwrap_or(0));
        events.push_back(Event::Source {
            source: "meaning".into(),
            state: "running".into(),
            reason: None,
            received: 0,
        });
        let task = pending.spawn(async move {
            if !immediate {
                tokio::time::sleep(std::time::Duration::from_millis(u64::from(delay))).await;
            }
            (
                1,
                super::meaning::retrieve(ctx, meaning, positions, offset)
                    .await
                    .map(|batch| Batch {
                        source: 1,
                        hits: batch.hits,
                        ranks: batch.ranks,
                        reason: batch.unavailable,
                        paused: batch.paused,
                        next: None,
                        more: batch.more,
                        meaning_positions: batch.positions,
                        fusion_k: batch.fusion_k,
                    }),
            )
        });
        tasks.insert(task.id(), 1);
    }
    let state = Streaming {
        pending,
        tasks,
        events,
        cursor,
        lists: vec![Vec::new()],
        sent: HashSet::new(),
        fields: HashMap::new(),
        fusion_k: DEFAULT_FUSION_K,
        finished: false,
        started: false,
        ctx: ctx.clone(),
        ttl_secs: options[keys::SEARCH_CURSOR_TTL_SECS],
        cursor_entries: options[keys::SEARCH_CURSOR_ENTRIES],
    };
    let stream = futures::stream::unfold(state, |mut state| async move {
        match state.next().await {
            Ok(Some(event)) => {
                let bytes = serde_json::to_vec(&event)
                    .map(|mut bytes| {
                        bytes.push(b'\n');
                        Bytes::from(bytes)
                    })
                    .map_err(std::io::Error::other);
                Some((bytes, state))
            }
            Ok(None) => None,
            Err(error) => {
                state.finished = true;
                Some((Err(std::io::Error::other(error.to_string())), state))
            }
        }
    });
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/x-ndjson"),
    );
    response.headers_mut().insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}
