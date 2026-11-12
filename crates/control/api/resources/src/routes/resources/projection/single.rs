//! Native singleton cardinality, including individual styles and metadata generation cuts.

use super::*;

use crate::routes::document_tree;

use crate::routes::documents;

use crate::routes::environments;

use orch_access_contract::permissions::ObjectKind as K;

use orch_api_contract::resources::Resource;

use orch_types::Time;

use std::collections::BTreeMap;

pub use orch_api_base::routes::resources::projection::single::Body;

pub use orch_api_base::routes::resources::projection::single::Single;

pub use orch_api_base::routes::resources::projection::single::Roles;

pub use orch_api_base::routes::resources::projection::single::Origin;

pub use orch_api_base::routes::resources::projection::single::Bounds;

pub use orch_api_base::routes::resources::projection::single::Snapshot;

pub use orch_api_base::routes::resources::projection::single::Validation;
/// An individual projection's native subject may differ from the ownership page.
pub(crate) async fn subject(conn: &mut Conn, object: ObjectRef) -> ApiResult<KeyResolvedTarget> {
    if object.kind == K::Machine {
        return targets::machine_subject(conn, object.id).await;
    }
    if object.kind == K::McpServer {
        let selected = orch_db::queries::resources::mcp::subject(conn, object.id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if let Some(id) = selected.environment_id {
            return targets::page_object(
                conn,
                ObjectRef {
                    kind: K::Environment,
                    id,
                },
            )
            .await;
        }
    }
    targets::page_object(conn, object).await
}
orch_api_base::__api_bind_227c63a964!();

async fn seen(conn: &mut Conn, principal: &Principal, object: ObjectRef) -> ApiResult<Seen> {
    let (_, resource, _) = orch_authz::resources::site(conn, object)
        .await
        .map_err(ApiError::internal)?;
    let held =
        orch_authz::resources::effective(conn, principal, object, orch_authz::Policy::default())
            .await
            .map_err(ApiError::internal)?;
    Ok(Seen {
        resource,
        admin_place: held.admin_place,
        admin_only: held.admin_only,
        admin_access: held.admin_access,
        participating: held.participating,
        owner: held.owner,
        administers: held.administers,
        held: held.held,
    })
}

/// Select the existing native individual style; every resource reuses the common factual handoff.
pub(crate) async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
) -> ApiResult<Single> {
    let id = object.id;
    let body = match object.kind {
        K::Document => Body::Document(Box::new(
            documents::metadata_projection::prepare(conn, principal, id).await?,
        )),
        K::DocumentContainer => {
            let read = orch_engine::document_tree::read_in(
                conn,
                orch_engine::document_tree::Actor::Person(principal),
                orch_engine::document_tree::Item::Container(id),
                principal.now(),
            )
            .await;
            Body::Folder(document_tree::result(conn, read).await?)
        }
        kind => {
            let view = match kind {
                K::Provider => View::IndividualProvider(Box::new(
                    providers::prepare_handover_projection(ctx, conn, principal, id).await?,
                )),
                K::Agent => View::Agent(agents::prepare_projection(conn, principal, id).await?),
                K::Template => {
                    View::Template(templates::prepare_projection(conn, principal, id).await?)
                }
                K::Environment => {
                    let native = seen(conn, principal, object).await?;
                    View::IndividualEnvironment(
                        environments::prepare_inventory(ctx, conn, principal, &[native]).await?,
                        id,
                    )
                }
                K::Host => {
                    let native = seen(conn, principal, object).await?;
                    View::Host(hosts::prepare_inventory(ctx, conn, principal, &[native]).await?)
                }
                K::Machine => {
                    let row = orch_db::queries::resources::machine(conn, id)
                        .await?
                        .ok_or_else(ApiError::not_found)?;
                    View::Machine(machines::prepare_inventory(conn, principal, vec![row]).await?)
                }
                K::McpServer => View::Mcp(
                    mcp::prepare(conn, principal, &[id], None, mcp::Style::Individual).await?,
                    NameBounds::default(),
                ),
                K::Mount => {
                    View::Mount(mounts::prepare_inventory(ctx, conn, principal, &[id]).await?)
                }
                K::Skill => {
                    View::Skill(skills::prepare_single_projection(conn, principal, id).await?)
                }
                _ => return Err(ApiError::not_found()),
            };
            Body::Resource(Box::new(Prepared::from_view(conn, view).await?))
        }
    };
    Ok(Single { object, body })
}
orch_api_base::__api_bind_dbb26ef146!();

async fn current_cut(
    conn: &mut Conn,
    principal: &Principal,
    cut: Option<&document_tree::metadata::Cut>,
    now: Time,
) -> ApiResult<bool> {
    let Some(cut) = cut else {
        return Ok(true);
    };
    match document_tree::metadata::unchanged(conn, principal, cut, now).await {
        Ok(()) => Ok(true),
        Err(error)
            if error.code() == orch_api_contract::common::ErrorCode::ConflictStaleRevision =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}
orch_api_base::__api_bind_01cdf1c787!();

async fn bind_origins(
    conn: &mut Conn,
    plan: &mut OperationPlan,
    role: &'static str,
    origins: &[Origin],
) -> ApiResult<()> {
    let index = plan
        .variant()
        .facets
        .iter()
        .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
        .ok_or_else(|| ApiError::internal("the native singleton source role is undeclared"))?;
    let mut groups = BTreeMap::<KeyTargetKind, Vec<KeyResolvedTarget>>::new();
    for origin in origins {
        let target = if origin.object.kind == K::Machine && origin.retained {
            targets::machine_identities(conn, &[origin.object.id])
                .await?
                .remove(&origin.object.id)
                .ok_or_else(ApiError::not_found)?
        } else if origin.object.kind == K::Machine {
            targets::machine_subject(conn, origin.object.id).await?
        } else if origin.retained {
            targets::resource_identity(conn, origin.object).await?
        } else {
            targets::page_object(conn, origin.object).await?
        };
        groups
            .entry(KeyTargetKind::Object(origin.object.kind))
            .or_default()
            .push(target);
    }
    if groups.is_empty() {
        plan.bind_empty(index)
    } else {
        plan.bind_collection(index, &groups.into_iter().collect::<Vec<_>>())
    }
}
orch_api_base::__api_bind_ec4d5a12fd!();

fn one<T>(rows: Vec<T>) -> ApiResult<T> {
    let mut rows = rows.into_iter();
    let value = rows.next().ok_or_else(ApiError::not_found)?;
    if rows.next().is_some() {
        return Err(ApiError::internal(
            "the native singleton returned several rows",
        ));
    }
    Ok(value)
}

fn resource(rows: Rendered) -> ApiResult<Resource> {
    Ok(match rows {
        Rendered::Agent(rows) => Resource::Agent(one(rows)?),
        Rendered::Provider(rows) => Resource::Provider(one(rows)?),
        Rendered::Template(rows) => Resource::Template(one(rows)?),
        Rendered::Mcp(rows) => Resource::McpServer(one(rows)?),
        Rendered::Host(rows) => Resource::Host(one(rows)?),
        Rendered::Environment(rows) => Resource::Environment(one(rows)?),
        Rendered::Machine(rows) => Resource::Machine(one(rows)?),
        Rendered::Mount(rows) => Resource::Mount(one(rows)?),
        Rendered::Skill(rows) => Resource::Skill(one(rows)?),
    })
}
orch_api_base::__api_bind_07ede148ea!();

#[cfg(any(test, feature = "test-support"))]
#[path = "single/tests.rs"]
pub mod tests;
