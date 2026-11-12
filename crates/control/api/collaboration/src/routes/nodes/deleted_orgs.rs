//! Recoverable owned org identities through the shared native selected-row read.

use crate::key_operations::native;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::NodeRef;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::ownership::DeletedOrg;

use orch_api_contract::ownership_sources as sources;

use orch_api_contract::ownership_sources::Source;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::hierarchy;

use orch_db::queries::hierarchy::DeletedRow;

use orch_types::Id;

use orch_types::Time;
pub(super) struct Orgs;

impl native::Selected for Orgs {
    type Source = Source;
    type Query = ();
    type Value = Vec<DeletedRow>;
    type Reply = Vec<DeletedOrg>;
    const DECLARATION: &'static Declaration<Source> = &sources::DELETED_ORGS;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Vec<DeletedRow>, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let days = ctx.app.config.retention.soft_delete_days;
            let rows = hierarchy::deleted_orgs_of(
                conn,
                caller.user(),
                super::removal::purged_before(ctx.now, days),
            )
            .await?;
            if !native::prepared_read(ctx, caller) {
                return Ok((rows, Vec::new()));
            }
            let plan = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            let kind = KeyTargetKind::Object(ObjectKind::Org);
            let ids = rows.iter().map(|row| row.id).collect::<Vec<_>>();
            let graph = orch_authz::engine::Graph::load(conn, &ids)
                .await
                .map_err(crate::middleware::authorization::restoration_error)?;
            let mut selected = Vec::new();
            let mut facts = Vec::new();
            for row in rows {
                let target =
                    orch_engine::operation_scope::targets::from_node_identity(&graph, row.id)
                        .ok_or_else(ApiError::not_found)?;
                if target.object.kind != kind {
                    return Err(ApiError::not_found());
                }
                if native::permits(
                    &plan,
                    Self::DECLARATION,
                    caller,
                    &Source::DeletedOrgs,
                    kind,
                    &target,
                )? {
                    selected.push(row);
                    facts.push(target);
                }
            }
            let groups = if facts.is_empty() {
                Vec::new()
            } else {
                vec![(kind, facts)]
            };
            Ok((
                selected,
                vec![native::Facts::collection(Source::DeletedOrgs, groups)],
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::internal("the deletion list changed account"));
            }
            Ok(())
        })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Vec<DeletedRow>,
    ) -> BoxFuture<'a, ApiResult<Vec<DeletedOrg>>> {
        Box::pin(async move {
            let users = value
                .iter()
                .filter_map(|row| row.deleted_by)
                .collect::<Vec<Id>>();
            let people = refusals::user_refs(conn, &users).await?;
            Ok(value
                .iter()
                .map(|row| DeletedOrg {
                    org: NodeRef {
                        thread_display: None,
                        id: row.id,
                        kind: NodeKind::Org,
                        name: Some(row.name.clone()),
                        redacted: false,
                    },
                    slug: row.slug.clone().unwrap_or_default(),
                    deleted_at: Time::new(row.deleted_at),
                    deleted_by: row.deleted_by.and_then(|user| people.get(&user).cloned()),
                    purge_at: Time::new(super::removal::purge_at(
                        row.deleted_at,
                        ctx.app.config.retention.soft_delete_days,
                    )),
                })
                .collect())
        })
    }
}
