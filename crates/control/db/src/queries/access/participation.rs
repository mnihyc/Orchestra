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
/// When the caller specifies its participation targets, a resource-only page reads direct grants
/// and a single unrestricted node reads its descendant count. Other shapes use the full query.
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
    // A node is the home of its own assignments. For one unrestricted node,
    // its descendant count therefore includes both direct and descendant grants.
    // Each count is keyed by the original subject, scope root and expiry bucket.
    let assignments = if resources_only
        && kinds.as_slice() == ["node"]
        && ids.as_slice() == nodes
        && closed.is_empty()
        && subtree.is_none()
    {
        "SELECT 'node' AS kind, p.node_id AS id FROM participation_paths p
          WHERE p.node_id=ANY($5::uuid[]) AND p.live
          AND orch_participation_node_count(p.node_id,$1,$2,$8,$11,$4)>0"
    } else {
        include_str!("participation_assignments.sql")
    };
    let sql = include_str!("participation.sql")
        .replace("__ASSIGNMENTS__", assignments)
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
    // Parameter values share the same prepared statement for each projection shape.
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
        .fetch_all(conn)
        .await?)
}
