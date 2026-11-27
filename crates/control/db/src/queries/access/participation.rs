//! Personal assignment and follow facts for a permission frame, never a grant of access.

use super::{PageKey, role_subject_sql};
use crate::{Conn, DbError, queries::chats};
use orch_types::{Id, Time};

/// A page named by an ordinary assignment or follow, or a requested ancestor of one.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Mark {
    /// Stored page kind; `chat` also marks the independent chat area of a node.
    pub kind: String,
    /// Object id, or node id for that node's chat area.
    pub id: Id,
}

/// All SQL bindings of an assignment/follow read in one snapshot. Membership lists are sets.
#[derive(Hash, Eq, PartialEq)]
pub(in crate::queries) struct MarkSources {
    user: Id,
    at: Time,
    groups: Vec<Id>,
    members: Vec<Id>,
    nodes: Vec<Id>,
    pages: Vec<(String, Id)>,
    member_roots: Vec<Id>,
    closed: Vec<Id>,
    subtree: Option<Id>,
    scope_roots: Vec<Id>,
    resources_only: bool,
}

fn sorted<T: Ord>(values: impl Iterator<Item = T>) -> Vec<T> {
    let mut values: Vec<_> = values.collect();
    values.sort_unstable();
    values.dedup();
    values
}

/// Reads explicit assignments and follows from their indexed person/group side.
/// It does not evaluate View: the caller's frame still decides every returned row.
/// Ordinary thread ownership is aggregated into requested ancestors instead of
/// expanding a hundred thousand owned threads into the permission graph.
/// When the caller specifies its participation targets, a resource-only page reads direct grants
/// and unrestricted nodes read their descendant counts. Other shapes use the full query.
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
    let key = crate::queries::read_facts::with_snapshot(conn, |_| MarkSources {
        user,
        at,
        groups: sorted(groups.iter().copied()),
        members: sorted(members.iter().copied()),
        nodes: sorted(nodes.iter().copied()),
        pages: sorted(
            pages
                .iter()
                .map(|page| (page.object_kind.clone(), page.object_id)),
        ),
        member_roots: sorted(member_roots.iter().copied()),
        closed: sorted(closed.iter().copied()),
        subtree,
        scope_roots: sorted(scope_roots.iter().copied()),
        resources_only,
    });
    if let Some(key) = &key
        && let Some(marks) = crate::queries::read_facts::with_snapshot(conn, |facts| {
            facts.participation.get(key).cloned()
        })
        .flatten()
    {
        return Ok(marks);
    }
    let marks: Vec<Mark> = read(
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
    .await?;
    if let Some(key) = key {
        let _ = crate::queries::read_facts::with_snapshot(conn, |facts| {
            facts.participation.insert(key, marks.clone());
        });
    }
    Ok(marks)
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

/// Asks PostgreSQL for one plan of the summaries per connection, for the next statement of this transaction alone,
/// and says what to put back after it. Left to itself PostgreSQL plans the summaries again at every call: their
/// generic plan is estimated for ten members in each of the reader's sets, so it never looks cheaper than a plan made
/// for the actual sets, though it runs about as fast. On the team fixture the summaries spent 2.7-4.1 ms planning a
/// call against 0.3-0.5 ms running; their generic plan runs in 1.0-2.3 ms and is made once (D-3146). The setting is
/// the transaction's own (`set_config(…, true)`), so a failed statement's rollback puts it back too; outside a
/// transaction the summaries are planned as before.
async fn one_plan(conn: &mut Conn) -> Result<Option<String>, DbError> {
    use sqlx::Connection as _;
    if !conn.is_in_transaction() {
        return Ok(None);
    }
    // The current mode is read before it is set: the setting names what it was read from.
    Ok(sqlx::query_scalar(
        "SELECT was.mode FROM (SELECT current_setting('plan_cache_mode') AS mode) was,
           LATERAL (SELECT set_config('plan_cache_mode', 'force_generic_plan', true) WHERE was.mode IS NOT NULL) kept",
    )
    .fetch_optional(&mut *conn)
    .await?)
}

async fn read<R>(conn: &mut Conn, request: Request<'_>, explain: bool) -> Result<Vec<R>, DbError>
where
    R: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Unpin + Send,
{
    let was = if explain { None } else { one_plan(conn).await? };
    let rows = select(conn, request, explain).await?;
    if let Some(was) = was {
        sqlx::query("SELECT set_config('plan_cache_mode', $1, true)")
            .bind(was)
            .execute(&mut *conn)
            .await?;
    }
    Ok(rows)
}

async fn select<R>(conn: &mut Conn, request: Request<'_>, explain: bool) -> Result<Vec<R>, DbError>
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
    // A node's descendant summaries already include its direct grants. Without
    // subtree or suspension exclusions, any one current positive bucket suffices.
    let assignments = if resources_only
        && !nodes.is_empty()
        && kinds.iter().all(|kind| *kind == "node")
        && ids.as_slice() == nodes
        && closed.is_empty()
        && subtree.is_none()
    {
        include_str!("participation_nodes.sql")
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
