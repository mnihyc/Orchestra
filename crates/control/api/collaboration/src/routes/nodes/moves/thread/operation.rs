//! Actual movement operands and independent current authority of the stored review.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::routes::Endpoint;

use orch_api_contract::thread_move_sources as sources;

use orch_api_contract::thread_move_sources::Source;

use orch_authz::Caller;

use orch_engine::operation_scope::continuing;

pub use orch_api_base::routes::nodes::moves::thread::operation::Base;

pub use orch_api_base::routes::nodes::moves::thread::operation::Association;

pub use orch_api_base::routes::nodes::moves::thread::operation::ReviewSources;

pub use orch_api_base::routes::nodes::moves::thread::operation::Origin;

pub use orch_api_base::routes::nodes::moves::thread::operation::Kept;

pub use orch_api_base::routes::nodes::moves::thread::operation::Confirmation;

pub use orch_api_base::routes::nodes::moves::thread::operation::Prepared;

pub use orch_api_base::routes::nodes::moves::thread::operation::Preview;

pub use orch_api_base::routes::nodes::moves::thread::operation::Move;

pub use orch_api_base::routes::nodes::moves::thread::operation::OriginalGate;

pub use orch_api_base::routes::nodes::moves::thread::operation::check_first;
const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);

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
orch_api_base::__api_bind_df48d8cc75!();

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
orch_api_base::__api_bind_01f987e298!();

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
orch_api_base::__api_bind_8039c90d48!();

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
orch_api_base::__api_bind_8ce72302f8!();
