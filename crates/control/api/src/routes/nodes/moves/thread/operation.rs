//! Actual movement operands and independent current authority of the stored review.

use super::*;
use crate::key_operations::native::effects::{Effect, FirstEffect};
use crate::key_operations::{native, targets};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::routes::Endpoint;
use orch_api_contract::thread_move_sources::{self as sources, Source};
use orch_authz::{Caller, CapturedOperation};
use orch_engine::operation_scope::continuing;
use serde::{Deserialize, Serialize};

const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);

#[derive(Clone)]
pub(super) struct Base {
    pub row: threads::ThreadRow,
    pub lanes: Vec<threads::ThreadRow>,
    pub target: Id,
    pub target_kind: ObjectKind,
    pub required: Vec<native::Facts<Source>>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Association {
    id: Id,
    topic: Id,
    parent: Option<Id>,
    side_of: Option<Id>,
    environment: Option<Id>,
    segment: Option<Id>,
    agent: Option<Id>,
    provider: Option<Id>,
}
impl Association {
    fn of(row: &threads::ThreadRow) -> Self {
        Self {
            id: row.node_id,
            topic: row.topic_id,
            parent: row.parent_id,
            side_of: row.side_of,
            environment: row.environment_id,
            segment: row.current_segment_id,
            agent: row.agent_id,
            provider: row.provider_id,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct ReviewSources {
    pub main: Id,
    pub target: Id,
    pub from: Id,
    pub lanes: Vec<Association>,
    pub content: Vec<(ObjectKind, Id)>,
}
impl ReviewSources {
    pub fn preserves(&self, base: &Base) -> bool {
        self.main == base.row.node_id
            && self.target == base.target
            && self.from == base.row.topic_id
            && self.lanes == base.lanes.iter().map(Association::of).collect::<Vec<_>>()
    }
    pub fn of(base: &Base, mut content: Vec<(ObjectKind, Id)>) -> Self {
        content.extend([
            (ObjectKind::Topic, base.row.topic_id),
            (base.target_kind, base.target),
        ]);
        if let Some(environment) = base.row.environment_id {
            content.push((ObjectKind::Environment, environment));
        }
        content.sort_unstable();
        content.dedup();
        Self {
            main: base.row.node_id,
            target: base.target,
            from: base.row.topic_id,
            lanes: base.lanes.iter().map(Association::of).collect(),
            content,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Origin {
    pub operation: CapturedOperation,
    pub session: Option<Id>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Kept {
    pub review: ThreadMovePlan,
    pub fingerprint: String,
    #[serde(default)]
    pub sources: Option<ReviewSources>,
    #[serde(default)]
    pub origin: Option<Origin>,
}

pub(super) struct Confirmation {
    pub body: ThreadMoveBody,
    pub kept: Kept,
    pub until: Time,
}
pub(super) struct Prepared {
    pub base: Base,
    pub confirmation: Option<Confirmation>,
}

pub(super) async fn base(conn: &mut Conn, main: Id, target: Id) -> ApiResult<Base> {
    let row = threads::thread(conn, main)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    let root = targets::node(conn, main).await?;
    if root.object.kind != THREAD {
        return Err(ApiError::not_found());
    }
    let destination = targets::node(conn, target).await?;
    let kind = match destination.object.kind {
        KeyTargetKind::Object(kind) => kind,
        _ => return Err(ApiError::not_found()),
    };
    let receives =
        super::super::permission_to_receive(NodeKind::Thread, Some(kind.as_str())).is_some();
    let lanes = thread_move::lanes(conn, main)
        .await
        .map_err(ApiError::engine)?;
    let ids = lanes
        .iter()
        .filter(|lane| lane.node_id != main)
        .map(|lane| lane.node_id)
        .collect::<Vec<_>>();
    let graph = orch_authz::engine::Graph::load(conn, &ids)
        .await
        .map_err(ApiError::internal)?;
    let mut sides = Vec::new();
    for id in ids {
        let target = orch_engine::operation_scope::targets::from_node_identity(&graph, id)
            .filter(|target| target.object.kind == THREAD)
            .ok_or_else(ApiError::not_found)?;
        sides.push(target);
    }
    let required = vec![
        native::Facts::one(Source::Main, THREAD, root),
        native::Facts::collection(
            Source::Sides,
            if sides.is_empty() {
                Vec::new()
            } else {
                vec![(THREAD, sides)]
            },
        ),
        native::Facts::collection(
            Source::Destination,
            if receives {
                vec![(THREAD, vec![destination])]
            } else {
                Vec::new()
            },
        ),
    ];
    Ok(Base {
        row,
        lanes,
        target,
        target_kind: kind,
        required,
    })
}

async fn require(conn: &mut Conn, principal: &Principal, value: &Base) -> ApiResult<()> {
    ordinary(conn, principal, value.row.node_id).await?;
    may_move(
        conn,
        principal,
        NodeKind::Thread,
        value.row.node_id,
        value.target,
    )
    .await
}

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    target: Id,
    body: Option<ThreadMoveBody>,
) -> ApiResult<(Prepared, Vec<native::Facts<Source>>)> {
    let value = base(conn, ctx.id("id")?, target).await?;
    let declaration = if body.is_some() {
        &sources::MOVE
    } else {
        &sources::PREVIEW
    };
    let known = native::bind_required(ctx, declaration, &value.required)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    require(conn, &principal, &value).await?;
    let confirmation = if let Some(body) = body {
        let saved = jobs::forks::preflight(conn, body.preflight_id)
            .await?
            .filter(|saved| {
                saved.kind == KIND
                    && saved.node_id == Some(value.row.node_id)
                    && saved.created_by == principal.user()
            })
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::PreflightExpired,
                    "the stored thread move review is no longer available",
                )
            })?;
        if saved.expires_at <= ctx.app.clock.now() {
            return Err(ApiError::new(
                ErrorCode::PreflightExpired,
                "the thread move review expired; read it again",
            ));
        }
        let kept: Kept = serde_json::from_value(saved.plan).map_err(ApiError::internal)?;
        if kept.origin.is_some() != kept.sources.is_some()
            || kept.origin.as_ref().is_some_and(|origin| {
                origin.operation.user() != saved.created_by
                    || origin.operation.operation() != sources::PREVIEW.operation
            })
        {
            return Err(refused());
        }
        if kept.review.movement.to.id != body.parent_id
            || kept.review.expected_revision != body.expected_revision
        {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the destination or displayed revision differs from the stored review",
            ));
        }
        if kept
            .sources
            .as_ref()
            .is_some_and(|sources| !sources.preserves(&value))
        {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the reviewed conversation or side-lane association changed",
            ));
        }
        Some(Confirmation {
            body,
            kept,
            until: Time::new(saved.expires_at),
        })
    } else {
        None
    };
    let facts = value.required.clone();
    Ok((
        Prepared {
            base: value,
            confirmation,
        },
        facts,
    ))
}

pub(super) struct Preview;
pub(super) struct Move;

impl Effect for Preview {
    type Source = Source;
    type Query = ();
    type Body = ThreadMovePreviewBody;
    type Value = Prepared;
    type Output = super::Reviewed;
    fn declaration(_value: &Prepared) -> &'static Declaration<Source> {
        &sources::PREVIEW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ThreadMovePreviewBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare(ctx, conn, caller, body.parent_id, None))
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(conn, principal, &value.base))
    }
}
impl Effect for Move {
    type Source = Source;
    type Query = ();
    type Body = ThreadMoveBody;
    type Value = Prepared;
    type Output = super::MovedThread;
    fn declaration(_value: &Prepared) -> &'static Declaration<Source> {
        &sources::MOVE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ThreadMoveBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare(ctx, conn, caller, body.parent_id, Some(body)))
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(conn, principal, &value.base))
    }
}

struct Original;
struct OriginalInput {
    sources: ReviewSources,
    session: Option<Id>,
}
struct OriginalValue {
    base: Base,
    session_until: Option<Time>,
}
impl continuing::Source for Original {
    type Role = Source;
    type Input = OriginalInput;
    type Value = OriginalValue;
    const ROUTE: orch_api_contract::routes::Route = ThreadMovePreview::ROUTE;
    const DECLARATION: &'static Declaration<Source> = &sources::PREVIEW;
    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a OriginalInput,
        now: Time,
    ) -> BoxFuture<'a, Result<(OriginalValue, Vec<native::Facts<Source>>), continuing::Error>> {
        Box::pin(async move {
            let base = base(conn, input.sources.main, input.sources.target)
                .await
                .map_err(original_error)?;
            if !input.sources.preserves(&base) {
                return Err(continuing::Error::Refused);
            }
            let session_until = match (caller.grant(), input.session) {
                (Some(_), None) => None,
                (None, Some(session)) => {
                    let (admin, end) =
                        orch_db::queries::tokens::lock_session_state(conn, session, caller.user())
                            .await?
                            .filter(|(_, end)| *end > now.get())
                            .ok_or(continuing::Error::Refused)?;
                    if caller.as_admin() && !admin {
                        return Err(continuing::Error::Refused);
                    }
                    Some(Time::new(end))
                }
                _ => return Err(continuing::Error::Refused),
            };
            let facts = base.required.clone();
            Ok((
                OriginalValue {
                    base,
                    session_until,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a OriginalInput,
        value: &'a OriginalValue,
    ) -> BoxFuture<'a, Result<(), continuing::Error>> {
        Box::pin(async move {
            require(conn, principal, &value.base)
                .await
                .map_err(original_error)
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a OriginalInput,
        value: &'a OriginalValue,
    ) -> BoxFuture<'a, Result<(), continuing::Error>> {
        Box::pin(async move {
            super::locks(
                conn,
                value.base.row.org_id,
                value.base.row.node_id,
                value.base.target,
            )
            .await
            .map_err(original_error)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a OriginalInput,
        old: &'a OriginalValue,
        fresh: &'a OriginalValue,
    ) -> BoxFuture<'a, Result<bool, continuing::Error>> {
        Box::pin(async move {
            Ok(input.sources.preserves(&old.base) && input.sources.preserves(&fresh.base))
        })
    }
}
fn original_error(error: ApiError) -> continuing::Error {
    continuing::Error::Native(orch_engine::EngineError::BeforeEffect(Box::new(error)))
}
fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the original move review no longer permits this confirmation",
    )
}

pub(super) struct OriginalGate {
    // Absence is limited to a pre-upgrade review with neither provenance field.
    // It grants no original authority; the real native expiry still vetoes work.
    caller: Option<Caller>,
    session_until: Option<Time>,
    plan_until: Time,
}
impl OriginalGate {
    pub fn check(&self, ctx: &Ctx) -> ApiResult<()> {
        if self.plan_until <= ctx.app.clock.time() {
            return Err(ApiError::new(
                ErrorCode::PreflightExpired,
                "the thread move review expired; read it again",
            ));
        }
        if self
            .caller
            .as_ref()
            .is_some_and(|caller| !caller.key_deadline_allows(ctx.app.clock.as_ref()))
            || self
                .session_until
                .is_some_and(|end| end <= ctx.app.clock.time())
        {
            return Err(refused());
        }
        Ok(())
    }
}
pub(super) async fn original(
    ctx: &Ctx,
    conn: &mut Conn,
    confirmation: &Confirmation,
) -> ApiResult<OriginalGate> {
    let Some(origin) = &confirmation.kept.origin else {
        return if confirmation.kept.sources.is_none() {
            Ok(OriginalGate {
                caller: None,
                session_until: None,
                plan_until: confirmation.until,
            })
        } else {
            Err(refused())
        };
    };
    if origin.operation.user() != ctx.user()?.id() {
        return Err(refused());
    }
    let sources = confirmation.kept.sources.clone().ok_or_else(refused)?;
    let frame = native::continuing::prepare::<Original>(
        conn,
        ctx.app.clock.as_ref(),
        &origin.operation,
        OriginalInput {
            sources,
            session: origin.session,
        },
    )
    .await
    .map_err(|error| match error {
        continuing::Error::Native(error) => ApiError::engine(error),
        _ => refused(),
    })?;
    let gate = OriginalGate {
        caller: Some(frame.principal().caller().clone()),
        session_until: frame.value().session_until,
        plan_until: confirmation.until,
    };
    gate.check(ctx)?;
    Ok(gate)
}

pub(super) fn check_first(
    ctx: &Ctx,
    first: &mut Option<FirstEffect<'_>>,
    original: Option<&OriginalGate>,
) -> Result<(), orch_engine::EngineError> {
    if let Some(original) = original {
        original
            .check(ctx)
            .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))?;
    }
    if let Some(first) = first.take() {
        first
            .check()
            .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))?;
    }
    Ok(())
}
