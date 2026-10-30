//! Candidate documents and folders for Shared with me. Native document Read decides every returned row.

use super::access::kept::Held;
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// A scanned document-domain candidate; names are loaded only after the native Read decision.
#[derive(Debug, sqlx::FromRow)]
pub struct DocumentCandidate {
    /// The document-domain permission-page kind.
    pub kind: String,
    /// Stable container identity.
    pub id: Id,
    /// The same creation-time order as shared hierarchy nodes.
    pub created_at: DateTime<Utc>,
}

/// Current ordinary holder sets and the scanned boundary of a merged node/document/folder page.
#[derive(Debug)]
pub struct DocumentQuery<'a> {
    /// The person; system-admin standing is not a substitute for a named holder.
    pub user: Id,
    /// Their current ordinary identities, including nested groups and actual roles.
    pub held: &'a Held,
    /// The application's sampled time.
    pub at: DateTime<Utc>,
    /// Last scanned creation time and identity.
    pub before: Option<(DateTime<Utc>, Id)>,
    /// Kind at the boundary: node, document_container, then document on an exact tie.
    pub after_kind: &'a str,
    /// At most one page plus its lookahead.
    pub limit: i64,
}

/// Owners and current matching holders on real document-domain pages, without hidden names.
/// Candidate membership alone never grants Read; the caller applies one batched native decision.
pub async fn documents(
    conn: &mut Conn,
    query: &DocumentQuery<'_>,
) -> Result<Vec<DocumentCandidate>, DbError> {
    Ok(sqlx::query_as(
        "WITH held AS MATERIALIZED (
           SELECT * FROM unnest($3::text[],$4::uuid[],$5::text[]) AS h(kind,id,role)
         )
         , candidates AS (
           SELECT 'document_container'::text AS kind,id,node_id,owner_id,created_at,deleted_at FROM document_containers
           UNION ALL
           SELECT 'document'::text AS kind,id,node_id,owner_id,created_at,deleted_at FROM documents
         )
         SELECT c.kind,c.id,c.created_at FROM candidates c JOIN nodes n ON n.id=c.node_id
          WHERE c.deleted_at IS NULL
            AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id=ANY(n.path) AND a.deleted_at IS NOT NULL)
            AND (c.owner_id=$1 OR EXISTS (
              SELECT 1 FROM permission_entries e
               WHERE e.object_kind=c.kind AND e.object_id=c.id AND e.page='live'
                 AND e.removed_at IS NULL AND (e.expires_at IS NULL OR e.expires_at>$2)
                 AND (EXISTS (SELECT 1 FROM held h WHERE
                       (e.holder_kind=h.kind AND e.holder_id=h.id AND e.holder_kind IN ('user','group','members'))
                       OR (e.holder_kind='role' AND h.kind IN ('role','visible') AND h.id=COALESCE(e.holder_id,c.node_id)
                           AND array_position(ARRAY['viewer','interactor','developer','admin'],h.role)
                               >=array_position(ARRAY['viewer','interactor','developer','admin'],e.holder_role)))
                      OR (e.holder_kind='everyone' AND EXISTS (
                           SELECT 1 FROM held h WHERE h.kind='visible' AND h.id=ANY(n.path))))))
            AND ($6::timestamptz IS NULL OR (c.created_at,c.id)<($6,$7::uuid)
                 OR (c.kind<$8::text AND (c.created_at,c.id)=($6,$7::uuid)))
          ORDER BY c.created_at DESC,c.id DESC,c.kind DESC LIMIT $9",
    )
    .bind(query.user).bind(query.at)
    .bind(&query.held.kinds).bind(&query.held.ids).bind(&query.held.roles)
    .bind(query.before.map(|(at,_)|at)).bind(query.before.map(|(_,id)|id))
    .bind(query.after_kind).bind(query.limit)
    .fetch_all(conn).await?)
}
