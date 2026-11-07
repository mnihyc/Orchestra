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
    /// An exact accepted capability removal and its owned native replacement command.
    CapabilityRecreation(Box<crate::jobs::recreate::capability_revocation::Authority>),
    /// An exact retained reservation and its current native command.
    RetainedLifecycle(Box<crate::jobs::retained::operation::Authority>),
    /// The original reader and native source of a generated semantic query.
    SemanticQuery(Box<crate::semantic::query_origin::QueryAuthority>),
    /// The native prompt-transition job and its exact forward/recovery command.
    Continuation(Box<crate::continuations::command::Authority>),
    /// The canonical native phase of an accepted side-history creation.
    SideHistory(Box<crate::side_threads::command::Authority>),
    /// The retained native fork job and its exact startup/history command.
    ForkNative(Box<crate::jobs::fork::command::Authority>),
    /// The actual binding descriptor/startup phase and exact native command.
    RebindPreparation(Box<crate::binding::operation::Command>),
    /// The original native fork/switch planner and its exact bounded supplier.
    ForkPreparation(Box<crate::fork_plan::command::Authority>),
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
    /// The original recorded restore review or exact native job continuation.
    WorkspaceRestore(Box<crate::jobs::restore::operation::RestoreCommand>),
    /// One exact native build claim, program and physical checker command.
    HarnessCheck(Box<crate::build_checks::CheckCommand>),
    /// The actual accepted setting and exact native runtime command, private to the journal.
    Setting(Box<crate::setting_operations::CommandEvidence>),
    /// The actual confirmed cleanup item and exact native command.
    Cleanup(Box<crate::jobs::cleanup::operation::Command>),
    /// Exact known-source adoption preparation; never an admitted main creator act.
    AdoptionPreparation(Box<crate::adoption::operation::CommandEvidence>),
    /// The original recycling operation and exact native process probe.
    RecycleProbe(Box<crate::recycle::operation::CommandEvidence>),
    /// The original update operation and its exact native file-size observation.
    UpdatePlanningProbe(Box<crate::updates::operation::CommandEvidence>),
    /// Earlier private probe spelling, verified by the same canonical native source.
    UpdateMeasurement(Box<crate::updates::operation::CommandEvidence>),
    /// Original global batch review/confirmation and its exact native command.
    UpdateJob(Box<crate::jobs::recreate::environment_authority::Command>),
    /// One actual accepted Environment replacement and its exact native canonical proposal.
    EnvironmentUpdate(Box<crate::jobs::recreate::batch_authority::environment::Command>),
    /// The actual Host Get reader and its exact once-only native proposal.
    HostPortsProbe(Box<super::HostPortsProbe>),
    /// Original native Machine facts observation.
    MachineProbe(Box<super::MachineProbeAuthority>),
    /// Original native program mutation.
    MachinePrograms(Box<super::MachineProgramsAuthority>),
    /// Original delegated role selection.
    MachineRoles(Box<super::MachineRolesAuthority>),
    /// Original key-by-key policy choice.
    MachinePolicy(Box<super::MachinePolicyAuthority>),
    /// Actual Host operation retained on its native policy synchronization.
    HostPolicy(Box<super::HostPolicyAuthority>),
    /// The actual manual Host request and its exact deferred physical update.
    ComponentUpdate(Box<crate::component_update_authority::UpdateCommand>),
    /// The selected proposal Host, original creation authority and exact archive sources.
    TemplateImport(Box<crate::template_imports::Command>),
    /// The original environment import and its exact native definition-copy command.
    McpImport(Box<crate::environment_mcp::ImportCommand>),
    /// One exact unfiled native copy or a journaled resource deletion owns its removal.
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
            Self::SideHistory(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "side_history")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::ForkNative(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "fork_native")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::RebindPreparation(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "rebind_preparation")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::ForkPreparation(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "fork_preparation")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::Continuation(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "continuation")?;
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
            Self::WorkspaceRestore(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "workspace_restore")?;
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
            Self::HostPortsProbe(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "host_ports_probe")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::MachineProbe(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "machine_probe")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::MachinePrograms(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "machine_programs")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::MachineRoles(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "machine_roles")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::MachinePolicy(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "machine_policy")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::HostPolicy(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "host_policy")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::EnvironmentUpdate(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "environment_update")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::UpdateMeasurement(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "update_measurement")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::UpdateJob(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "environment_update_job")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::UpdatePlanningProbe(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "update_planning_probe")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::ComponentUpdate(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "component_update")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::TemplateImport(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "template_import")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::RecycleProbe(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "recycle_probe")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::AdoptionPreparation(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "adoption_preparation")?;
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
            Self::Cleanup(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "storage_cleanup")?;
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
            Self::RetainedLifecycle(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "retained_lifecycle")?;
                value.serialize_field("authority", authority)?;
                value.end()
            }
            Self::CapabilityRecreation(authority) => {
                let mut value = serializer.serialize_struct("CommandAuthority", 3)?;
                value.serialize_field("version", &1_u8)?;
                value.serialize_field("kind", "capability_recreation")?;
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
                "side_history" => serde_json::from_value(value.authority)
                    .map(|authority| Self::SideHistory(Box::new(authority)))
                    .map_err(D::Error::custom),
                "fork_native" => serde_json::from_value(value.authority)
                    .map(|authority| Self::ForkNative(Box::new(authority)))
                    .map_err(D::Error::custom),
                "rebind_preparation" => serde_json::from_value(value.authority)
                    .map(|authority| Self::RebindPreparation(Box::new(authority)))
                    .map_err(D::Error::custom),
                "fork_preparation" => serde_json::from_value(value.authority)
                    .map(|authority| Self::ForkPreparation(Box::new(authority)))
                    .map_err(D::Error::custom),
                "continuation" => serde_json::from_value(value.authority)
                    .map(|authority| Self::Continuation(Box::new(authority)))
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
                "workspace_restore" => serde_json::from_value(value.authority)
                    .map(|authority| Self::WorkspaceRestore(Box::new(authority)))
                    .map_err(D::Error::custom),
                "harness_check" => serde_json::from_value(value.authority)
                    .map(|authority| Self::HarnessCheck(Box::new(authority)))
                    .map_err(D::Error::custom),
                "host_ports_probe" => serde_json::from_value(value.authority)
                    .map(|authority| Self::HostPortsProbe(Box::new(authority)))
                    .map_err(D::Error::custom),
                "machine_probe" => serde_json::from_value(value.authority)
                    .map(|value| Self::MachineProbe(Box::new(value)))
                    .map_err(D::Error::custom),
                "machine_programs" => serde_json::from_value(value.authority)
                    .map(|value| Self::MachinePrograms(Box::new(value)))
                    .map_err(D::Error::custom),
                "machine_roles" => serde_json::from_value(value.authority)
                    .map(|value| Self::MachineRoles(Box::new(value)))
                    .map_err(D::Error::custom),
                "machine_policy" => serde_json::from_value(value.authority)
                    .map(|value| Self::MachinePolicy(Box::new(value)))
                    .map_err(D::Error::custom),
                "host_policy" => serde_json::from_value(value.authority)
                    .map(|value| Self::HostPolicy(Box::new(value)))
                    .map_err(D::Error::custom),
                "component_update" => serde_json::from_value(value.authority)
                    .map(|authority| Self::ComponentUpdate(Box::new(authority)))
                    .map_err(D::Error::custom),
                "template_import" => serde_json::from_value(value.authority)
                    .map(|authority| Self::TemplateImport(Box::new(authority)))
                    .map_err(D::Error::custom),
                "storage_cleanup" => serde_json::from_value(value.authority)
                    .map(|authority| Self::Cleanup(Box::new(authority)))
                    .map_err(D::Error::custom),
                "environment_update" => serde_json::from_value(value.authority)
                    .map(|authority| Self::EnvironmentUpdate(Box::new(authority)))
                    .map_err(D::Error::custom),
                "update_measurement" => serde_json::from_value(value.authority)
                    .map(|authority| Self::UpdateMeasurement(Box::new(authority)))
                    .map_err(D::Error::custom),
                "environment_update_job" => serde_json::from_value(value.authority)
                    .map(|authority| Self::UpdateJob(Box::new(authority)))
                    .map_err(D::Error::custom),
                "update_planning_probe" => serde_json::from_value(value.authority)
                    .map(|authority| Self::UpdatePlanningProbe(Box::new(authority)))
                    .map_err(D::Error::custom),
                "recycle_probe" => serde_json::from_value(value.authority)
                    .map(|authority| Self::RecycleProbe(Box::new(authority)))
                    .map_err(D::Error::custom),
                "adoption_preparation" => serde_json::from_value(value.authority)
                    .map(|authority| Self::AdoptionPreparation(Box::new(authority)))
                    .map_err(D::Error::custom),
                "setting_apply" => serde_json::from_value(value.authority)
                    .map(|authority| Self::Setting(Box::new(authority)))
                    .map_err(D::Error::custom),
                "mcp_import" => serde_json::from_value(value.authority)
                    .map(|authority| Self::McpImport(Box::new(authority)))
                    .map_err(D::Error::custom),
                "retained_lifecycle" => serde_json::from_value(value.authority)
                    .map(|authority| Self::RetainedLifecycle(Box::new(authority)))
                    .map_err(D::Error::custom),
                "capability_recreation" => serde_json::from_value(value.authority)
                    .map(|authority| Self::CapabilityRecreation(Box::new(authority)))
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
    /// Take the native owner's factual row fence before its canonical command row.
    /// This grants nothing; the live verifier still restores and checks the original source.
    pub async fn lock_native_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        match self {
            Self::SideHistory(authority) => authority.lock_source(conn).await,
            Self::ForkNative(authority) => authority.lock_source(conn).await,
            Self::RebindPreparation(authority) => authority.lock_source(conn).await,
            Self::ForkPreparation(authority) => authority.lock_source(conn).await,
            Self::AdoptionPreparation(authority) => authority.lock_source(conn).await,
            Self::UpdatePlanningProbe(authority) | Self::UpdateMeasurement(authority) => {
                authority.lock_source(conn).await
            }
            Self::UpdateJob(authority) => authority.lock_source(conn).await,
            Self::EnvironmentUpdate(authority) => authority.lock_source(conn).await,
            Self::Continuation(authority) => authority.lock_source(conn).await,
            Self::MachineProbe(authority) => authority.lock_source(conn).await,
            Self::MachinePrograms(authority) => authority.lock_source(conn).await,
            Self::MachineRoles(authority) => authority.lock_source(conn).await,
            Self::MachinePolicy(authority) => authority.lock_source(conn).await,
            Self::HostPolicy(authority) => authority.lock_source(conn).await,
            Self::CapabilityRecreation(authority) => authority.lock_source(conn).await,
            Self::RetainedLifecycle(authority) => authority.lock_source(conn).await,
            Self::ComponentUpdate(authority) => authority.lock_source(conn).await,
            Self::McpImport(authority) => authority.lock_source(conn).await,
            Self::McpForget(authority) => authority.lock_source(conn).await,
            _ => Ok(()),
        }
    }

    /// Native operation-owned suppliers also prepare their actual queue insertion after waits.
    /// Existing recorded commands retain their independent receipt/reconciliation path.
    pub fn requires_issue_checkpoint(&self) -> bool {
        matches!(
            self,
            Self::Cleanup(_)
                | Self::CapabilityRecreation(_)
                | Self::RetainedLifecycle(_)
                | Self::Continuation(_)
                | Self::ForkNative(_)
                | Self::RebindPreparation(_)
                | Self::ForkPreparation(_)
                | Self::SideHistory(_)
                | Self::SemanticEmbedding(_)
                | Self::HostPortsProbe(_)
                | Self::MachineProbe(_)
                | Self::MachinePrograms(_)
                | Self::MachineRoles(_)
                | Self::MachinePolicy(_)
                | Self::HostPolicy(_)
                | Self::ComponentUpdate(_)
                | Self::TemplateImport(_)
                | Self::Setting(_)
                | Self::HarnessCheck(_)
                | Self::Workspace(_)
                | Self::SnapshotCreation(_)
                | Self::FileCopy(_)
                | Self::SourcePreparation(_)
                | Self::WorkspaceRestore(_)
                | Self::AdoptionPreparation(_)
                | Self::RecycleProbe(_)
                | Self::UpdatePlanningProbe(_)
                | Self::UpdateMeasurement(_)
                | Self::UpdateJob(_)
                | Self::EnvironmentUpdate(_)
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
            Self::Cleanup(authority) => authority.current_issue_callers(conn, command, clock).await,
            Self::EnvironmentUpdate(authority) => {
                authority
                    .current_issue_callers(conn, command, clock, access)
                    .await
            }
            Self::UpdateMeasurement(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::UpdateJob(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::UpdatePlanningProbe(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::RecycleProbe(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::AdoptionPreparation(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            // The deletion writer already journals cleanup atomically. A missing retained
            // command cannot use accepted-cleanup evidence to admit a fresh insertion.
            Self::SideHistory(authority) => {
                authority
                    .current_issue_callers(conn, command, clock, access)
                    .await
            }
            Self::ForkNative(authority) => {
                authority
                    .current_issue_callers(conn, command, clock, access)
                    .await
            }
            Self::RebindPreparation(authority) => {
                authority
                    .current_issue_callers(conn, command, clock, access)
                    .await
            }
            Self::ForkPreparation(authority) => {
                authority
                    .current_issue_callers(conn, command, clock, access)
                    .await
            }
            Self::Continuation(authority) => {
                authority
                    .current_issue_callers(conn, command, clock, access)
                    .await
            }
            Self::HostPortsProbe(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::MachineProbe(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::MachinePrograms(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::MachineRoles(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::MachinePolicy(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::HostPolicy(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::ComponentUpdate(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::TemplateImport(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::McpImport(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::RetainedLifecycle(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::CapabilityRecreation(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::McpForget(authority) => {
                authority.current_issue_callers(conn, command, clock).await
            }
            Self::WorkspaceRestore(authority) => {
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
        if let Self::EnvironmentUpdate(authority) = self {
            return authority
                .current_callers(conn, command, clock, access)
                .await;
        }
        if let Self::UpdateMeasurement(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::UpdateJob(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::UpdatePlanningProbe(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::RecycleProbe(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::AdoptionPreparation(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
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
        if let Self::SideHistory(authority) = self {
            return authority
                .current_callers(conn, command, clock, access)
                .await;
        }
        if let Self::ForkNative(authority) = self {
            return authority
                .current_callers(conn, command, clock, access)
                .await;
        }
        if let Self::RebindPreparation(authority) = self {
            return authority
                .current_callers(conn, command, clock, access)
                .await;
        }
        if let Self::ForkPreparation(authority) = self {
            return authority
                .current_callers(conn, command, clock, access)
                .await;
        }
        if let Self::Continuation(authority) = self {
            return authority
                .current_callers(conn, command, clock, access)
                .await;
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
        if let Self::WorkspaceRestore(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::SourcePreparation(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::HarnessCheck(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::HostPortsProbe(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::MachineProbe(value) = self {
            return value.current_callers(conn, command, clock).await;
        }
        if let Self::MachinePrograms(value) = self {
            return value.current_callers(conn, command, clock).await;
        }
        if let Self::MachineRoles(value) = self {
            return value.current_callers(conn, command, clock).await;
        }
        if let Self::MachinePolicy(value) = self {
            return value.current_callers(conn, command, clock).await;
        }
        if let Self::HostPolicy(value) = self {
            return value.current_callers(conn, command, clock).await;
        }
        if let Self::ComponentUpdate(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::TemplateImport(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::Cleanup(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::Setting(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::McpImport(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::RetainedLifecycle(authority) = self {
            return authority.current_callers(conn, command, clock).await;
        }
        if let Self::CapabilityRecreation(authority) = self {
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
            | Self::CapabilityRecreation(_)
            | Self::RetainedLifecycle(_)
            | Self::SemanticQuery(_)
            | Self::Continuation(_)
            | Self::ForkNative(_)
            | Self::RebindPreparation(_)
            | Self::ForkPreparation(_)
            | Self::SideHistory(_)
            | Self::SemanticEmbedding(_)
            | Self::Workspace(_)
            | Self::SnapshotCreation(_)
            | Self::FileCopy(_)
            | Self::SourcePreparation(_)
            | Self::Cleanup(_)
            | Self::WorkspaceRestore(_)
            | Self::AdoptionPreparation(_)
            | Self::RecycleProbe(_)
            | Self::UpdatePlanningProbe(_)
            | Self::UpdateMeasurement(_)
            | Self::UpdateJob(_)
            | Self::EnvironmentUpdate(_)
            | Self::Setting(_)
            | Self::HostPortsProbe(_)
            | Self::MachineProbe(_)
            | Self::MachinePrograms(_)
            | Self::MachineRoles(_)
            | Self::MachinePolicy(_)
            | Self::HostPolicy(_)
            | Self::ComponentUpdate(_)
            | Self::TemplateImport(_)
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

    /// Retain only the actual native recorded restore source, never authority in wire bytes.
    pub fn workspace_restore(authority: crate::jobs::restore::operation::RestoreCommand) -> Self {
        Self::WorkspaceRestore(Box::new(authority))
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
    /// Retain the exact native facts request.
    pub fn machine_probe(value: super::MachineProbeAuthority) -> Self {
        Self::MachineProbe(Box::new(value))
    }
    /// Retain the exact native program request.
    pub fn machine_programs(value: super::MachineProgramsAuthority) -> Self {
        Self::MachinePrograms(Box::new(value))
    }
    /// Retain the exact delegated role request.
    pub fn machine_roles(value: super::MachineRolesAuthority) -> Self {
        Self::MachineRoles(Box::new(value))
    }
    /// Retain the exact key-by-key Machine request.
    pub fn machine_policy(value: super::MachinePolicyAuthority) -> Self {
        Self::MachinePolicy(Box::new(value))
    }
    /// Retain the exact native Host policy request.
    pub fn host_policy(value: super::HostPolicyAuthority) -> Self {
        Self::HostPolicy(Box::new(value))
    }

    /// Keeps the original physical request separate from its connector wire bytes.
    pub fn component_update(
        authority: impl Into<crate::component_update_authority::UpdateCommand>,
    ) -> Self {
        Self::ComponentUpdate(Box::new(authority.into()))
    }
    /// Retains only the actual template proposal's private native source.
    pub fn template_import(authority: crate::template_imports::Command) -> Self {
        Self::TemplateImport(Box::new(authority))
    }

    /// Retains the admitted definition-copy producer without putting authority on the wire.
    pub fn mcp_import(authority: crate::environment_mcp::CopyCommand) -> Self {
        Self::McpImport(Box::new(crate::environment_mcp::ImportCommand::Creation(
            authority,
        )))
    }

    /// The same native command carries the existing server's distinct source proof.
    pub fn mcp_existing_import(authority: crate::environment_mcp::existing::Command) -> Self {
        Self::McpImport(Box::new(crate::environment_mcp::ImportCommand::Existing(
            authority,
        )))
    }

    /// Owns only the exact copied file's inverse or accepted deletion, never a general MCP grant.
    pub fn mcp_forget(authority: crate::environment_mcp::ForgetCommand) -> Self {
        Self::McpForget(Box::new(authority))
    }

    /// Keeps cleanup provenance in the existing private channel, outside connector bytes.
    pub fn cleanup(authority: crate::jobs::cleanup::operation::Command) -> Self {
        Self::Cleanup(Box::new(authority))
    }

    /// Only the native recycling supplier can construct this exact process-query carrier.
    pub fn recycle_probe(authority: crate::recycle::operation::CommandEvidence) -> Self {
        Self::RecycleProbe(Box::new(authority))
    }

    /// The native replacement runner seals its canonical proposal, retaining inner source evidence.
    pub fn environment_update(
        authority: crate::jobs::recreate::batch_authority::environment::Command,
    ) -> Self {
        Self::EnvironmentUpdate(Box::new(authority))
    }

    /// Only the native update supplier can construct this exact file-observation carrier.
    pub fn update_planning_probe(authority: crate::updates::operation::CommandEvidence) -> Self {
        Self::UpdatePlanningProbe(Box::new(authority))
    }

    /// Only the native adoption supplier can construct this closed preparation carrier.
    pub fn adoption_preparation(authority: crate::adoption::operation::CommandEvidence) -> Self {
        Self::AdoptionPreparation(Box::new(authority))
    }

    /// The native kind belongs to the typed producer, never an incoming key claim.
    pub fn command_kind(&self) -> &'static str {
        match self {
            Self::AdoptionPreparation(authority) => authority.command_kind(),
            Self::RecycleProbe(_) => "proc.list",
            Self::UpdatePlanningProbe(_) | Self::UpdateMeasurement(_) => {
                "node.runtime.keep-estimate"
            }
            Self::UpdateJob(authority) => authority.command_kind(),
            Self::EnvironmentUpdate(authority) => authority.command_kind(),
            Self::Login(authority) => authority.command_kind(),
            Self::TerminalClose(_) => "pty.close",
            Self::TerminalCreation(authority) => authority.command_kind(),
            Self::SnapshotRead(_) => "fs.record.read",
            Self::InputSteer(_) => "turn.steer",
            Self::CapabilityChange(_) => "node.capabilities.apply",
            Self::CapabilityRecreation(authority) => authority.command_kind(),
            Self::RetainedLifecycle(authority) => authority.command_kind(),
            Self::SemanticQuery(_) => "provider.embed",
            Self::Continuation(authority) => authority.command_kind(),
            Self::ForkNative(authority) => authority.command_kind(),
            Self::RebindPreparation(authority) => authority.command_kind(),
            Self::ForkPreparation(authority) => authority.command_kind(),
            Self::SideHistory(authority) => authority.command_kind(),
            Self::SemanticEmbedding(_) => "provider.embed",
            Self::Workspace(_) => "fs.measure",
            Self::SnapshotCreation(_) => "fs.snapshot",
            Self::FileCopy(_) => "fs.copy",
            Self::SourcePreparation(authority) => authority.command_kind(),
            Self::WorkspaceRestore(authority) => authority.command_kind(),
            Self::HarnessCheck(authority) => authority.command_kind(),
            Self::Setting(authority) => authority.command_kind(),
            Self::Cleanup(authority) => authority.command_kind(),
            Self::HostPortsProbe(_) => "host.ports",
            Self::MachineProbe(_) => "machine.probe",
            Self::MachinePrograms(_) => "machine.programs",
            Self::MachineRoles(_) => "machine.roles.set",
            Self::MachinePolicy(_) | Self::HostPolicy(_) => "machine.policy.set",
            Self::ComponentUpdate(_) => "machine.update.now",
            Self::TemplateImport(_) => "node.template.import",
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
            | Self::CapabilityRecreation(_)
            | Self::RetainedLifecycle(_)
            | Self::SemanticQuery(_)
            | Self::Continuation(_)
            | Self::ForkNative(_)
            | Self::RebindPreparation(_)
            | Self::ForkPreparation(_)
            | Self::SideHistory(_)
            | Self::SemanticEmbedding(_)
            | Self::Workspace(_)
            | Self::SnapshotCreation(_)
            | Self::FileCopy(_)
            | Self::SourcePreparation(_)
            | Self::Cleanup(_)
            | Self::WorkspaceRestore(_)
            | Self::AdoptionPreparation(_)
            | Self::RecycleProbe(_)
            | Self::UpdatePlanningProbe(_)
            | Self::UpdateMeasurement(_)
            | Self::UpdateJob(_)
            | Self::EnvironmentUpdate(_)
            | Self::Setting(_)
            | Self::HostPortsProbe(_)
            | Self::MachineProbe(_)
            | Self::MachinePrograms(_)
            | Self::MachineRoles(_)
            | Self::MachinePolicy(_)
            | Self::HostPolicy(_)
            | Self::ComponentUpdate(_)
            | Self::TemplateImport(_)
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
