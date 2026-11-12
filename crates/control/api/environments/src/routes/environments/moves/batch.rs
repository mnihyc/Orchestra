//! Native node move observations supplied once, then admitted and saved on the real action connection.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::move_batch_sources::PREFLIGHT;

use orch_api_contract::move_batch_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_authz::Caller;

use std::collections::BTreeMap;

use std::collections::BTreeSet;

use std::sync::Arc;
pub(in crate::routes) struct Preview;

pub(in crate::routes) struct Input {
    id: Id,
    body: api::NodeEnvironmentMovePreflightBody,
    allowed: BTreeSet<Id>,
}

pub(in crate::routes) struct Ready {
    observed: Arc<ObservedMoveBatch>,
}

pub(in crate::routes) struct Saved {
    observed: Arc<ObservedMoveBatch>,
}

async fn node(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<()> {
    authorization::require(
        conn,
        principal,
        id,
        authorization::VIEW,
        orch_authz::Act::Use,
        Policy::default(),
    )
    .await
}

async fn objects(
    conn: &mut Conn,
    source: Source,
    objects: impl IntoIterator<Item = (ObjectKind, Id)>,
) -> ApiResult<native::Facts<Source>> {
    let mut groups = BTreeMap::new();
    for (kind, id) in objects.into_iter().collect::<BTreeSet<_>>() {
        let target = match kind {
            ObjectKind::Org | ObjectKind::Project | ObjectKind::Topic | ObjectKind::Thread => {
                targets::node_identity(conn, id).await?
            }
            ObjectKind::Machine => targets::machine_subject(conn, id).await?,
            _ => targets::resource_identity(conn, ObjectRef { kind, id }).await?,
        };
        groups
            .entry(KeyTargetKind::Object(kind))
            .or_insert_with(Vec::new)
            .push(target);
    }
    Ok(native::Facts::collection(
        source,
        groups.into_iter().collect(),
    ))
}

async fn required(
    conn: &mut Conn,
    observed: &ObservedMoveBatch,
) -> ApiResult<Vec<native::Facts<Source>>> {
    Ok(vec![
        objects(
            conn,
            Source::Environments,
            observed
                .managed
                .iter()
                .map(|id| (ObjectKind::Environment, *id)),
        )
        .await?,
        objects(
            conn,
            Source::Destinations,
            observed.destinations.iter().copied(),
        )
        .await?,
    ])
}

async fn initial_facts(
    conn: &mut Conn,
    allowed: &BTreeSet<Id>,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let mut facts = vec![
        objects(
            conn,
            Source::Environments,
            allowed.iter().map(|id| (ObjectKind::Environment, *id)),
        )
        .await?,
    ];
    if allowed.is_empty() {
        // No selected move can use a destination. Nonempty selections still need
        // the native observation to resolve their actual Host/Template/Machine.
        facts.push(objects(conn, Source::Destinations, std::iter::empty()).await?);
    }
    Ok(facts)
}

async fn ordinary(
    conn: &mut Conn,
    principal: &Principal,
    observed: &ObservedMoveBatch,
) -> ApiResult<()> {
    node(conn, principal, observed.node).await?;
    for id in &observed.managed {
        if !updates::may_manage(conn, principal, updates::Target::Environment(*id))
            .await
            .map_err(ApiError::engine)?
        {
            return Err(refused(updates::UpdateError::Denied));
        }
    }
    for (kind, id) in &observed.destinations {
        let permission = match kind {
            ObjectKind::Host => Permission::HostUse,
            ObjectKind::Machine => Permission::MachineUse,
            ObjectKind::Template => Permission::TemplateUse,
            _ => {
                return Err(ApiError::internal(
                    "the native move selected an unsupported destination",
                ));
            }
        };
        if !orch_authz::resources::check(
            conn,
            principal,
            ObjectRef {
                kind: *kind,
                id: *id,
            },
            permission,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .is_allowed()
        {
            return Err(refused(updates::UpdateError::Denied));
        }
    }
    Ok(())
}

async fn associations(conn: &mut Conn, observed: &ObservedMoveBatch) -> ApiResult<bool> {
    for (id, expected) in &observed.associations {
        let Some(row) = orch_db::queries::environments::hosting::environment(conn, *id).await?
        else {
            return Ok(false);
        };
        let runtime = orch_db::queries::environments::hosting::live_runtime(conn, *id)
            .await?
            .map(|runtime| runtime.id);
        if row.deleted_at.is_some()
            || (MoveSourceAssociation {
                owner: row.owner_id,
                home: row.home_node_id,
                generation: row.generation,
                machine: row.machine_id,
                runtime,
            }) != *expected
        {
            return Ok(false);
        }
    }
    Ok(true)
}

impl effects::Effect for Preview {
    type Source = Source;
    type Query = ();
    type Body = api::NodeEnvironmentMovePreflightBody;
    type Value = Ready;
    type Output = Saved;
    fn declaration(_: &Ready) -> &'static Declaration<Source> {
        &PREFLIGHT
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        (): (),
        _: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let observed = effects::initial_value::<Self>(ctx)?;
            if observed.node != ctx.id("id")? {
                return Err(ApiError::not_found());
            }
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                let mut ids = observed.visible.clone();
                ids.sort_unstable();
                for id in ids {
                    envs_lock(conn, id).await?;
                }
            }
            let under =
                orch_db::queries::environments::hosting::environments_under(conn, observed.node)
                    .await?;
            if observed.visible.iter().any(|id| !under.contains(id)) {
                return Err(ApiError::not_found());
            }
            if observed.plan.expires_at <= ctx.app.clock.time() {
                return Err(refused(updates::UpdateError::Expired));
            }
            if !associations(conn, &observed).await? {
                return Err(ApiError::new(
                    orch_api_contract::common::ErrorCode::ConflictStaleRevision,
                    "a move source changed during observation; review the batch again",
                ));
            }
            let facts = required(conn, &observed).await?;
            Ok((Ready { observed }, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { ordinary(conn, principal, &value.observed).await })
    }
}

async fn envs_lock(conn: &mut Conn, id: Id) -> ApiResult<()> {
    orch_db::queries::environments::hosting::lock_environment(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(())
}

impl effects::Initial for Preview {
    type Input = Input;
    type Fetched = ObservedMoveBatch;
    fn declaration(_: &Input) -> &'static Declaration<Source> {
        &PREFLIGHT
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let known = native::bind_known(ctx, &PREFLIGHT, &[])?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let id = ctx.id("id")?;
            node(conn, &principal, id).await?;
            let under =
                orch_db::queries::environments::hosting::environments_under(conn, id).await?;
            if body.environment_ids.iter().any(|id| !under.contains(id)) {
                return Err(ApiError::not_found());
            }
            let selected =
                super::super::prepare_viewed(ctx, conn, &principal, &body.environment_ids).await?;
            let mut allowed = BTreeSet::new();
            for id in selected.selected_ids() {
                let target = targets::resource_identity(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id,
                    },
                )
                .await?;
                if native::permits_usage(
                    &known,
                    &PREFLIGHT,
                    principal.caller(),
                    &Source::Environments,
                    FacetUsage::Required,
                    KeyTargetKind::Object(ObjectKind::Environment),
                    &target,
                )? {
                    allowed.insert(id);
                }
            }
            let facts = initial_facts(conn, &allowed).await?;
            Ok((
                Input {
                    id: new_id(),
                    body,
                    allowed,
                },
                facts,
            ))
        })
    }
    fn require_initial<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { node(conn, principal, ctx.id("id")?).await })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Input,
    ) -> BoxFuture<'a, ApiResult<ObservedMoveBatch>> {
        Box::pin(async move {
            let principal = {
                let mut conn = ctx.app.db.acquire().await?;
                let facts = initial_facts(&mut conn, &input.allowed).await?;
                let known = native::bind_known(ctx, &PREFLIGHT, &facts)?;
                native::preparation_principal(ctx, &mut conn, caller, &known).await?
            };
            observe_batch(ctx, &principal, input.body, input.id, &input.allowed).await
        })
    }
}

impl effects::Apply for Preview {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Saved>>> {
        Box::pin(async move {
            let observed = &value.observed;
            let deadline = before.deadline();
            before.check()?;
            jobs::forks::save_preflight(
                conn,
                &PreflightRow {
                    id: observed.plan.preflight_id,
                    kind: "move_batch".into(),
                    node_id: Some(observed.node),
                    environment_id: None,
                    created_by: principal.user(),
                    plan: serde_json::to_value(&observed.stored).map_err(ApiError::internal)?,
                    expires_at: observed.plan.expires_at.get(),
                    created_at: ctx.now,
                },
            )
            .await?;
            orch_db::queries::notifications::operations::preview(
                conn,
                observed.plan.preflight_id,
                &observed
                    .audience
                    .iter()
                    .map(|person| person.user)
                    .collect::<Vec<_>>(),
            )
            .await?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(Saved {
                observed: observed.clone(),
            }))
        })
    }
}

async fn content(
    conn: &mut Conn,
    observed: &ObservedMoveBatch,
) -> ApiResult<native::Facts<Source>> {
    let mut selected = observed.content.clone();
    let mut names = BTreeSet::new();
    for (kind, id) in &selected {
        let target = match kind {
            ObjectKind::Org | ObjectKind::Project | ObjectKind::Topic | ObjectKind::Thread => {
                targets::node_identity(conn, *id).await?
            }
            ObjectKind::Machine => targets::machine_subject(conn, *id).await?,
            _ => {
                targets::resource_identity(
                    conn,
                    ObjectRef {
                        kind: *kind,
                        id: *id,
                    },
                )
                .await?
            }
        };
        names.extend(
            target
                .node_ancestors
                .into_iter()
                .filter_map(|parent| parent.id.map(|id| (parent.kind, id))),
        );
    }
    for (kind, id) in names {
        if let KeyTargetKind::Object(kind) = kind {
            selected.insert((kind, id));
        }
    }
    objects(conn, Source::Content, selected).await
}

struct Content;

struct Subject;

impl reply::Projection for Content {
    type Source = Source;
    type Input = Saved;
    type Subject = Subject;
    type Value = ();
    type Reply = api::EnvironmentMoveBatchPlan;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Saved) -> &'static Declaration<Source> {
        &PREFLIGHT
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Saved,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = jobs::forks::preflight(conn, input.observed.plan.preflight_id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if row.kind != "move_batch" || row.node_id != Some(input.observed.node) {
                return Err(ApiError::not_found());
            }
            if !associations(conn, &input.observed).await? {
                return Err(ApiError::not_found());
            }
            let mut facts = required(conn, &input.observed).await?;
            facts.push(content(conn, &input.observed).await?);
            Ok((Subject, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Saved,
        _: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            ordinary(conn, principal, &input.observed).await?;
            for id in &input.observed.visible {
                super::super::reading::decide(conn, principal, *id).await?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Saved,
        _: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        input: &'a Saved,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<api::EnvironmentMoveBatchPlan>> {
        Box::pin(async move { Ok(input.observed.plan.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Saved,
        _: &'a reply::Prepared<Self>,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let under = orch_db::queries::environments::hosting::environments_under(
                conn,
                input.observed.node,
            )
            .await?;
            Ok(input.observed.visible.iter().all(|id| under.contains(id)))
        })
    }
}

pub(in crate::routes) async fn preflight(
    ctx: Ctx,
    (): (),
    _: api::NodeEnvironmentMovePreflightBody,
) -> ApiResult<api::NodeEnvironmentMovePreflightResponse> {
    let (mut tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Preview>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = api::NodeEnvironmentMovePreflightReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        node_id: done.output.observed.node,
        preflight_id: done.output.observed.plan.preflight_id,
        expires_at: done.output.observed.plan.expires_at,
        executable: done.output.observed.plan.executable,
    };
    publication::receipt(&done.ctx, &receipt)?;
    match reply::render::<Content>(&done.ctx, &done.accepted, done.output).await? {
        Some(full) => Ok(api::NodeEnvironmentMovePreflightResponse::Full(Box::new(
            full,
        ))),
        None => Ok(api::NodeEnvironmentMovePreflightResponse::Receipt(receipt)),
    }
}

#[path = "batch/confirmation.rs"]
pub mod confirmation;
