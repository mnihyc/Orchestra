//! Resource rows contain table and menu facts; configuration belongs to each resource's own reader.

use crate::access::ResourceAccess;
use crate::environment::{
    Amounts, Environment, EnvironmentRef, EnvironmentRemoval, EnvironmentTemplate,
    EnvironmentTemporary, HostStorage, PendingUpdate,
};
use crate::mounts::{Mount, MountLocation, MountRef};
use crate::nodes::{JobRef, NodeRef};
use crate::resources::{
    Agent, AgentRef, ComponentUpdate, Host, HostDriver, HostRef, HostRunning, HostUsage, Machine,
    MachineRef, MachineRoleCapability, MachineStatus, McpServer, McpServerImported, Provider,
    ProviderChoiceUsage, ProviderFacts, ProviderFactsState, ProviderRef, ProviderRuntime,
    ProviderRuntimeSupport, Template, UnitKind,
};
use crate::skills::{Skill, SkillVersionInfo};
use crate::users::UserRef;
use orch_types::{Id, Json, Revision, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentListRow {
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub list_row: bool,
    #[serde(flatten)]
    pub base: AgentRef,
    pub owner: Option<UserRef>,
    pub home: Option<NodeRef>,
    pub profile: Json,
    pub revision: Revision,
    pub access: ResourceAccess,
    pub org_id: Option<Id>,
}

impl From<&Agent> for AgentListRow {
    fn from(row: &Agent) -> Self {
        Self {
            list_row: true,
            org_id: row.org_id,
            base: row.base.clone(),
            owner: row.owner.clone(),
            home: row.home.clone(),
            profile: row.profile.clone(),
            revision: row.revision,
            access: row.access.clone(),
        }
    }
}

/// A provider's runtime location and update state, without its network or lifecycle configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderRuntimeRow {
    pub state: String,
    pub host: Option<HostRef>,
    pub updates: Vec<PendingUpdate>,
    pub update_required: bool,
}

impl From<&ProviderRuntime> for ProviderRuntimeRow {
    fn from(runtime: &ProviderRuntime) -> Self {
        Self {
            state: runtime.state.clone(),
            host: runtime.host.clone(),
            updates: runtime.updates.clone(),
            update_required: runtime.update_required,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderListRow {
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub list_row: bool,
    #[serde(flatten)]
    pub base: ProviderRef,
    pub owner: UserRef,
    pub home: NodeRef,
    pub key_set: bool,
    pub key_machine: Option<MachineRef>,
    pub machine: Option<MachineRef>,
    pub status_detail: Option<String>,
    pub status_at: Time,
    pub facts: ProviderFacts,
    #[serde(default)]
    pub facts_state: ProviderFactsState,
    pub models_count: i64,
    pub running_threads: i64,
    pub runtime: Option<ProviderRuntimeRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_support: Option<ProviderRuntimeSupport>,
    pub revision: Revision,
    pub access: ResourceAccess,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice_usage: Option<ProviderChoiceUsage>,
    #[serde(default)]
    pub enabled_models_count: i64,
    #[serde(default)]
    pub upstreams_count: i64,
    #[serde(default)]
    pub no_key_needed: bool,
    #[serde(default)]
    pub plan_label: Option<String>,
    #[serde(default)]
    pub source_consent: bool,
    pub org_id: Id,
}

impl From<&Provider> for ProviderListRow {
    fn from(row: &Provider) -> Self {
        Self {
            list_row: true,
            org_id: row.org_id,
            base: row.base.clone(),
            owner: row.owner.clone(),
            home: row.home.clone(),
            key_set: row.key_set,
            key_machine: row.key_machine.clone(),
            machine: row.machine.clone(),
            status_detail: row.status_detail.clone(),
            status_at: row.status_at,
            facts: row.facts.clone(),
            facts_state: row.facts_state.clone(),
            models_count: row.models_count,
            running_threads: row.running_threads,
            runtime: row.runtime.as_ref().map(ProviderRuntimeRow::from),
            runtime_support: row.runtime_support.clone(),
            revision: row.revision,
            access: row.access.clone(),
            choice_usage: row.choice_usage.clone(),
            enabled_models_count: i64::try_from(
                row.models.iter().filter(|model| model.enabled).count(),
            )
            .unwrap_or(i64::MAX),
            upstreams_count: i64::try_from(row.upstreams.len()).unwrap_or(i64::MAX),
            no_key_needed: !row.upstreams.is_empty()
                && row.upstreams.iter().all(|upstream| {
                    upstream.options.get("auth").and_then(Json::as_str) == Some("none")
                }),
            plan_label: row.facts.plan.as_ref().map(|plan| {
                row.sources
                    .facts
                    .descriptor
                    .as_ref()
                    .and_then(|descriptor| descriptor.facts.as_ref())
                    .and_then(|facts| facts.plans.get(plan))
                    .cloned()
                    .unwrap_or_else(|| plan.clone())
            }),
            source_consent: [
                Some(&row.sources.facts),
                Some(&row.sources.models),
                row.sources.presenter.as_ref(),
            ]
            .into_iter()
            .flatten()
            .any(|source| source.address.is_some()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentListRow {
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub list_row: bool,
    #[serde(flatten)]
    pub base: EnvironmentRef,
    pub machine: Option<MachineRef>,
    pub home: NodeRef,
    pub threads_hidden: i64,
    pub workspace_root: String,
    pub template: Option<EnvironmentTemplate>,
    pub host: Option<HostRef>,
    pub updates: Vec<PendingUpdate>,
    pub update_required: bool,
    pub status_detail: Option<String>,
    pub job: Option<JobRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporary: Option<EnvironmentTemporary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_update: Option<crate::resources::ComponentUpdate>,
    pub removal: Option<EnvironmentRemoval>,
    pub remove_at: Option<Time>,
    pub last_activity_at: Option<Time>,
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub can_declare_lost: bool,
    pub access: ResourceAccess,
    pub revision: Revision,
    #[serde(default)]
    pub threads_count: i64,
    #[serde(default)]
    pub topics_count: i64,
    #[serde(default)]
    pub start_allowed: bool,
    pub org_id: Id,
}

impl From<&Environment> for EnvironmentListRow {
    fn from(row: &Environment) -> Self {
        Self {
            list_row: true,
            org_id: row.org_id,
            base: row.base.clone(),
            machine: row.machine.clone(),
            home: row.home.clone(),
            threads_hidden: row.threads_hidden,
            workspace_root: row.workspace_root.clone(),
            template: row.template.clone(),
            host: row.host.clone(),
            updates: row.updates.clone(),
            update_required: row.update_required,
            status_detail: row.status_detail.clone(),
            job: row.job.clone(),
            temporary: row.temporary.clone(),
            connector_update: row.connector_update.clone(),
            removal: row.removal.clone(),
            remove_at: row.remove_at,
            last_activity_at: row.last_activity_at,
            can_declare_lost: row.can_declare_lost,
            access: row.access.clone(),
            revision: row.revision,
            threads_count: i64::try_from(row.threads.len()).unwrap_or(i64::MAX),
            topics_count: i64::try_from(
                row.threads
                    .iter()
                    .map(|thread| thread.topic.id)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len(),
            )
            .unwrap_or(i64::MAX),
            start_allowed: !(row.network.required_mode
                == crate::environment::EgressMode::Restricted
                && row.network.mode == crate::environment::EgressMode::Open),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HostListRow {
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub list_row: bool,
    #[serde(flatten)]
    pub base: HostRef,
    pub owner: Option<UserRef>,
    pub home: Option<NodeRef>,
    pub machine: MachineRef,
    pub labels: Vec<String>,
    pub drivers: Vec<HostDriver>,
    pub capacity: Amounts,
    pub declared: Amounts,
    pub schedulable: bool,
    pub running: HostRunning,
    pub revision: Revision,
    pub access: ResourceAccess,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost_at: Option<Time>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<HostUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_since: Option<Time>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub software_update: Option<ComponentUpdate>,
    pub storages: Vec<HostStorage>,
    pub org_id: Option<Id>,
}

impl From<&Host> for HostListRow {
    fn from(row: &Host) -> Self {
        Self {
            list_row: true,
            org_id: row.org_id,
            base: row.base.clone(),
            owner: row.owner.clone(),
            home: row.home.clone(),
            machine: row.machine.clone(),
            labels: row.labels.clone(),
            drivers: row.drivers.clone(),
            capacity: row.capacity.clone(),
            declared: row.declared.clone(),
            schedulable: row.schedulable,
            running: row.running,
            revision: row.revision,
            access: row.access.clone(),
            lost_at: row.lost_at,
            full: row.full,
            usage: row.usage.clone(),
            offline_since: row.offline_since,
            software_update: row.software_update.clone(),
            storages: row.storages.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MachineListRow {
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub list_row: bool,
    #[serde(flatten)]
    pub base: MachineRef,
    pub revision: Revision,
    pub owner: Option<UserRef>,
    pub platform_arch: String,
    pub connector_version: String,
    pub roles: Vec<String>,
    pub status: MachineStatus,
    pub last_seen_at: Option<Time>,
    pub cert_not_after: Time,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<crate::access::ResourceAccess>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost_at: Option<Time>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub software_update: Option<ComponentUpdate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_management: Option<MachineRoleCapability>,
}

impl From<&Machine> for MachineListRow {
    fn from(row: &Machine) -> Self {
        Self {
            list_row: true,
            base: row.base.clone(),
            revision: row.revision,
            owner: row.owner.clone(),
            platform_arch: row.platform_arch.clone(),
            connector_version: row.connector_version.clone(),
            roles: row.roles.clone(),
            status: row.status,
            last_seen_at: row.last_seen_at,
            cert_not_after: row.cert_not_after,
            access: row.access.clone(),
            host: row.host.clone(),
            lost_at: row.lost_at,
            software_update: row.software_update.clone(),
            role_management: row.role_management.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TemplateListRow {
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub list_row: bool,
    pub id: Id,
    pub name: String,
    pub owner: Option<UserRef>,
    pub home: Option<NodeRef>,
    pub address: String,
    pub kind: UnitKind,
    pub driver: String,
    pub spec: Json,
    pub units: i64,
    pub enabled: bool,
    pub revision: Revision,
    pub access: ResourceAccess,
    pub org_id: Option<Id>,
}

impl From<&Template> for TemplateListRow {
    fn from(row: &Template) -> Self {
        Self {
            list_row: true,
            org_id: row.org_id,
            id: row.id,
            name: row.name.clone(),
            owner: row.owner.clone(),
            home: row.home.clone(),
            address: row.address.clone(),
            kind: row.kind,
            driver: row.driver.clone(),
            spec: row.spec.clone(),
            units: row.units,
            enabled: row.enabled,
            revision: row.revision,
            access: row.access.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct McpServerListRow {
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub list_row: bool,
    pub id: Id,
    pub name: String,
    pub description: String,
    pub owner: Option<UserRef>,
    pub home: Option<NodeRef>,
    pub address: String,
    pub machine: Option<MachineRef>,
    pub environment: Option<EnvironmentRef>,
    pub imported: Option<McpServerImported>,
    pub spec: Json,
    pub enabled: bool,
    pub revision: Revision,
    pub access: ResourceAccess,
    pub org_id: Option<Id>,
}

impl From<&McpServer> for McpServerListRow {
    fn from(row: &McpServer) -> Self {
        Self {
            list_row: true,
            org_id: row.org_id,
            id: row.id,
            name: row.name.clone(),
            description: row.description.clone(),
            owner: row.owner.clone(),
            home: row.home.clone(),
            address: row.address.clone(),
            machine: row.machine.clone(),
            environment: row.environment.clone(),
            imported: row.imported.clone(),
            spec: row.spec.clone(),
            enabled: row.enabled,
            revision: row.revision,
            access: row.access.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MountListRow {
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub list_row: bool,
    #[serde(flatten)]
    pub base: MountRef,
    pub description: String,
    pub home: Option<NodeRef>,
    pub owner: Option<UserRef>,
    pub target: String,
    pub read_only: bool,
    pub enabled: bool,
    pub locations: Vec<MountLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations_hidden: Option<usize>,
    pub revision: Revision,
    pub access: ResourceAccess,
    pub org_id: Option<Id>,
}

impl From<&Mount> for MountListRow {
    fn from(row: &Mount) -> Self {
        Self {
            list_row: true,
            org_id: row.org_id,
            base: row.base.clone(),
            description: row.description.clone(),
            home: row.home.clone(),
            owner: row.owner.clone(),
            target: row.target.clone(),
            read_only: row.read_only,
            enabled: row.enabled,
            locations: row.locations.clone(),
            locations_hidden: row.locations_hidden,
            revision: row.revision,
            access: row.access.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SkillListRow {
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub list_row: bool,
    pub id: Id,
    pub name: String,
    pub description: String,
    pub owner: Option<UserRef>,
    pub home: NodeRef,
    pub address: String,
    pub enabled: bool,
    pub latest: Option<SkillVersionInfo>,
    pub revision: Revision,
    pub access: ResourceAccess,
    pub org_id: Id,
}

impl From<&Skill> for SkillListRow {
    fn from(row: &Skill) -> Self {
        Self {
            list_row: true,
            org_id: row.org_id,
            id: row.id,
            name: row.name.clone(),
            description: row.description.clone(),
            owner: row.owner.clone(),
            home: row.home.clone(),
            address: row.address.clone(),
            enabled: row.enabled,
            latest: row.latest.clone(),
            revision: row.revision,
            access: row.access.clone(),
        }
    }
}

/// The compact view selected by a resource list, independently of a full resource read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ResourceListRow {
    Agent(Box<AgentListRow>),
    Provider(Box<ProviderListRow>),
    Environment(Box<EnvironmentListRow>),
    Host(Box<HostListRow>),
    Machine(Box<MachineListRow>),
    Template(Box<TemplateListRow>),
    McpServer(Box<McpServerListRow>),
    Mount(Box<MountListRow>),
    Skill(Box<SkillListRow>),
}

/// One home's current sharing rows, carried once per list page and never borrowed from another reader.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceSharing {
    pub home_id: Id,
    pub rows: Option<Vec<crate::access::PermissionRow>>,
    pub problem: Option<crate::common::Problem>,
}
