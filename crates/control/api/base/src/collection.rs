//! Shared continuations for authorized named collections and complete native snapshots.
use crate::middleware::requests::{ApiError, ApiResult};
use orch_api_contract::common::{CollectionQuery, DEFAULT_LIMIT, Page};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// One captured native catalogue, kept independently of its currently visible page.
#[derive(Clone)]
pub struct Captured<T> {
    /// The opaque source of its continuations.
    pub id: orch_types::Id,
    /// The complete observation; a cache budget never crops it.
    pub rows: std::sync::Arc<Vec<T>>,
}

struct Held<T> {
    captured: Captured<T>,
    scope: String,
    bytes: usize,
    touched: std::time::Instant,
}

/// An application's native observations, with idle expiry and oldest-unused budget relief.
pub struct Snapshots<T> {
    held: std::sync::Mutex<std::collections::VecDeque<Held<T>>>,
}

impl<T> Default for Snapshots<T> {
    fn default() -> Self {
        Self {
            held: std::sync::Mutex::new(std::collections::VecDeque::new()),
        }
    }
}
impl<T> std::fmt::Debug for Snapshots<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Snapshots").finish_non_exhaustive()
    }
}
impl<T: Clone + Serialize> Snapshots<T> {
    fn locked(&self) -> std::sync::MutexGuard<'_, std::collections::VecDeque<Held<T>>> {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A fresh complete catalogue may occupy the cache alone when it exceeds the target budget.
    pub fn remember(
        &self,
        scope: &str,
        rows: Vec<T>,
        budget: usize,
        idle: Option<std::time::Duration>,
        now: std::time::Instant,
    ) -> Result<Captured<T>, serde_json::Error> {
        let bytes = serde_json::to_vec(&rows)?.len();
        let captured = Captured {
            id: orch_types::new_id(),
            rows: std::sync::Arc::new(rows),
        };
        let mut held = self.locked();
        held.retain(|entry| {
            idle.is_none_or(|idle| now.saturating_duration_since(entry.touched) < idle)
        });
        if budget > 0 {
            let mut total = held
                .iter()
                .fold(0usize, |sum, entry| sum.saturating_add(entry.bytes));
            while total.saturating_add(bytes) > budget {
                let Some(oldest) = held.pop_front() else {
                    break;
                };
                total = total.saturating_sub(oldest.bytes);
            }
        }
        held.push_back(Held {
            captured: captured.clone(),
            scope: scope.to_owned(),
            bytes,
            touched: now,
        });
        Ok(captured)
    }

    /// Current authorization and native configuration select the scope again before this lookup.
    pub fn get(
        &self,
        id: orch_types::Id,
        scope: &str,
        idle: Option<std::time::Duration>,
        now: std::time::Instant,
    ) -> Option<Captured<T>> {
        let mut held = self.locked();
        held.retain(|entry| {
            idle.is_none_or(|idle| now.saturating_duration_since(entry.touched) < idle)
        });
        let position = held
            .iter()
            .position(|entry| entry.captured.id == id && entry.scope == scope)?;
        let mut entry = held.remove(position)?;
        entry.touched = now;
        let captured = entry.captured.clone();
        held.push_back(entry);
        Some(captured)
    }
}

fn role_name(role: orch_api_contract::access::Role) -> &'static str {
    use orch_api_contract::access::Role;
    match role {
        Role::Viewer => "viewer",
        Role::Interactor => "interactor",
        Role::Developer => "developer",
        Role::Admin => "admin",
    }
}

/// A holder's live people count never changes its continuation identity.
#[must_use]
pub fn holder_key(row: &orch_api_contract::access::Holder) -> String {
    use orch_api_contract::access::Holder;
    match row {
        Holder::User { user } => format!("user:{}", user.id),
        Holder::Group { group } => format!("group:{}", group.id),
        Holder::Members { node } => format!("members:{}", node.id),
        Holder::Role { role, at, .. } => format!(
            "role:{}:{:?}",
            role_name(*role),
            at.as_ref().map(|node| node.id)
        ),
        Holder::Node { node } => format!("node:{}", node.id),
        Holder::Agents { node } => format!("agents:{}", node.id),
        Holder::Everyone => "everyone".to_owned(),
    }
}

/// Only the admitted visible name participates in holder search.
#[must_use]
pub fn holder_name(row: &orch_api_contract::access::Holder) -> String {
    use orch_api_contract::access::Holder;
    match row {
        Holder::User { user } => user.display_name.clone(),
        Holder::Group { group } => group.name.clone(),
        Holder::Members { node } | Holder::Node { node } | Holder::Agents { node } => {
            node.name.clone().unwrap_or_default()
        }
        Holder::Role {
            role, at, place, ..
        } => format!(
            "{} {} {}",
            role_name(*role),
            at.as_ref()
                .and_then(|node| node.name.as_deref())
                .unwrap_or(""),
            place.as_deref().unwrap_or("")
        ),
        Holder::Everyone => "everyone".to_owned(),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor<K> {
    source: String,
    query: String,
    after: K,
}

fn search(query: &CollectionQuery) -> String {
    query
        .q
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Search words share the same Unicode lowercase comparison as snapshot paging.
#[must_use]
pub fn terms(query: &CollectionQuery) -> Vec<String> {
    search(query)
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// A page size is a batch choice, never a ceiling on the collection.
pub fn limit(query: &CollectionQuery) -> ApiResult<u32> {
    match query.limit.unwrap_or(DEFAULT_LIMIT) {
        0 => Err(ApiError::invalid("limit", "must be greater than zero")),
        value => Ok(value),
    }
}

/// A continuation is meaningful only for its original authorized collection and search.
pub fn after<K: DeserializeOwned>(query: &CollectionQuery, source: &str) -> ApiResult<Option<K>> {
    query
        .cursor
        .as_deref()
        .map(|value| {
            let cursor: Cursor<K> = serde_json::from_str(value)
                .map_err(|_| ApiError::invalid("cursor", "is not a cursor this list gave"))?;
            if cursor.source != source || cursor.query != search(query) {
                return Err(ApiError::invalid(
                    "cursor",
                    "belongs to another collection or search",
                ));
            }
            Ok(cursor.after)
        })
        .transpose()
}

/// Encode an actual last row under its original collection and normalized search.
pub fn cursor<K: Serialize>(query: &CollectionQuery, source: &str, after: K) -> ApiResult<String> {
    serde_json::to_string(&Cursor {
        source: source.to_owned(),
        query: search(query),
        after,
    })
    .map_err(ApiError::internal)
}

/// Ordered rows include one lookahead; only an actual remainder produces a continuation.
pub fn finish<T, K: Serialize>(
    query: &CollectionQuery,
    source: &str,
    mut rows: Vec<T>,
    key: impl Fn(&T) -> K,
) -> ApiResult<Page<T>> {
    let size = usize::try_from(limit(query)?).map_err(ApiError::internal)?;
    let more = rows.len() > size;
    if more {
        rows.truncate(size);
    }
    let next_cursor = if more {
        rows.last()
            .map(|row| cursor(query, source, key(row)))
            .transpose()?
    } else {
        None
    };
    Ok(Page {
        items: rows,
        next_cursor,
    })
}

/// A complete authorized snapshot can use the same stable name search and continuation as stored collections.
pub fn snapshot<T, K: Ord + Clone + Serialize + DeserializeOwned>(
    query: &CollectionQuery,
    source: &str,
    rows: Vec<T>,
    key: impl Fn(&T) -> K,
    text: impl Fn(&T) -> String,
    descending: bool,
) -> ApiResult<Page<T>> {
    let after = after::<K>(query, source)?;
    let words = terms(query);
    let mut selected: Vec<(K, T)> = rows
        .into_iter()
        .filter(|row| {
            if words.is_empty() {
                return true;
            }
            let name = text(row).to_lowercase();
            words.iter().all(|word| name.contains(word))
        })
        .map(|row| (key(&row), row))
        .filter(|(key, _)| {
            after
                .as_ref()
                .is_none_or(|after| if descending { key < after } else { key > after })
        })
        .collect();
    selected.sort_by(|(one, _), (other, _)| {
        if descending {
            other.cmp(one)
        } else {
            one.cmp(other)
        }
    });
    let page = finish(query, source, selected, |(key, _)| key.clone())?;
    Ok(Page {
        items: page.items.into_iter().map(|(_, row)| row).collect(),
        next_cursor: page.next_cursor,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "assertions fail by panicking")]
mod tests {
    use super::*;

    #[test]
    fn captured_catalogues_keep_every_row_and_only_their_requesting_scope() {
        let cache = Snapshots::<String>::default();
        let now = std::time::Instant::now();
        let rows: Vec<_> = (0..1_001).map(|id| format!("Entry {id}")).collect();
        let held = cache
            .remember("reader:provider:profile", rows.clone(), 64, None, now)
            .unwrap();
        assert_eq!(*held.rows, rows);
        assert!(cache.get(held.id, "another-reader", None, now).is_none());
        assert_eq!(
            cache
                .get(held.id, "reader:provider:profile", None, now)
                .unwrap()
                .rows
                .len(),
            1_001
        );
        let newer = cache
            .remember(
                "reader:another-provider",
                vec!["New".to_owned()],
                64,
                None,
                now,
            )
            .unwrap();
        assert!(
            cache
                .get(held.id, "reader:provider:profile", None, now)
                .is_none()
        );
        assert_eq!(
            cache
                .get(newer.id, "reader:another-provider", None, now)
                .unwrap()
                .rows
                .len(),
            1
        );
    }

    #[test]
    fn active_catalogue_reads_renew_expiry_and_another_application_has_no_snapshot() {
        let cache = Snapshots::<String>::default();
        let now = std::time::Instant::now();
        let idle = Some(std::time::Duration::from_secs(30));
        let held = cache
            .remember("scope", vec!["One".to_owned()], 0, idle, now)
            .unwrap();
        assert!(
            Snapshots::<String>::default()
                .get(held.id, "scope", idle, now)
                .is_none()
        );
        assert!(
            cache
                .get(
                    held.id,
                    "scope",
                    idle,
                    now + std::time::Duration::from_secs(25)
                )
                .is_some()
        );
        assert!(
            cache
                .get(
                    held.id,
                    "scope",
                    idle,
                    now + std::time::Duration::from_secs(50)
                )
                .is_some()
        );
        assert!(
            cache
                .get(
                    held.id,
                    "scope",
                    idle,
                    now + std::time::Duration::from_secs(81)
                )
                .is_none()
        );
    }

    #[test]
    fn named_snapshot_pages_search_every_entry_and_bind_their_continuations() {
        let rows: Vec<_> = (0..1_001)
            .map(|id| (id, format!("Entry {id:04}")))
            .collect();
        let query = CollectionQuery {
            limit: Some(37),
            ..Default::default()
        };
        let first = snapshot(
            &query,
            "named",
            rows.clone(),
            |row| row.0,
            |row| row.1.clone(),
            false,
        )
        .unwrap();
        assert_eq!(first.items.len(), 37);
        let mut next = CollectionQuery {
            cursor: first.next_cursor,
            ..query
        };
        let second = snapshot(
            &next,
            "named",
            rows.clone(),
            |row| row.0,
            |row| row.1.clone(),
            false,
        )
        .unwrap();
        assert_eq!(second.items[0].0, 37);
        assert!(
            snapshot(
                &next,
                "another",
                rows.clone(),
                |row| row.0,
                |row| row.1.clone(),
                false
            )
            .is_err()
        );
        next.q = Some("Entry 1000".to_owned());
        assert!(
            snapshot(
                &next,
                "named",
                rows.clone(),
                |row| row.0,
                |row| row.1.clone(),
                false
            )
            .is_err()
        );
        next.cursor = None;
        let found = snapshot(
            &next,
            "named",
            rows,
            |row| row.0,
            |row| row.1.clone(),
            false,
        )
        .unwrap();
        assert_eq!(found.items, [(1_000, "Entry 1000".to_owned())]);
        assert!(found.next_cursor.is_none());
    }
}
