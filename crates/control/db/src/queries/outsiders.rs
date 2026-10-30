//! People from outside a root tree who reach something in one of its orgs, and each way they do: entries and roles that name them, and those that name a group, an org's members or a role of another place that they belong to.
//!
//! What lies in an org is every node of its subtree, the documents beside
//! them, and the resources whose home is one of them. A source is an active,
//! unexpired entry on the page of such a thing, or a role given at such a node,
//! whose holder is the person: by name (a direct source), or a group, the
//! members of an org or a role held at another place that the person belongs
//! to now (an indirect source). An entry for everyone names nobody and is no
//! source. Whether the person belongs to a holder is the access model's own
//! answer (`access_holders`), so a source here is one that admits them.
//!
//! The people are found the other way round, from what is named here: every
//! holder named on a thing of the org, opened (a group to its members, nested
//! groups too; an org's members to the people whose domain lies in it; a role
//! at a place to whoever is given a role there or above), cut to people who are
//! no member of the tree and no service identity of its own. That set may hold
//! somebody whom no source admits after all (a role of too low a rank); the
//! caller keeps only those with a source.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// The subtree of an org (`$1`), the things that lie in it, and nothing deleted.
const THINGS: &str = "sub AS (
        SELECT n.id, n.name, n.kind FROM nodes n
         WHERE $1 = ANY(n.path) AND n.deleted_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM nodes g WHERE g.id = ANY(n.path) AND g.deleted_at IS NOT NULL)
    ), things(kind, id, name, shown_kind) AS (
        SELECT 'node'::text, sub.id, sub.name, sub.kind FROM sub
        UNION ALL SELECT 'document', d.id, d.title, 'document' FROM documents d JOIN sub ON sub.id = d.node_id
                   WHERE d.deleted_at IS NULL
        UNION ALL SELECT 'provider', r.id, r.name, 'provider' FROM providers r JOIN sub ON sub.id = r.home_id
                   WHERE r.deleted_at IS NULL
        UNION ALL SELECT 'agent', r.id, r.name, 'agent' FROM agents r JOIN sub ON sub.id = r.home_id
                   WHERE r.deleted_at IS NULL
        UNION ALL SELECT 'environment', r.id, r.name, 'environment' FROM environments r
                   JOIN sub ON sub.id = r.home_node_id WHERE r.deleted_at IS NULL
        UNION ALL SELECT 'host', r.id, r.name, 'host' FROM hosts r JOIN sub ON sub.id = r.home_id
        UNION ALL SELECT 'mcp_server', r.id, r.name, 'mcp_server' FROM mcp_servers r JOIN sub ON sub.id = r.home_id
                   WHERE r.deleted_at IS NULL
        UNION ALL SELECT 'mount', r.id, r.name, 'mount' FROM mounts r JOIN sub ON sub.id = r.home_id
                   WHERE r.deleted_at IS NULL
    )";

/// One way a person from outside reaches a thing of an org.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SourceRow {
    /// The entry or the role.
    pub id: Id,
    /// `entry` (a row of a thing's page) or `role` (a role given at a node).
    pub source: String,
    /// Its revision, which a stored plan compares.
    pub revision: i64,
    /// What kind of thing it is on: `node`, `document`, or a kind of resource.
    pub object_kind: String,
    /// The thing.
    pub object_id: Id,
    /// The thing's name.
    pub object_name: String,
    /// What a reader calls the kind of the thing: a node's own kind (`org`, `project`, `topic`,
    /// `thread`), else the kind of its page.
    pub object_shown_kind: String,
    /// The permission of the row, or `role:<role>` for a role.
    pub row: String,
    /// How the holder is named: `user`, `group`, `members` or `role`.
    pub holder_kind: String,
    /// The group, the user, the org or the place.
    pub holder_id: Option<Id>,
    /// The role a role holder names, that one or higher.
    pub holder_role: Option<String>,
    /// The group's or the place's name, where the holder is one.
    pub holder_name: Option<String>,
    /// Who gave it; none for what was seeded.
    pub created_by: Option<Id>,
    /// When it was given.
    pub created_at: DateTime<Utc>,
    /// When it ends.
    pub expires_at: Option<DateTime<Utc>>,
    /// It names the person themself.
    pub direct: bool,
}

/// Every way the person `person` reaches a thing of the org `org` now, by the identifier of the
/// entry or the role, after `after` and at most `limit` of them.
pub async fn sources(
    conn: &mut Conn,
    org: Id,
    person: Id,
    now: DateTime<Utc>,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<SourceRow>, DbError> {
    let sql = format!(
        "WITH {THINGS},
         me AS (SELECT kind, id, role FROM access_holders($2, $3)),
         ranks(role, rank) AS (VALUES ('viewer', 1), ('interactor', 2), ('developer', 3), ('admin', 4)),
         found AS (
            SELECT e.id, 'entry'::text AS source, e.revision, t.kind AS object_kind, t.id AS object_id,
                   t.name AS object_name, t.shown_kind AS object_shown_kind, e.permission AS row,
                   e.holder_kind, e.holder_id, e.holder_role,
                   e.created_by, e.created_at, e.expires_at,
                   (e.holder_kind = 'user' AND e.holder_id = $2) AS direct
              FROM permission_entries e JOIN things t ON t.kind = e.object_kind AND t.id = e.object_id
             WHERE e.removed_at IS NULL AND (e.expires_at IS NULL OR e.expires_at > $3)
               AND ((e.holder_kind = 'user' AND e.holder_id = $2)
                 OR (e.holder_kind = 'group' AND EXISTS (
                       SELECT 1 FROM me WHERE me.kind = 'group' AND me.id = e.holder_id))
                 OR (e.holder_kind = 'members' AND EXISTS (
                       SELECT 1 FROM me WHERE me.kind = 'members' AND me.id = e.holder_id))
                 OR (e.holder_kind = 'role' AND e.holder_id IS NOT NULL AND EXISTS (
                       SELECT 1 FROM me JOIN ranks mine ON mine.role = me.role
                                        JOIN ranks asked ON asked.role = e.holder_role
                        WHERE me.kind = 'role' AND me.id = e.holder_id AND mine.rank >= asked.rank)))
            UNION ALL
            SELECT r.id, 'role', r.revision, 'node', sub.id, sub.name, sub.kind, 'role:' || r.role, r.subject_kind,
                   r.subject_id, r.subject_role, r.created_by, r.created_at, r.expires_at,
                   (r.subject_kind = 'user' AND r.subject_id = $2)
              FROM node_roles r JOIN sub ON sub.id = r.node_id
             WHERE r.removed_at IS NULL AND (r.expires_at IS NULL OR r.expires_at > $3)
               AND ((r.subject_kind = 'user' AND r.subject_id = $2)
                 OR (r.subject_kind = 'group' AND EXISTS (
                       SELECT 1 FROM me WHERE me.kind = 'group' AND me.id = r.subject_id))
                 OR (r.subject_kind = 'members' AND EXISTS (
                       SELECT 1 FROM me WHERE me.kind = 'members' AND me.id = r.subject_id))
                 OR (r.subject_kind = 'role' AND EXISTS (
                       SELECT 1 FROM me JOIN ranks mine ON mine.role = me.role
                                        JOIN ranks asked ON asked.role = r.subject_role
                        WHERE me.kind = 'role' AND me.id = r.subject_id AND mine.rank >= asked.rank)))
         )
         SELECT f.id, f.source, f.revision, f.object_kind, f.object_id, f.object_name, f.object_shown_kind, f.row,
                f.holder_kind, f.holder_id, f.holder_role,
                COALESCE(g.name, h.name) AS holder_name,
                f.created_by, f.created_at, f.expires_at, f.direct
           FROM found f
           LEFT JOIN groups g ON f.holder_kind = 'group' AND g.id = f.holder_id
           LEFT JOIN nodes h ON f.holder_kind IN ('members', 'role') AND h.id = f.holder_id
          WHERE ($4::uuid IS NULL OR f.id > $4)
          ORDER BY f.id
          LIMIT $5"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(org)
        .bind(person)
        .bind(now)
        .bind(after)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// A person from outside who may reach something of an org, as the list pages them.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CandidateRow {
    /// The person.
    pub id: Id,
    /// Their username, by which the list is ordered.
    pub username: String,
}

/// The people who are no member of the tree of `root`, nor a service identity of one of its orgs,
/// and whom a holder named on a thing of the org `org` may stand for now; those whose name or
/// username holds `q`, after `after` (a username and an identifier), at most `limit`, by username.
pub async fn candidates(
    conn: &mut Conn,
    org: Id,
    root: Id,
    now: DateTime<Utc>,
    q: Option<&str>,
    after: Option<(&str, Id)>,
    limit: i64,
) -> Result<Vec<CandidateRow>, DbError> {
    let sql = format!(
        "WITH RECURSIVE {THINGS},
         named(kind, id) AS (
            SELECT e.holder_kind, e.holder_id
              FROM permission_entries e JOIN things t ON t.kind = e.object_kind AND t.id = e.object_id
             WHERE e.removed_at IS NULL AND (e.expires_at IS NULL OR e.expires_at > $3)
               AND e.holder_kind IN ('user', 'group', 'members', 'role') AND e.holder_id IS NOT NULL
            UNION
            SELECT r.subject_kind, r.subject_id FROM node_roles r JOIN sub ON sub.id = r.node_id
             WHERE r.removed_at IS NULL AND (r.expires_at IS NULL OR r.expires_at > $3)
         ), opened(kind, id) AS (
            SELECT kind, id FROM named
            UNION
            SELECT next.kind, next.id FROM opened x CROSS JOIN LATERAL (
                SELECT CASE WHEN m.user_id IS NOT NULL THEN 'user'
                            WHEN m.member_group_id IS NOT NULL THEN 'group'
                            WHEN m.member_org_id IS NOT NULL THEN 'members' ELSE 'role' END AS kind,
                       COALESCE(m.user_id, m.member_group_id, m.member_org_id, m.member_node_id) AS id
                  FROM group_members m WHERE x.kind = 'group' AND m.group_id = x.id
                UNION ALL
                SELECT 'user', om.user_id FROM org_members om JOIN nodes d ON d.id = om.domain_id
                 WHERE x.kind = 'members' AND x.id = ANY(d.path)
                UNION ALL
                SELECT r.subject_kind, r.subject_id FROM nodes p JOIN node_roles r ON r.node_id = ANY(p.path)
                 WHERE x.kind = 'role' AND p.id = x.id AND r.removed_at IS NULL
                   AND (r.expires_at IS NULL OR r.expires_at > $3)
            ) next
         )
         SELECT DISTINCT u.id, u.username
           FROM opened x JOIN users u ON u.id = x.id
          WHERE x.kind = 'user'
            AND NOT EXISTS (SELECT 1 FROM org_members m WHERE m.org_id = $2 AND m.user_id = u.id)
            AND NOT EXISTS (SELECT 1 FROM nodes s WHERE s.id = u.service_org_id AND s.org_id = $2)
            AND ($4::text IS NULL OR u.username ILIKE '%' || $4 || '%' OR u.display_name ILIKE '%' || $4 || '%')
            AND ($5::text IS NULL OR (u.username, u.id) > ($5, $6))
          ORDER BY u.username, u.id
          LIMIT $7"
    );
    let (after_name, after_id) = after.map_or((None, None), |(name, id)| (Some(name), Some(id)));
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(org)
        .bind(root)
        .bind(now)
        .bind(q)
        .bind(after_name)
        .bind(after_id)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Those of `users` who are held out of the org `org` by a suspension.
pub async fn suspended_among(conn: &mut Conn, org: Id, users: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT user_id FROM org_suspensions WHERE org_id = $1 AND user_id = ANY($2)",
    )
    .bind(org)
    .bind(users)
    .fetch_all(conn)
    .await?)
}

/// Locks the entries and roles `ids` against a change until the caller's transaction ends, so that
/// what a confirmed plan compares is what it removes.
pub async fn lock_sources(conn: &mut Conn, entries: &[Id], roles: &[Id]) -> Result<(), DbError> {
    sqlx::query("SELECT id FROM permission_entries WHERE id = ANY($1) ORDER BY id FOR UPDATE")
        .bind(entries)
        .fetch_all(&mut *conn)
        .await?;
    sqlx::query("SELECT id FROM node_roles WHERE id = ANY($1) ORDER BY id FOR UPDATE")
        .bind(roles)
        .fetch_all(&mut *conn)
        .await?;
    Ok(())
}

/// The values `sharing.external_max_days` is set to, in whole days: at the system (`None`) and at each node
/// that sets it. The policy only narrows wherever it is set, so the bound at a place is the fewest of those
/// on its way; the access engine reads them where it decides, for a person from outside the tree.
pub async fn external_max_days(conn: &mut Conn) -> Result<Vec<(Option<Id>, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT node_id, floor((value #>> '{}')::numeric)::bigint
           FROM settings
          WHERE key = 'sharing.external_max_days' AND jsonb_typeof(value) = 'number'",
    )
    .fetch_all(conn)
    .await?)
}
