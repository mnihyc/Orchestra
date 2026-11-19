//! Read-only prompt/queue pages and explicit guarded creation or selection of native continuations.

use crate::{
    middleware::{
        authorization,
        requests::{ApiError, ApiResult},
        transaction_guard,
    },
    registry::{Ctx, Registry},
};
use orch_api_contract::{
    common::ErrorCode,
    threads::{
        ITEMS_DEFAULT_LIMIT, ITEMS_MAX_LIMIT, ThreadContinuationAccepted,
        ThreadContinuationActivate, ThreadContinuationActivateBody, ThreadInputList,
        ThreadInputListQuery, ThreadInputPage, ThreadPromptVersionCreate,
        ThreadPromptVersionCreateBody, ThreadPromptVersionList, ThreadPromptVersionListQuery,
        ThreadPromptVersionsPage,
    },
};
use orch_db::queries::{thread_continuations as rows, threads};
use orch_engine::continuations::{self, projection};

pub(super) fn mount(registry: &mut Registry) {
    registry
        .on(ThreadPromptVersionList, versions)
        .on(ThreadPromptVersionCreate, edit)
        .on(ThreadContinuationActivate, activate)
        .on(ThreadInputList, inputs);
}

pub(super) async fn refused(ctx: &Ctx, error: continuations::Error) -> ApiError {
    let thread = match ctx.id("id") {
        Ok(thread) => thread,
        Err(error) => return error,
    };
    match error {
        continuations::Error::Input(error) => match ctx.app.db.acquire().await {
            Ok(mut conn) => super::input_failed(&mut conn, thread, error).await,
            Err(error) => error.into(),
        },
        other => plain_refusal(other),
    }
}

/// Reads already own a snapshot connection; refusal projection must use that same connection.
pub(super) async fn refused_in(
    conn: &mut orch_db::Conn,
    thread: orch_types::Id,
    error: continuations::Error,
) -> ApiError {
    match error {
        continuations::Error::Input(error) => super::input_failed(conn, thread, error).await,
        other => plain_refusal(other),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct FailureFacts {
    category: &'static str,
    sqlstate: Option<String>,
    constraint: Option<String>,
}

fn schema_name(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
    .then(|| value.to_owned())
}

/// Classify internal failures without formatting SQL, row values, prompts or credentials.
fn failure_facts(error: &orch_engine::error::EngineError) -> FailureFacts {
    use orch_db::DbError;
    let mut facts = FailureFacts {
        category: "engine",
        sqlstate: None,
        constraint: None,
    };
    if let orch_engine::error::EngineError::Db(error) = error {
        facts.category = "database";
        let (state, constraint) = match error {
            DbError::Unique { constraint } => (Some("23505".to_owned()), Some(constraint.as_str())),
            DbError::Reference { constraint } => {
                (Some("23503".to_owned()), Some(constraint.as_str()))
            }
            DbError::Check { constraint, .. } => {
                (Some("23514".to_owned()), Some(constraint.as_str()))
            }
            DbError::Rule { constraint } => (None, Some(constraint.as_str())),
            DbError::Retry => (None, None),
            DbError::Other(error) => match error.as_database_error() {
                Some(database) => (
                    database
                        .code()
                        .filter(|code| {
                            code.len() == 5
                                && code
                                    .bytes()
                                    .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
                        })
                        .map(|code| code.into_owned()),
                    database.constraint(),
                ),
                None => (None, None),
            },
        };
        facts.sqlstate = state;
        facts.constraint = constraint.and_then(schema_name);
    }
    facts
}

fn internal_refusal(phase: &'static str, error: orch_engine::error::EngineError) -> ApiError {
    let facts = failure_facts(&error);
    tracing::error!(
        phase,
        category = facts.category,
        sqlstate = facts.sqlstate.as_deref().unwrap_or("unknown"),
        constraint = facts.constraint.as_deref().unwrap_or("unknown"),
        "a continuation request failed internally"
    );
    ApiError::new(
        ErrorCode::Internal,
        "something went wrong; the log has the cause",
    )
}

async fn admission_refusal(
    ctx: &Ctx,
    phase: &'static str,
    error: continuations::Error,
) -> ApiError {
    match error {
        continuations::Error::Internal(error) => internal_refusal(phase, error),
        other => refused(ctx, other).await,
    }
}

fn plain_refusal(error: continuations::Error) -> ApiError {
    match error {
        continuations::Error::NotFound => ApiError::not_found(),
        continuations::Error::NotAuthor => ApiError::new(
            ErrorCode::AuthzDenied,
            "Only the person who sent these words can edit them.",
        ),
        continuations::Error::Invalid(detail) => ApiError::invalid("", detail),
        continuations::Error::Conflict(detail) => {
            ApiError::new(ErrorCode::ConflictStaleRevision, detail)
        }
        continuations::Error::Unsupported(detail) => {
            ApiError::new(ErrorCode::RequestValueRefused, detail)
                .with("reason", "native_boundary_unsupported")
        }
        continuations::Error::Internal(error) => internal_refusal("continuation_projection", error),
        continuations::Error::Input(_) => ApiError::new(
            ErrorCode::AuthzDenied,
            "This input requires current interaction and provider access.",
        ),
    }
}

async fn versions(
    ctx: Ctx,
    query: ThreadPromptVersionListQuery,
    (): (),
) -> ApiResult<ThreadPromptVersionsPage> {
    let (thread, input) = (ctx.id("id")?, ctx.id("input_id")?);
    let mut tx = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let result =
        projection::versions(&ctx.app.engine, &mut tx, &principal, thread, input, &query).await;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    authorization::require_on_path(&ctx, "thread.view").await?;
    match result {
        Ok(result) => Ok(result),
        Err(error) => Err(refused(&ctx, error).await),
    }
}

async fn edit(
    ctx: Ctx,
    (): (),
    body: ThreadPromptVersionCreateBody,
) -> ApiResult<ThreadContinuationAccepted> {
    let (thread, input) = (ctx.id("id")?, ctx.id("input_id")?);
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    match ctx
        .app
        .engine
        .create_prompt_version_guarded(tx, guard.into_principal(), thread, input, body)
        .await
    {
        Ok(result) => Ok(result),
        Err(error) => Err(admission_refusal(&ctx, "prompt_edit_admission", error).await),
    }
}

async fn activate(
    ctx: Ctx,
    (): (),
    body: ThreadContinuationActivateBody,
) -> ApiResult<ThreadContinuationAccepted> {
    let (thread, target) = (ctx.id("id")?, ctx.id("continuation_id")?);
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    match ctx
        .app
        .engine
        .activate_continuation_guarded(tx, guard.into_principal(), thread, target, body)
        .await
    {
        Ok(result) => Ok(result),
        Err(error) => Err(admission_refusal(&ctx, "continuation_activate_admission", error).await),
    }
}

async fn inputs(ctx: Ctx, query: ThreadInputListQuery, (): ()) -> ApiResult<ThreadInputPage> {
    let thread = ctx.id("id")?;
    let limit = i64::from(super::read::limit_of(
        query.limit,
        ITEMS_DEFAULT_LIMIT,
        ITEMS_MAX_LIMIT,
    )?);
    let mut tx = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let head = threads::thread(&mut tx, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let scope = rows::projection(&mut tx, thread, query.continuation_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !scope.accepts_cursor(query.after.is_some(), query.projection_revision) {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "The viewed continuation changed; reload this queue page.",
        ));
    }
    let own = rows::continuation(&mut tx, thread, scope.viewed_continuation_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if query.after.is_some() && query.queue_revision != Some(own.queue_revision) {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "This continuation's pending-input order changed; reload its queue.",
        ));
    }
    let mut inputs =
        rows::reading::open_inputs(&mut tx, thread, own.id, query.after, limit + 1).await?;
    let extra = i64::try_from(inputs.len()).unwrap_or(i64::MAX) > limit;
    if extra {
        inputs.pop();
    }
    let next_after = extra.then(|| inputs.last().map(|input| input.id)).flatten();
    let people = orch_engine::views::People::load(
        &mut tx,
        inputs
            .iter()
            .flat_map(|input| input.author_id.into_iter().chain(input.run_as)),
    )
    .await
    .map_err(ApiError::internal)?;
    let people = people
        .with_approvals(&mut tx, inputs.iter().filter_map(|input| input.approval_id))
        .await
        .map_err(ApiError::internal)?;
    let inputs = inputs
        .iter()
        .map(|input| orch_engine::views::input_view(input, &people))
        .collect::<Result<Vec<_>, _>>()
        .map_err(ApiError::internal)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    authorization::require_on_path(&ctx, "thread.view").await?;
    Ok(ThreadInputPage {
        projection: projection::scope(&scope),
        inputs,
        next_after,
        queue_revision: own.queue_revision,
        seq: head.seq(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_admission_check_failure_retains_only_phase_and_safe_schema_identity() {
        let error = orch_engine::error::EngineError::Db(orch_db::DbError::Check {
            table: "jobs".into(),
            constraint: "jobs_kind_check".into(),
        });
        assert_eq!(
            failure_facts(&error),
            FailureFacts {
                category: "database",
                sqlstate: Some("23514".into()),
                constraint: Some("jobs_kind_check".into())
            }
        );
        let refused = internal_refusal("prompt_edit_admission", error);
        assert_eq!(refused.problem.code, ErrorCode::Internal);
        assert!(refused.problem.extra.is_empty());
        assert_eq!(
            refused.problem.detail,
            "something went wrong; the log has the cause"
        );
    }

    #[test]
    fn internal_diagnostics_do_not_format_row_values_or_arbitrary_error_text() {
        let error = orch_engine::error::EngineError::Db(orch_db::DbError::Check {
            table: "synthetic row contents must not be printed".into(),
            constraint: "untrusted\nsynthetic content".into(),
        });
        assert_eq!(
            failure_facts(&error),
            FailureFacts {
                category: "database",
                sqlstate: Some("23514".into()),
                constraint: None
            }
        );
        assert_eq!(
            failure_facts(&orch_engine::error::EngineError::Unreadable(
                "synthetic private text".into()
            )),
            FailureFacts {
                category: "engine",
                sqlstate: None,
                constraint: None
            }
        );
        assert_eq!(schema_name(&"x".repeat(64)), None);
    }
}
