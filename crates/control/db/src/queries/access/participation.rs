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
    candidate: Option<&super::kept::HolderSets>,
) -> Result<Vec<Mark>, DbError> {
    read(
        conn,
        Request {
            user,
            groups,
            members,
            at,
            nodes,
            pages,
            member_roots,
            closed,
            subtree,
            scope_roots,
            resources_only,
            candidate,
        },
        false,
    )
    .await
}

/// The actual assignment read with the same SQL and bindings, including planning and buffer work.
#[expect(
    clippy::too_many_arguments,
    reason = "the plan uses exactly the ordinary frame's dependency bindings"
)]
pub async fn explain_marks(
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
) -> Result<orch_types::Json, DbError> {
    let mut rows: Vec<(orch_types::Json,)> = read(
        conn,
        Request {
            user,
            groups,
            members,
            at,
            nodes,
            pages,
            member_roots,
            closed,
            subtree,
            scope_roots,
            resources_only,
            candidate: None,
        },
        true,
    )
    .await?;
    rows.pop().map(|(plan,)| plan).ok_or_else(|| DbError::Rule {
        constraint: "explain_plan_missing".into(),
    })
}

struct Request<'a> {
    user: Id,
    groups: &'a [Id],
    members: &'a [Id],
    at: Time,
    nodes: &'a [Id],
    pages: &'a [PageKey],
    member_roots: &'a [Id],
    closed: &'a [Id],
    subtree: Option<Id>,
    scope_roots: &'a [Id],
    resources_only: bool,
    candidate: Option<&'a super::kept::HolderSets>,
}

fn statement(sql: String, explain: bool) -> String {
    if explain {
        format!("EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) {sql}")
    } else {
        sql
    }
}

async fn read<R>(conn: &mut Conn, request: Request<'_>, explain: bool) -> Result<Vec<R>, DbError>
where
    R: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Unpin + Send,
{
    let Request {
        user,
        groups,
        members,
        at,
        nodes,
        pages,
        member_roots,
        closed,
        subtree,
        scope_roots,
        resources_only,
        candidate,
    } = request;
    let kinds: Vec<&str> = pages.iter().map(|page| page.object_kind.as_str()).collect();
    let ids: Vec<Id> = pages.iter().map(|page| page.object_id).collect();
    if resources_only
        && nodes.is_empty()
        && pages
            .iter()
            .all(|page| !matches!(page.object_kind.as_str(), "node" | "chat"))
    {
        return Ok(sqlx::query_as(sqlx::AssertSqlSafe(statement(
            include_str!("participation_resources.sql").to_owned(),
            explain,
        )))
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
    let candidate = candidate.filter(|sets| sets.stamp.keepable() && sets.stamp.covers(at.get()));
    let sql = include_str!("participation.sql")
        .replace(
            "__STAMP_HOLDS__",
            &include_str!("stamp_holds.sql")
                .replace("$4", "$15")
                .replace("$5", "$16"),
        )
        .replace(
            "__ROLE_SUBJECT__",
            &role_subject_sql("$1", "$2", "$3", "role_holders"),
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
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(statement(sql, explain)))
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
        .bind(candidate.map(|s| s.held.kinds.as_slice()))
        .bind(candidate.map(|s| s.held.ids.as_slice()))
        .bind(candidate.map(|s| s.held.roles.as_slice()))
        .bind(candidate.map(|s| s.stamp.snapshot.as_str()))
        .bind(candidate.map(|s| s.stamp.database))
        .fetch_all(conn)
        .await?)
}
