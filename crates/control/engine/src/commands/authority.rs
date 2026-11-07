//! One server-private durable command evidence channel; never part of Cmd or its body.

use super::Outgoing;
use crate::archive::snapshot_authority::SnapshotReadAuthority;
use crate::jobs::capability_change::authority::CapabilityChangeAuthority;
use crate::{
    EngineError,
    operation_scope::{Prepared, targets},
    provider_setup::LoginCommandAuthority,
};
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn,
    queries::{link::CommandRow, terminals},
};
use orch_link_contract::commands::{CommandKind, PtyClose, SerialKey};
use orch_types::{Clock, Id, Time};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::SerializeStruct,
};
use serde_json::Value;

/// Existing Login/Handover evidence or a disjoint captured ordinary operation.
#[derive(Clone, PartialEq)]
pub enum CommandAuthority {
    /// The exact old value format and provider verifier, without new wrapping fields.
    Login(Box<LoginCommandAuthority>),
    /// One actual captured close and its canonical physical shell.
    TerminalClose(Box<TerminalCloseAuthority>),
    /// One actual accepted shell creator, retaining its typed native phase/body.
    TerminalCreation(Box<super::TerminalCreationAuthority>),
    /// One actual recorded read and its current physical custodian.
    SnapshotRead(Box<SnapshotReadAuthority>),
    /// Only the actual immutable input identity; its authority stays on the input row.
    InputSteer(InputSteerAuthority),
    /// Only the actual immutable live-capability job/direction; origin stays on its job.
    CapabilityChange(Box<CapabilityChangeAuthority>),
    /// The original reader and native source of a generated semantic query.
    SemanticQuery(Box<crate::semantic::query_origin::QueryAuthority>),
    /// The actual administrative index request and its immutable embedding command.
    SemanticEmbedding(Box<crate::semantic::administration::EmbeddingCommand>),
    /// The original workspace measurement and its exact current native source.
    Workspace(Box<super::WorkspaceCommand>),
    /// One native queued Snapshot's original stored source and canonical command.
    SnapshotCreation(Box<super::SnapshotCommand>),
    /// The actual native workspace copy and its original source/recipient.
    FileCopy(Box<super::FileCopyCommand>),
    /// The native multi-command source preparation and its immutable original job.
    SourcePreparation(Box<crate::jobs::source::operation::SourceCommand>),
    /// One exact native build claim, program and physical checker command.
    HarnessCheck(Box<crate::build_checks::CheckCommand>),
    /// The actual accepted setting and exact native runtime command, private to the journal.
    Setting(Box<crate::setting_operations::CommandEvidence>),
    /// The actual Host Get reader and its exact once-only native proposal.
    HostPortsProbe(Box<super::HostPortsProbe>),
    /// Exact reducing cleanup, journaled atomically with its accepted MCP deletion.
    McpDeletionCleanup(Box<super::McpDeletionCleanup>),
    /// The original environment import and its exact native definition-copy command.
    McpImport(Box<crate::environment_mcp::CopyCommand>),
    /// Only an exact successful native copy owns removal of its still-unfiled definition.
    McpForget(Box<crate::environment_mcp::ForgetCommand>),
}
impl std::fmt::Debug for CommandAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CommandAuthority([private])")
    }
}
impl Serialize for CommandAuthority {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Login(authority) => authority.serialize(serializer),
            Self::TerminalClose(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "terminal_close")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::TerminalCreation(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "terminal_creation")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::SnapshotRead(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "snapshot_read")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::InputSteer(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "input_steer")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::CapabilityChange(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "capability_change")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::SemanticQuery(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "semantic_query")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::SemanticEmbedding(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "semantic_embedding")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::Workspace(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "workspace")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::SnapshotCreation(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "snapshot_creation")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::FileCopy(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "file_copy")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::SourcePreparation(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "source_preparation")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::HarnessCheck(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "harness_check")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::McpDeletionCleanup(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "mcp_deletion_cleanup")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::HostPortsProbe(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "host_ports_probe")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::Setting(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "setting_apply")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::McpImport(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "mcp_import")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::McpForget(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "mcp_forget")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Versioned {
    version: u64,
    kind: String,
    authority: Value,
}
impl<'de> Deserialize<'de> for CommandAuthority {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| D::Error::custom("private command authority must be an object"))?;
        if object.contains_key("version")
            || object.contains_key("kind")
            || object.contains_key("authority")
        {
            let value: Versioned = serde_json::from_value(value).map_err(D::Error::custom)?;
            if value.version != 1 {
                return Err(D::Error::custom(
                    "unknown private command authority version",
                ));
            }
            match value.kind.as_str() {
                "terminal_close" => serde_json::from_value(value.authority)
                    .map(|authority| Self::TerminalClose(Box::new(authority)))
                    .map_err(D::Error::custom),
                "terminal_creation" => serde_json::from_value(value.authority)
                    .map(|authority| Self::TerminalCreation(Box::new(authority)))
                    .map_err(D::Error::custom),
                "snapshot_read" => serde_json::from_value(value.authority)
                    .map(|authority| Self::SnapshotRead(Box::new(authority)))
                    .map_err(D::Error::custom),
                "input_steer" => serde_json::from_value(value.authority)
                    .map(Self::InputSteer)
                    .map_err(D::Error::custom),
                "capability_change" => serde_json::from_value(value.authority)
                    .map(|authority| Self::CapabilityChange(Box::new(authority)))
                    .map_err(D::Error::custom),
                "semantic_query" => serde_json::from_value(value.authority)
                    .map(|authority| Self::SemanticQuery(Box::new(authority)))
                    .map_err(D::Error::custom),
                "semantic_embedding" => serde_json::from_value(value.authority)
                    .map(|authority| Self::SemanticEmbedding(Box::new(authority)))
                    .map_err(D::Error::custom),
                "workspace" => serde_json::from_value(value.authority)
                    .map(|authority| Self::Workspace(Box::new(authority)))
                    .map_err(D::Error::custom),
                "snapshot_creation" => serde_json::from_value(value.authority)
                    .map(|authority| Self::SnapshotCreation(Box::new(authority)))
                    .map_err(D::Error::custom),
                "file_copy" => serde_json::from_value(value.authority)
                    .map(|authority| Self::FileCopy(Box::new(authority)))
                    .map_err(D::Error::custom),
                "source_preparation" => serde_json::from_value(value.authority)
                    .map(|authority| Self::SourcePreparation(Box::new(authority)))
                    .map_err(D::Error::custom),
                "harness_check" => serde_json::from_value(value.authority)
                    .map(|authority| Self::HarnessCheck(Box::new(authority)))
                    .map_err(D::Error::custom),
                "mcp_deletion_cleanup" => serde_json::from_value(value.authority)
                    .map(|authority| Self::McpDeletionCleanup(Box::new(authority)))
                    .map_err(D::Error::custom),
                "host_ports_probe" => serde_json::from_value(value.authority)
                    .map(|authority| Self::HostPortsProbe(Box::new(authority)))
                    .map_err(D::Error::custom),
                "setting_apply" => serde_json::from_value(value.authority)
                    .map(|authority| Self::Setting(Box::new(authority)))
                    .map_err(D::Error::custom),
                "mcp_import" => serde_json::from_value(value.authority)
                    .map(|authority| Self::McpImport(Box::new(authority)))
                    .map_err(D::Error::custom),
                "mcp_forget" => serde_json::from_value(value.authority)
                    .map(|authority| Self::McpForget(Box::new(authority)))
                    .map_err(D::Error::custom),
                _ => Err(D::Error::custom("unknown private command authority kind")),
            }
        } else {
            serde_json::from_value(value)
                .map(|authority| Self::Login(Box::new(authority)))
                .map_err(D::Error::custom)
        }
    }
}

/// Missing old metadata is None; explicitly present null/malformed evidence is not absence.
pub(super) fn deserialize_optional<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<CommandAuthority>, D::Error> {
    CommandAuthority::deserialize(deserializer).map(Some)
}
impl CommandAuthority {
    /// Native operation-owned suppliers also prepare their actual queue insertion after waits.
    /// Existing recorded commands retain their independent receipt/reconciliation path.
    pub fn requires_issue_checkpoint(&self) -> bool {
        matches!(
            self,
            Self::SemanticEmbedding(_)
                | Self::HostPortsProbe(_)
                | Self::McpDeletionCleanup(_)
                | Self::Setting(_)
                | Self::HarnessCheck(_)
                | Self::Workspace(_)
                | Self::SnapshotCreation(_)
                | Self::FileCopy(_)
                | Self::SourcePreparation(_)
                | Self::McpImport(_)
                | Self::McpForget(_)
        )
    }

    /// Current authority for the actual server-built command, before its first durable insert.
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::NewCommand<'_>,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        match self {
            // The deletion writer already journals cleanup atomically. A missing retained
            // command cannot use accepted-cleanup evidence to admit a fresh insertion.
            Self::McpDeletionCleanup(_) => Ok(None),
            Self::HostPortsProbe(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::McpImport(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::McpForget(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::SourcePreparation(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::FileCopy(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::SemanticEmbedding(authority) => {
                authority
                    .current_issue_callers(conn, command, clock, access)
                    .await
            }
            Self::Workspace(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::SnapshotCreation(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::HarnessCheck(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::Setting(authority) => {
                authority
                    .current_proposed(
                        conn,
                        command.id,
                        command.machine_id,
                        command.kind,
                        command.serial_key,
                        command.body,
                        clock,
                    )
                    .await
            }
            _ => Ok(None),
        }
    }

    /// The ordinary typed verifier is unchanged. This new/live path retains actual
    /// original/current identities and raw source deadlines through later coordinator waits.
    /// These tuples are not serializable permissions, receipts or reusable admission.
    /// A missing caller carries only an independently checked owned-cleanup deadline;
    /// it never means an unrestricted Session or a new credential authority.
    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if let Self::InputSteer(authority) = self {
            return crate::inputs::native_delivery::current_callers(
                conn,
                command,
                Some(authority.input_id),
                clock,
                access,
            )
            .await;
        }
        if let Self::CapabilityChange(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::SemanticQuery(authority) = self {
            return authority
                .current_callers(conn, command, clock, access)
                .await;
        }
        if let Self::SemanticEmbedding(authority) = self {
            return authority
                .current_callers(conn, command, clock, access)
                .await;
        }
        if let Self::Workspace(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::SnapshotCreation(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::FileCopy(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::SourcePreparation(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::HarnessCheck(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::McpDeletionCleanup(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::HostPortsProbe(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::Setting(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::McpImport(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::McpForget(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if !self.permits_command(conn, command, clock.time()).await? {
            return Ok(None);
        }
        match self {
            Self::Login(authority) => authority.current_callers(conn, clock).await,
            Self::TerminalClose(authority) => Ok(authority
                .restore_current(conn, clock)
                .await?
                .map(|(caller, deadline)| vec![(Some(caller), deadline)])),
            Self::TerminalCreation(authority) => authority.current_callers(conn, clock).await,
            Self::SnapshotRead(authority) => authority.current_callers(conn, clock).await,
            Self::InputSteer(_)
            | Self::CapabilityChange(_)
            | Self::SemanticQuery(_)
            | Self::SemanticEmbedding(_)
            | Self::Workspace(_)
            | Self::SnapshotCreation(_)
            | Self::FileCopy(_)
            | Self::SourcePreparation(_)
            | Self::Setting(_)
            | Self::HostPortsProbe(_)
            | Self::McpDeletionCleanup(_)
            | Self::McpImport(_)
            | Self::McpForget(_)
            | Self::HarnessCheck(_) => Ok(None),
        }
    }

    /// Retains exactly the existing provider data format.
    pub fn login(authority: LoginCommandAuthority) -> Self {
        Self::Login(Box::new(authority))
    }
    /// Binds one already captured terminal operation, not a provider/agent substitute.
    pub fn terminal_close(authority: TerminalCloseAuthority) -> Self {
        Self::TerminalClose(Box::new(authority))
    }
    /// Binds only the native command of one accepted shell creation.
    pub fn terminal_creation(authority: super::TerminalCreationAuthority) -> Self {
        Self::TerminalCreation(Box::new(authority))
    }
    /// Binds one already captured Snapshot operation and physical request.
    pub fn snapshot_read(authority: SnapshotReadAuthority) -> Self {
        Self::SnapshotRead(Box::new(authority))
    }
    /// The ordinary input writer pins this immutable factual identity at first issue.
    /// It never copies input privileges or adopts an older canonical None command.
    pub(crate) fn input_steer(input_id: Id) -> Self {
        Self::InputSteer(InputSteerAuthority { input_id })
    }

    /// Retains the capability producer's actual factual job/direction reference.
    pub fn capability_change(authority: CapabilityChangeAuthority) -> Self {
        Self::CapabilityChange(Box::new(authority))
    }

    /// Retains the real index request without putting authority into provider wire bytes.
    pub fn semantic_embedding(
        authority: crate::semantic::administration::EmbeddingCommand,
    ) -> Self {
        Self::SemanticEmbedding(Box::new(authority))
    }

    /// Retains the actual workspace measurement without adding authority to wire bytes.
    pub fn workspace(authority: super::WorkspaceCommand) -> Self {
        Self::Workspace(Box::new(authority))
    }
    /// Binds the actual queued snapshot's original recipe, without authority in native bytes.
    pub fn snapshot_creation(authority: super::SnapshotCommand) -> Self {
        Self::SnapshotCreation(Box::new(authority))
    }
    /// Keeps copy provenance in the existing private channel, never in filesystem wire bytes.
    pub fn file_copy(authority: super::FileCopyCommand) -> Self {
        Self::FileCopy(Box::new(authority))
    }
    /// Retains the original native source operation without authority in any wire body.
    pub fn source_preparation(authority: crate::jobs::source::operation::SourceCommand) -> Self {
        Self::SourcePreparation(Box::new(authority))
    }

    /// The existing checker supplies its own factual native claim and exact emitted bytes.
    pub fn harness_check(authority: crate::build_checks::CheckCommand) -> Self {
        Self::HarnessCheck(Box::new(authority))
    }

    /// Retains the actual native setting application without putting its origin in connector bytes.
    pub fn setting(authority: crate::setting_operations::CommandEvidence) -> Self {
        Self::Setting(Box::new(authority))
    }

    /// Retains the native Host proposal reader without changing connector bytes.
    pub fn host_ports_probe(authority: super::HostPortsProbe) -> Self {
        Self::HostPortsProbe(Box::new(authority))
    }

    /// Retains only cleanup already recorded with the actual accepted resource deletion.
    pub fn mcp_deletion_cleanup(authority: super::McpDeletionCleanup) -> Self {
        Self::McpDeletionCleanup(Box::new(authority))
    }

    /// Retains the admitted definition-copy producer without putting authority on the wire.
    pub fn mcp_import(authority: crate::environment_mcp::CopyCommand) -> Self {
        Self::McpImport(Box::new(authority))
    }

    /// Owns only the exact successful copy's inverse, never an invoker or general MCP grant.
    pub fn mcp_forget(authority: crate::environment_mcp::ForgetCommand) -> Self {
        Self::McpForget(Box::new(authority))
    }

    /// The native kind belongs to the typed producer, never an incoming key claim.
    pub fn command_kind(&self) -> &'static str {
        match self {
            Self::Login(authority) => authority.command_kind(),
            Self::TerminalClose(_) => "pty.close",
            Self::TerminalCreation(authority) => authority.command_kind(),
            Self::SnapshotRead(_) => "fs.record.read",
            Self::InputSteer(_) => "turn.steer",
            Self::CapabilityChange(_) => "node.capabilities.apply",
            Self::SemanticQuery(_) => "provider.embed",
            Self::SemanticEmbedding(_) => "provider.embed",
            Self::Workspace(_) => "fs.measure",
            Self::SnapshotCreation(_) => "fs.snapshot",
            Self::FileCopy(_) => "fs.copy",
            Self::SourcePreparation(authority) => authority.command_kind(),
            Self::HarnessCheck(authority) => authority.command_kind(),
            Self::Setting(authority) => authority.command_kind(),
            Self::HostPortsProbe(_) => "host.ports",
            Self::McpDeletionCleanup(_) => "mcp.forget",
            Self::McpImport(_) => "mcp.import",
            Self::McpForget(_) => "mcp.forget",
        }
    }
    /// New operation-owned native evidence may not adopt an older canonical missing origin.
    pub fn is_scoped_native(&self) -> bool {
        !matches!(self, Self::Login(_))
    }
    /// Existing physical return custody remains the original provider verifier's fact.
    pub fn handover_source(&self) -> Option<&crate::credentials::handover_authority::ReturnSource> {
        match self {
            Self::Login(authority) => authority.handover_source(),
            _ => None,
        }
    }
    /// Reauthorizes an actual canonical UNACCEPTED command; Accepted reconciliation stays separate.
    pub async fn permits_command(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        now: Time,
    ) -> Result<bool, EngineError> {
        match self {
            Self::Login(authority) => {
                authority
                    .permits_command(
                        conn,
                        command.id,
                        command.machine_id,
                        &command.kind,
                        command.serial_key(),
                        &command.body,
                        now,
                    )
                    .await
            }
            Self::TerminalClose(authority) => authority.authorized(conn, command, now).await,
            Self::TerminalCreation(authority) => authority.authorized(conn, command, now).await,
            Self::SnapshotRead(authority) => authority.authorized(conn, command, now).await,
            // New input publication requires the actual Clock/ResourceAccess path.
            // A sampled Time cannot provide a final native acceptance proof.
            Self::InputSteer(_)
            | Self::CapabilityChange(_)
            | Self::SemanticQuery(_)
            | Self::SemanticEmbedding(_)
            | Self::Workspace(_)
            | Self::SnapshotCreation(_)
            | Self::FileCopy(_)
            | Self::SourcePreparation(_)
            | Self::Setting(_)
            | Self::HostPortsProbe(_)
            | Self::McpDeletionCleanup(_)
            | Self::McpImport(_)
            | Self::McpForget(_)
            | Self::HarnessCheck(_) => Ok(false),
        }
    }
}

/// A factual reference in the existing private channel, never a second input authority.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSteerAuthority {
    input_id: Id,
}
impl std::fmt::Debug for InputSteerAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InputSteerAuthority([private])")
    }
}

/// Captured ordinary close facts only, with no fabricated Session/account ancestry.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalCloseAuthority {
    origin: CapturedOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<Id>,
    terminal_id: Id,
    environment_id: Id,
    machine_id: Id,
    environment_generation: i32,
}
impl std::fmt::Debug for TerminalCloseAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TerminalCloseAuthority([private])")
    }
}
impl TerminalCloseAuthority {
    /// The caller and row come from the actual admitted Close writer; the command is canonical.
    pub fn capture(
        caller: &Caller,
        session: &terminals::SessionRow,
        machine: Id,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        Self::capture_with_session(caller, None, session, machine, command)
    }

    /// The HTTP producer also retains its actual raw browser-session identity, never a new lease.
    pub fn capture_with_session(
        caller: &Caller,
        raw_session: Option<Id>,
        session: &terminals::SessionRow,
        machine: Id,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        if caller.grant().is_some() && raw_session.is_some() {
            return Err(EngineError::unreadable(
                "a key close has no browser-session origin",
            ));
        }
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let authority = Self {
            origin,
            session: raw_session,
            terminal_id: session.id,
            environment_id: session.environment_id,
            machine_id: machine,
            environment_generation: session.env_generation,
        };
        if command.kind != CommandKind::PtyClose
            || command.key != SerialKey::None
            || command.machine != machine
            || command.id != authority.command_id()
            || command.body
                != serde_json::to_value(PtyClose {
                    terminal_id: session.id,
                })?
            || authority.origin.operation() != "TerminalClose"
        {
            return Err(EngineError::unreadable(
                "terminal close origin does not match its actual native command",
            ));
        }
        Ok(authority)
    }

    async fn session_state(
        &self,
        conn: &mut Conn,
        now: Time,
    ) -> Result<Option<Option<(bool, Time)>>, EngineError> {
        session_state(conn, &self.origin, self.session, now).await
    }

    async fn restore_current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<(Caller, Option<Time>)>, EngineError> {
        let Some(session) = self.session_state(conn, clock.time()).await? else {
            return Ok(None);
        };
        let Some(caller) = self.origin.restore_current(conn, clock).await? else {
            return Ok(None);
        };
        let enabled = caller.as_admin() && session.is_some_and(|(admin, _)| admin);
        let caller = caller.with_admin_mode(enabled);
        let deadline = session.map(|(_, end)| end);
        Ok((caller.key_deadline_allows(clock)
            && deadline.is_none_or(|end| clock.time().get() < end.get()))
        .then_some((caller, deadline)))
    }
    fn command_id(&self) -> Id {
        crate::ids::derived("pty.close", &[self.terminal_id.as_bytes().as_slice()])
    }
    /// Current native forwarding proof; a closed metadata row never invents native Accepted facts.
    pub async fn authorized(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        now: Time,
    ) -> Result<bool, EngineError> {
        if self.origin.operation() != "TerminalClose"
            || self.terminal_id.is_nil()
            || self.environment_id.is_nil()
            || self.machine_id.is_nil()
            || command.id != self.command_id()
            || command.machine_id != self.machine_id
            || command.kind != "pty.close"
            || command.serial_key().is_some()
            || command.body
                != serde_json::to_value(PtyClose {
                    terminal_id: self.terminal_id,
                })?
        {
            return Ok(false);
        }
        let Some(shell) = terminals::get(conn, self.terminal_id).await? else {
            return Ok(false);
        };
        if shell.environment_id != self.environment_id
            || shell.env_generation != self.environment_generation
        {
            return Ok(false);
        }
        let Some(site) = terminals::site(conn, self.environment_id).await? else {
            return Ok(false);
        };
        if site.machine_id != Some(self.machine_id)
            || site.generation != self.environment_generation
        {
            return Ok(false);
        }
        let Some(session) = self.session_state(conn, now).await? else {
            return Ok(false);
        };
        let Some(caller) = self.origin.restore(conn, now).await? else {
            return Ok(false);
        };
        let enabled = caller.as_admin() && session.is_some_and(|(admin, _)| admin);
        let caller = caller.with_admin_mode(enabled);
        let target = match targets::resource_identity(
            conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: self.environment_id,
            },
        )
        .await
        {
            Ok(target) => target,
            Err(targets::TargetError::Missing) => return Ok(false),
            Err(targets::TargetError::Db(error)) => return Err(error.into()),
            Err(targets::TargetError::Authz(error)) => return Err(error.into()),
        };
        let mut plan = Prepared::for_selection("TerminalClose").map_err(EngineError::unreadable)?;
        plan.bind(0, KeyTargetKind::Object(ObjectKind::Environment), &[target])
            .map_err(EngineError::unreadable)?;
        let Ok(caller) = plan.admit(&caller, false) else {
            return Ok(false);
        };
        let principal = Principal::load(conn, caller, now).await?;
        Ok(orch_authz::environments::may_close_shell(
            conn,
            &principal,
            &orch_authz::environments::Shell {
                environment: shell.environment_id,
                thread: shell.thread_id,
                opened_by: shell.opened_by,
            },
            Policy::default(),
        )
        .await?)
    }
}

/// The actual raw-session state beside a captured operation; None is not a new session lease.
pub(crate) async fn session_state(
    conn: &mut Conn,
    origin: &CapturedOperation,
    session: Option<Id>,
    now: Time,
) -> Result<Option<Option<(bool, Time)>>, EngineError> {
    let Some(session) = session else {
        return Ok(Some(None));
    };
    Ok(
        orch_db::queries::tokens::lock_session_state(conn, session, origin.user())
            .await?
            .filter(|(_, end)| *end > now.get())
            .map(|(admin, end)| Some((admin, Time::new(end)))),
    )
}
