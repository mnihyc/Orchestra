//! Durable reported invocations and creation memberships join the shared resource facts reader.

use async_trait::async_trait;
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_api_contract::resource_facts::{ResourceUseKind, ResourceUses, UseCoverage};
use orch_db::{Conn, DbError};
use orch_types::{Id, Json, Time};

use super::{ReportedUses, UseRequest};
use crate::error::EngineError;

/// The production reported-operation source; turns remain in the existing consumption ledger.
pub struct RecordedUses;

/// Freezes the acknowledged session membership before a turn can outlive a reload or reconnect.
pub async fn capture_turn(conn: &mut Conn, turn: Id) -> Result<(), EngineError> {
    sqlx::query("UPDATE turns t SET resource_delivery=(SELECT jsonb_agg(jsonb_build_object('name',m->'name','server_id',m->'server_id')) FROM commands c CROSS JOIN LATERAL jsonb_array_elements(COALESCE(c.body->'session',c.body)->'mcp') m WHERE c.id=(s.delivered_resources->>'command')::uuid) FROM segments s WHERE t.id=$1 AND s.id=t.segment_id AND t.resource_delivery IS NULL")
        .bind(turn).execute(conn).await.map_err(DbError::from)?;
    Ok(())
}

/// A closed turn cannot accept another use, so its replay fence is no longer needed.
pub async fn close_turn(conn: &mut Conn, turn: Id) -> Result<(), EngineError> {
    sqlx::query("UPDATE turns SET resource_delivery=NULL WHERE id=$1")
        .bind(turn)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("DELETE FROM resource_use_receipts WHERE turn_id=$1")
        .bind(turn)
        .execute(conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct Counted {
    ordinal: i64,
    since: chrono::DateTime<chrono::Utc>,
    all_time: i64,
    recent: i64,
    last_at: Option<chrono::DateTime<chrono::Utc>>,
    people: i64,
    recent_people: i64,
    unknown_people: bool,
    unknown_recent_people: bool,
}

#[async_trait]
impl ReportedUses for RecordedUses {
    async fn read(
        &self,
        conn: &mut Conn,
        requested: &[UseRequest],
        at: Time,
    ) -> Result<Vec<(UseRequest, ResourceUses)>, EngineError> {
        let wanted: Vec<_> = requested.iter().enumerate().map(|(i, r)| serde_json::json!({"ordinal":i,"kind":r.object.kind.as_str(),"id":r.object.id,"version":r.version_id,"unit":r.kind.as_str()})).collect();
        let rows: Vec<Counted> = sqlx::query_as(include_str!("library.sql"))
            .bind(serde_json::json!(wanted))
            .bind(at.get())
            .fetch_all(conn)
            .await
            .map_err(DbError::from)?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                // No retained operation is not proof of an empty lifetime.
                row.last_at?;
                let request = usize::try_from(row.ordinal)
                    .ok()
                    .and_then(|i| requested.get(i))?
                    .clone();
                Some((
                    request.clone(),
                    ResourceUses {
                        kind: request.kind,
                        coverage: UseCoverage::Partial {
                            since: Time::new(row.since),
                        },
                        last_used_at: row.last_at.map(Time::new),
                        all_time: Some(row.all_time),
                        last_30_days: Some(row.recent),
                        people: (!row.unknown_people).then_some(row.people),
                        people_30_days: (!row.unknown_recent_people).then_some(row.recent_people),
                    },
                ))
            })
            .collect())
    }
}

#[derive(sqlx::FromRow)]
struct Lifetime {
    ordinal: i64,
    uses: i64,
}

/// Orders an inventory by the same lifetime sources without loading its dependency names or people.
pub async fn lifetimes(
    conn: &mut Conn,
    objects: &[ObjectRef],
    at: Time,
) -> Result<Vec<(ObjectRef, i64)>, EngineError> {
    let mut values = std::collections::HashMap::new();
    for (kind, dimension) in [
        (
            ObjectKind::Provider,
            orch_db::usage::rollup::ResourceTurnDimension::Provider,
        ),
        (
            ObjectKind::Agent,
            orch_db::usage::rollup::ResourceTurnDimension::Agent,
        ),
        (
            ObjectKind::Environment,
            orch_db::usage::rollup::ResourceTurnDimension::Environment,
        ),
    ] {
        let wanted: Vec<_> = objects
            .iter()
            .filter(|object| object.kind == kind)
            .map(|object| object.id)
            .collect();
        if wanted.is_empty() {
            continue;
        }
        for id in &wanted {
            values.insert(ObjectRef { kind, id: *id }, 0);
        }
        for row in
            orch_db::usage::rollup::resource_turn_counts(conn, &wanted, dimension, at.get()).await?
        {
            values.insert(ObjectRef { kind, id: row.id }, row.all_time);
        }
    }
    let wanted: Vec<_> = objects.iter().enumerate().filter(|(_,object)|matches!(object.kind,ObjectKind::McpServer|ObjectKind::Skill|ObjectKind::Host|ObjectKind::Template))
        .map(|(i,object)|serde_json::json!({"ordinal":i,"kind":object.kind.as_str(),"id":object.id})).collect();
    if !wanted.is_empty() {
        let rows: Vec<Lifetime> = sqlx::query_as(include_str!("library_lifetime.sql"))
            .bind(serde_json::json!(wanted))
            .fetch_all(conn)
            .await
            .map_err(DbError::from)?;
        for row in rows {
            if let Some(object) = usize::try_from(row.ordinal)
                .ok()
                .and_then(|index| objects.get(index))
            {
                values.insert(*object, row.uses);
            }
        }
    }
    Ok(values.into_iter().collect())
}

/// One accepted native execution, independent of a resource's mutable display name.
pub struct Report<'a> {
    /// The real open turn for a tool report; creation memberships have none.
    pub turn: Option<Id>,
    /// Its frozen resource identity.
    pub object: ObjectRef,
    /// The exact delivered skill version, where applicable.
    pub version: Option<Id>,
    /// The kind of real operation.
    pub kind: ResourceUseKind,
    /// A stable native identity within this resource's execution history.
    pub source: &'a str,
    /// The captured actor, never the resource's present owner.
    pub person: Option<Id>,
    /// When the operation was reported.
    pub at: Time,
}

/// Records the operation and both resource/version aggregates atomically in the caller's transaction.
pub async fn record(conn: &mut Conn, report: &Report<'_>) -> Result<(), EngineError> {
    sqlx::query(include_str!("library_record.sql"))
        .bind(report.object.kind.as_str())
        .bind(report.object.id)
        .bind(report.version)
        .bind(report.kind.as_str())
        .bind(report.source)
        .bind(report.at.get())
        .bind(report.person)
        .bind(report.turn)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

/// A completed native tool item captures only the version/server that its session actually received.
pub async fn tool_item(
    conn: &mut Conn,
    segment: Id,
    turn: Option<Id>,
    native: &str,
    payload: &Json,
    at: Time,
) -> Result<(), EngineError> {
    let Some(turn) = turn else {
        return Ok(());
    };
    let tool = &payload["tool"];
    let unit = tool["kind"].as_str();
    let name = tool["name"].as_str();
    let mcp = unit == Some("mcp");
    let skill = unit == Some("other") && name == Some("Skill");
    if !mcp && !skill {
        return Ok(());
    }
    // Refused local approvals never reach the target, even when the native tool prints an error.
    let denied: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM approvals a LEFT JOIN items i ON i.id=a.tool_item_id WHERE a.turn_id=$1 AND a.state='denied' AND (a.native_ref=$2 OR i.native_ref=$2))")
        .bind(turn).bind(native).fetch_one(&mut *conn).await.map_err(DbError::from)?;
    if denied {
        return Ok(());
    }
    let actor: Option<Id> = sqlx::query_scalar("SELECT audit_for_user_id FROM turns WHERE id=$1 AND segment_id=$2 AND started_at IS NOT NULL")
        .bind(turn).bind(segment).fetch_optional(&mut *conn).await.map_err(DbError::from)?.flatten();
    let found: Vec<(Id, Option<Id>)> = if mcp {
        if tool["result"].is_null() && tool["error"].is_null() {
            return Ok(());
        }
        sqlx::query_as("SELECT (m->>'server_id')::uuid,NULL::uuid FROM turns t CROSS JOIN LATERAL jsonb_array_elements(t.resource_delivery) m WHERE t.id=$1 AND m->>'name'=$2 AND m->>'server_id' IS NOT NULL")
            .bind(turn).bind(tool["server"].as_str()).fetch_all(&mut *conn).await.map_err(DbError::from)?
    } else {
        if tool["result"].is_null() {
            return Ok(());
        }
        let Some(reported) = tool.pointer("/arguments/skill").and_then(Json::as_str) else {
            return Ok(());
        };
        // Only the session plugin's own namespace names governed skills; another plugin is unrelated.
        let Some(reported) = reported.strip_prefix("orch-skills:") else {
            return Ok(());
        };
        sqlx::query_as("SELECT v.skill_id,v.id FROM turns t JOIN skill_versions v ON v.id=ANY(t.skill_versions) WHERE t.id=$1 AND v.name=$2")
            .bind(turn).bind(reported).fetch_all(&mut *conn).await.map_err(DbError::from)?
    };
    // Equal names without a unique captured native membership are unrecorded rather than guessed.
    if let [(id, version)] = found.as_slice() {
        record(
            conn,
            &Report {
                turn: Some(turn),
                object: ObjectRef {
                    kind: if mcp {
                        ObjectKind::McpServer
                    } else {
                        ObjectKind::Skill
                    },
                    id: *id,
                },
                version: *version,
                kind: if mcp {
                    ResourceUseKind::McpCall
                } else {
                    ResourceUseKind::SkillInvocation
                },
                source: &format!("{segment}:{native}"),
                person: actor,
                at,
            },
        )
        .await?;
    }
    Ok(())
}

/// Successful generations preserve one logical creation membership and refresh its exact last use.
pub async fn created(
    conn: &mut Conn,
    template: Id,
    host: Option<Id>,
    resource: Id,
    provider: bool,
    person: Option<Id>,
    at: Time,
) -> Result<(), EngineError> {
    let kind = if provider {
        ResourceUseKind::ProviderMade
    } else {
        ResourceUseKind::EnvironmentMade
    };
    let source = resource.to_string();
    let subjects = std::iter::once(ObjectRef {
        kind: ObjectKind::Template,
        id: template,
    })
    .chain(host.map(|id| ObjectRef {
        kind: ObjectKind::Host,
        id,
    }));
    for object in subjects {
        record(
            conn,
            &Report {
                turn: None,
                object,
                version: None,
                kind,
                source: &source,
                person,
                at,
            },
        )
        .await?;
        sqlx::query("UPDATE resource_use_rollups SET last_at=GREATEST(last_at,$4) WHERE subject_kind=$1 AND subject_id=$2 AND use_kind=$3 AND version_id IS NULL AND day IS NULL")
            .bind(object.kind.as_str()).bind(object.id).bind(kind.as_str()).bind(at.get()).execute(&mut *conn).await.map_err(DbError::from)?;
    }
    Ok(())
}
