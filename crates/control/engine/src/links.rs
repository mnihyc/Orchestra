//! Which sources refer to a document, and the graph around it, found only through what the reader may read, in an order a cursor continues.
//!
//! A statement of the database narrows the sources by the reader's conservative scopes and by what is still alive.
//! Each source that is left is then checked exactly, in batches, and only checked sources are counted, listed or
//! drawn. A hidden source is never an answer, never a count and never the position a cursor names: the position of a
//! cursor is a source the reader was given, or is sealed by the caller.

use crate::{engine::Engine, error::EngineError, related};
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName};
use orch_api_contract::search::{RelatedBy, RelatedMeaning};
use orch_authz::{Act, Policy, Principal};
use orch_db::queries::{links as rows, related as related_rows};
use orch_db::{Conn, queries::chats as chat_rows};
use orch_types::{Hash, Id, Json};
use std::collections::{HashMap, HashSet};

/// A round of checking asks for this many pages of candidates, so that most pages are found in one round.
const ROUND_PAGES: usize = 4;
/// A round asks for at least this many candidates, so that a short page does not cost many rounds.
const ROUND_MIN: usize = 100;

/// Who may read which source: the one question the listing and the graph put to whoever is asking.
pub trait SourceAccess: Sync {
    /// Of these sources, the identities this reader may read now.
    fn readable(
        &self,
        conn: &mut Conn,
        sources: &[rows::LiveSource],
    ) -> impl Future<Output = Result<HashSet<Id>, EngineError>> + Send;
}

/// A person, with the ordinary current access checks of a document, a thread and a chat.
pub struct Person<'a>(pub &'a Principal);

impl SourceAccess for Person<'_> {
    async fn readable(
        &self,
        conn: &mut Conn,
        sources: &[rows::LiveSource],
    ) -> Result<HashSet<Id>, EngineError> {
        readable_by_person(conn, self.0, sources).await
    }
}

/// The documents, threads and chats among `sources` that a person may read now.
pub async fn readable_by_person(
    conn: &mut Conn,
    principal: &Principal,
    sources: &[rows::LiveSource],
) -> Result<HashSet<Id>, EngineError> {
    let ids = |kind: &str| {
        sources
            .iter()
            .filter(|source| source.kind == kind)
            .map(|source| source.source_id)
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
    let chat_ids = ids("chat");
    if !chat_ids.is_empty() {
        let chats = chat_rows::chats(conn, &chat_ids).await?;
        let access = orch_authz::chats::listed_access(conn, principal, &chats).await?;
        // A direct or group chat is read by the people in it: reading one by inspection is an audited opening
        // of that chat and never a mention in a list.
        let direct: Vec<Id> = chats
            .iter()
            .filter(|chat| chat.node_id.is_none())
            .map(|chat| chat.id)
            .collect();
        let member: HashSet<Id> =
            orch_db::queries::search::participant_chats(conn, principal.user(), &direct)
                .await?
                .into_iter()
                .map(|chat| chat.id)
                .collect();
        readable.extend(chats.iter().filter_map(|chat| {
            (access.contains_key(&chat.id) && (chat.node_id.is_some() || member.contains(&chat.id)))
                .then_some(chat.id)
        }));
    }
    Ok(readable)
}

/// A page of the sources that refer to a document.
#[derive(Debug, Clone)]
pub struct SourcePage {
    /// The sources the reader may read, newest identity first, at most a page of them.
    pub sources: Vec<rows::LiveSource>,
    /// Where the next page goes on, as an identity the next page lies below: the last source delivered, or, when the
    /// access checks of this page ran out first, the last candidate examined. `None` at the end.
    pub next: Option<Id>,
    /// Whether `next` is the last candidate examined rather than a source delivered, and so must not be shown as it is.
    pub hides_position: bool,
}

/// What a page of sources asks.
#[derive(Debug, Clone, Copy)]
pub struct SourcePageAsk<'a> {
    /// The document they refer to.
    pub target: Id,
    /// The reader's conservative scopes.
    pub scopes: &'a Json,
    /// The sources the page continues below; none for the first page.
    pub before: Option<Id>,
    /// How many sources the page holds at most.
    pub limit: usize,
    /// How many candidates may be examined for it.
    pub checks: usize,
}

/// The next page of the sources that refer to `target` and that `access` may read.
pub async fn sources_page(
    conn: &mut Conn,
    access: &impl SourceAccess,
    ask: SourcePageAsk<'_>,
) -> Result<SourcePage, EngineError> {
    let limit = ask.limit.max(1);
    let checks = ask.checks.max(1);
    let mut found: Vec<rows::LiveSource> = Vec::new();
    let mut below = ask.before;
    let mut examined = 0usize;
    loop {
        let round = (limit * ROUND_PAGES).max(ROUND_MIN).min(checks - examined);
        let ids = rows::raw_sources(
            conn,
            ask.target,
            below,
            i64::try_from(round).unwrap_or(i64::MAX),
        )
        .await?;
        let Some(&last) = ids.last() else {
            return Ok(SourcePage {
                sources: found,
                next: None,
                hides_position: false,
            });
        };
        let mut live = rows::live_sources(conn, ask.target, &ids, ask.scopes).await?;
        live.sort_by_key(|source| std::cmp::Reverse(source.source_id));
        let readable = access.readable(conn, &live).await?;
        for source in live {
            if !readable.contains(&source.source_id) {
                continue;
            }
            if found.len() == limit {
                // One more readable source stands beyond the page, so a next page is worth asking for.
                return Ok(SourcePage {
                    next: found.last().map(|delivered| delivered.source_id),
                    sources: found,
                    hides_position: false,
                });
            }
            found.push(source);
        }
        examined += ids.len();
        below = Some(last);
        if ids.len() < round {
            return Ok(SourcePage {
                sources: found,
                next: None,
                hides_position: false,
            });
        }
        if examined >= checks {
            // The checks ran out with candidates left: go on below the last one examined, which may be hidden.
            return Ok(SourcePage {
                sources: found,
                next: Some(last),
                hides_position: true,
            });
        }
    }
}

/// A whole thing the graph may draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphMember {
    /// `document`, `chat` or `conversation`.
    pub kind: String,
    /// Its identity.
    pub id: Id,
    /// How many steps from the centre.
    pub distance: u32,
}

/// The readable things around a centre, and the references among them.
#[derive(Debug, Clone, Default)]
pub struct GraphRows {
    /// The centre first, then the rest by distance.
    pub members: Vec<GraphMember>,
    /// The references among them, in the direction they are written.
    pub links: Vec<rows::Link>,
    /// Whether a readable thing was left out because the graph was full.
    pub truncated: bool,
}

/// What a graph asks.
#[derive(Debug, Clone, Copy)]
pub struct GraphAsk<'a> {
    /// The centre: its kind (`document`, `chat` or `conversation`) and identity.
    pub center: (&'a str, Id),
    /// The reader's conservative scopes.
    pub scopes: &'a Json,
    /// How many steps from the centre, at most.
    pub depth: u32,
    /// The kinds that may be drawn besides the centre.
    pub kinds: &'a [&'a str],
    /// The most members.
    pub limit: usize,
    /// How many candidates may be examined in all.
    pub checks: usize,
}

/// The things within `depth` steps of the centre that `access` may read, found only through readable things.
pub async fn graph(
    conn: &mut Conn,
    access: &impl SourceAccess,
    ask: GraphAsk<'_>,
) -> Result<GraphRows, EngineError> {
    let mut out = GraphRows {
        members: vec![GraphMember {
            kind: ask.center.0.to_owned(),
            id: ask.center.1,
            distance: 0,
        }],
        ..GraphRows::default()
    };
    let mut seen: HashSet<Id> = HashSet::from([ask.center.1]);
    let mut frontier: Vec<(String, Id)> = vec![(ask.center.0.to_owned(), ask.center.1)];
    let mut examined = 0usize;
    for distance in 1..=ask.depth {
        if frontier.is_empty() || examined >= ask.checks {
            break;
        }
        let budget = ask.checks - examined;
        // The checks are shared among the things being walked from, so that one that is cited a hundred thousand
        // times does not use them all, and each of them is asked for no more than its share.
        let share = budget.div_ceil(frontier.len()).max(1);
        let walked: Vec<rows::Whole<'_>> = frontier
            .iter()
            .map(|(kind, id)| (kind.as_str(), *id))
            .collect();
        let links = rows::neighbours(
            conn,
            &walked,
            ask.scopes,
            i64::try_from(share).unwrap_or(i64::MAX),
            i64::try_from(budget).unwrap_or(i64::MAX),
        )
        .await?;
        let mut candidates: Vec<rows::LiveSource> = Vec::new();
        let mut listed: HashSet<Id> = HashSet::new();
        for link in &links {
            for (kind, id) in [(&link.from_kind, link.from_id), (&link.to_kind, link.to_id)] {
                if seen.contains(&id) || !ask.kinds.contains(&kind.as_str()) {
                    continue;
                }
                if listed.insert(id) {
                    candidates.push(rows::LiveSource {
                        kind: kind.clone(),
                        source_id: id,
                    });
                }
            }
        }
        candidates.sort_by_key(|source| std::cmp::Reverse(source.source_id));
        candidates.truncate(budget);
        examined += candidates.len();
        if candidates.is_empty() {
            break;
        }
        let readable = access.readable(conn, &candidates).await?;
        let mut next: Vec<(String, Id)> = Vec::new();
        for candidate in candidates {
            if !readable.contains(&candidate.source_id) {
                continue;
            }
            if out.members.len() >= ask.limit {
                // Only a thing already known to be readable is ever the reason the graph is cut.
                out.truncated = true;
                break;
            }
            seen.insert(candidate.source_id);
            next.push((candidate.kind.clone(), candidate.source_id));
            out.members.push(GraphMember {
                kind: candidate.kind,
                id: candidate.source_id,
                distance,
            });
        }
        frontier = next;
    }
    let members: Vec<rows::Whole<'_>> = out
        .members
        .iter()
        .map(|member| (member.kind.as_str(), member.id))
        .collect();
    out.links = rows::among(conn, &members).await?;
    Ok(out)
}

/// Where a Related page goes on: the last item delivered, in the order Related ranks them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelatedPosition {
    /// Its score.
    pub score: f64,
    /// When it last changed.
    pub changed_at: chrono::DateTime<chrono::Utc>,
    /// Its identity.
    pub id: Id,
}

impl RelatedPosition {
    /// Whether an item ranks after this position: a lower score, then an older change, then a later identity.
    #[must_use]
    pub fn precedes(&self, score: f64, changed_at: chrono::DateTime<chrono::Utc>, id: Id) -> bool {
        match score.total_cmp(&self.score) {
            std::cmp::Ordering::Less => true,
            std::cmp::Ordering::Greater => false,
            std::cmp::Ordering::Equal => match changed_at.cmp(&self.changed_at) {
                std::cmp::Ordering::Less => true,
                std::cmp::Ordering::Greater => false,
                std::cmp::Ordering::Equal => id > self.id,
            },
        }
    }
}

/// A page of related items, with where the next goes on.
#[derive(Debug, Clone)]
pub struct RelatedPage {
    /// The page, as the plain related list answers it.
    pub answer: orch_api_contract::search::RelatedItems,
    /// The position after the last item delivered, when more readable items follow.
    pub next: Option<RelatedPosition>,
    /// The current readable order and page default, used to retire a stale continuation.
    pub fingerprint: Hash,
}

impl Engine {
    /// A page of the items related to a document, for a person; the readable items after `after`, at most `limit`
    /// (the document's own count of related items when none is given).
    pub async fn related_page_for_person(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        document: Id,
        after: Option<RelatedPosition>,
        limit: Option<usize>,
    ) -> Result<Option<RelatedPage>, EngineError> {
        let Some(source) = related_source(conn, principal, document).await? else {
            return Ok(None);
        };
        let ranked = self
            .related_candidates(conn, &source.item, &source.scopes)
            .await?;
        let readable = readable_related(conn, principal, &ranked).await?;
        let mut digest = Hash::hasher();
        digest.update(document.as_bytes());
        digest.update(principal.user().as_bytes());
        digest.update(&source.item.marker.to_le_bytes());
        digest.update(
            &u64::try_from(ranked.count)
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        digest.update(
            &serde_json::to_vec(&(&ranked.meaning, &ranked.meaning_reason))
                .map_err(EngineError::unreadable)?,
        );
        for item in ranked
            .items
            .iter()
            .filter(|item| readable.contains(&item.item.source_id))
        {
            digest.update(
                &serde_json::to_vec(&(item.score, item.item.changed_at, &item.result))
                    .map_err(EngineError::unreadable)?,
            );
        }
        let fingerprint = Hash::finish(&digest);
        let page = limit.unwrap_or(ranked.count);
        let mut delivered: Vec<(RelatedPosition, orch_api_contract::search::RelatedItem)> =
            Vec::new();
        let mut more = false;
        for item in ranked.items {
            if !readable.contains(&item.item.source_id) {
                continue;
            }
            if after.is_some_and(|position| {
                !position.precedes(item.score, item.item.changed_at, item.item.source_id)
            }) {
                continue;
            }
            if delivered.len() == page {
                // One more readable item follows the page, so a next page is worth asking for.
                more = true;
                break;
            }
            delivered.push((
                RelatedPosition {
                    score: item.score,
                    changed_at: item.item.changed_at,
                    id: item.item.source_id,
                },
                item.result,
            ));
        }
        let next = if more {
            delivered.last().map(|(position, _)| *position)
        } else {
            None
        };
        Ok(Some(RelatedPage {
            answer: orch_api_contract::search::RelatedItems {
                seq: source.item.marker,
                items: delivered.into_iter().map(|(_, item)| item).collect(),
                meaning: ranked.meaning,
                meaning_reason: ranked.meaning_reason,
                next_cursor: None,
            },
            next,
            fingerprint,
        }))
    }

    /// The readable items of a document that its meaning, and nothing else, makes related: the dashed edges of its
    /// graph, at most `limit`, with whether meaning could be used at all.
    pub async fn meaning_neighbours(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        document: Id,
        limit: usize,
    ) -> Result<(Vec<(String, Id)>, RelatedMeaning), EngineError> {
        let Some(source) = related_source(conn, principal, document).await? else {
            return Ok((Vec::new(), RelatedMeaning::Off));
        };
        let ranked = self
            .related_candidates(conn, &source.item, &source.scopes)
            .await?;
        let readable = readable_related(conn, principal, &ranked).await?;
        let found = ranked
            .items
            .iter()
            .filter(|item| {
                readable.contains(&item.item.source_id)
                    && item.result.related_by.contains(&RelatedBy::Meaning)
            })
            .take(limit)
            .map(|item| (item.item.kind.clone(), item.item.source_id))
            .collect();
        Ok((found, ranked.meaning))
    }
}

/// A document that a person may read, with the scopes its related items are searched in.
struct RelatedSource {
    item: related_rows::Item,
    scopes: Json,
}

async fn related_source(
    conn: &mut Conn,
    principal: &Principal,
    document: Id,
) -> Result<Option<RelatedSource>, EngineError> {
    let item = related_rows::items(conn, &[document])
        .await?
        .into_iter()
        .find(|item| item.kind == "document");
    let Some(item) = item else {
        return Ok(None);
    };
    let held =
        orch_authz::documents::listed(conn, principal, &[document], Policy::default()).await?;
    if !held
        .iter()
        .any(|held| held.holds(Permission::DocView) && !held.document.deleted)
    {
        return Ok(None);
    }
    let projection =
        orch_authz::everything::project_search(conn, principal, None, &[document], &[], &[])
            .await?;
    Ok(Some(RelatedSource {
        item,
        scopes: projection.search_candidates(principal),
    }))
}

/// Of a ranked pool, the identities this person may read now.
async fn readable_related(
    conn: &mut Conn,
    principal: &Principal,
    ranked: &related::Ranked,
) -> Result<HashSet<Id>, EngineError> {
    let sources: Vec<rows::LiveSource> = ranked
        .items
        .iter()
        .map(|item| rows::LiveSource {
            kind: item.item.kind.clone(),
            source_id: item.item.source_id,
        })
        .collect();
    readable_by_person(conn, principal, &sources).await
}

/// The whole things among `ids`, by identity: their kinds as the edge store names them.
pub async fn kinds_of(conn: &mut Conn, ids: &[Id]) -> Result<HashMap<Id, String>, EngineError> {
    Ok(related_rows::items(conn, ids)
        .await?
        .into_iter()
        .map(|item| (item.source_id, item.kind))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn position(score: f64, day: u32, id: u128) -> RelatedPosition {
        RelatedPosition {
            score,
            changed_at: Utc.with_ymd_and_hms(2026, 10, day, 0, 0, 0).unwrap(),
            id: Id::from_u128(id),
        }
    }

    #[test]
    fn an_item_follows_a_position_by_score_then_age_then_identity() {
        let at = position(0.5, 10, 5);
        let day = |day| Utc.with_ymd_and_hms(2026, 10, day, 0, 0, 0).unwrap();
        assert!(at.precedes(0.4, day(20), Id::from_u128(1)));
        assert!(!at.precedes(0.6, day(1), Id::from_u128(9)));
        assert!(at.precedes(0.5, day(9), Id::from_u128(1)));
        assert!(!at.precedes(0.5, day(11), Id::from_u128(9)));
        assert!(at.precedes(0.5, day(10), Id::from_u128(6)));
        assert!(!at.precedes(0.5, day(10), Id::from_u128(5)));
    }
}
