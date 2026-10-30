//! Document-only container rows, exact membership, structural locks and committed operation receipts.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A real folder, with no editable document payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct ContainerRow {
    /// Its identity.
    pub id: Id,
    /// The node beside which its document tree stands.
    pub node_id: Id,
    /// That node's root organization.
    pub org_id: Id,
    /// Its containing folder, or the virtual docs root.
    pub parent_container_id: Option<Id>,
    /// Its accountable owner.
    pub owner_id: Id,
    /// Original person, when a person made it.
    pub created_by: Option<Id>,
    /// Calling thread, when an agent made it.
    pub created_by_thread: Option<Id>,
    /// Agent name at creation.
    pub created_by_agent: Option<String>,
    /// Its single path component.
    pub name: String,
    /// Its current revision.
    pub revision: i64,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last change time.
    pub updated_at: DateTime<Utc>,
    /// Current deletion time.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Person accountable for deletion.
    pub deleted_by: Option<Id>,
    /// Recursive operation which deleted it.
    pub deleted_in: Option<Id>,
}

const CONTAINER_COLUMNS: &str = "c.id,c.node_id,c.org_id,c.parent_container_id,c.owner_id,c.created_by,
 c.created_by_thread,c.created_by_agent,c.name,c.revision,c.created_at,c.updated_at,c.deleted_at,c.deleted_by,c.deleted_in";

/// One stored member of the common document namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct EntryRow {
    /// `document` or `container`.
    pub kind: String,
    /// Native identity.
    pub id: Id,
    /// Anchor node.
    pub node_id: Id,
    /// Root organization.
    pub org_id: Id,
    /// Immediate folder, or the virtual root.
    pub parent_container_id: Option<Id>,
    /// Exact path component.
    pub basename: String,
    /// Human title.
    pub title: String,
    /// Current content/container revision.
    pub revision: i64,
    /// Accountable owner.
    pub owner_id: Id,
    /// Current deletion.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Its deletion cohort.
    pub deleted_in: Option<Id>,
    /// Current attached-file count.
    pub files: i64,
    /// Retained version count.
    pub versions: i64,
}

const ENTRIES: &str = "SELECT 'document'::text AS kind,d.id,d.node_id,d.org_id,d.parent_container_id,
 d.basename,d.title,d.doc_seq AS revision,d.owner_id,d.deleted_at,d.deleted_in,
 (SELECT count(*) FROM document_files f WHERE f.document_id=d.id) AS files,
 (SELECT count(*) FROM document_versions v WHERE v.document_id=d.id) AS versions FROM documents d
 UNION ALL SELECT 'container',c.id,c.node_id,c.org_id,c.parent_container_id,c.name,c.name,c.revision,
 c.owner_id,c.deleted_at,c.deleted_in,0::bigint,0::bigint FROM document_containers c";

/// The existing shared-tree page budget; all continuations are followed, with no whole-reach cap.
pub const TREE_READ_PAGE: i64 = 20_000;

/// Reads a folder, including one retained after deletion.
pub async fn get(conn: &mut Conn, id: Id) -> Result<Option<ContainerRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {CONTAINER_COLUMNS} FROM document_containers c WHERE c.id=$1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Locks the actual folder scope before its row, rejecting a concurrent anchor change.
pub async fn lock(conn: &mut Conn, id: Id) -> Result<Option<ContainerRow>, DbError> {
    let Some(before) = get(conn, id).await? else {
        return Ok(None);
    };
    lock_scopes(conn, &[before.node_id]).await?;
    let row: Option<ContainerRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {CONTAINER_COLUMNS} FROM document_containers c WHERE c.id=$1 FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?;
    if row
        .as_ref()
        .is_some_and(|row| row.node_id != before.node_id)
    {
        return Err(DbError::Retry);
    }
    Ok(row)
}

/// Replaces a folder's first owner after its ordinary ownership/page authorization.
pub async fn set_owner(
    conn: &mut Conn,
    id: Id,
    owner: Id,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE document_containers SET owner_id=$2,updated_at=$3 WHERE id=$1")
        .bind(id)
        .bind(owner)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Publishes a folder's current metadata/permission revision through its anchor tree.
pub async fn touch(conn: &mut Conn, id: Id, now: DateTime<Utc>) -> Result<(), DbError> {
    let Some(row) = lock(conn, id).await? else {
        return Ok(());
    };
    sqlx::query("UPDATE document_containers SET updated_at=$2 WHERE id=$1")
        .bind(id)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    crate::queries::hierarchy::stamp(conn, row.node_id).await?;
    Ok(())
}

/// Purge takes the same structural scopes before it locks/removes owning rows or their pins.
pub async fn lock_purge_scopes(conn: &mut Conn, table: &str, ids: &[Id]) -> Result<(), DbError> {
    let nodes = purge_nodes(conn, table, ids).await?;
    lock_scopes(conn, &nodes).await?;
    if purge_nodes(conn, table, ids)
        .await?
        .iter()
        .any(|id| !nodes.contains(id))
    {
        return Err(DbError::Retry);
    }
    Ok(())
}

async fn purge_nodes(conn: &mut Conn, table: &str, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(match table {
        "nodes" => ids.to_vec(),
        "documents" => {
            sqlx::query_scalar("SELECT DISTINCT node_id FROM documents WHERE id=ANY($1)")
                .bind(ids)
                .fetch_all(&mut *conn)
                .await?
        }
        "document_containers" => {
            sqlx::query_scalar("SELECT DISTINCT node_id FROM document_containers WHERE id=ANY($1)")
                .bind(ids)
                .fetch_all(&mut *conn)
                .await?
        }
        _ => Vec::new(),
    })
}

/// Capture only surviving document anchors before a successful purge removes their owning rows.
pub(crate) async fn purge_anchors(
    conn: &mut Conn,
    table: &str,
    ids: &[Id],
) -> Result<Vec<Id>, DbError> {
    if matches!(table, "documents" | "document_containers") {
        purge_nodes(conn, table, ids).await
    } else {
        Ok(Vec::new())
    }
}

/// Reads a batch of folders without filtering their authority or deletion state.
pub async fn get_many(conn: &mut Conn, ids: &[Id]) -> Result<Vec<ContainerRow>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {CONTAINER_COLUMNS} FROM document_containers c WHERE c.id=ANY($1) ORDER BY c.id"
    )))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// All container ancestors of the named document-domain identities, including selected containers.
pub async fn ancestors_many(conn: &mut Conn, ids: &[Id]) -> Result<Vec<ContainerRow>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH RECURSIVE ancestors AS (
          SELECT c.* FROM document_containers c WHERE c.id=ANY($1)
          UNION SELECT p.* FROM document_containers p JOIN documents d ON d.parent_container_id=p.id WHERE d.id=ANY($1)
          UNION SELECT p.* FROM document_containers p JOIN ancestors a ON p.id=a.parent_container_id)
         SELECT {CONTAINER_COLUMNS} FROM ancestors c ORDER BY c.id")))
        .bind(ids).fetch_all(conn).await?)
}

/// Immediate folder identities for text documents, without changing DocumentRow's content interface.
pub async fn document_parents(
    conn: &mut Conn,
    ids: &[Id],
) -> Result<HashMap<Id, Option<Id>>, DbError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Id, Option<Id>)> =
        sqlx::query_as("SELECT id,parent_container_id FROM documents WHERE id=ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?;
    Ok(rows.into_iter().collect())
}

/// Reads one exact typed entry, including a retained deletion.
pub async fn entry(conn: &mut Conn, kind: &str, id: Id) -> Result<Option<EntryRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT * FROM ({ENTRIES}) e WHERE e.id=$1 AND e.kind=$2"
    )))
    .bind(id)
    .bind(kind)
    .fetch_optional(conn)
    .await?)
}

/// Complete siblings for namespace and authorization checks; the caller applies visibility afterward.
pub async fn entries_in(
    conn: &mut Conn,
    node: Id,
    parent: Option<Id>,
    deleted: bool,
) -> Result<Vec<EntryRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT * FROM ({ENTRIES}) e WHERE e.node_id=$1 AND e.parent_container_id IS NOT DISTINCT FROM $2
         AND (e.deleted_at IS NOT NULL)=$3 ORDER BY e.basename,e.kind,e.id")))
        .bind(node).bind(parent).bind(deleted).fetch_all(conn).await?)
}

/// A scope's membership generation, including the empty directory before its first writer.
pub async fn scope_seq(conn: &mut Conn, node: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE((SELECT seq FROM document_tree_scopes WHERE node_id=$1),0)",
    )
    .bind(node)
    .fetch_one(conn)
    .await?)
}

/// Small retained facts for an already authorized bounded page, never whole replay receipts.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RetentionRow {
    /// Native entry kind, document or container.
    pub kind: String,
    /// Exact entry identity.
    pub id: Id,
    /// Original accountable deleting person where one exists.
    pub deleted_by: Option<Id>,
    /// The stored lifecycle promise.
    pub purge_at: Option<DateTime<Utc>>,
    /// Actual collection, when recorded.
    pub purged_at: Option<DateTime<Utc>>,
    /// The cohort receipt's original promise for a legacy lifecycle projection.
    pub cohort_until: Option<DateTime<Utc>>,
}

/// Batch counterpart of the native single-entry retained deadline's metadata reads.
pub async fn retention_rows(conn: &mut Conn, ids: &[Id]) -> Result<Vec<RetentionRow>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as("WITH wanted AS (
          SELECT 'document'::text AS kind,id,deleted_by,deleted_in FROM documents WHERE id=ANY($1) AND deleted_at IS NOT NULL
          UNION ALL SELECT 'container',id,deleted_by,deleted_in FROM document_containers WHERE id=ANY($1) AND deleted_at IS NOT NULL)
        SELECT w.kind,w.id,w.deleted_by,s.purge_at,s.purged_at,(o.result->>'restore_until')::timestamptz AS cohort_until
          FROM wanted w LEFT JOIN audit_object_state s ON s.object_id=w.id
            AND s.object_kind=CASE WHEN w.kind='container' THEN 'document_container' ELSE 'document' END
          LEFT JOIN document_tree_operations o ON o.operation_id=w.deleted_in")
        .bind(ids).fetch_all(conn).await?)
}

/// Bounded internal scan progress. Retained inventory follows document ancestry, not node ancestry.
/// The offset is sealed in the API cursor; skipped private names are never a continuation key.
pub async fn page_candidates(
    conn: &mut Conn,
    node: Id,
    parent: Option<Id>,
    deleted: bool,
    offset: i64,
    limit: i64,
) -> Result<Vec<EntryRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH RECURSIVE folder_scope AS (
            SELECT id FROM document_containers WHERE id=$2
            UNION ALL SELECT c.id FROM document_containers c JOIN folder_scope p ON c.parent_container_id=p.id)
         SELECT * FROM ({ENTRIES}) e WHERE e.node_id=$1 AND (e.deleted_at IS NOT NULL)=$3
           AND (CASE WHEN $3 THEN $2::uuid IS NULL OR e.parent_container_id IN(SELECT id FROM folder_scope)
                ELSE e.parent_container_id IS NOT DISTINCT FROM $2 END)
         ORDER BY e.basename COLLATE \"C\",e.kind,e.id OFFSET $4 LIMIT $5")))
        .bind(node).bind(parent).bind(deleted).bind(offset).bind(limit).fetch_all(conn).await?)
}

/// Scoped candidates for a shared-tree batch; exact identities are included independently of node visibility.
pub async fn candidates(
    conn: &mut Conn,
    nodes: &[Id],
    documents: &[Id],
    folders: &[Id],
) -> Result<Vec<EntryRow>, DbError> {
    if nodes.is_empty() && documents.is_empty() && folders.is_empty() {
        return Ok(Vec::new());
    }
    let mut document_scope = Vec::new();
    let mut folder_scope = Vec::new();
    if !nodes.is_empty() {
        document_scope.push("d.node_id=ANY($1::uuid[])");
        folder_scope.push("c.node_id=ANY($1::uuid[])");
    }
    if !documents.is_empty() {
        document_scope.push("d.id=ANY($2::uuid[])");
    }
    if !folders.is_empty() {
        document_scope.push("d.parent_container_id IN(SELECT id FROM scoped_folders)");
        folder_scope.push("c.id IN(SELECT id FROM scoped_folders) OR c.parent_container_id IN(SELECT id FROM scoped_folders)");
    }
    let document_scope = if document_scope.is_empty() {
        "false".to_owned()
    } else {
        document_scope.join(" OR ")
    };
    let folder_scope = if folder_scope.is_empty() {
        "false".to_owned()
    } else {
        folder_scope.join(" OR ")
    };
    let mut found = Vec::new();
    let mut cursor: Option<(Id, String, Id)> = None;
    loop {
        let page:Vec<EntryRow>=sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH RECURSIVE scoped_folders AS (
           SELECT id FROM document_containers WHERE id=ANY($3::uuid[])
           UNION SELECT c.id FROM document_containers c JOIN scoped_folders p ON c.parent_container_id=p.id)
         SELECT e.* FROM (
           (SELECT 'document'::text kind,d.id,d.node_id FROM documents d WHERE ({document_scope})
             AND ($1::uuid[] IS NOT NULL AND $2::uuid[] IS NOT NULL)
             AND ($4::uuid IS NULL OR (d.node_id,'document'::text,d.id)>($4,$5::text,$6::uuid))
             ORDER BY d.node_id,d.id LIMIT $7)
           UNION ALL
           (SELECT 'container'::text kind,c.id,c.node_id FROM document_containers c WHERE ({folder_scope})
             AND ($4::uuid IS NULL OR (c.node_id,'container'::text,c.id)>($4,$5::text,$6::uuid))
             ORDER BY c.node_id,c.id LIMIT $7)
         ) selected CROSS JOIN LATERAL (
           SELECT * FROM ({ENTRIES}) e WHERE e.kind=selected.kind AND e.id=selected.id OFFSET 0
         ) e ORDER BY e.node_id,e.kind,e.id LIMIT $7")))
            .bind(nodes).bind(documents).bind(folders).bind(cursor.as_ref().map(|value|value.0))
            .bind(cursor.as_ref().map(|value|value.1.as_str())).bind(cursor.as_ref().map(|value|value.2))
            .bind(TREE_READ_PAGE).fetch_all(&mut *conn).await?;
        let complete = i64::try_from(page.len()).unwrap_or(i64::MAX) < TREE_READ_PAGE;
        cursor = page
            .last()
            .map(|row| (row.node_id, row.kind.clone(), row.id));
        found.extend(page);
        if complete {
            return Ok(found);
        }
    }
}

/// One live folder by exact name, using the node/parent/name index without loading its siblings.
pub async fn folder_named(
    conn: &mut Conn,
    node: Id,
    parent: Option<Id>,
    name: &str,
) -> Result<Option<Id>, DbError> {
    Ok(if let Some(parent) = parent {
        sqlx::query_scalar("SELECT id FROM document_containers WHERE node_id=$1 AND parent_container_id=$2 AND name=$3 AND deleted_at IS NULL")
            .bind(node).bind(parent).bind(name).fetch_optional(conn).await?
    } else {
        sqlx::query_scalar("SELECT id FROM document_containers WHERE node_id=$1 AND parent_container_id IS NULL AND name=$2 AND deleted_at IS NULL")
            .bind(node).bind(name).fetch_optional(conn).await?
    })
}

/// Cross-kind namespace test under the scope lock; hidden entries are tested but never returned.
pub async fn name_taken(
    conn: &mut Conn,
    node: Id,
    parent: Option<Id>,
    name: &str,
    except: Option<(&str, Id)>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(
        SELECT 1 FROM documents d WHERE d.node_id=$1 AND d.parent_container_id IS NOT DISTINCT FROM $2 AND d.deleted_at IS NULL
          AND translate(d.basename,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')=translate($3,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')
          AND NOT (COALESCE($4,'')='document' AND d.id IS NOT DISTINCT FROM $5)
        UNION ALL SELECT 1 FROM document_containers c WHERE c.node_id=$1 AND c.parent_container_id IS NOT DISTINCT FROM $2 AND c.deleted_at IS NULL
          AND translate(c.name,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')=translate($3,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')
          AND NOT (COALESCE($4,'')='container' AND c.id IS NOT DISTINCT FROM $5))")
        .bind(node).bind(parent).bind(name).bind(except.map(|item|item.0)).bind(except.map(|item|item.1)).fetch_one(conn).await?)
}

/// Reads a cohort's committed receipt without exposing another actor's request key.
pub async fn operation_by_id(conn: &mut Conn, id: Id) -> Result<Option<Operation>, DbError> {
    Ok(sqlx::query_as(
        "SELECT arguments,operation_id,result FROM document_tree_operations WHERE operation_id=$1",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Next time an authority/audience grant can end without an epoch mutation.
/// Using the global boundary is conservative and never accepts a changed deletion audience.
pub async fn authority_until(
    conn: &mut Conn,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, DbError> {
    Ok(sqlx::query_scalar("SELECT min(expires_at) FROM (
        SELECT expires_at FROM permission_entries WHERE removed_at IS NULL AND expires_at>$1
        UNION ALL SELECT expires_at FROM node_roles WHERE removed_at IS NULL AND expires_at>$1) grants")
        .bind(now).fetch_one(conn).await?)
}

/// Exact relevant stored page state, hashed by the domain rather than disclosed in a preview.
pub async fn authority_state(conn: &mut Conn, items: &[Id], nodes: &[Id]) -> Result<Json, DbError> {
    let mut ids = items.to_vec();
    ids.extend(
        ancestors_many(conn, items)
            .await?
            .into_iter()
            .map(|row| row.id),
    );
    let places: Vec<Id> =
        sqlx::query_scalar("SELECT DISTINCT unnest(path) FROM nodes WHERE id=ANY($1)")
            .bind(nodes)
            .fetch_all(&mut *conn)
            .await?;
    Ok(sqlx::query_scalar("SELECT jsonb_build_array(
      COALESCE((SELECT jsonb_agg(to_jsonb(r) ORDER BY object_kind,object_id,page,profile,permission) FROM permission_rows r
        WHERE object_kind='system' OR (object_kind='node' AND object_id=ANY($2))
          OR (object_kind IN ('document','document_container') AND object_id=ANY($1))),'[]'::jsonb),
      COALESCE((SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM permission_entries e
        WHERE object_kind='system' OR (object_kind='node' AND object_id=ANY($2))
          OR (object_kind IN ('document','document_container') AND object_id=ANY($1))),'[]'::jsonb))")
        .bind(ids).bind(places).fetch_one(conn).await?)
}

/// Bounded breadth-first closure. Returning limit+1 means the complete operation is too large.
pub async fn subtree(
    conn: &mut Conn,
    roots: &[EntryRow],
    limit: usize,
) -> Result<Vec<EntryRow>, DbError> {
    let mut all = roots.to_vec();
    let mut visited: HashSet<(String, Id)> =
        all.iter().map(|row| (row.kind.clone(), row.id)).collect();
    let mut frontier: Vec<Id> = all
        .iter()
        .filter(|row| row.kind == "container")
        .map(|row| row.id)
        .collect();
    while !frontier.is_empty() && all.len() <= limit {
        let remaining =
            i64::try_from(limit.saturating_add(1).saturating_sub(all.len())).unwrap_or(i64::MAX);
        let children: Vec<EntryRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT * FROM ({ENTRIES}) e WHERE e.parent_container_id=ANY($1) LIMIT $2"
        )))
        .bind(&frontier)
        .bind(remaining)
        .fetch_all(&mut *conn)
        .await?;
        frontier.clear();
        for row in children {
            if !visited.insert((row.kind.clone(), row.id)) {
                return Err(DbError::Rule {
                    constraint: "document_container_cycle_or_overlap".into(),
                });
            }
            if row.kind == "container" {
                frontier.push(row.id);
            }
            all.push(row);
        }
    }
    all.sort_by(|a, b| (&a.kind, a.id).cmp(&(&b.kind, b.id)));
    Ok(all)
}

/// Acquires all document-scope locks in stable order on this transaction connection.
pub async fn lock_scopes(conn: &mut Conn, nodes: &[Id]) -> Result<(), DbError> {
    if nodes.is_empty() {
        return Ok(());
    }
    sqlx::query("SELECT document_tree_lock($1)")
        .bind(nodes)
        .execute(conn)
        .await?;
    Ok(())
}

/// Locks the actual current document scope before its content row; a concurrent reparent retries.
pub async fn lock_document_scope(conn: &mut Conn, id: Id) -> Result<(), DbError> {
    let node: Option<Id> = sqlx::query_scalar("SELECT node_id FROM documents WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    if let Some(node) = node {
        lock_scopes(conn, &[node]).await?;
        let current: Option<Id> = sqlx::query_scalar("SELECT node_id FROM documents WHERE id=$1")
            .bind(id)
            .fetch_optional(conn)
            .await?;
        if current != Some(node) {
            return Err(DbError::Retry);
        }
    }
    Ok(())
}

/// Locks every member's current row after the scope locks have excluded structural phantoms.
pub async fn lock_entries(conn: &mut Conn, entries: &[EntryRow]) -> Result<(), DbError> {
    let mut folders: Vec<_> = entries
        .iter()
        .filter(|row| row.kind == "container")
        .map(|row| row.id)
        .collect();
    let mut documents: Vec<_> = entries
        .iter()
        .filter(|row| row.kind == "document")
        .map(|row| row.id)
        .collect();
    folders.sort_unstable();
    documents.sort_unstable();
    sqlx::query("SELECT id FROM document_containers WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(&folders)
        .execute(&mut *conn)
        .await?;
    sqlx::query("SELECT id FROM documents WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(&documents)
        .execute(conn)
        .await?;
    Ok(())
}

/// A committed actor-scoped operation answer.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Operation {
    /// Exact canonical arguments.
    pub arguments: Json,
    /// Stable operation identity.
    pub operation_id: Id,
    /// Its committed receipt.
    pub result: Json,
}

/// Reads an existing operation only within the exact actor namespace.
pub async fn operation(
    conn: &mut Conn,
    actor_kind: &str,
    actor: Id,
    request: &str,
) -> Result<Option<Operation>, DbError> {
    Ok(sqlx::query_as("SELECT arguments,operation_id,result FROM document_tree_operations WHERE actor_kind=$1 AND actor_id=$2 AND request_ref=$3")
        .bind(actor_kind).bind(actor).bind(request).fetch_optional(conn).await?)
}

/// Stores the receipt in the same transaction as all its changes.
pub async fn remember(
    conn: &mut Conn,
    actor_kind: &str,
    actor: Id,
    request: &str,
    operation: &Operation,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO document_tree_operations(actor_kind,actor_id,request_ref,arguments,operation_id,result,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(actor_kind).bind(actor).bind(request).bind(&operation.arguments).bind(operation.operation_id).bind(&operation.result).bind(now)
        .execute(conn).await?;
    Ok(())
}

/// New folder material; ordinary ownership is supplied by the authorized destination producer.
pub struct NewContainer<'a> {
    /// Native identity.
    pub id: Id,
    /// Anchor node.
    pub node: Id,
    /// Root organization.
    pub root: Id,
    /// Immediate folder.
    pub parent: Option<Id>,
    /// Accountable owner.
    pub owner: Id,
    /// Calling thread for an agent producer.
    pub thread: Option<Id>,
    /// Agent name at creation.
    pub agent: Option<&'a str>,
    /// Exact basename.
    pub name: &'a str,
    /// Current time.
    pub now: DateTime<Utc>,
}

/// Inserts a real empty folder; the common schema guard protects every producer.
pub async fn insert(conn: &mut Conn, new: &NewContainer<'_>) -> Result<(), DbError> {
    sqlx::query("INSERT INTO document_containers(id,node_id,org_id,parent_container_id,owner_id,created_by,created_by_thread,created_by_agent,name,created_at,updated_at)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$10)")
        .bind(new.id).bind(new.node).bind(new.root).bind(new.parent).bind(new.owner).bind(new.thread.is_none().then_some(new.owner))
        .bind(new.thread).bind(new.agent).bind(new.name).bind(new.now).execute(&mut *conn).await?;
    crate::queries::hierarchy::stamp(conn, new.node).await?;
    Ok(())
}

/// Normal text creation in a typed parent, using the same stored identity and content producer.
pub async fn insert_document(
    conn: &mut Conn,
    new: &super::NewDocument<'_>,
    parent: Option<Id>,
    basename: &str,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO documents(id,node_id,org_id,parent_container_id,basename,owner_id,created_by_thread,created_by_agent,title,created_at,updated_at,updated_by,created_by)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$10,$11,$11)")
        .bind(new.id).bind(new.node_id).bind(new.org_id).bind(parent).bind(basename).bind(new.owner_id).bind(new.created_by_thread).bind(new.created_by_agent)
        .bind(new.title).bind(new.now).bind(new.created_by_thread.is_none().then_some(new.owner_id)).execute(&mut *conn).await?;
    crate::queries::hierarchy::stamp(conn, new.node_id).await?;
    crate::queries::hierarchy::touch_activity(conn, new.node_id, new.now).await?;
    Ok(())
}
