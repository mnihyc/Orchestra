//! Exact live work dependencies share one reader and the ordinary batched privacy projection.

use super::{Dependent, RawFacts};
use crate::EngineError;
use chrono::{DateTime, Utc};
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_api_contract::resource_facts::{
    ActivityFacts, ActivityGroup, DependentRelation, FactSubject, FactTarget,
};
use orch_authz::{Policy, Principal};
use orch_db::{Conn, DbError};
use orch_types::{Id, Time};
use std::collections::{BTreeMap, HashSet};

/// A separate server-owned request cannot be admitted after a resource it runs on is off.
pub async fn accepts_request(
    conn: &mut Conn,
    provider: Id,
    model: &str,
) -> Result<bool, EngineError> {
    accepts_context(conn, provider, model, None, None).await
}

/// Captured source work cannot spend through a profile or environment that is switched off.
pub async fn accepts_context(
    conn: &mut Conn,
    provider: Id,
    model: &str,
    agent: Option<Id>,
    environment: Option<Id>,
) -> Result<bool, EngineError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM providers p JOIN provider_models m ON m.provider_id=p.id AND m.model_name=$2 LEFT JOIN agents a ON a.id=p.agent_id LEFT JOIN environments e ON e.id=p.held_by_environment_id WHERE p.id=$1 AND p.deleted_at IS NULL AND p.status='ready' AND m.enabled AND (a.id IS NULL OR (NOT a.disabled AND a.deleted_at IS NULL)) AND (e.id IS NULL OR (e.status<>'disabled' AND e.deleted_at IS NULL)) AND ($3::uuid IS NULL OR EXISTS(SELECT 1 FROM agents source WHERE source.id=$3 AND NOT source.disabled AND source.deleted_at IS NULL)) AND ($4::uuid IS NULL OR EXISTS(SELECT 1 FROM environments source WHERE source.id=$4 AND source.status<>'disabled' AND source.deleted_at IS NULL)))").bind(provider).bind(model).bind(agent).bind(environment).fetch_one(conn).await.map_err(DbError::from)?)
}

/// A durable stop belongs to the accepted id and survives making its resource available again.
pub async fn request_stop_reason(conn: &mut Conn, id: Id) -> Result<Option<String>, EngineError> {
    Ok(sqlx::query_scalar::<_, Option<String>>(
        "SELECT cancel_reason FROM summary_calls WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(conn)
    .await
    .map_err(DbError::from)?
    .flatten())
}

/// Dispatch checks the accepted call's frozen context and stop before admitting or transmitting it.
pub async fn accepts_command(
    conn: &mut Conn,
    id: Id,
    provider: Id,
    model: &str,
) -> Result<bool, EngineError> {
    let context: Option<(Option<String>, Option<Id>, Option<Id>)> = sqlx::query_as(
        "SELECT cancel_reason,agent_id,environment_id FROM summary_calls WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    if context
        .as_ref()
        .is_some_and(|(reason, _, _)| reason.is_some())
    {
        return Ok(false);
    }
    let (_, agent, environment) = context.unwrap_or_default();
    accepts_context(conn, provider, model, agent, environment).await
}

/// Existing accepted calls and completion commands share one live reader, without a new use producer.
pub async fn requests(conn: &mut Conn, target: &FactTarget) -> Result<Vec<Id>, EngineError> {
    Ok(sqlx::query_scalar(include_str!("requests.sql"))
        .bind(target.object.kind.as_str())
        .bind(target.object.id)
        .bind(target.model_id)
        .fetch_all(conn)
        .await
        .map_err(DbError::from)?)
}

/// The resource transaction freezes the first stop reason before any native cancellation is sent.
pub async fn stop_requests(conn: &mut Conn, ids: &[Id], reason: &str) -> Result<(), EngineError> {
    sqlx::query(
        "UPDATE summary_calls SET cancel_reason=COALESCE(cancel_reason,$2) WHERE id=ANY($1)",
    )
    .bind(ids)
    .bind(reason)
    .execute(conn)
    .await
    .map_err(DbError::from)?;
    Ok(())
}

/// Enable checks the accepted stop dependency so an old unit stop cannot race new work.
pub async fn stopping(conn: &mut Conn, target: &FactTarget) -> Result<bool, EngineError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE kind='resource_quiesce' AND state IN ('queued','running') AND plan->'quiesce'->'pending_native' IS DISTINCT FROM 'false'::jsonb AND plan->'quiesce'->'target'->'object'->>'id'=$1 AND plan->'quiesce'->'target'->'object'->>'kind'=$2 AND ($3::uuid IS NULL OR plan->'quiesce'->'target'->>'model_id'=$3::text))").bind(target.object.id.to_string()).bind(target.object.kind.as_str()).bind(target.model_id).fetch_one(conn).await.map_err(DbError::from)?)
}

/// A pause's visible note is read from the resource itself, not a possibly collected job.
pub async fn note(conn: &mut Conn, target: &FactTarget) -> Result<Option<String>, EngineError> {
    let query = match (target.object.kind, target.model_id) {
        (ObjectKind::Provider, Some(_)) => {
            "SELECT pause_note FROM provider_models WHERE id=$1 AND NOT enabled"
        }
        (ObjectKind::Provider, None) => {
            "SELECT status_detail FROM providers WHERE id=$1 AND status='disabled'"
        }
        (ObjectKind::Agent, None) => "SELECT pause_note FROM agents WHERE id=$1 AND disabled",
        (ObjectKind::Environment, None) => {
            "SELECT status_detail FROM environments WHERE id=$1 AND status='disabled'"
        }
        _ => return Ok(None),
    };
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(query))
        .bind(target.model_id.unwrap_or(target.object.id))
        .fetch_optional(conn)
        .await
        .map_err(DbError::from)?
        .flatten())
}

/// An authoritative work membership, kept internal until its thread is projected.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Work {
    /// Running, waiting or stopped.
    pub kind: String,
    /// The turn or input's stable identity.
    pub id: Id,
    /// Exact pending messages in a waiting membership, or one running/stopped turn.
    pub count: i64,
    /// Its thread, also the name's visibility boundary.
    pub thread: Id,
    /// The immutable segment for a turn.
    pub segment: Option<Id>,
    /// The captured single-turn author; an aggregate waiting membership has no invented author.
    pub author: Option<Id>,
    /// The current title, kept private until projection.
    pub name: String,
    /// When that work began waiting or running.
    pub since: DateTime<Utc>,
}

/// One resource's exact work at the caller's observation instant.
pub async fn read(conn: &mut Conn, target: &FactTarget) -> Result<Vec<Work>, EngineError> {
    let kind = match target.object.kind {
        ObjectKind::Provider => "provider",
        ObjectKind::Agent => "agent",
        ObjectKind::Environment => "environment",
        _ => return Ok(Vec::new()),
    };
    let mut work: Vec<Work> = sqlx::query_as(include_str!("activity.sql"))
        .bind(kind)
        .bind(target.object.id)
        .bind(target.model_id)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    if let Some(model) = target.model_id {
        let name: Option<String> = sqlx::query_scalar(
            "SELECT model_name FROM provider_models WHERE id=$1 AND provider_id=$2",
        )
        .bind(model)
        .bind(target.object.id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?;
        let threads: Vec<_> = work
            .iter()
            .filter(|row| row.kind == "waiting")
            .map(|row| row.thread)
            .collect();
        let selected = orch_settings::resolver::get_at_nodes::<String>(
            conn,
            &threads,
            orch_settings_contract::registry::keys::AGENT_MODEL,
        )
        .await
        .map_err(EngineError::from)?;
        work.retain(|row| {
            row.kind != "waiting"
                || name.is_some()
                    && selected.get(&row.thread).and_then(Option::as_ref) == name.as_ref()
        });
    }
    Ok(work)
}

/// Projects the same work records once; all stop/resume counts stay in this service.
pub async fn project(
    conn: &mut Conn,
    principal: &Principal,
    target: &FactTarget,
    work: &[Work],
    at: Time,
) -> Result<ActivityFacts, EngineError> {
    let mut sources = Vec::new();
    for kind in ["running", "waiting", "stopped"] {
        let mut by_thread = BTreeMap::new();
        for row in work.iter().filter(|row| row.kind == kind) {
            by_thread.entry(row.thread).or_insert(row);
        }
        sources.push(RawFacts {
            subject: FactSubject::Resource {
                object: target.object,
            },
            as_of: at,
            dependents: by_thread
                .into_values()
                .map(|row| {
                    let object = ObjectRef {
                        kind: ObjectKind::Thread,
                        id: row.thread,
                    };
                    Dependent {
                        key: row.thread.to_string(),
                        visibility: Some(object),
                        object: Some(object),
                        name: row.name.clone(),
                        relation: DependentRelation::Operation,
                        until: None,
                    }
                })
                .collect(),
            dependents_complete: true,
            uses: None,
        });
    }
    let projected = super::project(conn, principal, Policy::default(), &sources).await?;
    let mut groups = Vec::new();
    for (index, kind) in ["running", "waiting", "stopped"].into_iter().enumerate() {
        let rows: Vec<_> = work.iter().filter(|row| row.kind == kind).collect();
        let threads: HashSet<_> = rows.iter().map(|row| row.thread).collect();
        let names = projected[index].dependents.first();
        groups.push(ActivityGroup {
            count: rows
                .iter()
                .fold(0_i64, |total, row| total.saturating_add(row.count)),
            threads: i64::try_from(threads.len()).unwrap_or(i64::MAX),
            visible: names.map(|group| group.visible.clone()).unwrap_or_default(),
            hidden: names.map_or(0, |group| group.hidden),
            since: rows.iter().map(|row| row.since).min().map(Time::new),
        });
    }
    let mut groups = groups.into_iter();
    let empty = || ActivityGroup {
        count: 0,
        threads: 0,
        visible: Vec::new(),
        hidden: 0,
        since: None,
    };
    Ok(ActivityFacts {
        running: groups.next().unwrap_or_else(empty),
        waiting: groups.next().unwrap_or_else(empty),
        stopped: groups.next().unwrap_or_else(empty),
    })
}
