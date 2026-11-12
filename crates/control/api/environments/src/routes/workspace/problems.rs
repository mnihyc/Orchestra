//! What a machine's refusal is as a problem of the API: the code a machine answers with, and what was being done, tell which.
//!
//! A machine says why it did not do what it was asked in the codes of the link; the same code means
//! another thing to a person for each request. That a destination is there already is a conflict of
//! names for a rename and a copy, and a version that was not the one expected for a write. Paths
//! the ignore rules leave out, and paths that are not there, are not there for anybody. A machine
//! that did not answer is a source that is offline.

use crate::middleware::requests::ApiError;

use orch_api_contract::common::ErrorCode;

use orch_link_contract::commands::CommandError;

pub use orch_api_base::routes::workspace::problems::Doing;

pub use orch_api_base::routes::workspace::problems::Failure;
fn machine_problem(error: &CommandError, doing: Doing) -> ApiError {
    let detail = if error.detail.is_empty() {
        error.code.clone()
    } else {
        error.detail.clone()
    };
    let problem = match error.code.as_str() {
        "not_found" | "ignored" => ApiError::not_found(),
        "conflict" => match doing {
            Doing::Writing => ApiError::new(ErrorCode::ConflictStaleRevision, detail),
            Doing::Making | Doing::Renaming | Doing::Copying | Doing::Removing => {
                ApiError::new(ErrorCode::ConflictExists, detail)
            }
            _ => ApiError::new(ErrorCode::ConflictStaleRevision, detail),
        },
        "changed" => ApiError::new(ErrorCode::ConflictStaleRevision, detail),
        "too_large" => ApiError::new(ErrorCode::RequestTooLarge, detail),
        "unsupported" => match doing {
            Doing::Searching => ApiError::new(ErrorCode::RequestInvalid, detail),
            _ => ApiError::new(ErrorCode::CapabilityUnsupported, detail),
        },
        "policy.remote_write_disabled" => {
            ApiError::new(ErrorCode::PolicyRemoteWriteDisabled, detail)
        }
        "policy.deliver_files_disabled" => {
            ApiError::new(ErrorCode::WorkspaceRemoteWriteDisabled, detail)
        }
        "offline_peer" => ApiError::new(ErrorCode::SourceOffline, detail),
        "busy" | "timeout" | "policy.root_not_allowed" => {
            ApiError::new(ErrorCode::EnvironmentUnavailable, detail)
        }
        _ => ApiError::internal(format!(
            "a machine failed a command: {}: {}",
            error.code, error.detail
        )),
    };
    // Recovery controls use the structured cause; the public sentence is catalogued at the
    // response boundary and must not be parsed to rediscover a timeout or a size limit.
    problem.with("reason", &error.code)
}
orch_api_base::__api_bind_a1ebe93450!();

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;

    use orch_link_contract::commands::ErrorCode as MachineCode;
    fn refused(code: MachineCode) -> Failure {
        Failure::Machine(CommandError::new(code, "because"))
    }

    #[cfg(any(test, feature = "test-support"))]
    fn directory_capacity_refusal_is_available_to_retry_with_its_native_reason() {
        let problem = refused(MachineCode::Busy).problem(Doing::Reading);
        assert_eq!(problem.code(), ErrorCode::EnvironmentUnavailable);
        assert_eq!(
            problem.problem.extra.get("reason"),
            Some(&serde_json::json!("busy"))
        );
        assert_eq!(problem.problem.detail, "because");
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_directory_capacity_refusal_is_available_to_retry_with_its_native_reason() {
        self::directory_capacity_refusal_is_available_to_retry_with_its_native_reason()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn the_same_refusal_of_a_machine_is_another_problem_for_each_thing_asked() {
        assert_eq!(
            refused(MachineCode::Conflict)
                .problem(Doing::Writing)
                .code(),
            ErrorCode::ConflictStaleRevision
        );
        assert_eq!(
            refused(MachineCode::Conflict)
                .problem(Doing::Renaming)
                .code(),
            ErrorCode::ConflictExists
        );
        assert_eq!(
            refused(MachineCode::Ignored).problem(Doing::Reading).code(),
            ErrorCode::NotFound
        );
        assert_eq!(
            refused(MachineCode::PolicyRemoteWriteDisabled)
                .problem(Doing::Writing)
                .code(),
            ErrorCode::PolicyRemoteWriteDisabled
        );
        assert_eq!(
            Failure::Away("gone".into()).problem(Doing::Reading).code(),
            ErrorCode::SourceOffline
        );
        assert_eq!(
            refused(MachineCode::TooLarge)
                .problem(Doing::Copying)
                .code(),
            ErrorCode::RequestTooLarge
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_the_same_refusal_of_a_machine_is_another_problem_for_each_thing_asked() {
        self::the_same_refusal_of_a_machine_is_another_problem_for_each_thing_asked()
    }
}
