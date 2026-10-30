//! Related documents, chats and threads ranked by current-generation meaning, shared place and typed references.

use crate::{engine::Engine, error::EngineError};
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName};
use orch_api_contract::search::{
    RelatedBy, RelatedItem, RelatedItems, RelatedKind, RelatedMeaning, RelatedPlace,
    RelatedReference,
};
use orch_authz::{Act, Policy, Principal};
use orch_db::{Conn, DbError, queries::related as rows};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use std::collections::{HashMap, HashSet};

/// A scored item retained until its caller has checked exact current reading access.
pub struct Scored {
    /// The live source and its hierarchy identities.
    pub item: rows::Item,
    /// Its normalized weighted score.
    pub score: f64,
    /// The safe compact result, delivered only after checking the source identity.
    pub result: RelatedItem,
}

/// A bounded ranked candidate pool, with its resolved display count and meaning state.
pub struct Ranked {
    /// Candidates in descending score, then activity, then stable identity.
    pub items: Vec<Scored>,
    /// The setting's result count; zero disables the list.
    pub count: usize,
    /// Whether the current source could contribute meaning.
    pub meaning: RelatedMeaning,
    /// Safe machine-readable unavailability reason.
    pub meaning_reason: Option<String>,
}

#[derive(Clone, Copy)]
struct Generation {
    id: Id,
    dimensions: i32,
    exact_filter_candidates: i64,
}

struct MeaningRequest<'a> {
    source: &'a rows::Item,
    readable: &'a Json,
    count: i64,
}

impl Ranked {
    /// Delivers only identities authorized by the caller's current read frame.
    #[must_use]
    pub fn finish(self, seq: i64, readable: &HashSet<Id>) -> RelatedItems {
        RelatedItems {
            seq,
            items: self
                .items
                .into_iter()
                .filter(|item| readable.contains(&item.item.source_id))
                .take(self.count)
                .map(|item| item.result)
                .collect(),
            meaning: self.meaning,
            meaning_reason: self.meaning_reason,
            next_cursor: None,
        }
    }
}

async fn fractions(
    conn: &mut Conn,
    node: Id,
    key: &str,
) -> Result<HashMap<String, f64>, EngineError> {
    let values = get::<HashMap<String, String>>(conn, Place::Node(node), key)
        .await?
        .ok_or_else(|| EngineError::unreadable("related setting is absent"))?;
    values
        .into_iter()
        .map(|(name, value)| {
            let number = value.parse::<f64>().map_err(EngineError::unreadable)?;
            if !number.is_finite() || !(0.0..=1.0).contains(&number) {
                return Err(EngineError::unreadable(
                    "related fractions must be from zero to one",
                ));
            }
            Ok((name, number))
        })
        .collect()
}

fn place(source: &rows::Item, item: &rows::Item) -> Option<(RelatedPlace, &'static str)> {
    let same = |left: Option<Id>, right: Option<Id>| left.is_some() && left == right;
    if same(source.parent_container_id, item.parent_container_id)
        || same(source.folder_id, item.folder_id)
    {
        Some((RelatedPlace::Folder, "folder"))
    } else if same(source.topic_id, item.topic_id) {
        Some((RelatedPlace::Topic, "topic"))
    } else if source.projects.iter().any(|id| item.projects.contains(id)) {
        Some((RelatedPlace::Project, "project"))
    } else if source.org_id == item.org_id {
        Some((RelatedPlace::Org, "org"))
    } else {
        None
    }
}

/// Combines normalized signals by their available positive weights; missing meaning never dilutes the others.
#[must_use]
pub fn score(signals: [Option<f64>; 3], weights: [f64; 3]) -> f64 {
    let mut total = 0.0;
    let mut weight = 0.0;
    for (signal, w) in signals.into_iter().zip(weights) {
        if let Some(signal) = signal.filter(|value| value.is_finite())
            && w > 0.0
        {
            total += signal.clamp(0.0, 1.0) * w;
            weight += w;
        }
    }
    if weight > 0.0 { total / weight } else { 0.0 }
}

fn scored(
    source: &rows::Item,
    item: rows::Item,
    weights: &HashMap<String, f64>,
    places: &HashMap<String, f64>,
    meaning: Option<f64>,
    reference: Option<&rows::ReferenceScore>,
) -> Option<Scored> {
    let weight = |name: &str| weights.get(name).copied().unwrap_or(0.0);
    let shared_place = place(source, &item);
    let p = shared_place
        .and_then(|(_, name)| places.get(name))
        .copied()
        .unwrap_or(0.0);
    let r = reference.map_or(0.0, |r| {
        if r.outgoing || r.incoming {
            1.0
        } else {
            r.overlap
        }
    });
    // Cosine below zero expresses opposition, rather than a positive related-item signal.
    let m = meaning.map(|value| value.clamp(0.0, 1.0));
    let value = score(
        [m, Some(p), Some(r)],
        [weight("meaning"), weight("place"), weight("references")],
    );
    if value <= 0.0 {
        return None;
    }
    let mut by = Vec::new();
    if m.is_some_and(|m| m > 0.0) && weight("meaning") > 0.0 {
        by.push(RelatedBy::Meaning);
    }
    if p > 0.0 && weight("place") > 0.0 {
        by.push(RelatedBy::Place);
    }
    if r > 0.0 && weight("references") > 0.0 {
        by.push(RelatedBy::References);
    }
    let result = RelatedItem {
        kind: match item.kind.as_str() {
            "chat" => RelatedKind::Chat,
            "conversation" => RelatedKind::Thread,
            _ => RelatedKind::Document,
        },
        id: item.source_id,
        name: item.name.clone(),
        context: Vec::new(),
        place: shared_place
            .filter(|_| by.contains(&RelatedBy::Place))
            .map(|(place, _)| place),
        references: reference
            .filter(|_| by.contains(&RelatedBy::References))
            .map(|reference| {
                if reference.outgoing {
                    RelatedReference::Outgoing
                } else if reference.incoming {
                    RelatedReference::Incoming
                } else {
                    RelatedReference::Shared
                }
            }),
        related_by: by,
    };
    Some(Scored {
        item,
        score: value,
        result,
    })
}

impl Engine {
    /// Collects and ranks a bounded pool in the caller's snapshot; exact item access is checked before delivery.
    pub async fn related_candidates(
        &self,
        conn: &mut Conn,
        source: &rows::Item,
        readable: &Json,
    ) -> Result<Ranked, EngineError> {
        let count = get::<i64>(conn, Place::Node(source.node_id), keys::RELATED_COUNT)
            .await?
            .ok_or_else(|| EngineError::unreadable("related count is absent"))?;
        let mut ranked = Ranked {
            items: Vec::new(),
            count: usize::try_from(count).unwrap_or(0),
            meaning: RelatedMeaning::Off,
            meaning_reason: None,
        };
        if count == 0 {
            return Ok(ranked);
        }
        let candidates = get::<i64>(conn, Place::Node(source.node_id), keys::RELATED_CANDIDATES)
            .await?
            .ok_or_else(|| EngineError::unreadable("related candidates are absent"))?;
        let weights = fractions(conn, source.node_id, keys::RELATED_WEIGHTS).await?;
        let places = fractions(conn, source.node_id, keys::RELATED_PLACE_SCORES).await?;
        let weight = |name: &str| weights.get(name).copied().unwrap_or(0.0);
        let mut ids = HashSet::new();
        if weight("place") > 0.0 {
            ids.extend(rows::by_place(conn, source, readable, candidates).await?);
        }
        if weight("references") > 0.0 {
            ids.extend(rows::by_references(conn, source, candidates).await?);
        }
        let meanings = if weight("meaning") > 0.0 {
            self.related_meaning(
                conn,
                MeaningRequest {
                    source,
                    readable,
                    count: candidates,
                },
                &mut ids,
                &mut ranked,
            )
            .await?
        } else {
            HashMap::new()
        };
        ids.remove(&source.source_id);
        let ids: Vec<_> = ids.into_iter().collect();
        let items = rows::items(conn, &ids).await?;
        let references: HashMap<_, _> = if weight("references") > 0.0 {
            rows::reference_scores(conn, source, &ids, candidates)
                .await?
                .into_iter()
                .map(|row| (row.source_id, row))
                .collect()
        } else {
            HashMap::new()
        };
        for item in items {
            let id = item.source_id;
            if let Some(item) = scored(
                source,
                item,
                &weights,
                &places,
                meanings.get(&id).copied(),
                references.get(&id),
            ) {
                ranked.items.push(item);
            }
        }
        ranked.items.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| right.item.changed_at.cmp(&left.item.changed_at))
                .then_with(|| left.item.source_id.cmp(&right.item.source_id))
        });
        Ok(ranked)
    }

    async fn related_meaning(
        &self,
        conn: &mut Conn,
        request: MeaningRequest<'_>,
        ids: &mut HashSet<Id>,
        ranked: &mut Ranked,
    ) -> Result<HashMap<Id, f64>, EngineError> {
        let source = request.source;
        if get::<bool>(
            conn,
            Place::Node(source.org_id),
            keys::SEARCH_SEMANTIC_ENABLED,
        )
        .await?
            != Some(true)
        {
            return Ok(HashMap::new());
        }
        let tuning = get::<crate::semantic::Tuning>(
            conn,
            Place::Node(source.org_id),
            keys::SEARCH_SEMANTIC_TUNING,
        )
        .await?
        .ok_or_else(|| EngineError::unreadable("semantic tuning is absent"))?;
        if tuning.query_wait_secs <= 0 {
            return Err(EngineError::unreadable(
                "semantic query wait must be positive",
            ));
        }
        let wait = std::time::Duration::from_secs(
            u64::try_from(tuning.query_wait_secs).map_err(EngineError::unreadable)?,
        );
        let (statement, lock): (String, String) = sqlx::query_as(
            "SELECT current_setting('statement_timeout'),current_setting('lock_timeout')",
        )
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
        sqlx::query("SAVEPOINT related_meaning")
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
        let budget = format!("{}ms", wait.as_millis());
        sqlx::query(
            "SELECT set_config('statement_timeout',$1,true),set_config('lock_timeout',$1,true)",
        )
        .bind(budget)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
        let work = async {
            let Some(generation) = self
                .related_generation(conn, source, ranked, &tuning)
                .await?
            else {
                return Ok::<_, EngineError>((Vec::new(), Vec::new()));
            };
            let near = rows::by_meaning(
                conn,
                source,
                generation.id,
                generation.dimensions,
                request.readable,
                request.count,
                generation.exact_filter_candidates,
            )
            .await?;
            let mut selected = ids.clone();
            selected.extend(near.iter().copied());
            selected.remove(&source.source_id);
            let selected: Vec<_> = selected.into_iter().collect();
            let scores = rows::meaning_scores(conn, source, generation.id, &selected).await?;
            Ok((near, scores))
        };
        let outcome = tokio::time::timeout(wait, work).await;
        match outcome {
            Ok(Ok((near, scores))) => {
                sqlx::query("SELECT set_config('statement_timeout',$1,true),set_config('lock_timeout',$2,true)")
                    .bind(statement).bind(lock).execute(&mut *conn).await.map_err(DbError::from)?;
                sqlx::query("RELEASE SAVEPOINT related_meaning")
                    .execute(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
                ids.extend(near);
                Ok(scores.into_iter().collect())
            }
            outcome => {
                if let Ok(Err(error)) = &outcome {
                    tracing::warn!(%error,"optional related meaning lookup failed");
                }
                sqlx::query("ROLLBACK TO SAVEPOINT related_meaning")
                    .execute(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
                sqlx::query("RELEASE SAVEPOINT related_meaning")
                    .execute(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
                ranked.meaning = RelatedMeaning::Unavailable;
                ranked.meaning_reason = Some(
                    if outcome.is_err() {
                        "query_timeout"
                    } else {
                        "index"
                    }
                    .into(),
                );
                Ok(HashMap::new())
            }
        }
    }

    async fn related_generation(
        &self,
        conn: &mut Conn,
        source: &rows::Item,
        ranked: &mut Ranked,
        tuning: &crate::semantic::Tuning,
    ) -> Result<Option<Generation>, EngineError> {
        let stored: bool =
            sqlx::query_scalar("SELECT to_regclass('search_item_vectors') IS NOT NULL")
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
        if !stored {
            ranked.meaning = RelatedMeaning::Unavailable;
            ranked.meaning_reason = Some("absent".to_owned());
            return Ok(None);
        }
        let active = orch_db::queries::semantic::generations(conn, source.org_id)
            .await?
            .into_iter()
            .find(|row| row.state == "active");
        let Some(active) = active.filter(|row| row.dimensions.is_some()) else {
            ranked.meaning = RelatedMeaning::NotReady;
            return Ok(None);
        };
        let present: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM search_item_vectors v
           JOIN search_sources s ON s.generation_id=v.generation_id AND s.kind=v.kind AND s.source_id=v.source_id
           WHERE v.generation_id=$1 AND v.kind=$2 AND v.source_id=$3 AND v.marker=$4 AND s.marker=$4 AND s.title=$5 AND vector_norm(v.embedding)>0)")
            .bind(active.id).bind(&source.kind).bind(source.source_id).bind(source.marker)
            .bind(&source.name)
            .fetch_one(&mut *conn).await.map_err(DbError::from)?;
        if !present {
            ranked.meaning = RelatedMeaning::NotReady;
            return Ok(None);
        }
        sqlx::query("SELECT set_config('hnsw.iterative_scan','strict_order',true),set_config('hnsw.max_scan_tuples',$1,true),set_config('hnsw.scan_mem_multiplier',$2,true)")
            .bind(tuning.max_scan_tuples.to_string()).bind(tuning.scan_mem_multiplier.to_string())
            .execute(&mut *conn).await.map_err(DbError::from)?;
        ranked.meaning = RelatedMeaning::Ready;
        Ok(active.dimensions.map(|dimensions| Generation {
            id: active.id,
            dimensions,
            exact_filter_candidates: tuning.exact_filter_candidates,
        }))
    }

    /// The same related ranking for a person, with the ordinary current document, thread and chat access checks.
    pub async fn related_for_person(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        document: Id,
    ) -> Result<Option<RelatedItems>, EngineError> {
        let source = rows::items(conn, &[document])
            .await?
            .into_iter()
            .find(|item| item.kind == "document");
        let Some(source) = source else {
            return Ok(None);
        };
        let source_access =
            orch_authz::documents::listed(conn, principal, &[document], Policy::default()).await?;
        if !source_access
            .iter()
            .any(|item| item.holds(Permission::DocView) && !item.document.deleted)
        {
            return Ok(None);
        }
        let projection =
            orch_authz::everything::project_search(conn, principal, None, &[document], &[], &[])
                .await?;
        let candidates = self
            .related_candidates(conn, &source, &projection.search_candidates(principal))
            .await?;
        let ids = |kind: &str| {
            candidates
                .items
                .iter()
                .filter(|item| item.item.kind == kind)
                .map(|item| item.item.source_id)
                .collect::<Vec<_>>()
        };
        let mut readable: HashSet<Id> =
            orch_authz::documents::listed(conn, principal, &ids("document"), Policy::default())
                .await?
                .into_iter()
                .filter(|item| item.holds(Permission::DocView) && !item.document.deleted)
                .map(|item| item.document.id)
                .collect();
        readable.extend(
            orch_authz::nodes::holding(
                conn,
                principal,
                &ids("conversation"),
                PermissionName::OfNode(NodePermission::View),
                Act::Use,
                Policy::default(),
            )
            .await?
            .into_iter()
            .map(|item| item.id),
        );
        let chats = orch_db::queries::chats::chats(conn, &ids("chat")).await?;
        readable.extend(
            orch_authz::chats::listed_access(conn, principal, &chats)
                .await?
                .into_keys(),
        );
        Ok(Some(candidates.finish(source.marker, &readable)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_rank_alone_together_and_without_meaning() {
        assert_eq!(
            score([Some(0.9), Some(0.1), Some(0.0)], [1.0, 0.0, 0.0]),
            0.9
        );
        assert!(
            (score([None, Some(0.7), Some(1.0)], [0.6, 0.25, 0.15]) - 0.8125).abs() < f64::EPSILON
        );
        assert!(
            (score([Some(0.9), Some(0.7), Some(1.0)], [0.6, 0.25, 0.15]) - 0.865).abs()
                < f64::EPSILON
        );
        assert_eq!(score([None, Some(0.7), Some(1.0)], [0.0; 3]), 0.0);
    }
}
