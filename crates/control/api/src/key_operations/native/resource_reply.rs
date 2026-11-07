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
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<Facts<Self::Source>>)>>;
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

struct Opened<F: ResourceProjection> {
    object: ObjectRef,
    primary: (KeyTargetKind, KeyObjectRef),
    bounds: single::Bounds,
    validation: single::Validation,
    value: F::Value,
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
    let input = Arc::new(input);
    let producing_input = Arc::clone(&input);
    let initial = completed(ctx, accepted, move |ctx, conn, current| {
        Box::pin(async move {
            let declaration = F::declaration(ctx, &producing_input)?;
            let (value, facts) = F::load(ctx, conn, &producing_input).await?;
            let object = F::object(&value);
            let primary = primary(&facts, &F::READ_SOURCE)?;
            let mut plan = bind_required(ctx, declaration, &facts)?;
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
            let snapshot = if modern(reader.caller()) {
                plan.read_response(ctx, conn, reader.caller(), move |conn, reader| {
                    Box::pin(async move { prepared.render(&projection, conn, reader).await })
                })
                .await?
            } else {
                Some(prepared.render(ctx, conn, &reader).await?)
            };
            Ok(snapshot.map(|snapshot| (snapshot, bounds, primary, value)))
        })
    })
    .await?;
    let Some((snapshot, bounds, primary, value)) = initial else {
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
    });
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
    let declaration = F::declaration(ctx, input)?;
    let (value, facts) = F::load(ctx, conn, input).await?;
    if F::object(&value) != opened.object || primary(&facts, &F::READ_SOURCE)? != opened.primary {
        return Ok(false);
    }
    let mut plan = bind_required(ctx, declaration, &facts)?;
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
        return Ok(false);
    };
    F::require_read(ctx, conn, &reader, &value).await?;
    if !F::check_opened(ctx, conn, &reader, &opened.value, &value).await? {
        return Ok(false);
    }
    if !opened
        .bounds
        .refresh(conn, reader.caller(), &mut plan)
        .await?
    {
        return Ok(false);
    }
    if modern(reader.caller()) {
        let validation = opened.validation.clone();
        let validation_ctx = ctx.clone();
        Ok(plan
            .read_response(ctx, conn, reader.caller(), move |conn, reader| {
                Box::pin(async move {
                    validation
                        .current(
                            &validation_ctx,
                            conn,
                            reader,
                            validation_ctx.app.clock.time(),
                        )
                        .await
                })
            })
            .await?
            .unwrap_or(false))
    } else {
        opened
            .validation
            .current(ctx, conn, &reader, ctx.app.clock.time())
            .await
    }
}

#[cfg(test)]
#[path = "resource_reply/tests.rs"]
mod tests;
