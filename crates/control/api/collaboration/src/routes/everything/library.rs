//! Library views, privacy projections and recorded-use ordering share the inventory's single read.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::Level;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::resource_facts::DependentRelation;

use orch_api_contract::everything::EverythingListQuery;

use orch_api_contract::everything::EverythingSort;

use orch_api_contract::resource_facts::FactSubject;

use orch_api_contract::resource_facts::ResourceFacts;

use orch_api_contract::resources::ProviderListScope;

use orch_authz::Principal;

use orch_authz::everything::Projection;

use orch_db::Conn;

use orch_db::queries::everything::Grant;

use orch_db::queries::everything::Row;

use orch_engine::resource_facts;

use orch_engine::resource_facts::library::RecordedUses;

use orch_types::Time;

use std::collections::HashMap;

use std::collections::HashSet;
pub(super) fn validate(query: &EverythingListQuery, kinds: &[String]) -> ApiResult<()> {
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
            .map_err(crate::middleware::authorization::restoration_error)?;
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
            .map_err(crate::key_operations::snapshot_read::engine_error)?
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
    let raw = raw_page(conn, rows, grants, at).await?;
    Ok(keyed(
        resource_facts::project(conn, principal, Default::default(), &raw)
            .await
            .map_err(crate::key_operations::snapshot_read::engine_error)?,
    ))
}

async fn raw_page(
    conn: &mut Conn,
    rows: &[Row],
    grants: &[Grant],
    at: Time,
) -> ApiResult<Vec<resource_facts::RawFacts>> {
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
        .map_err(crate::key_operations::snapshot_read::engine_error)?;
    let mut raw = raw;
    select_relations(&mut raw);
    Ok(raw)
}

pub(super) struct Prepared {
    raw: Vec<resource_facts::RawFacts>,
    visibility: resource_facts::PreparedVisibility,
}

impl Prepared {
    pub(super) fn name_sources(&self) -> impl Iterator<Item = ObjectRef> + '_ {
        self.visibility.sources()
    }

    pub(super) fn bound_sources(
        &mut self,
        allowed: &HashSet<(
            orch_access_contract::permissions::ObjectKind,
            orch_types::Id,
        )>,
    ) {
        self.visibility.restrict_to(allowed);
    }

    pub(super) fn render(&self) -> HashMap<String, ResourceFacts> {
        keyed(self.visibility.project(&self.raw))
    }
}

pub(super) async fn prepare_page(
    conn: &mut Conn,
    principal: &Principal,
    rows: &[Row],
    grants: &[Grant],
    at: Time,
) -> ApiResult<Prepared> {
    let raw = raw_page(conn, rows, grants, at).await?;
    let visibility = resource_facts::prepare_projection(conn, principal, Default::default(), &raw)
        .await
        .map_err(crate::key_operations::snapshot_read::engine_error)?;
    Ok(Prepared { raw, visibility })
}

fn keyed(rows: Vec<ResourceFacts>) -> HashMap<String, ResourceFacts> {
    rows.into_iter()
        .filter_map(|facts| match &facts.subject {
            FactSubject::Resource { object } => Some((object.id.to_string(), facts)),
            _ => None,
        })
        .collect()
}

pub(crate) fn select_relations(raw: &mut [resource_facts::RawFacts]) {
    for facts in raw {
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
}
orch_api_base::__api_bind_874c2d3b53!();

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
orch_api_base::__api_bind_3582a8c8ce!();
