//! Stored candidates for permission-filtered setup observations and atomic personal choices.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// Providers at this org or below it; no credential contents are read.
pub async fn providers(conn: &mut Conn, org: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT p.id FROM providers p JOIN nodes n ON n.id=p.home_id WHERE n.path @> ARRAY[$1]::uuid[] AND n.deleted_at IS NULL AND p.deleted_at IS NULL ORDER BY p.id").bind(org).fetch_all(conn).await?)
}

/// Completed main-stream answers, excluding imported history; the first answer's segment keeps its scripted identity.
pub async fn answers(conn: &mut Conn, org: Id) -> Result<Vec<(Id, bool)>, DbError> {
    Ok(sqlx::query_as("SELECT DISTINCT ON (n.id) n.id,COALESCE(a.harness_kind='fake',false) FROM nodes n JOIN turns r ON r.thread_id=n.id JOIN segments s ON s.id=r.segment_id LEFT JOIN agents a ON a.id=s.agent_id WHERE n.path @> ARRAY[$1]::uuid[] AND n.deleted_at IS NULL AND r.state='completed' AND (r.error->>'incomplete') IS DISTINCT FROM 'true' AND EXISTS (SELECT 1 FROM items i WHERE i.turn_id=r.id AND i.stream='main' AND i.kind='agent_message' AND i.status='completed' AND NOT i.before_orch AND length(trim(COALESCE(i.payload->>'text','')))>0 AND (i.payload->>'phase'='final' OR (i.payload->>'phase' IS NULL AND NOT EXISTS (SELECT 1 FROM items phased WHERE phased.turn_id=r.id AND phased.stream='main' AND phased.kind='agent_message' AND phased.payload->>'phase' IS NOT NULL) AND NOT EXISTS (SELECT 1 FROM items later WHERE later.turn_id=r.id AND later.stream='main' AND later.kind='agent_message' AND later.seq_created>i.seq_created)))) ORDER BY n.id,r.ordinal,r.id").bind(org).fetch_all(conn).await?)
}

/// Domains of other active human members; the caller filters the domains before reporting any fact.
pub async fn member_domains(conn: &mut Conn, org: Id, user: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT DISTINCT m.domain_id FROM org_members m JOIN nodes n ON n.id=m.domain_id JOIN users u ON u.id=m.user_id WHERE n.path @> ARRAY[$1]::uuid[] AND n.deleted_at IS NULL AND m.user_id<>$2 AND u.kind='human' AND u.status='active' ORDER BY m.domain_id").bind(org).bind(user).fetch_all(conn).await?)
}

/// Only invitation managers use this metadata; no token or recipient is returned.
pub async fn invitation_waiting(
    conn: &mut Conn,
    org: Id,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM invitations WHERE org_id=$1 AND kind='org' AND uses=0 AND revoked_at IS NULL AND expires_at>$2 AND uses<max_uses)").bind(org).bind(now).fetch_one(conn).await?)
}

/// Merges the two private preference entries under their row locks, preserving unrelated hints and navigation.
pub async fn save_choice(
    conn: &mut Conn,
    user: Id,
    choice: &Json,
    org: Option<Id>,
    now: DateTime<Utc>,
) -> Result<Vec<i64>, DbError> {
    Ok(sqlx::query_scalar("INSERT INTO user_state(user_id,key,value,updated_at) VALUES ($1,'help',jsonb_build_object('first_steps',$2::jsonb),$4),($1,'sidebar',jsonb_build_object('last_org_id',$3::uuid),$4) ON CONFLICT(user_id,key) DO UPDATE SET value=(CASE WHEN jsonb_typeof(user_state.value)='object' THEN user_state.value ELSE '{}'::jsonb END)||EXCLUDED.value,updated_at=EXCLUDED.updated_at RETURNING octet_length(value::text)::bigint").bind(user).bind(choice).bind(org).bind(now).fetch_all(conn).await?)
}
