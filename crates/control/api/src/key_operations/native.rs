//! Shared factual source binding and current native read preparation.
//!
//! The domain selects native subjects and rows. This adapter only connects their
//! explicit business families and cardinality to the canonical operation facets.

use super::{admission, resolver};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard::{self, CurrentHttpIdentity, HttpIdentityMode};
use crate::registry::{Ctx, OperationPlan, OptionalOperationFuture};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_api_contract::operation_sources::{Binding, Cardinality, Declaration};
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_api_contract::routes::Route;
use orch_authz::{Caller, Principal};
use orch_db::{Conn, DbError};
use std::sync::Arc;

pub(crate) mod effects;

/// Native source facts, supplied before any response is serialized.
#[derive(Clone)]
pub(crate) struct Facts<S> {
    pub source: S,
    pub cardinality: Cardinality,
    pub groups: Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)>,
}

impl<S> Facts<S> {
    pub(crate) fn one(source: S, family: KeyTargetKind, target: KeyResolvedTarget) -> Self {
        Self {
            source,
            cardinality: Cardinality::One,
            groups: vec![(family, vec![target])],
        }
    }

    pub(crate) fn collection(
        source: S,
        groups: Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)>,
    ) -> Self {
        Self {
            source,
            cardinality: Cardinality::Collection,
            groups,
        }
    }
}

fn declaration_error() -> ApiError {
    ApiError::internal("the native source declaration does not match its operation")
}

/// A role may have several canonical actions; the declaration does not restate them.
fn indices<S>(plan: &OperationPlan, binding: &Binding<S>) -> ApiResult<Vec<usize>> {
    let indices = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter(|(_, facet)| facet.target == binding.target && facet.usage == binding.usage)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if indices.is_empty() {
        return Err(declaration_error());
    }
    Ok(indices)
}

/// Bind every declared facet once. An absent source is never a checked-empty collection.
pub(crate) fn bind<S: Eq + 'static>(
    ctx: &Ctx,
    declaration: &Declaration<S>,
    facts: &[Facts<S>],
) -> ApiResult<OperationPlan> {
    bind_route(ctx.route, declaration, facts)
}

/// Bind only known native facts for effect-free preparation of remaining sources.
/// Missing roles stay unbound; this is never an empty or complete admission proof.
pub(crate) fn bind_known<S: Eq + 'static>(
    ctx: &Ctx,
    declaration: &Declaration<S>,
    facts: &[Facts<S>],
) -> ApiResult<OperationPlan> {
    bind_facts(ctx.route, declaration, facts, false)
}

/// A writer may defer optional content, but every actual required role must be complete.
/// Unselected response sources stay unresolved, never a fabricated empty collection.
pub(crate) fn bind_required<S: Eq + 'static>(
    ctx: &Ctx,
    declaration: &Declaration<S>,
    facts: &[Facts<S>],
) -> ApiResult<OperationPlan> {
    bind_required_route(ctx.route, declaration, facts)
}

fn bind_required_route<S: Eq + 'static>(
    route: Route,
    declaration: &Declaration<S>,
    facts: &[Facts<S>],
) -> ApiResult<OperationPlan> {
    let plan = bind_facts(route, declaration, facts, false)?;
    plan.shared()
        .required_facets()
        .map_err(admission::binding_error)?;
    Ok(plan)
}

fn bind_route<S: Eq + 'static>(
    route: Route,
    declaration: &Declaration<S>,
    facts: &[Facts<S>],
) -> ApiResult<OperationPlan> {
    bind_facts(route, declaration, facts, true)
}

fn bind_facts<S: Eq + 'static>(
    route: Route,
    declaration: &Declaration<S>,
    facts: &[Facts<S>],
    complete: bool,
) -> ApiResult<OperationPlan> {
    let mut plan = OperationPlan::new(route, declaration.operation)?;
    let mut filled = vec![false; plan.variant().facets.len()];
    for binding in declaration.bindings {
        let binding_indices = indices(&plan, binding)?;
        let mut matches = facts.iter().filter(|fact| fact.source == binding.source);
        let Some(fact) = matches.next() else {
            if complete {
                return Err(declaration_error());
            }
            continue;
        };
        if matches.next().is_some() || fact.cardinality != binding.cardinality {
            return Err(declaration_error());
        }
        for index in binding_indices {
            if filled[index] {
                return Err(declaration_error());
            }
            match binding.cardinality {
                Cardinality::One => {
                    let [(family, targets)] = fact.groups.as_slice() else {
                        return Err(declaration_error());
                    };
                    if targets.len() != 1
                        || matches!(binding.target, TargetResolver::ResolvedCollection(_))
                    {
                        return Err(declaration_error());
                    }
                    plan.bind(index, *family, targets)?;
                }
                Cardinality::Collection => {
                    if !matches!(binding.target, TargetResolver::ResolvedCollection(_)) {
                        return Err(declaration_error());
                    }
                    if fact.groups.is_empty() {
                        plan.bind_empty(index)?;
                    } else {
                        plan.bind_collection(index, &fact.groups)?;
                    }
                }
            }
            filled[index] = true;
        }
    }
    if complete && filled.iter().any(|filled| !filled)
        || facts.iter().any(|fact| {
            !declaration
                .bindings
                .iter()
                .any(|binding| binding.source == fact.source)
        })
    {
        return Err(declaration_error());
    }
    Ok(plan)
}

/// Check a native candidate under the consuming operation before the output window.
pub(crate) fn permits<S: Eq + 'static>(
    plan: &OperationPlan,
    declaration: &Declaration<S>,
    caller: &Caller,
    source: &S,
    family: KeyTargetKind,
    target: &KeyResolvedTarget,
) -> ApiResult<bool> {
    permits_usage(
        plan,
        declaration,
        caller,
        source,
        FacetUsage::Required,
        family,
        target,
    )
}

/// Select a native source for its declared effect/disclosure role, without lending another role.
pub(crate) fn permits_usage<S: Eq + 'static>(
    plan: &OperationPlan,
    declaration: &Declaration<S>,
    caller: &Caller,
    source: &S,
    usage: FacetUsage,
    family: KeyTargetKind,
    target: &KeyResolvedTarget,
) -> ApiResult<bool> {
    let bindings = declaration
        .bindings
        .iter()
        .filter(|binding| &binding.source == source && binding.usage == usage)
        .collect::<Vec<_>>();
    if bindings.is_empty() {
        return Err(declaration_error());
    }
    for binding in bindings {
        for index in indices(plan, binding)? {
            if !plan.permits_binding(caller, index, family, std::slice::from_ref(target))? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Retained native operands and their consuming operation's current response facts.
pub(crate) struct Prepared<T> {
    pub response: OperationPlan,
    pub value: T,
}

/// One native stored-subject provider, shared by its declared operations.
pub(crate) trait Stored: Send + Sync + 'static {
    type Source: Eq + Send + Sync + 'static;
    type Value: Send + Sync + 'static;
    const ROWS: &'static [Declaration<Self::Source>];

    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        declaration: &'static Declaration<Self::Source>,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<Facts<Self::Source>>)>>;

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>>;
}

/// A stored native provider whose authoritative subject also uses its typed query.
/// The ordinary handler still owns that same query's validation/business behavior.
pub(crate) trait StoredQuery: Stored {
    type Query: Send + 'static;

    fn load_query<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        declaration: &'static Declaration<Self::Source>,
        query: Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<Facts<Self::Source>>)>>;
}

async fn finish_stored<F: Stored>(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    declaration: &'static Declaration<F::Source>,
    value: F::Value,
    facts: Vec<Facts<F::Source>>,
) -> ApiResult<OperationPlan> {
    let response = bind(ctx, declaration, &facts)?;
    finish_stored_plan::<F>(ctx, conn, caller, response, value).await
}

async fn finish_stored_plan<F: Stored>(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    response: OperationPlan,
    value: F::Value,
) -> ApiResult<OperationPlan> {
    let principal = response.preparation_principal(ctx, conn, caller).await?;
    F::require(ctx, conn, &principal, &value).await?;
    Ok(response.clone().with_data(Prepared { response, value }))
}

/// A stored writer's optional response source set is selected only at its response boundary.
/// The native loader still supplies every real required subject and every known Read root.
pub(crate) fn stored_deferred<'a, F: Stored, Q, B>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    _query: Q,
    _body: B,
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !modern(caller) {
            return Ok(None);
        }
        let declaration = F::ROWS
            .iter()
            .find(|row| row.operation == ctx.route.name)
            .ok_or_else(declaration_error)?;
        let (value, facts) = F::load(ctx, conn, declaration).await?;
        let response = bind_required(ctx, declaration, &facts)?;
        finish_stored_plan::<F>(ctx, conn, caller, response, value)
            .await
            .map(Some)
    })
}

/// Registry adapter for stored subjects. Ordinary/legacy paths retain their native cold path.
pub(crate) fn stored<'a, F: Stored, Q, B>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    _query: Q,
    _body: B,
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !modern(caller) {
            return Ok(None);
        }
        let declaration = F::ROWS
            .iter()
            .find(|row| row.operation == ctx.route.name)
            .ok_or_else(declaration_error)?;
        let (value, facts) = F::load(ctx, conn, declaration).await?;
        finish_stored::<F>(ctx, conn, caller, declaration, value, facts)
            .await
            .map(Some)
    })
}

/// Typed-query Registry wrapper over the very same stored-subject adapter.
pub(crate) fn stored_query<'a, F: StoredQuery, B>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: F::Query,
    _body: B,
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !modern(caller) {
            return Ok(None);
        }
        let declaration = F::ROWS
            .iter()
            .find(|row| row.operation == ctx.route.name)
            .ok_or_else(declaration_error)?;
        let (value, facts) = F::load_query(ctx, conn, declaration, query).await?;
        finish_stored::<F>(ctx, conn, caller, declaration, value, facts)
            .await
            .map(Some)
    })
}

/// An effect-free native renderer for a stored subject. Auxiliary effects stay outside this read.
pub(crate) trait StoredRead: Stored {
    type Reply: Send + 'static;

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>>;
}

/// A stored reader uses the same factual preparation and current response boundary as a selection.
pub(crate) async fn stored_read<F: StoredRead>(ctx: Ctx, (): (), (): ()) -> ApiResult<F::Reply> {
    let (reply, publication) = read(ctx.clone(), |ctx, conn, principal| {
        Box::pin(async move {
            if modern(principal.caller()) {
                let ready = ctx.operation_data::<Prepared<F::Value>>()?;
                let projection = ctx.clone();
                let response = ready.response.clone();
                let accepted = principal.caller().clone();
                let reply = response
                    .clone()
                    .read_response(ctx, conn, principal.caller(), move |conn, reader| {
                        Box::pin(async move {
                            F::require(&projection, conn, reader, &ready.value).await?;
                            F::render(&projection, conn, reader, &ready.value).await
                        })
                    })
                    .await?
                    .ok_or_else(|| {
                        ApiError::new(
                            orch_api_contract::common::ErrorCode::AuthzDenied,
                            "the native stored response has not been admitted",
                        )
                    })?;
                Ok((reply, Some((response, accepted))))
            } else {
                let declaration = F::ROWS
                    .iter()
                    .find(|row| row.operation == ctx.route.name)
                    .ok_or_else(declaration_error)?;
                let (value, _) = F::load(ctx, conn, declaration).await?;
                F::require(ctx, conn, principal, &value).await?;
                Ok((F::render(ctx, conn, principal, &value).await?, None))
            }
        })
    })
    .await?;
    if let Some((earlier, accepted)) = publication {
        super::publication::retain(&ctx, &accepted, earlier, |ctx, conn, reader, earlier| {
            Box::pin(async move {
                let declaration = F::ROWS
                    .iter()
                    .find(|row| row.operation == ctx.route.name)
                    .ok_or_else(declaration_error)?;
                let (value, facts) = F::load(ctx, conn, declaration).await?;
                let fresh = bind(ctx, declaration, &facts)?;
                let principal = publication_principal(ctx, conn, reader, &fresh, earlier).await?;
                F::require(ctx, conn, &principal, &value).await
            })
        })?;
    }
    Ok(reply)
}

pub(crate) fn modern(caller: &Caller) -> bool {
    caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
}

/// Keep ordinary/legacy preparation on its actual caller; only scoped preparation uses facts.
pub(crate) async fn preparation_principal(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    known: &OperationPlan,
) -> ApiResult<Principal> {
    if modern(caller) {
        return known.preparation_principal(ctx, conn, caller).await;
    }
    let principal =
        authorization::load_principal(conn, caller.clone(), ctx.app.clock.time()).await?;
    crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await
}

/// One native selected-row factory and renderer. Its cursor and output shape stay native.
pub(crate) trait Selected: Send + Sync + 'static {
    type Source: Eq + Send + Sync + 'static;
    type Query: Clone + Send + Sync + 'static;
    type Value: Send + Sync + 'static;
    type Reply: Send + 'static;
    const DECLARATION: &'static Declaration<Self::Source>;

    /// Select from the server's prepared native intent, never an arbitrary client label.
    /// The Registry invokes this again after each fresh native selection checkpoint.
    fn declaration(
        _query: &Self::Query,
        _value: &Self::Value,
    ) -> &'static Declaration<Self::Source> {
        Self::DECLARATION
    }

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<Facts<Self::Source>>)>>;

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>>;

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>>;
}

/// Typed Registry preparation for any declared native selected-row family.
pub(crate) fn selected<'a, F: Selected>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: F::Query,
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !modern(caller) {
            return Ok(None);
        }
        let (value, facts) = F::select(ctx, conn, caller, &query).await?;
        let response = bind(ctx, F::declaration(&query, &value), &facts)?;
        let principal = response.preparation_principal(ctx, conn, caller).await?;
        F::require(ctx, conn, &principal).await?;
        Ok(Some(
            response.clone().with_data(Prepared { response, value }),
        ))
    })
}

/// Native output under the fresh snapshot's own operation and current reader.
pub(crate) async fn selected_read<F: Selected>(
    ctx: Ctx,
    query: F::Query,
    (): (),
) -> ApiResult<F::Reply> {
    let retained_query = query.clone();
    let (reply, publication) = read(ctx.clone(), move |ctx, conn, principal| {
        let query = query.clone();
        Box::pin(async move {
            if modern(principal.caller()) {
                let ready = ctx.operation_data::<Prepared<F::Value>>()?;
                let projection = ctx.clone();
                let response = ready.response.clone();
                let accepted = principal.caller().clone();
                let reply = response
                    .clone()
                    .read_response(ctx, conn, principal.caller(), move |conn, reader| {
                        Box::pin(async move {
                            F::require(&projection, conn, reader).await?;
                            F::render(&projection, conn, reader, &ready.value).await
                        })
                    })
                    .await?
                    .ok_or_else(|| {
                        ApiError::new(
                            orch_api_contract::common::ErrorCode::AuthzDenied,
                            "the native selected response has not been admitted",
                        )
                    })?;
                Ok((reply, Some((response, accepted))))
            } else {
                F::require(ctx, conn, principal).await?;
                let (value, _) = F::select(ctx, conn, principal.caller(), &query).await?;
                Ok((F::render(ctx, conn, principal, &value).await?, None))
            }
        })
    })
    .await?;
    if let Some((earlier, accepted)) = publication {
        super::publication::retain(
            &ctx,
            &accepted,
            (retained_query, earlier),
            |ctx, conn, reader, (query, earlier)| {
                Box::pin(async move {
                    let (value, facts) = F::select(ctx, conn, reader.caller(), query).await?;
                    let fresh = bind(ctx, F::declaration(query, &value), &facts)?;
                    let principal =
                        publication_principal(ctx, conn, reader, &fresh, earlier).await?;
                    F::require(ctx, conn, &principal).await
                })
            },
        )?;
    }
    Ok(reply)
}

/// Raw formatting of an effect-free prepared native selection. Byte suppliers and live
/// streams retain their own once-only opening and per-delivery native witness boundaries.
pub(crate) async fn selected_raw<F: Selected<Reply = axum::response::Response>>(
    ctx: Ctx,
    query: F::Query,
    _body: axum::body::Body,
) -> ApiResult<axum::response::Response> {
    selected_read::<F>(ctx, query, ()).await
}

/// Re-admit fresh native facts only if they cover the actual earlier body's sources.
/// No fresh row, variant or optional-name collection can lend evidence to an old body.
async fn publication_principal(
    ctx: &Ctx,
    conn: &mut Conn,
    reader: &Principal,
    fresh: &OperationPlan,
    earlier: &OperationPlan,
) -> ApiResult<Principal> {
    if !fresh.shared().covers_sources(earlier.shared()) {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "a selected response source is no longer readable",
        ));
    }
    let admitted = fresh.clone().admit(reader.caller())?;
    if !admitted.content {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "the response content is no longer admitted",
        ));
    }
    let principal =
        authorization::load_principal(conn, admitted.caller, ctx.app.clock.time()).await?;
    crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await
}

/// One real current HTTP origin and its effect-free operation preparation.
pub(crate) struct ReadFrame {
    pub ctx: Ctx,
    pub identity: CurrentHttpIdentity,
    pub principal: Principal,
}

/// The sole native read preparation, shared by snapshot readers and their guards.
/// Entry registration defers this resolver until the actual snapshot is open.
pub(crate) async fn current_read_frame(ctx: &Ctx, conn: &mut Conn) -> ApiResult<ReadFrame> {
    if !ctx.snapshot_read()
        || transaction_guard::changes(ctx)
        || !orch_db::queries::transaction_guard::in_transaction(conn)
    {
        return Err(ApiError::internal(
            "snapshot read preparation requires its declared read transaction",
        ));
    }
    let identity =
        transaction_guard::current_http_identity(ctx, conn, HttpIdentityMode::Read).await?;
    let result: ApiResult<(Ctx, Principal)> = async {
        transaction_guard::guard_user(&identity.user)?;
        let mut current = ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
        let caller = match resolver::resolve(&current, conn, &identity.caller).await? {
            Some(resolved) => {
                let admitted = resolved.complete()?;
                let caller = admitted.caller.clone();
                current = admission::install(current, admitted)?;
                caller
            }
            None => {
                resolver::ordinary_unclassified(&identity.caller)?;
                identity.caller.clone()
            }
        };
        let principal = authorization::load_principal(conn, caller, identity.now).await?;
        let principal = crate::sign_in::second_step::hold(conn, &identity.user, principal).await?;
        let guard =
            transaction_guard::finish_read_guard(&current, conn, &identity, principal).await?;
        Ok((current, guard.into_principal()))
    }
    .await;
    identity.check_deadline(&ctx.app)?;
    let (ctx, principal) = result?;
    transaction_guard::checked_snapshot_read();
    Ok(ReadFrame {
        ctx,
        identity,
        principal,
    })
}

pub(crate) struct Completed<T> {
    pub(crate) ctx: Ctx,
    pub(crate) identity: CurrentHttpIdentity,
    pub(crate) value: T,
}

/// Prepare once through the shared native adapter on this attempt's snapshot and
/// project from those same operands. Auxiliary work stays outside this transaction.
pub(crate) async fn read_once<T, F>(ctx: Ctx, project: F) -> ApiResult<Completed<T>>
where
    T: Send + 'static,
    F: for<'a> FnOnce(&'a Ctx, &'a mut Conn, &'a Principal) -> BoxFuture<'a, ApiResult<T>> + Send,
{
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let ReadFrame {
        ctx,
        identity,
        principal,
    } = current_read_frame(&ctx, &mut tx).await?;
    let result: ApiResult<T> = async {
        let value = project(&ctx, &mut tx, &principal).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(value)
    }
    .await;
    identity.check_deadline(&ctx.app)?;
    Ok(Completed {
        ctx,
        identity,
        value: result?,
    })
}

/// Effect-free selected-row reads share current identity, snapshot retry and final cut.
pub(crate) async fn read<T, F>(ctx: Ctx, project: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: for<'a> Fn(&'a Ctx, &'a mut Conn, &'a Principal) -> BoxFuture<'a, ApiResult<T>>
        + Send
        + Sync
        + 'static,
{
    let project = Arc::new(project);
    super::snapshot_read::run(ctx, move |ctx| {
        let project = Arc::clone(&project);
        async move {
            let completed = read_once(ctx, move |ctx, conn, principal| {
                project(ctx, conn, principal)
            })
            .await?;
            Ok(completed.value)
        }
    })
    .await
}

/// A completed writer's optional native readback. No effect resolver is replayed.
/// The exact HTTP lifetime also covers the snapshot commit, not only its renderer.
pub(crate) async fn completed<T, F>(
    ctx: &Ctx,
    accepted: &Caller,
    project: F,
) -> ApiResult<Option<T>>
where
    T: Send,
    F: for<'a> FnOnce(&'a Ctx, &'a mut Conn, &'a Principal) -> BoxFuture<'a, ApiResult<Option<T>>>
        + Send,
{
    super::completed_projection::run(ctx, async {
        let result: ApiResult<Option<T>> = async {
            if !admission::same_response_origin(ctx, accepted)? {
                ctx.complete_disclosure(resolver::Disclosure::Denied);
                return Ok(None);
            }
            if ctx.disclosure() == resolver::Disclosure::Denied {
                return Ok(None);
            }
            let mut tx = ctx.app.db.begin_snapshot().await?;
            let identity = match transaction_guard::current_http_identity(
                ctx,
                &mut tx,
                HttpIdentityMode::Read,
            )
            .await
            {
                Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
                    ctx.complete_disclosure(resolver::Disclosure::Denied);
                    return Ok(None);
                }
                other => other?,
            };
            let accepted = if accepted.grant().is_some() {
                match orch_authz::CapturedCaller::capture(accepted)
                    .restore_current(&mut tx, ctx.app.clock.as_ref())
                    .await
                    .map_err(authorization::restoration_error)?
                {
                    Some(caller) => caller,
                    None => {
                        ctx.complete_disclosure(resolver::Disclosure::Denied);
                        return Ok(None);
                    }
                }
            } else {
                accepted.clone()
            };
            let caller = identity
                .caller
                .intersect_restrictions(&accepted)
                .map_err(ApiError::internal)?;
            let deadline_caller = caller.clone();
            ctx.publication
                .retain_deadline(caller.grant().and_then(orch_authz::TokenGrant::expires_at))?;
            let principal = authorization::load_principal(&mut tx, caller, identity.now).await?;
            let principal =
                crate::sign_in::second_step::hold(&mut tx, &identity.user, principal).await?;
            let value = match project(ctx, &mut tx, &principal).await {
                Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
                    ctx.complete_disclosure(resolver::Disclosure::Denied);
                    None
                }
                other => other?,
            };
            tx.commit().await.map_err(DbError::from)?;
            if identity.check_deadline(&ctx.app).is_err()
                || !deadline_caller.key_deadline_allows(ctx.app.clock.as_ref())
            {
                ctx.complete_disclosure(resolver::Disclosure::Denied);
                return Ok(None);
            }
            Ok(value)
        }
        .await;
        match result {
            Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
                ctx.complete_disclosure(resolver::Disclosure::Denied);
                Ok(None)
            }
            other => other,
        }
    })
    .await
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "fixture invariants and test assertions fail by panicking"
)]
mod tests {
    use super::*;
    use orch_access_contract::key_scope::{KeyIdentityKind, KeyObjectRef, KeyScope};
    use orch_access_contract::permissions::ObjectKind;
    use orch_api_contract::directory::{OrgMemberList, TagUpdate};
    use orch_api_contract::directory_sources::{self as directory, Source};
    use orch_api_contract::routes::Endpoint;
    use orch_types::Id;
    use serde_json::json;

    fn target(kind: KeyTargetKind, id: u128) -> KeyResolvedTarget {
        KeyResolvedTarget {
            object: KeyObjectRef {
                kind,
                id: Some(Id::from_u128(id)),
            },
            node_ancestors: Vec::new(),
            document_ancestors: Vec::new(),
            credential_account: None,
        }
    }
    fn caller(id: u128, actions: &[&str]) -> Caller {
        let scope: KeyScope = serde_json::from_value(json!({"rules":[{
            "target":{"kind":"org","id":Id::from_u128(id),"reach":"self"},
            "resources":["org"],"allow":[{"operation":"TagUpdate","actions":actions}]
        }]}))
        .expect("valid fixture scope");
        Caller::token(
            Id::from_u128(9),
            orch_authz::TokenGrant::from_stored_scope(&[], None, Some(scope), None)
                .expect("valid fixture grant"),
        )
    }

    #[test]
    fn stored_declaration_keeps_manage_and_response_read_independent() {
        let org = KeyTargetKind::Object(ObjectKind::Org);
        let facts = [Facts::one(Source::TagNode, org, target(org, 1))];
        let receipt = bind_route(TagUpdate::ROUTE, &directory::TAG_UPDATE, &facts)
            .expect("complete stored facts")
            .admit(&caller(1, &["manage"]))
            .expect("manage-only is a real accepted write");
        assert!(!receipt.content);
        let full = bind_route(TagUpdate::ROUTE, &directory::TAG_UPDATE, &facts)
            .expect("complete stored facts")
            .admit(&caller(1, &["manage", "read"]))
            .expect("full own-operation projection");
        assert!(full.content);
        assert!(
            bind_route(TagUpdate::ROUTE, &directory::TAG_UPDATE, &facts)
                .expect("facts")
                .admit(&caller(2, &["manage", "read"]))
                .is_err()
        );
        assert!(
            bind_route(TagUpdate::ROUTE, &directory::TAG_UPDATE, &facts)
                .expect("facts")
                .admit(&caller(1, &["read"]))
                .is_err()
        );
    }

    #[test]
    fn missing_sources_and_wrong_cardinality_never_become_empty_proof() {
        assert!(bind_route(TagUpdate::ROUTE, &directory::TAG_UPDATE, &[]).is_err());
        assert!(
            bind_route(
                TagUpdate::ROUTE,
                &directory::TAG_UPDATE,
                &[Facts::collection(Source::TagNode, Vec::new()),]
            )
            .is_err()
        );
        assert!(
            bind_route(
                OrgMemberList::ROUTE,
                &directory::MEMBER_LIST,
                &[Facts::collection(Source::Members, Vec::new()),]
            )
            .is_err(),
            "the missing domain projection is not an empty source"
        );
        assert!(
            bind_route(
                OrgMemberList::ROUTE,
                &directory::MEMBER_LIST,
                &[
                    Facts::collection(Source::Members, Vec::new()),
                    Facts::collection(Source::MemberDomains, Vec::new()),
                ]
            )
            .is_ok(),
            "a fully selected empty list binds both actual collections"
        );
    }

    #[test]
    fn one_native_role_binds_all_of_its_canonical_required_actions() {
        use orch_access_contract::key_scope::KeyAction::{Read, Write};
        use orch_api_contract::operations::{FacetSpec, OperationSpec};
        const FACETS: &[FacetSpec] = &[
            FacetSpec::required(
                Read,
                &[KeyTargetKind::Object(ObjectKind::Org)],
                TargetResolver::Resolved("tag_node"),
            ),
            FacetSpec::required(
                Write,
                &[KeyTargetKind::Object(ObjectKind::Org)],
                TargetResolver::Resolved("tag_node"),
            ),
        ];
        let mut route = TagUpdate::ROUTE;
        route.operation = Some(OperationSpec::facets(FACETS));
        let declaration = Declaration {
            operation: "TagUpdate",
            bindings: directory::TAG_DELETE.bindings,
        };
        let org = KeyTargetKind::Object(ObjectKind::Org);
        let facts = [Facts::one(Source::TagNode, org, target(org, 1))];
        for actions in [&["read"][..], &["write"][..]] {
            assert!(
                bind_route(route, &declaration, &facts)
                    .expect("both native action facts are bound")
                    .admit(&caller(1, actions))
                    .is_err()
            );
        }
        assert!(
            bind_route(route, &declaration, &facts)
                .expect("both native action facts")
                .admit(&caller(1, &["read", "write"]))
                .is_ok()
        );
    }

    #[test]
    fn undeclared_business_kind_and_duplicate_source_are_rejected() {
        let org = KeyTargetKind::Object(ObjectKind::Org);
        let account = KeyTargetKind::Identity(KeyIdentityKind::Account);
        assert!(
            bind_route(
                TagUpdate::ROUTE,
                &directory::TAG_UPDATE,
                &[Facts::one(Source::TagNode, account, target(org, 1)),]
            )
            .is_err(),
            "business family does not come from the governor"
        );
        assert!(
            bind_route(
                TagUpdate::ROUTE,
                &directory::TAG_UPDATE,
                &[
                    Facts::one(Source::TagNode, org, target(org, 1)),
                    Facts::one(Source::TagNode, org, target(org, 2)),
                ]
            )
            .is_err()
        );
    }

    #[test]
    fn known_preparation_does_not_replace_an_unresolved_required_collection() {
        use orch_api_contract::personal::MeSubscriptionList;
        use orch_api_contract::personal_sources::{self as personal, Source as PersonalSource};
        let account = KeyTargetKind::Identity(KeyIdentityKind::Account);
        let facts = [Facts::one(
            PersonalSource::Account,
            account,
            target(account, 9),
        )];
        let partial = bind_facts(
            MeSubscriptionList::ROUTE,
            &personal::SUBSCRIPTIONS,
            &facts,
            false,
        )
        .expect("known owner is usable for effect-free selection");
        assert!(partial.shared().required_facets().is_err());
        assert!(bind_route(MeSubscriptionList::ROUTE, &personal::SUBSCRIPTIONS, &facts).is_err());
    }

    #[test]
    fn deferred_response_binding_keeps_required_subjects_complete_and_optional_sources_unproven() {
        use orch_access_contract::key_scope::KeyAction::Manage;
        use orch_api_contract::operations::{FacetSpec, OperationSpec};
        const ORG: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Org);
        const OPERATION: OperationSpec = OperationSpec::receipt(&OperationSpec::facets(&[
            FacetSpec::required(Manage, &[ORG], TargetResolver::Resolved("tag_node")),
            FacetSpec::response_content(&[ORG], TargetResolver::Resolved("tag_node")),
            FacetSpec::response_content(&[ORG], TargetResolver::ResolvedCollection("native_names")),
        ]));
        const ROW: Declaration<Source> = Declaration {
            operation: "TagUpdate",
            bindings: &[
                Binding {
                    source: Source::TagNode,
                    target: TargetResolver::Resolved("tag_node"),
                    usage: FacetUsage::Required,
                    cardinality: Cardinality::One,
                },
                Binding {
                    source: Source::TagNode,
                    target: TargetResolver::Resolved("tag_node"),
                    usage: FacetUsage::ResponseContent,
                    cardinality: Cardinality::One,
                },
                Binding {
                    source: Source::Organization,
                    target: TargetResolver::ResolvedCollection("native_names"),
                    usage: FacetUsage::ResponseContent,
                    cardinality: Cardinality::Collection,
                },
            ],
        };
        let mut route = TagUpdate::ROUTE;
        route.operation = Some(OPERATION);
        let facts = [Facts::one(Source::TagNode, ORG, target(ORG, 1))];
        let partial = bind_required_route(route, &ROW, &facts).expect("complete effect subject");
        let admission = partial
            .admit(&caller(1, &["manage", "read"]))
            .expect("a native writer with a real receipt can defer content");
        assert_eq!(admission.disclosure, resolver::Disclosure::Pending);
        assert!(!admission.content);
        assert!(bind_route(route, &ROW, &facts).is_err());
        let empty_names = Facts::collection(Source::Organization, Vec::new());
        assert!(bind_required_route(route, &ROW, std::slice::from_ref(&empty_names)).is_err());
        let mut complete = facts.to_vec();
        complete.push(empty_names);
        assert!(
            bind_route(route, &ROW, &complete)
                .expect("the actual response selector supplied its checked empty collection")
                .admit(&caller(1, &["manage", "read"]))
                .expect("complete same-operation response")
                .content
        );
    }
}
