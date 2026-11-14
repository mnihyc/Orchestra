//! Shared continuations for authorized named collections and complete native snapshots.
use crate::middleware::requests::{ApiError, ApiResult};
use orch_api_contract::common::{CollectionQuery, DEFAULT_LIMIT, Page};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

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
            .map(|row| {
                serde_json::to_string(&Cursor {
                    source: source.to_owned(),
                    query: search(query),
                    after: key(row),
                })
                .map_err(ApiError::internal)
            })
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
