//! The ordinary Environment list consumes the shared native resource collection adapter.

use super::projection::{self, Rendered};
use super::read::{self, Cardinality, Family};
use super::*;
use futures::future::BoxFuture;
use orch_api_contract::environment::{
    Environment, EnvironmentHolding, EnvironmentList, EnvironmentListQuery,
};
use orch_api_contract::{environment_sources, resource_reads};
use orch_authz::resources::Seen;

struct Environments;

pub(crate) fn mount(registry: &mut Registry) {
    read::register_list::<Environments, _>(registry, EnvironmentList);
}

impl Family for Environments {
    type Item = Environment;
    type ListQuery = EnvironmentListQuery;
    const SPEC: &'static resource_reads::Family = &environment_sources::LIST;
    const CARDINALITY: Cardinality = Cardinality::Capped;

    fn require_get<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _id: Id,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async {
            Err(ApiError::internal(
                "the Environment list does not select a Get reading shape",
            ))
        })
    }

    fn prepare_get<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _id: Id,
    ) -> BoxFuture<'a, ApiResult<projection::Prepared>> {
        Box::pin(async {
            Err(ApiError::internal(
                "the Environment list does not prepare a Get reading shape",
            ))
        })
    }

    fn filter<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        seen: Vec<Seen>,
        query: &'a EnvironmentListQuery,
    ) -> BoxFuture<'a, ApiResult<Vec<Seen>>> {
        Box::pin(async move {
            let mut seen: Vec<_> = seen
                .into_iter()
                .filter(|each| match query.holding {
                    Some(EnvironmentHolding::Manage) => each
                        .held
                        .iter()
                        .any(|held| held.permission == Permission::EnvironmentManage),
                    Some(EnvironmentHolding::View) => each
                        .held
                        .iter()
                        .any(|held| held.permission == Permission::EnvironmentView),
                    None => true,
                })
                .collect();
            if let Some(org) = query.org_id {
                let homes: Vec<_> = seen.iter().filter_map(|each| each.resource.home).collect();
                let inside =
                    orch_db::queries::resources::routes::homes_under(conn, org, &homes).await?;
                seen.retain(|each| {
                    each.resource
                        .home
                        .is_some_and(|home| inside.contains(&home))
                });
            }
            Ok(seen)
        })
    }

    fn into_items(rendered: Rendered) -> ApiResult<Vec<Environment>> {
        match rendered {
            Rendered::Environment(rows) => Ok(rows),
            _ => Err(ApiError::internal(
                "the Environment native list returned another family",
            )),
        }
    }
}
