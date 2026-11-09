//! Typed native projections after an accepted effect, including jobs and genuine no-job results.

use super::*;

use orch_access_contract::key_scope::KeyObjectRef;

use orch_api_contract::common::ErrorCode;
/// Native response sources and projection, independent of a writer or its preflight.
/// Subject lookup and preparation must be effect-free. Rendering runs only after
/// same-operation Read admission; validation never repeats rendering or business work.
pub trait Projection: Sized + Send + Sync + 'static {
    type Source: Eq + Send + Sync + 'static;
    type Input: Send + Sync + 'static;
    type Subject: Send + Sync + 'static;
    type Value: Send + Sync + 'static;
    type Reply: Send + 'static;

    const READ_SOURCE: Self::Source;

    fn declaration(input: &Self::Input) -> &'static Declaration<Self::Source>;

    /// Server-owned native response names that also protect first-effect notices.
    /// Replay selects only these explicit canonical Required Read sources; it never
    /// infers a list from unresolved required inputs or admits Write/Manage roles.
    fn replay_read_sources(_input: &Self::Input) -> &'static [Self::Source] {
        &[]
    }

    /// Resolve the actual stored response subjects and known mandatory/root facts.
    /// Input is the immutable native outcome, never a replayable request body.
    /// READ_SOURCE may be a singleton or a genuinely selected, possibly empty collection.
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Self::Input,
    ) -> SourceFuture<'a, Self::Subject, Self::Source>;

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
    ) -> SourceFuture<'a, Self::Value, Self::Source>;

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

pub struct Prepared<F: Projection> {
    pub subject: F::Subject,
    pub value: F::Value,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Completed,
    Refusal,
    Replay,
}

impl Kind {
    fn response_only(self) -> bool {
        self != Self::Completed
    }
}

struct Opened<F: Projection> {
    prepared: Prepared<F>,
    response: OperationPlan,
    primary: ReadSources,
    accepted: Caller,
    response_only: Option<ReadSelection>,
    kind: Kind,
}

impl<F: Projection> Opened<F> {
    fn projection(&self) -> ApiResult<crate::key_operations::admission::ResponseProjection> {
        use crate::key_operations::admission::ResponseProjection as Mode;
        Ok(match self.kind {
            Kind::Completed => Mode::CompletedContent,
            Kind::Refusal => Mode::Refusal(
                self.response_only
                    .as_ref()
                    .ok_or_else(declaration_error)?
                    .content
                    .clone(),
            ),
            Kind::Replay => {
                let selected = self.response_only.as_ref().ok_or_else(declaration_error)?;
                if selected.required_reads.is_empty() {
                    Mode::Replay(selected.content.clone())
                } else {
                    Mode::ReplayReads {
                        content: selected.content.clone(),
                        required_reads: selected.required_reads.clone(),
                    }
                }
            }
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ReadSelection {
    content: Vec<usize>,
    required_reads: Vec<usize>,
}

fn selected_reads<S: Eq + 'static>(
    plan: &OperationPlan,
    declaration: &Declaration<S>,
    required_sources: &[S],
) -> ApiResult<ReadSelection> {
    let mut content = BTreeSet::new();
    for binding in declaration
        .bindings
        .iter()
        .filter(|binding| binding.usage == FacetUsage::ResponseContent)
    {
        content.extend(indices(plan, binding)?);
    }
    let mut required_reads = BTreeSet::new();
    for (index, source) in required_sources.iter().enumerate() {
        if required_sources[..index].contains(source) {
            return Err(declaration_error());
        }
        let mut found = false;
        for binding in declaration
            .bindings
            .iter()
            .filter(|binding| &binding.source == source)
        {
            found = true;
            if binding.usage != FacetUsage::Required {
                return Err(declaration_error());
            }
            let selected = indices(plan, binding)?;
            if selected.iter().any(|index| {
                plan.variant().facets[*index].action
                    != orch_access_contract::key_scope::KeyAction::Read
            }) {
                return Err(declaration_error());
            }
            required_reads.extend(selected);
        }
        if !found {
            return Err(declaration_error());
        }
    }
    let selection = ReadSelection {
        content: content.into_iter().collect(),
        required_reads: required_reads.into_iter().collect(),
    };
    plan.shared()
        .response_read_facets_for(&selection.content, &selection.required_reads)
        .map_err(admission::binding_error)?;
    Ok(selection)
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

/// A completed act has no new effect operands. Its declared content roles must still
/// be genuine and complete; first publication and refusal keep their original strict rows.
fn bind_shape<S: Eq + 'static>(
    route: orch_api_contract::routes::Route,
    declaration: &Declaration<S>,
    facts: &[Facts<S>],
    kind: Kind,
) -> ApiResult<OperationPlan> {
    use orch_engine::operation_scope::sources;
    let response = match kind {
        Kind::Completed => sources::bind(route, declaration, facts),
        Kind::Refusal => sources::bind_declared(route, declaration, facts),
        Kind::Replay => {
            let known =
                sources::bind_known(route, declaration, facts).map_err(admission::binding_error)?;
            for binding in declaration
                .bindings
                .iter()
                .filter(|binding| binding.usage == FacetUsage::ResponseContent)
            {
                if !facts.iter().any(|fact| fact.source == binding.source) {
                    return Err(declaration_error());
                }
            }
            return Ok(OperationPlan::from_shared(known));
        }
    };
    response
        .map(OperationPlan::from_shared)
        .map_err(admission::binding_error)
}

async fn prepare<F: Projection>(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    input: &F::Input,
    accepted: &Caller,
    kind: Kind,
) -> ApiResult<Option<(Opened<F>, Principal)>> {
    let declaration = F::declaration(input);
    let (subject, mut facts) = F::subject(ctx, conn, input).await?;
    let primary = read_sources(&facts, &F::READ_SOURCE)?;
    if kind.response_only() && primary.identities.is_empty() {
        return Ok(None);
    }
    let known = if kind.response_only() {
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
    let response = bind_shape(ctx.route, declaration, &facts, kind)?;
    let active = if kind.response_only() {
        let required = if kind == Kind::Replay {
            F::replay_read_sources(input)
        } else {
            &[]
        };
        Some(selected_reads(&response, declaration, required)?)
    } else {
        None
    };
    let accepted = if kind.response_only() {
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
            response_only: active,
            kind,
        },
        reader,
    )))
}

/// Read a completed native outcome without rerunning its writer or preflight.
/// Outer None means optional disclosure closed. A native Reply of Option<T> still
/// represents a genuine no-result outcome as Some(None). The domain owns its typed
/// receipt and must retain that fallback immediately after its effect commits.
pub async fn render<F: Projection>(
    ctx: &Ctx,
    accepted: &Caller,
    input: F::Input,
) -> ApiResult<Option<F::Reply>> {
    render_kind::<F>(ctx, accepted, input, Kind::Completed).await
}

pub(super) async fn render_refusal<F: Projection>(
    ctx: &Ctx,
    attempted: &Caller,
    input: F::Input,
) -> ApiResult<Option<F::Reply>> {
    render_kind::<F>(ctx, attempted, input, Kind::Refusal).await
}

/// Project an already completed creation under this request's new current credential.
/// The account comes from the actual completed native record, never a user body or old token.
/// No first credential or spent Write/Manage/copy source is restored or re-admitted.
pub async fn render_completed_replay<F: Projection>(
    ctx: &Ctx,
    completed_by: Id,
    input: F::Input,
) -> ApiResult<Option<F::Reply>> {
    if ctx.authenticated_user()?.id() != completed_by {
        return Err(ApiError::not_found());
    }
    let current = authorization::caller_of(ctx.user()?)?;
    let ctx = ctx.for_completed_replay_projection();
    render_kind::<F>(&ctx, &current, input, Kind::Replay).await
}

async fn render_kind<F: Projection>(
    ctx: &Ctx,
    accepted: &Caller,
    input: F::Input,
    kind: Kind,
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
                kind,
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
                let shape = opened.projection()?;
                response
                    .read_response_projection(ctx, conn, &accepted, shape, move |conn, reader| {
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
    if !modern(&opened.accepted) {
        // The ordinary native Read has now produced and committed this exact response.
        // Scoped reads complete inside their own facet projection; Denied never widens.
        ctx.complete_disclosure(resolver::Disclosure::Full);
    }
    let publication_caller = if kind.response_only() {
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
                let ctx = match opened.kind {
                    Kind::Refusal => ctx.for_refusal_projection(),
                    Kind::Replay => ctx.for_completed_replay_projection(),
                    Kind::Completed => ctx.clone(),
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
    mut opened: Opened<F>,
) -> ApiResult<(F::Reply, Opened<F>)> {
    F::require(ctx, conn, reader, input, &opened.prepared.subject).await?;
    let value = F::render(ctx, conn, reader, input, &opened.prepared).await?;
    if opened.kind.response_only() {
        opened.accepted = reader.caller().clone();
    }
    Ok((value, opened))
}

async fn validate<F: Projection>(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    input: &Arc<F::Input>,
    opened: &Arc<Opened<F>>,
) -> ApiResult<bool> {
    let Some((fresh, reader)) =
        prepare::<F>(ctx, conn, current, input, &opened.accepted, opened.kind).await?
    else {
        return Ok(false);
    };
    let covered = match (&opened.response_only, &fresh.response_only) {
        (None, None) => fresh
            .response
            .shared()
            .covers_sources(opened.response.shared()),
        (Some(old), Some(new)) if old == new => {
            fresh.response.shared().covers_response_read_sources(
                opened.response.shared(),
                &new.content,
                &new.required_reads,
            )
        }
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
        let shape = fresh.projection()?;
        Ok(response
            .read_response_projection(ctx, conn, &accepted, shape, move |conn, reader| {
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
        let preserved =
            F::check_opened(ctx, conn, &reader, input, &opened.prepared, &fresh.prepared).await?;
        if preserved {
            ctx.complete_disclosure(resolver::Disclosure::Full);
        }
        Ok(preserved)
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {
    #![allow(clippy::unwrap_used, reason = "test assertions fail by panicking")]

    use super::*;

    use crate::key_operations::targets;

    use orch_api_contract::admin::AdminUserCreate;

    use orch_api_contract::admin_sources;

    use orch_api_contract::admin_sources::Source;

    use orch_api_contract::routes::Endpoint;
    #[cfg(any(test, feature = "test-support"))]
    fn a_completed_account_replay_requires_its_declared_response_source_and_no_spent_creation_source()
    -> ApiResult<()> {
        let facts = [Facts::one(
            Source::Content,
            KeyTargetKind::Identity(orch_access_contract::key_scope::KeyIdentityKind::Account),
            targets::system(),
        )];
        let replay = bind_shape(
            AdminUserCreate::ROUTE,
            &admin_sources::USER_CREATE,
            &facts,
            Kind::Replay,
        )?;
        let content = replay
            .shared()
            .response_facets()
            .map_err(admission::binding_error)?;
        assert_eq!(content.len(), 1);
        assert_eq!(
            content[0].action,
            orch_access_contract::key_scope::KeyAction::Read
        );
        assert!(
            replay.shared().required_facets().is_err(),
            "spent Required stays unresolved"
        );
        assert!(
            bind_shape(
                AdminUserCreate::ROUTE,
                &admin_sources::USER_CREATE,
                &[],
                Kind::Replay
            )
            .is_err(),
            "a replay cannot skip its actual declared Read source"
        );
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_completed_account_replay_requires_its_declared_response_source_and_no_spent_creation_source()
    -> impl ::std::process::Termination {
        self::a_completed_account_replay_requires_its_declared_response_source_and_no_spent_creation_source()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_first_account_creation_without_its_declared_required_source_is_refused() {
        let facts = [Facts::one(
            Source::Content,
            KeyTargetKind::Identity(orch_access_contract::key_scope::KeyIdentityKind::Account),
            targets::system(),
        )];
        assert!(
            orch_engine::operation_scope::sources::bind_required(
                AdminUserCreate::ROUTE,
                &admin_sources::USER_CREATE,
                &facts,
            )
            .is_err(),
            "the native effect gate still refuses before a first write"
        );
        assert!(
            bind_shape(
                AdminUserCreate::ROUTE,
                &admin_sources::USER_CREATE,
                &facts,
                Kind::Completed
            )
            .is_err()
        );
        assert!(
            bind_shape(
                AdminUserCreate::ROUTE,
                &admin_sources::USER_CREATE,
                &facts,
                Kind::Refusal
            )
            .is_err(),
            "a refusal's declared row also keeps its original strict completeness"
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_first_account_creation_without_its_declared_required_source_is_refused() {
        self::a_first_account_creation_without_its_declared_required_source_is_refused()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn additional_native_response_roles_are_explicit_and_cannot_include_management() -> ApiResult<()>
    {
        use orch_api_contract::operations::TargetResolver;
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Role {
            Body,
            Names,
            Manage,
            Missing,
        }
        const ROW: Declaration<Role> = Declaration {
            operation: "ShareAgentCreate.live",
            bindings: &[
                Binding {
                    source: Role::Body,
                    target: TargetResolver::Resolved("response_source"),
                    usage: FacetUsage::ResponseContent,
                    cardinality: Cardinality::One,
                },
                Binding {
                    source: Role::Names,
                    target: TargetResolver::ResolvedCollection("share_name_threads"),
                    usage: FacetUsage::Required,
                    cardinality: Cardinality::Collection,
                },
                Binding {
                    source: Role::Manage,
                    target: TargetResolver::Resolved("source"),
                    usage: FacetUsage::Required,
                    cardinality: Cardinality::One,
                },
            ],
        };
        let mut core = orch_engine::operation_scope::Prepared::for_selection(ROW.operation)
            .map_err(admission::binding_error)?;
        for binding in ROW.bindings {
            if binding.source == Role::Manage {
                continue;
            }
            for index in orch_engine::operation_scope::sources::indices(&core, binding)
                .map_err(admission::binding_error)?
            {
                let kind = core.variant().facets[index].resources[0];
                let actual = KeyResolvedTarget {
                    object: KeyObjectRef {
                        kind,
                        id: Some(Id::from_u128(index as u128 + 1)),
                    },
                    node_ancestors: vec![],
                    document_ancestors: vec![],
                    credential_account: None,
                };
                core.bind(index, kind, &[actual])
                    .map_err(admission::binding_error)?;
            }
        }
        let plan = OperationPlan::from_shared(core);
        let selected = selected_reads(&plan, &ROW, &[Role::Names])?;
        assert_eq!(selected.content.len(), 1);
        assert_eq!(selected.required_reads.len(), 1);
        assert!(plan.shared().required_facets().is_err());
        assert!(selected_reads(&plan, &ROW, &[Role::Manage]).is_err());
        assert!(selected_reads(&plan, &ROW, &[Role::Missing]).is_err());
        assert!(selected_reads(&plan, &ROW, &[Role::Body]).is_err());
        assert!(selected_reads(&plan, &ROW, &[Role::Names, Role::Names]).is_err());
        assert!(selected_reads(&plan, &ROW, &[])?.required_reads.is_empty());
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_additional_native_response_roles_are_explicit_and_cannot_include_management()
    -> impl ::std::process::Termination {
        self::additional_native_response_roles_are_explicit_and_cannot_include_management()
    }

    #[cfg(any(test, feature = "test-support"))]
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
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_retained_collection_subjects_are_order_independent_but_not_replaceable() {
        self::retained_collection_subjects_are_order_independent_but_not_replaceable()
    }
}
