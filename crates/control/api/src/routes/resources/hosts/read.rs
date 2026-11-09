//! The native Host read factory and its typed collection declaration.

use super::*;
use crate::routes::resources::projection::{self, Rendered};
use crate::routes::resources::read::{self, Cardinality, Family};
use futures::future::BoxFuture;
use orch_api_contract::resource_reads;
use orch_authz::{engine::Graph, resources::Seen};

struct Hosts;

pub(super) fn mount(registry: &mut Registry) {
    read::register::<Hosts, _, _>(registry, api::HostGet, api::HostList);
}

impl Family for Hosts {
    type Item = api::Host;
    type ListQuery = api::HostListQuery;
    const SPEC: &'static resource_reads::Family = &resource_reads::HOST;
    const CARDINALITY: Cardinality = Cardinality::Capped;

    fn require_get<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: Id,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(
            conn,
            principal,
            ResourceKind::Host,
            id,
            Permission::HostView,
        ))
    }

    fn prepare_get<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: Id,
    ) -> BoxFuture<'a, ApiResult<projection::Prepared>> {
        Box::pin(async move {
            let object = ObjectRef {
                kind: ResourceKind::Host.into(),
                id,
            };
            let resource = resources::read_into(conn, &mut Graph::default(), &[id])
                .await
                .map_err(ApiError::internal)?
                .into_iter()
                .find(|row| row.object() == object)
                .ok_or_else(ApiError::not_found)?;
            let effective = resources::effective(conn, principal, object, Policy::default())
                .await
                .map_err(ApiError::internal)?;
            let seen = Seen {
                resource,
                admin_place: effective.admin_place,
                admin_only: effective.admin_only,
                admin_access: effective.admin_access,
                participating: effective.participating,
                owner: effective.owner,
                administers: effective.administers,
                held: effective.held,
            };
            projection::prepare(
                ctx,
                conn,
                principal,
                projection::Selection {
                    kind: ResourceKind::Host,
                    ids: vec![id],
                    seen: &[seen],
                },
            )
            .await?
            .ok_or_else(ApiError::not_found)
        })
    }

    fn filter<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        seen: Vec<Seen>,
        query: &'a Self::ListQuery,
    ) -> BoxFuture<'a, ApiResult<Vec<Seen>>> {
        Box::pin(async move {
            let mut seen: Vec<_> = seen
                .into_iter()
                .filter(|each| {
                    each.administers
                        || each.held.iter().any(|held| {
                            matches!(
                                held.permission,
                                Permission::HostView
                                    | Permission::HostUse
                                    | Permission::HostManage
                                    | Permission::AccessManage
                            )
                        })
                })
                .collect();
            if let Some(org) = query.org_id {
                let homes: Vec<_> = seen.iter().filter_map(|row| row.resource.home).collect();
                let inside = queries::homes_under(conn, org, &homes).await?;
                seen.retain(|row| row.resource.home.is_none_or(|home| inside.contains(&home)));
            }
            Ok(seen)
        })
    }

    fn after_get<'a>(ctx: &'a Ctx, host: &'a Self::Item) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if host.ports_proposal.is_none()
                && host.machine.online
                && host
                    .access
                    .permissions
                    .iter()
                    .any(|permission| permission == "host.manage")
            {
                crate::host_ports::ask_in_background(ctx, host.base.id, host.machine.id);
            }
            Ok(())
        })
    }

    fn into_items(rendered: Rendered) -> ApiResult<Vec<Self::Item>> {
        match rendered {
            Rendered::Host(rows) => Ok(rows),
            _ => Err(ApiError::internal(
                "the Host native factory returned another family",
            )),
        }
    }
}
