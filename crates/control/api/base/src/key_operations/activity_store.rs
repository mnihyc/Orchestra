//! Persistence for the shared collector, plus safe historical facet projection.

use super::activity::FinishedCall;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::state::App;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyAction;

use orch_access_contract::key_scope::KeyFacet;

use orch_access_contract::key_scope::KeyObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::auth::ApiTokenActivity;

use orch_api_contract::auth::ApiTokenActivityFacet;

use orch_authz::engine::Frame;

use orch_authz::engine::Graph;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::key_activity;

use orch_types::Id;

use orch_types::Time;

use serde::Deserialize;

use serde::Serialize;

use std::collections::BTreeSet;

use std::collections::HashMap;

use std::collections::HashSet;
/// Historical metadata only. It cannot be deserialized into the trusted KeyResolvedTarget
/// used for permission decisions; ancestry here is retained only for org-history slicing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredFacet {
    pub operation: String,
    pub action: KeyAction,
    pub resource_kind: KeyTargetKind,
    pub target: KeyObjectRef,
    pub organizations: Vec<Id>,
}

impl StoredFacet {
    pub(crate) fn capture(facet: &KeyFacet) -> Self {
        let organizations = facet
            .target
            .node_ancestors
            .iter()
            .chain(std::iter::once(&facet.target.object))
            .filter(|object| object.kind == KeyTargetKind::Object(ObjectKind::Org))
            .filter_map(|object| object.id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Self {
            operation: facet.operation.clone(),
            action: facet.action,
            resource_kind: facet.resource_kind,
            target: facet.target.object,
            organizations,
        }
    }
}

/// Names are current ordinary page projections. Historical target IDs never become
/// permission facts, and reading an audit boundary never grants resource content.
async fn target_names(
    conn: &mut Conn,
    principal: &Principal,
    facets: &[Vec<StoredFacet>],
) -> ApiResult<HashMap<ObjectRef, String>> {
    let objects: HashSet<_> = facets
        .iter()
        .flatten()
        .filter_map(|facet| match (facet.target.kind, facet.target.id) {
            (KeyTargetKind::Object(kind), Some(id)) => Some(ObjectRef { kind, id }),
            _ => None,
        })
        .collect();
    if objects.is_empty() {
        return Ok(HashMap::new());
    }
    let nodes: Vec<_> = objects
        .iter()
        .filter(|o| o.kind.node().is_some())
        .map(|o| o.id)
        .collect();
    let documents: Vec<_> = objects
        .iter()
        .filter(|o| o.kind.is_document())
        .map(|o| o.id)
        .collect();
    let resources: Vec<_> = objects
        .iter()
        .filter(|o| o.kind.resource().is_some())
        .map(|o| o.id)
        .collect();
    let mut graph = Graph::load(conn, &nodes)
        .await
        .map_err(ApiError::internal)?;
    let mut sites: Vec<_> = nodes.iter().filter_map(|id| graph.site_of(*id)).collect();
    if !documents.is_empty() {
        for document in orch_authz::documents::read_into(conn, &mut graph, &documents)
            .await
            .map_err(ApiError::internal)?
        {
            if !document.deleted
                && let Some(site) = orch_authz::documents::site_of(&graph, &document)
            {
                sites.push(site);
            }
        }
    }
    if !resources.is_empty() {
        for resource in orch_authz::resources::read_into(conn, &mut graph, &resources)
            .await
            .map_err(ApiError::internal)?
        {
            if !resource.deleted
                && let Some(site) = orch_authz::resources::site_of(&graph, &resource)
            {
                sites.push(site);
            }
        }
    }
    sites.retain(|site| {
        objects.contains(&ObjectRef {
            kind: site.kind,
            id: site.id,
        })
    });
    if sites.is_empty() {
        return Ok(HashMap::new());
    }
    let mut permissions = Vec::new();
    for site in &sites {
        for permission in site.kind.permissions() {
            if !permissions.contains(&permission) {
                permissions.push(permission);
            }
        }
    }
    let ordinary = principal.ordinary(conn).await.map_err(ApiError::internal)?;
    let frame = Frame::load_with_participation(
        conn,
        &ordinary,
        Policy::default(),
        graph,
        &sites,
        &permissions,
        &[],
    )
    .await
    .map_err(ApiError::internal)?;
    Ok(sites
        .into_iter()
        .filter(|site| frame.holds_any(site))
        .map(|site| {
            (
                ObjectRef {
                    kind: site.kind,
                    id: site.id,
                },
                site.name,
            )
        })
        .collect())
}

/// The org filter applies to each facet, not merely the compound call's union of orgs.
/// Targets of another org never reach the naming lookup or the returned wire object.
pub async fn project(
    conn: &mut Conn,
    principal: &Principal,
    rows: Vec<key_activity::Row>,
    org: Option<Id>,
    show_ip: bool,
) -> ApiResult<Vec<ApiTokenActivity>> {
    let facets: Vec<Vec<StoredFacet>> = rows
        .iter()
        .map(|row| {
            let stored: Vec<StoredFacet> =
                serde_json::from_value(row.facets.clone()).map_err(ApiError::internal)?;
            Ok(stored
                .into_iter()
                .filter(|facet| org.is_none_or(|org| facet.organizations.contains(&org)))
                .collect())
        })
        .collect::<ApiResult<_>>()?;
    let names = target_names(conn, principal, &facets).await?;
    rows.into_iter()
        .zip(facets)
        .map(|(row, facets)| {
            let facets = facets
                .into_iter()
                .map(|facet| {
                    let target_name = match (facet.target.kind, facet.target.id) {
                        (KeyTargetKind::Object(kind), Some(id)) => {
                            names.get(&ObjectRef { kind, id }).cloned()
                        }
                        _ => None,
                    };
                    ApiTokenActivityFacet {
                        operation: facet.operation,
                        action: facet.action,
                        resource_kind: facet.resource_kind,
                        target: facet.target,
                        target_name,
                    }
                })
                .collect();
            Ok(ApiTokenActivity {
                id: row.id,
                at: Time::new(row.at),
                operation: row.operation,
                selection: row.selection,
                facets,
                method: row.method,
                route: row.route,
                status: u16::try_from(row.status).map_err(ApiError::internal)?,
                error_code: row.error_code,
                replayed: row.replayed,
                duration_ms: u64::try_from(row.duration_ms).map_err(ApiError::internal)?,
                request_id: row.request_id,
                ip: if show_ip { row.ip } else { None },
            })
        })
        .collect()
}

/// Validate protected names of the actual emitted native history, without repaging
/// or rendering a replacement response. Historical IDs remain metadata, never authority.
pub async fn current_names(
    conn: &mut Conn,
    principal: &Principal,
    emitted: &[ApiTokenActivity],
) -> ApiResult<bool> {
    let facets: Vec<Vec<StoredFacet>> = emitted
        .iter()
        .map(|row| {
            row.facets
                .iter()
                .map(|facet| StoredFacet {
                    operation: facet.operation.clone(),
                    action: facet.action,
                    resource_kind: facet.resource_kind,
                    target: facet.target,
                    organizations: Vec::new(),
                })
                .collect()
        })
        .collect();
    let names = target_names(conn, principal, &facets).await?;
    Ok(emitted
        .iter()
        .flat_map(|row| &row.facets)
        .all(|facet| match &facet.target_name {
            None => true,
            Some(name) => match (facet.target.kind, facet.target.id) {
                (KeyTargetKind::Object(kind), Some(id)) => {
                    names.get(&ObjectRef { kind, id }) == Some(name)
                }
                _ => false,
            },
        }))
}

/// Called exactly once by the shared HTTP/realtime boundary after its real outcome is known.
/// The boundary reports persistence failures without replacing a completed business response.
pub(crate) async fn record(app: &App, call: FinishedCall) -> ApiResult<()> {
    let facets: Vec<_> = call.facets.iter().map(StoredFacet::capture).collect();
    let organizations = facets
        .iter()
        .flat_map(|facet| facet.organizations.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let facets = serde_json::to_value(&facets).map_err(ApiError::internal)?;
    let ip = call.remote.map(|address| address.to_string());
    let error = call.error.map(|code| code.as_str().to_owned());
    let mut conn = app.db.acquire().await?;
    key_activity::append(
        &mut conn,
        &key_activity::NewCall {
            id: orch_types::new_id(),
            token: call.origin.key_id,
            account: call.origin.account_id,
            at: call.at.get(),
            operation: &call.operation,
            selection: call.selection.as_deref(),
            facets: &facets,
            orgs: &organizations,
            method: &call.method,
            route: &call.route,
            status: call.status,
            error: error.as_deref(),
            replayed: call.replayed,
            duration_ms: i64::try_from(call.duration_ms).unwrap_or(i64::MAX),
            request_id: &call.request_id,
            ip: ip.as_deref(),
        },
        app.clock.now(),
    )
    .await?;
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
pub mod tests {

    use super::*;

    use crate::key_operations::activity;

    use crate::key_operations::activity::Call;

    use crate::key_operations::activity::KeyOrigin;

    use orch_access_contract::key_scope::KeyResolvedTarget;

    use orch_api_contract::nodes::NodeGet;

    use orch_api_contract::routes::Endpoint;
    #[cfg(any(test, feature = "test-support"))]
    async fn the_shared_context_keeps_explicit_replay_and_outcome_once_without_raw_inputs() {
        let at = Time::new(chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap());
        let call = Call::http(
            NodeGet::ROUTE,
            "request-one".to_owned(),
            at,
            Some("198.51.100.7".parse().unwrap()),
        );
        let org = orch_types::new_id();
        let target = KeyObjectRef {
            kind: KeyTargetKind::Object(ObjectKind::Topic),
            id: Some(orch_types::new_id()),
        };
        let facet = KeyFacet {
            operation: "NodeGet".to_owned(),
            action: KeyAction::Read,
            resource_kind: target.kind,
            target: KeyResolvedTarget {
                object: target,
                node_ancestors: vec![KeyObjectRef {
                    kind: KeyTargetKind::Object(ObjectKind::Org),
                    id: Some(org),
                }],
                document_ancestors: Vec::new(),
                credential_account: None,
            },
        };
        call.scope(async {
            activity::authenticated(KeyOrigin {
                key_id: orch_types::new_id(),
                account_id: orch_types::new_id(),
            });
            activity::selected("NodeGet", &[facet]);
            activity::replayed();
        })
        .await;
        let result = call
            .finish(403, Some(orch_api_contract::common::ErrorCode::AuthzDenied))
            .unwrap();
        assert!(result.replayed);
        assert_eq!(result.status, 403);
        assert_eq!(result.request_id, "request-one");
        assert_eq!(result.route, "/nodes/{id}");
        let stored = StoredFacet::capture(&result.facets[0]);
        assert_eq!(stored.organizations, [org]);
        let wire = serde_json::to_value(stored).unwrap();
        let fields: BTreeSet<_> = wire
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            fields,
            BTreeSet::from([
                "operation",
                "action",
                "resource_kind",
                "target",
                "organizations"
            ])
        );
        assert!(
            call.finish(200, None).is_none(),
            "a transaction or transport retry cannot record a logical call twice"
        );
        let unknown = Call::http(NodeGet::ROUTE, "unknown".to_owned(), at, None);
        assert!(
            unknown
                .finish(
                    401,
                    Some(orch_api_contract::common::ErrorCode::AuthInvalidToken)
                )
                .is_none()
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_the_shared_context_keeps_explicit_replay_and_outcome_once_without_raw_inputs()
    -> ::futures::future::LocalBoxFuture<'static, ()> {
        ::std::boxed::Box::pin(
            self::the_shared_context_keeps_explicit_replay_and_outcome_once_without_raw_inputs(),
        )
    }
}
