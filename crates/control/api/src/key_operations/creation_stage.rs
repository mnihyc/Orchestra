//! Rollbackable identity prefixes of the two existing thread-creating operations.
//!
//! A prefix is deliberately not an admitted Caller or a deferred-work origin. The
//! transaction remains private until actual Topic, Environment and Thread identities
//! have supplied every dependent target to the complete shared operation gate.

use super::admission::{Prepared, binding_error};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard::{self, Guard};
use crate::registry::Ctx;
use orch_access_contract::key_scope::{KeyAction, KeyFacet, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::nodes::{CreateThread, ThreadCreate, TopicCreate, TopicCreateBody};
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_api_contract::routes::Endpoint;
use orch_authz::Caller;
use orch_db::{Conn, Tx};
use orch_types::{Id, Time};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Selection {
    Thread,
    TopicWithThread,
}

impl Selection {
    fn of(ctx: &Ctx, operation: &str) -> ApiResult<Self> {
        match (ctx.route.name, operation) {
            (name, "ThreadCreate") if name == ThreadCreate::ROUTE.name => Ok(Self::Thread),
            (name, "TopicCreate.with_thread") if name == TopicCreate::ROUTE.name => {
                Ok(Self::TopicWithThread)
            }
            _ => Err(ApiError::internal("this endpoint has no creator prefix")),
        }
    }

    fn operation(self) -> &'static str {
        match self {
            Self::Thread => "ThreadCreate",
            Self::TopicWithThread => "TopicCreate.with_thread",
        }
    }

    fn parent(self) -> ObjectKind {
        match self {
            Self::Thread => ObjectKind::Topic,
            Self::TopicWithThread => ObjectKind::Project,
        }
    }

    fn family(self) -> ObjectKind {
        match self {
            Self::Thread => ObjectKind::Thread,
            Self::TopicWithThread => ObjectKind::Topic,
        }
    }

    fn permission(self) -> &'static str {
        match self {
            Self::Thread => "topic.start_threads",
            Self::TopicWithThread => "project.create_topics",
        }
    }
}

/// Unconstructible outside this module: only `begin` may enter or complete a prefix guard.
pub(crate) struct PrefixAccess(());

/// Request-local credential ceiling and exact prefix identity. No admission proof is
/// exposed or serializable; only the private staged transaction can consume it.
pub(crate) struct TopicPrefix {
    accepted: Caller,
    parent: Id,
    selection: Selection,
    complete: AtomicBool,
}

impl std::fmt::Debug for TopicPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TopicPrefix([private creation stage])")
    }
}

impl TopicPrefix {
    pub(crate) fn require_complete(&self) -> ApiResult<()> {
        if self.complete.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(ApiError::internal(
                "thread creation did not complete its scope stage",
            ))
        }
    }
}

async fn prefix_facets(
    ctx: &Ctx,
    conn: &mut Conn,
    selection: Selection,
) -> ApiResult<Vec<KeyFacet>> {
    Selection::of(ctx, selection.operation())?;
    let plan = Prepared::new(ctx.route, selection.operation())?;
    let Some(facet) = plan.variant().facets.first() else {
        return Err(ApiError::internal(
            "compound topic creation has no prefix facet",
        ));
    };
    if facet.action != KeyAction::Write
        || facet.usage != FacetUsage::Required
        || facet.target != TargetResolver::PermissionObject
        || facet.resources != [KeyTargetKind::Object(selection.family())]
    {
        return Err(ApiError::internal(
            "compound topic creation prefix metadata changed",
        ));
    }
    let parent = super::targets::permission_object(ctx, conn).await?;
    if parent.object.kind != KeyTargetKind::Object(selection.parent()) {
        return Err(ApiError::not_found());
    }
    orch_engine::operation_scope::facets(
        selection.operation(),
        facet,
        KeyTargetKind::Object(selection.family()),
        &[parent],
    )
    .map_err(binding_error)
}

async fn checked_prefix_caller(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    selection: Selection,
) -> ApiResult<Caller> {
    let plan = Prepared::new(ctx.route, selection.operation())?;
    plan.shared().compatible(caller).map_err(binding_error)?;
    let facets = prefix_facets(ctx, conn, selection).await?;
    caller
        .admit_facets(&facets)
        .map(|proof| proof.into_caller())
        .map_err(|_| {
            ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "the key does not allow this creation prefix",
            )
        })
}

/// Initial, effect-free prefix validation. Missing dependent facts are never marked
/// empty or admitted; the returned object cannot authorize a general business action.
pub(crate) async fn prepare_prefix(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    plan: &Prepared,
) -> ApiResult<TopicPrefix> {
    let selection = Selection::of(ctx, &plan.variant().operation)?;
    if !plan.matches_route(&ctx.route)
        || caller
            .grant()
            .and_then(|grant| grant.operation_scope())
            .is_none()
        || !authorization::same_credential_origin(ctx.user()?, caller)
    {
        return Err(ApiError::internal("an invalid creator prefix was prepared"));
    }
    let checked = checked_prefix_caller(ctx, conn, caller, selection).await?;
    let principal =
        authorization::load_principal(conn, checked, Time::new(ctx.app.clock.now())).await?;
    let principal = crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await?;
    authorization::require_on_path_in(ctx, conn, &principal, selection.permission()).await?;
    Ok(TopicPrefix {
        accepted: caller.clone(),
        parent: ctx.id("id")?,
        selection,
        complete: AtomicBool::new(false),
    })
}

/// Called only by the private prefix guard while its identity/epoch locks are held.
pub(crate) async fn current_prefix_caller(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Caller,
    _access: &PrefixAccess,
) -> ApiResult<Caller> {
    let prefix = ctx
        .topic_creation_prefix()
        .ok_or_else(|| ApiError::internal("the creation prefix is absent"))?;
    if ctx.id("id")? != prefix.parent {
        return Err(ApiError::internal("the creation prefix parent changed"));
    }
    let caller = prefix
        .accepted
        .intersect_restrictions(current)
        .map_err(ApiError::internal)?;
    checked_prefix_caller(ctx, conn, &caller, prefix.selection).await
}

/// The only function that owns an incomplete creation transaction. It performs the
/// existing Topic-only database prefix. Its hardwired factory may create rollbackable
/// Environment/Thread rows through the Engine's private staged writer, but must complete
/// every dependent effect before birth/source/input work. No arbitrary callback is accepted.
/// The transaction is returned only after the complete normal effect gate and ordinary guard.
pub(crate) async fn begin(ctx: &Ctx, body: &TopicCreateBody) -> ApiResult<(Ctx, Tx, Guard, Id)> {
    if !ctx
        .topic_creation_prefix()
        .is_some_and(|prefix| prefix.selection == Selection::TopicWithThread)
        || body.first_thread.is_none()
    {
        return Err(ApiError::internal(
            "the compound topic creation stage was not selected",
        ));
    }
    let access = PrefixAccess(());
    let mut tx = ctx.app.db.begin().await?;
    let prefix_guard = transaction_guard::check_topic_prefix(ctx, &mut tx, &access).await?;
    let made =
        crate::routes::nodes::make::topic_prefix(ctx, &mut tx, prefix_guard.principal(), body)
            .await?;
    let plan = crate::routes::threads::creation_operation::complete_topic(
        ctx,
        &mut tx,
        prefix_guard.principal().caller(),
        made.id,
        body,
    )
    .await?;
    let (complete_ctx, guard) = finish(ctx, &mut tx, prefix_guard, plan, made.id, &access).await?;
    Ok((complete_ctx, tx, guard, made.id))
}

/// Existing-topic counterpart. Environment/Thread prefixes are owned by the one shared
/// creation producer; no incomplete transaction or partial Caller escapes this function.
pub(crate) async fn begin_thread(ctx: &Ctx, body: &CreateThread) -> ApiResult<(Ctx, Tx, Guard)> {
    if !ctx
        .topic_creation_prefix()
        .is_some_and(|prefix| prefix.selection == Selection::Thread)
    {
        return Err(ApiError::internal(
            "the thread creation stage was not selected",
        ));
    }
    let access = PrefixAccess(());
    let mut tx = ctx.app.db.begin().await?;
    let prefix_guard = transaction_guard::check_topic_prefix(ctx, &mut tx, &access).await?;
    let topic = ctx.id("id")?;
    let plan = crate::routes::threads::creation_operation::complete_staged(
        ctx,
        &mut tx,
        prefix_guard.principal().caller(),
        topic,
        body,
    )
    .await?;
    let (complete, guard) = finish(ctx, &mut tx, prefix_guard, plan, topic, &access).await?;
    Ok((complete, tx, guard))
}

async fn finish(
    ctx: &Ctx,
    conn: &mut Conn,
    prefix_guard: Guard,
    plan: Prepared,
    topic: Id,
    access: &PrefixAccess,
) -> ApiResult<(Ctx, Guard)> {
    let prefix = ctx
        .topic_creation_prefix()
        .ok_or_else(|| ApiError::internal("the creation prefix is absent"))?;
    if !plan.matches_route(&ctx.route)
        || plan.variant().operation != prefix.selection.operation()
        || plan.is_topic_creation_prefix()
    {
        return Err(ApiError::internal(
            "the final creation preparation is still a prefix or another selection",
        ));
    }
    let admission = plan.admit(prefix_guard.principal().caller())?;
    if !admission.caller.admitted_facets().is_some_and(|facets| {
        facets.iter().any(|facet| {
            facet.operation == prefix.selection.operation()
                && facet.action == KeyAction::Write
                && facet.resource_kind == KeyTargetKind::Object(ObjectKind::Thread)
                && facet.target.object.kind == KeyTargetKind::Object(ObjectKind::Topic)
                && facet.target.object.id == Some(topic)
        })
    }) {
        return Err(ApiError::internal(
            "the complete creation did not bind its actual Topic",
        ));
    }
    let (complete_ctx, guard) =
        transaction_guard::complete_topic_prefix(ctx, conn, prefix_guard, admission, access)
            .await?;
    prefix.complete.store(true, Ordering::Release);
    Ok((complete_ctx, guard))
}
