//! Safe document-tree refusals, distinct from native storage and authorization failures.

/// Why a complete document operation could not be performed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid shape, basename or operation combination, without hidden item data.
    #[error("{0}")]
    Invalid(&'static str),
    /// A requested readable item or parent is absent.
    #[error("the requested document entry is not available")]
    NotFound,
    /// At least one required item is not authorized; no partial tree is disclosed.
    #[error("the complete document operation is not permitted")]
    Denied,
    /// A namespace, deletion identity or replay argument conflicts.
    #[error("a document entry conflicts with this operation")]
    Conflict,
    /// Current membership or material differs from the complete deletion plan.
    #[error("the document tree changed; obtain a new complete deletion preview")]
    Stale,
    /// The complete affected tree or material exceeds its ordinary limits.
    #[error("the complete document operation exceeds its limit")]
    Limit,
    /// Native storage failure; callers return their ordinary internal problem.
    #[error(transparent)]
    Storage(orch_db::DbError),
    /// Native authorization state could not be loaded.
    #[error(transparent)]
    Authorization(#[from] orch_authz::Error),
    /// A stored receipt or content does not have its native shape.
    #[error(transparent)]
    Content(#[from] serde_json::Error),
    /// A normal document producer failed.
    #[error(transparent)]
    Engine(#[from] crate::EngineError),
}

impl From<orch_db::DbError> for Error {
    fn from(error: orch_db::DbError) -> Self {
        match error {
            orch_db::DbError::Unique { .. } | orch_db::DbError::Reference { .. } => Self::Conflict,
            orch_db::DbError::Rule { .. } | orch_db::DbError::Check { .. } => {
                Self::Invalid("the document parent, name or membership is invalid")
            }
            orch_db::DbError::Retry => Self::Stale,
            other => Self::Storage(other),
        }
    }
}

impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        orch_db::DbError::from(error).into()
    }
}
