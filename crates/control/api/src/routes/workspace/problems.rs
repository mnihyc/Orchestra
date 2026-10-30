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

/// What was being done when the machine refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doing {
    /// Reading a directory, a file or what an archive holds.
    Reading,
    /// Writing a file.
    Writing,
    /// Making a directory.
    Making,
    /// Renaming or moving.
    Renaming,
    /// Copying.
    Copying,
    /// Deleting.
    Removing,
    /// Searching the text of files.
    Searching,
    /// Asking about a repository.
    Repository,
    /// Listing or signalling processes.
    Processes,
    /// Attaching an environment.
    Attaching,
}

/// Why a command to a machine did not give a result.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// The machine answered that it would not or could not.
    Machine(CommandError),
    /// The machine did not answer: it is away, or did not take the command up in time.
    Away(String),
    /// Something of the system's own failed.
    Other(String),
}

impl Failure {
    /// The code the machine gave, when it gave one.
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Machine(error) => Some(error.code.as_str()),
            _ => None,
        }
    }

    /// The failure as the problem a client acts on.
    #[must_use]
    pub fn problem(self, doing: Doing) -> ApiError {
        match self {
            Self::Away(why) => ApiError::new(ErrorCode::SourceOffline, why),
            Self::Other(why) => ApiError::internal(why),
            Self::Machine(error) => machine_problem(&error, doing),
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use orch_link_contract::commands::ErrorCode as MachineCode;

    fn refused(code: MachineCode) -> Failure {
        Failure::Machine(CommandError::new(code, "because"))
    }

    #[test]
    fn directory_capacity_refusal_is_available_to_retry_with_its_native_reason() {
        let problem = refused(MachineCode::Busy).problem(Doing::Reading);
        assert_eq!(problem.code(), ErrorCode::EnvironmentUnavailable);
        assert_eq!(
            problem.problem.extra.get("reason"),
            Some(&serde_json::json!("busy"))
        );
        assert_eq!(problem.problem.detail, "because");
    }

    #[test]
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
}
