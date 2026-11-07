//! One native import authority, with distinct creation and stored-resource source proofs.

use super::{CopyCommand, existing::Command};
use crate::EngineError;
use orch_authz::Caller;
use orch_db::{
    Conn,
    queries::link::{CommandRow, NewCommand},
};
use orch_types::{Clock, Time};
use serde::{Deserialize, Serialize};

/// Strict leaf inputs distinguish the original creator format from a real stored server.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ImportCommand {
    Existing(Command),
    Creation(CopyCommand),
}
impl std::fmt::Debug for ImportCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpImportCommand([private])")
    }
}
impl ImportCommand {
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        match self {
            Self::Creation(_) => Ok(()),
            Self::Existing(source) => source.lock_source(conn).await,
        }
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        match self {
            Self::Creation(source) => source.current_issue_callers(conn, command, clock).await,
            Self::Existing(source) => source.current_issue_callers(conn, command, clock).await,
        }
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        match self {
            Self::Creation(source) => source.current_callers(conn, command, clock).await,
            Self::Existing(source) => source.current_callers(conn, command, clock).await,
        }
    }
}
