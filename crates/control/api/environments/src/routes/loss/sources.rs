//! Actual native recovery inventory consumed by the trusted body-read adapter.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::environment_sources as sources;

use orch_api_contract::environment_sources::RecoveryInventorySource as Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_types::Id;

use std::collections::BTreeMap;

use std::collections::BTreeSet;
pub(super) struct Recovery;

async fn require_target(
    conn: &mut Conn,
    principal: &Principal,
    target: api::LossTarget,
) -> ApiResult<()> {
    let result = require_recovery_target(conn, principal, target).await;
    if native::modern(principal.caller()) {
        result.map_err(authorization::problem_without_content)
    } else {
        result
    }
}

pub(super) struct Inventory {
    target: api::LossTarget,
    environments: Vec<(Id, String)>,
    others: i64,
}

fn collection(groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>>) -> native::Facts<Source> {
    native::Facts::collection(Source::Subjects, groups.into_iter().collect())
}

async fn subject(
    conn: &mut Conn,
    target: api::LossTarget,
) -> ApiResult<(KeyTargetKind, KeyResolvedTarget)> {
    let object = target.object();
    let kind = object.kind;
    let identity = if kind == ObjectKind::Machine {
        targets::machine_subject(conn, target.id()).await?
    } else {
        targets::page_object(conn, object).await?
    };
    Ok((KeyTargetKind::Object(kind), identity))
}

impl native::Selected for Recovery {
    type Source = Source;
    type Query = ((), api::LossPreflightBody);
    type Value = Inventory;
    type Reply = api::RecoverySources;
    const DECLARATION: &'static Declaration<Source> = &sources::LOSS_RECOVERY;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Inventory, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let target = query.1.target;
            let modern = native::modern(caller);
            let mut groups = BTreeMap::new();
            if modern {
                let (kind, target) = subject(conn, target).await?;
                groups.insert(kind, vec![target]);
            }
            let known = if modern {
                vec![collection(groups.clone())]
            } else {
                Vec::new()
            };
            let plan = native::bind_known(ctx, Self::DECLARATION, &known)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            require_target(conn, &principal, target).await?;
            let inventory = orch_db::queries::loss::inventory(conn, target.kind(), target.id())
                .await?
                .ok_or_else(ApiError::not_found)?;
            let mut environments = Vec::new();
            let mut others = 0;
            for environment in inventory.environments {
                if environment.status == "removed" {
                    continue;
                }
                let object = ObjectRef {
                    kind: ObjectKind::Environment,
                    id: environment.id,
                };
                let identity = if modern {
                    let target = targets::page_object(conn, object).await?;
                    if !native::permits(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &Source::Subjects,
                        KeyTargetKind::Object(ObjectKind::Environment),
                        &target,
                    )? {
                        others += 1;
                        continue;
                    }
                    Some(target)
                } else {
                    None
                };
                let access = orch_engine::operation_access::review(
                    conn,
                    &principal,
                    object,
                    Permission::EnvironmentManage,
                )
                .await;
                if matches!(access, Err(orch_engine::updates::UpdateError::Denied)) {
                    others += 1;
                    continue;
                }
                access.map_err(crate::routes::environments::updates::refused)?;
                environments.push((environment.id, environment.name));
                if let Some(target) = identity {
                    groups
                        .entry(KeyTargetKind::Object(ObjectKind::Environment))
                        .or_default()
                        .push(target);
                }
            }
            Ok((
                Inventory {
                    target,
                    environments,
                    others,
                },
                if modern {
                    vec![collection(groups)]
                } else {
                    Vec::new()
                },
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
                return Err(ApiError::internal("the recovery inventory changed account"));
            }
            Ok(())
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Inventory,
    ) -> BoxFuture<'a, ApiResult<api::RecoverySources>> {
        Box::pin(async move {
            require_target(conn, principal, value.target).await?;
            Ok(api::RecoverySources {
                environments: value
                    .environments
                    .iter()
                    .map(|(id, name)| api::LossResource {
                        id: *id,
                        name: Some(name.clone()),
                    })
                    .collect(),
                others: value.others,
            })
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a Inventory,
        fresh: &'a Inventory,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let current: BTreeSet<_> = fresh.environments.iter().map(|(id, _)| *id).collect();
            Ok(opened.target == fresh.target
                && opened
                    .environments
                    .iter()
                    .all(|(id, _)| current.contains(id)))
        })
    }
}
