//! Request IDs, errors, body limits and rate limits.
//!
//! A handler fails with an [`ApiError`], which is a problem of the
//! contract; the answer is written here, with the request's identifier in
//! the header and in the body. A failure nobody foresaw is answered as
//! `internal` and its cause goes to the log, never to the caller.

use axum::response::{IntoResponse, Response};
use http::{HeaderValue, StatusCode, header};
use orch_api_contract::common::{ErrorCode, FieldFault, PROBLEM_JSON, Problem, REQUEST_ID_HEADER};
use orch_db::DbError;

mod words;

/// Why a request was refused or failed.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    /// The problem, as it is answered.
    pub problem: Box<Problem>,
    /// Seconds after which the caller may try again, where that is known.
    pub retry_after: Option<u64>,
    /// Trusted database failure category, never inferred from an HTTP status or serialized.
    database_retry: bool,
}

/// What a handler returns.
pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    /// An error of `code`, saying `detail`.
    #[must_use]
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Problem::new(code, detail).into()
    }

    /// Nothing is there, or the caller may not know that it is.
    #[must_use]
    pub fn not_found() -> Self {
        Problem::not_found().into()
    }

    /// A request with one fault at `path`.
    #[must_use]
    pub fn invalid(path: &str, message: &str) -> Self {
        Problem::invalid_at(path, message).into()
    }

    /// A failure nobody foresaw: logged with its cause, answered without.
    #[must_use]
    pub fn internal(cause: impl std::fmt::Display) -> Self {
        tracing::error!(%cause, "a request failed unexpectedly");
        Problem::new(
            ErrorCode::Internal,
            "something went wrong; the log has the cause",
        )
        .into()
    }

    /// A failure of the engine. A commit refused because what it decided under changed meanwhile is the
    /// caller's to send again, as any write's is (`conflict.stale_revision`); anything else is a failure
    /// nobody foresaw. A database fault of another kind stays the engine's: it is not the request's fault.
    #[must_use]
    pub fn engine(error: orch_engine::EngineError) -> Self {
        match error {
            orch_engine::EngineError::BackgroundRunning(_thread) => Self::new(
                ErrorCode::PlanBlocked,
                "Background tasks are still running. Review and end them before changing this session.",
            )
            .with(
                "blockers",
                [orch_api_contract::forks::PlanBlocker {
                    code: "background_tasks_running".to_owned(),
                    message: "Review and end the running background tasks first.".to_owned(),
                    missing: None,
                }],
            ),
            orch_engine::EngineError::BeforeEffect(error) => match error.downcast::<Self>() {
                Ok(error) => *error,
                Err(error) => match error.downcast::<orch_engine::operation_scope::continuing::Error>() {
                    Ok(error) => match *error {
                        orch_engine::operation_scope::continuing::Error::Refused
                        | orch_engine::operation_scope::continuing::Error::Binding(orch_engine::operation_scope::BindingError::Refused) =>
                            Self::new(ErrorCode::AuthzDenied,"the original native operation no longer permits this work"),
                        orch_engine::operation_scope::continuing::Error::Native(error)=>Self::engine(error),
                        error=>Self::internal(error),
                    },
                    Err(error)=>Self::internal(error),
                },
            },
            orch_engine::EngineError::Db(DbError::Retry)
            | orch_engine::EngineError::Authz(orch_authz::Error::Db(DbError::Retry)) => {
                DbError::Retry.into()
            }
            orch_engine::EngineError::MountFingerprint(reason) => Self::new(
                ErrorCode::PlanBlocked,
                "the requested content check could not finish",
            )
            .with(
                "blockers",
                [orch_api_contract::forks::PlanBlocker {
                    code: "mount_fingerprint_unavailable".into(),
                    message: reason,
                    missing: None,
                }],
            ),
            other => Self::internal(other),
        }
    }

    /// Adds a member that the code defines.
    #[must_use]
    pub fn with(mut self, member: &str, value: impl serde::Serialize) -> Self {
        *self.problem = (*self.problem).clone().with(member, value);
        self
    }

    /// Says when the caller may try again.
    #[must_use]
    pub fn retry_after(mut self, seconds: u64) -> Self {
        self.retry_after = Some(seconds);
        self
    }

    /// The code of the problem.
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        self.problem.code
    }

    /// Replace the entire public content-bearing envelope while retaining this
    /// error's trusted cause category and transport retry metadata.
    #[must_use]
    pub(crate) fn without_content(mut self) -> Self {
        let status = self.problem.status;
        self.problem = Box::new(Problem::new(
            self.code(),
            "the operation could not be completed",
        ));
        self.problem.status = status;
        self
    }

    /// A real database transaction conflict, for explicitly effect-free read retries only.
    #[must_use]
    pub(crate) fn is_database_retry(&self) -> bool {
        self.database_retry
    }

    /// The answer, for the request `request_id`.
    #[must_use]
    pub fn into_response_for(self, request_id: &str) -> Response {
        let (problem, retry_after) = self.into_parts_for(request_id);
        let status =
            StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = serde_json::to_vec(&problem).unwrap_or_default();
        let mut response = (status, body).into_response();
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
        if let Ok(value) = HeaderValue::from_str(request_id) {
            headers.insert(REQUEST_ID_HEADER, value);
        }
        if let Some(seconds) = retry_after {
            headers.insert(header::RETRY_AFTER, HeaderValue::from(seconds));
        }
        response
    }

    /// The same native problem and retry hint used by an HTTP reply, for a bounded opening member.
    pub(crate) fn into_parts_for(mut self, request_id: &str) -> (Problem, Option<u64>) {
        self.problem.request_id = request_id.to_owned();
        words::prepare(&mut self.problem);
        (*self.problem, self.retry_after)
    }
}

impl From<Problem> for ApiError {
    fn from(problem: Problem) -> Self {
        Self {
            problem: Box::new(problem),
            retry_after: None,
            database_retry: false,
        }
    }
}

impl From<DbError> for ApiError {
    fn from(error: DbError) -> Self {
        match error {
            // A handler that expects a clash answers it itself, with the
            // thing that exists; one that reaches here did not expect it.
            DbError::Unique { constraint } => {
                tracing::debug!(%constraint, "a request used a value already taken");
                Self::new(
                    ErrorCode::ConflictExists,
                    "that name or value is taken; choose another",
                )
            }
            DbError::Retry => {
                let mut error = Self::new(
                    ErrorCode::ConflictStaleRevision,
                    "it was changed at the same moment; try again",
                );
                error.database_retry = true;
                error
            }
            // A value a handler let through that the schema's check on its column refuses: the request's
            // fault, at that field, not a failure of the server.
            DbError::Check { table, constraint } => {
                let path = checked_field(&table, &constraint);
                let message = "is not a value it may take";
                Problem::new(ErrorCode::RequestValueRefused, format!("{path}: {message}"))
                    .with(
                        "errors",
                        [FieldFault {
                            path: path.to_owned(),
                            message: message.to_owned(),
                        }],
                    )
                    .into()
            }
            other => Self::internal(other),
        }
    }
}

/// The field a check of the schema is about: `<column>` of a check named `<table>_<column>_check`, as the
/// schema names a check it did not name itself, else the check's name without its table.
fn checked_field<'a>(table: &str, constraint: &'a str) -> &'a str {
    let Some(unprefixed) = (!table.is_empty())
        .then(|| {
            constraint
                .strip_prefix(table)
                .and_then(|rest| rest.strip_prefix('_'))
        })
        .flatten()
    else {
        return constraint;
    };
    unprefixed.strip_suffix("_check").unwrap_or(unprefixed)
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.problem)
    }
}

impl std::error::Error for ApiError {}

#[cfg(test)]
mod check_tests {
    use super::*;

    #[test]
    fn a_value_the_schemas_check_refuses_is_the_requests_fault_at_its_field() {
        let refused = ApiError::from(DbError::Check {
            table: "notification_routes".to_owned(),
            constraint: "notification_routes_unread_delay_minutes_check".to_owned(),
        });
        assert_eq!(refused.problem.code, ErrorCode::RequestValueRefused);
        assert_eq!(refused.problem.code.status(), 422);
        let body = serde_json::to_value(&*refused.problem).unwrap();
        assert_eq!(body["errors"][0]["path"], "unread_delay_minutes");
        assert_eq!(
            checked_field("leases", "leases_shared_recipient"),
            "shared_recipient"
        );
        assert_eq!(checked_field("", "a_named_check"), "a_named_check");
        assert_eq!(checked_field("leases", "a_named_check"), "a_named_check");
        assert_eq!(
            checked_field("lease", "leases_duration_check"),
            "leases_duration_check"
        );
    }

    #[test]
    fn a_known_native_source_refusal_is_an_authorization_answer_without_an_internal_error() {
        let reply = ApiError::engine(orch_engine::EngineError::BeforeEffect(Box::new(
            orch_engine::operation_scope::continuing::Error::Refused,
        )));
        assert_eq!(reply.code(), ErrorCode::AuthzDenied);
        assert!(!reply.is_database_retry());
    }

    #[test]
    fn only_a_typed_database_conflict_marks_an_effect_free_read_for_retry() {
        let conflict = ApiError::from(DbError::Retry);
        let revision = ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "an explicit business revision conflict",
        );
        assert_eq!(conflict.code(), revision.code());
        assert!(conflict.is_database_retry());
        assert!(!revision.is_database_retry());
    }

    #[test]
    fn an_engine_commit_that_lost_a_race_is_the_callers_to_send_again_and_nothing_else_is() {
        let lost = ApiError::engine(orch_engine::EngineError::Db(DbError::Retry));
        assert_eq!(lost.problem.code, ErrorCode::ConflictStaleRevision);
        assert_eq!(lost, ApiError::from(DbError::Retry));
        let broken = ApiError::engine(orch_engine::EngineError::Db(DbError::Check {
            table: "jobs".to_owned(),
            constraint: "jobs_kind_check".to_owned(),
        }));
        assert_eq!(
            broken.problem.code,
            ErrorCode::Internal,
            "a check the engine itself broke is no fault of the request"
        );
        let unreadable = ApiError::engine(orch_engine::EngineError::unreadable("a stored value"));
        assert_eq!(unreadable.problem.code, ErrorCode::Internal);
    }
}
