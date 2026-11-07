//! The native reviewed node update batch, with once-only observations and optional readback.

use super::*;
use crate::key_operations::native::effects::{
    self, ApplyOwned, Effect, FirstEffect, Initial, Outcome,
};
use crate::key_operations::{native, targets};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    common::ReceiptProjection,
    environment_sources::update_batch::{NODE_PREFLIGHT, Source},
    operation_sources::Declaration,
    operations::FacetUsage,
};
use orch_authz::Caller;
use orch_engine::updates::{Draft, PlanningObservation, operation::Input as NativeInput};
use std::collections::BTreeMap;

mod readback;

pub(super) struct Preflight;

#[derive(Clone)]
pub(super) struct Selection {
    node: Id,
    visible: Vec<Id>,
    managed: Vec<Id>,
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

fn source_error(error: orch_engine::EngineError) -> ApiError {
    match error {
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
fn update_error(error: UpdateError) -> ApiError {
    match error {
        UpdateError::Internal(error) => source_error(error),
        other => plans::refused(other),
    }
}

async fn node_view(conn: &mut Conn, principal: &Principal, node: Id) -> ApiResult<()> {
    super::recycle_operation::node_view(conn, principal, node).await
}

async fn select(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: api::NodeEnvironmentUpdatePreflightBody,
) -> ApiResult<Selection> {
    let node = ctx.id("id")?;
    let known = native::bind_known(ctx, &NODE_PREFLIGHT, &[])?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    node_view(conn, &principal, node).await?;
    let mut ids = body.environment_ids;
    ids.sort_unstable();
    ids.dedup();
    let under = envs::environments_under(conn, node).await?;
    if ids.iter().any(|id| !under.contains(id)) {
        return Err(ApiError::not_found());
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
            &NODE_PREFLIGHT,
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
    sources: BTreeMap<Id, NativeInput>,
    observations: BTreeMap<Id, PlanningObservation>,
    accepted: Caller,
    until: Option<Time>,
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

fn retained_deadline(ctx: &Ctx, caller: &Caller, until: Option<Time>) -> ApiResult<()> {
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
    fn receipt(&self) -> api::NodeEnvironmentUpdatePreflightReceipt {
        api::NodeEnvironmentUpdatePreflightReceipt {
            projection: ReceiptProjection::Receipt,
            node_id: self.planned.selection.node,
            preflight_id: self.id,
            expires_at: Time::new(updates::expires(self.at)),
            executable: self
                .planned
                .drafts
                .iter()
                .filter(|draft| draft.blockers.is_empty())
                .count() as i64,
            skipped: self.planned.skipped.len() as i64,
        }
    }
}

impl Effect for Preflight {
    type Source = Source;
    type Query = ();
    type Body = api::NodeEnvironmentUpdatePreflightBody;
    type Value = Planned;
    type Output = Stored;

    fn declaration(_: &Planned) -> &'static Declaration<Source> {
        &NODE_PREFLIGHT
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
            let mut selection = select(ctx, conn, caller, body.clone()).await?;
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                // Lock the original native set in identifier order, including members that
                // currently become skipped. Then obtain the current visibility/scope selection.
                for source in initial.sources.values() {
                    source.lock(conn).await.map_err(source_error)?;
                }
                selection = select(ctx, conn, caller, body).await?;
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
            let known = native::bind_known(ctx, &NODE_PREFLIGHT, &facts)?;
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
            let place = orch_settings::Place::Node(selection.node);
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

impl Initial for Preflight {
    type Input = Selection;
    type Fetched = Fetched;

    fn declaration(_: &Selection) -> &'static Declaration<Source> {
        &NODE_PREFLIGHT
    }

    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Selection, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let selected = select(ctx, conn, caller, body).await?;
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
        Box::pin(async move {
            let (credential, mut until) = transaction_guard::native_origin(ctx).await?;
            let mut accepted = caller.clone();
            let mut conn = ctx.app.db.acquire().await?;
            let principal =
                authorization::load_principal(&mut conn, caller.clone(), ctx.app.clock.time())
                    .await?;
            let principal =
                crate::sign_in::second_step::hold(&mut conn, ctx.user()?, principal).await?;
            let mut sources = BTreeMap::new();
            let mut observations = BTreeMap::new();
            for id in &selected.visible {
                let mut source = NativeInput::capture(
                    &mut conn,
                    &accepted,
                    credential.clone(),
                    until,
                    selected.node,
                    *id,
                )
                .await
                .map_err(source_error)?;
                if selected.managed.contains(id) {
                    let (result, observation) = ctx
                        .app
                        .engine
                        .observe_update(
                            &mut conn,
                            &principal,
                            Target::Environment(*id),
                            api::UpdatePreflightReason::Update,
                            None,
                            &mut source,
                        )
                        .await;
                    match result {
                        Ok(_)
                        | Err(
                            UpdateError::Denied | UpdateError::Blocked(_) | UpdateError::NotFound,
                        ) => {}
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
        })
    }
}

impl ApplyOwned for Preflight {
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
            plans::keep_batch(
                ctx,
                &mut tx,
                id,
                Some(value.selection.node),
                &recipes,
                false,
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

pub(super) async fn preflight(
    ctx: Ctx,
    (): (),
    _: api::NodeEnvironmentUpdatePreflightBody,
) -> ApiResult<api::NodeEnvironmentUpdatePreflightResponse> {
    let tx = ctx.app.db.begin().await?;
    let done = effects::apply_owned::<Preflight>(&ctx, tx).await?;
    let receipt = done.output.receipt();
    crate::key_operations::publication::receipt(&done.ctx, &receipt)?;
    let reader = done
        .accepted
        .intersect_restrictions(&done.output.planned.accepted)
        .map_err(ApiError::internal)?;
    done.ctx
        .publication
        .retain_deadline(done.output.planned.until)?;
    match native::reply::render::<readback::Reply>(&done.ctx, &reader, done.output).await? {
        Some(full) => Ok(api::NodeEnvironmentUpdatePreflightResponse::Full(Box::new(
            full,
        ))),
        None => Ok(api::NodeEnvironmentUpdatePreflightResponse::Receipt(
            receipt,
        )),
    }
}
