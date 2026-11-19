//! Once-only native observation, followed by the common selected-source preview.

use super::{adoption_of, failed};
use crate::key_operations::native;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard::{self, HttpIdentityMode};
use crate::registry::Ctx;
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    adoption_sources::{self as declarations, Source},
    nodes::{AdoptPlan, ThreadAdoptBody},
    operation_sources::Declaration,
};
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::Conn;
use orch_engine::adoption::operation as source;
use orch_engine::adoption::{AdoptError, Adoption, Fetched, Planned};
use std::sync::Arc;

pub(crate) mod creator;

pub(crate) fn complete_staged<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    topic: orch_types::Id,
    body: &'a ThreadAdoptBody,
) -> crate::registry::OperationFuture<'a> {
    creator::complete_staged(ctx, conn, caller, topic, body)
}

pub(super) struct Preview;
pub(super) struct Value {
    fetched: Arc<Fetched>,
    native: Planned,
    names: std::collections::BTreeSet<orch_types::Id>,
    limited: bool,
}

async fn problem(ctx: &Ctx, caller: &Caller, conn: &mut Conn, error: AdoptError) -> ApiError {
    let error = failed(conn, error).await;
    if native::modern(caller) || ctx.key_receipt_required() {
        crate::middleware::authorization::problem_without_content(error)
    } else {
        error
    }
}

async fn select(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: ThreadAdoptBody,
) -> ApiResult<(Value, Vec<native::Facts<Source>>)> {
    let adoption = adoption_of(ctx, body)?;
    let fetched = native::effects::initial_value::<Preview>(ctx)?;
    if fetched.adoption() != &adoption {
        return Err(ApiError::internal(
            "the adoption observation has another request",
        ));
    }
    let preparation = fetched
        .preparation()
        .ok_or_else(|| ApiError::internal("the native observation lost its HTTP operation"))?;
    if preparation.origin().operation() != ctx.route.name {
        return Err(ApiError::internal(
            "the native observation changed its HTTP operation",
        ));
    }
    let initial = source::initial_facts(conn, &adoption)
        .await
        .map_err(ApiError::engine)?;
    let known = native::bind_known(ctx, &declarations::PREVIEW_PREPARATION, &initial)?
        .with_origin(ctx, conn, caller, preparation.origin())
        .await?;
    let selecting = native::preparation_principal(ctx, conn, caller, &known).await?;
    let planned = match ctx
        .app
        .engine
        .prepare_adoption_plan_in(conn, &selecting, &fetched)
        .await
    {
        Ok(value) => value,
        Err(error) => return Err(problem(ctx, selecting.caller(), conn, error).await),
    };
    let mut facts = orch_engine::adoption::operation::initial_facts(conn, &adoption)
        .await
        .map_err(ApiError::engine)?;
    let mut read = Vec::new();
    if planned.reads_topic_policy()
        || (planned.environment().is_some() && planned.adopted_thread().is_none())
    {
        read.push((
            KeyTargetKind::Object(ObjectKind::Topic),
            vec![crate::key_operations::targets::node(conn, adoption.topic).await?],
        ));
    }
    if let Some(environment) = planned.environment() {
        let target = orch_engine::operation_scope::targets::resource_identity(
            conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: environment.id,
            },
        )
        .await
        .map_err(ApiError::internal)?;
        let Some(native::Facts { groups, .. }) =
            facts.iter_mut().find(|fact| fact.source == Source::Binding)
        else {
            return Err(ApiError::internal("the native adoption binding is absent"));
        };
        groups.push((
            KeyTargetKind::Object(ObjectKind::Environment),
            vec![target.clone()],
        ));
        read.push((KeyTargetKind::Object(ObjectKind::Environment), vec![target]));
    }
    facts.push(native::Facts::collection(Source::PreviewContent, read));
    let homes = planned
        .environment()
        .map(|row| vec![row.home_node_id])
        .unwrap_or_default();
    let graph = orch_authz::engine::Graph::load(conn, &homes)
        .await
        .map_err(ApiError::internal)?;
    let ids = homes
        .iter()
        .filter_map(|id| graph.site_of(*id))
        .flat_map(|site| site.path.unwrap_or_default())
        .collect();
    let known = native::bind_known(ctx, &declarations::PREVIEW, &facts)?;
    let (name_facts, names) = native::node_name_facts(
        &known,
        &declarations::PREVIEW,
        selecting.caller(),
        &graph,
        &ids,
        &[(ObjectKind::Org, Source::NameOrgs)],
    )?;
    facts.extend(name_facts);
    Ok((
        Value {
            fetched,
            native: planned,
            names,
            limited: native::modern(selecting.caller()),
        },
        facts,
    ))
}

impl native::effects::Effect for Preview {
    type Source = Source;
    type Query = ();
    type Body = ThreadAdoptBody;
    type Value = Value;
    type Output = ();
    fn declaration(_: &Value) -> &'static Declaration<Source> {
        &declarations::PREVIEW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ThreadAdoptBody,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move { select(ctx, conn, caller, body).await })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { require(ctx, conn, principal, value.fetched.adoption()).await })
    }
}

async fn require(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    adoption: &Adoption,
) -> ApiResult<()> {
    match orch_engine::adoption::operation::require_initial(conn, principal, adoption).await {
        Ok(()) => Ok(()),
        Err(error) => Err(problem(ctx, principal.caller(), conn, error).await),
    }
}

impl native::effects::Initial for Preview {
    type Input = Adoption;
    type Fetched = Fetched;
    fn declaration(_: &Adoption) -> &'static Declaration<Source> {
        &declarations::PREVIEW_PREPARATION
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        (): (),
        body: ThreadAdoptBody,
    ) -> BoxFuture<'a, ApiResult<(Adoption, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let adoption = adoption_of(ctx, body)?;
            let facts = orch_engine::adoption::operation::initial_facts(conn, &adoption)
                .await
                .map_err(ApiError::engine)?;
            Ok((adoption, facts))
        })
    }
    fn require_initial<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Adoption,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { require(ctx, conn, principal, input).await })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Adoption,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move { fetch(ctx, caller, input).await })
    }
}

impl native::Selected for Preview {
    type Source = Source;
    type Query = ((), ThreadAdoptBody);
    type Value = Value;
    type Reply = AdoptPlan;
    const DECLARATION: &'static Declaration<Source> = &declarations::PREVIEW;
    fn origin<'a>(_: &Self::Query, value: &'a Value) -> Option<&'a CapturedOperation> {
        value.fetched.preparation().map(|value| value.origin())
    }
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move { select(ctx, conn, caller, query.1.clone()).await })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let fetched = native::effects::initial_value::<Preview>(ctx)?;
            require(ctx, conn, principal, fetched.adoption()).await
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<AdoptPlan>> {
        Box::pin(async move {
            let mut plan = match ctx
                .app
                .engine
                .render_adoption_plan_with_names_in(
                    conn,
                    &value.fetched,
                    &value.native,
                    value.limited.then_some(&value.names),
                )
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(problem(ctx, principal.caller(), conn, error).await),
            };
            if value.native.adopted_thread().is_none()
                && let Some(environment) = value.native.environment()
            {
                plan.agent_access = Some(
                    super::super::binding::creation::existing_topic(
                        conn,
                        principal,
                        value.fetched.adoption().topic,
                        None,
                        None,
                        Some(environment),
                    )
                    .await?,
                );
            }
            Ok(plan)
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Query,
        opened: &'a Value,
        fresh: &'a Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(Arc::ptr_eq(&opened.fetched, &fresh.fetched)
                && opened.native == fresh.native
                && opened.names == fresh.names
                && opened.limited == fresh.limited)
        })
    }
}

async fn fetch(ctx: &Ctx, caller: &Caller, input: Adoption) -> ApiResult<Fetched> {
    let (credential, until) = http_origin(ctx).await?;
    let transport = history_transport(ctx.clone());
    match ctx
        .app
        .engine
        .fetch_adoption(caller, credential, input, Some(transport), until)
        .await
    {
        Ok(value) => Ok(value),
        Err(error) => {
            let mut conn = ctx.app.db.acquire().await?;
            Err(problem(ctx, caller, &mut conn, error).await)
        }
    }
}

pub(crate) async fn http_origin(
    ctx: &Ctx,
) -> ApiResult<(source::HttpCredential, Option<orch_types::Time>)> {
    transaction_guard::native_origin(ctx).await
}

/// The same actual request bridge for adopting, previewing and retained-history retry.
pub(crate) fn history_transport(ctx: Ctx) -> Arc<dyn source::Transport> {
    Arc::new(HttpTransport(ctx))
}

struct HttpTransport(Ctx);
impl source::Transport for HttpTransport {
    fn checkpoint<'a>(
        &'a self,
        conn: &'a mut Conn,
        input: source::Input,
    ) -> BoxFuture<'a, Result<source::Checkpoint, orch_engine::EngineError>> {
        Box::pin(async move {
            let ctx = &self.0;
            let origin = input.origin().clone();
            // Both branches consume the same native factory and exact raw HTTP bridge.
            // Their disjoint phase types cannot admit the unresolved main creator.
            match origin.operation() {
                "ThreadAdoptPreview" => {
                    http_checkpoint::<source::Previewing>(ctx, conn, &origin, input).await
                }
                "ThreadAdopt" => {
                    http_checkpoint::<source::Adopting>(ctx, conn, &origin, input).await
                }
                "ThreadAdoptRetryHistory" => {
                    http_checkpoint::<source::Retrying>(ctx, conn, &origin, input).await
                }
                _ => Err(orch_engine::EngineError::BeforeEffect(Box::new(
                    source::PreparationEnded,
                ))),
            }
        })
    }
}
async fn http_checkpoint<E: source::Entry>(
    ctx: &Ctx,
    conn: &mut Conn,
    origin: &CapturedOperation,
    input: source::Input,
) -> Result<source::Checkpoint, orch_engine::EngineError> {
    let (frame, deadline) =
        native::continuing::prepare_initial_on::<source::Phase<E>>(ctx, conn, origin, input)
            .await
            .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))?;
    let raw = transaction_guard::current_http_identity(ctx, conn, HttpIdentityMode::Read)
        .await
        .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))?;
    Ok(source::Checkpoint::from_frame(
        frame,
        raw.expires_at,
        Box::new(move || {
            deadline
                .check()
                .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))
        }),
    ))
}
