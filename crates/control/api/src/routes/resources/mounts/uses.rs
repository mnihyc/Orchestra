//! Native Mount usage selection shared by its reader and the existing deletion review.

use super::{api, node_view, require, rows};
use crate::key_operations::native;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, OperationPlan};
use crate::routes::resources::view::environment_refs;
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, Permission, ResourceKind};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::mount_sources::{self as sources, UseSource};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::{Caller, Policy, Principal, resources};
use orch_db::{Conn, DbError};
use orch_engine::operation_scope::targets::{self, TargetError};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
struct Frozen {
    mount: Id,
    scopes: BTreeSet<Id>,
    environments: BTreeSet<Id>,
    visible_scopes: BTreeSet<Id>,
    visible_environments: BTreeSet<Id>,
}

pub(super) struct Prepared {
    reply: api::MountUsesGetReply,
    frozen: Frozen,
}

pub(super) struct Rendered {
    ctx: Ctx,
    accepted: Caller,
    reply: api::MountUsesGetReply,
    frozen: Frozen,
}

pub(super) struct Uses;

struct Bounds<'a> {
    plan: &'a OperationPlan,
    caller: &'a Caller,
}

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the Mount usage source is no longer admitted",
    )
}

fn target_error(error: TargetError) -> ApiError {
    match error {
        TargetError::Missing => ApiError::not_found(),
        TargetError::Db(error) => error.into(),
        TargetError::Authz(error) => ApiError::internal(error),
    }
}

async fn mount_target(conn: &mut Conn, mount: Id) -> ApiResult<KeyResolvedTarget> {
    targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Mount,
            id: mount,
        },
    )
    .await
    .map_err(target_error)
}

async fn environment_targets(
    conn: &mut Conn,
    ids: &[Id],
) -> ApiResult<BTreeMap<Id, KeyResolvedTarget>> {
    let mut graph = orch_authz::engine::Graph::default();
    let found = resources::read_into(conn, &mut graph, ids)
        .await
        .map_err(ApiError::internal)?;
    Ok(found
        .into_iter()
        .filter(|row| row.kind == ResourceKind::Environment)
        .filter_map(|row| {
            targets::from_resource_identity(&graph, &row).map(|target| (row.id, target))
        })
        .collect())
}

async fn facts(conn: &mut Conn, frozen: &Frozen) -> ApiResult<Vec<native::Facts<UseSource>>> {
    let primary = mount_target(conn, frozen.mount).await?;
    let graph =
        orch_authz::engine::Graph::load(conn, &frozen.scopes.iter().copied().collect::<Vec<_>>())
            .await
            .map_err(ApiError::internal)?;
    let mut scopes: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>> = BTreeMap::new();
    for id in &frozen.scopes {
        let target = targets::from_node_identity(&graph, *id).ok_or_else(ApiError::not_found)?;
        scopes.entry(target.object.kind).or_default().push(target);
    }
    let mut environments = environment_targets(
        conn,
        &frozen.environments.iter().copied().collect::<Vec<_>>(),
    )
    .await?;
    let environments = frozen
        .environments
        .iter()
        .map(|id| environments.remove(id).ok_or_else(ApiError::not_found))
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(vec![
        native::Facts::one(
            UseSource::Mount,
            KeyTargetKind::Object(ObjectKind::Mount),
            primary,
        ),
        native::Facts::collection(UseSource::Scopes, scopes.into_iter().collect()),
        native::Facts::collection(
            UseSource::Environments,
            if environments.is_empty() {
                Vec::new()
            } else {
                vec![(KeyTargetKind::Object(ObjectKind::Environment), environments)]
            },
        ),
    ])
}

/// Native filters run on stored rows before adding either names or hidden counts.
async fn capture(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    mount: Id,
    bounds: Option<Bounds<'_>>,
) -> ApiResult<Prepared> {
    let mut frozen = Frozen {
        mount,
        scopes: BTreeSet::new(),
        environments: BTreeSet::new(),
        visible_scopes: BTreeSet::new(),
        visible_environments: BTreeSet::new(),
    };
    let scopes = rows::scopes(conn, mount).await?;
    let ids: Vec<_> = scopes.iter().map(|(id, _)| *id).collect();
    let graph = if bounds.is_some() {
        orch_authz::engine::Graph::load(conn, &ids)
            .await
            .map_err(ApiError::internal)?
    } else {
        orch_authz::engine::Graph::default()
    };
    let mut selected = Vec::new();
    for (id, value) in scopes {
        if let Some(bounds) = &bounds {
            let target = targets::from_node_identity(&graph, id).ok_or_else(ApiError::not_found)?;
            if !native::permits(
                bounds.plan,
                &sources::USES,
                bounds.caller,
                &UseSource::Scopes,
                target.object.kind,
                &target,
            )? {
                continue;
            }
        }
        frozen.scopes.insert(id);
        selected.push((id, value));
    }
    let refs = node_view::refs(
        conn,
        principal,
        &selected.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    let mut reply = api::MountUsesGetReply {
        scopes: Vec::new(),
        environments: Vec::new(),
        hidden: 0,
    };
    for (id, value) in selected {
        match refs.get(&id).filter(|node| !node.redacted) {
            Some(node) => {
                frozen.visible_scopes.insert(id);
                reply.scopes.push(api::MountScope {
                    node: node.clone(),
                    value: serde_json::from_value(value).map_err(ApiError::internal)?,
                });
            }
            None => reply.hidden += 1,
        }
    }
    let candidates = rows::candidates(conn, mount).await?;
    let targets = if bounds.is_some() {
        environment_targets(conn, &candidates).await?
    } else {
        BTreeMap::new()
    };
    for environment in candidates {
        if let Some(bounds) = &bounds {
            let target = targets.get(&environment).ok_or_else(ApiError::not_found)?;
            if !native::permits(
                bounds.plan,
                &sources::USES,
                bounds.caller,
                &UseSource::Environments,
                KeyTargetKind::Object(ObjectKind::Environment),
                target,
            )? {
                continue;
            }
        }
        if !enabled(conn, mount, environment).await? {
            continue;
        }
        match crate::routes::environments::reading::decide(conn, principal, environment).await {
            Ok(_) => {
                let refs = environment_refs(conn, &[environment], &[], Time::new(ctx.now)).await?;
                if let Some((_, environment)) = refs.into_iter().next() {
                    let facts: Option<Json> =
                        sqlx::query_scalar("SELECT facts FROM environments WHERE id=$1")
                            .bind(environment.id)
                            .fetch_optional(&mut *conn)
                            .await
                            .map_err(DbError::from)?;
                    let state = facts
                        .as_ref()
                        .and_then(|facts| facts.get("mounts"))
                        .and_then(Json::as_array)
                        .and_then(|mounts| {
                            mounts
                                .iter()
                                .find(|value| value.get("mount_id") == Some(&json!(mount)))
                        })
                        .and_then(|value| value.get("state"))
                        .and_then(Json::as_str)
                        .unwrap_or("pending")
                        .to_owned();
                    frozen.environments.insert(environment.id);
                    frozen.visible_environments.insert(environment.id);
                    reply.environments.push(api::MountUse {
                        environment,
                        state,
                        workspace: false,
                    });
                }
            }
            Err(_) => {
                frozen.environments.insert(environment);
                reply.hidden += 1;
            }
        }
    }
    Ok(Prepared { reply, frozen })
}

async fn enabled(conn: &mut Conn, mount: Id, environment: Id) -> ApiResult<bool> {
    let value: Option<Json> = orch_settings::get(
        conn,
        orch_settings::Place::Environment(environment),
        orch_settings_contract::registry::keys::ENV_MOUNTS,
    )
    .await
    .map_err(ApiError::internal)?;
    Ok(value
        .as_ref()
        .and_then(|mounts| mounts.get(mount.to_string()))
        .is_some_and(|value| *value != json!(false)))
}

/// The original unbounded native deletion review keeps its own operation and authority.
pub(super) async fn ordinary(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    mount: Id,
) -> ApiResult<api::MountUsesGetReply> {
    Ok(capture(ctx, conn, principal, mount, None).await?.reply)
}

impl native::Selected for Uses {
    type Source = UseSource;
    type Query = ();
    type Value = Prepared;
    type Reply = Rendered;
    const DECLARATION: &'static Declaration<UseSource> = &sources::USES;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<UseSource>>)>> {
        Box::pin(async move {
            let mount = ctx.id("id")?;
            let target = mount_target(conn, mount).await?;
            let known = native::bind_known(
                ctx,
                Self::DECLARATION,
                &[native::Facts::one(
                    UseSource::Mount,
                    KeyTargetKind::Object(ObjectKind::Mount),
                    target,
                )],
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require(
                conn,
                &principal,
                ResourceKind::Mount,
                mount,
                Permission::MountView,
            )
            .await?;
            let prepared = capture(
                ctx,
                conn,
                &principal,
                mount,
                native::modern(caller).then_some(Bounds {
                    plan: &known,
                    caller,
                }),
            )
            .await?;
            let facts = facts(conn, &prepared.frozen).await?;
            Ok((prepared, facts))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require(
                conn,
                principal,
                ResourceKind::Mount,
                ctx.id("id")?,
                Permission::MountView,
            )
            .await
        })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<Rendered>> {
        Box::pin(async move {
            Ok(Rendered {
                ctx: ctx.clone(),
                accepted: principal.caller().clone(),
                reply: value.reply.clone(),
                frozen: value.frozen.clone(),
            })
        })
    }
}

async fn check(ctx: &Ctx, conn: &mut Conn, reader: &Principal, frozen: &Frozen) -> ApiResult<()> {
    let plan = native::bind(ctx, &sources::USES, &facts(conn, frozen).await?)?;
    if native::modern(reader.caller()) {
        plan.clone().admit(reader.caller())?;
    }
    let principal = native::preparation_principal(ctx, conn, reader.caller(), &plan).await?;
    require(
        conn,
        &principal,
        ResourceKind::Mount,
        frozen.mount,
        Permission::MountView,
    )
    .await?;
    let scopes: BTreeSet<_> = rows::scopes(conn, frozen.mount)
        .await?
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    if !frozen.scopes.is_subset(&scopes) {
        return Err(refused());
    }
    let refs = node_view::refs(
        conn,
        &principal,
        &frozen.visible_scopes.iter().copied().collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    if frozen
        .visible_scopes
        .iter()
        .any(|id| refs.get(id).is_none_or(|node| node.redacted))
    {
        return Err(refused());
    }
    let candidates: BTreeSet<_> = rows::candidates(conn, frozen.mount)
        .await?
        .into_iter()
        .collect();
    if !frozen.environments.is_subset(&candidates) {
        return Err(refused());
    }
    for id in &frozen.environments {
        if !enabled(conn, frozen.mount, *id).await? {
            return Err(refused());
        }
    }
    for id in &frozen.visible_environments {
        crate::routes::environments::reading::decide(conn, &principal, *id).await?;
    }
    Ok(())
}

pub(super) async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<api::MountUsesGetReply> {
    let value = native::selected_read::<Uses>(ctx, (), ()).await?;
    let (mut tx, guard) = transaction_guard::begin(&value.ctx).await?;
    check(&value.ctx, &mut tx, guard.principal(), &value.frozen).await?;
    guard.check_deadline(&value.ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    guard.check_deadline(&value.ctx)?;
    crate::key_operations::publication::retain(
        &value.ctx,
        &value.accepted,
        value.frozen,
        |ctx, conn, reader, frozen| Box::pin(check(ctx, conn, reader, frozen)),
    )?;
    Ok(value.reply)
}
