//! Paged administration of agent programs, their immutable builds and the capabilities proved for them.

use crate::admin::{HarnessArch, HarnessBuild, HarnessBuildStatus, HarnessChannelsStatus};
use crate::common::Page;
use crate::programs::ProgramInfo;
use crate::resources::{HarnessKindName, MachineOs};
use crate::routes::Access::Session;
use orch_agent_programs::Localized;
use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One platform's independent release selection and newest checked build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramPlatform {
    /// Operating system.
    pub os: MachineOs,
    /// Processor architecture.
    pub arch: HarnessArch,
    /// Build shipped by this release, independent of administrator defaults.
    pub shipped: Option<String>,
    /// Newest known build, independently of the current default.
    pub newest: Option<String>,
    /// Newest build whose interface and native checks passed.
    pub newest_checked: Option<String>,
    /// Current default selected by an administrator or the release.
    pub default_version: Option<String>,
    /// Number of known immutable builds for this platform.
    pub builds: i64,
}

/// State of this program's release channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProgramChannelState {
    /// No automatic release source was declared.
    None,
    /// A source exists but has not been read successfully.
    Unread,
    /// A successful read has been recorded.
    Ready,
    /// The latest attempt failed.
    Failed,
    /// Scheduled reads are disabled by the system interval.
    Disabled,
}

/// A compact program row; summaries never contain the complete build table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramSummary {
    /// Declared names, with the stable kind shown beside them for administrators.
    pub harness: HarnessKindName,
    /// Whether the release ships this program.
    pub builtin: bool,
    /// Wire adapter that drives this program; null if no manifest has been reported.
    pub protocol: Option<String>,
    /// Live profile count, aggregated by the database.
    pub profiles: i64,
    /// Active machines reporting this adapter, aggregated by the database.
    pub machines: i64,
    /// Platform selections computed by the server, independent of page boundaries.
    pub platforms: Vec<ProgramPlatform>,
    /// Release channel state for this program.
    pub channel: ProgramChannelState,
    /// Result of the newest known build; null when no build exists.
    pub check: Option<HarnessBuildStatus>,
}

/// A declaration's availability, with its localized reason when off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramCapability {
    /// Stable capability identifier.
    pub id: String,
    /// Declared title in each supported language.
    pub title: Localized,
    /// Whether the reported build supports it after its check narrowed the declaration.
    pub available: bool,
    /// Reason when unavailable, including the check's failure when it supplied one.
    pub reason: Option<Localized>,
}

/// Related capabilities arranged by the server catalogue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramCapabilityGroup {
    /// Stable group identifier.
    pub id: String,
    /// Localized short heading.
    pub title: Localized,
    /// All declarations belonging to this group.
    pub items: Vec<ProgramCapability>,
}

/// Everything the program's own page needs before reading one build page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramDetail {
    /// Summary selections, counts and names.
    pub summary: ProgramSummary,
    /// Static manifest projection, null until a manifest is reported.
    pub program: Option<ProgramInfo>,
    /// Running adapter declaration, null before a build has reported it.
    #[schemars(schema_with = "crate::resources::descriptor_schema")]
    pub descriptor: Option<Json>,
    /// Localized groups; null only when the descriptor itself is absent.
    pub capabilities: Option<Vec<ProgramCapabilityGroup>>,
    /// System version choices; the editor writes through the ordinary settings endpoint.
    pub versions: Json,
    /// Current policy resolved independently of the visible page; at most one build per supported platform.
    pub resolved_builds: Vec<HarnessBuild>,
    /// Current channel schedule, read without contacting a vendor.
    pub channels: HarnessChannelsStatus,
}

/// A program page selection. Cursors bind the search and snapshot and cannot cross lists.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminProgramListQuery {
    /// Page size; the ordinary list default applies when omitted.
    pub limit: Option<u32>,
    /// Continuation returned by this list.
    pub cursor: Option<String>,
    /// Search the declared English and translated names and the kind.
    pub q: Option<String>,
}

/// A build page selection for one program.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminProgramBuildListQuery {
    /// Page size; the ordinary list default applies when omitted.
    pub limit: Option<u32>,
    /// Continuation returned with the same program and filters.
    pub cursor: Option<String>,
    /// Operating system filter.
    pub os: Option<MachineOs>,
    /// Processor filter.
    pub arch: Option<HarnessArch>,
    /// Check state filter.
    pub status: Option<HarnessBuildStatus>,
}

/// A build check accepted for background execution; platform rows refresh through their bounded page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramBuildCheckReply {
    /// Number of platform builds newly claimed by this request.
    pub queued: u32,
}

/// The number of newly retained platform builds after an explicit channel read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgramChannelReadReply {
    /// Newly retained platform builds; already known builds are not counted again.
    pub kept: u32,
}

crate::endpoints! {
    /// Reads release channels explicitly and returns a bounded acknowledgement.
    AdminProgramChannelsRead: POST "/admin/programs/{kind}/channels", Session, none, none, json ProgramChannelReadReply, 200;
    /// Fetches and checks one exact version; at most the six supported platform builds are returned.
    AdminProgramVersionFetch: POST "/admin/programs/{kind}/builds/{version}/fetch", Session, none, none, json Vec<HarnessBuild>, 200;
    /// Reads one version, independently of the current build page.
    AdminProgramVersionGet: GET "/admin/programs/{kind}/builds/{version}", Session, none, none, json Vec<HarnessBuild>, 200;
    /// Adds or selects one build without replying with the complete catalogue.
    AdminProgramBuildSet: PUT "/admin/programs/{kind}/builds/{version}", Session, none, json HarnessBuild, json HarnessBuild, 200;
    /// Claims the platform checks of this exact version in the background.
    AdminProgramBuildCheck: POST "/admin/programs/{kind}/builds/{version}/check", Session, none, none, json ProgramBuildCheckReply, 200;
    /// Lists agent programs with server-selected builds and channel/check states.
    AdminProgramList: GET "/admin/programs", Session, query AdminProgramListQuery, none, json Page<ProgramSummary>, 200;
    /// Reads one program's declaration, grouped capabilities and version setup.
    AdminProgramGet: GET "/admin/programs/{kind}", Session, none, none, json ProgramDetail, 200;
    /// Reads one bounded page of a program's builds.
    AdminProgramBuildList: GET "/admin/programs/{kind}/builds", Session, query AdminProgramBuildListQuery, none, json Page<HarnessBuild>, 200;
}
