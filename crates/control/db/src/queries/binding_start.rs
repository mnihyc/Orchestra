//! Startup harness discovery candidates and locally found subscriptions, without creating a thread.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// Profiles whose real harness has not reported, paired with an existing provider for their process. One
/// that was tried and failed (or reported nothing) is tried again only where `retry`, and only once its last
/// try is older than `retry_before`: a machine that cannot run a harness is not asked at every report.
pub async fn undiscovered(
    conn: &mut Conn,
    machine: Id,
    retry: bool,
    retry_before: DateTime<Utc>,
) -> Result<Vec<(Id, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT a.id, p.id FROM agents a
         JOIN machines m ON m.id = $1 AND m.deleted_at IS NULL AND m.status = 'active'
         CROSS JOIN LATERAL (
             SELECT p.id FROM providers p WHERE p.deleted_at IS NULL AND p.status <> 'disabled'
               AND (p.kind = 'api' OR (p.agent_id = a.id AND p.machine_id = m.id))
               AND (a.org_id IS NULL OR p.org_id = a.org_id)
             ORDER BY (p.machine_id = m.id) DESC NULLS LAST, p.created_at, p.id LIMIT 1
         ) p
         WHERE a.deleted_at IS NULL AND NOT a.disabled
           AND (NOT EXISTS (SELECT 1 FROM harnesses h WHERE h.agent_id = a.id AND h.machine_id = m.id)
                OR ($2 AND EXISTS (SELECT 1 FROM harnesses h WHERE h.agent_id = a.id AND h.machine_id = m.id
                    AND (h.status = 'error' OR COALESCE(h.descriptor, '{}'::jsonb) = '{}'))
                    AND NOT EXISTS (SELECT 1 FROM harnesses h WHERE h.agent_id = a.id AND h.machine_id = m.id
                    AND h.updated_at > $3)))
           AND (a.machine_id = m.id OR (a.machine_id IS NULL AND (a.org_id IS NULL OR a.owner_id = m.owner_id)))
         ORDER BY a.created_at, a.id",
    )
    .bind(machine)
    .bind(retry)
    .bind(retry_before)
    .fetch_all(conn)
    .await?)
}

/// What an actual process of this profile on the machine last reported, if it has been probed.
/// The final flag permits a fresh start only for a recorded pin refusal resolved by the current pin.
pub async fn harness_report(
    conn: &mut Conn,
    machine: Id,
    agent: Id,
    requested_version: Option<&str>,
) -> Result<Option<(String, Option<String>, orch_types::Json, bool)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT status, status_detail, descriptor,
            COALESCE(status='error'
              AND after_up_plan->'version_pin_refusal'->>'detail'=status_detail
              AND after_up_plan->'version_pin_refusal'->>'actual'=harness_version
              AND ($3::text IS NULL OR after_up_plan->'version_pin_refusal'->>'actual'=$3),false)
         FROM harnesses WHERE machine_id = $1 AND agent_id = $2
        ORDER BY updated_at DESC, provider_id LIMIT 1",
    )
    .bind(machine)
    .bind(agent)
    .bind(requested_version)
    .fetch_optional(conn)
    .await?)
}

/// Harness kinds actually probed on the machine, including compiled ones with no separate binary.
pub async fn reported_kinds(conn: &mut Conn, machine: Id) -> Result<Vec<String>, DbError> {
    Ok(sqlx::query_scalar("SELECT DISTINCT a.harness_kind FROM harnesses h JOIN agents a ON a.id = h.agent_id
        WHERE h.machine_id = $1 AND h.descriptor <> '{}' AND h.status <> 'error' AND a.deleted_at IS NULL AND NOT a.disabled")
        .bind(machine).fetch_all(conn).await?)
}

/// The connected machine's owner and, where they have one, their personal home; discovery never grants anybody
/// else's access. Without a personal home an existing registration is still kept up to date, and a new one waits
/// for the owner to choose its home.
pub async fn discovery_owner(
    conn: &mut Conn,
    machine: Id,
) -> Result<Option<(Id, Option<Id>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT u.id, n.id FROM machines m JOIN users u ON u.id = m.owner_id
         LEFT JOIN nodes n ON n.id = u.personal_org_id AND n.deleted_at IS NULL
         WHERE m.id = $1 AND m.placement = 'connected' AND m.deleted_at IS NULL AND m.status = 'active'
           AND u.status = 'active' FOR UPDATE OF m",
    ).bind(machine).fetch_optional(conn).await?)
}

/// An existing registration of the machine's default harness home, if one already names this login.
pub async fn found_subscription(
    conn: &mut Conn,
    machine: Id,
    kind: &str,
) -> Result<Option<(Id, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT p.id, a.id FROM providers p JOIN agents a ON a.id = p.agent_id
         WHERE p.key_machine_id = $1 AND p.harness_kind = $2 AND p.kind = 'subscription'
           AND p.deleted_at IS NULL AND a.deleted_at IS NULL
           AND (a.profile #>> '{attach,home}') IS NULL
         ORDER BY p.created_at, p.id LIMIT 1",
    )
    .bind(machine)
    .bind(kind)
    .fetch_optional(conn)
    .await?)
}

/// Records a changed identity without restamping a repeated discovery.
pub async fn set_identity(
    conn: &mut Conn,
    provider: Id,
    identity: Option<&str>,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE providers SET facts = facts || jsonb_build_object('identity', $2::text), facts_at = $3, updated_at = $3
        WHERE id = $1 AND (facts -> 'identity') IS DISTINCT FROM COALESCE(to_jsonb($2::text), 'null'::jsonb)")
        .bind(provider).bind(identity).bind(now).execute(conn).await?.rows_affected() > 0)
}

/// Whether an environment serves work anywhere inside the project or folder.
pub async fn used_in_project(
    conn: &mut Conn,
    environment: Id,
    project: Id,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM threads t JOIN nodes n ON n.id = t.topic_id
         WHERE t.environment_id = $1 AND n.deleted_at IS NULL AND $2 = ANY(n.path))",
    )
    .bind(environment)
    .bind(project)
    .fetch_one(conn)
    .await?)
}

/// Whether the person has a connected environment machine with a locally allowed root.
pub async fn environment_machine_owned(conn: &mut Conn, owner: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM machines WHERE owner_id = $1 AND placement = 'connected'
        AND status = 'active' AND deleted_at IS NULL AND 'environment' = ANY(roles)
        AND COALESCE(jsonb_array_length(facts #> '{policy,environment.roots}'), 0) > 0)",
    )
    .bind(owner)
    .fetch_one(conn)
    .await?)
}
