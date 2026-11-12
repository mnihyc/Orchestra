//! Shared factual Environment binding and response disclosure for existing session closes.

use crate::key_operations::admission::Prepared;

use crate::key_operations::resolver::Disclosure;

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::routes::access::operations::projection_node_names;

use crate::routes::access::page::NameBounds;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_authz::engine::Graph;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_db::Conn;

use orch_types::Id;
const EFFECT: usize = 0;

const CONTENT: usize = 1;

/// Native close and creation projections use their actual session Environment and thread labels.
pub(super) async fn plan(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    environment: Id,
    thread: Option<Id>,
    effect: bool,
) -> ApiResult<(Prepared, NameBounds)> {
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
    )
    .await?;
    let mut plan = Prepared::new(ctx.route, ctx.route.name)?;
    if effect {
        plan.bind(EFFECT, target.object.kind, std::slice::from_ref(&target))?;
    }
    plan.bind(CONTENT, target.object.kind, &[target])?;
    let graph = Graph::load(conn, &thread.into_iter().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let selected = projection_node_names(
        caller,
        &mut plan,
        &graph,
        &[(ObjectKind::Thread, "close_threads")],
    )?;
    Ok((
        plan,
        NameBounds {
            limited: caller
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some()),
            nodes: selected,
            ..Default::default()
        },
    ))
}

/// Current Read may narrow a completed effect to a receipt, never replace its outcome.
pub(super) fn projection<T>(ctx: &Ctx, result: ApiResult<Option<T>>) -> ApiResult<Option<T>> {
    match result {
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
            ctx.complete_disclosure(Disclosure::Denied);
            Ok(None)
        }
        other => other,
    }
}

/// Rich error details use the same operation's current Read and its ordinary Environment door.
pub(super) async fn may_disclose(
    ctx: &Ctx,
    caller: &Caller,
    environment: Id,
    permission: Permission,
) -> bool {
    if ctx.key_receipt_required() {
        return false;
    }
    let result = async {
        let mut conn = ctx.app.db.acquire().await?;
        let (plan, _) = plan(ctx, &mut conn, caller, environment, None, false).await?;
        plan.read_response(ctx, &mut conn, caller, move |conn, reader| {
            Box::pin(async move {
                authorization::require_resource(
                    conn,
                    reader,
                    ResourceKind::Environment,
                    environment,
                    permission,
                    Policy::default(),
                )
                .await
            })
        })
        .await
    }
    .await;
    matches!(projection(ctx, result), Ok(Some(())))
}

pub(super) fn error(ctx: &Ctx, problem: ApiError, disclose: bool) -> ApiError {
    if disclose && !ctx.key_receipt_required() {
        problem
    } else {
        authorization::problem_without_content(problem)
    }
}
