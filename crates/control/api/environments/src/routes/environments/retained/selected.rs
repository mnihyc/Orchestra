//! Native retained Environment selection consumed by the shared selected-row adapter.

use super::*;

use crate::key_operations::native;

use crate::registry::OperationPlan;

use crate::routes::access::operations::NameRoles;

use crate::routes::access::operations::projection_names;

use crate::routes::access::page::NameSources;

use crate::routes::nodes::read_sources::Sources;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::common::Page;

use orch_api_contract::environment_sources as sources;

use orch_api_contract::environment_sources::RetainedSource as Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::engine::Graph;

use std::collections::BTreeSet;
const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

const HOST: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Host);

const NAMES: NameRoles = NameRoles {
    groups: None,
    nodes: [
        (ObjectKind::Org, "deleted_environment_names_orgs"),
        (ObjectKind::Project, "deleted_environment_names_projects"),
        (ObjectKind::Topic, "deleted_environment_names_topics"),
        (ObjectKind::Thread, "deleted_environment_names_threads"),
    ],
    containers: None,
};

pub(super) struct RetainedRows;

pub(super) struct RetainedPage {
    rows: Vec<PreparedRead>,
    next_cursor: Option<String>,
}

fn collection(
    source: Source,
    kind: KeyTargetKind,
    targets: Vec<KeyResolvedTarget>,
) -> native::Facts<Source> {
    native::Facts::collection(
        source,
        if targets.is_empty() {
            Vec::new()
        } else {
            vec![(kind, targets)]
        },
    )
}

impl native::Selected for RetainedRows {
    type Source = Source;
    type Query = api::NodeDeletedEnvironmentListQuery;
    type Value = RetainedPage;
    type Reply = Page<api::DeletedEnvironment>;
    const DECLARATION: &'static Declaration<Source> = &sources::DELETED_LIST;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(RetainedPage, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let node = ctx.id("id")?;
            let limit = usize::try_from(
                query
                    .limit
                    .unwrap_or(orch_api_contract::common::DEFAULT_LIMIT)
                    .clamp(1, orch_api_contract::common::MAX_LIMIT),
            )
            .map_err(ApiError::internal)?;
            let mut after = query
                .cursor
                .as_deref()
                .map(|cursor| {
                    cursor.parse::<Id>().map_err(|_| {
                        super::super::invalid("cursor", "is not a cursor of this list")
                    })
                })
                .transpose()?;
            let raw =
                authorization::load_principal(conn, caller.clone(), ctx.app.clock.time()).await?;
            let raw = crate::sign_in::second_step::hold(conn, ctx.user()?, raw).await?;
            let plan = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            let modern = native::modern(caller);
            let mut selected = Vec::new();
            let mut selected_targets = Vec::new();
            loop {
                let candidates:Vec<Id>=sqlx::query_scalar("SELECT e.id FROM environments e JOIN nodes n ON n.id=e.home_node_id WHERE e.deleted_at IS NOT NULL AND e.purged_at IS NULL AND e.removal_phase IN ('kept','restoring','purging') AND (e.home_node_id=$1 OR ($2 AND $1=ANY(n.path))) AND ($3::uuid IS NULL OR e.id>$3) ORDER BY e.id LIMIT $4")
                    .bind(node).bind(query.under.unwrap_or(false)).bind(after)
                    .bind(i64::from(orch_api_contract::common::MAX_LIMIT)).fetch_all(&mut *conn).await.map_err(DbError::from)?;
                if candidates.is_empty() {
                    break;
                }
                let mut targets = if modern {
                    Sources::from_objects(candidates.iter().map(|id| ObjectRef {
                        kind: ObjectKind::Environment,
                        id: *id,
                    }))
                    .identity_targets(conn)
                    .await?
                } else {
                    HashMap::new()
                };
                for id in &candidates {
                    after = Some(*id);
                    let target = if modern {
                        let target = targets
                            .remove(&ObjectRef {
                                kind: ObjectKind::Environment,
                                id: *id,
                            })
                            .ok_or_else(ApiError::not_found)?;
                        if !native::permits(
                            &plan,
                            Self::DECLARATION,
                            caller,
                            &Source::Environments,
                            ENVIRONMENT,
                            &target,
                        )? {
                            continue;
                        }
                        Some(target)
                    } else {
                        None
                    };
                    let principal = if let Some(target) = &target {
                        let known = native::bind_known(
                            ctx,
                            Self::DECLARATION,
                            &[collection(
                                Source::Environments,
                                ENVIRONMENT,
                                vec![target.clone()],
                            )],
                        )?;
                        native::preparation_principal(ctx, conn, caller, &known).await?
                    } else {
                        raw.clone()
                    };
                    match may_read(conn, &principal, *id).await {
                        Ok(_) => {}
                        Err(error)
                            if matches!(
                                error.code(),
                                ErrorCode::AuthzDenied | ErrorCode::NotFound | ErrorCode::Gone
                            ) =>
                        {
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                    selected.push(prepare_read(ctx, conn, &principal, *id).await?);
                    if let Some(target) = target {
                        selected_targets.push(target);
                    }
                    if selected.len() > limit {
                        break;
                    }
                }
                if selected.len() > limit
                    || candidates.len()
                        < usize::try_from(orch_api_contract::common::MAX_LIMIT)
                            .map_err(ApiError::internal)?
                {
                    break;
                }
            }
            let more = selected.len() > limit;
            // The native look-ahead is validated but supplies no source to the returned window.
            if more && !modern {
                selected
                    .last()
                    .ok_or_else(ApiError::not_found)?
                    .render(ctx, conn, &raw)
                    .await?;
            }
            selected.truncate(limit);
            selected_targets.truncate(limit);
            let next_cursor = more
                .then(|| selected.last().map(|row| row.id.to_string()))
                .flatten();
            if !modern {
                return Ok((
                    RetainedPage {
                        rows: selected,
                        next_cursor,
                    },
                    Vec::new(),
                ));
            }

            let mut facts = vec![collection(
                Source::Environments,
                ENVIRONMENT,
                selected_targets,
            )];
            let hosts: BTreeSet<_> = selected
                .iter()
                .flat_map(|row| row.projection.host_ids().iter().copied())
                .collect();
            let mut host_targets = Sources::from_objects(hosts.iter().map(|id| ObjectRef {
                kind: ObjectKind::Host,
                id: *id,
            }))
            .identity_targets(conn)
            .await?;
            facts.push(collection(
                Source::Hosts,
                HOST,
                hosts
                    .iter()
                    .map(|id| {
                        host_targets
                            .remove(&ObjectRef {
                                kind: ObjectKind::Host,
                                id: *id,
                            })
                            .ok_or_else(ApiError::not_found)
                    })
                    .collect::<ApiResult<Vec<_>>>()?,
            ));
            let mut partial = native::bind_known(ctx, Self::DECLARATION, &facts)?;
            let names = NameSources {
                nodes: selected.iter().map(|row| row.home).collect(),
                ..NameSources::default()
            };
            let bounds = projection_names(conn, caller, &mut partial, names, &NAMES).await?;
            let graph = Graph::load(conn, &bounds.nodes.iter().copied().collect::<Vec<_>>())
                .await
                .map_err(ApiError::internal)?;
            for (source, kind) in [
                (Source::HomeOrgs, ObjectKind::Org),
                (Source::HomeProjects, ObjectKind::Project),
                (Source::HomeTopics, ObjectKind::Topic),
                (Source::HomeThreads, ObjectKind::Thread),
            ] {
                let mut named = Vec::new();
                for id in &bounds.nodes {
                    let target =
                        orch_engine::operation_scope::targets::from_node_identity(&graph, *id)
                            .ok_or_else(ApiError::not_found)?;
                    if target.object.kind == KeyTargetKind::Object(kind) {
                        named.push(target);
                    }
                }
                facts.push(collection(source, KeyTargetKind::Object(kind), named));
            }
            for row in &mut selected {
                row.projection.bound_names(bounds.clone());
            }
            Ok((
                RetainedPage {
                    rows: selected,
                    next_cursor,
                },
                facts,
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
                return Err(ApiError::internal(
                    "the retained Environment reader changed account",
                ));
            }
            Ok(())
        })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a RetainedPage,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            let mut items = Vec::new();
            for row in &value.rows {
                let shown = row.render(ctx, conn, principal).await?;
                if let Some(removal) = shown.removal {
                    items.push(api::DeletedEnvironment {
                        environment: shown.base,
                        home: shown.home,
                        host: shown.host,
                        storage: shown.workspace_storage,
                        removal,
                    });
                }
            }
            Ok(Page {
                items,
                next_cursor: value.next_cursor.clone(),
            })
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a RetainedPage,
        fresh: &'a RetainedPage,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let current: HashMap<_, _> = fresh.rows.iter().map(|row| (row.id, row)).collect();
            Ok(opened.rows.iter().all(|old| {
                current.get(&old.id).is_some_and(|now| {
                    old.home == now.home
                        && old.deleted_at == now.deleted_at
                        && (!old.authority.0 || now.authority.0)
                        && (!old.authority.1 || now.authority.1)
                        && old
                            .projection
                            .host_ids()
                            .iter()
                            .all(|host| now.projection.host_ids().contains(host))
                })
            }))
        })
    }
}
