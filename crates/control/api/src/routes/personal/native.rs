//! Native personal row selection, before any personal response DTO is rendered.

use super::*;
use crate::key_operations::{native, targets};
use crate::registry::OperationPlan;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyResolvedTarget, KeyTargetKind};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::operations::FacetUsage;
use orch_api_contract::personal_sources::{self as sources, Source};
use orch_authz::Caller;
use orch_db::Conn;
use std::collections::BTreeSet;

const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);

async fn account(ctx: &Ctx, conn: &mut Conn) -> ApiResult<native::Facts<Source>> {
    Ok(native::Facts::one(
        Source::Account,
        ACCOUNT,
        targets::account(conn, ctx.user()?.id()).await?,
    ))
}

fn group(groups: &mut Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)>, target: KeyResolvedTarget) {
    let kind = target.object.kind;
    if let Some((_, targets)) = groups.iter_mut().find(|(family, _)| *family == kind) {
        targets.push(target);
    } else {
        groups.push((kind, vec![target]));
    }
}

fn require_owner<'a>(ctx: &'a Ctx, principal: &'a Principal) -> BoxFuture<'a, ApiResult<()>> {
    Box::pin(async move {
        if ctx.user()?.id() != principal.user() {
            return Err(ApiError::internal(
                "the personal state reader changed account",
            ));
        }
        Ok(())
    })
}

pub(super) struct ReadStates;

impl native::Selected for ReadStates {
    type Source = Source;
    type Query = MeReadStateListQuery;
    type Value = Vec<(Id, Id, i64)>;
    type Reply = Vec<ReadState>;
    const DECLARATION: &'static Declaration<Source> = &sources::READ_STATES;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let rows = hierarchy::read_states_of(conn, ctx.user()?.id(), query.org_id).await?;
            if !native::modern(caller) {
                return Ok((rows, Vec::new()));
            }
            let own = account(ctx, conn).await?;
            let ids = rows
                .iter()
                .map(|(thread, _, _)| *thread)
                .collect::<Vec<_>>();
            let graph = orch_authz::engine::Graph::load(conn, &ids)
                .await
                .map_err(authorization::restoration_error)?;
            let plan = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            let mut selected = Vec::new();
            let mut groups = Vec::new();
            for row in rows {
                let target =
                    orch_engine::operation_scope::targets::from_node_identity(&graph, row.0)
                        .ok_or_else(ApiError::not_found)?;
                if !native::modern(caller)
                    || native::permits(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &Source::ReadThreads,
                        target.object.kind,
                        &target,
                    )?
                {
                    group(&mut groups, target);
                    selected.push(row);
                }
            }
            Ok((
                selected,
                vec![own, native::Facts::collection(Source::ReadThreads, groups)],
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        require_owner(ctx, principal)
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            Ok(value
                .iter()
                .map(|(thread_id, continuation_id, last_read_seq)| ReadState {
                    thread_id: *thread_id,
                    continuation_id: Some(*continuation_id),
                    last_read_seq: *last_read_seq,
                })
                .collect())
        })
    }
}

pub(super) struct Subscriptions;

impl native::Selected for Subscriptions {
    type Source = Source;
    type Query = MeSubscriptionListQuery;
    type Value = Vec<hierarchy::SubscriptionRow>;
    type Reply = Vec<MeSubscription>;
    const DECLARATION: &'static Declaration<Source> = &sources::SUBSCRIPTIONS;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let rows = hierarchy::subscriptions_of(conn, ctx.user()?.id(), query.org_id).await?;
            if !native::modern(caller) {
                return Ok((rows, Vec::new()));
            }
            let own = account(ctx, conn).await?;
            let ids = rows.iter().map(|row| row.node_id).collect::<Vec<_>>();
            let graph = orch_authz::engine::Graph::load(conn, &ids)
                .await
                .map_err(authorization::restoration_error)?;
            let plan = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            let mut selected = Vec::new();
            let mut groups = Vec::new();
            for row in rows {
                if super::subscription(&row).is_none() {
                    continue;
                }
                let target =
                    orch_engine::operation_scope::targets::from_node_identity(&graph, row.node_id)
                        .ok_or_else(ApiError::not_found)?;
                if !native::modern(caller)
                    || native::permits(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &Source::Subscriptions,
                        target.object.kind,
                        &target,
                    )?
                {
                    group(&mut groups, target);
                    selected.push(row);
                }
            }
            Ok((
                selected,
                vec![
                    own,
                    native::Facts::collection(Source::Subscriptions, groups),
                ],
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        require_owner(ctx, principal)
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move { Ok(value.iter().filter_map(super::subscription).collect()) })
    }
}

pub(super) struct Favorites;

pub(super) struct FavoriteRows {
    rows: Vec<hierarchy::FavoriteRow>,
    names: BTreeSet<Id>,
    limited: bool,
}

const FAVORITE_NAMES: &[(orch_access_contract::permissions::ObjectKind, Source)] = &[
    (
        orch_access_contract::permissions::ObjectKind::Org,
        Source::FavoriteOrgNames,
    ),
    (
        orch_access_contract::permissions::ObjectKind::Project,
        Source::FavoriteProjectNames,
    ),
    (
        orch_access_contract::permissions::ObjectKind::Topic,
        Source::FavoriteTopicNames,
    ),
    (
        orch_access_contract::permissions::ObjectKind::Thread,
        Source::FavoriteThreadNames,
    ),
];

impl native::Selected for Favorites {
    type Source = Source;
    type Query = ();
    type Value = FavoriteRows;
    type Reply = Vec<Favorite>;
    const DECLARATION: &'static Declaration<Source> = &sources::FAVORITES;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(FavoriteRows, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let rows = hierarchy::favorites_of(conn, ctx.user()?.id()).await?;
            if !native::modern(caller) {
                return Ok((
                    FavoriteRows {
                        rows,
                        names: BTreeSet::new(),
                        limited: false,
                    },
                    Vec::new(),
                ));
            }
            let own = account(ctx, conn).await?;
            let ids = rows.iter().map(|row| row.node_id).collect::<Vec<_>>();
            let graph = orch_authz::engine::Graph::load(conn, &ids)
                .await
                .map_err(authorization::restoration_error)?;
            let plan = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            let limited = native::modern(caller);
            let mut candidates = Vec::new();
            let mut groups = Vec::new();
            for row in rows {
                let Some(target) =
                    orch_engine::operation_scope::targets::from_node_identity(&graph, row.node_id)
                else {
                    continue;
                };
                if !limited
                    || native::permits(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &Source::Favorites,
                        target.object.kind,
                        &target,
                    )?
                {
                    candidates.push(row);
                    group(&mut groups, target);
                }
            }
            let mut known_facts = vec![own, native::Facts::collection(Source::Favorites, groups)];
            let known = native::bind_known(ctx, Self::DECLARATION, &known_facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let ids = candidates.iter().map(|row| row.node_id).collect::<Vec<_>>();
            // Preserve the existing ordinary native visibility supplier. Its DTO is
            // used only as a visibility result; identities remain the stored rows.
            let visible = view::visible(
                conn,
                &principal,
                &ids,
                principal.now().get(),
                Policy::default(),
            )
            .await?
            .iter()
            .map(crate::routes::nodes::node_id)
            .collect::<BTreeSet<_>>();
            candidates.retain(|row| visible.contains(&row.node_id));
            let mut actual = Vec::new();
            let mut name_ids = BTreeSet::new();
            for row in &candidates {
                let target =
                    orch_engine::operation_scope::targets::from_node_identity(&graph, row.node_id)
                        .ok_or_else(ApiError::not_found)?;
                group(&mut actual, target);
                if let Some(node) = graph.get(row.node_id) {
                    name_ids.extend(node.path.iter().copied());
                }
            }
            known_facts[1] = native::Facts::collection(Source::Favorites, actual);
            let mut names = BTreeSet::new();
            for &(kind, source) in FAVORITE_NAMES {
                let mut selected = Vec::new();
                for id in &name_ids {
                    let Some(target) =
                        orch_engine::operation_scope::targets::from_node_identity(&graph, *id)
                    else {
                        continue;
                    };
                    let family = KeyTargetKind::Object(kind);
                    if target.object.kind != family {
                        continue;
                    }
                    if !limited
                        || native::permits_usage(
                            &plan,
                            Self::DECLARATION,
                            caller,
                            &source,
                            FacetUsage::ResponseContent,
                            family,
                            &target,
                        )?
                    {
                        names.insert(*id);
                        selected.push(target);
                    }
                }
                let groups = if selected.is_empty() {
                    Vec::new()
                } else {
                    vec![(KeyTargetKind::Object(kind), selected)]
                };
                known_facts.push(native::Facts::collection(source, groups));
            }
            Ok((
                FavoriteRows {
                    rows: candidates,
                    names,
                    limited,
                },
                known_facts,
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        require_owner(ctx, principal)
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a FavoriteRows,
    ) -> BoxFuture<'a, ApiResult<Vec<Favorite>>> {
        Box::pin(async move {
            let ids = value.rows.iter().map(|row| row.node_id).collect::<Vec<_>>();
            let visible = view::visible(
                conn,
                principal,
                &ids,
                principal.now().get(),
                Policy::default(),
            )
            .await?
            .iter()
            .map(crate::routes::nodes::node_id)
            .collect::<BTreeSet<_>>();
            let ways = if value.limited {
                view::ways_with_name_sources(conn, principal, &ids, Policy::default(), &value.names)
                    .await?
            } else {
                view::ways(conn, principal, &ids, Policy::default()).await?
            };
            Ok(value
                .rows
                .iter()
                .filter(|row| visible.contains(&row.node_id))
                .filter_map(|row| {
                    let mut path = ways.get(&row.node_id)?.clone();
                    Some(Favorite {
                        node: path.pop()?,
                        path,
                        position: row.position.clone(),
                        view: row
                            .view
                            .clone()
                            .and_then(|view| serde_json::from_value(view).ok()),
                    })
                })
                .collect())
        })
    }
}
