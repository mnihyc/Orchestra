//! Typed native projections after an accepted effect, including jobs and genuine no-job results.

use super::*;
use orch_access_contract::key_scope::KeyObjectRef;
use orch_api_contract::common::ErrorCode;

/// Native response sources and projection, independent of a writer or its preflight.
/// Subject lookup and preparation must be effect-free. Rendering runs only after
/// same-operation Read admission; validation never repeats rendering or business work.
pub(crate) trait Projection: Sized + Send + Sync + 'static {
    type Source: Eq + Send + Sync + 'static;
    type Input: Send + Sync + 'static;
    type Subject: Send + Sync + 'static;
    type Value: Send + Sync + 'static;
    type Reply: Send + 'static;

    const READ_SOURCE: Self::Source;

    fn declaration(input: &Self::Input) -> &'static Declaration<Self::Source>;

    /// Resolve the actual stored response subjects and known mandatory/root facts.
    /// Input is the immutable native outcome, never a replayable request body.
    /// READ_SOURCE may be a singleton or a genuinely selected, possibly empty collection.
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<Facts<Self::Source>>)>>;

    /// Keep the native ordinary response gate; completed Write/Manage is not re-admitted.
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        subject: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// Prepare remaining real response sources after the primary Read and native gate.
    /// These are additional roles; an already supplied role cannot be overwritten.
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        subject: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<Facts<Self::Source>>)>>;

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        prepared: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>>;

    /// Preserve actual native association and disclosure gates of the producing snapshot.
    /// This hook checks retained values, not a newly rendered replacement response.
    fn check_opened<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        opened: &'a Prepared<Self>,
        fresh: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>>;
}

pub(crate) struct Prepared<F: Projection> {
    pub subject: F::Subject,
    pub value: F::Value,
}

struct Opened<F: Projection> {
    prepared: Prepared<F>,
    response: OperationPlan,
    primary: ReadSources,
    accepted: Caller,
    refusal: Option<Vec<usize>>,
}

#[derive(Debug, PartialEq, Eq)]
struct ReadSources {
    cardinality: Cardinality,
    identities: BTreeSet<(KeyTargetKind, KeyObjectRef)>,
}

fn read_sources<S: Eq>(facts: &[Facts<S>], source: &S) -> ApiResult<ReadSources> {
    let mut found = facts.iter().filter(|fact| &fact.source == source);
    let fact = found.next().ok_or_else(declaration_error)?;
    if found.next().is_some() || fact.groups.iter().any(|(_, targets)| targets.is_empty()) {
        return Err(declaration_error());
    }
    if fact.cardinality == Cardinality::One {
        resource_reply::primary(facts, source)?;
    }
    Ok(ReadSources {
        cardinality: fact.cardinality,
        identities: fact
            .groups
            .iter()
            .flat_map(|(family, targets)| targets.iter().map(|target| (*family, target.object)))
            .collect(),
    })
}

async fn prepare<F: Projection>(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    input: &F::Input,
    accepted: &Caller,
    refusal: bool,
) -> ApiResult<Option<(Opened<F>, Principal)>> {
    let declaration = F::declaration(input);
    let (subject, mut facts) = F::subject(ctx, conn, input).await?;
    let primary = read_sources(&facts, &F::READ_SOURCE)?;
    if refusal && primary.identities.is_empty() {
        return Ok(None);
    }
    let known = if refusal {
        bind_known(ctx, declaration, &facts)?
    } else {
        bind_required(ctx, declaration, &facts)?
    };
    let Some(reader) = resource_reply::reader(
        ctx,
        conn,
        current,
        &known,
        declaration,
        &F::READ_SOURCE,
        &facts,
    )
    .await?
    else {
        return Ok(None);
    };
    F::require(ctx, conn, &reader, input, &subject).await?;
    let (value, additional) = F::prepare(ctx, conn, &reader, input, &subject).await?;
    facts.extend(additional);
    let response = if refusal {
        orch_engine::operation_scope::sources::bind_declared(ctx.route, declaration, &facts)
            .map(OperationPlan::from_shared)
            .map_err(admission::binding_error)?
    } else {
        bind(ctx, declaration, &facts)?
    };
    let active = if refusal {
        let mut active = BTreeSet::new();
        for binding in declaration
            .bindings
            .iter()
            .filter(|binding| binding.usage == FacetUsage::ResponseContent)
        {
            active.extend(indices(&response, binding)?);
        }
        let indices = active.into_iter().collect::<Vec<_>>();
        response
            .shared()
            .response_facets_for(&indices)
            .map_err(admission::binding_error)?;
        Some(indices)
    } else {
        None
    };
    let accepted = if refusal {
        reader.caller().clone()
    } else {
        accepted.clone()
    };
    Ok(Some((
        Opened {
            prepared: Prepared { subject, value },
            response,
            primary,
            accepted,
            refusal: active,
        },
        reader,
    )))
}

/// Read a completed native outcome without rerunning its writer or preflight.
/// Outer None means optional disclosure closed. A native Reply of Option<T> still
/// represents a genuine no-result outcome as Some(None). The domain owns its typed
/// receipt and must retain that fallback immediately after its effect commits.
pub(crate) async fn render<F: Projection>(
    ctx: &Ctx,
    accepted: &Caller,
    input: F::Input,
) -> ApiResult<Option<F::Reply>> {
    render_kind::<F>(ctx, accepted, input, false).await
}

pub(super) async fn render_refusal<F: Projection>(
    ctx: &Ctx,
    attempted: &Caller,
    input: F::Input,
) -> ApiResult<Option<F::Reply>> {
    render_kind::<F>(ctx, attempted, input, true).await
}

async fn render_kind<F: Projection>(
    ctx: &Ctx,
    accepted: &Caller,
    input: F::Input,
    refusal: bool,
) -> ApiResult<Option<F::Reply>> {
    let input = Arc::new(input);
    let producing_input = Arc::clone(&input);
    let producing_origin = accepted.clone();
    let initial = completed(ctx, accepted, move |ctx, conn, current| {
        Box::pin(async move {
            let Some((opened, reader)) = prepare::<F>(
                ctx,
                conn,
                current,
                &producing_input,
                &producing_origin,
                refusal,
            )
            .await?
            else {
                return Ok(None);
            };
            let projection = ctx.clone();
            let response = opened.response.clone();
            if modern(reader.caller()) {
                // The accepted selection is provenance, never reused target admission.
                // read_response restores it and admits only the new response Read facts.
                let accepted = opened.accepted.clone();
                let shape = opened.refusal.clone();
                response
                    .read_projected_response(ctx, conn, &accepted, shape, move |conn, reader| {
                        Box::pin(async move {
                            produce::<F>(&projection, conn, reader, &producing_input, opened).await
                        })
                    })
                    .await
            } else {
                produce::<F>(&projection, conn, &reader, &producing_input, opened)
                    .await
                    .map(Some)
            }
        })
    })
    .await?;
    let Some((value, opened)) = initial else {
        ctx.complete_disclosure(resolver::Disclosure::Denied);
        return Ok(None);
    };
    let publication_caller = if refusal {
        opened.accepted.clone()
    } else {
        accepted.clone()
    };
    crate::key_operations::publication::retain(
        ctx,
        &publication_caller,
        (input, Arc::new(opened)),
        |ctx, conn, current, retained| {
            Box::pin(async move {
                let (input, opened) = retained;
                let ctx = if opened.refusal.is_some() {
                    ctx.for_refusal_projection()
                } else {
                    ctx.clone()
                };
                if validate::<F>(&ctx, conn, current, input, opened).await? {
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
    Ok(Some(value))
}

async fn produce<F: Projection>(
    ctx: &Ctx,
    conn: &mut Conn,
    reader: &Principal,
    input: &F::Input,
    opened: Opened<F>,
) -> ApiResult<(F::Reply, Opened<F>)> {
    F::require(ctx, conn, reader, input, &opened.prepared.subject).await?;
    let value = F::render(ctx, conn, reader, input, &opened.prepared).await?;
    Ok((value, opened))
}

async fn validate<F: Projection>(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    input: &Arc<F::Input>,
    opened: &Arc<Opened<F>>,
) -> ApiResult<bool> {
    let Some((fresh, reader)) = prepare::<F>(
        ctx,
        conn,
        current,
        input,
        &opened.accepted,
        opened.refusal.is_some(),
    )
    .await?
    else {
        return Ok(false);
    };
    let covered = match (&opened.refusal, &fresh.refusal) {
        (None, None) => fresh
            .response
            .shared()
            .covers_sources(opened.response.shared()),
        (Some(old), Some(new)) if old == new => fresh
            .response
            .shared()
            .covers_response_sources(opened.response.shared(), new),
        _ => false,
    };
    if opened.primary != fresh.primary || !covered {
        return Ok(false);
    }
    if modern(reader.caller()) {
        let response = fresh.response.clone();
        let opened = Arc::clone(opened);
        let projection = ctx.clone();
        let input = Arc::clone(input);
        let accepted = opened.accepted.clone();
        let shape = fresh.refusal.clone();
        Ok(response
            .read_projected_response(ctx, conn, &accepted, shape, move |conn, reader| {
                Box::pin(async move {
                    F::require(&projection, conn, reader, &input, &fresh.prepared.subject).await?;
                    F::check_opened(
                        &projection,
                        conn,
                        reader,
                        &input,
                        &opened.prepared,
                        &fresh.prepared,
                    )
                    .await
                })
            })
            .await?
            .unwrap_or(false))
    } else {
        F::require(ctx, conn, &reader, input, &fresh.prepared.subject).await?;
        F::check_opened(ctx, conn, &reader, input, &opened.prepared, &fresh.prepared).await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test assertions fail by panicking")]
    use super::*;
    use crate::key_operations::targets;

    #[test]
    fn retained_collection_subjects_are_order_independent_but_not_replaceable() {
        let system = targets::system();
        let account = KeyResolvedTarget {
            object: KeyObjectRef {
                kind: orch_access_contract::key_scope::KeyTargetKind::Identity(
                    orch_access_contract::key_scope::KeyIdentityKind::Account,
                ),
                id: Some(Id::from_u128(1)),
            },
            node_ancestors: Vec::new(),
            document_ancestors: Vec::new(),
            credential_account: None,
        };
        let collection = |targets: Vec<KeyResolvedTarget>| {
            let groups = targets
                .into_iter()
                .map(|target| (target.object.kind, vec![target]))
                .collect();
            read_sources(&[Facts::collection((), groups)], &())
        };
        let opened = collection(vec![system.clone(), account.clone()]).unwrap();
        assert_eq!(opened, collection(vec![account, system.clone()]).unwrap());
        assert_ne!(opened, collection(vec![system.clone()]).unwrap());
        assert_ne!(opened, collection(Vec::new()).unwrap());
        assert_ne!(
            collection(vec![system.clone()]).unwrap(),
            read_sources(&[Facts::one((), system.object.kind, system)], &()).unwrap()
        );
        assert!(read_sources::<()>(&[], &()).is_err());
        assert!(
            read_sources(
                &[Facts::collection(
                    (),
                    vec![(KeyTargetKind::Object(ObjectKind::Thread), Vec::new())]
                )],
                &()
            )
            .is_err()
        );
    }
}
