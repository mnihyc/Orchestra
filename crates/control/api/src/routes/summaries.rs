//! Resource paragraphs through native Read, paid-refresh admission and metadata-only receipts.
use crate::{
    key_operations::{
        native::{self, effects},
        publication,
    },
    middleware::{
        requests::{ApiError, ApiResult},
        transaction_guard,
    },
    registry::{Ctx, Registry},
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyIdentityKind, KeyResolvedTarget, KeyTargetKind},
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    common::{ErrorCode, Problem, ReceiptProjection},
    operation_sources::Declaration,
    routes::Endpoint,
    summaries::*,
    summary_sources::{self as declarations, ResourceSource as Role},
};
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::{Conn, DbError, queries::summaries as rows};
use orch_engine::{
    operation_scope::Prepared,
    summaries::{options, resource, sources},
};
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::BTreeMap;
mod batch;
mod cache_read;
mod follow;

/// Binds the native resource paragraph read and paid refresh acts.
pub fn mount(registry: &mut Registry) {
    registry.on_snapshot_read_with_optional_operation(
        SummaryGet,
        native::selected::<Reader>,
        native::selected_read::<Reader>,
    );
    registry.on_snapshot_body_read_with_optional_operation(
        SummaryBatchGet,
        native::selected_body::<batch::Reader, (), SummaryBatchBody>,
        native::selected_body_read::<batch::Reader, (), SummaryBatchBody>,
    );
    registry.on_with_operation(SummaryRefresh, effects::prepare::<Refresher>, refresh);
}
fn source_error(_: impl std::fmt::Display) -> ApiError {
    ApiError::not_found()
}
fn target(ctx: &Ctx) -> ApiResult<SummaryTarget> {
    let id = ctx.id("id")?;
    match ctx.param("kind")? {
        "thread" => Ok(SummaryTarget::Thread { id }),
        "topic" => Ok(SummaryTarget::Topic { id }),
        "document" => Ok(SummaryTarget::Document { id }),
        "chat" => Ok(SummaryTarget::Chat { id }),
        "chat_title" => Ok(SummaryTarget::ChatTitle { id }),
        _ => Err(ApiError::not_found()),
    }
}
fn range(query: &SummaryReadQuery, target: SummaryTarget) -> ApiResult<Option<SummaryChatRange>> {
    match (target, query.after, query.through) {
        (SummaryTarget::Chat { .. }, Some(after), Some(through))
            if after >= 0 && through >= after =>
        {
            Ok(Some(SummaryChatRange { after, through }))
        }
        (_, None, None) => Ok(None),
        _ => Err(ApiError::invalid(
            "range",
            "Use an ordered chat range with both after and through",
        )),
    }
}
async fn require_source(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
) -> ApiResult<()> {
    if !sources::readable(conn, principal, target)
        .await
        .map_err(source_error)?
    {
        return Err(ApiError::not_found());
    }
    Ok(())
}

async fn require_disclosure(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
) -> ApiResult<()> {
    if !sources::viewable(conn, principal, target)
        .await
        .map_err(source_error)?
    {
        return Err(ApiError::not_found());
    }
    Ok(())
}

/// The same native opening audit for stored paragraphs and protected model-input text.
/// Selection/publication retain all canonical Read facts; this once-only hook creates no
/// chat membership, cursor, draft, notification or model request.
pub(crate) async fn note_chat_openings(
    conn: &mut Conn,
    principal: &Principal,
    inputs: &[KeyResolvedTarget],
) -> ApiResult<()> {
    let origin = orch_db::audit::current_origin();
    let act = orch_chat::Acting {
        principal,
        now: principal.now().get(),
        origin: &origin,
    };
    let mut seen = std::collections::BTreeSet::new();
    for fact in inputs {
        if fact.object.kind != KeyTargetKind::Identity(KeyIdentityKind::Chat) {
            continue;
        }
        let id = fact.object.id.ok_or_else(ApiError::not_found)?;
        if seen.insert(id) {
            let loaded = orch_chat::chats::load(conn, &act, id, orch_chat::chats::Mode::Read)
                .await
                .map_err(source_error)?;
            orch_chat::inspection::note_opened(conn, &act, &loaded)
                .await
                .map_err(source_error)?;
        }
    }
    Ok(())
}
fn read_facts(
    identity: &sources::Identity,
    inputs: &[KeyResolvedTarget],
    provider: &[KeyResolvedTarget],
) -> Vec<native::Facts<Role>> {
    let mut groups = BTreeMap::new();
    for input in inputs {
        groups
            .entry(input.object.kind)
            .or_insert_with(Vec::new)
            .push(input.clone());
    }
    let mut providers = BTreeMap::new();
    for input in provider {
        providers
            .entry(input.object.kind)
            .or_insert_with(Vec::new)
            .push(input.clone());
    }
    vec![
        native::Facts::one(
            Role::Target,
            identity.fact().object.kind,
            identity.fact().clone(),
        ),
        native::Facts::collection(Role::Inputs, groups.into_iter().collect()),
        native::Facts::collection(Role::Provider, providers.into_iter().collect()),
    ]
}

async fn readable_provider_fact(
    conn: &mut Conn,
    principal: &Principal,
    plan: &Prepared,
    id: Id,
) -> ApiResult<Option<KeyResolvedTarget>> {
    let Ok(fact) = orch_engine::operation_scope::targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id,
        },
    )
    .await
    else {
        return Ok(None);
    };
    if !plan
        .permits_binding(
            principal.caller(),
            2,
            fact.object.kind,
            std::slice::from_ref(&fact),
        )
        .map_err(source_error)?
        || !orch_authz::resources::check(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Provider,
                id,
            },
            Permission::ProviderView,
            orch_authz::Policy::default(),
        )
        .await
        .map_err(source_error)?
        .is_allowed()
    {
        return Ok(None);
    }
    Ok(Some(fact))
}

struct View {
    identity: sources::Identity,
    inputs: Vec<KeyResolvedTarget>,
    providers: Vec<KeyResolvedTarget>,
    snapshot: SummarySnapshot,
    policy: orch_types::Hash,
}
struct Reader;
impl native::Selected for Reader {
    type Source = Role;
    type Query = SummaryReadQuery;
    type Value = View;
    type Reply = SummarySnapshot;
    const DECLARATION: &'static Declaration<Role> = &declarations::RESOURCE_READ;
    const WATCH_ORDINARY: bool = true;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a SummaryReadQuery,
    ) -> BoxFuture<'a, ApiResult<(View, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let target = target(ctx)?;
            let wanted = range(query, target)?;
            let plan = Prepared::new(SummaryGet::ROUTE, declarations::RESOURCE_READ.operation)
                .map_err(source_error)?;
            let principal = Principal::load(
                conn,
                plan.preparation_caller(caller).map_err(source_error)?,
                ctx.app.clock.time(),
            )
            .await
            .map_err(ApiError::internal)?;
            let identity = sources::identify(conn, target)
                .await
                .map_err(source_error)?;
            require_disclosure(conn, &principal, target).await?;
            let policy = options::resolve(conn, &identity)
                .await
                .map_err(ApiError::engine)?;
            let personal = rows::get(conn, target, Some(principal.caller().user())).await?;
            let shared = rows::get(conn, target, None).await?;
            let latest = match personal.as_ref().or(shared.as_ref()) {
                Some(cache) => rows::latest(conn, cache.id).await?,
                None => None,
            };
            let cache = personal
                .as_ref()
                .filter(|row| row.text.is_some())
                .or(shared.as_ref())
                .cloned();
            let checked = cache_read::guard(conn, &principal, target, &plan, wanted, cache).await?;
            let cache = checked.cache;
            let inputs = checked.inputs;
            let inspecting = checked.inspecting;
            let fresh = if let Some(cache) = &cache {
                let cursor = if matches!(target, SummaryTarget::ChatTitle { .. }) {
                    None
                } else {
                    cache
                        .source_snapshot
                        .as_ref()
                        .and_then(|source| source.get("range"))
                        .cloned()
                        .map(serde_json::from_value::<Option<SummaryChatRange>>)
                        .transpose()
                        .map_err(ApiError::internal)?
                        .flatten()
                };
                let scope = sources::ReadScope::new(&plan, 0, 1).map_err(source_error)?;
                let previous = cache
                    .source_snapshot
                    .as_ref()
                    .map(sources::context_plan)
                    .transpose()
                    .map_err(source_error)?
                    .flatten();
                let budget = previous.as_ref().map_or(
                    sources::Budget {
                        tokens: policy.context_tokens.get(),
                        frame: 0,
                    },
                    |plan| plan.budget,
                );
                let material = sources::disclosure_material(
                    conn, &principal, target, &scope, cursor, budget, previous,
                )
                .await
                .map_err(source_error)?;
                cache
                    .source_snapshot
                    .as_ref()
                    .is_some_and(|kept| sources::same_content(kept, &material.snapshot))
                    && cache.completed_dirty_revision == Some(cache.dirty_revision)
            } else {
                false
            };
            let (producer, mut providers) =
                cache_read::producer(conn, &principal, &plan, cache.as_ref()).await?;
            let manual = if policy.producer.is_none() {
                Some(
                    resource::manual_default(
                        conn,
                        &principal,
                        identity.place(),
                        ctx.app.clock.time(),
                    )
                    .await
                    .map_err(ApiError::engine)?,
                )
            } else {
                None
            };
            let manual_producer = manual
                .as_ref()
                .and_then(|manual| manual.config.as_ref())
                .map(|config| {
                    Ok::<_, ApiError>(options::Producer {
                        kind: "model".into(),
                        config: serde_json::to_value(config).map_err(ApiError::internal)?,
                        writer: None,
                    })
                })
                .transpose()?;
            let preview_producer = policy.producer.as_ref().or(manual_producer.as_ref());
            let default_provider = preview_producer
                .filter(|producer| producer.kind == "model")
                .and_then(|producer| producer.config.get("provider_id"))
                .and_then(Json::as_str)
                .and_then(|id| id.parse::<Id>().ok());
            let default_fact = match default_provider {
                Some(id) => readable_provider_fact(conn, &principal, &plan, id).await?,
                None => None,
            };
            let route = resource::preview_route(
                conn,
                &principal,
                identity.place(),
                preview_producer,
                default_fact.is_some(),
            )
            .await
            .ok()
            .flatten();
            if route.as_ref().is_some_and(|route| {
                route.provider.is_some() || route.offering.is_some() || !route.prices.is_empty()
            }) && let Some(fact) = default_fact
                && !providers.iter().any(|old| old.object == fact.object)
            {
                providers.push(fact);
            }
            let unavailable = route
                .as_ref()
                .is_none_or(|resolution| resolution.route.is_none());
            let follow = follow::load(
                conn,
                identity.place(),
                latest.as_ref(),
                ctx.now,
                policy.follow_interval_secs,
            )
            .await?;
            let failed = latest.as_ref().is_some_and(|row| row.state == "failed");
            let state = if follow.scheduled {
                SummaryState::Scheduled
            } else if follow.updating {
                SummaryState::Updating
            } else if failed {
                SummaryState::Failed
            } else if policy.producer.is_none() && manual_producer.is_none() {
                SummaryState::NoSummarizer
            } else if cache.as_ref().and_then(|row| row.text.as_ref()).is_none() {
                if unavailable {
                    SummaryState::NoSummarizer
                } else {
                    SummaryState::Empty
                }
            } else if fresh {
                SummaryState::Fresh
            } else {
                SummaryState::Stale
            };
            let root_org_id = orch_db::queries::hierarchy::node(conn, identity.place())
                .await?
                .ok_or_else(ApiError::not_found)?
                .org_id;
            let snapshot = SummarySnapshot {
                root_org_id,
                target,
                inspecting,
                revision: personal
                    .as_ref()
                    .or(shared.as_ref())
                    .map_or(0, |row| row.revision),
                state,
                text: cache.as_ref().and_then(|row| row.text.clone()),
                producer,
                manual_problem: manual.and_then(|manual| manual.problem),
                wait: follow.wait,
                poll_after_ms: follow.poll_after_ms,
                input: cache
                    .as_ref()
                    .and_then(|row| row.source_snapshot.as_ref())
                    .and_then(|source| source.get("context"))
                    .and_then(|context| context.get("facts"))
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(ApiError::internal)?,
                updated_at: cache
                    .as_ref()
                    .and_then(|row| row.completed_at)
                    .map(Into::into),
                route,
                problem: (failed && !follow.updating).then(|| {
                    Problem::new(
                        ErrorCode::ProviderUnavailable,
                        "The summary could not be refreshed. The previous paragraph is kept.",
                    )
                }),
            };
            let facts = read_facts(&identity, &inputs, &providers);
            Ok((
                View {
                    identity,
                    inputs,
                    providers,
                    snapshot,
                    policy: policy.fingerprint,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { require_disclosure(conn, principal, target(ctx)?).await })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        value: &'a View,
    ) -> BoxFuture<'a, ApiResult<SummarySnapshot>> {
        Box::pin(async move { Ok(value.snapshot.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a SummaryReadQuery,
        old: &'a View,
        fresh: &'a View,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.identity.fact() == fresh.identity.fact()
                && old.inputs == fresh.inputs
                && old.providers == fresh.providers
                && old.policy == fresh.policy
                && old.snapshot == fresh.snapshot)
        })
    }
    fn after_read<'a>(
        ctx: &'a Ctx,
        _: &'a Caller,
        _: &'a SummaryReadQuery,
        value: &'a View,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let (mut conn, guard) = transaction_guard::begin(ctx).await?;
            let mut inputs = vec![value.identity.fact().clone()];
            inputs.extend(value.inputs.iter().cloned());
            note_chat_openings(&mut conn, guard.principal(), &inputs).await?;
            conn.commit().await.map_err(DbError::from)?;
            Ok(())
        })
    }
}

struct Refresh {
    selected: resource::Selection,
    body: SummaryRefreshBody,
}
struct Refresher;
impl effects::Effect for Refresher {
    type Source = Role;
    type Query = ();
    type Body = SummaryRefreshBody;
    type Value = Refresh;
    type Output = SummaryReceipt;
    fn declaration(_: &Refresh) -> &'static Declaration<Role> {
        &declarations::RESOURCE_REFRESH
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: (),
        body: SummaryRefreshBody,
    ) -> BoxFuture<'a, ApiResult<(Refresh, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let target = target(ctx)?;
            if body.range.is_some() && !matches!(target, SummaryTarget::Chat { .. }) {
                return Err(ApiError::invalid(
                    "range",
                    "Only a chat has a catch-up range",
                ));
            }
            let range = if let SummaryTarget::Chat { id } = target {
                match body.range {
                    Some(range) => Some(range),
                    None => {
                        let stage = Prepared::new(
                            SummaryRefresh::ROUTE,
                            declarations::RESOURCE_REFRESH.operation,
                        )
                        .map_err(source_error)?;
                        let principal = Principal::load(
                            conn,
                            stage.preparation_caller(caller).map_err(source_error)?,
                            ctx.app.clock.time(),
                        )
                        .await
                        .map_err(ApiError::internal)?;
                        Some(
                            sources::catch_up_range(conn, &principal, id)
                                .await
                                .map_err(source_error)?,
                        )
                    }
                }
            } else {
                None
            };
            let stage = Prepared::new(
                SummaryRefresh::ROUTE,
                declarations::RESOURCE_REFRESH.operation,
            )
            .map_err(source_error)?;
            let principal = Principal::load(
                conn,
                stage.preparation_caller(caller).map_err(source_error)?,
                ctx.app.clock.time(),
            )
            .await
            .map_err(ApiError::internal)?;
            let identity = sources::identify(conn, target)
                .await
                .map_err(source_error)?;
            if !stage
                .permits_binding(
                    principal.caller(),
                    0,
                    identity.fact().object.kind,
                    std::slice::from_ref(identity.fact()),
                )
                .map_err(source_error)?
            {
                return Err(ApiError::not_found());
            }
            require_source(conn, &principal, target).await?;
            if let Some(range) = range {
                let chat = orch_db::queries::chats::chat(conn, target.id())
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                if range.after < 0
                    || range.through < range.after
                    || range.through > chat.last_message_seq
                {
                    return Err(ApiError::invalid(
                        "range",
                        "Use an ordered range inside this chat",
                    ));
                }
            }
            if let Some(model) = &body.model
                && (model.model.trim().is_empty()
                    || model
                        .params
                        .as_ref()
                        .is_some_and(|params| !params.is_object()))
            {
                return Err(ApiError::invalid(
                    "model",
                    "Choose a named model with object options",
                ));
            }
            let fallback = if body.model.is_none()
                && options::resolve(conn, &identity)
                    .await
                    .map_err(ApiError::engine)?
                    .producer
                    .is_none()
            {
                let picked = resource::manual_default(
                    conn,
                    &principal,
                    identity.place(),
                    ctx.app.clock.time(),
                )
                .await
                .map_err(ApiError::engine)?;
                let config=picked.config.ok_or_else(||ApiError::new(ErrorCode::ProviderUnavailable,if picked.problem==Some(orch_api_contract::summaries::SummaryManualProblem::CapReached) {"A usage cap has no room for summaries"} else {"No permitted API model is available; check provider access and model allowlists"}))?;
                Some(orch_api_contract::summaries::SummaryModelChoice {
                    provider_id: config.provider_id,
                    model: config.model_name,
                    params: Some(config.params),
                })
            } else {
                None
            };
            let choice = body.model.as_ref().or(fallback.as_ref());
            let initial =
                resource::select(conn, caller, target, range, choice, ctx.app.clock.time())
                    .await
                    .map_err(ApiError::engine)?;
            resource::lock_selection(conn, &initial)
                .await
                .map_err(ApiError::engine)?;
            let selected = resource::select_with_snapshot(
                conn,
                caller,
                target,
                range,
                choice,
                ctx.app.clock.time(),
                Some(&initial.material.snapshot),
            )
            .await
            .map_err(ApiError::engine)?;
            let facts = selected.facts();
            Ok((Refresh { selected, body }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Refresh,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require_source(conn, principal, value.selected.material.identity.target()).await?;
            resource::require_model(
                conn,
                principal,
                value.selected.material.identity.place(),
                &value.selected.config,
            )
            .await
            .map_err(ApiError::engine)?;
            if !ctx
                .app
                .engine
                .room_for_own_use(
                    conn,
                    &orch_engine::caps::OwnCapAsk {
                        place: value.selected.material.identity.place(),
                        provider: value.selected.config.provider_id,
                        model: &value.selected.config.model_name,
                        tier: None,
                        purpose: "summary",
                        person: Some(principal.caller().user()),
                    },
                )
                .await
                .map_err(ApiError::engine)?
            {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "A usage cap has no room for summaries",
                ));
            }
            Ok(())
        })
    }
}
impl effects::ApplyOwned for Refresher {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Refresh,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<SummaryReceipt>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            let target = value.selected.material.identity.target();
            let origin =
                CapturedOperation::capture(principal.caller()).map_err(ApiError::internal)?;
            let http = native::http_origin::capture_operation(ctx, &mut tx, &origin).await?;
            let accepted = serde_json::to_value(http.origin()).map_err(ApiError::internal)?;
            let arguments = serde_json::to_value(&value.body).map_err(ApiError::internal)?;
            let source = json!({"target":target,"range":value.selected.material.range,"snapshot":value.selected.material.snapshot,"policy":value.selected.policy.fingerprint,"http":http,
                "model":{"provider_id":value.selected.config.provider_id,"model":value.selected.config.model_name,"params":value.selected.config.params}});
            before.check()?;
            let cache = rows::ensure(
                &mut tx,
                target,
                Some(principal.caller().user()),
                ctx.app.clock.now(),
            )
            .await?
            .ok_or_else(ApiError::not_found)?;
            let accepted = rows::enqueue(
                &mut tx,
                cache.id,
                rows::Origin::Person {
                    issuer: principal.caller().user(),
                    client_id: value.body.client_id,
                    accepted_operation: &accepted,
                },
                &arguments,
                &source,
                ctx.app.clock.now(),
                ctx.app.clock.now(),
            )
            .await?;
            let (work, changed) = match accepted {
                rows::Enqueued::New(row) => (row, true),
                rows::Enqueued::Replay(row) => (row, false),
                rows::Enqueued::Busy => {
                    return Err(ApiError::new(
                        ErrorCode::ConflictExists,
                        "A summary refresh is already in progress",
                    ));
                }
                rows::Enqueued::Conflict | rows::Enqueued::WrongOrigin => {
                    return Err(ApiError::new(
                        ErrorCode::ConflictIdempotencyMismatch,
                        "This refresh identity was used for a different request",
                    ));
                }
            };
            let revision = rows::get(&mut tx, target, Some(principal.caller().user()))
                .await?
                .map_or(cache.revision, |cache| cache.revision);
            deadline.check()?;
            tx.commit().await.map_err(DbError::from)?;
            let receipt = SummaryReceipt {
                projection: ReceiptProjection::Receipt,
                id: work.id,
                target,
                revision,
            };
            Ok(if changed {
                effects::Outcome::Applied(receipt)
            } else {
                effects::Outcome::Unchanged(receipt)
            })
        })
    }
}
async fn refresh(ctx: Ctx, _: (), _: SummaryRefreshBody) -> ApiResult<SummaryReceipt> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply_owned::<Refresher>(&ctx, tx).await?;
    publication::receipt(&done.ctx, &done.output)?;
    Ok(done.output)
}
