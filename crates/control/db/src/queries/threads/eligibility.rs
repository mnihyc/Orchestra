//! Small queue observations are retired by their real admission dependencies, without admitting work on read.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json, Seq};

/// One snapshot of metadata already used by admission. No credential bytes or source content is read.
/// Usage and authority use their existing commit counters, not a new shared writer lock.
pub async fn basis(
    conn: &mut Conn,
    thread: Id,
    now: DateTime<Utc>,
) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(
        "WITH place AS MATERIALIZED (
           SELECT t.node_id,t.agent_id,t.provider_id,t.environment_id,t.execution_revision,t.current_segment_id,
                  n.path,n.owner_id,a.home_id AS agent_home,p.home_id AS provider_home,e.home_node_id AS environment_home,
                  a.machine_id AS agent_machine,p.machine_id AS provider_machine,p.key_machine_id,e.machine_id AS environment_machine
             FROM threads t JOIN nodes n ON n.id=t.node_id
             LEFT JOIN agents a ON a.id=t.agent_id LEFT JOIN providers p ON p.id=t.provider_id
             LEFT JOIN environments e ON e.id=t.environment_id WHERE t.node_id=$1
         ), pending AS MATERIALIZED (
           SELECT i.id,i.author_id,i.run_as,i.submission_authority#>>'{capability_parent,answer}' AS parent_answer
             FROM thread_inputs i,place t WHERE i.thread_id=t.node_id AND i.state='pending'
              AND i.continuation_id=(SELECT active_continuation_id FROM threads WHERE node_id=$1)
         ), work AS MATERIALIZED (
           SELECT i.id,p.thread_id AS node_id,p.xmin::text AS version FROM pending i
             JOIN thread_tool_calls p ON p.answer_id=CASE WHEN pg_input_is_valid(i.parent_answer,'uuid') THEN i.parent_answer::uuid END
         ), scopes AS MATERIALIZED (
           SELECT DISTINCT unnest(n.path || n.id) AS id FROM nodes n,place t
            WHERE n.id=ANY(ARRAY[t.node_id,t.agent_home,t.provider_home,t.environment_home]) OR n.id IN (SELECT node_id FROM work)
         ), actors AS MATERIALIZED (
           SELECT author_id AS id FROM pending WHERE author_id IS NOT NULL
           UNION SELECT run_as FROM pending WHERE run_as IS NOT NULL
           UNION SELECT owner_id FROM place
         )
         SELECT encode(sha256(convert_to(jsonb_build_object(
           'binding',jsonb_build_array(t.agent_id,t.provider_id,t.environment_id,t.execution_revision,t.current_segment_id),
           'authority',(SELECT epoch FROM authz_state WHERE id),
           'usage',(SELECT revision FROM usage_change_state WHERE singleton),
           'work',(SELECT jsonb_agg(jsonb_build_array(w.id,w.node_id,w.version) ORDER BY w.id) FROM work w),
           'utc_day',($2::timestamptz AT TIME ZONE 'UTC')::date,
           'permission_expiry',(SELECT expires_at FROM permission_entries WHERE removed_at IS NULL AND expires_at IS NOT NULL AND expires_at<=$2 ORDER BY expires_at DESC LIMIT 1),
           'role_expiry',(SELECT expires_at FROM node_roles WHERE removed_at IS NULL AND expires_at IS NOT NULL AND expires_at<=$2 ORDER BY expires_at DESC LIMIT 1),
           'places',(SELECT jsonb_agg(jsonb_build_array(n.id,n.revision) ORDER BY n.id) FROM nodes n WHERE n.id IN (SELECT id FROM scopes)),
           'settings',(SELECT jsonb_agg(jsonb_build_array(s.node_id,s.key,s.revision,s.xmin::text) ORDER BY s.node_id,s.key) FROM settings s WHERE s.node_id IS NULL OR s.node_id IN (SELECT id FROM scopes)),
           'caps',(SELECT jsonb_agg(jsonb_build_array(c.id,c.revision,c.enabled,c.xmin::text) ORDER BY c.id) FROM usage_caps c WHERE c.node_id IS NULL OR c.node_id IN (SELECT id FROM scopes) OR c.provider_id=t.provider_id),
           'agent',(SELECT xmin::text FROM agents WHERE id=t.agent_id),
           'provider',(SELECT xmin::text FROM providers WHERE id=t.provider_id),
           'environment',(SELECT xmin::text FROM environments WHERE id=t.environment_id),
           'models',(SELECT jsonb_agg(jsonb_build_array(m.model_name,m.xmin::text) ORDER BY m.model_name) FROM provider_models m WHERE m.provider_id=t.provider_id),
           'machines',(SELECT jsonb_agg(jsonb_build_array(m.id,m.online) ORDER BY m.id) FROM machines m WHERE m.id=ANY(ARRAY[t.agent_machine,t.provider_machine,t.key_machine_id,t.environment_machine])),
           'people',(SELECT jsonb_agg(jsonb_build_array(u.id,u.status,u.kind,u.is_system_admin,u.totp_enabled_at,u.email_verified_at) ORDER BY u.id) FROM users u WHERE u.id IN (SELECT id FROM actors)),
           'keys',(SELECT jsonb_agg(jsonb_build_array(k.id,k.scopes,k.node_id,k.operation_scope,k.expires_at) ORDER BY k.id) FROM api_tokens k WHERE k.user_id IN (SELECT id FROM actors) AND k.revoked_at IS NULL AND (k.expires_at IS NULL OR k.expires_at>$2)),
           'sessions',(SELECT jsonb_agg(jsonb_build_array(s.id,s.as_admin,s.second_step_at,s.expires_at) ORDER BY s.id) FROM sessions s WHERE s.user_id IN (SELECT id FROM actors) AND s.revoked_at IS NULL AND s.expires_at>$2)
         )::text,'UTF8')),'hex') FROM place t",
    ).bind(thread).bind(now).fetch_optional(conn).await?)
}

/// A retained observation; its private basis is never a client authority or reservation.
#[derive(Debug, sqlx::FromRow)]
pub struct Observation {
    pub basis: String,
    pub queue_revision: i64,
    pub state_seq: Seq,
    pub result: Json,
}

pub async fn observation(conn: &mut Conn, thread: Id) -> Result<Option<Observation>, DbError> {
    Ok(sqlx::query_as("SELECT basis,queue_revision,state_seq,result FROM thread_queue_eligibility WHERE thread_id=$1")
        .bind(thread).fetch_optional(conn).await?)
}

pub async fn keep(conn: &mut Conn, thread: Id, value: &Observation) -> Result<(), DbError> {
    sqlx::query("INSERT INTO thread_queue_eligibility(thread_id,basis,queue_revision,state_seq,result) VALUES($1,$2,$3,$4,$5)
       ON CONFLICT(thread_id) DO UPDATE SET basis=EXCLUDED.basis,queue_revision=EXCLUDED.queue_revision,state_seq=EXCLUDED.state_seq,result=EXCLUDED.result")
        .bind(thread).bind(&value.basis).bind(value.queue_revision).bind(value.state_seq).bind(&value.result)
        .execute(conn).await?;
    Ok(())
}
