//! What can go wrong inside the engine, and does not belong to one of its requests.
//!
//! Each request of the engine has its own error type, which names what the
//! caller can act on (a refusal, a missing thing, a conflict). What is left
//! is the failure of a part the engine stands on: the database, the access
//! engine, the settings, the link. Those are not the caller's to repair and
//! travel together as an [`EngineError`], which a request type wraps as its
//! `Internal` case.

use crate::access::AccessFault;
use crate::commands::CommandsError;
use orch_db::DbError;

/// A part the engine stands on failed, or what it stores does not read.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// A requested informational mount comparison could not read one of its locations.
    #[error("{0}")]
    MountFingerprint(String),
    /// A fresh own-tool call no longer belongs to the selected active execution.
    #[error(
        "This call belongs to a turn that is no longer selected or running. No further work was admitted; make a new call from the current turn."
    )]
    StaleToolSource,
    /// A logical call reference already has another retained source or argument identity.
    #[error(
        "This call reference already identifies different work. No new work was admitted; use a new call from the current turn."
    )]
    ToolCallIdentity,
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The access engine could not answer.
    #[error(transparent)]
    Authz(#[from] orch_authz::Error),
    /// The resource side of the access engine could not answer.
    #[error(transparent)]
    Access(#[from] AccessFault),
    /// The settings could not be read.
    #[error(transparent)]
    Settings(#[from] orch_settings::Error),
    /// A command could not be sent, or its end could not be learned.
    #[error(transparent)]
    Commands(#[from] CommandsError),
    /// Something stored, or something a machine said, does not read as what
    /// it should be.
    #[error("{0}")]
    Unreadable(String),
}

impl EngineError {
    /// A stored value or a machine's word that does not read.
    pub fn unreadable(what: impl std::fmt::Display) -> Self {
        Self::Unreadable(what.to_string())
    }
}

impl From<serde_json::Error> for EngineError {
    fn from(error: serde_json::Error) -> Self {
        Self::Unreadable(error.to_string())
    }
}
