//! Saved choice reads never enter the resolver, reserve policy or start native work.
use super::*;
use orch_db::queries::pool_choices as saved;
use orch_engine::resource_choice::Hop;
use orch_provider_contract::pools::{Kind, Leaf};
use std::collections::BTreeMap;

type Disclosure = BTreeMap<KeyTargetKind, Vec<orch_access_contract::key_scope::KeyResolvedTarget>>;

pub(super) fn mount(registry: &mut Registry) {
    registry.on_snapshot_read_with_optional_operation(
        api::PoolChoicesGet,
        native::selected::<History>,
        native::selected_read::<History>,
    );
    registry.on_snapshot_read_with_optional_operation(
        api::PoolChoiceDetailsGet,
        native::selected::<Details>,
        native::selected_read::<Details>,
    );
}

struct Read {
    pool: Id,
    principal: Principal,
    plan: crate::registry::OperationPlan,
    sources: Vec<native::Facts<api::Source>>,
    kind: Kind,
    batch: usize,
    limit: i64,
}
async fn begin(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    declaration: &'static Declaration<api::Source>,
) -> ApiResult<Read> {
    let pool = ctx.id("id")?;
    let sources = facts(conn, pool).await?;
    let plan = native::bind_known(ctx, declaration, &sources)?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    require(
        conn,
        &principal,
        ResourceKind::Pool,
        pool,
        Permission::PoolView,
    )
    .await?;
    let row = rows::get(conn, pool)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let definition: Definition =
        serde_json::from_value(row.definition).map_err(ApiError::internal)?;
    let batch = orch_settings::get::<i64>(
        conn,
        orch_settings::Place::Pool(pool),
        orch_settings_contract::registry::keys::POOLS_READ_BATCH_SIZE,
    )
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(|| ApiError::internal("the pool read batch has no registered default"))?;
    let limit = batch.checked_add(1).ok_or_else(|| {
        invalid(
            "pools.read_batch_size",
            "the batch exceeds its stored integer representation",
        )
    })?;
    Ok(Read {
        pool,
        principal,
        plan,
        sources,
        kind: definition.kind,
        batch: usize::try_from(batch).map_err(ApiError::internal)?,
        limit,
    })
}
async fn visible(
    conn: &mut Conn,
    caller: &Caller,
    read: &Read,
    leaf: Option<&Leaf>,
    path: &Json,
    selected: &mut Disclosure,
) -> ApiResult<bool> {
    let path: Vec<Hop> = serde_json::from_value(path.clone()).map_err(ApiError::internal)?;
    for hop in path {
        if !scoped_rule(
            conn,
            caller,
            &read.plan,
            &read.principal,
            &MemberRule::Pool {
                id: hop.pool_id,
                kind: read.kind,
            },
            selected,
        )
        .await?
        {
            return Ok(false);
        }
    }
    match leaf {
        Some(leaf) => {
            scoped_rule(
                conn,
                caller,
                &read.plan,
                &read.principal,
                &MemberRule::Single {
                    resource: leaf.clone(),
                },
                selected,
            )
            .await
        }
        None => Ok(true),
    }
}
async fn label(conn: &mut Conn, leaf: Option<&Leaf>) -> ApiResult<Option<String>> {
    match leaf {
        Some(Leaf::Model { backend }) => Ok(backend
            .offering
            .as_ref()
            .map(|offering| offering.offering.clone())),
        Some(leaf) => {
            rule_label(
                conn,
                &MemberRule::Single {
                    resource: leaf.clone(),
                },
            )
            .await
        }
        None => Ok(None),
    }
}
fn complete(mut read: Read, selected: Disclosure) -> Vec<native::Facts<api::Source>> {
    read.sources.push(native::Facts::collection(
        api::Source::Members,
        selected.into_iter().collect(),
    ));
    read.sources
}
fn require_read<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    principal: &'a Principal,
) -> BoxFuture<'a, ApiResult<()>> {
    Box::pin(async move {
        require(
            conn,
            principal,
            ResourceKind::Pool,
            ctx.id("id")?,
            Permission::PoolView,
        )
        .await
    })
}

struct History;
impl native::Selected for History {
    type Source = api::Source;
    type Query = api::PoolChoicesQuery;
    type Value = api::PoolChoicesReply;
    type Reply = api::PoolChoicesReply;
    const DECLARATION: &'static Declaration<api::Source> = &api::CHOICES;
    const WATCH_ORDINARY: bool = true;
    const MOVES: native::Moves = native::Moves::Repeat;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> native::SourceFuture<'a, Self::Value, api::Source> {
        Box::pin(async move {
            let read = begin(ctx, conn, caller, Self::DECLARATION).await?;
            let before = match (query.before_created_at, query.before_id) {
                (None, None) => None,
                (Some(at), Some(id)) => Some((at.get(), id)),
                _ => {
                    return Err(invalid(
                        "before_id",
                        "both choice cursor fields are required together",
                    ));
                }
            };
            let mut rows = saved::history(conn, read.pool, before, read.limit).await?;
            let more = rows.len() > read.batch;
            rows.truncate(read.batch);
            let next = more
                .then(|| {
                    rows.last().map(|row| api::PoolChoicesQuery {
                        before_created_at: Some(Time::new(row.created_at)),
                        before_id: Some(row.id),
                    })
                })
                .flatten();
            let mut items = Vec::with_capacity(rows.len());
            let mut selected = Disclosure::new();
            for row in rows {
                let leaf: Option<Leaf> = row
                    .leaf
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(ApiError::internal)?;
                let readable = visible(
                    conn,
                    caller,
                    &read,
                    leaf.as_ref(),
                    &row.path.unwrap_or_else(|| serde_json::json!([])),
                    &mut selected,
                )
                .await?;
                let name = if readable {
                    label(conn, leaf.as_ref()).await?
                } else {
                    None
                };
                items.push(api::PoolChoiceView {
                    id: row.id,
                    created_at: Time::new(row.created_at),
                    state: serde_json::from_value(Json::String(row.state))
                        .map_err(ApiError::internal)?,
                    resource: if readable { leaf } else { None },
                    label: name,
                    policy: row
                        .policy
                        .map(serde_json::from_value::<Option<_>>)
                        .transpose()
                        .map_err(ApiError::internal)?
                        .flatten(),
                    reason: if readable {
                        row.reason
                    } else {
                        Some("member_hidden".into())
                    },
                    details_expired_at: row.details_expired_at.map(Time::new),
                });
            }
            Ok((
                api::PoolChoicesReply { items, next },
                complete(read, selected),
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        require_read(ctx, conn, principal)
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move { Ok(value.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Query,
        opened: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened == fresh) })
    }
}

struct Details;
impl native::Selected for Details {
    type Source = api::Source;
    type Query = api::PoolChoiceDetailsQuery;
    type Value = api::PoolChoiceDetailsReply;
    type Reply = api::PoolChoiceDetailsReply;
    const DECLARATION: &'static Declaration<api::Source> = &api::CHOICE_DETAILS;
    const WATCH_ORDINARY: bool = true;
    const MOVES: native::Moves = native::Moves::Repeat;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> native::SourceFuture<'a, Self::Value, api::Source> {
        Box::pin(async move {
            let read = begin(ctx, conn, caller, Self::DECLARATION).await?;
            let id = ctx.id("choice_id")?;
            let row = saved::recorded(conn, read.pool, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let mut rows = if row.details_expired_at.is_none() {
                saved::details(conn, row.choice_id, query.after_ordinal, read.limit).await?
            } else {
                vec![]
            };
            let more = rows.len() > read.batch;
            rows.truncate(read.batch);
            let next = more.then(|| rows.last().map(|row| row.ordinal)).flatten();
            let mut items = Vec::with_capacity(rows.len());
            let mut selected = Disclosure::new();
            for row in rows {
                let leaf: Option<Leaf> = row
                    .leaf
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(ApiError::internal)?;
                let readable =
                    visible(conn, caller, &read, leaf.as_ref(), &row.path, &mut selected).await?;
                let name = if readable {
                    label(conn, leaf.as_ref()).await?
                } else {
                    None
                };
                items.push(api::PoolChoiceDetail {
                    ordinal: row.ordinal,
                    resource: if readable { leaf } else { None },
                    label: name,
                    reason: if readable {
                        row.reason
                    } else {
                        Some("member_hidden".into())
                    },
                    cost: if readable { row.cost } else { None },
                });
            }
            Ok((
                api::PoolChoiceDetailsReply {
                    id,
                    expired_at: row.details_expired_at.map(Time::new),
                    items,
                    next,
                },
                complete(read, selected),
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        require_read(ctx, conn, principal)
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move { Ok(value.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Query,
        opened: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened == fresh) })
    }
}
