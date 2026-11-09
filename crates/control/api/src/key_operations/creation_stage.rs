//! Closed rollbackable identity prefixes of native Thread creators and adoption.
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
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::nodes::{
    CreateThread, ThreadAdopt, ThreadAdoptBody, ThreadCreate, TopicCreate, TopicCreateBody,
};
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
    Adoption,
}

impl Selection {
    fn of(ctx: &Ctx, operation: &str) -> ApiResult<Self> {
        match (ctx.route.name, operation) {
            (name, "ThreadCreate") if name == ThreadCreate::ROUTE.name => Ok(Self::Thread),
            (name, "ThreadAdopt") if name == ThreadAdopt::ROUTE.name => Ok(Self::Adoption),
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
            Self::Adoption => "ThreadAdopt",
        }
    }

    fn parent(self) -> ObjectKind {
        match self {
            Self::Thread | Self::Adoption => ObjectKind::Topic,
            Self::TopicWithThread => ObjectKind::Project,
        }
    }

    fn family(self) -> ObjectKind {
        match self {
            Self::Thread | Self::Adoption => ObjectKind::Thread,
            Self::TopicWithThread => ObjectKind::Topic,
        }
    }

    fn permission(self) -> &'static str {
        match self {
            Self::Thread | Self::Adoption => "topic.start_threads",
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
    let target = match selection {
        Selection::Adoption => TargetResolver::TypedPath {
            parameter: "id",
            kind: KeyTargetKind::Object(ObjectKind::Topic),
        },
        Selection::TopicWithThread | Selection::Thread => TargetResolver::PermissionObject,
    };
    if facet.action != KeyAction::Write
        || facet.usage != FacetUsage::Required
        || facet.target != target
        || facet.resources != [KeyTargetKind::Object(selection.family())]
    {
        return Err(ApiError::internal(
            "compound topic creation prefix metadata changed",
        ));
    }
    let parent = if selection == Selection::Adoption {
        // The compound native rule is not a permission-string resolver. This closed
        // creator knows its actual path Topic and validates that stored native kind.
        super::targets::node(conn, ctx.id("id")?).await?
    } else {
        super::targets::permission_object(ctx, conn).await?
    };
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

async fn require_adoption_parent(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &orch_authz::Principal,
) -> ApiResult<()> {
    authorization::require(
        conn,
        principal,
        ctx.id("id")?,
        authorization::exact(Permission::TopicStartThreads),
        orch_authz::Act::Use,
        orch_authz::Policy::default(),
    )
    .await
}

/// The compound adoption rule needs the same current parent prerequisite that the
/// existing simple creators retain through their native Permission route gate.
pub(crate) async fn require_prefix_parent(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    _access: &PrefixAccess,
) -> ApiResult<()> {
    let prefix = ctx
        .topic_creation_prefix()
        .ok_or_else(|| ApiError::internal("the creation prefix is absent"))?;
    if prefix.selection == Selection::Adoption {
        require_adoption_parent(ctx, conn, principal).await?;
    }
    Ok(())
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
    let accepted = plan.creation_prefix_restrictions(caller)?;
    let checked = checked_prefix_caller(ctx, conn, &accepted, selection).await?;
    let principal =
        authorization::load_principal(conn, checked, Time::new(ctx.app.clock.now())).await?;
    let principal = crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await?;
    if selection == Selection::Adoption {
        require_adoption_parent(ctx, conn, &principal).await?;
    } else {
        authorization::require_on_path_in(ctx, conn, &principal, selection.permission()).await?;
    }
    Ok(TopicPrefix {
        accepted,
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
    let accepted = orch_authz::CapturedCaller::capture(&prefix.accepted)
        .restore_current(conn, ctx.app.clock.as_ref())
        .await
        .map_err(authorization::restoration_error)?
        .ok_or_else(|| {
            ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "the original creation prefix is closed",
            )
        })?;
    let caller = accepted
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

/// The only private native adoption identity prefix. The once-only observed supplier
/// is consumed by its hardwired factory; no callback, incomplete transaction or partial
/// caller escapes before full actual creator admission and the existing action guard.
pub(crate) async fn begin_adoption(
    ctx: &Ctx,
    body: &ThreadAdoptBody,
) -> ApiResult<(Ctx, Tx, Guard)> {
    if !ctx
        .topic_creation_prefix()
        .is_some_and(|prefix| prefix.selection == Selection::Adoption)
    {
        return Err(ApiError::internal(
            "the native adoption creation stage was not selected",
        ));
    }
    let access = PrefixAccess(());
    let mut tx = ctx.app.db.begin().await?;
    let prefix_guard = transaction_guard::check_topic_prefix(ctx, &mut tx, &access).await?;
    let topic = ctx.id("id")?;
    let plan = crate::routes::threads::adopt::operation::complete_staged(
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
