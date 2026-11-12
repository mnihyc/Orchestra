//! Native cap selectors and projectors shared by the operation-row adapter.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::registry::OperationPlan;

use crate::routes::usage::native as sources_native;

use futures::future::BoxFuture;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::usage::UsageCapWaiting;

use orch_api_contract::usage_sources as sources;

use orch_api_contract::usage_sources::Source;

use orch_authz::Caller;
pub(crate) enum StatusIntent {
    Defined(UsageCapListQuery),
    Limits(UsageLimitsGetQuery),
}

pub(crate) struct PreparedStatuses {
    intent: StatusIntent,
    computed: Vec<ComputedStatus>,
    names: NameSources,
    providers: Vec<Id>,
    bounds: NameBounds,
    at: DateTime<Utc>,
}

async fn select_statuses(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    intent: StatusIntent,
    declaration: &'static Declaration<Source>,
) -> ApiResult<(PreparedStatuses, Vec<native::Facts<Source>>)> {
    let (root, source) = match &intent {
        StatusIntent::Defined(query) => (
            sources_native::home(conn, place_of(query)?.0).await?,
            Source::CapHome,
        ),
        StatusIntent::Limits(query) => (targets::node(conn, query.node_id).await?, Source::Node),
    };
    let mut facts = vec![native::Facts::one(source, root.object.kind, root)];
    let plan = native::bind_known(ctx, declaration, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    let mut shown = match &intent {
        StatusIntent::Defined(query) => {
            let (home, defined) = place_of(query)?;
            require(conn, &principal, home, Need::View).await?;
            let mut caps = rows::defined_on(conn, defined).await?;
            if query.covering == Some(true) {
                let DefinedOn::Node(node) = defined else {
                    return Err(invalid(
                        "covering",
                        "needs node_id: it asks what covers work at a node",
                    ));
                };
                let here: HashSet<_> = caps.iter().map(|cap| cap.id).collect();
                caps.extend(
                    on_the_way(conn, node)
                        .await?
                        .into_iter()
                        .filter(|cap| !here.contains(&cap.id)),
                );
            }
            let caps = select_caps(ctx, conn, caller, declaration, &plan, caps).await?;
            let mut shown = Vec::new();
            for cap in caps {
                shown.extend(rows_of(conn, &cap, ctx.now).await?)
            }
            shown
        }
        StatusIntent::Limits(query) => {
            let person = require_limits(conn, &principal, query).await?;
            let caps = on_the_way(conn, query.node_id).await?;
            let mut covering = Vec::new();
            for cap in select_caps(ctx, conn, caller, declaration, &plan, caps).await? {
                if engine::covers_person(conn, &cap, person, Some(query.node_id), ctx.now)
                    .await
                    .map_err(ApiError::internal)?
                {
                    covering.push(cap)
                }
            }
            share_of(covering, person)
        }
    };
    // Selection precedes use/status aggregation and every full subject/name projection.
    let mut homes = Vec::new();
    for each in &shown {
        homes.push(sources_native::home(conn, engine::home_of(&each.cap)).await?);
    }
    facts.push(sources_native::collection(Source::Caps, homes));
    let computed = compute_statuses(ctx, conn, &principal, std::mem::take(&mut shown)).await?;
    let (names, providers) = status_sources(&computed);
    let groups = crate::routes::access::operations::group_targets(
        conn,
        &names.groups.iter().copied().collect::<Vec<_>>(),
    )
    .await?;
    facts.push(sources_native::collection(Source::Groups, groups));
    let (bounds, name_facts) =
        sources_native::names(ctx, conn, caller, declaration, names.clone()).await?;
    facts.push(name_facts);
    Ok((
        PreparedStatuses {
            intent,
            computed,
            names,
            providers,
            bounds,
            at: ctx.now,
        },
        facts,
    ))
}

async fn select_caps(
    _ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
    plan: &OperationPlan,
    caps: Vec<CapRow>,
) -> ApiResult<Vec<CapRow>> {
    if !native::modern(caller) {
        return Ok(caps);
    }
    let mut selected = Vec::new();
    for cap in caps {
        let target = sources_native::home(conn, engine::home_of(&cap)).await?;
        if !native::permits(
            plan,
            declaration,
            caller,
            &Source::Caps,
            target.object.kind,
            &target,
        )? {
            continue;
        }
        if cap.subject_kind == subject::GROUP
            && let Some(group) = cap.subject_id
        {
            let targets = crate::routes::access::operations::group_targets(conn, &[group]).await?;
            let mut allowed = true;
            for target in &targets {
                allowed &= native::permits(
                    plan,
                    declaration,
                    caller,
                    &Source::Groups,
                    target.object.kind,
                    target,
                )?;
            }
            if !allowed {
                continue;
            }
        }
        selected.push(cap);
    }
    Ok(selected)
}

async fn require_limits(
    conn: &mut Conn,
    principal: &Principal,
    query: &UsageLimitsGetQuery,
) -> ApiResult<Id> {
    authorization::require(
        conn,
        principal,
        query.node_id,
        authorization::VIEW,
        Act::Use,
        Policy::default(),
    )
    .await?;
    match query.user_id {
        Some(other) if other != principal.user() => {
            authorization::require(
                conn,
                principal,
                query.node_id,
                exact(Permission::UsageView),
                Act::Use,
                Policy::default(),
            )
            .await?;
            Ok(other)
        }
        _ => Ok(principal.user()),
    }
}

async fn render_statuses(
    _ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &PreparedStatuses,
) -> ApiResult<Vec<CapStatus>> {
    require_statuses(conn, principal, value).await?;
    project_statuses(
        value.at,
        conn,
        principal,
        &value.computed,
        &value.names,
        &value.providers,
        Some(&value.bounds),
    )
    .await
}

async fn require_statuses(
    conn: &mut Conn,
    principal: &Principal,
    value: &PreparedStatuses,
) -> ApiResult<()> {
    match &value.intent {
        StatusIntent::Defined(query) => {
            require(conn, principal, place_of(query)?.0, Need::View).await?
        }
        StatusIntent::Limits(query) => {
            require_limits(conn, principal, query).await?;
        }
    }
    Ok(())
}

pub(crate) struct List;

impl native::Selected for List {
    type Source = Source;
    type Query = UsageCapListQuery;
    type Value = PreparedStatuses;
    type Reply = Vec<CapStatus>;
    const DECLARATION: &'static Declaration<Source> = &sources::CAP_LIST;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(select_statuses(
            ctx,
            conn,
            caller,
            StatusIntent::Defined(query.clone()),
            Self::DECLARATION,
        ))
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(render_statuses(_ctx, conn, principal, value))
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            require_statuses(conn, principal, fresh).await?;
            Ok(preserves_statuses(opened, fresh))
        })
    }
}

fn preserves_statuses(opened: &PreparedStatuses, fresh: &PreparedStatuses) -> bool {
    let identities = fresh
        .computed
        .iter()
        .map(|status| status.0.id)
        .collect::<HashSet<_>>();
    opened
        .computed
        .iter()
        .all(|status| identities.contains(&status.0.id))
}

pub(crate) struct LimitsReader;

impl native::Selected for LimitsReader {
    type Source = Source;
    type Query = UsageLimitsGetQuery;
    type Value = PreparedStatuses;
    type Reply = Limits;
    const DECLARATION: &'static Declaration<Source> = &sources::LIMITS;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(select_statuses(
            ctx,
            conn,
            caller,
            StatusIntent::Limits(query.clone()),
            Self::DECLARATION,
        ))
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            Ok(Limits {
                caps: render_statuses(_ctx, conn, principal, value).await?,
            })
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            require_statuses(conn, principal, fresh).await?;
            Ok(preserves_statuses(opened, fresh))
        })
    }
}

pub(crate) struct PreparedQueue {
    cap: CapRow,
    queue: Vec<rows::Waiter>,
    reader: bool,
    bounds: NameBounds,
}

pub(crate) struct Waiting;

impl native::Selected for Waiting {
    type Source = Source;
    type Query = ();
    type Value = PreparedQueue;
    type Reply = UsageCapWaitingListReply;
    const DECLARATION: &'static Declaration<Source> = &sources::WAITING;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let cap = rows::get(conn, ctx.id("id")?)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let target = sources_native::home(conn, engine::home_of(&cap)).await?;
            let mut facts = vec![native::Facts::one(
                Source::CapHome,
                target.object.kind,
                target,
            )];
            let plan = native::bind_known(ctx, Self::DECLARATION, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let reader = queue_reader(ctx, conn, &principal, &cap, ctx.now).await?;
            let queue = engine::waiting_for(conn, &cap, ctx.now)
                .await
                .map_err(ApiError::internal)?;
            let names = NameSources {
                nodes: queue
                    .iter()
                    .filter(|waiter| reader || waiter.author_id == Some(principal.user()))
                    .map(|waiter| waiter.thread_id)
                    .collect(),
                users: queue.iter().filter_map(|waiter| waiter.author_id).collect(),
                ..NameSources::default()
            };
            let (bounds, names) =
                sources_native::names(ctx, conn, caller, Self::DECLARATION, names).await?;
            facts.push(names);
            Ok((
                PreparedQueue {
                    cap,
                    queue,
                    reader,
                    bounds,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            queue_reader(ctx, conn, principal, &value.cap, ctx.now).await?;
            let threads: Vec<_> = value.queue.iter().map(|waiter| waiter.thread_id).collect();
            let people: Vec<_> = value
                .queue
                .iter()
                .filter_map(|waiter| waiter.author_id)
                .collect();
            let names =
                Names::load_bounded(conn, principal, &people, &[], &threads, Some(&value.bounds))
                    .await?;
            Ok(UsageCapWaitingListReply {
                items: value
                    .queue
                    .iter()
                    .map(|waiter| {
                        let mine = waiter.author_id == Some(principal.user());
                        let named = value.reader || mine;
                        let thread = names.node(waiter.thread_id, NodeKind::Thread);
                        UsageCapWaiting {
                            thread: (named && !thread.redacted).then_some(thread),
                            author: waiter
                                .author_id
                                .filter(|_| named)
                                .map(|author| names.user(author)),
                            since: Time::new(waiter.since),
                            mine,
                        }
                    })
                    .collect(),
            })
        })
    }
}

async fn queue_reader(
    _ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    cap: &CapRow,
    at: DateTime<Utc>,
) -> ApiResult<bool> {
    let reader = may(conn, principal, engine::home_of(cap), Need::View).await?;
    if !reader
        && !engine::covers_person(conn, cap, principal.user(), None, at)
            .await
            .map_err(ApiError::internal)?
    {
        require(conn, principal, engine::home_of(cap), Need::View).await?;
    }
    Ok(reader)
}
