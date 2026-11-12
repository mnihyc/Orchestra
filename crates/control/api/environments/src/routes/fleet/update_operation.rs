//! The native reviewed node update batch, with once-only observations and optional readback.

use super::*;

use crate::key_operations::native::effects;

use crate::key_operations::native::effects::ApplyOwned;

use crate::key_operations::native::effects::Effect;

use crate::key_operations::native::effects::FirstEffect;

use crate::key_operations::native::effects::Initial;

use crate::key_operations::native::effects::Outcome;

use crate::key_operations::native;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::environment_sources::update_batch::GLOBAL_PREFLIGHT;

use orch_api_contract::environment_sources::update_batch::NODE_PREFLIGHT;

use orch_api_contract::environment_sources::update_batch::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_authz::Caller;

use orch_engine::updates::Draft;

use orch_engine::updates::PlanningObservation;

use orch_engine::updates::operation::Input as NativeInput;

use std::collections::BTreeMap;
pub(super) struct Preflight<const GLOBAL: bool = false>;

const fn declaration<const GLOBAL: bool>() -> &'static Declaration<Source> {
    if GLOBAL {
        &GLOBAL_PREFLIGHT
    } else {
        &NODE_PREFLIGHT
    }
}

#[derive(Clone)]
pub(super) struct Selection {
    pub(super) node: Option<Id>,
    pub(super) visible: Vec<Id>,
    pub(super) managed: Vec<Id>,
}

pub(super) async fn environment_facts(
    conn: &mut Conn,
    source: Source,
    ids: &[Id],
) -> ApiResult<native::Facts<Source>> {
    let mut found = Vec::new();
    for id in ids {
        found.push(
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: *id,
                },
            )
            .await?,
        );
    }
    Ok(native::Facts::collection(
        source,
        if found.is_empty() {
            Vec::new()
        } else {
            vec![(KeyTargetKind::Object(ObjectKind::Environment), found)]
        },
    ))
}

pub(super) fn source_error(error: orch_engine::EngineError) -> ApiError {
    match error {
        orch_engine::EngineError::BeforeEffect(error)
            if error.is::<orch_engine::updates::operation::ReviewExpired>() =>
        {
            plans::refused(UpdateError::Expired)
        }
        orch_engine::EngineError::BeforeEffect(error)
            if error.is::<orch_engine::updates::operation::Ended>()
                || error.is::<orch_engine::updates::operation::Denied>() =>
        {
            ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "the original update review no longer permits this work",
            )
        }
        other => ApiError::engine(other),
    }
}

pub(super) fn update_error(error: UpdateError) -> ApiError {
    match error {
        UpdateError::Internal(error) => source_error(error),
        other => plans::refused(other),
    }
}

async fn node_view(conn: &mut Conn, principal: &Principal, node: Option<Id>) -> ApiResult<()> {
    if let Some(node) = node {
        super::recycle_operation::node_view(conn, principal, node).await
    } else {
        Ok(())
    }
}

async fn select<const GLOBAL: bool>(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: api::NodeEnvironmentUpdatePreflightBody,
) -> ApiResult<Selection> {
    let node = if GLOBAL { None } else { Some(ctx.id("id")?) };
    let known = native::bind_known(ctx, declaration::<GLOBAL>(), &[])?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    node_view(conn, &principal, node).await?;
    let mut ids = body.environment_ids;
    ids.sort_unstable();
    ids.dedup();
    if let Some(node) = node {
        let under = envs::environments_under(conn, node).await?;
        if ids.iter().any(|id| !under.contains(id)) {
            return Err(ApiError::not_found());
        }
    }
    // This is the same ordinary visibility selector used by environments_of, before rendering.
    let allowed = environments::reading::among(conn, &principal, &ids).await?;
    let rows = orch_db::queries::environments::routes::environments(conn, &ids).await?;
    let visible = rows
        .into_iter()
        .filter(|row| allowed.contains_key(&row.id))
        .map(|row| row.id)
        .collect::<Vec<_>>();
    let mut managed = Vec::new();
    for id in &visible {
        let fact = targets::resource_identity(
            conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: *id,
            },
        )
        .await?;
        if native::permits_usage(
            &known,
            declaration::<GLOBAL>(),
            principal.caller(),
            &Source::Environments,
            FacetUsage::Required,
            KeyTargetKind::Object(ObjectKind::Environment),
            &fact,
        )? {
            managed.push(*id);
        }
    }
    Ok(Selection {
        node,
        visible,
        managed,
    })
}

pub(super) struct Fetched {
    pub(super) sources: BTreeMap<Id, NativeInput>,
    pub(super) observations: BTreeMap<Id, PlanningObservation>,
    pub(super) accepted: Caller,
    pub(super) until: Option<Time>,
}

#[derive(Clone)]
pub(super) struct Planned {
    selection: Selection,
    drafts: Vec<Draft>,
    skipped: Vec<(Id, api::UpdateSkipReason)>,
    options: orch_api_contract::forks::AppliedChangeOptions,
    parallel: i64,
    accepted: Caller,
    until: Option<Time>,
}

pub(super) fn retained_deadline(ctx: &Ctx, caller: &Caller, until: Option<Time>) -> ApiResult<()> {
    if !caller.key_deadline_allows(ctx.app.clock.as_ref())
        || until.is_some_and(|end| ctx.app.clock.time() >= end)
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthInvalidToken,
            "the native update review credential has ended",
        ));
    }
    Ok(())
}

fn minimum(a: Option<Time>, b: Option<Time>) -> Option<Time> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

#[derive(Clone)]
pub(super) struct Stored {
    planned: Planned,
    sources: BTreeMap<Id, NativeInput>,
    id: Id,
    at: chrono::DateTime<chrono::Utc>,
}

impl Stored {
    fn executable(&self) -> i64 {
        self.planned
            .drafts
            .iter()
            .filter(|draft| draft.blockers.is_empty())
            .count() as i64
    }
}

impl<const GLOBAL: bool> Effect for Preflight<GLOBAL> {
    type Source = Source;
    type Query = ();
    type Body = api::NodeEnvironmentUpdatePreflightBody;
    type Value = Planned;
    type Output = Stored;

    fn declaration(_: &Planned) -> &'static Declaration<Source> {
        declaration::<GLOBAL>()
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Planned, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let initial = effects::initial_value::<Self>(ctx)?;
            let caller = &caller
                .intersect_restrictions(&initial.accepted)
                .map_err(ApiError::internal)?;
            retained_deadline(ctx, caller, initial.until)?;
            let mut selection = select::<GLOBAL>(ctx, conn, caller, body.clone()).await?;
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                // Lock the original native set in identifier order, including members that
                // currently become skipped. Then obtain the current visibility/scope selection.
                for source in initial.sources.values() {
                    source.lock(conn).await.map_err(source_error)?;
                }
                selection = select::<GLOBAL>(ctx, conn, caller, body).await?;
            }
            if selection
                .visible
                .iter()
                .any(|id| !initial.sources.contains_key(id))
            {
                return Err(plans::refused(UpdateError::Expired));
            }
            let facts =
                vec![environment_facts(conn, Source::Environments, &selection.managed).await?];
            let known = native::bind_known(ctx, declaration::<GLOBAL>(), &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            node_view(conn, &principal, selection.node).await?;
            let mut drafts = Vec::new();
            let mut skipped = Vec::new();
            for id in &selection.visible {
                if !selection.managed.contains(id) {
                    skipped.push((*id, api::UpdateSkipReason::NoPermission));
                    continue;
                }
                let observation = initial
                    .observations
                    .get(id)
                    .ok_or_else(|| plans::refused(UpdateError::Expired))?;
                match ctx
                    .app
                    .engine
                    .replay_update(
                        conn,
                        &principal,
                        Target::Environment(*id),
                        api::UpdatePreflightReason::Update,
                        None,
                        observation,
                    )
                    .await
                {
                    Ok(draft) if draft.updates.is_empty() => {
                        skipped.push((*id, api::UpdateSkipReason::UpToDate))
                    }
                    Ok(draft) => drafts.push(draft),
                    Err(UpdateError::Denied) => {
                        skipped.push((*id, api::UpdateSkipReason::NoPermission))
                    }
                    Err(UpdateError::Blocked(_) | UpdateError::NotFound) => {
                        skipped.push((*id, api::UpdateSkipReason::NotUpdatable))
                    }
                    Err(error) => return Err(update_error(error)),
                }
            }
            let place = selection
                .node
                .map_or(orch_settings::Place::System, orch_settings::Place::Node);
            let options = updates::options(conn, place, None)
                .await
                .map_err(plans::refused)?;
            let parallel = orch_settings::get::<i64>(
                conn,
                place,
                orch_settings_contract::registry::keys::SWITCH_BATCH_PARALLEL,
            )
            .await
            .map_err(ApiError::internal)?
            .unwrap_or(updates::DEFAULT_BATCH_PARALLEL);
            Ok((
                Planned {
                    selection,
                    drafts,
                    skipped,
                    options,
                    parallel,
                    accepted: caller.clone(),
                    until: initial.until,
                },
                facts,
            ))
        })
    }

    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Planned,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { node_view(conn, principal, value.selection.node).await })
    }
}

impl<const GLOBAL: bool> Initial for Preflight<GLOBAL> {
    type Input = Selection;
    type Fetched = Fetched;

    fn declaration(_: &Selection) -> &'static Declaration<Source> {
        declaration::<GLOBAL>()
    }

    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Selection, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let selected = select::<GLOBAL>(ctx, conn, caller, body).await?;
            let facts = environment_facts(conn, Source::Environments, &selected.managed).await?;
            Ok((selected, vec![facts]))
        })
    }

    fn require_initial<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Selection,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { node_view(conn, principal, input.node).await })
    }

    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        selected: Selection,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move { observe(ctx, caller, &selected, None, None, None).await })
    }
}

pub(super) async fn observe(
    ctx: &Ctx,
    caller: &Caller,
    selected: &Selection,
    given: Option<&orch_api_contract::forks::ChangeOptions>,
    expected: Option<&BTreeMap<Id, Id>>,
    review: Option<Id>,
) -> ApiResult<Fetched> {
    let (credential, mut until) = transaction_guard::native_origin(ctx).await?;
    let mut accepted = caller.clone();
    let mut conn = ctx.app.db.acquire().await?;
    let principal =
        authorization::load_principal(&mut conn, caller.clone(), ctx.app.clock.time()).await?;
    let principal = crate::sign_in::second_step::hold(&mut conn, ctx.user()?, principal).await?;
    let mut sources = BTreeMap::new();
    let mut observations = BTreeMap::new();
    for id in &selected.visible {
        let mut source = match selected.node {
            Some(node) => {
                NativeInput::capture(&mut conn, caller, credential.clone(), until, node, *id).await
            }
            None => {
                NativeInput::capture_global(
                    &mut conn,
                    caller,
                    credential.clone(),
                    until,
                    *id,
                    ctx.route.name
                        == orch_api_contract::environment_sources::update_batch::GLOBAL_CREATE
                            .operation,
                )
                .await
            }
        }
        .map_err(source_error)?;
        if let Some(review) = review {
            source = source
                .reviewing(&mut conn, review)
                .await
                .map_err(source_error)?;
        }
        if expected.is_some_and(|expected| expected.get(id).copied() != source.runtime()) {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::ConflictStaleRevision,
                "a reviewed native runtime changed; review a new plan",
            ));
        }
        if selected.managed.contains(id) {
            source
                .narrow(&mut conn, ctx.app.clock.as_ref(), &accepted, until)
                .await
                .map_err(source_error)?;
            let (result, observation) = ctx
                .app
                .engine
                .observe_update(
                    &mut conn,
                    &principal,
                    Target::Environment(*id),
                    api::UpdatePreflightReason::Update,
                    given,
                    &mut source,
                )
                .await;
            match result {
                Ok(_)
                | Err(UpdateError::Denied | UpdateError::Blocked(_) | UpdateError::NotFound) => {}
                Err(error) => return Err(update_error(error)),
            }
            observations.insert(*id, observation);
        }
        let current = source
            .origin()
            .restore_current(&mut conn, ctx.app.clock.as_ref())
            .await
            .map_err(authorization::restoration_error)?
            .ok_or_else(|| {
                ApiError::new(
                    orch_api_contract::common::ErrorCode::AuthzDenied,
                    "the original update observation is closed",
                )
            })?;
        accepted = accepted
            .intersect_restrictions(&current)
            .map_err(ApiError::internal)?;
        until = minimum(until, source.until());
        retained_deadline(ctx, &accepted, until)?;
        sources.insert(*id, source);
    }
    Ok(Fetched {
        sources,
        observations,
        accepted,
        until,
    })
}

impl<const GLOBAL: bool> ApplyOwned for Preflight<GLOBAL> {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        _: &'a Principal,
        value: &'a Planned,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Stored>>> {
        Box::pin(async move {
            let initial = effects::initial_value::<Self>(ctx)?;
            let sources = value
                .selection
                .visible
                .iter()
                .map(|id| {
                    initial
                        .sources
                        .get(id)
                        .map(|source| (*id, source.clone()))
                        .ok_or_else(|| plans::refused(UpdateError::Expired))
                })
                .collect::<ApiResult<BTreeMap<_, _>>>()?;
            let id = new_id();
            let recipes = value
                .drafts
                .iter()
                .filter(|draft| draft.blockers.is_empty())
                .map(|draft| draft.recipe.clone())
                .collect::<Vec<_>>();
            let deadline = before.deadline();
            before.check()?;
            retained_deadline(ctx, &value.accepted, value.until)?;
            plans::keep_batch_with_sources(
                ctx,
                &mut tx,
                id,
                value.selection.node,
                &recipes,
                &sources,
            )
            .await?;
            deadline.check()?;
            retained_deadline(ctx, &value.accepted, value.until)?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(Outcome::Applied(Stored {
                planned: value.clone(),
                sources,
                id,
                at: ctx.now,
            }))
        })
    }
}

#[derive(serde::Serialize)]
#[serde(untagged)]
enum Receipt {
    Node(api::NodeEnvironmentUpdatePreflightReceipt),
    Global(api::EnvironmentUpdateBatchPreflightReceipt),
}

async fn run_preflight<const GLOBAL: bool>(
    ctx: Ctx,
) -> ApiResult<(Option<api::EnvironmentUpdateBatchPlan>, Receipt)> {
    let tx = ctx.app.db.begin().await?;
    let done = effects::apply_owned::<Preflight<GLOBAL>>(&ctx, tx).await?;
    let receipt = if GLOBAL {
        Receipt::Global(api::EnvironmentUpdateBatchPreflightReceipt {
            projection: ReceiptProjection::Receipt,
            preflight_id: done.output.id,
        })
    } else {
        Receipt::Node(api::NodeEnvironmentUpdatePreflightReceipt {
            projection: ReceiptProjection::Receipt,
            node_id: done
                .output
                .planned
                .selection
                .node
                .ok_or_else(|| ApiError::internal("the native node selector is missing"))?,
            preflight_id: done.output.id,
            expires_at: Time::new(updates::expires(done.output.at)),
            executable: done.output.executable(),
            skipped: done.output.planned.skipped.len() as i64,
        })
    };
    crate::key_operations::publication::receipt(&done.ctx, &receipt)?;
    let reader = done
        .accepted
        .intersect_restrictions(&done.output.planned.accepted)
        .map_err(ApiError::internal)?;
    done.ctx
        .publication
        .retain_deadline(done.output.planned.until)?;
    let full = native::reply::render::<readback::Reply>(&done.ctx, &reader, done.output).await?;
    Ok((full, receipt))
}

pub(super) async fn preflight(
    ctx: Ctx,
    (): (),
    _: api::NodeEnvironmentUpdatePreflightBody,
) -> ApiResult<api::NodeEnvironmentUpdatePreflightResponse> {
    let (full, receipt) = run_preflight::<false>(ctx).await?;
    match (full, receipt) {
        (Some(full), _) => Ok(api::NodeEnvironmentUpdatePreflightResponse::Full(Box::new(
            full,
        ))),
        (None, Receipt::Node(receipt)) => Ok(api::NodeEnvironmentUpdatePreflightResponse::Receipt(
            receipt,
        )),
        _ => Err(ApiError::internal("the native update selector changed")),
    }
}

pub(super) async fn global_preflight(
    ctx: Ctx,
    (): (),
    _: api::NodeEnvironmentUpdatePreflightBody,
) -> ApiResult<api::EnvironmentUpdateBatchPreflightResponse> {
    let (full, receipt) = run_preflight::<true>(ctx).await?;
    match (full, receipt) {
        (Some(full), _) => Ok(api::EnvironmentUpdateBatchPreflightResponse::Full(full)),
        (None, Receipt::Global(receipt)) => Ok(
            api::EnvironmentUpdateBatchPreflightResponse::Receipt(receipt),
        ),
        _ => Err(ApiError::internal("the native update selector changed")),
    }
}

pub(super) async fn response_caller(
    ctx: &Ctx,
    caller: &Caller,
    origins: &[NativeInput],
) -> ApiResult<Option<Caller>> {
    let mut caller = caller.clone();
    let mut conn = ctx.app.db.acquire().await?;
    for source in origins
        .iter()
        .filter(|source| source.origin().operation() == ctx.route.name)
    {
        let (current, until) = match source
            .restore_current(&mut conn, ctx.app.clock.as_ref())
            .await
        {
            Ok(current) => current,
            Err(orch_engine::EngineError::BeforeEffect(error))
                if error.is::<orch_engine::updates::operation::Ended>()
                    || error.is::<orch_engine::updates::operation::Denied>() =>
            {
                return Ok(None);
            }
            Err(error) => return Err(source_error(error)),
        };
        caller = caller
            .intersect_restrictions(&current)
            .map_err(ApiError::internal)?;
        ctx.publication.retain_deadline(until)?;
    }
    Ok(Some(caller))
}

#[path = "update_operation/confirmation.rs"]
pub mod confirmation;

#[path = "update_operation/names.rs"]
pub mod names;

#[path = "update_operation/readback.rs"]
pub mod readback;
