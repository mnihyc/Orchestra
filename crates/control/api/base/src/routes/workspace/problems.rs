//! What a machine's refusal is as a problem of the API: the code a machine answers with, and what was being done, tell which.
//!
//! A machine says why it did not do what it was asked in the codes of the link; the same code means
//! another thing to a person for each request. That a destination is there already is a conflict of
//! names for a rename and a copy, and a version that was not the one expected for a write. Paths
//! the ignore rules leave out, and paths that are not there, are not there for anybody. A machine
//! that did not answer is a source that is offline.

pub use crate::middleware::requests::ApiError;

pub use orch_api_contract::common::ErrorCode;

pub use orch_link_contract::commands::CommandError;
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

orch_api_base::api_family_callback! {
    bind __api_bind_a1ebe93450, __api_install_a1ebe93450, __api_callback_a1ebe93450;
    slot __ApiCallbacka1ebe93450, __API_CALLBACK_A1EBE93450, __api_set_a1ebe93450;
    base orch_api_base::routes::workspace::problems;

    fn machine_problem<'__api>(arg0: &'__api orch_link_contract::commands::CommandError, arg1: orch_api_base::routes::workspace::problems::Doing) -> orch_api_base::middleware::requests::ApiError;
}
