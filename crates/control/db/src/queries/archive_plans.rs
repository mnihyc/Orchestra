//! The exact subtree and settled-work identity reviewed before an archive or its inverse.

use crate::{Conn, DbError};
use orch_types::{Id, Json};

/// Current work of one live thread, without any input, credential or link contents.
#[derive(Debug, sqlx::FromRow)]
pub struct Work {
    /// Thread identity.
    pub id: Id,
    /// Starting or running turns.
    pub turns: i64,
    /// Pending or delivering inputs.
    pub inputs: i64,
    /// Undecided approvals.
    pub approvals: i64,
    /// Open leases.
    pub leases: i64,
    /// A current open harness segment.
    pub session: bool,
    /// Retained external links.
    pub links: i64,
}

/// A deterministic identity of the complete reviewed set: replacements with equal counts still differ.
/// A node is identified by its revision, which every change of it moves, and not by its tree
/// number: that is given at commit, so the inverse plan an archive answers with, made in the
/// archive's own transaction, would never match the state the archive leaves.
pub async fn snapshot(conn: &mut Conn, root: Id) -> Result<Json, DbError> {
    Ok(sqlx::query_scalar(
        "WITH scope AS (SELECT id FROM nodes WHERE $1=ANY(path) AND deleted_at IS NULL),
          docs AS (SELECT id FROM documents WHERE node_id IN (SELECT id FROM scope) AND deleted_at IS NULL),
          talks AS (SELECT id FROM chats WHERE node_id IN (SELECT id FROM scope))
         SELECT jsonb_build_object(
           'nodes', (SELECT jsonb_agg(jsonb_build_array(n.id,n.revision,n.archived_at,n.parent_id,n.path) ORDER BY n.id)
             FROM nodes n WHERE n.id IN (SELECT id FROM scope) OR n.id IN (SELECT unnest(path) FROM nodes WHERE id=$1)),
           'documents', (SELECT jsonb_agg(jsonb_build_array(d.id,d.node_id,d.seq_updated) ORDER BY d.id) FROM documents d WHERE d.id IN (SELECT id FROM docs)),
           'threads', (SELECT jsonb_agg(jsonb_build_array(t.node_id,t.seq_state,t.state,t.current_turn_id,t.current_segment_id,t.environment_id,t.side_of,t.links) ORDER BY t.node_id)
             FROM threads t WHERE t.node_id IN (SELECT id FROM scope)),
           'segments', (SELECT jsonb_agg(jsonb_build_array(s.id,s.thread_id,s.ended_at) ORDER BY s.id) FROM segments s
             WHERE s.id IN (SELECT current_segment_id FROM threads WHERE node_id IN (SELECT id FROM scope))),
           'turns', (SELECT jsonb_agg(jsonb_build_array(t.id,t.thread_id,t.state) ORDER BY t.id) FROM turns t WHERE t.thread_id IN (SELECT id FROM scope) AND t.state IN ('starting','running')),
           'inputs', (SELECT jsonb_agg(jsonb_build_array(i.id,i.thread_id,i.state) ORDER BY i.id) FROM thread_inputs i WHERE i.thread_id IN (SELECT id FROM scope) AND i.state IN ('pending','delivering')),
           'approvals', (SELECT jsonb_agg(jsonb_build_array(a.id,a.thread_id,a.state) ORDER BY a.id) FROM approvals a WHERE a.thread_id IN (SELECT id FROM scope) AND a.state='pending'),
           'leases', (SELECT jsonb_agg(jsonb_build_array(l.id,l.thread_id,l.mode,l.copy_expires_at) ORDER BY l.id) FROM leases l WHERE l.thread_id IN (SELECT id FROM scope) AND l.closed_at IS NULL),
           'environments', (SELECT jsonb_agg(jsonb_build_array(e.id,e.revision,e.home_node_id) ORDER BY e.id) FROM environments e WHERE e.deleted_at IS NULL AND
             (e.home_node_id IN (SELECT id FROM scope) OR e.id IN (SELECT environment_id FROM threads WHERE node_id IN (SELECT id FROM scope)))),
           'copies', (SELECT jsonb_agg(jsonb_build_array(s.id,s.thread_id,s.object_kind,s.object_id,s.state) ORDER BY s.id) FROM agent_shares s WHERE s.state<>'revoked' AND
             (s.thread_id IN (SELECT id FROM scope) OR s.object_id IN (SELECT id FROM scope UNION SELECT id FROM docs UNION SELECT id FROM talks))),
           'entries', (SELECT jsonb_agg(jsonb_build_array(e.id,e.revision,e.expires_at) ORDER BY e.id) FROM permission_entries e WHERE e.removed_at IS NULL AND (
             e.object_id IN (SELECT id FROM scope UNION SELECT id FROM docs UNION SELECT id FROM talks) OR (e.holder_kind IN ('agents','node') AND e.holder_id IN (SELECT id FROM scope)))),
           'chats', (SELECT jsonb_agg(id ORDER BY id) FROM talks))",
    ).bind(root).fetch_one(conn).await?)
}

/// Work whose identity the snapshot fences, under the archive writer's thread locks.
pub async fn work(conn: &mut Conn, root: Id) -> Result<Vec<Work>, DbError> {
    Ok(sqlx::query_as(
        "SELECT t.node_id AS id,
          (SELECT count(*) FROM turns x WHERE x.thread_id=t.node_id AND x.state IN ('starting','running')) AS turns,
          (SELECT count(*) FROM thread_inputs x WHERE x.thread_id=t.node_id AND x.state IN ('pending','delivering')) AS inputs,
          (SELECT count(*) FROM approvals x WHERE x.thread_id=t.node_id AND x.state='pending') AS approvals,
          (SELECT count(*) FROM leases x WHERE x.thread_id=t.node_id AND x.closed_at IS NULL) AS leases,
          EXISTS(SELECT 1 FROM segments x WHERE x.id=t.current_segment_id AND x.ended_at IS NULL) AS session,
          jsonb_array_length(t.links)::bigint AS links
         FROM threads t JOIN nodes n ON n.id=t.node_id WHERE $1=ANY(n.path) AND n.deleted_at IS NULL ORDER BY t.node_id",
    ).bind(root).fetch_all(conn).await?)
}

/// Environments which remain untouched, whether their home or a bound thread lies in the subtree.
pub async fn environments(conn: &mut Conn, root: Id) -> Result<Vec<(Id, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT e.id,e.name FROM environments e WHERE e.deleted_at IS NULL AND
          (e.home_node_id IN (SELECT id FROM nodes WHERE $1=ANY(path) AND deleted_at IS NULL)
           OR e.id IN (SELECT t.environment_id FROM threads t JOIN nodes n ON n.id=t.node_id WHERE $1=ANY(n.path) AND n.deleted_at IS NULL)) ORDER BY e.id",
    ).bind(root).fetch_all(conn).await?)
}

/// Counts retained copies and live agent entries without disclosing their private source contents.
pub async fn shares(conn: &mut Conn, root: Id) -> Result<(i64, i64), DbError> {
    Ok(sqlx::query_as(
        "WITH scope AS (SELECT id FROM nodes WHERE $1=ANY(path) AND deleted_at IS NULL),
          objects AS (SELECT id FROM scope UNION SELECT id FROM documents WHERE node_id IN (SELECT id FROM scope) AND deleted_at IS NULL
            UNION SELECT id FROM chats WHERE node_id IN (SELECT id FROM scope))
         SELECT (SELECT count(*) FROM agent_shares WHERE state<>'revoked' AND (thread_id IN (SELECT id FROM scope) OR object_id IN (SELECT id FROM objects))),
           (SELECT count(*) FROM permission_entries WHERE removed_at IS NULL AND holder_kind IN ('agents','node') AND (object_id IN (SELECT id FROM objects) OR holder_id IN (SELECT id FROM scope)))",
    ).bind(root).fetch_one(conn).await?)
}
