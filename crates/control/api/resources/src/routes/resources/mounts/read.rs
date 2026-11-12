//! The Mount native factory shared by Get, List and the existing inventory projection.

use super::*;

use crate::routes::resources::projection;

use crate::routes::resources::projection::Rendered;

use crate::routes::resources::read;

use crate::routes::resources::read::Cardinality;

use crate::routes::resources::read::Family;

use futures::future::BoxFuture;

use orch_api_contract::resource_reads;

use orch_authz::resources::Seen;
struct Mount;

pub(super) fn mount(registry: &mut Registry) {
    read::register::<Mount, _, _>(registry, api::MountGet, api::MountList);
}

impl Family for Mount {
    type Item = api::Mount;
    type ListQuery = api::MountListQuery;
    const SPEC: &'static resource_reads::Family = &resource_reads::MOUNT;
    const CARDINALITY: Cardinality = Cardinality::Exhaustive;

    fn require_get<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: Id,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(
            conn,
            principal,
            ResourceKind::Mount,
            id,
            Permission::MountView,
        ))
    }

    fn prepare_get<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: Id,
    ) -> BoxFuture<'a, ApiResult<projection::Prepared>> {
        Box::pin(async move {
            // All three entry points already use prepare_inventory_one/show_inventory_one.
            // No location or source authority is reconstructed by this adapter.
            projection::prepare(
                ctx,
                conn,
                principal,
                projection::Selection {
                    kind: ResourceKind::Mount,
                    ids: vec![id],
                    seen: &[],
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
            let homes: Vec<_> = seen.iter().filter_map(|row| row.resource.home).collect();
            let inside = match query.org_id {
                Some(org) => {
                    orch_db::queries::resources::routes::homes_under(conn, org, &homes).await?
                }
                None => homes,
            };
            Ok(seen
                .into_iter()
                .filter(|row| row.resource.home.is_none_or(|home| inside.contains(&home)))
                .collect())
        })
    }

    fn into_items(rendered: Rendered) -> ApiResult<Vec<Self::Item>> {
        match rendered {
            Rendered::Mount(rows) => Ok(rows),
            _ => Err(ApiError::internal(
                "the Mount native factory returned another family",
            )),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
#[path = "read/tests.rs"]
pub mod tests;
