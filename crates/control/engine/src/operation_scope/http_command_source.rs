//! Domain-owned native HTTP command sources, inside the one raw HTTP command family.
//!
//! Each producer supplies its actual immutable native operands and physical/current verifier.
//! No source value exists until that implementation is present; arbitrary JSON cannot create
//! a source, an operation alias or a successful permission check.
use crate::{EngineError, access::ResourceAccess};
use orch_authz::Caller;
use orch_db::{
    Conn,
    queries::link::{CommandRow, NewCommand},
};
use orch_types::{Clock, Time};
use serde::{Deserialize, Serialize};

/// Actual native source evidence, populated only by its owning native producers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "source",
    content = "evidence",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CommandEvidence {
    /// The actual logical Provider Stop and its original selected managed unit.
    ProviderUnitStop(Box<crate::commands::provider_unit::Command>),
    /// The actual Host Stop and exact physical unit, separate from Tenant permissions.
    HostUnitStop(Box<crate::commands::host_unit::Command>),
    /// The original administrator and exact bundled-node release update job.
    ServerUpdate(Box<crate::jobs::server_update::command::Source>),
    /// The actual retained Ask model call and its still-current controller.
    Ask(Box<crate::ask::command::CommandSource>),
    Skill(super::skill_creation::CommandSource),
    MountRemove(super::mount_locations::RemoveSource),
    MountEnsure(super::mount_locations::SetSource),
    ReferenceStat(super::reference_preview::FileSource),
    ReferenceFetch(super::reference_preview::FileSource),
    FileMkdir(crate::commands::FileMkdirCommand),
}
impl CommandEvidence {
    pub fn command_kind(&self) -> &'static str {
        match self {
            Self::ProviderUnitStop(_) => "node.runtime.stop",
            Self::HostUnitStop(_) => "node.runtime.stop",
            Self::ServerUpdate(_) => "node.server.update",
            Self::Ask(_) => "provider.generate",
            Self::Skill(source) => source.command_kind(),
            Self::MountRemove(_) => "node.volume.remove",
            Self::MountEnsure(_) => "node.volume.ensure",
            Self::ReferenceStat(_) => "fs.stat",
            Self::ReferenceFetch(_) => "fs.fetch",
            Self::FileMkdir(source) => source.command_kind(),
        }
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        match self {
            Self::ProviderUnitStop(source) => source.lock_source(conn).await,
            Self::HostUnitStop(source) => source.lock_source(conn).await,
            Self::ServerUpdate(source) => source.lock_source(conn).await,
            Self::Ask(source) => source.lock_source(conn).await,
            Self::Skill(source) => source.lock_source(conn).await,
            Self::MountEnsure(source) => source.lock_source(conn).await,
            Self::MountRemove(source) => source.lock_source(conn).await,
            Self::ReferenceStat(source) | Self::ReferenceFetch(source) => {
                source.lock_source(conn).await
            }
            Self::FileMkdir(source) => source.lock_source(conn).await,
        }
    }
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
        _access: &dyn ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.kind != self.command_kind() {
            return Ok(None);
        }
        match self {
            Self::ProviderUnitStop(source) => source.current_issue(conn, command, clock).await,
            Self::HostUnitStop(source) => source.current_issue(conn, command, clock).await,
            Self::ServerUpdate(source) => source.current_issue(conn, command, clock).await,
            Self::Ask(source) => source.current_issue_callers(conn, command, clock).await,
            Self::Skill(source) => source.current_issue(conn, command, clock).await,
            Self::MountEnsure(source) => source.current(conn, clock).await,
            Self::MountRemove(source) => source.current(conn, clock).await,
            Self::ReferenceStat(source) | Self::ReferenceFetch(source) => {
                source.current(conn, clock).await
            }
            Self::FileMkdir(source) => source.current_issue(conn, command, clock).await,
        }
    }
    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
        _access: &dyn ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.kind != self.command_kind() {
            return Ok(None);
        }
        match self {
            Self::ProviderUnitStop(source) => source.current_stored(conn, command, clock).await,
            Self::HostUnitStop(source) => source.current_stored(conn, command, clock).await,
            Self::ServerUpdate(source) => source.current_stored(conn, command, clock).await,
            Self::Ask(source) => source.current_callers(conn, command, clock).await,
            Self::Skill(source) => source.current_stored(conn, command, clock).await,
            Self::MountEnsure(source) => source.current(conn, clock).await,
            Self::MountRemove(source) => source.current(conn, clock).await,
            Self::ReferenceStat(source) | Self::ReferenceFetch(source) => {
                source.current(conn, clock).await
            }
            Self::FileMkdir(source) => source.current_stored(conn, command, clock).await,
        }
    }
}
