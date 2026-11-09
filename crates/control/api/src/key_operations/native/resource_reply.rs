//! Optional native singleton content after a stored or creation writer has completed.

use super::*;
use crate::routes::resources::projection::single;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyObjectRef;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::Resource;

/// The stored factory owns its exact subject and ordinary response gate. The existing
/// singleton factory owns native styles, foreign sources, names and metadata cuts.
pub(crate) trait StoredResource: Stored {
    const READ_SOURCE: Self::Source;
    const ROLES: &'static single::Roles;

    fn object(value: &Self::Value) -> ObjectRef;

    fn require_read<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// Once-only native auxiliary work after the successful read snapshot. Native locks
    /// and its first SQL must use this response's current Read checkpoint, not its writer.
    fn after_read<'a>(
        _ctx: &'a Ctx,
        _value: &'a Self::Value,
        _read: &'a ResponseRead,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }

    /// Preserve the producing snapshot's actual native associations and body authority.
    /// A different current registration at the same governor cannot validate an older body.
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _opened: &'a Self::Value,
        _fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

/// A created native singleton may use an immutable accepted output rather than
/// the request path to select its stored subject. This input is never a marker,
/// a fabricated path, or an instruction to repeat the creating writer.
pub(crate) trait ResourceProjection: Send + Sync + 'static {
    type Source: Eq + Send + Sync + 'static;
    type Value: Send + Sync + 'static;
    type Input: Send + Sync + 'static;
    const READ_SOURCE: Self::Source;
    const ROLES: &'static single::Roles;

    fn declaration(ctx: &Ctx, input: &Self::Input)
    -> ApiResult<&'static Declaration<Self::Source>>;
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<PreparedSources<Self::Value, Self::Source>>>;
    fn object(value: &Self::Value) -> ObjectRef;
    fn require_read<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>>;
    fn check_opened<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        opened: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>>;
    /// The same successful native response auxiliary hook for identity-input factories.
    fn after_read<'a>(
        _ctx: &'a Ctx,
        _value: &'a Self::Value,
        _read: &'a ResponseRead,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
}

struct StoredProjection<F: StoredResource>(std::marker::PhantomData<F>);
impl<F: StoredResource> ResourceProjection for StoredProjection<F> {
    type Source = F::Source;
    type Value = F::Value;
    type Input = ();
    const READ_SOURCE: F::Source = F::READ_SOURCE;
    const ROLES: &'static single::Roles = F::ROLES;
    fn declaration(ctx: &Ctx, _input: &()) -> ApiResult<&'static Declaration<F::Source>> {
        declaration::<F>(ctx)
    }
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _input: &'a (),
    ) -> BoxFuture<'a, ApiResult<(F::Value, Vec<Facts<F::Source>>)>> {
        Box::pin(async move { F::load(ctx, conn, declaration::<F>(ctx)?).await })
    }
    fn object(value: &F::Value) -> ObjectRef {
        F::object(value)
    }
    fn after_read<'a>(
        ctx: &'a Ctx,
        value: &'a F::Value,
        read: &'a ResponseRead,
    ) -> BoxFuture<'a, ApiResult<()>> {
        F::after_read(ctx, value, read)
    }

    fn require_read<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a F::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        F::require_read(ctx, conn, principal, value)
    }
    fn check_opened<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        opened: &'a F::Value,
        fresh: &'a F::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        F::check_opened(ctx, conn, principal, opened, fresh)
    }
}

type ReadCheckpoint =
    dyn for<'a> Fn(&'a Ctx, &'a mut Conn) -> BoxFuture<'a, ApiResult<ReadGuard>> + Send + Sync;
/// Actual successful singleton source; no path override, operation alias or writer entry.
pub(crate) struct ResponseRead {
    ctx: Ctx,
    object: ObjectRef,
    checkpoint: Arc<ReadCheckpoint>,
}
/// Current native response Read on the owner's still-open transaction.
/// Its deadline check grants no business effect or native command authority.
pub(crate) struct ReadGuard {
    ctx: Ctx,
    identity: CurrentHttpIdentity,
    principal: Principal,
}
impl ResponseRead {
    pub(crate) fn object(&self) -> ObjectRef {
        self.object
    }
    /// Invoke after native maintenance locks on this same transaction; no full effect resolver.
    pub(crate) async fn prepare_on(&self, conn: &mut Conn) -> ApiResult<ReadGuard> {
        if !orch_db::queries::transaction_guard::in_transaction(conn) {
            return Err(declaration_error());
        }
        (self.checkpoint)(&self.ctx, conn).await
    }
}
impl ReadGuard {
    pub(crate) fn principal(&self) -> &Principal {
        &self.principal
    }
    /// Use at the native first SQL and again after audit/commit preparation waits.
    pub(crate) fn check(&self) -> ApiResult<()> {
        self.identity.check_deadline(&self.ctx.app)?;
        if !self
            .principal
            .caller()
            .key_deadline_allows(self.ctx.app.clock.as_ref())
            || self
                .ctx
                .publication
                .retained_deadline()?
                .is_some_and(|end| self.ctx.app.clock.time() >= end)
        {
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "the native response Read has ended",
            ));
        }
        Ok(())
    }
}

struct Opened<F: ResourceProjection> {
    object: ObjectRef,
    primary: (KeyTargetKind, KeyObjectRef),
    bounds: single::Bounds,
    validation: single::Validation,
    value: F::Value,
    purpose: Purpose,
    operation: String,
}

#[derive(Clone, Copy)]
enum Purpose {
    Completed,
    Replay,
}
impl Purpose {
    fn bind<S: Eq + 'static>(
        self,
        ctx: &Ctx,
        declaration: &Declaration<S>,
        facts: &[Facts<S>],
    ) -> ApiResult<OperationPlan> {
        match self {
            Self::Completed => bind_required(ctx, declaration, facts),
            Self::Replay => bind_known(ctx, declaration, facts),
        }
    }
    fn projection(self, plan: &OperationPlan) -> ApiResult<admission::ResponseProjection> {
        Ok(match self {
            Self::Completed => admission::ResponseProjection::CompletedContent,
            Self::Replay => {
                let indices = plan
                    .variant()
                    .facets
                    .iter()
                    .enumerate()
                    .filter(|(_, facet)| facet.usage == FacetUsage::ResponseContent)
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                plan.shared()
                    .response_facets_for(&indices)
                    .map_err(admission::binding_error)?;
                admission::ResponseProjection::Replay(indices)
            }
        })
    }
}

fn declaration<F: StoredResource>(ctx: &Ctx) -> ApiResult<&'static Declaration<F::Source>> {
    F::ROWS
        .iter()
        .find(|row| row.operation == ctx.route.name)
        .ok_or_else(declaration_error)
}

pub(super) fn primary<S: Eq>(
    facts: &[Facts<S>],
    source: &S,
) -> ApiResult<(KeyTargetKind, KeyObjectRef)> {
    let mut found = facts.iter().filter(|fact| &fact.source == source);
    let fact = found.next().ok_or_else(declaration_error)?;
    let [(family, targets)] = fact.groups.as_slice() else {
        return Err(declaration_error());
    };
    let [target] = targets.as_slice() else {
        return Err(declaration_error());
    };
    if fact.cardinality != Cardinality::One || found.next().is_some() {
        return Err(declaration_error());
    }
    Ok((*family, target.object))
}

/// Only known same-operation response Read is admitted before any optional materialization.
/// Completed Write/Manage authority is deliberately not re-admitted by this read boundary.
pub(super) async fn reader<S: Eq + Sync + 'static>(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    plan: &OperationPlan,
    declaration: &Declaration<S>,
    source: &S,
    facts: &[Facts<S>],
) -> ApiResult<Option<Principal>> {
    if !modern(current.caller()) {
        return Ok(Some(current.clone()));
    }
    let Some(caller) = read_caller(current.caller(), plan, declaration, source, facts)? else {
        return Ok(None);
    };
    let principal = authorization::load_principal(conn, caller, ctx.app.clock.time()).await?;
    Ok(Some(
        crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await?,
    ))
}

fn read_caller<S: Eq + 'static>(
    current: &Caller,
    plan: &OperationPlan,
    declaration: &Declaration<S>,
    source: &S,
    facts: &[Facts<S>],
) -> ApiResult<Option<Caller>> {
    plan.shared()
        .compatible(current)
        .map_err(admission::binding_error)?;
    let mut found = facts.iter().filter(|fact| &fact.source == source);
    let fact = found.next().ok_or_else(declaration_error)?;
    if found.next().is_some() {
        return Err(declaration_error());
    }
    let mut roles = 0;
    let mut selected = Vec::new();
    for binding in declaration
        .bindings
        .iter()
        .filter(|binding| &binding.source == source && binding.usage == FacetUsage::ResponseContent)
    {
        for index in indices(plan, binding)? {
            roles += 1;
            if plan.variant().facets[index].action
                != orch_access_contract::key_scope::KeyAction::Read
            {
                return Err(declaration_error());
            }
            for (family, targets) in &fact.groups {
                if !plan.permits_binding(current, index, *family, targets)? {
                    return Ok(None);
                }
                selected.extend(
                    orch_engine::operation_scope::facets(
                        &plan.variant().operation,
                        &plan.variant().facets[index],
                        *family,
                        targets,
                    )
                    .map_err(admission::binding_error)?,
                );
            }
        }
    }
    if roles == 0 {
        return Err(declaration_error());
    }
    if selected.is_empty() {
        if fact.cardinality != Cardinality::Collection || !fact.groups.is_empty() {
            return Err(declaration_error());
        }
        // No actual content is read. Keep the same unadmitted restriction, rather
        // than manufacture a target or re-admit a completed Write/Manage facet.
        return Ok(Some(current.clone()));
    }
    let caller = current
        .admit_facets(&selected)
        .map_err(|_| ApiError::new(ErrorCode::AuthzDenied, "the response Read is closed"))?
        .into_caller();
    Ok(Some(caller))
}

/// Shared completed readback. The domain still creates its actual typed receipt and
/// registers that fallback after its native effect; this function never repeats a writer.
pub(crate) async fn stored<F: StoredResource>(
    ctx: &Ctx,
    accepted: &Caller,
) -> ApiResult<Option<Resource>> {
    with_input::<StoredProjection<F>>(ctx, accepted, ()).await
}

/// The same native singleton boundary for an actual immutable creation outcome.
/// The domain retains its truthful receipt after commit; closed Read returns None.
pub(crate) async fn with_input<F: ResourceProjection>(
    ctx: &Ctx,
    accepted: &Caller,
    input: F::Input,
) -> ApiResult<Option<Resource>> {
    project::<F>(ctx, accepted, input, Purpose::Completed).await
}

/// Project the completed record's actual singleton under the new request credential.
pub(crate) async fn stored_completed_replay<F: StoredResource>(
    ctx: &Ctx,
    completed_account: Id,
) -> ApiResult<Option<Resource>> {
    with_input_completed_replay::<StoredProjection<F>>(ctx, completed_account, ()).await
}

/// Existing styles, metadata cuts and ordinary Read; no first credential or spent effect proof.
pub(crate) async fn with_input_completed_replay<F: ResourceProjection>(
    ctx: &Ctx,
    completed_account: Id,
    input: F::Input,
) -> ApiResult<Option<Resource>> {
    if ctx.authenticated_user()?.id() != completed_account {
        return Err(ApiError::not_found());
    }
    let current = authorization::caller_of(ctx.user()?)?;
    let projection = ctx.for_completed_replay_projection();
    project::<F>(&projection, &current, input, Purpose::Replay).await
}

async fn project<F: ResourceProjection>(
    ctx: &Ctx,
    accepted: &Caller,
    input: F::Input,
    purpose: Purpose,
) -> ApiResult<Option<Resource>> {
    let input = Arc::new(input);
    let producing_input = Arc::clone(&input);
    let initial = completed(ctx, accepted, move |ctx, conn, current| {
        Box::pin(async move {
            let declaration = F::declaration(ctx, &producing_input)?;
            let (value, facts) = F::load(ctx, conn, &producing_input).await?;
            let object = F::object(&value);
            let primary = primary(&facts, &F::READ_SOURCE)?;
            let mut plan = purpose.bind(ctx, declaration, &facts)?;
            let Some(reader) = reader(
                ctx,
                conn,
                current,
                &plan,
                declaration,
                &F::READ_SOURCE,
                &facts,
            )
            .await?
            else {
                return Ok(None);
            };
            F::require_read(ctx, conn, &reader, &value).await?;
            let mut prepared = single::prepare(ctx, conn, &reader, object).await?;
            let bounds = prepared
                .bind(conn, reader.caller(), &mut plan, F::ROLES)
                .await?;
            let projection = ctx.clone();
            let operation = plan.variant().operation.clone();
            let snapshot = if modern(reader.caller()) {
                let shape = purpose.projection(&plan)?;
                plan.read_response_projection(
                    ctx,
                    conn,
                    reader.caller(),
                    shape,
                    move |conn, reader| {
                        Box::pin(async move { prepared.render(&projection, conn, reader).await })
                    },
                )
                .await?
            } else {
                Some(prepared.render(ctx, conn, &reader).await?)
            };
            Ok(snapshot.map(|snapshot| (snapshot, bounds, primary, value, operation)))
        })
    })
    .await?;
    let Some((snapshot, bounds, primary, value, operation)) = initial else {
        return Ok(None);
    };
    // This native hook runs once, after the completed snapshot. Validation never repeats it.
    snapshot.after_snapshot().await;
    let opened = Arc::new(Opened::<F> {
        object: snapshot.object(),
        primary,
        bounds,
        validation: snapshot.validation(),
        value,
        purpose,
        operation,
    });
    let source = Arc::clone(&opened);
    let source_input = Arc::clone(&input);
    let original = orch_authz::CapturedCaller::capture(accepted);
    let checkpoint: Arc<ReadCheckpoint> = Arc::new(move |ctx, conn| {
        let source = Arc::clone(&source);
        let input = Arc::clone(&source_input);
        let original = original.clone();
        Box::pin(async move {
            // Native maintenance owns its locks before this action-identity cut. No
            // registered transfer/creation operation is resolved or applied again.
            let mut identity =
                transaction_guard::current_http_identity(ctx, conn, HttpIdentityMode::Action)
                    .await?;
            transaction_guard::guard_user(&identity.user)?;
            let accepted = original
                .restore_current(conn, ctx.app.clock.as_ref())
                .await
                .map_err(authorization::restoration_error)?
                .ok_or_else(|| {
                    ApiError::new(ErrorCode::AuthzDenied, "the native response origin ended")
                })?;
            if !admission::same_response_origin(ctx, &accepted)? {
                return Err(declaration_error());
            }
            let caller = identity
                .caller
                .intersect_restrictions(&accepted)
                .map_err(ApiError::internal)?;
            let current = ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
            let principal =
                authorization::load_principal(conn, caller, ctx.app.clock.time()).await?;
            let principal =
                crate::sign_in::second_step::hold(conn, &identity.user, principal).await?;
            let principal = validated_reader::<F>(&current, conn, &principal, &input, &source)
                .await?
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::AuthzDenied,
                        "the native response source is closed",
                    )
                })?;
            identity.caller = principal.caller().clone();
            current.publication.retain_ceiling(principal.caller())?;
            let guard = ReadGuard {
                ctx: current,
                identity,
                principal,
            };
            guard.check()?;
            Ok(guard)
        })
    });
    let read = ResponseRead {
        ctx: ctx.clone(),
        object: opened.object,
        checkpoint,
    };
    match F::after_read(ctx, &opened.value, &read).await {
        Err(error)
            if error.is_database_retry()
                || matches!(error.problem.status, 401 | 403 | 404 | 410) =>
        {
            ctx.complete_disclosure(resolver::Disclosure::Denied);
            return Ok(None);
        }
        Err(error) => return Err(error),
        Ok(()) => {}
    }
    let validation = Arc::clone(&opened);
    let validation_input = Arc::clone(&input);
    let valid = completed(ctx, accepted, move |ctx, conn, current| {
        Box::pin(async move {
            Ok(
                validate::<F>(ctx, conn, current, &validation_input, &validation)
                    .await?
                    .then_some(()),
            )
        })
    })
    .await?;
    if valid.is_none() {
        ctx.complete_disclosure(resolver::Disclosure::Denied);
        return Ok(None);
    }
    crate::key_operations::publication::retain(
        ctx,
        accepted,
        (input, opened),
        |ctx, conn, current, (input, opened)| {
            Box::pin(async move {
                if validate::<F>(ctx, conn, current, input, opened).await? {
                    Ok(())
                } else {
                    Err(ApiError::new(
                        ErrorCode::AuthzDenied,
                        "the native response source is closed",
                    ))
                }
            })
        },
    )?;
    Ok(Some(snapshot.value))
}

async fn validate<F: ResourceProjection>(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    input: &F::Input,
    opened: &Opened<F>,
) -> ApiResult<bool> {
    Ok(validated_reader::<F>(ctx, conn, current, input, opened)
        .await?
        .is_some())
}

async fn validated_reader<F: ResourceProjection>(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    input: &F::Input,
    opened: &Opened<F>,
) -> ApiResult<Option<Principal>> {
    let declaration = F::declaration(ctx, input)?;
    if declaration.operation != opened.operation {
        return Ok(None);
    }
    let (value, facts) = F::load(ctx, conn, input).await?;
    if F::object(&value) != opened.object || primary(&facts, &F::READ_SOURCE)? != opened.primary {
        return Ok(None);
    }
    let mut plan = opened.purpose.bind(ctx, declaration, &facts)?;
    let Some(reader) = reader(
        ctx,
        conn,
        current,
        &plan,
        declaration,
        &F::READ_SOURCE,
        &facts,
    )
    .await?
    else {
        return Ok(None);
    };
    F::require_read(ctx, conn, &reader, &value).await?;
    if !F::check_opened(ctx, conn, &reader, &opened.value, &value).await? {
        return Ok(None);
    }
    if !opened
        .bounds
        .refresh(conn, reader.caller(), &mut plan)
        .await?
    {
        return Ok(None);
    }
    if modern(reader.caller()) {
        let validation = opened.validation.clone();
        let validation_ctx = ctx.clone();
        let shape = opened.purpose.projection(&plan)?;
        Ok(plan
            .read_response_projection(ctx, conn, reader.caller(), shape, move |conn, reader| {
                Box::pin(async move {
                    Ok(validation
                        .current(
                            &validation_ctx,
                            conn,
                            reader,
                            validation_ctx.app.clock.time(),
                        )
                        .await?
                        .then(|| reader.clone()))
                })
            })
            .await?
            .flatten())
    } else {
        Ok(opened
            .validation
            .current(ctx, conn, &reader, ctx.app.clock.time())
            .await?
            .then_some(reader))
    }
}

#[cfg(test)]
#[path = "resource_reply/tests.rs"]
mod tests;
