//! Resource pages read the same batched library figures as inventory rows, after ordinary View checks.

use crate::{
    middleware::{
        authorization,
        requests::{ApiError, ApiResult},
    },
    registry::{Ctx, Registry},
};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::{
    library::{self as api, LibraryEntry, LibraryReadBody},
    resource_facts::{FactSubject, ResourceUseKind},
};
use orch_engine::resource_facts::{self, UseRequest, library::RecordedUses};
use orch_types::Time;
use std::collections::HashSet;

/// Binds the ordinary resource-page library read.
pub fn mount(registry: &mut Registry) {
    registry.on(api::ResourceLibraryRead, read);
    registry.on(api::ResourceLibraryPeopleRead, people);
}

/// An environment owns the authority of its own tool definitions; presets retain their own View.
pub(crate) async fn readable(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    objects: &[ObjectRef],
) -> ApiResult<HashSet<ObjectRef>> {
    let ids: Vec<_> = objects.iter().map(|o| o.id).collect();
    let mut allowed: HashSet<_> = resource_facts::authorities(conn, principal, &ids)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .filter(|row| row.readable)
        .map(|row| row.resource.object())
        .collect();
    let own:Vec<(orch_types::Id,orch_types::Id)>=sqlx::query_as("SELECT id,environment_id FROM mcp_servers WHERE id=ANY($1) AND environment_id IS NOT NULL AND deleted_at IS NULL").bind(&ids).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let environments: Vec<_> = own.iter().map(|(_, env)| *env).collect();
    if !environments.is_empty() {
        let visible: HashSet<_> = orch_authz::resources::holding(
            conn,
            principal,
            &environments,
            orch_access_contract::permissions::Permission::EnvironmentView,
            Default::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|env| env.id)
        .collect();
        for (id, env) in own {
            if visible.contains(&env) {
                allowed.insert(ObjectRef {
                    kind: ObjectKind::McpServer,
                    id,
                });
            }
        }
    }
    Ok(allowed)
}

async fn read(ctx: Ctx, (): (), body: LibraryReadBody) -> ApiResult<Vec<LibraryEntry>> {
    if body.targets.len()
        > usize::try_from(orch_api_contract::common::MAX_LIMIT).map_err(ApiError::internal)?
    {
        return Err(ApiError::invalid(
            "targets",
            "Read resource figures one page at a time.",
        ));
    }
    let mut tx = ctx.app.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let objects: Vec<_> = body.targets.iter().map(|target| target.object).collect();
    let authorized = readable(&mut tx, &principal, &objects).await?;
    let version_ids: Vec<_> = body
        .targets
        .iter()
        .filter_map(|target| target.version_id)
        .collect();
    let memberships: HashSet<(orch_types::Id, orch_types::Id)> =
        sqlx::query_as("SELECT id,skill_id FROM skill_versions WHERE id=ANY($1)")
            .bind(&version_ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?
            .into_iter()
            .collect();
    let mut seen = HashSet::new();
    let mut subjects = Vec::new();
    let mut requested = Vec::new();
    for target in &body.targets {
        if !authorized.contains(&target.object) {
            return Err(ApiError::not_found());
        }
        if let Some(version) = target.version_id {
            if target.object.kind != ObjectKind::Skill
                || !memberships.contains(&(version, target.object.id))
            {
                return Err(ApiError::not_found());
            }
            requested.push(UseRequest {
                object: target.object,
                version_id: Some(version),
                kind: ResourceUseKind::SkillInvocation,
            });
        } else if seen.insert(target.object) {
            subjects.push(FactSubject::Resource {
                object: target.object,
            });
        }
    }
    let at = Time::new(ctx.now);
    let parents = resource_facts::read_with(&mut tx, &subjects, at, Some(&RecordedUses))
        .await
        .map_err(ApiError::internal)?;
    let parents: std::collections::HashMap<_, _> = parents
        .into_iter()
        .filter_map(|facts| match &facts.subject {
            FactSubject::Resource { object } => Some((*object, facts)),
            _ => None,
        })
        .collect();
    let versions = resource_facts::reported_uses(&mut tx, &requested, at, Some(&RecordedUses))
        .await
        .map_err(ApiError::internal)?;
    let versions: std::collections::HashMap<_, _> = versions
        .into_iter()
        .map(|(request, uses)| ((request.object, request.version_id), uses))
        .collect();
    let raw: Vec<_> = body
        .targets
        .iter()
        .map(|target| {
            if target.version_id.is_some() {
                // A version reports its uses; current giving belongs to the resource once, not to every version.
                Ok(resource_facts::RawFacts {
                    subject: FactSubject::Resource {
                        object: target.object,
                    },
                    as_of: at,
                    dependents: Vec::new(),
                    dependents_complete: false,
                    uses: versions.get(&(target.object, target.version_id)).cloned(),
                })
            } else {
                parents
                    .get(&target.object)
                    .cloned()
                    .ok_or_else(|| ApiError::internal("the resource facts were not read"))
            }
        })
        .collect::<ApiResult<_>>()?;
    let facts = crate::routes::everything::library::project(&mut tx, &principal, raw).await?;
    let provider_ids: Vec<_> = body
        .targets
        .iter()
        .filter(|target| target.object.kind == ObjectKind::Provider)
        .map(|target| target.object.id)
        .collect();
    let threads = crate::routes::everything::library::threads(&mut tx, &provider_ids, at).await?;
    Ok(body
        .targets
        .into_iter()
        .zip(facts)
        .map(|(target, facts)| {
            let count = threads.get(&target.object.id).copied();
            LibraryEntry {
                target,
                facts,
                threads: count,
            }
        })
        .collect())
}

async fn people(
    ctx: Ctx,
    (): (),
    body: api::LibraryPeopleBody,
) -> ApiResult<api::LibraryPeopleReply> {
    let mut conn = ctx.app.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let object = body.target.object;
    if !readable(&mut conn, &principal, &[object])
        .await?
        .contains(&object)
    {
        return Err(ApiError::not_found());
    }
    if let Some(version) = body.target.version_id {
        let member: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM skill_versions WHERE id=$1 AND skill_id=$2)",
        )
        .bind(version)
        .bind(object.id)
        .fetch_one(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
        if object.kind != ObjectKind::Skill || !member {
            return Err(ApiError::not_found());
        }
    }
    let column = match object.kind {
        ObjectKind::Provider => Some("provider_id"),
        ObjectKind::Agent => Some("agent_id"),
        ObjectKind::Environment => Some("environment_id"),
        _ => None,
    };
    // Identifiers come only from the closed resource vocabulary; values remain parameters.
    let source=column.map_or_else(||include_str!("library_people_reported.sql").trim().to_owned(),|column|format!("SELECT user_id id FROM usage_events WHERE {column}=$2 AND turn_id IS NOT NULL AND at<=$6 AND (NOT $9 OR at >= (($6::timestamptz AT TIME ZONE 'UTC')::date-29)::timestamp AT TIME ZONE 'UTC') UNION SELECT t.audit_for_user_id FROM turns t JOIN segments s ON s.id=t.segment_id WHERE s.{column}=$2 AND $1<>'provider' AND t.started_at IS NOT NULL AND t.started_at<=$6 AND (NOT $9 OR t.started_at >= (($6::timestamptz AT TIME ZONE 'UTC')::date-29)::timestamp AT TIME ZONE 'UTC') AND (t.native_turn_ref IS NULL OR t.native_turn_ref NOT LIKE 'fork-copy:%')"));
    let sql = include_str!("library_people.sql").replace("__SOURCE__", &source);
    let limit = orch_api_contract::common::DEFAULT_LIMIT;
    let result: orch_types::Json = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(object.kind.as_str())
        .bind(object.id)
        .bind(body.target.version_id)
        .bind(principal.user())
        .bind(principal.memberships().roots())
        .bind(ctx.now)
        .bind(body.cursor)
        .bind(i64::from(limit) + 1)
        .bind(body.recent)
        .fetch_one(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
    let mut ids: Vec<orch_types::Id> =
        serde_json::from_value(result["ids"].clone()).map_err(ApiError::internal)?;
    let more = ids.len() > usize::try_from(limit).map_err(ApiError::internal)?;
    ids.truncate(usize::try_from(limit).map_err(ApiError::internal)?);
    let next_cursor = more.then(|| ids.last().copied()).flatten();
    let names =
        crate::routes::access::page::user_refs_at(&mut conn, &principal, None, &ids).await?;
    Ok(api::LibraryPeopleReply {
        items: ids
            .into_iter()
            .filter_map(|id| names.get(&id).cloned())
            .collect(),
        hidden: result["hidden"].as_i64().unwrap_or(0),
        next_cursor,
    })
}
