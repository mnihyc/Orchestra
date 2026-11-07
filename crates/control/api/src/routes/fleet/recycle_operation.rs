//! Native recycling keeps per-candidate probes, current stop decisions and accepted cleanup separate.

use super::*;
use crate::key_operations::native::effects::{Apply, Effect, FirstEffect, Outcome};
use crate::key_operations::{native, targets};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    common::{ErrorCode, ReceiptProjection},
    environment_sources::{RECYCLE, RecycleSource as Source},
    operation_sources::Declaration,
    operations::FacetUsage,
};
use orch_authz::Caller;
use orch_db::queries::environments::routes::{self as rows, EnvironmentFull};
use orch_engine::recycle::{Candidate, operation as source};
use std::collections::{BTreeMap, BTreeSet};

mod readback;

pub(super) struct Recycle;
#[derive(Clone)]
pub(super) struct Selected {
    node: Id,
    body: api::RecycleBody,
    ids: Vec<Id>,
    skipped_no_permission: i64,
}

async fn facts(conn: &mut Conn, role: Source, ids: &[Id]) -> ApiResult<native::Facts<Source>> {
    let mut targets = Vec::new();
    for id in ids {
        targets.push(
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
        role,
        if targets.is_empty() {
            Vec::new()
        } else {
            vec![(KeyTargetKind::Object(ObjectKind::Environment), targets)]
        },
    ))
}
async fn node_view(conn: &mut Conn, principal: &Principal, node: Id) -> ApiResult<()> {
    authorization::require(
        conn,
        principal,
        node,
        authorization::VIEW,
        orch_authz::Act::Use,
        orch_authz::Policy::default(),
    )
    .await
    .map_err(|error| {
        if native::modern(principal.caller()) {
            authorization::problem_without_content(error)
        } else {
            error
        }
    })
}
impl Effect for Recycle {
    type Source = Source;
    type Query = ();
    type Body = api::RecycleBody;
    type Value = Selected;
    type Output = Selected;
    fn declaration(_: &Selected) -> &'static Declaration<Source> {
        &RECYCLE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let node = ctx.id("id")?;
            let known = native::bind_known(ctx, &RECYCLE, &[])?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            node_view(conn, &principal, node).await?;
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR SHARE")
                    .bind(node)
                    .execute(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            }
            let under = envs::environments_under(conn, node).await?;
            let known = native::bind_known(ctx, &RECYCLE, &[])?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            node_view(conn, &principal, node).await?;
            let mut ids = Vec::new();
            let mut skipped_no_permission = 0;
            for id in under {
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
                    &RECYCLE,
                    principal.caller(),
                    &Source::Environments,
                    FacetUsage::Required,
                    KeyTargetKind::Object(ObjectKind::Environment),
                    &target,
                )? {
                    ids.push(id);
                } else {
                    skipped_no_permission += 1;
                }
            }
            let selected = Selected {
                node,
                body,
                ids,
                skipped_no_permission,
            };
            let facts = facts(conn, Source::Environments, &selected.ids).await?;
            Ok((selected, vec![facts]))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { node_view(conn, principal, value.node).await })
    }
}
impl Apply for Recycle {
    fn apply<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        value: &'a Selected,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Selected>>> {
        Box::pin(async move {
            // The entry only selects real work. Each later native probe/stop has its own current frame.
            before.deadline().check()?;
            Ok(Outcome::Unchanged(value.clone()))
        })
    }
}

#[derive(Clone)]
pub(super) struct Snapshot {
    row: EnvironmentFull,
    runtime: Option<Id>,
    host: Option<Id>,
    at: Time,
    threads: i64,
    active_turns: i64,
    open_shells: i64,
    accepted: Caller,
    until: Option<Time>,
}
impl Snapshot {
    async fn capture(
        conn: &mut Conn,
        candidate: &Candidate,
        principal: &Principal,
        until: Option<Time>,
    ) -> ApiResult<Self> {
        let id = candidate.state.id;
        let row = rows::environments(conn, &[id])
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        let threads = envs::thread_counts(conn, &[id])
            .await?
            .into_iter()
            .find(|(environment, _)| *environment == id)
            .map_or(0, |(_, count)| count);
        Ok(Self {
            row,
            runtime: candidate.runtime.as_ref().map(|runtime| runtime.id),
            host: candidate.runtime.as_ref().map(|runtime| runtime.host_id),
            at: principal.now(),
            threads,
            active_turns: envs::active_turns(conn, id).await?,
            open_shells: envs::open_shells(conn, id).await?,
            accepted: principal.caller().clone(),
            until,
        })
    }
    async fn current(&self, conn: &mut Conn) -> ApiResult<bool> {
        let Some(row) = rows::environments(conn, &[self.row.id]).await?.pop() else {
            return Ok(false);
        };
        let runtime = envs::live_runtime(conn, self.row.id).await?;
        Ok(row.id == self.row.id
            && row.owner_id == self.row.owner_id
            && row.home_node_id == self.row.home_node_id
            && row.org_id == self.row.org_id
            && row.name == self.row.name
            && row.placement == self.row.placement
            && row.machine_id == self.row.machine_id
            && row.template_id == self.row.template_id
            && row.workspace_root == self.row.workspace_root
            && row.config == self.row.config
            && row.generation == self.row.generation
            && runtime.as_ref().map(|runtime| runtime.id) == self.runtime
            && runtime.as_ref().map(|runtime| runtime.host_id) == self.host)
    }
}

pub(super) struct Recycled {
    selected: Selected,
    snapshots: Vec<Snapshot>,
    accepted: i64,
    stopped: i64,
    skipped_no_permission: i64,
    skipped_active: i64,
    remaining: i64,
}
impl Recycled {
    fn receipt(&self) -> api::RecycleReceipt {
        api::RecycleReceipt {
            projection: ReceiptProjection::Receipt,
            node_id: self.selected.node,
            dry_run: self.selected.body.dry_run,
            candidates: i64::try_from(self.snapshots.len()).unwrap_or(i64::MAX),
            accepted: self.accepted,
            stopped: self.stopped,
            skipped_no_permission: self.skipped_no_permission,
            skipped_active: self.skipped_active,
            remaining: self.remaining,
        }
    }
}
fn denied(error: &orch_engine::EngineError) -> bool {
    matches!(error, orch_engine::EngineError::BeforeEffect(error) if error.is::<source::Denied>())
}
fn source_error(error: orch_engine::EngineError) -> ApiError {
    match error {
        orch_engine::EngineError::BeforeEffect(error)
            if error.is::<source::Ended>() || error.is::<source::Denied>() =>
        {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the current request no longer permits recycling",
            )
        }
        error => ApiError::engine(error),
    }
}
fn live(
    caller: &Caller,
    until: Option<Time>,
    clock: &dyn orch_types::Clock,
) -> Result<(), orch_engine::EngineError> {
    if caller.key_deadline_allows(clock) && until.is_none_or(|end| clock.time() < end) {
        Ok(())
    } else {
        Err(orch_engine::EngineError::BeforeEffect(Box::new(
            source::Ended,
        )))
    }
}

pub(super) async fn recycle(
    ctx: Ctx,
    (): (),
    _: api::RecycleBody,
) -> ApiResult<api::RecycleResponse> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let done = native::effects::apply::<Recycle>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let (credential, until) = transaction_guard::native_origin(&done.ctx).await?;
    let selected = done.output;
    let mut output = Recycled {
        skipped_no_permission: selected.skipped_no_permission,
        selected,
        snapshots: Vec::new(),
        accepted: 0,
        stopped: 0,
        skipped_active: 0,
        remaining: 0,
    };
    let count = output.selected.ids.len();
    for position in 0..count {
        let id = output.selected.ids[position];
        let result = candidate(
            &done.ctx,
            &done.accepted,
            &credential,
            until,
            &output.selected,
            id,
        )
        .await;
        match result {
            Ok(CandidateOutcome::SkippedPermission) => output.skipped_no_permission += 1,
            Ok(CandidateOutcome::SkippedActive) => output.skipped_active += 1,
            Ok(CandidateOutcome::Dry(snapshot)) => output.snapshots.push(snapshot),
            Ok(CandidateOutcome::Accepted(snapshot, accepted)) => {
                output.accepted += 1;
                output.remaining = i64::try_from(count - position - 1).unwrap_or(i64::MAX);
                crate::key_operations::publication::receipt(&done.ctx, &output.receipt())?;
                match done
                    .ctx
                    .app
                    .engine
                    .finish_accepted_environment_stop(accepted)
                    .await
                {
                    Ok(()) => {
                        output.stopped += 1;
                        output.snapshots.push(snapshot);
                    }
                    Err(
                        orch_engine::environments::StopError::Busy(_)
                        | orch_engine::environments::StopError::Unavailable(_),
                    ) => output.skipped_active += 1,
                    Err(error) => {
                        return Err(ApiError::new(
                            ErrorCode::EnvironmentUnavailable,
                            error.to_string(),
                        ));
                    }
                }
            }
            Err(error) if output.accepted > 0 => {
                output.remaining = i64::try_from(count - position).unwrap_or(i64::MAX);
                if !matches!(error.problem.status, 401 | 403 | 404 | 409 | 410) {
                    return Err(error);
                }
                break;
            }
            Err(error) => return Err(error),
        }
        output.remaining = 0;
    }
    let receipt = output.receipt();
    crate::key_operations::publication::receipt(&done.ctx, &receipt)?;
    let mut reader = done.accepted;
    for snapshot in &output.snapshots {
        reader = reader
            .intersect_restrictions(&snapshot.accepted)
            .map_err(ApiError::internal)?;
        done.ctx.publication.retain_deadline(snapshot.until)?;
    }
    match native::reply::render::<readback::Reply>(&done.ctx, &reader, output).await? {
        Some(full) => Ok(api::RecycleResponse::Full(Box::new(full))),
        None => Ok(api::RecycleResponse::Receipt(receipt)),
    }
}

enum CandidateOutcome {
    SkippedPermission,
    SkippedActive,
    Dry(Snapshot),
    Accepted(Snapshot, orch_engine::environments::AcceptedStop),
}
async fn candidate(
    ctx: &Ctx,
    accepted: &Caller,
    credential: &orch_engine::operation_scope::request_origin::Credential,
    until: Option<Time>,
    selected: &Selected,
    id: Id,
) -> ApiResult<CandidateOutcome> {
    let native = {
        let mut conn = ctx.app.db.acquire().await?;
        let Some(state) = envs::environment(&mut conn, id)
            .await?
            .filter(|state| state.deleted_at.is_none())
        else {
            return Ok(CandidateOutcome::SkippedActive);
        };
        Candidate {
            state,
            runtime: envs::live_runtime(&mut conn, id).await?,
        }
    };
    let mut input = source::Input::capture(
        accepted,
        credential.clone(),
        until,
        selected.node,
        selected.body.idle_minutes,
        &native,
    )
    .map_err(source_error)?;
    let observed = {
        let mut tx = ctx.app.db.begin().await?;
        let check = match input.checkpoint(&mut tx, ctx.app.clock.as_ref()).await {
            Ok(check) => check,
            Err(error) if denied(&error) => return Ok(CandidateOutcome::SkippedPermission),
            Err(error) => return Err(source_error(error)),
        };
        if !check.matches() {
            return Ok(CandidateOutcome::SkippedActive);
        }
        let native = check
            .frame
            .value()
            .candidate
            .clone()
            .ok_or_else(ApiError::not_found)?;
        check.check(ctx.app.clock.as_ref()).map_err(source_error)?;
        input = check.accepted_input().map_err(source_error)?;
        tx.commit().await.map_err(DbError::from)?;
        native
    };
    match ctx
        .app
        .engine
        .probe_recycle_candidate(
            &observed,
            selected.body.include_with_processes.unwrap_or(false),
            Some(&input),
        )
        .await
    {
        Ok(observation) if observation.ready() => {
            input = observation.into_source().ok_or_else(|| {
                ApiError::internal("the native process observation lost its original source")
            })?;
        }
        Ok(_) => return Ok(CandidateOutcome::SkippedActive),
        Err(error) if denied(&error) => return Ok(CandidateOutcome::SkippedPermission),
        Err(error) => return Err(source_error(error)),
    }
    let mut tx = ctx.app.db.begin().await?;
    let check = match input.checkpoint(&mut tx, ctx.app.clock.as_ref()).await {
        Ok(check) => check,
        Err(error) if denied(&error) => return Ok(CandidateOutcome::SkippedPermission),
        Err(error) => return Err(source_error(error)),
    };
    if !check.matches() {
        return Ok(CandidateOutcome::SkippedActive);
    }
    let native = check
        .frame
        .value()
        .candidate
        .as_ref()
        .ok_or_else(ApiError::not_found)?;
    let snapshot = Snapshot::capture(&mut tx, native, check.frame.principal(), check.until).await?;
    check.check(ctx.app.clock.as_ref()).map_err(source_error)?;
    if selected.body.dry_run {
        tx.commit().await.map_err(DbError::from)?;
        return Ok(CandidateOutcome::Dry(snapshot));
    }
    let until = check.until;
    let parts = check.frame.into_parts();
    let caller = parts.principal.caller().clone();
    let clock = ctx.app.clock.clone();
    let after_clock = clock.clone();
    let first = parts.first_effect;
    let request = orch_engine::environments::StopRequest {
        environment: id,
        interrupt: false,
        reason: orch_engine::environments::StopReason::Recycle(parts.principal.user()),
    };
    let accepted = ctx
        .app
        .engine
        .accept_environment_stop_in(
            tx,
            &parts.principal,
            &crate::sign_in::origin(ctx),
            request,
            move || {
                first
                    .check(clock.as_ref())
                    .map_err(|_| orch_engine::EngineError::BeforeEffect(Box::new(source::Ended)))?;
                live(&caller, until, clock.as_ref())
            },
            {
                let caller = parts.principal.caller().clone();
                move || live(&caller, until, after_clock.as_ref())
            },
        )
        .await;
    match accepted {
        Ok(accepted) => Ok(CandidateOutcome::Accepted(snapshot, accepted)),
        Err(
            orch_engine::environments::StopError::Busy(_)
            | orch_engine::environments::StopError::Unavailable(_),
        ) => Ok(CandidateOutcome::SkippedActive),
        Err(orch_engine::environments::StopError::Internal(error)) => Err(source_error(error)),
        Err(error) => Err(ApiError::new(
            ErrorCode::EnvironmentUnavailable,
            error.to_string(),
        )),
    }
}
