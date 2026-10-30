//! Library views, privacy projections and recorded-use ordering share the inventory's single read.

use crate::middleware::requests::{ApiError, ApiResult};
use orch_access_contract::{
    decisions::ObjectRef,
    permissions::{Level, ResourceKind},
};
use orch_api_contract::resource_facts::DependentRelation;
use orch_api_contract::{
    everything::{EverythingListQuery, EverythingSort},
    resource_facts::{FactSubject, ResourceFacts},
    resources::ProviderListScope,
};
use orch_authz::{Principal, everything::Projection};
use orch_db::{
    Conn,
    queries::everything::{Grant, Row},
};
use orch_engine::resource_facts::{self, library::RecordedUses};
use orch_types::Time;
use std::collections::HashMap;

pub(super) fn validate(query: &EverythingListQuery, kinds: &[String]) -> ApiResult<()> {
    if query.resource_view == Some(ProviderListScope::Usable)
        && matches!(kinds.first().map(String::as_str), Some("environment"))
    {
        return Err(ApiError::invalid(
            "resource_view",
            "This resource has View and Manage.",
        ));
    }
    if query.sort == Some(EverythingSort::Use)
        && (kinds.len() != 1
            || !matches!(
                kinds[0].as_str(),
                "provider" | "agent" | "environment" | "host" | "template" | "mcp_server" | "skill"
            ))
    {
        return Err(ApiError::invalid(
            "sort",
            "Choose a resource with recorded uses.",
        ));
    }
    Ok(())
}

pub(super) async fn prepare(
    conn: &mut Conn,
    principal: &Principal,
    query: &EverythingListQuery,
    projection: &mut Projection,
    at: Time,
) -> ApiResult<()> {
    if query.resource_view == Some(ProviderListScope::All) {
        let home = query
            .home
            .ok_or_else(|| ApiError::invalid("home", "Choose an administered organization."))?;
        let access = orch_authz::nodes::effective(conn, principal, home, Default::default())
            .await
            .map_err(ApiError::internal)?;
        if !access.administers {
            return Err(ApiError::not_found());
        }
    }
    if let Some(view) = query.resource_view {
        for each in &projection.resources {
            let held = |level| {
                each.held.iter().any(|p| {
                    p.permission.level() == level
                        && p.permission.area() == each.resource.kind.area()
                })
            };
            let manage = each.owner || held(Level::Manage);
            let included = match view {
                ProviderListScope::Mine => each.owner,
                ProviderListScope::Managed => manage,
                ProviderListScope::Usable => manage || held(Level::Use),
                ProviderListScope::Viewable | ProviderListScope::All => true,
            };
            if let Some(grant) = projection
                .grants
                .get_mut("objects")
                .and_then(|all| all.get_mut(each.resource.id.to_string()))
                .and_then(serde_json::Value::as_object_mut)
            {
                grant.insert("library_scope".to_owned(), serde_json::json!(included));
            }
        }
    }
    if query.sort == Some(EverythingSort::Use) {
        let objects: Vec<_> = projection
            .resources
            .iter()
            .filter(|r| Some(r.resource.kind.as_str()) == query.kinds.as_deref())
            .map(|r| r.resource.object())
            .collect();
        let counts: HashMap<_, _> = resource_facts::library::lifetimes(conn, &objects, at)
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|(object, count)| (object.id.to_string(), count))
            .collect();
        projection.grants["resource_uses"] =
            serde_json::to_value(counts).map_err(ApiError::internal)?;
    }
    Ok(())
}

pub(super) async fn page(
    conn: &mut Conn,
    principal: &Principal,
    rows: &[Row],
    grants: &[Grant],
    at: Time,
) -> ApiResult<HashMap<String, ResourceFacts>> {
    let subjects: Vec<_> = rows
        .iter()
        .zip(grants)
        .filter(|(_, g)| g.view)
        .filter_map(|(row, _)| {
            let kind = serde_json::from_value::<ResourceKind>(serde_json::json!(row.kind)).ok()?;
            Some(FactSubject::Resource {
                object: ObjectRef {
                    kind: kind.into(),
                    id: row.id.parse().ok()?,
                },
            })
        })
        .collect();
    let raw = resource_facts::read_with(conn, &subjects, at, Some(&RecordedUses))
        .await
        .map_err(ApiError::internal)?;
    Ok(project(conn, principal, raw)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .filter_map(|facts| match &facts.subject {
            FactSubject::Resource { object } => Some((object.id.to_string(), facts)),
            _ => None,
        })
        .collect())
}

/// A library selects the meaningful current relationships before using the shared privacy projector.
pub(crate) async fn project(
    conn: &mut Conn,
    principal: &Principal,
    mut raw: Vec<resource_facts::RawFacts>,
) -> ApiResult<Vec<ResourceFacts>> {
    for facts in &mut raw {
        let FactSubject::Resource { object } = &facts.subject else {
            continue;
        };
        facts.dependents.retain(|edge| {
            let target = edge.object.map(|o| o.kind);
            match object.kind {
                orch_access_contract::permissions::ObjectKind::Provider
                | orch_access_contract::permissions::ObjectKind::Agent
                | orch_access_contract::permissions::ObjectKind::Environment => {
                    edge.relation == DependentRelation::Binding
                }
                orch_access_contract::permissions::ObjectKind::Skill => {
                    edge.relation == DependentRelation::ExplicitGiving
                        || edge.relation == DependentRelation::Attachment
                            && target == Some(orch_access_contract::permissions::ObjectKind::Thread)
                }
                orch_access_contract::permissions::ObjectKind::McpServer => {
                    edge.relation == DependentRelation::ExplicitGiving
                        || edge.relation == DependentRelation::Attachment
                            && target
                                == Some(orch_access_contract::permissions::ObjectKind::Environment)
                }
                orch_access_contract::permissions::ObjectKind::Mount => {
                    edge.relation == DependentRelation::Attachment
                        && target
                            == Some(orch_access_contract::permissions::ObjectKind::Environment)
                }
                orch_access_contract::permissions::ObjectKind::Template
                | orch_access_contract::permissions::ObjectKind::Host => {
                    edge.relation == DependentRelation::Attachment
                        && matches!(
                            target,
                            Some(
                                orch_access_contract::permissions::ObjectKind::Environment
                                    | orch_access_contract::permissions::ObjectKind::Provider
                            )
                        )
                }
                _ => false,
            }
        });
    }
    resource_facts::project(conn, principal, Default::default(), &raw)
        .await
        .map_err(ApiError::internal)
}

/// Historical thread counts reuse the exact same source and turn deduplication as Usage.
pub(crate) async fn threads(
    conn: &mut Conn,
    ids: &[orch_types::Id],
    at: Time,
) -> ApiResult<HashMap<orch_types::Id, i64>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = orch_db::usage::rollup::resource_turn_counts(
        conn,
        ids,
        orch_db::usage::rollup::ResourceTurnDimension::Provider,
        at.get(),
    )
    .await?;
    let mut result: HashMap<_, _> = ids.iter().map(|id| (*id, 0)).collect();
    for row in rows {
        if let Some(count) = row.threads {
            result.insert(row.id, count);
        } else {
            result.remove(&row.id);
        }
    }
    Ok(result)
}
