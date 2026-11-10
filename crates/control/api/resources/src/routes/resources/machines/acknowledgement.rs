//! Native individual acknowledgement sources shared by remote machine mutations.

use super::MachineRow;

use super::individual;

use super::live_machine;

use super::require_machine;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::machine_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::resources::Machine;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_types::Id;

use std::marker::PhantomData;

use std::sync::Arc;
pub(super) trait Body: Send + Sync + 'static {
    type Reply: Send + 'static;
    const DECLARATION: &'static Declaration<Source>;
    const WITH_MACHINE: bool = true;
    fn require_read<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a MachineRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_machine(
            conn,
            principal,
            row.id,
            Permission::MachineView,
        ))
    }
    /// Native command refusals contain intrinsic physical-machine state only.
    /// This phase selects the two existing governor bindings from the same source row.
    const REFUSAL: Declaration<Source> = Declaration {
        operation: Self::DECLARATION.operation,
        bindings: &[Self::DECLARATION.bindings[0], Self::DECLARATION.bindings[1]],
    };
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a MachineRow,
        machine: Option<Machine>,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>>;
}

struct Projection<F>(PhantomData<F>);

impl<F: Body> native::reply::Projection for Projection<F> {
    type Source = Source;
    type Input = Id;
    type Subject = MachineRow;
    type Value = Option<Arc<individual::Opened>>;
    type Reply = F::Reply;
    const READ_SOURCE: Source = Source::Governor;

    fn declaration(_input: &Id) -> &'static Declaration<Source> {
        F::DECLARATION
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        id: &'a Id,
    ) -> BoxFuture<'a, ApiResult<(MachineRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = live_machine(conn, *id).await?;
            let target = targets::machine_subject(conn, row.id).await?;
            Ok((
                row,
                vec![native::Facts::one(
                    Source::Governor,
                    KeyTargetKind::Object(ObjectKind::Machine),
                    target,
                )],
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _id: &'a Id,
        subject: &'a MachineRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        F::require_read(ctx, conn, principal, subject)
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: &'a Id,
        subject: &'a MachineRow,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if !F::WITH_MACHINE {
                // This native shape returns intrinsic policy only and selects no individual
                // Machine body. Its registration/readiness/access-name collections are empty.
                let mut facts: Vec<native::Facts<Source>> = Vec::new();
                for binding in F::DECLARATION.bindings {
                    if binding.source == Source::Governor {
                        continue;
                    }
                    if binding.usage != orch_api_contract::operations::FacetUsage::ResponseContent
                        || binding.cardinality
                            != orch_api_contract::operation_sources::Cardinality::Collection
                        || !matches!(
                            binding.source,
                            Source::Registered
                                | Source::ReadinessMachines
                                | Source::Groups
                                | Source::Orgs
                                | Source::Projects
                                | Source::Topics
                                | Source::Threads
                        )
                    {
                        return Err(ApiError::internal(
                            "the intrinsic Machine policy has another source kind",
                        ));
                    }
                    if !facts.iter().any(|fact| fact.source == binding.source) {
                        facts.push(native::Facts::collection(binding.source, Vec::new()));
                    }
                }
                return Ok((None, facts));
            }
            if subject.id != *id {
                return Err(ApiError::internal(
                    "the individual response has another native subject",
                ));
            }
            let (value, mut facts) =
                individual::prepare_response(ctx, conn, principal, subject.clone(), F::DECLARATION)
                    .await?;
            facts.retain(|fact| fact.source != Source::Governor);
            Ok((Some(value), facts))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _id: &'a Id,
        prepared: &'a native::reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<F::Reply>> {
        Box::pin(async move {
            let machine = match &prepared.value {
                Some(value) => Some(value.render(ctx, conn, principal).await?),
                None => None,
            };
            F::render(ctx, conn, principal, &prepared.subject, machine).await
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _id: &'a Id,
        opened: &'a native::reply::Prepared<Self>,
        fresh: &'a native::reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.subject.id == fresh.subject.id
                && opened.subject.owner_id == fresh.subject.owner_id
                && opened.subject.host_id == fresh.subject.host_id
                && opened.subject.runtime_id == fresh.subject.runtime_id
                && match (&opened.value, &fresh.value) {
                    (Some(old), Some(fresh)) => old.preserved_by(fresh),
                    (None, None) => true,
                    _ => false,
                })
        })
    }
}

pub(super) async fn project<F: Body>(
    ctx: &Ctx,
    accepted: &Caller,
    id: Id,
) -> ApiResult<Option<F::Reply>> {
    native::reply::render::<Projection<F>>(ctx, accepted, id).await
}

/// One current raw/all-key cut after native command and persistence waits.
pub(super) fn deadline(
    ctx: &Ctx,
    caller: &Caller,
    until: Option<orch_types::Time>,
) -> ApiResult<()> {
    if !caller.key_deadline_allows(ctx.app.clock.as_ref())
        || until.is_some_and(|end| end.get() <= ctx.app.clock.now())
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "the original machine operation has ended",
        ));
    }
    Ok(())
}

struct Refused {
    id: Id,
    error: ApiError,
}

struct ErrorProjection<F>(PhantomData<F>);

impl<F: Body> native::reply::Projection for ErrorProjection<F> {
    type Source = Source;
    type Input = Refused;
    type Subject = MachineRow;
    type Value = ();
    type Reply = ApiError;
    const READ_SOURCE: Source = Source::Governor;
    fn declaration(_input: &Refused) -> &'static Declaration<Source> {
        &F::REFUSAL
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Refused,
    ) -> BoxFuture<'a, ApiResult<(MachineRow, Vec<native::Facts<Source>>)>> {
        <Projection<F> as native::reply::Projection>::subject(ctx, conn, &input.id)
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Refused,
        subject: &'a MachineRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        <Projection<F> as native::reply::Projection>::require(
            ctx, conn, principal, &input.id, subject,
        )
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Refused,
        _subject: &'a MachineRow,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Refused,
        _prepared: &'a native::reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ApiError>> {
        Box::pin(async move {
            Ok(input
                .error
                .clone()
                .with("diagnostic", input.error.problem.detail.clone()))
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Refused,
        old: &'a native::reply::Prepared<Self>,
        fresh: &'a native::reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.id == fresh.subject.id
                && old.subject.owner_id == fresh.subject.owner_id
                && old.subject.host_id == fresh.subject.host_id
                && old.subject.runtime_id == fresh.subject.runtime_id)
        })
    }
}

/// Native refusal text concerns this physical machine alone; success-only sources stay absent.
pub(super) async fn refusal<F: Body>(ctx: &Ctx, error: ApiError) -> ApiResult<ApiError> {
    let caller = crate::middleware::authorization::caller_of(ctx.user()?)?;
    if !native::modern(&caller) {
        return Ok(error);
    }
    let Some(accepted) = ctx.admitted_caller() else {
        return Ok(crate::middleware::authorization::problem_without_content(
            error,
        ));
    };
    let sparse = crate::middleware::authorization::problem_without_content(error.clone());
    native::refusal::project::<ErrorProjection<F>>(
        ctx,
        accepted,
        Refused {
            id: ctx.id("id")?,
            error,
        },
        sparse,
    )
    .await
}
