//! Nodes with orgs inside orgs and the two limits of a root org, topics, tags, favorites and recents; what was deleted and when it is purged.
//!
//! This first part is what the access engine reads: a node with the whole
//! path above it, the nodes inside one, and the two limits of a root org.
//! A node's `path` runs from its root org down to the node itself, so the
//! ancestors of many nodes are found by one statement, and so is the kind
//! of every node on the way. Deleted nodes are returned as they are, marked
//! by `deleted_at`: whether a deleted node is a node at all is for the
//! caller to say.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// A node of the hierarchy, as the access engine needs it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct NodeRow {
    /// Its identifier.
    pub id: Id,
    /// `org`, `project`, `topic` or `thread`.
    pub kind: String,
    /// The node it lies in; none for a root org alone.
    pub parent_id: Option<Id>,
    /// Its root org; its own identifier for a root org.
    pub org_id: Id,
    /// The nodes from its root org down to itself, both included.
    pub path: Vec<Id>,
    /// Its name.
    pub name: String,
    /// Its first owner.
    pub owner_id: Id,
    /// When it was archived itself.
    pub archived_at: Option<DateTime<Utc>>,
    /// When it was deleted itself.
    pub deleted_at: Option<DateTime<Utc>>,
    /// How many times it was changed.
    pub revision: i64,
    /// For a thread in an environment: `managed` or `connected`, the kind
    /// of environment it runs in. None for every other node, and for a
    /// thread that has no environment yet.
    pub placement: Option<String>,
    /// The thread's current environment from the same hierarchy observation.
    pub environment_id: Option<Id>,
}

/// Current environments of a frame's threads, using their indexed node identifiers.
pub async fn thread_environments(
    conn: &mut Conn,
    threads: &[Id],
) -> Result<Vec<(Id, Option<Id>)>, DbError> {
    Ok(
        sqlx::query_as("SELECT node_id, environment_id FROM threads WHERE node_id=ANY($1)")
            .bind(threads)
            .fetch_all(conn)
            .await?,
    )
}

/// The nodes `ids` and every node on the way from their root orgs down to
/// them, each once. A node that does not exist is not among them.
pub async fn with_ancestors(conn: &mut Conn, ids: &[Id]) -> Result<Vec<NodeRow>, DbError> {
    // Every permission decision about a node reads it here. In a write whose commit checks the authorization
    // epochs (`transaction_guard`), the epochs of these nodes' root orgs are read in this same statement,
    // so that a change of authority committed after these facts were read refuses that commit.
    Ok(sqlx::query_as(
        "WITH found AS MATERIALIZED (
             SELECT n.id, n.kind, n.parent_id, n.org_id, n.path, n.name, n.owner_id, n.archived_at, n.deleted_at,
                    n.revision, e.placement, t.environment_id
               FROM nodes n
               LEFT JOIN threads t ON t.node_id = n.id
               LEFT JOIN environments e ON e.id = t.environment_id
              WHERE n.id IN (SELECT unnest(path) FROM nodes WHERE id = ANY($1))
         ), watched AS MATERIALIZED (
             SELECT orch_authz_saw(orch_authz_epochs(ARRAY(SELECT DISTINCT org_id FROM found)))
              WHERE COALESCE(current_setting('orch.authz_seen', true), '') <> ''
         )
         SELECT found.* FROM found, (SELECT count(*) FROM watched) w",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The nodes that lie directly in `parent` and are not deleted, in the
/// order they were made, those made after `after` first and at most
/// `limit` of them. Identifiers are ordered by the moment they were made,
/// which is what lets a list page by the last one it showed.
pub async fn children_after(
    conn: &mut Conn,
    parent: Id,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<NodeRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.kind, n.parent_id, n.org_id, n.path, n.name, n.owner_id, n.archived_at, n.deleted_at,
                n.revision, e.placement, t.environment_id
           FROM nodes n
           LEFT JOIN threads t ON t.node_id = n.id
           LEFT JOIN environments e ON e.id = t.environment_id
          WHERE n.parent_id = $1 AND n.deleted_at IS NULL AND ($2::uuid IS NULL OR n.id > $2)
          ORDER BY n.id
          LIMIT $3",
    )
    .bind(parent)
    .bind(after)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// What deleting `place` removes, each with the number a change of it moves on: the place and every
/// node inside it that is not deleted (`org`, `project`, `topic`, `thread`, with their revision),
/// then every document beside them that is not deleted (`document`, with the stamp of its row), each
/// part in the order of its identifiers. With `lock`, the rows are taken FOR UPDATE until the
/// transaction ends, so that nothing changes in the set, and no document is made beside one of its
/// nodes, between this reading and the deletion that compares it.
pub async fn deletion_set(
    conn: &mut Conn,
    place: Id,
    lock: bool,
) -> Result<Vec<(String, Id, i64)>, DbError> {
    let nodes = if lock {
        "SELECT kind, id, revision FROM nodes
          WHERE path @> ARRAY[$1]::uuid[] AND deleted_at IS NULL
          ORDER BY id FOR UPDATE"
    } else {
        "SELECT kind, id, revision FROM nodes
          WHERE path @> ARRAY[$1]::uuid[] AND deleted_at IS NULL
          ORDER BY id"
    };
    let documents = if lock {
        "SELECT 'document'::text, d.id, d.seq_updated FROM documents d JOIN nodes n ON n.id = d.node_id
          WHERE n.path @> ARRAY[$1]::uuid[] AND n.deleted_at IS NULL AND d.deleted_at IS NULL
          ORDER BY d.id FOR UPDATE OF d"
    } else {
        "SELECT 'document'::text, d.id, d.seq_updated FROM documents d JOIN nodes n ON n.id = d.node_id
          WHERE n.path @> ARRAY[$1]::uuid[] AND n.deleted_at IS NULL AND d.deleted_at IS NULL
          ORDER BY d.id"
    };
    let mut set: Vec<(String, Id, i64)> = sqlx::query_as(nodes)
        .bind(place)
        .fetch_all(&mut *conn)
        .await?;
    set.extend(
        sqlx::query_as::<_, (String, Id, i64)>(documents)
            .bind(place)
            .fetch_all(&mut *conn)
            .await?,
    );
    Ok(set)
}

/// Every node inside `place`, at any depth, that is not deleted: what a
/// change of the place's defaults can reach.
pub async fn descendants(conn: &mut Conn, place: Id) -> Result<Vec<NodeRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.kind, n.parent_id, n.org_id, n.path, n.name, n.owner_id, n.archived_at, n.deleted_at,
                n.revision, e.placement, t.environment_id
           FROM nodes n
           LEFT JOIN threads t ON t.node_id = n.id
           LEFT JOIN environments e ON e.id = t.environment_id
          WHERE n.path @> ARRAY[$1]::uuid[] AND n.id <> $1 AND n.deleted_at IS NULL
          ORDER BY n.id",
    )
    .bind(place)
    .fetch_all(conn)
    .await?)
}

/// An org at any level, with what only an org has.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct OrgRow {
    /// The org's node.
    pub node_id: Id,
    /// The org it lies in; none for a root org.
    pub parent_id: Option<Id>,
    /// Its short name, one part of an address.
    pub slug: String,
    /// `personal` or `team`.
    pub org_kind: String,
    /// A root org's: its tree takes no member beyond those it has.
    pub closed: bool,
    /// A root org's: no org is made inside it, and none moved into it.
    pub flat: bool,
}

/// The org whose node is `id`.
pub async fn org(conn: &mut Conn, id: Id) -> Result<Option<OrgRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT node_id, parent_id, slug, org_kind, closed, flat FROM orgs WHERE node_id = $1",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Marks `id` as archived by `by` at `at`, and the node's revision.
/// Nothing happens to a node that is archived already; the answer says
/// whether this call archived it.
pub async fn archive(conn: &mut Conn, id: Id, by: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
    archive_by(conn, id, Some(by), at).await
}

/// Archives with an explicit system actor when `by` is absent, without attributing a schedule to an owner.
pub async fn archive_by(
    conn: &mut Conn,
    id: Id,
    by: Option<Id>,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE nodes SET archived_at = $3, archived_by = $2, revision = revision + 1, updated_at = $3
          WHERE id = $1 AND archived_at IS NULL",
    )
    .bind(id)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Brings `id` back from the archive. The answer says whether it was
/// archived itself.
pub async fn unarchive(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE nodes SET archived_at = NULL, archived_by = NULL, revision = revision + 1, updated_at = $2
          WHERE id = $1 AND archived_at IS NOT NULL",
    )
    .bind(id)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Sets the first owner of `id`, which the Owners row names, and counts
/// the change.
pub async fn set_first_owner(
    conn: &mut Conn,
    id: Id,
    owner: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE nodes SET owner_id = $2, revision = revision + 1, updated_at = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(owner)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Locks the row of `id` until the transaction ends, so that two changes
/// of the one page are made one after the other. Nothing is locked, and
/// nothing said, for a node that is not there.
pub async fn lock(conn: &mut Conn, id: Id) -> Result<bool, DbError> {
    let found: Option<Id> = sqlx::query_scalar("SELECT id FROM nodes WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(conn)
        .await?;
    Ok(found.is_some())
}

// ---- reading nodes whole ----

/// Everything the `nodes` row and its side rows say of a node: what a view
/// of it is made from, apart from what the caller holds on it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct NodeDetail {
    /// Its identifier.
    pub id: Id,
    /// `org`, `project`, `topic` or `thread`.
    pub kind: String,
    /// The node it lies in; none for a root org.
    pub parent_id: Option<Id>,
    /// Its root org.
    pub org_id: Id,
    /// The nodes from its root org down to itself.
    pub path: Vec<Id>,
    /// Its name.
    pub name: String,
    /// Its description.
    pub description: String,
    /// The icon's palette colour, or the colour of its kind when absent.
    pub color: Option<i16>,
    /// A colour of its own for the icon, `#rrggbb`, in place of the palette's.
    pub custom_color: Option<String>,
    /// Who maintains the name: `user` or `auto`.
    pub title_source: String,
    /// Who maintains the description: `user` or `auto`.
    pub description_source: String,
    /// When automation last wrote the name or the description.
    pub auto_updated_at: Option<DateTime<Utc>>,
    /// Its first owner.
    pub owner_id: Id,
    /// When it was archived itself.
    pub archived_at: Option<DateTime<Utc>>,
    /// Who archived it.
    pub archived_by: Option<Id>,
    /// When it was deleted itself.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Who deleted it.
    pub deleted_by: Option<Id>,
    /// How many times it was changed.
    pub revision: i64,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it was last changed.
    pub updated_at: DateTime<Utc>,
    /// The tree sequence of its last change that a tree view shows.
    pub tree_seq: i64,
    /// Its chat, once it has one.
    pub chat_id: Option<Id>,
    /// The sequence of the last message of that chat.
    pub chat_seq: i64,
    /// How many people its Owners row names.
    pub owners: i64,
    /// An org's short name.
    pub slug: Option<String>,
    /// An org's kind: `personal` or `team`.
    pub org_kind: Option<String>,
    /// A root org's limit: it takes no further member.
    pub closed: Option<bool>,
    /// A root org's limit: it holds no org inside it.
    pub flat: Option<bool>,
    /// A topic's status: `open` or `done`.
    pub topic_status: Option<String>,
    /// When a topic was marked done.
    pub done_at: Option<DateTime<Utc>>,
    /// Who maintains this place's assigned tags.
    pub tags_source: Option<String>,
    /// The sequence of a topic's feed.
    pub topic_seq: Option<i64>,
}

/// The nodes `ids` with their side rows, as of `now` for the Owners row
/// (an entry that ended by then names nobody). Deleted nodes are returned
/// as they are; one that is not there is not among them.
pub async fn details(
    conn: &mut Conn,
    ids: &[Id],
    now: DateTime<Utc>,
) -> Result<Vec<NodeDetail>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.kind, n.parent_id, n.org_id, n.path, n.name, n.description, n.color, n.custom_color, n.title_source,
                n.description_source, n.auto_updated_at, n.owner_id, n.archived_at, n.archived_by,
                n.deleted_at, n.deleted_by, n.revision, n.created_at, n.updated_at, n.tree_seq,
                c.id AS chat_id, COALESCE(c.last_message_seq, 0) AS chat_seq,
                (SELECT count(DISTINCT x.person) FROM (
                    SELECT e.holder_id AS person FROM permission_entries e
                     WHERE e.object_kind = 'node' AND e.object_id = n.id AND e.page = 'live'
                       AND e.permission = 'access.own' AND e.removed_at IS NULL
                       AND (e.expires_at IS NULL OR e.expires_at > $2) AND e.holder_kind = 'user'
                    UNION ALL
                    SELECT gm.user_id FROM permission_entries e
                      JOIN group_members gm ON gm.group_id = e.holder_id
                     WHERE e.object_kind = 'node' AND e.object_id = n.id AND e.page = 'live'
                       AND e.permission = 'access.own' AND e.removed_at IS NULL
                       AND (e.expires_at IS NULL OR e.expires_at > $2) AND e.holder_kind = 'group') x) AS owners,
                o.slug, o.org_kind, o.closed, o.flat,
                t.status AS topic_status, t.done_at, n.tags_source, t.topic_seq
           FROM nodes n
           LEFT JOIN chats c ON c.node_id = n.id
           LEFT JOIN orgs o ON o.node_id = n.id
           LEFT JOIN topics t ON t.node_id = n.id
          WHERE n.id = ANY($1)",
    )
    .bind(ids)
    .bind(now)
    .fetch_all(conn)
    .await?)
}

/// What only an org has, counted: its address, how deep it lies, and what
/// is in it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct OrgFacts {
    /// The org's node.
    pub node_id: Id,
    /// The short names from the root org down to it, joined by dots.
    pub address: String,
    /// How many orgs lie above it.
    pub depth: i64,
    /// How many people have it, or an org inside it, as their domain.
    pub member_count: i64,
    /// Distinct active people from native branch membership and inherited Viewer-or-higher roles.
    pub people_count: i64,
    /// The orgs directly inside it.
    pub org_count: i64,
    /// The projects directly inside it.
    pub project_count: i64,
    /// When anything in its root org's tree last changed.
    pub changed_at: DateTime<Utc>,
}

/// The facts of the orgs among `ids`; other nodes are not among them.
pub async fn org_facts(
    conn: &mut Conn,
    ids: &[Id],
    now: DateTime<Utc>,
) -> Result<Vec<OrgFacts>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "WITH people AS MATERIALIZED (
           SELECT requested.id, count(DISTINCT h.user_id) AS total
             FROM access_holder_people_many(
               array_fill('members'::text, ARRAY[cardinality($1::uuid[])]) || array_fill('role'::text, ARRAY[cardinality($1::uuid[])]),
               $1::uuid[] || $1::uuid[],
               array_fill(''::text, ARRAY[cardinality($1::uuid[])]) || array_fill('viewer'::text, ARRAY[cardinality($1::uuid[])]),
               $2, false) h
             JOIN users person ON person.id = h.user_id AND person.kind = 'human'
             JOIN unnest($1::uuid[]) WITH ORDINALITY AS requested(id, ord)
               ON ((h.wanted - 1) % cardinality($1::uuid[])) + 1 = requested.ord
            GROUP BY requested.id
         ) SELECT n.id AS node_id,
                COALESCE((SELECT string_agg(o.slug, '.' ORDER BY p.ord)
                            FROM unnest(n.path) WITH ORDINALITY AS p(id, ord)
                            JOIN orgs o ON o.node_id = p.id), '') AS address,
                (SELECT count(*) FROM unnest(n.path) AS p(id) JOIN orgs o ON o.node_id = p.id) - 1 AS depth,
                (SELECT count(*) FROM org_members m JOIN nodes d ON d.id = m.domain_id
                  WHERE m.org_id = n.org_id AND n.id = ANY(d.path)) AS member_count,
                COALESCE((SELECT total FROM people WHERE people.id = n.id), 0) AS people_count,
                (SELECT count(*) FROM nodes c
                  WHERE c.parent_id = n.id AND c.kind = 'org' AND c.deleted_at IS NULL) AS org_count,
                (SELECT count(*) FROM nodes c
                  WHERE c.parent_id = n.id AND c.kind = 'project' AND c.deleted_at IS NULL) AS project_count,
                (SELECT r.tree_changed_at FROM orgs r WHERE r.node_id = n.org_id) AS changed_at
           FROM nodes n
          WHERE n.id = ANY($1) AND n.kind = 'org'",
    )
    .bind(ids)
    .bind(now)
    .fetch_all(conn)
    .await?)
}

/// How many topics each project among `ids` holds that are not deleted.
pub async fn topic_counts(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, (SELECT count(*) FROM nodes c
                        WHERE c.parent_id = n.id AND c.kind = 'topic' AND c.deleted_at IS NULL)
           FROM nodes n WHERE n.id = ANY($1) AND n.kind = 'project'",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// What the threads of a topic say of it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct TopicFacts {
    /// The topic's node.
    pub node_id: Id,
    /// How many threads it holds, side threads not counted.
    pub thread_count: i64,
    /// Threads with a turn running.
    pub running: i64,
    /// Threads waiting for an approval.
    pub waiting_approval: i64,
    /// Threads that cannot go on.
    pub blocked: i64,
    /// When something last happened in one of them.
    pub last_activity_at: Option<DateTime<Utc>>,
}

/// The thread counts of the topics among `ids`.
pub async fn topic_facts(conn: &mut Conn, ids: &[Id]) -> Result<Vec<TopicFacts>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id AS node_id,
                count(th.node_id) AS thread_count,
                count(th.node_id) FILTER (WHERE th.state = 'running') AS running,
                count(th.node_id) FILTER (WHERE th.state = 'waiting_approval') AS waiting_approval,
                count(th.node_id) FILTER (WHERE th.state = 'blocked') AS blocked,
                max(th.last_activity_at) AS last_activity_at
           FROM nodes n
           LEFT JOIN nodes c ON c.parent_id = n.id AND c.kind = 'thread' AND c.deleted_at IS NULL
           LEFT JOIN threads th ON th.node_id = c.id AND th.side_of IS NULL
          WHERE n.id = ANY($1) AND n.kind = 'topic'
          GROUP BY n.id",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// A tag laid on a topic.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct NodeTagRow {
    /// The topic.
    pub tagged_id: Id,
    /// The tag.
    pub id: Id,
    /// The org or project that defines it.
    pub node_id: Id,
    /// Its name.
    pub name: String,
    /// Its colour, an index of the palette.
    pub color: i16,
}

/// The tags of the topics `topics`, in the order they were defined.
pub async fn node_tags(conn: &mut Conn, topics: &[Id]) -> Result<Vec<NodeTagRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT tt.node_id AS tagged_id, g.id, g.node_id, g.name, g.color
           FROM node_tags tt JOIN tags g ON g.id = tt.tag_id
          WHERE tt.node_id = ANY($1)
          ORDER BY g.created_at, g.id",
    )
    .bind(topics)
    .fetch_all(conn)
    .await?)
}

/// A caller's subscription on a node.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SubscriptionRow {
    /// The node.
    pub node_id: Id,
    /// `all`, `important` or `off`.
    pub level: String,
    /// `auto` or `manual`.
    pub source: String,
}

/// The subscriptions `user` has on the nodes `nodes`.
pub async fn subscriptions_on(
    conn: &mut Conn,
    user: Id,
    nodes: &[Id],
) -> Result<Vec<SubscriptionRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT node_id, level, source FROM subscriptions WHERE user_id = $1 AND node_id = ANY($2)",
    )
    .bind(user)
    .bind(nodes)
    .fetch_all(conn)
    .await?)
}

/// Sets the subscription of `user` on `node` to `level`, by `source`.
pub async fn put_subscription(
    conn: &mut Conn,
    user: Id,
    node: Id,
    level: &str,
    source: &str,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO subscriptions (user_id, node_id, level, source, updated_at)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (user_id, node_id) DO UPDATE
            SET level = EXCLUDED.level, source = EXCLUDED.source, updated_at = EXCLUDED.updated_at",
    )
    .bind(user)
    .bind(node)
    .bind(level)
    .bind(source)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Removes the subscription of `user` on `node`; the answer says whether
/// there was one.
pub async fn delete_subscription(conn: &mut Conn, user: Id, node: Id) -> Result<bool, DbError> {
    let done = sqlx::query("DELETE FROM subscriptions WHERE user_id = $1 AND node_id = $2")
        .bind(user)
        .bind(node)
        .execute(conn)
        .await?;
    Ok(done.rows_affected() == 1)
}

// ---- making and changing nodes ----

/// What an org is beyond a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewOrg<'a> {
    /// Its short name.
    pub slug: &'a str,
    /// `team` or `personal`.
    pub org_kind: &'a str,
}

/// A node to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNode<'a> {
    /// Its identifier.
    pub id: Id,
    /// `org`, `project` or `topic`.
    pub kind: &'a str,
    /// The node it lies in; none for a root org.
    pub parent: Option<Id>,
    /// Its name.
    pub name: &'a str,
    /// Whether a person chose the name, which stops automation writing it.
    pub name_by_person: bool,
    /// Its description.
    pub description: &'a str,
    /// Whether a person wrote the description.
    pub description_by_person: bool,
    /// Its first owner, who is also its maker.
    pub owner: Id,
    /// What an org adds.
    pub org: Option<NewOrg<'a>>,
    /// When it is made.
    pub at: DateTime<Utc>,
}

/// Makes a node with its side row and its chat, and stamps it into the feed
/// of its tree. The Owners row and the epoch are the caller's (the access
/// engine writes them). A parent that is not there is a missing reference.
pub async fn create(conn: &mut Conn, new: &NewNode<'_>) -> Result<(), DbError> {
    let title_source = if new.name_by_person { "user" } else { "auto" };
    let description_source = if new.description_by_person {
        "user"
    } else {
        "auto"
    };
    let inserted = match new.parent {
        None => {
            sqlx::query(
                "INSERT INTO nodes (id, kind, parent_id, org_id, path, name, description, title_source,
                                    description_source, owner_id, created_by, created_at, updated_at)
                 VALUES ($1, $2, NULL, $1, ARRAY[$1]::uuid[], $3, $4, $5, $6, $7, $7, $8, $8)",
            )
            .bind(new.id)
            .bind(new.kind)
            .bind(new.name)
            .bind(new.description)
            .bind(title_source)
            .bind(description_source)
            .bind(new.owner)
            .bind(new.at)
            .execute(&mut *conn)
            .await?
        }
        Some(parent) => {
            sqlx::query(
                "INSERT INTO nodes (id, kind, parent_id, org_id, path, name, description, title_source,
                                    description_source, owner_id, created_by, created_at, updated_at)
                 SELECT $1, $2, p.id, p.org_id, p.path || $1, $3, $4, $5, $6, $7, $7, $8, $8
                   FROM nodes p WHERE p.id = $9 AND p.deleted_at IS NULL",
            )
            .bind(new.id)
            .bind(new.kind)
            .bind(new.name)
            .bind(new.description)
            .bind(title_source)
            .bind(description_source)
            .bind(new.owner)
            .bind(new.at)
            .bind(parent)
            .execute(&mut *conn)
            .await?
        }
    };
    if inserted.rows_affected() != 1 {
        return Err(DbError::Reference {
            constraint: "nodes_parent_id_fkey".to_owned(),
        });
    }
    match (new.kind, new.org) {
        ("org", Some(org)) => {
            sqlx::query(
                "INSERT INTO orgs (node_id, parent_id, slug, org_kind) VALUES ($1, $2, $3, $4)",
            )
            .bind(new.id)
            .bind(new.parent)
            .bind(org.slug)
            .bind(org.org_kind)
            .execute(&mut *conn)
            .await?;
        }
        ("topic", _) => {
            sqlx::query("INSERT INTO topics (node_id) VALUES ($1)")
                .bind(new.id)
                .execute(&mut *conn)
                .await?;
        }
        _ => {}
    }
    sqlx::query(
        "INSERT INTO chats (id, kind, node_id, org_id, created_by, created_at)
         SELECT $1, 'node', n.id, n.org_id, $3, $4 FROM nodes n WHERE n.id = $2",
    )
    .bind(orch_types::new_id())
    .bind(new.id)
    .bind(new.owner)
    .bind(new.at)
    .execute(&mut *conn)
    .await?;
    stamp(conn, new.id).await?;
    Ok(())
}

/// Stamps `node` with the next sequence of the feed of its tree, and of its
/// own feed where it has one (a topic), so that whoever has it open learns
/// that there is something to read. The node is locked now; its root org's
/// next number is taken, the node stamped with it and the tree announced at
/// the transaction's commit (`orch_commit_work`), so that the org's
/// row is never held while the transaction runs. What is returned is the
/// number it takes unless another commit of that tree comes first.
pub async fn stamp(conn: &mut Conn, node: Id) -> Result<i64, DbError> {
    let (_org, seq): (Id, i64) = sqlx::query_as(
        "WITH node AS MATERIALIZED (SELECT id, org_id FROM nodes WHERE id = $1 FOR NO KEY UPDATE)
         SELECT n.org_id, COALESCE((SELECT o.tree_seq FROM orgs o WHERE o.node_id = n.org_id), 0) + 1
           FROM node n WHERE orch_tree_mark(n.org_id, ARRAY[n.id], NULL, NULL)",
    )
    .bind(node)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query("UPDATE topics SET topic_seq = topic_seq + 1 WHERE node_id = $1")
        .bind(node)
        .execute(&mut *conn)
        .await?;
    Ok(seq)
}

/// Streaming work may change many times a second. Five seconds bounds each
/// ancestor's extra writes while keeping the changed order visibly current.
pub const ACTIVITY_ROLLUP_SECONDS: i64 = 5;

/// Rolls an event's clock up its actual path, monotonically and at most once
/// per five seconds at each node. The changed nodes take their tree's next
/// number at commit (`orch_commit_work`). An ancestor that another
/// transaction is rolling up at this moment is left to it (`SKIP LOCKED`): the
/// clocks of a whole org's threads never queue on their org's row.
/// No timestamp is invented from the database clock.
pub async fn touch_activity(conn: &mut Conn, node: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query(
        "WITH chain AS MATERIALIZED (SELECT path,org_id FROM nodes WHERE id=$1 AND deleted_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id=ANY(nodes.path) AND gone.deleted_at IS NOT NULL)),
         due AS MATERIALIZED (
           SELECT n.id FROM nodes n, chain c WHERE n.id=ANY(c.path) AND n.org_id=c.org_id AND n.deleted_at IS NULL
              AND (n.activity_at IS NULL OR n.activity_at <= $2::timestamptz-$3::bigint*interval '1 second')
            ORDER BY n.id FOR NO KEY UPDATE OF n SKIP LOCKED
         ), touched AS (
           UPDATE nodes n SET activity_at=GREATEST(n.activity_at,$2)
             FROM due WHERE n.id=due.id
             RETURNING n.id,n.org_id
         ) SELECT orch_tree_mark(org_id,array_agg(id),NULL,$2) FROM touched GROUP BY org_id",
    ).bind(node).bind(at).bind(ACTIVITY_ROLLUP_SECONDS).execute(&mut *conn).await?;
    Ok(())
}

/// Stamps every node in the subtree of `node`, itself included, with one
/// new sequence of its tree: what a move, a deletion or a restore of the
/// whole subtree amounts to for a tree view.
pub async fn stamp_subtree(conn: &mut Conn, node: Id) -> Result<i64, DbError> {
    // The subtree is locked now and numbered at commit (`orch_commit_work`), as `stamp` is.
    let (_org, seq, _locked): (Id, i64, i64) = sqlx::query_as(
        "WITH locked AS MATERIALIZED (SELECT n.id FROM nodes n WHERE $1 = ANY(n.path) ORDER BY n.id FOR NO KEY UPDATE)
         SELECT r.org_id, COALESCE((SELECT o.tree_seq FROM orgs o WHERE o.node_id = r.org_id), 0) + 1,
                (SELECT count(*) FROM locked)
           FROM nodes r WHERE r.id = $1 AND orch_tree_mark(r.org_id, NULL, ARRAY[r.id], NULL)",
    )
    .bind(node)
    .fetch_one(&mut *conn)
    .await?;
    Ok(seq)
}

/// What a person wrote into a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextChange<'a> {
    /// A new name.
    pub name: Option<&'a str>,
    /// A new description.
    pub description: Option<&'a str>,
}

/// Writes a new name and/or description by a person, if the node is still
/// at `expected` revision. Either one stops automation writing that field.
/// The answer is the new revision, or none when the node moved on (or is
/// not there).
pub async fn change_text(
    conn: &mut Conn,
    id: Id,
    change: TextChange<'_>,
    expected: i64,
    at: DateTime<Utc>,
) -> Result<Option<i64>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE nodes SET name = COALESCE($2, name), description = COALESCE($3, description),
                title_source = CASE WHEN $2::text IS NULL THEN title_source ELSE 'user' END,
                description_source = CASE WHEN $3::text IS NULL THEN description_source ELSE 'user' END,
                revision = revision + 1, updated_at = $5
          WHERE id = $1 AND revision = $4 AND deleted_at IS NULL
          RETURNING revision",
    )
    .bind(id)
    .bind(change.name)
    .bind(change.description)
    .bind(expected)
    .bind(at)
    .fetch_optional(conn)
    .await?)
}

/// What automation keeps up to date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// The name.
    Title,
    /// The description.
    Description,
    /// A topic's tags.
    Tags,
}

/// Hands a field back to automation, so that it writes it again when it
/// next can. Counts as a change of the node.
pub async fn hand_back(
    conn: &mut Conn,
    id: Id,
    field: Field,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let statement = match field {
        Field::Title => {
            "UPDATE nodes SET title_source = 'auto', revision = revision + 1, updated_at = $2 WHERE id = $1"
        }
        Field::Description => {
            "UPDATE nodes SET description_source = 'auto', revision = revision + 1, updated_at = $2 WHERE id = $1"
        }
        Field::Tags => "UPDATE nodes SET revision = revision + 1, updated_at = $2 WHERE id = $1",
    };
    sqlx::query(statement)
        .bind(id)
        .bind(at)
        .execute(&mut *conn)
        .await?;
    if field == Field::Tags {
        sqlx::query("UPDATE nodes SET tags_source = 'auto' WHERE id = $1")
            .bind(id)
            .execute(conn)
            .await?;
    }
    Ok(())
}

/// Sets a topic's status and counts the change on its node. The answer says
/// whether the status was another.
pub async fn set_topic_status(
    conn: &mut Conn,
    topic: Id,
    status: &str,
    by: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE topics SET status = $2, done_at = CASE WHEN $2 = 'done' THEN $4 ELSE NULL END,
                done_by = CASE WHEN $2 = 'done' THEN $3 ELSE NULL END
          WHERE node_id = $1 AND status <> $2",
    )
    .bind(topic)
    .bind(status)
    .bind(by)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 1 {
        sqlx::query("UPDATE nodes SET revision = revision + 1, updated_at = $2 WHERE id = $1")
            .bind(topic)
            .bind(at)
            .execute(conn)
            .await?;
    }
    Ok(done.rows_affected() == 1)
}

/// Replaces the tags of `topic` with `tags` and hands the keeping of them to
/// a person. Counts the change on the node.
pub async fn set_node_tags(
    conn: &mut Conn,
    topic: Id,
    tags: &[Id],
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("DELETE FROM node_tags WHERE node_id = $1")
        .bind(topic)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO node_tags (node_id, tag_id) SELECT DISTINCT $1, t FROM unnest($2::uuid[]) AS t",
    )
    .bind(topic)
    .bind(tags)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE nodes SET tags_source = 'user' WHERE id = $1")
        .bind(topic)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE nodes SET revision = revision + 1, updated_at = $2 WHERE id = $1")
        .bind(topic)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// Moves `node` under `parent`: its path and everything under it is
/// rewritten, an org's own parent follows, and the move counts as a change
/// of the node. The caller has checked that both lie in one tree and that
/// the new parent is not inside the node.
pub async fn move_under(
    conn: &mut Conn,
    node: Id,
    parent: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE nodes SET path = (SELECT p.path FROM nodes p WHERE p.id = $2)
                                 || path[array_position(path, $1)::int:array_upper(path, 1)]
          WHERE $1 = ANY(path)",
    )
    .bind(node)
    .bind(parent)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE nodes SET parent_id = $2, revision = revision + 1, updated_at = $3 WHERE id = $1",
    )
    .bind(node)
    .bind(parent)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE orgs SET parent_id = $2 WHERE node_id = $1")
        .bind(node)
        .bind(parent)
        .execute(conn)
        .await?;
    Ok(())
}

// ---- finding nodes ----

/// The root orgs `user` belongs to or owns that are not deleted, their own
/// first and the rest in the order they were made.
pub async fn root_orgs_of(conn: &mut Conn, user: Id) -> Result<Vec<Id>, DbError> {
    visible_root_orgs(conn, user, false).await
}

/// Native system reach includes every root while that browser session's mode is enabled.
pub async fn visible_root_orgs(
    conn: &mut Conn,
    user: Id,
    as_system_admin: bool,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT n.id FROM nodes n
          WHERE n.parent_id IS NULL AND n.kind = 'org' AND n.deleted_at IS NULL
            AND ($2 OR EXISTS (SELECT 1 FROM org_members m WHERE m.org_id = n.id AND m.user_id = $1)
                 OR n.owner_id = $1
                 OR EXISTS (SELECT 1 FROM permission_entries e
                             WHERE e.object_kind = 'node' AND e.object_id = n.id AND e.page = 'live'
                               AND e.permission = 'access.own' AND e.removed_at IS NULL
                               AND e.holder_kind = 'user' AND e.holder_id = $1))
          ORDER BY (n.id IS NOT DISTINCT FROM (SELECT personal_org_id FROM users WHERE id = $1)) DESC,
                   n.created_at, n.id",
    )
    .bind(user)
    .bind(as_system_admin)
    .fetch_all(conn)
    .await?)
}

/// The org whose address is `address`: the short name of a root org, then
/// the short names of the orgs inside it down to this one, joined by dots.
/// A deleted org is not found.
pub async fn org_by_address(conn: &mut Conn, address: &str) -> Result<Option<Id>, DbError> {
    // Short names are kept in small letters: an address written with capitals names the same org.
    let address = address.to_lowercase();
    let mut found: Option<Id> = None;
    for (index, slug) in address.split('.').enumerate() {
        let next: Option<Id> = if index == 0 {
            sqlx::query_scalar(
                "SELECT o.node_id FROM orgs o JOIN nodes n ON n.id = o.node_id
                  WHERE o.parent_id IS NULL AND o.slug = $1 AND n.deleted_at IS NULL",
            )
            .bind(slug)
            .fetch_optional(&mut *conn)
            .await?
        } else {
            sqlx::query_scalar(
                "SELECT o.node_id FROM orgs o JOIN nodes n ON n.id = o.node_id
                  WHERE o.parent_id = $1 AND o.slug = $2 AND n.deleted_at IS NULL",
            )
            .bind(found)
            .bind(slug)
            .fetch_optional(&mut *conn)
            .await?
        };
        match next {
            Some(id) => found = Some(id),
            None => return Ok(None),
        }
    }
    Ok(found)
}

/// An org found by its short name among the orgs of one parent.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SiblingOrg {
    /// Its node.
    pub id: Id,
    /// Its name, as it is shown.
    pub name: String,
    /// Whether it is deleted: it may still be brought back, with its address.
    pub deleted: bool,
}

/// The org among the orgs of `parent` (the root orgs for `None`) whose short name is `slug`, a deleted one
/// included, since one that is brought back takes its address again: an address names exactly one org.
pub async fn org_with_slug(
    conn: &mut Conn,
    parent: Option<Id>,
    slug: &str,
) -> Result<Option<SiblingOrg>, DbError> {
    Ok(sqlx::query_as(
        "SELECT o.node_id AS id, n.name, n.deleted_at IS NOT NULL AS deleted
           FROM orgs o JOIN nodes n ON n.id = o.node_id
          WHERE o.parent_id IS NOT DISTINCT FROM $1 AND o.slug = $2",
    )
    .bind(parent)
    .bind(slug)
    .fetch_optional(conn)
    .await?)
}

/// The short names among the orgs of `parent` that begin with `stem`, and for the root orgs the usernames that
/// do, which share their namespace: what a short name offered in place of a taken one must not be.
pub async fn slugs_beginning(
    conn: &mut Conn,
    parent: Option<Id>,
    stem: &str,
) -> Result<Vec<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT o.slug FROM orgs o
          WHERE $1::uuid IS NOT NULL AND o.parent_id = $1 AND starts_with(o.slug, $2)
         UNION ALL
         SELECT name FROM public_names
          WHERE $1::uuid IS NULL AND starts_with(name, $2)",
    )
    .bind(parent)
    .bind(stem)
    .fetch_all(conn)
    .await?)
}

/// One node of an org's tree as a tree view shows it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct TreeRow {
    /// Nullable work and immutable failure ordering, projected without a Git request.
    pub attention: Option<orch_types::Json>,
    /// An org's address, absent for the other kinds of node.
    pub address: Option<String>,
    /// Its identifier.
    pub id: Id,
    /// Its kind.
    pub kind: String,
    /// The node it lies in.
    pub parent_id: Option<Id>,
    /// Its name.
    pub name: String,
    /// Its first owner: what the minimal row of a private thing its reader administers names.
    pub owner_id: Id,
    /// The icon's palette colour, or the colour of its kind when absent.
    pub color: Option<i16>,
    /// A colour of its own for the icon, `#rrggbb`, in place of the palette's.
    pub custom_color: Option<String>,
    /// Live documents beside it whose View still follows its documents row.
    pub docs: i64,
    /// Whether it, or a node above it, is archived.
    pub archived: bool,
    /// Whether it was deleted, itself or through a node above.
    pub deleted: bool,
    /// The tree sequence of its last change.
    pub tree_seq: i64,
    /// The latest own or rolled-up activity, including current thread activity.
    pub activity_at: DateTime<Utc>,
    /// A topic's status.
    pub topic_status: Option<String>,
    /// A topic's tags.
    pub tag_ids: Vec<Id>,
    /// A thread's state.
    pub state: Option<String>,
    /// Why a blocked thread cannot go on.
    pub blocked_reason: Option<String>,
    /// Engine diagnostic used only to classify known setup refusals in the readable tree.
    pub blocked_detail: Option<String>,
    /// How many approvals of a thread wait.
    pub pending_approvals: i64,
    /// The thread sequence of its last change worth attention.
    pub activity_seq: Option<i64>,
    /// When something last happened in a thread.
    pub last_activity_at: Option<DateTime<Utc>>,
    /// Its chat.
    pub chat_id: Option<Id>,
    /// The sequence of its chat's last message.
    pub chat_seq: i64,
}

/// The nodes of the tree of the root org `root`: those not deleted when
/// `since` is none, else every node changed after that tree sequence,
/// deleted ones included, at most `limit` changes. A whole read is never capped.
/// The answer is ordered by
/// identifier, that is by the moment a node was made: a node that was moved
/// can come before the node it lies in now, so a reader places nodes by
/// `parent_id` and not by their position in the answer.
pub async fn tree(
    conn: &mut Conn,
    root: Id,
    since: Option<i64>,
    limit: i64,
) -> Result<Vec<TreeRow>, DbError> {
    tree_rows(conn, root, since, since.map(|_| limit), None).await
}

/// One id-keyset page of a full tree. Callers continue until there is no next
/// row, independently of how many rows the reader may view in this page.
pub async fn tree_page(
    conn: &mut Conn,
    root: Id,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<TreeRow>, DbError> {
    tree_rows(conn, root, None, Some(limit), after).await
}

async fn tree_rows(
    conn: &mut Conn,
    root: Id,
    since: Option<i64>,
    limit: Option<i64>,
    after: Option<Id>,
) -> Result<Vec<TreeRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(
        "WITH document_counts AS (
            SELECT d.node_id, count(*) AS docs FROM documents d
             WHERE d.org_id = $1 AND d.deleted_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM permission_rows r
                                WHERE r.object_kind = 'document' AND r.object_id = d.id
                                  AND r.page = 'live' AND r.permission = 'doc.view' AND NOT r.inherit)
             GROUP BY d.node_id
         )
         SELECT n.id, n.kind, n.parent_id, n.name, n.owner_id, n.color, n.custom_color, COALESCE(dc.docs, 0) AS docs,
                CASE WHEN n.kind = 'org' THEN (SELECT string_agg(o.slug, '.' ORDER BY p.ord) FROM unnest(n.path) WITH ORDINALITY p(id, ord) JOIN orgs o ON o.node_id = p.id) END AS address,
                EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.archived_at IS NOT NULL) AS archived,
                EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL) AS deleted,
                n.tree_seq, GREATEST(n.activity_at,n.updated_at,n.created_at,th.last_activity_at) AS activity_at,
                t.status AS topic_status,
                COALESCE((SELECT array_agg(tt.tag_id ORDER BY tt.tag_id) FROM node_tags tt WHERE tt.node_id = n.id),
                         ARRAY[]::uuid[]) AS tag_ids,
                th.state, th.blocked_reason, th.blocked_detail,
                __WORK_ATTENTION__ AS attention,
                (SELECT count(*) FROM approvals a WHERE a.thread_id = n.id AND a.state = 'pending') AS pending_approvals,
                th.activity_seq, th.last_activity_at,
                c.id AS chat_id, COALESCE(c.last_message_seq, 0) AS chat_seq
           FROM nodes n
           LEFT JOIN topics t ON t.node_id = n.id
           LEFT JOIN threads th ON th.node_id = n.id
           LEFT JOIN chats c ON c.node_id = n.id
           LEFT JOIN document_counts dc ON dc.node_id = n.id
          WHERE n.org_id = $1
            AND (($2::bigint IS NULL AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL))
                 OR n.tree_seq > $2)
            AND ($4::uuid IS NULL OR n.id > $4)
          ORDER BY n.id
          LIMIT $3".replace("__WORK_ATTENTION__", super::source::ATTENTION_SQL),
    ))
    .bind(root)
    .bind(since)
    .bind(limit)
    .bind(after)
    .fetch_all(conn)
    .await?)
}

/// The current sequence of the feed of the tree of the root org `root`.
pub async fn tree_seq(conn: &mut Conn, root: Id) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT tree_seq FROM orgs WHERE node_id = $1")
            .bind(root)
            .fetch_one(conn)
            .await?,
    )
}

// ---- tags ----

/// A tag, with the node that defines it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct TagRow {
    /// The tag's identifier.
    pub id: Id,
    /// The org or project that defines it.
    pub node_id: Id,
    /// Its name.
    pub name: String,
    /// Its colour.
    pub color: i16,
}

/// The tags defined by the nodes `nodes`, in the order they were defined.
pub async fn tags_of(conn: &mut Conn, nodes: &[Id]) -> Result<Vec<TagRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, node_id, name, color FROM tags WHERE node_id = ANY($1) ORDER BY created_at, id",
    )
    .bind(nodes)
    .fetch_all(conn)
    .await?)
}

/// The tags with the identifiers `ids`.
pub async fn tags_by_id(conn: &mut Conn, ids: &[Id]) -> Result<Vec<TagRow>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, node_id, name, color FROM tags WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// Defines a tag. A name another tag of the node has, without regard to
/// case, is a taken unique value.
pub async fn create_tag(
    conn: &mut Conn,
    id: Id,
    node: Id,
    name: &str,
    color: i16,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO tags (id, node_id, name, color, created_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(node)
    .bind(name)
    .bind(color)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Changes a tag's name and/or colour.
pub async fn change_tag(
    conn: &mut Conn,
    id: Id,
    name: Option<&str>,
    color: Option<i16>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE tags SET name = COALESCE($2, name), color = COALESCE($3, color) WHERE id = $1",
    )
    .bind(id)
    .bind(name)
    .bind(color)
    .execute(conn)
    .await?;
    Ok(())
}

/// Removes a tag from every topic and deletes it; the answer is the topics
/// that carried it.
pub async fn delete_tag(conn: &mut Conn, id: Id) -> Result<Vec<Id>, DbError> {
    let carried: Vec<Id> =
        sqlx::query_scalar("SELECT node_id FROM node_tags WHERE tag_id = $1 ORDER BY node_id")
            .bind(id)
            .fetch_all(&mut *conn)
            .await?;
    sqlx::query("DELETE FROM tags WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(carried)
}

/// How many tags the topic `topic` carries.
pub async fn tag_count(conn: &mut Conn, topic: Id) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM node_tags WHERE node_id = $1")
            .bind(topic)
            .fetch_one(conn)
            .await?,
    )
}

/// The different people who hold a role in the places `places` by being
/// given one, as a member of a group or an org that was, or as an owner: the
/// people a node inherits a role from when it lies under those places.
pub async fn people_holding_roles(
    conn: &mut Conn,
    places: &[Id],
    now: DateTime<Utc>,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT x.person FROM (
            SELECT r.subject_id AS person FROM node_roles r
             WHERE r.node_id = ANY($1) AND r.removed_at IS NULL AND r.subject_kind = 'user'
               AND (r.expires_at IS NULL OR r.expires_at > $2)
            UNION ALL
            SELECT gm.user_id FROM node_roles r JOIN group_members gm ON gm.group_id = r.subject_id
             WHERE r.node_id = ANY($1) AND r.removed_at IS NULL AND r.subject_kind = 'group'
               AND (r.expires_at IS NULL OR r.expires_at > $2)
            UNION ALL
            SELECT m.user_id FROM node_roles r
              JOIN nodes d ON r.subject_id = ANY(d.path)
              JOIN org_members m ON m.domain_id = d.id
             WHERE r.node_id = ANY($1) AND r.removed_at IS NULL AND r.subject_kind = 'members'
               AND (r.expires_at IS NULL OR r.expires_at > $2)
            UNION ALL
            SELECT e.holder_id FROM permission_entries e
             WHERE e.object_kind = 'node' AND e.object_id = ANY($1) AND e.page = 'live'
               AND e.permission = 'access.own' AND e.removed_at IS NULL AND e.holder_kind = 'user'
         ) x ORDER BY x.person",
    )
    .bind(places)
    .bind(now)
    .fetch_all(conn)
    .await?)
}

/// How many pages inside the node `node`, itself included, follow the
/// defaults of the place they lie in, and name nobody themselves: the pages
/// whose rows a move of the node changes the meaning of.
pub async fn pages_following_defaults(conn: &mut Conn, node: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM nodes n
          WHERE $1 = ANY(n.path) AND n.deleted_at IS NULL
            AND NOT EXISTS (SELECT 1 FROM permission_rows r
                             WHERE r.object_kind = 'node' AND r.object_id = n.id AND r.page = 'live'
                               AND NOT r.inherit)
            AND NOT EXISTS (SELECT 1 FROM permission_entries e
                             WHERE e.object_kind = 'node' AND e.object_id = n.id AND e.page = 'live'
                               AND e.removed_at IS NULL AND e.permission <> 'access.own')",
    )
    .bind(node)
    .fetch_one(conn)
    .await?)
}

// ---- favorites, recents, and what a person is subscribed to ----

/// A favorite of a person.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct FavoriteRow {
    /// The node.
    pub node_id: Id,
    /// Its place among the favorites, as a number written out.
    pub position: String,
    /// For a project starred with a view: that view.
    pub view: Option<orch_types::Json>,
}

/// The favorites of `user`, in order.
pub async fn favorites_of(conn: &mut Conn, user: Id) -> Result<Vec<FavoriteRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id AS node_id, position::text AS position, view FROM favorites
          WHERE user_id = $1 AND kind IN ('org','project','topic','thread') ORDER BY position, kind, id",
    )
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// Where a favorite goes in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placing {
    /// First.
    First,
    /// Last.
    Last,
    /// Right after another favorite.
    After(Id),
}

/// Makes `node` a favorite of `user` at `placing`, or moves it there. The
/// answer says whether the favorite it was to go after is there; nothing is
/// written when it is not. Positions are numbers of any precision, so that a
/// favorite put between two others needs no renumbering of the rest.
pub async fn put_favorite(
    conn: &mut Conn,
    user: Id,
    node: Id,
    placing: Placing,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let kind: Option<String> = sqlx::query_scalar("SELECT kind FROM nodes WHERE id=$1")
        .bind(node)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(kind) = kind else {
        return Ok(false);
    };
    match placing {
        Placing::First => put_item_favorite(conn, user, &kind, node, ItemPlacing::First, at).await,
        Placing::Last => put_item_favorite(conn, user, &kind, node, ItemPlacing::Last, at).await,
        Placing::After(after) => {
            let after_kind: Option<String> =
                sqlx::query_scalar("SELECT kind FROM nodes WHERE id=$1")
                    .bind(after)
                    .fetch_optional(&mut *conn)
                    .await?;
            match after_kind {
                Some(after_kind) => {
                    put_item_favorite(
                        conn,
                        user,
                        &kind,
                        node,
                        ItemPlacing::After {
                            kind: &after_kind,
                            id: after,
                        },
                        at,
                    )
                    .await
                }
                None => Ok(false),
            }
        }
    }
}

/// A position in the one favorites list, whose identities include their kind.
#[derive(Debug, Clone, Copy)]
pub enum ItemPlacing<'a> {
    /// Before every other favorite.
    First,
    /// After every other favorite.
    Last,
    /// Between an existing favorite and its successor.
    After {
        /// Its kind.
        kind: &'a str,
        /// Its identifier.
        id: Id,
    },
}

/// Stars or reorders any readable kind; the route authorizes the target, this writes only personal state.
pub async fn put_item_favorite(
    conn: &mut Conn,
    user: Id,
    kind: &str,
    id: Id,
    placing: ItemPlacing<'_>,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let (position, after_kind, after_id) = match placing {
        ItemPlacing::First => ("first", None, None),
        ItemPlacing::Last => ("last", None, None),
        ItemPlacing::After { kind, id } => ("after", Some(kind), Some(id)),
    };
    Ok(sqlx::query("WITH position AS (SELECT CASE
        WHEN $4::text='first' THEN (SELECT COALESCE(min(position)-1,1) FROM favorites WHERE user_id=$1 AND (kind,id)<>($2,$3))
        WHEN $4='last' THEN (SELECT COALESCE(max(position)+1,1) FROM favorites WHERE user_id=$1 AND (kind,id)<>($2,$3))
        ELSE (SELECT (a.position+COALESCE((SELECT min(b.position) FROM favorites b WHERE b.user_id=$1 AND b.position>a.position AND (b.kind,b.id)<>($2,$3)),a.position+2))/2
              FROM favorites a WHERE a.user_id=$1 AND a.kind=$5 AND a.id=$6 AND (a.kind,a.id)<>($2,$3)) END value)
        INSERT INTO favorites (user_id,kind,id,position,created_at) SELECT $1,$2,$3,value,$7 FROM position WHERE value IS NOT NULL
        ON CONFLICT(user_id,kind,id) DO UPDATE SET position=EXCLUDED.position")
        .bind(user).bind(kind).bind(id).bind(position).bind(after_kind).bind(after_id).bind(at)
        .execute(conn).await?.rows_affected()>0)
}

/// Sets the view a favorite of `user` is starred with: a project's view, or `None` for the place alone.
/// Nothing is written for a node that is no favorite of theirs.
pub async fn set_favorite_view(
    conn: &mut Conn,
    user: Id,
    node: Id,
    view: Option<&orch_types::Json>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE favorites SET view = $3 WHERE user_id = $1 AND id = $2 AND kind=(SELECT kind FROM nodes WHERE id=$2)")
        .bind(user)
        .bind(node)
        .bind(view)
        .execute(conn)
        .await?;
    Ok(())
}

/// Takes `node` out of the favorites of `user`.
pub async fn delete_favorite(conn: &mut Conn, user: Id, node: Id) -> Result<(), DbError> {
    sqlx::query("DELETE FROM favorites WHERE user_id = $1 AND id = $2 AND kind=(SELECT kind FROM nodes WHERE id=$2)")
        .bind(user)
        .bind(node)
        .execute(conn)
        .await?;
    Ok(())
}

/// Notes that `user` went to `node` at `at`, and keeps the `kept` newest
/// visits of theirs.
pub async fn touch_recent(
    conn: &mut Conn,
    user: Id,
    node: Id,
    at: DateTime<Utc>,
    kept: i64,
) -> Result<(), DbError> {
    let kind: Option<String> = sqlx::query_scalar("SELECT kind FROM nodes WHERE id=$1")
        .bind(node)
        .fetch_optional(&mut *conn)
        .await?;
    if let Some(kind) = kind {
        touch_item_recent(conn, user, &kind, node, at, kept).await?;
    }
    Ok(())
}

/// A week's age halves habit weight, so old frequent work yields to current work without discarding its visits.
pub const VISIT_HALF_LIFE_SECONDS: f64 = 7.0 * 24.0 * 60.0 * 60.0;

/// The one time-decaying visit score; expression arguments are source-owned SQL, never user input.
#[must_use]
pub fn visit_score_sql(count: &str, visited: &str, at: &str) -> String {
    format!(
        "COALESCE({count},0) * power(0.5,greatest(0,extract(epoch FROM ({at}::timestamptz-{visited})))/{VISIT_HALF_LIFE_SECONDS})"
    )
}

/// Records one visit to any kind, decaying its stored count before adding the new visit.
pub async fn touch_item_recent(
    conn: &mut Conn,
    user: Id,
    kind: &str,
    id: Id,
    at: DateTime<Utc>,
    kept: i64,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO recents(user_id,kind,id,visited_at,visit_count) VALUES($1,$2,$3,$4,1)
        ON CONFLICT(user_id,kind,id) DO UPDATE SET
          visit_count=recents.visit_count*power(0.5,greatest(0,extract(epoch FROM(EXCLUDED.visited_at-recents.visited_at)))/$5::double precision)+1,
          visited_at=greatest(recents.visited_at,EXCLUDED.visited_at)")
        .bind(user).bind(kind).bind(id).bind(at).bind(VISIT_HALF_LIFE_SECONDS).execute(&mut *conn).await?;
    sqlx::query("DELETE FROM recents WHERE user_id=$1 AND (kind,id) IN(SELECT kind,id FROM recents WHERE user_id=$1 ORDER BY visited_at DESC,kind,id OFFSET $2)")
        .bind(user).bind(kept).execute(conn).await?;
    Ok(())
}

/// Takes one kind/id favorite away without touching another kind with the same identifier.
pub async fn delete_item_favorite(
    conn: &mut Conn,
    user: Id,
    kind: &str,
    id: Id,
) -> Result<(), DbError> {
    sqlx::query("DELETE FROM favorites WHERE user_id=$1 AND kind=$2 AND id=$3")
        .bind(user)
        .bind(kind)
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// The nodes `user` visited last, the latest first.
pub async fn recents_of(conn: &mut Conn, user: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM recents WHERE user_id = $1 AND kind IN ('org','project','topic','thread') ORDER BY visited_at DESC,kind,id",
    )
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// The subscriptions of `user`, those on nodes of the root org `org` only
/// when it is given.
pub async fn subscriptions_of(
    conn: &mut Conn,
    user: Id,
    org: Option<Id>,
) -> Result<Vec<SubscriptionRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT s.node_id, s.level, s.source FROM subscriptions s JOIN nodes n ON n.id = s.node_id
          WHERE s.user_id = $1 AND n.deleted_at IS NULL AND ($2::uuid IS NULL OR n.org_id = $2)
          ORDER BY s.node_id",
    )
    .bind(user)
    .bind(org)
    .fetch_all(conn)
    .await?)
}

/// How far `user` has read their threads, those of the root org `org` only
/// when it is given.
pub async fn read_states_of(
    conn: &mut Conn,
    user: Id,
    org: Option<Id>,
) -> Result<Vec<(Id, Id, i64)>, DbError> {
    Ok(sqlx::query_as(
        "WITH known AS (SELECT DISTINCT thread_id FROM thread_reads WHERE user_id=$1)
         SELECT t.node_id,t.active_continuation_id,COALESCE(r.last_read_seq,0)::bigint
         FROM known k JOIN threads t ON t.node_id=k.thread_id JOIN nodes n ON n.id=t.node_id
         JOIN thread_continuations c ON c.id=t.active_continuation_id AND c.thread_id=t.node_id
         LEFT JOIN thread_reads r ON r.user_id=$1 AND r.thread_id=t.node_id AND r.continuation_id=c.id
         WHERE n.deleted_at IS NULL AND ($2::uuid IS NULL OR n.org_id=$2) ORDER BY t.node_id",
    )
    .bind(user)
    .bind(org)
    .fetch_all(conn)
    .await?)
}

// ---- deleting and bringing back ----

/// Keeps how the node `node` and everything below it that is not deleted yet stand, archived or not
/// and by whom, before a deletion archives them: the deletion's Undo puts that back.
pub async fn keep_archive_states(conn: &mut Conn, node: Id) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE nodes SET archived_before_delete = archived_at, archived_by_before_delete = archived_by
          WHERE $1 = ANY(path) AND deleted_at IS NULL",
    )
    .bind(node)
    .execute(conn)
    .await?;
    Ok(())
}

/// How a node of a deletion stood before it: archived when, and by whom; never archived when `None`.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct KeptArchive {
    /// The node.
    pub id: Id,
    /// Its kind: `org`, `project`, `topic` or `thread`.
    pub kind: String,
    /// Its name, for the line its history is given when it is taken out of the archive again.
    pub name: String,
    /// When it was archived before the deletion.
    pub archived_at: Option<DateTime<Utc>>,
    /// Who had archived it.
    pub archived_by: Option<Id>,
}

/// How each node of the deletion group `group` stood before it, read before the group is brought back.
pub async fn kept_archive_states(conn: &mut Conn, group: Id) -> Result<Vec<KeptArchive>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, kind, name, archived_before_delete AS archived_at, archived_by_before_delete AS archived_by
           FROM nodes WHERE deletion_id = $1 AND deleted_at IS NOT NULL",
    )
    .bind(group)
    .fetch_all(conn)
    .await?)
}

/// Puts back how the nodes stood before a deletion, once they are brought back, and forgets it.
pub async fn put_back_archive_states(conn: &mut Conn, kept: &[KeptArchive]) -> Result<(), DbError> {
    let ids: Vec<Id> = kept.iter().map(|one| one.id).collect();
    let at: Vec<Option<DateTime<Utc>>> = kept.iter().map(|one| one.archived_at).collect();
    let by: Vec<Option<Id>> = kept.iter().map(|one| one.archived_by).collect();
    sqlx::query(
        "UPDATE nodes n SET archived_at = k.archived_at, archived_by = k.archived_by,
                archived_before_delete = NULL, archived_by_before_delete = NULL
           FROM unnest($1::uuid[], $2::timestamptz[], $3::uuid[]) AS k(id, archived_at, archived_by)
          WHERE n.id = k.id AND n.deleted_at IS NULL",
    )
    .bind(&ids)
    .bind(&at)
    .bind(&by)
    .execute(conn)
    .await?;
    Ok(())
}

/// Marks the node `node` and everything below it deleted by `by` at `at`:
/// what is deleted already keeps its own moment, so that bringing the node
/// back brings back what went with it and nothing else.
pub async fn delete_subtree(
    conn: &mut Conn,
    node: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<i64, DbError> {
    delete_subtree_by(conn, node, Some(by), at).await
}

/// The ordinary recoverable deletion, including documents, with a person or the system as actor.
pub async fn delete_subtree_by(
    conn: &mut Conn,
    node: Id,
    by: Option<Id>,
    at: DateTime<Utc>,
) -> Result<i64, DbError> {
    let group = orch_types::new_id();
    let done=sqlx::query("UPDATE nodes SET deleted_at=$3,deleted_by=$2,deletion_id=$4,updated_at=$3 WHERE $1=ANY(path) AND deleted_at IS NULL")
        .bind(node).bind(by).bind(at).bind(group).execute(&mut *conn).await?;
    sqlx::query("WITH removed AS (UPDATE documents d SET deleted_at=$3,deleted_by=$2,updated_at=$3 FROM nodes n WHERE d.node_id=n.id AND n.deletion_id=$4 AND $1=ANY(n.path) AND d.deleted_at IS NULL RETURNING d.id,d.title,d.node_id) INSERT INTO audit_object_state(object_kind,object_id,last_name,deletion_id,deleted_at,deleted_by,home_node_id) SELECT 'document',id,title,$4,$3,$2,node_id FROM removed ON CONFLICT(object_kind,object_id) DO UPDATE SET last_name=EXCLUDED.last_name,deletion_id=EXCLUDED.deletion_id,deleted_at=EXCLUDED.deleted_at,deleted_by=EXCLUDED.deleted_by,purged_at=NULL,purged_by=NULL,home_node_id=EXCLUDED.home_node_id")
        .bind(node).bind(by).bind(at).bind(group).execute(conn).await?;
    Ok(i64::try_from(done.rows_affected()).unwrap_or(i64::MAX))
}

/// Stable pages for retention; orgs and side lanes are never independent age-based targets.
pub async fn retention_page(
    conn: &mut Conn,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT n.id FROM nodes n LEFT JOIN threads t ON t.node_id=n.id
        WHERE n.kind<>'org' AND n.deleted_at IS NULL AND t.side_of IS NULL
          AND ($1::uuid IS NULL OR n.id>$1) ORDER BY n.id LIMIT $2",
    )
    .bind(after)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Recent activity and unfinished turns, re-read after the caller locks the target and its threads.
pub async fn retention_activity(
    conn: &mut Conn,
    node: Id,
) -> Result<(DateTime<Utc>, bool), DbError> {
    Ok(sqlx::query_as("SELECT GREATEST(n.created_at,n.updated_at,
        COALESCE((SELECT max(GREATEST(c.created_at,c.updated_at,t.last_activity_at)) FROM nodes c
            LEFT JOIN threads t ON t.node_id=c.id WHERE $1=ANY(c.path) AND c.deleted_at IS NULL),n.created_at),
        COALESCE((SELECT max(d.updated_at) FROM documents d JOIN nodes c ON c.id=d.node_id
            WHERE $1=ANY(c.path) AND d.deleted_at IS NULL AND c.deleted_at IS NULL),n.created_at)),
        EXISTS(SELECT 1 FROM turns t JOIN nodes c ON c.id=t.thread_id
            WHERE $1=ANY(c.path) AND c.deleted_at IS NULL AND t.state IN ('starting','running'))
        FROM nodes n WHERE n.id=$1")
        .bind(node).fetch_one(conn).await?)
}

/// Gives the root's deletion identity to accepted live members closed during its settlement.
pub async fn include_settled_in_deletion(
    conn: &mut Conn,
    node: Id,
    members: &[Id],
) -> Result<i64, DbError> {
    let done = sqlx::query(
        "UPDATE nodes n SET deletion_id=root.deletion_id FROM nodes root
         WHERE root.id=$1 AND root.deleted_at IS NOT NULL AND root.deletion_id IS NOT NULL
           AND n.id=ANY($2) AND $1=ANY(n.path) AND n.deleted_at IS NOT NULL AND n.deletion_id IS NULL",
    )
    .bind(node)
    .bind(members)
    .execute(conn)
    .await?;
    Ok(i64::try_from(done.rows_affected()).unwrap_or(i64::MAX))
}

/// A node that was deleted, with when and by whom.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct DeletedRow {
    /// The node.
    pub id: Id,
    /// Its kind.
    pub kind: String,
    /// Its name.
    pub name: String,
    /// The node it lay in.
    pub parent_id: Option<Id>,
    /// Its root org.
    pub org_id: Id,
    /// When it was deleted.
    pub deleted_at: DateTime<Utc>,
    /// Who deleted it.
    pub deleted_by: Option<Id>,
    /// How many nodes lay directly in it.
    pub children: i64,
    /// An org's short name.
    pub slug: Option<String>,
}

/// The deleted node `node`, whether it is deleted itself or through a node
/// above it, with the moment of the deletion that took it.
pub async fn deleted_node(conn: &mut Conn, node: Id) -> Result<Option<DeletedRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.kind, n.name, n.parent_id, n.org_id, n.deleted_at, n.deleted_by,
                (SELECT count(*) FROM nodes c WHERE c.parent_id = n.id) AS children, o.slug
           FROM nodes n LEFT JOIN orgs o ON o.node_id = n.id
          WHERE n.id = $1 AND n.deleted_at IS NOT NULL",
    )
    .bind(node)
    .fetch_optional(conn)
    .await?)
}

/// What was deleted directly under the node `parent` after `purged_before`
/// (what was deleted earlier is purged), the latest first, after the node
/// `after` (a moment and an identifier), at most `limit`.
pub async fn deleted_under(
    conn: &mut Conn,
    parent: Id,
    purged_before: DateTime<Utc>,
    after: Option<AuditCursor>,
    limit: i64,
) -> Result<Vec<DeletedRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.kind, n.name, n.parent_id, n.org_id, n.deleted_at, n.deleted_by,
                (SELECT count(*) FROM nodes c WHERE c.parent_id = n.id) AS children, o.slug
           FROM nodes n LEFT JOIN orgs o ON o.node_id = n.id
          WHERE n.parent_id = $1 AND n.deleted_at IS NOT NULL AND n.deleted_at > $2
            AND ($3::uuid IS NULL OR (n.deleted_at, n.id) < ($4, $3))
          ORDER BY n.deleted_at DESC, n.id DESC
          LIMIT $5",
    )
    .bind(parent)
    .bind(purged_before)
    .bind(after.map(|cursor| cursor.id))
    .bind(after.map(|cursor| cursor.at))
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// The team root orgs that `user` owns and that were deleted after
/// `purged_before`, the latest first.
pub async fn deleted_orgs_of(
    conn: &mut Conn,
    user: Id,
    purged_before: DateTime<Utc>,
) -> Result<Vec<DeletedRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.kind, n.name, n.parent_id, n.org_id, n.deleted_at, n.deleted_by,
                (SELECT count(*) FROM nodes c WHERE c.parent_id = n.id) AS children, o.slug
           FROM nodes n JOIN orgs o ON o.node_id = n.id
          WHERE n.parent_id IS NULL AND o.org_kind = 'team'
            AND n.deleted_at IS NOT NULL AND n.deleted_at > $2
            AND (n.owner_id = $1
                 OR EXISTS (SELECT 1 FROM permission_entries e
                             WHERE e.object_kind = 'node' AND e.object_id = n.id AND e.page = 'live'
                               AND e.permission = 'access.own' AND e.removed_at IS NULL
                               AND e.holder_kind = 'user' AND e.holder_id = $1))
          ORDER BY n.deleted_at DESC, n.id DESC",
    )
    .bind(user)
    .bind(purged_before)
    .fetch_all(conn)
    .await?)
}

/// Brings back the node `node` and what was deleted with it, at the moment
/// `deleted_at` that took them: it comes back archived by `by`. The answer
/// is how many nodes came back.
pub async fn restore_subtree(
    conn: &mut Conn,
    node: Id,
    _deleted_at: DateTime<Utc>,
    by: Id,
    at: DateTime<Utc>,
) -> Result<i64, DbError> {
    let group = super::trace::node_deletion(conn, node).await?;
    let restored:Vec<Id>=sqlx::query_scalar("UPDATE nodes SET deleted_at=NULL,deleted_by=NULL,deletion_id=NULL,updated_at=$3,archived_at=CASE WHEN id=$1 THEN COALESCE(archived_at,$3) ELSE archived_at END,archived_by=CASE WHEN id=$1 THEN COALESCE(archived_by,$4) ELSE archived_by END WHERE $1=ANY(path) AND deleted_at IS NOT NULL AND (($2::uuid IS NULL AND id=$1) OR deletion_id=$2) RETURNING id")
        .bind(node).bind(group).bind(at).bind(by).fetch_all(&mut *conn).await?;
    if let Some(group) = group {
        sqlx::query("UPDATE documents d SET deleted_at=NULL,deleted_by=NULL,updated_at=$2 FROM audit_object_state s WHERE s.object_kind='document' AND s.object_id=d.id AND s.deletion_id=$1 AND d.node_id=ANY($3)").bind(group).bind(at).bind(&restored).execute(&mut *conn).await?;
        sqlx::query("UPDATE audit_object_state SET deletion_id=NULL,deleted_at=NULL,deleted_by=NULL,purge_at=NULL WHERE deletion_id=$1 AND ((object_kind='node' AND object_id=ANY($2)) OR (object_kind='document' AND home_node_id=ANY($2)))").bind(group).bind(&restored).execute(&mut *conn).await?;
    }
    Ok(i64::try_from(restored.len()).unwrap_or(i64::MAX))
}

/// Moves the members whose domain is the org `org` or an org inside it to
/// the org `to` above it, giving each the role `role` there unless they
/// hold a higher one by name. The answer is how many moved.
pub async fn move_members_up(
    conn: &mut Conn,
    org: Id,
    to: Id,
    role: &str,
    by: Id,
    at: DateTime<Utc>,
) -> Result<i64, DbError> {
    let moved: Vec<Id> = sqlx::query_scalar(
        "UPDATE org_members m SET domain_id = $2, moved_at = $4, moved_by = $3
           FROM nodes d
          WHERE d.id = m.domain_id AND $1 = ANY(d.path)
          RETURNING m.user_id",
    )
    .bind(org)
    .bind(to)
    .bind(by)
    .bind(at)
    .fetch_all(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO node_roles (id, node_id, subject_kind, subject_id, role, created_by, created_at)
         SELECT gen_random_uuid(), $2, 'user', u, $3, $4, $5 FROM unnest($1::uuid[]) AS u
          WHERE NOT EXISTS (SELECT 1 FROM node_roles r
                             WHERE r.node_id = $2 AND r.subject_kind = 'user' AND r.subject_id = u
                               AND r.removed_at IS NULL)",
    )
    .bind(&moved)
    .bind(to)
    .bind(role)
    .bind(by)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE node_roles r SET role = $3, revision = revision + 1
          WHERE r.node_id = $2 AND r.subject_kind = 'user' AND r.subject_id = ANY($1) AND r.removed_at IS NULL
            AND (CASE r.role WHEN 'admin' THEN 4 WHEN 'developer' THEN 3 WHEN 'interactor' THEN 2 ELSE 1 END)
              < (CASE $3::text WHEN 'admin' THEN 4 WHEN 'developer' THEN 3 WHEN 'interactor' THEN 2 ELSE 1 END)",
    )
    .bind(&moved)
    .bind(to)
    .bind(role)
    .execute(conn)
    .await?;
    Ok(i64::try_from(moved.len()).unwrap_or(i64::MAX))
}

/// How many members have the org `org`, or an org inside it, as their
/// domain.
pub async fn members_inside(conn: &mut Conn, org: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM org_members m JOIN nodes d ON d.id = m.domain_id WHERE $1 = ANY(d.path)",
    )
    .bind(org)
    .fetch_one(conn)
    .await?)
}

// ---- lists of topics and of what was shared ----

/// Which archived topics a list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchivedMode {
    /// None of them.
    Exclude,
    /// All of them, archived ones among.
    Include,
    /// Only the archived ones.
    Only,
}

/// The order of a list of topics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicOrder {
    /// The most recent activity first; a topic that nothing happened in
    /// counts from when it was made.
    Activity,
    /// The most recently made first.
    Created,
    /// By name.
    Name,
}

/// Where a list of topics goes on: the sort key and the identifier of the
/// last topic of the page before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopicCursor {
    /// After a topic of that moment of activity.
    Activity(DateTime<Utc>, Id),
    /// After a topic of that identifier.
    Created(Id),
    /// After a topic of that name.
    Name(String, Id),
}

/// What a list of the topics of a project asks.
#[derive(Debug, Clone)]
pub struct TopicQuery<'a> {
    /// The project.
    pub project: Id,
    /// `open` or `done`; none for both.
    pub status: Option<&'a str>,
    /// Tags that all must be carried.
    pub tags: &'a [Id],
    /// `running`, `waiting_approval` or `blocked`: a thread of the topic is
    /// in that state; `needs_attention`: one is waiting for an approval or
    /// cannot go on.
    pub attention: Option<&'a str>,
    /// Whether the topics of the folders below the project are listed too.
    pub with_folders: bool,
    /// Text the name or the description holds, without regard to case.
    pub text: Option<&'a str>,
    /// Which archived topics are listed.
    pub archived: ArchivedMode,
    /// The order.
    pub order: TopicOrder,
    /// Where it goes on.
    pub after: Option<TopicCursor>,
    /// The most topics.
    pub limit: i64,
}

/// The topics a query asks for, each with the cursor that goes on after it,
/// whether the caller may view them or not: that is for the caller to say.
pub async fn topics_of(
    conn: &mut Conn,
    query: &TopicQuery<'_>,
) -> Result<Vec<(Id, TopicCursor)>, DbError> {
    // The filters are the same for every order; only the key differs.
    macro_rules! base {
        () => {
            "WITH base AS (
        SELECT n.id, n.name, COALESCE(act.last, n.created_at) AS activity
          FROM nodes n
          JOIN topics t ON t.node_id = n.id
          LEFT JOIN LATERAL (
              SELECT max(th.last_activity_at) AS last
                FROM nodes c JOIN threads th ON th.node_id = c.id
               WHERE c.parent_id = n.id AND c.deleted_at IS NULL AND th.side_of IS NULL) act ON true
         WHERE (n.parent_id = $1 OR ($11::boolean AND $1 = ANY(n.path) AND n.id <> $1))
           AND n.kind = 'topic' AND n.deleted_at IS NULL
           AND ($2::text IS NULL OR t.status = $2)
           AND (cardinality($3::uuid[]) = 0
                OR (SELECT count(DISTINCT tt.tag_id) FROM node_tags tt
                     WHERE tt.node_id = n.id AND tt.tag_id = ANY($3)) = cardinality($3::uuid[]))
           AND ($4::text IS NULL
                OR EXISTS (SELECT 1 FROM nodes c JOIN threads th ON th.node_id = c.id
                            WHERE c.parent_id = n.id AND c.deleted_at IS NULL
                              AND th.side_of IS NULL
                              AND (th.state = $4
                                   OR ($4 = 'needs_attention' AND (th.state IN ('waiting_approval', 'blocked')
                                       OR th.work_observed ->> 'uncommitted' = 'true' OR th.work_observed ->> 'unpushed' = 'true'
                                       OR EXISTS (SELECT 1 FROM turns last_turn WHERE last_turn.thread_id = th.node_id
                                           AND last_turn.ordinal = (SELECT max(ordinal) FROM turns WHERE thread_id = th.node_id)
                                           AND last_turn.state = 'failed' AND NOT EXISTS (SELECT 1 FROM thread_inputs i WHERE i.thread_id = th.node_id
                                               AND ((i.seq_created > 0 AND i.seq_created > COALESCE(NULLIF(last_turn.completed_seq, 0), last_turn.seq_updated))
                                                   OR (i.seq_created = 0 AND i.created_at > last_turn.completed_at)))))))))
           AND ($5::text IS NULL
                OR strpos(lower(n.name), lower($5)) > 0 OR strpos(lower(n.description), lower($5)) > 0)
           AND ($6::text = 'include'
                OR ($6 = 'exclude') <> EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.archived_at IS NOT NULL))
    ) "
        };
    }
    let archived = match query.archived {
        ArchivedMode::Exclude => "exclude",
        ArchivedMode::Include => "include",
        ArchivedMode::Only => "only",
    };
    let (key_time, key_text, key_id) = match &query.after {
        None => (None, None, None),
        Some(TopicCursor::Activity(at, id)) => (Some(*at), None, Some(*id)),
        Some(TopicCursor::Created(id)) => (None, None, Some(*id)),
        Some(TopicCursor::Name(name, id)) => (None, Some(name.clone()), Some(*id)),
    };
    let statement: &'static str = match query.order {
        TopicOrder::Activity => concat!(
            base!(),
            "SELECT id, activity, ''::text AS name FROM base
              WHERE $7::uuid IS NULL OR (activity, id) < ($8, $7)
              ORDER BY activity DESC, id DESC LIMIT $10"
        ),
        TopicOrder::Created => concat!(
            base!(),
            "SELECT id, activity, ''::text AS name FROM base
              WHERE $7::uuid IS NULL OR id < $7
              ORDER BY id DESC LIMIT $10"
        ),
        TopicOrder::Name => concat!(
            base!(),
            "SELECT id, activity, name FROM base
              WHERE $7::uuid IS NULL OR (lower(name), id) > (lower($9), $7)
              ORDER BY lower(name), id LIMIT $10"
        ),
    };
    let rows: Vec<(Id, DateTime<Utc>, String)> = sqlx::query_as(statement)
        .bind(query.project)
        .bind(query.status)
        .bind(query.tags)
        .bind(query.attention)
        .bind(query.text)
        .bind(archived)
        .bind(key_id)
        .bind(key_time)
        .bind(key_text)
        .bind(query.limit)
        .bind(query.with_folders)
        .fetch_all(conn)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, activity, name)| {
            let cursor = match query.order {
                TopicOrder::Activity => TopicCursor::Activity(activity, id),
                TopicOrder::Created => TopicCursor::Created(id),
                TopicOrder::Name => TopicCursor::Name(name, id),
            };
            (id, cursor)
        })
        .collect())
}

/// Nodes where `user` is given something by name, which is what could let
/// them into a place whose parent they cannot view: a role given to them, to
/// a group of theirs or to the members of an org of theirs, or an entry of a
/// page that names them or a group of theirs. A root org has no parent, so
/// those the person is not a member of count too. At most `limit`, newest
/// first; `before` is the last scanned candidate, not the last visible one.
pub async fn named_for(
    conn: &mut Conn,
    user: Id,
    groups: &[Id],
    member_of: &[Id],
    now: DateTime<Utc>,
    before: Option<(DateTime<Utc>, Id)>,
    limit: i64,
) -> Result<Vec<(Id, DateTime<Utc>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.created_at FROM nodes n
          WHERE n.deleted_at IS NULL
            AND (n.parent_id IS NOT NULL OR NOT (n.id = ANY($3)))
            AND (EXISTS (SELECT 1 FROM node_roles r
                          WHERE r.node_id = n.id AND r.removed_at IS NULL
                            AND (r.expires_at IS NULL OR r.expires_at > $4)
                            AND ((r.subject_kind = 'user' AND r.subject_id = $1)
                                 OR (r.subject_kind = 'group' AND r.subject_id = ANY($2))
                                 OR (r.subject_kind = 'members' AND r.subject_id = ANY($3))))
                 OR EXISTS (SELECT 1 FROM permission_entries e
                             WHERE e.object_kind = 'node' AND e.object_id = n.id AND e.page = 'live'
                               AND e.removed_at IS NULL AND e.permission <> 'access.own'
                               AND (e.expires_at IS NULL OR e.expires_at > $4)
                               AND ((e.holder_kind = 'user' AND e.holder_id = $1)
                                    OR (e.holder_kind = 'group' AND e.holder_id = ANY($2)))))
            AND ($6::timestamptz IS NULL OR (n.created_at, n.id) < ($6, $7::uuid))
          ORDER BY n.created_at DESC, n.id DESC
          LIMIT $5",
    )
    .bind(user)
    .bind(groups)
    .bind(member_of)
    .bind(now)
    .bind(limit)
    .bind(before.map(|(at, _)| at))
    .bind(before.map(|(_, id)| id))
    .fetch_all(conn)
    .await?)
}

// ---- Activity and the audit trail of a place ----

/// One audit row, as the lists of a place show it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct AuditRow {
    /// Its identifier.
    pub id: Id,
    /// When it happened.
    pub at: DateTime<Utc>,
    /// `user`, `system` or `agent`.
    pub actor_kind: String,
    /// The user, or the thread of an agent.
    pub actor_id: Option<Id>,
    /// The action, a name of the catalogue.
    pub action: String,
    /// Whether it is a line of its object's history.
    pub history: bool,
    /// Whether an admin put themself on a page to do what it did not give.
    #[sqlx(rename = "override")]
    pub r#override: bool,
    /// The kind of thing it was done to.
    pub object_kind: String,
    /// That thing.
    pub object_id: Option<Id>,
    /// The nearest node of the hierarchy.
    pub node_id: Option<Id>,
    /// What happened, in a sentence.
    pub summary: String,
    /// The facts of it.
    pub details: orch_types::Json,
    /// The address it came from.
    pub ip: Option<String>,
}

/// Where a list of audit rows goes on: the moment and the identifier of the
/// last row of the page before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditCursor {
    /// The moment.
    pub at: DateTime<Utc>,
    /// The identifier.
    pub id: Id,
}

/// The lines of the history of the place `place`, newest first: those about
/// it and about what lies directly inside it, after `since` where that is
/// given, at most `limit`. For a project they take in its folders: the lines
/// about what lies directly inside each project inside it as well.
pub async fn activity(
    conn: &mut Conn,
    place: Id,
    since: Option<DateTime<Utc>>,
    after: Option<AuditCursor>,
    limit: i64,
) -> Result<Vec<AuditRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.at, a.actor_kind, a.actor_id, a.action, a.history, a.override, a.object_kind,
                a.object_id, a.node_id, a.summary, a.details, host(a.ip) AS ip
           FROM audit_log a JOIN nodes n ON n.id = a.node_id
          WHERE a.history
            AND (a.node_id = $1 OR n.parent_id = $1
                 OR n.parent_id IN (SELECT f.id FROM nodes f JOIN nodes p ON p.id = $1 AND p.kind = 'project'
                                     WHERE f.kind = 'project' AND f.id <> $1 AND f.deleted_at IS NULL
                                       AND f.path @> ARRAY[$1]::uuid[]))
            AND ($2::timestamptz IS NULL OR a.at > $2)
            AND ($3::uuid IS NULL OR (a.at, a.id) < ($4, $3))
          ORDER BY a.at DESC, a.id DESC
          LIMIT $5",
    )
    .bind(place)
    .bind(since)
    .bind(after.map(|cursor| cursor.id))
    .bind(after.map(|cursor| cursor.at))
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// The lines of the history of the node `node` itself, newest first: the
/// audit rows that are lines of history and are about it, by `actor` where
/// that is given, at most `limit`.
pub async fn node_history(
    conn: &mut Conn,
    node: Id,
    actor: Option<Id>,
    after: Option<AuditCursor>,
    limit: i64,
    actions: Option<&[String]>,
) -> Result<Vec<AuditRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.at, a.actor_kind, a.actor_id, a.action, a.history, a.override, a.object_kind,
                a.object_id, a.node_id, a.summary, a.details, host(a.ip) AS ip
           FROM audit_log a
          WHERE a.history AND a.node_id = $1
            AND ($6::text[] IS NULL OR (a.action = ANY($6) AND COALESCE(a.details->>'page', '') <> 'defaults'))
            AND ($2::uuid IS NULL OR a.actor_id = $2)
            AND ($3::uuid IS NULL OR (a.at, a.id) < ($4, $3))
          ORDER BY a.at DESC, a.id DESC
          LIMIT $5",
    )
    .bind(node)
    .bind(actor)
    .bind(after.map(|cursor| cursor.id))
    .bind(after.map(|cursor| cursor.at))
    .bind(limit)
    .bind(actions)
    .fetch_all(conn)
    .await?)
}

/// What an audit list narrows to.
#[derive(Debug, Clone, Copy, Default)]
pub struct AuditFilter<'a> {
    /// Only what this person did.
    pub actor: Option<Id>,
    /// Only this action.
    pub action: Option<&'a str>,
    /// Only rows from this moment on.
    pub from: Option<DateTime<Utc>>,
    /// Only rows up to this moment.
    pub to: Option<DateTime<Utc>>,
}

/// The audit rows about the node `node` and everything below it, newest
/// first, at most `limit`.
pub async fn audit_trail(
    conn: &mut Conn,
    node: Id,
    filter: AuditFilter<'_>,
    after: Option<AuditCursor>,
    limit: i64,
) -> Result<Vec<AuditRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.at, a.actor_kind, a.actor_id, a.action, a.history, a.override, a.object_kind,
                a.object_id, a.node_id, a.summary, a.details, host(a.ip) AS ip
           FROM audit_log a JOIN nodes n ON n.id = a.node_id
          WHERE $1 = ANY(n.path)
            AND ($2::uuid IS NULL OR a.actor_id = $2)
            AND ($3::text IS NULL OR a.action = $3)
            AND ($4::timestamptz IS NULL OR a.at >= $4)
            AND ($5::timestamptz IS NULL OR a.at <= $5)
            AND ($6::uuid IS NULL OR (a.at, a.id) < ($7, $6))
          ORDER BY a.at DESC, a.id DESC
          LIMIT $8",
    )
    .bind(node)
    .bind(filter.actor)
    .bind(filter.action)
    .bind(filter.from)
    .bind(filter.to)
    .bind(after.map(|cursor| cursor.id))
    .bind(after.map(|cursor| cursor.at))
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

// ---- folders: projects inside projects, and a topic that is moved ----

/// How many projects each of the projects among `ids` holds directly, those deleted not counted.
pub async fn project_counts(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, (SELECT count(*) FROM nodes c
                        WHERE c.parent_id = n.id AND c.kind = 'project' AND c.deleted_at IS NULL)
           FROM nodes n WHERE n.id = ANY($1) AND n.kind = 'project'",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// How many projects lie on the way down to `node`, itself counted when it is one: what a path holds
/// of them one inside another, which `[limits] project_depth` bounds.
pub async fn projects_on_the_way(conn: &mut Conn, node: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM nodes n JOIN nodes p ON p.id = ANY(n.path) AND p.kind = 'project'
          WHERE n.id = $1",
    )
    .bind(node)
    .fetch_one(conn)
    .await?)
}

/// The most projects that lie one inside another within `project`, itself counted: one for a project
/// that holds no folder. A project that is deleted is not counted.
pub async fn deepest_chain_of_projects(conn: &mut Conn, project: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(max((SELECT count(*)
                                FROM unnest(d.path[array_position(d.path, $1)::int:array_upper(d.path, 1)]) AS p(id)
                                JOIN nodes q ON q.id = p.id AND q.kind = 'project')), 0)
           FROM nodes d
          WHERE d.kind = 'project' AND d.deleted_at IS NULL AND d.path @> ARRAY[$1]::uuid[]",
    )
    .bind(project)
    .fetch_one(conn)
    .await?)
}

/// Holds the tree of the root org `root` against any other move until the transaction ends. Two moves
/// that cross, a project into one that lies in a project that is being moved into it, would each pass
/// the check that a place is not inside what is moved and together make a loop; taken one after the
/// other, the second reads the tree the first left.
pub async fn lock_tree_for_moves(conn: &mut Conn, root: Id) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('nodes.move:' || $1::text, 0))")
        .bind(root)
        .execute(conn)
        .await?;
    Ok(())
}

/// A thread that a fork, a switch or a move holds.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct HeldThread {
    /// The thread.
    pub thread: Id,
    /// What it is doing.
    pub state: String,
    /// The kind of the queued or running job that holds it.
    pub job: String,
}

/// The threads of `topic`, side threads among them and deleted ones not, that a fork, a switch or a
/// move holds now. A job holds a thread from its confirmation until it ends, while it is queued or
/// running: a switch holds the thread it replaces, a fork the thread it is making, and the move,
/// recreation or update of an environment every thread bound to it.
pub async fn threads_held_in_topic(conn: &mut Conn, topic: Id) -> Result<Vec<HeldThread>, DbError> {
    // Runtime changes and thread switches ask the same hold function.
    let candidates: Vec<(Id, String)> = sqlx::query_as(
        "SELECT t.node_id AS thread, t.state FROM threads t JOIN nodes n ON n.id = t.node_id
         WHERE t.topic_id = $1 AND n.deleted_at IS NULL ORDER BY t.node_id",
    )
    .bind(topic)
    .fetch_all(&mut *conn)
    .await?;
    let mut held = Vec::new();
    for (thread, state) in candidates {
        if let Some(job) = super::jobs::changes::thread_hold(conn, thread).await? {
            held.push(HeldThread {
                thread,
                state,
                job: job.kind,
            });
        }
    }
    Ok(held)
}

/// How many rows of the ledger and of the two rollups were given another project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repointed {
    /// Rows of the ledger.
    pub ledger: u64,
    /// Rows of the daily rollup.
    pub daily: u64,
    /// Rows of the monthly rollup.
    pub monthly: u64,
}

/// Gives the rows of the ledger and of both rollups that name `topic` the project `project`, which
/// is the one the topic lies in now. Rows name the project their topic lies in directly, so a
/// project's figures are those of the topics it holds; a topic that is moved takes its history with
/// it. Only the column that names the project is written: no figure, and no part of the key of
/// either rollup, changes, so that a rebuild from the ledger gives the rows that are here.
pub async fn repoint_usage(conn: &mut Conn, topic: Id, project: Id) -> Result<Repointed, DbError> {
    let mut changed = [0_u64; 3];
    for (slot, table) in ["usage_events", "usage_daily", "usage_monthly"]
        .into_iter()
        .enumerate()
    {
        let statement =
            format!("UPDATE {table} SET project_id = $2 WHERE topic_id = $1 AND project_id <> $2");
        changed[slot] = sqlx::query(sqlx::AssertSqlSafe(statement))
            .bind(topic)
            .bind(project)
            .execute(&mut *conn)
            .await?
            .rows_affected();
    }
    Ok(Repointed {
        ledger: changed[0],
        daily: changed[1],
        monthly: changed[2],
    })
}

/// Rows written directly, for tests of what stands on them. Not part of
/// any request: a real node is made by the request that makes it.
pub mod testing {
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::Id;

    /// A node of `kind` named `name` inside `parent` (none for a root
    /// org), first owned and made by `owner`. An org gets its side row with
    /// a short name made from `name`, a topic and a thread theirs.
    pub async fn insert_node(
        conn: &mut Conn,
        id: Id,
        kind: &str,
        parent: Option<Id>,
        name: &str,
        owner: Id,
    ) -> Result<(), DbError> {
        match parent {
            None => {
                sqlx::query(
                    "INSERT INTO nodes (id, kind, parent_id, org_id, path, name, owner_id, created_by)
                     VALUES ($1, $2, NULL, $1, ARRAY[$1]::uuid[], $3, $4, $4)",
                )
                .bind(id)
                .bind(kind)
                .bind(name)
                .bind(owner)
                .execute(&mut *conn)
                .await?;
            }
            Some(parent) => {
                sqlx::query(
                    "INSERT INTO nodes (id, kind, parent_id, org_id, path, name, owner_id, created_by)
                     SELECT $1, $2, p.id, p.org_id, p.path || $1, $3, $4, $4 FROM nodes p WHERE p.id = $5",
                )
                .bind(id)
                .bind(kind)
                .bind(name)
                .bind(owner)
                .bind(parent)
                .execute(&mut *conn)
                .await?;
            }
        }
        match kind {
            "org" => {
                let slug = format!(
                    "{}-{}",
                    name.to_lowercase().replace(' ', "-"),
                    &id.simple().to_string()[24..]
                );
                sqlx::query(
                    "INSERT INTO orgs (node_id, parent_id, slug, org_kind) VALUES ($1, $2, $3, 'team')",
                )
                .bind(id)
                .bind(parent)
                .bind(slug)
                .execute(&mut *conn)
                .await?;
            }
            "topic" => {
                sqlx::query("INSERT INTO topics (node_id) VALUES ($1)")
                    .bind(id)
                    .execute(&mut *conn)
                    .await?;
            }
            "thread" => {
                sqlx::query("INSERT INTO threads (node_id, topic_id) VALUES ($1, $2)")
                    .bind(id)
                    .bind(parent)
                    .execute(&mut *conn)
                    .await?;
            }
            _ => {}
        }
        // A node has its chat from the moment it is made.
        sqlx::query(
            "INSERT INTO chats (id, kind, node_id, org_id, created_by)
             SELECT $1, 'node', n.id, n.org_id, $3 FROM nodes n WHERE n.id = $2",
        )
        .bind(orch_types::new_id())
        .bind(id)
        .bind(owner)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Puts the thread `thread` in an environment of the kind `placement`,
    /// `managed` or `connected`, made for `owner` with `home` as its home
    /// node, and returns the environment.
    pub async fn put_thread_in_environment(
        conn: &mut Conn,
        thread: Id,
        placement: &str,
        owner: Id,
        home: Id,
    ) -> Result<Id, DbError> {
        let environment = orch_types::new_id();
        let template: Option<Id> = if placement == "managed" {
            let template = orch_types::new_id();
            sqlx::query("INSERT INTO env_templates (id, name, driver, spec) VALUES ($1, 'test', 'test', '{}')")
                .bind(template)
                .execute(&mut *conn)
                .await?;
            Some(template)
        } else {
            None
        };
        sqlx::query(
            "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, template_id)
             SELECT $1, 'env-' || $1::text, $2, h.org_id, $3, h.id, $4 FROM nodes h WHERE h.id = $5",
        )
        .bind(environment)
        .bind(owner)
        .bind(placement)
        .bind(template)
        .bind(home)
        .execute(&mut *conn)
        .await?;
        sqlx::query("UPDATE threads SET environment_id = $2 WHERE node_id = $1")
            .bind(thread)
            .bind(environment)
            .execute(conn)
            .await?;
        Ok(environment)
    }

    /// Takes the thread out of its environment.
    pub async fn take_thread_out_of_environment(
        conn: &mut Conn,
        thread: Id,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE threads SET environment_id = NULL WHERE node_id = $1")
            .bind(thread)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Marks `id` deleted at `at`, and everything below it.
    pub async fn soft_delete(
        conn: &mut Conn,
        id: Id,
        by: Id,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE nodes SET deleted_at = $3, deleted_by = $2 WHERE $1 = ANY(path)")
            .bind(id)
            .bind(by)
            .bind(at)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Sets the two limits of the root org `id`.
    pub async fn set_limits(
        conn: &mut Conn,
        id: Id,
        closed: bool,
        flat: bool,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE orgs SET closed = $2, flat = $3 WHERE node_id = $1")
            .bind(id)
            .bind(closed)
            .bind(flat)
            .execute(conn)
            .await?;
        Ok(())
    }
}

// ---- Management over HTTP: every org of the system, and the two limits of a root org ----

/// The live orgs of the system, at every level and personal ones among them: team orgs first, then
/// people's own, each in the order of their names (without regard to case) and then of their
/// identifiers; after the org `after` (whether it is personal, its name and identifier), at most
/// `limit`, those whose name holds `q` and whose kind is `org_kind` when those are given.
pub async fn orgs_by_name(
    conn: &mut Conn,
    q: Option<&str>,
    org_kind: Option<&str>,
    after: Option<(bool, &str, Id)>,
    limit: i64,
) -> Result<Vec<(Id, String, bool)>, DbError> {
    // Team orgs first, then people's own; each by name. `personal` is the first key of the order
    // and of the cursor: false comes before true.
    Ok(sqlx::query_as(
        "SELECT n.id, n.name, (o.org_kind = 'personal') AS personal
           FROM nodes n JOIN orgs o ON o.node_id = n.id
          WHERE n.kind = 'org' AND n.deleted_at IS NULL
            AND ($1::text IS NULL OR strpos(lower(n.name), lower($1)) > 0)
            AND ($5::text IS NULL OR o.org_kind = $5)
            AND ($2::text IS NULL
                 OR (o.org_kind = 'personal') > $6
                 OR ((o.org_kind = 'personal') = $6
                     AND (lower(n.name) COLLATE \"C\" > lower($2) COLLATE \"C\"
                          OR (lower(n.name) COLLATE \"C\" = lower($2) COLLATE \"C\" AND n.id > $3))))
          ORDER BY (o.org_kind = 'personal'), lower(n.name) COLLATE \"C\", n.id
          LIMIT $4",
    )
    .bind(q)
    .bind(after.map(|(_, name, _)| name))
    .bind(after.map(|(_, _, id)| id))
    .bind(limit)
    .bind(org_kind)
    .bind(after.map(|(personal, _, _)| personal))
    .fetch_all(conn)
    .await?)
}

/// The team root orgs, whoever owns them, that were deleted after `purged_before` and so are not
/// purged yet, the latest first, after the one deleted at `after.0` with the identifier `after.1`,
/// at most `limit`, those whose name holds `q` when that is given.
pub async fn deleted_root_orgs(
    conn: &mut Conn,
    purged_before: DateTime<Utc>,
    q: Option<&str>,
    after: Option<(DateTime<Utc>, Id)>,
    limit: i64,
) -> Result<Vec<DeletedRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.kind, n.name, n.parent_id, n.org_id, n.deleted_at, n.deleted_by,
                (SELECT count(*) FROM nodes c WHERE c.parent_id = n.id) AS children, o.slug
           FROM nodes n JOIN orgs o ON o.node_id = n.id
          WHERE n.parent_id IS NULL AND o.org_kind = 'team'
            AND n.deleted_at IS NOT NULL AND n.deleted_at > $1
            AND ($2::text IS NULL OR strpos(lower(n.name), lower($2)) > 0)
            AND ($3::timestamptz IS NULL OR (n.deleted_at, n.id) < ($3, $4))
          ORDER BY n.deleted_at DESC, n.id DESC
          LIMIT $5",
    )
    .bind(purged_before)
    .bind(q)
    .bind(after.map(|(at, _)| at))
    .bind(after.map(|(_, id)| id))
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// The live org `id`, locked until the transaction ends so that two changes of its limits are made
/// one after the other.
pub async fn lock_live_org(conn: &mut Conn, id: Id) -> Result<Option<OrgRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT o.node_id, o.parent_id, o.slug, o.org_kind, o.closed, o.flat
           FROM orgs o JOIN nodes n ON n.id = o.node_id
          WHERE o.node_id = $1 AND n.deleted_at IS NULL
            FOR UPDATE OF o",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Sets the limits of the org `id` that are given, and moves the revision of its node on: the org
/// a client holds is out of date.
pub async fn set_org_limits(
    conn: &mut Conn,
    id: Id,
    closed: Option<bool>,
    flat: Option<bool>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE orgs SET closed = COALESCE($2, closed), flat = COALESCE($3, flat) WHERE node_id = $1",
    )
    .bind(id)
    .bind(closed)
    .bind(flat)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE nodes SET revision = revision + 1, updated_at = $2 WHERE id = $1")
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// The kind and the name of each of the nodes `ids`, whether deleted or not: what a line of the
/// audit trail names a node by.
pub async fn node_names(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, String, String)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, kind, name FROM nodes WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// What a read of the audit trail of the whole system narrows to.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemAuditFilter<'a> {
    /// Only what this person did.
    pub actor: Option<Id>,
    /// Only this action.
    pub action: Option<&'a str>,
    /// Only what concerns this node or a node below it.
    pub node: Option<Id>,
    /// Only rows from this moment on.
    pub from: Option<DateTime<Utc>>,
    /// Only rows up to this moment.
    pub to: Option<DateTime<Utc>>,
}

macro_rules! system_audit_statement {
    ($after:literal, $order:literal) => {
        concat!(
            "SELECT a.id, a.at, a.actor_kind, a.actor_id, a.action, a.history, a.override, a.object_kind,
                    a.object_id, a.node_id, a.summary, a.details, host(a.ip) AS ip
               FROM audit_log a LEFT JOIN nodes n ON n.id = a.node_id
              WHERE ($1::uuid IS NULL OR a.actor_id = $1)
                AND ($2::text IS NULL OR a.action = $2)
                AND ($3::uuid IS NULL OR $3 = ANY(n.path))
                AND ($4::timestamptz IS NULL OR a.at >= $4)
                AND ($5::timestamptz IS NULL OR a.at <= $5)
                AND ($6::uuid IS NULL OR (a.at, a.id) ",
            $after,
            " ($7, $6))
              ORDER BY a.at ",
            $order,
            ", a.id ",
            $order,
            " LIMIT $8"
        )
    };
}

/// The audit rows of the whole system, those about no node among them, newest first, after the
/// row `after` (the last of the page before), at most `limit`.
pub async fn system_audit_newest_first(
    conn: &mut Conn,
    filter: SystemAuditFilter<'_>,
    after: Option<AuditCursor>,
    limit: i64,
) -> Result<Vec<AuditRow>, DbError> {
    system_audit(
        conn,
        system_audit_statement!("<", "DESC"),
        filter,
        after,
        limit,
    )
    .await
}

/// The same rows, oldest first, after the row `after`, at most `limit`: how a file is written.
pub async fn system_audit_oldest_first(
    conn: &mut Conn,
    filter: SystemAuditFilter<'_>,
    after: Option<AuditCursor>,
    limit: i64,
) -> Result<Vec<AuditRow>, DbError> {
    system_audit(
        conn,
        system_audit_statement!(">", "ASC"),
        filter,
        after,
        limit,
    )
    .await
}

async fn system_audit(
    conn: &mut Conn,
    statement: &'static str,
    filter: SystemAuditFilter<'_>,
    after: Option<AuditCursor>,
    limit: i64,
) -> Result<Vec<AuditRow>, DbError> {
    Ok(sqlx::query_as(statement)
        .bind(filter.actor)
        .bind(filter.action)
        .bind(filter.node)
        .bind(filter.from)
        .bind(filter.to)
        .bind(after.map(|cursor| cursor.id))
        .bind(after.map(|cursor| cursor.at))
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

// ---------------------------------------------------------------- what a place reaches (giving as a file or a folder is given)

/// The side threads inside `place`, at any depth: threads that are a small conversation beside
/// another thread and no thing of their own in a list of threads, so that what a role held in the
/// place reaches does not count them.
pub async fn side_threads_inside(conn: &mut Conn, place: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT n.id
           FROM nodes n JOIN threads t ON t.node_id = n.id
          WHERE n.path @> ARRAY[$1]::uuid[] AND n.id <> $1 AND n.deleted_at IS NULL
            AND t.side_of IS NOT NULL
          ORDER BY n.id",
    )
    .bind(place)
    .fetch_all(conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::queries::identity::testing::insert_user;
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;

    struct Tree {
        ada: Id,
        root: Id,
        department: Id,
        project: Id,
        topic: Id,
        thread: Id,
    }

    async fn tree(conn: &mut Conn) -> Tree {
        let ada = new_id();
        insert_user(conn, ada, "ada").await.unwrap();
        let (root, department, project, topic, thread) =
            (new_id(), new_id(), new_id(), new_id(), new_id());
        insert_node(conn, root, "org", None, "Acme", ada)
            .await
            .unwrap();
        insert_node(conn, department, "org", Some(root), "Research", ada)
            .await
            .unwrap();
        insert_node(conn, project, "project", Some(department), "Web", ada)
            .await
            .unwrap();
        insert_node(conn, topic, "topic", Some(project), "Login", ada)
            .await
            .unwrap();
        insert_node(conn, thread, "thread", Some(topic), "Fix", ada)
            .await
            .unwrap();
        Tree {
            ada,
            root,
            department,
            project,
            topic,
            thread,
        }
    }

    #[tokio::test]
    async fn a_node_comes_with_every_node_on_the_way_down_to_it() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let t = tree(&mut conn).await;
        let found = with_ancestors(&mut conn, &[t.thread]).await.unwrap();
        let mut ids: Vec<Id> = found.iter().map(|node| node.id).collect();
        ids.sort();
        let mut expected = vec![t.root, t.department, t.project, t.topic, t.thread];
        expected.sort();
        assert_eq!(ids, expected);
        let thread = found.iter().find(|node| node.id == t.thread).unwrap();
        assert_eq!(
            thread.path,
            vec![t.root, t.department, t.project, t.topic, t.thread]
        );
        assert_eq!(thread.org_id, t.root);
        assert_eq!(thread.kind, "thread");
        assert_eq!(thread.owner_id, t.ada);
        // Two nodes of one path are each returned once.
        let both = with_ancestors(&mut conn, &[t.thread, t.topic])
            .await
            .unwrap();
        assert_eq!(both.len(), 5);
        assert!(
            with_ancestors(&mut conn, &[new_id()])
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_thread_tells_where_its_environment_runs() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let t = tree(&mut conn).await;
        let placement = |nodes: Vec<NodeRow>, id: Id| {
            nodes
                .into_iter()
                .find(|node| node.id == id)
                .unwrap()
                .placement
        };
        let nodes = with_ancestors(&mut conn, &[t.thread]).await.unwrap();
        assert_eq!(placement(nodes, t.thread), None, "no environment yet");
        put_thread_in_environment(&mut conn, t.thread, "connected", t.ada, t.topic)
            .await
            .unwrap();
        let nodes = with_ancestors(&mut conn, &[t.thread]).await.unwrap();
        assert_eq!(placement(nodes, t.thread).as_deref(), Some("connected"));
        take_thread_out_of_environment(&mut conn, t.thread)
            .await
            .unwrap();
        put_thread_in_environment(&mut conn, t.thread, "managed", t.ada, t.topic)
            .await
            .unwrap();
        let nodes = with_ancestors(&mut conn, &[t.thread]).await.unwrap();
        assert_eq!(placement(nodes, t.thread).as_deref(), Some("managed"));
    }

    #[tokio::test]
    async fn the_children_of_a_node_page_by_identifier_and_leave_out_the_deleted() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let t = tree(&mut conn).await;
        let mut topics = vec![t.topic];
        for index in 0..4 {
            let topic = new_id();
            insert_node(
                &mut conn,
                topic,
                "topic",
                Some(t.project),
                &format!("T{index}"),
                t.ada,
            )
            .await
            .unwrap();
            topics.push(topic);
        }
        soft_delete(&mut conn, topics[2], t.ada, clock::start())
            .await
            .unwrap();
        let first = children_after(&mut conn, t.project, None, 2).await.unwrap();
        assert_eq!(
            first.iter().map(|node| node.id).collect::<Vec<_>>(),
            [topics[0], topics[1]]
        );
        let rest = children_after(&mut conn, t.project, Some(topics[1]), 10)
            .await
            .unwrap();
        assert_eq!(
            rest.iter().map(|node| node.id).collect::<Vec<_>>(),
            [topics[3], topics[4]]
        );
    }

    #[tokio::test]
    async fn archiving_and_bringing_back_say_whether_they_changed_anything() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let t = tree(&mut conn).await;
        let at = clock::start();
        assert!(archive(&mut conn, t.project, t.ada, at).await.unwrap());
        assert!(!archive(&mut conn, t.project, t.ada, at).await.unwrap());
        let nodes = with_ancestors(&mut conn, &[t.project]).await.unwrap();
        let project = nodes.iter().find(|node| node.id == t.project).unwrap();
        assert_eq!(project.archived_at, Some(at));
        assert_eq!(project.revision, 2);
        assert!(unarchive(&mut conn, t.project, at).await.unwrap());
        assert!(!unarchive(&mut conn, t.project, at).await.unwrap());
    }

    #[tokio::test]
    async fn the_two_limits_of_a_root_org_are_read_with_its_short_name() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let t = tree(&mut conn).await;
        let root = org(&mut conn, t.root).await.unwrap().unwrap();
        assert!(!root.closed && !root.flat && root.parent_id.is_none());
        set_limits(&mut conn, t.root, true, true).await.unwrap();
        let root = org(&mut conn, t.root).await.unwrap().unwrap();
        assert!(root.closed && root.flat);
        let department = org(&mut conn, t.department).await.unwrap().unwrap();
        assert_eq!(department.parent_id, Some(t.root));
        assert!(
            org(&mut conn, t.project).await.unwrap().is_none(),
            "a project is no org"
        );
    }

    #[tokio::test]
    async fn everything_inside_a_place_is_found_at_any_depth_and_the_deleted_are_left_out() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let t = tree(&mut conn).await;
        let found = descendants(&mut conn, t.department).await.unwrap();
        let mut ids: Vec<Id> = found.iter().map(|node| node.id).collect();
        ids.sort();
        let mut expected = vec![t.project, t.topic, t.thread];
        expected.sort();
        assert_eq!(
            ids, expected,
            "not the place itself, and not what is above it"
        );
        soft_delete(&mut conn, t.topic, t.ada, clock::start())
            .await
            .unwrap();
        let found = descendants(&mut conn, t.root).await.unwrap();
        assert_eq!(
            found.len(),
            2,
            "the department and the project; the topic and its thread are deleted"
        );
    }

    #[tokio::test]
    async fn the_first_owner_is_set_and_counted_as_a_change() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let t = tree(&mut conn).await;
        let grace = new_id();
        insert_user(&mut conn, grace, "grace").await.unwrap();
        set_first_owner(&mut conn, t.thread, grace, clock::start())
            .await
            .unwrap();
        let nodes = with_ancestors(&mut conn, &[t.thread]).await.unwrap();
        let thread = nodes.iter().find(|node| node.id == t.thread).unwrap();
        assert_eq!((thread.owner_id, thread.revision), (grace, 2));
        assert!(lock(&mut conn, t.thread).await.unwrap());
        assert!(!lock(&mut conn, new_id()).await.unwrap());
    }
}

// Node presentation and child-kind selection.
pub mod presentation;
