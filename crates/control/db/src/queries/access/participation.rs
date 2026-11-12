//! Personal assignment and follow facts for a permission frame, never a grant of access.

use super::{PageKey, role_subject_sql};
use crate::{Conn, DbError, queries::chats};
use orch_types::{Id, Time};

/// A page named by an ordinary assignment or follow, or a requested ancestor of one.
#[derive(Debug, sqlx::FromRow)]
pub struct Mark {
    /// Stored page kind; `chat` also marks the independent chat area of a node.
    pub kind: String,
    /// Object id, or node id for that node's chat area.
    pub id: Id,
}

/// Reads explicit assignments and follows from their indexed person/group side.
/// It does not evaluate View: the caller's frame still decides every returned row.
/// Ordinary thread ownership is aggregated into requested ancestors instead of
/// expanding a hundred thousand owned threads into the permission graph.
/// `resources_only` selects the direct query only with no node targets or node/chat pages;
/// other shapes retain the full query.
#[expect(
    clippy::too_many_arguments,
    reason = "one request's ordinary identity and requested pages are bound together"
)]
pub async fn marks(
    conn: &mut Conn,
    user: Id,
    groups: &[Id],
    members: &[Id],
    at: Time,
    nodes: &[Id],
    pages: &[PageKey],
    member_roots: &[Id],
    closed: &[Id],
    subtree: Option<Id>,
    scope_roots: &[Id],
    resources_only: bool,
) -> Result<Vec<Mark>, DbError> {
    let kinds: Vec<&str> = pages.iter().map(|page| page.object_kind.as_str()).collect();
    let ids: Vec<Id> = pages.iter().map(|page| page.object_id).collect();
    if resources_only
        && nodes.is_empty()
        && pages
            .iter()
            .all(|page| !matches!(page.object_kind.as_str(), "node" | "chat"))
    {
        return Ok(sqlx::query_as(include_str!("participation_resources.sql"))
            .bind(user)
            .bind(groups)
            .bind(at.get())
            .bind(&kinds)
            .bind(&ids)
            .bind(member_roots)
            .bind(closed)
            .bind(subtree)
            .bind(scope_roots)
            .fetch_all(conn)
            .await?);
    }
    let sql = include_str!("participation.sql")
        .replace(
            "__ROLE_SUBJECT__",
            &role_subject_sql("$1", "$2", "$3", "$4"),
        )
        .replace(
            "__NODE_FOLLOW_LEVEL__",
            &chats::node_follow_level_sql("s", "p"),
        )
        .replace(
            "__PEOPLE_FOLLOW_LEVEL__",
            &chats::people_follow_level_sql("s", "p"),
        );
    // The same text at every call: prepared once per connection, so that a frame does not plan it again (planning
    // took 17 ms for 5 ms of work, at every one of a request's frames).
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(user)
        .bind(groups)
        .bind(members)
        .bind(at.get())
        .bind(nodes)
        .bind(kinds)
        .bind(ids)
        .bind(member_roots)
        .bind(closed)
        .bind(subtree)
        .bind(scope_roots)
        .fetch_all(conn)
        .await?)
}
