//! The private documents administered beside a place, paged by name without content.

use orch_access_contract::permissions::Permission;
use orch_api_contract::common::{HiddenItem, Listed};
use orch_api_contract::documents::DocumentRef;
use orch_authz::Policy;
use orch_db::Conn;
use orch_db::queries::documents as rows;
use orch_types::Id;

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::routes::nodes::invalid;

/// The second phase has a scan position, never an unread document's name or activity timestamp.
const PRIVATE_CURSOR: &str = "private:";

/// Reads the private phase; ordinary activity cursors retain their existing grammar.
pub(super) fn offset(cursor: Option<&str>) -> ApiResult<Option<i64>> {
    cursor
        .and_then(|text| text.strip_prefix(PRIVATE_CURSOR))
        .map(|value| {
            value
                .parse::<i64>()
                .ok()
                .filter(|offset| *offset >= 0)
                .ok_or_else(|| invalid("cursor", "is not a cursor of this list"))
        })
        .transpose()
}

/// The scope and remaining position of the list's private phase.
pub(super) struct PrivateList {
    /// The place requested.
    pub node: Id,
    /// Whether its descendants are included.
    pub under: bool,
    /// How many rows at most the whole page holds.
    pub limit: i64,
    /// The name-ordered scan position.
    pub offset: i64,
}

/// Fills the room left by readable rows with minimal rows, under the same bounded scan as the readable list.
pub(super) struct PrivateRows {
    pub rows: Vec<rows::DocumentNameRow>,
    pub next_cursor: Option<String>,
}

pub(super) async fn select(
    conn: &mut Conn,
    reader: &mut super::list_read::Reader<'_>,
    request: PrivateList,
    shown: usize,
) -> ApiResult<PrivateRows> {
    let mut offset = request.offset;
    let mut exhausted = false;
    let mut more = false;
    let mut hidden = Vec::new();
    for _ in 0..super::LIST_ROUNDS_MAX {
        let found =
            rows::names_beside(conn, request.node, request.under, offset, request.limit + 1)
                .await?;
        exhausted = i64::try_from(found.len()).unwrap_or(i64::MAX) <= request.limit;
        let ids: Vec<Id> = found.iter().map(|row| row.id).collect();
        let allowed = reader.accept(conn, &ids).await?;
        let ids: Vec<_> = ids.into_iter().filter(|id| allowed.contains(id)).collect();
        let standing = if ids.is_empty() {
            std::collections::HashMap::new()
        } else {
            orch_authz::documents::listed(conn, reader.principal()?, &ids, Policy::default())
                .await
                .map_err(ApiError::internal)?
                .into_iter()
                .map(|held| (held.document.id, held))
                .collect()
        };
        for row in found {
            let private = standing.get(&row.id).is_some_and(|held| {
                !held.holds(Permission::DocView) && held.holds(Permission::AccessManage)
            });
            if private && i64::try_from(shown + hidden.len()).unwrap_or(i64::MAX) == request.limit {
                more = true;
                break;
            }
            offset = offset
                .checked_add(1)
                .ok_or_else(|| invalid("cursor", "is not a cursor of this list"))?;
            if private {
                hidden.push(row);
            }
        }
        if more || exhausted {
            break;
        }
    }
    Ok(PrivateRows {
        rows: hidden,
        next_cursor: (more || !exhausted).then(|| format!("{PRIVATE_CURSOR}{offset}")),
    })
}

/// The existing minimal private-row renderer, without content or activity facts.
pub(super) async fn render(
    conn: &mut Conn,
    hidden: &[rows::DocumentNameRow],
) -> ApiResult<Vec<Listed<DocumentRef>>> {
    let owners: Vec<Id> = hidden.iter().map(|row| row.owner_id).collect();
    let owners = refusals::user_refs(conn, &owners).await?;
    Ok(hidden
        .iter()
        .map(|row| {
            Listed::hidden(HiddenItem {
                id: row.id,
                parent_id: Some(row.node_id),
                kind: "document".to_owned(),
                name: row.title.clone(),
                owner: owners.get(&row.owner_id).cloned(),
                content_hidden: true,
                private: true,
            })
        })
        .collect())
}
