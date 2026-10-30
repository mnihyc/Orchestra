//! What a profile is configured with, and what a machine and an environment report of themselves.
//!
//! Declarations say what an implementation can do; facts say what is so now. A connector reports the facts
//! of its machine ([`MachineFacts`]) when it connects, and the facts of each environment it hosts
//! ([`EnvironmentFacts`]) when it attaches to it and whenever they change. The engine reads them to decide
//! which arrangements a thread has, and the interface shows them. An agent profile ([`AgentProfile`]) is
//! how a harness is configured: nothing the adapter declares, but what whoever manages the profile chose.

use std::collections::BTreeMap;

pub use orch_api_contract::native::{EnvironmentFactsBrowser, EnvironmentFactsVnc};
use orch_driver_contract::mounts::MountFact;
pub use orch_driver_contract::runtime::EnvironmentDataSource;
use orch_plugin_contract::PluginStatus;
use orch_types::{Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::descriptor::SandboxMode;
use crate::discovery::McpFound;

/// How the connector reaches the harness on a person's machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachMode {
    /// It starts the harness as a process of its own.
    #[default]
    Spawn,
    /// It connects to a daemon the person already runs, which keeps running as its owner configured it.
    Daemon,
}

/// How the harness is reached, with the same home choice on every runtime machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Attach {
    /// Spawn or daemon; spawn unless given.
    #[serde(default)]
    pub mode: AttachMode,
    /// The harness binary; `null` takes it from local policy.
    #[serde(default)]
    pub binary: Option<String>,
    /// Data made by Orch or an existing directory under this machine's environment.roots.
    /// An old stored path is read as an existing directory; new writes use the typed choice.
    #[serde(default, deserialize_with = "read_home")]
    pub home: Option<orch_driver_contract::runtime::EnvironmentDataSource>,
}

fn read_home<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<orch_driver_contract::runtime::EnvironmentDataSource>, D::Error> {
    use orch_driver_contract::runtime::EnvironmentDataSource;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        Choice(EnvironmentDataSource),
        Path(String),
    }
    Ok(
        Option::<Stored>::deserialize(deserializer)?.map(|home| match home {
            Stored::Choice(choice) => choice,
            Stored::Path(path) => EnvironmentDataSource::ExistingDirectory { path },
        }),
    )
}

/// How a harness is configured: kind, version, and what is merged over the adapter's base configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentProfile {
    /// The exact build required by this profile; a different installed build cannot open a session.
    #[serde(default)]
    pub harness_version: Option<String>,
    /// How the harness is reached where the machine declares attachment support. Absent means spawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach: Option<Attach>,
    /// Adapter-specific overrides, merged over the adapter's base configuration.
    #[serde(default)]
    pub config: Json,
    /// Variables the harness finds in its environment.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// The one profile pin's visible result, retained alongside another runtime notice.
#[must_use]
pub fn build_detail(pin: Option<&str>, installed: &str, detail: Option<String>) -> Option<String> {
    let notice=pin.filter(|pin|!orch_agent_programs::versions::same_release(pin,installed)).map(|pin|
        format!("The installed build {installed} differs from required build {pin}. Update the runtime or choose a matching profile."));
    match (detail, notice) {
        (Some(detail), Some(notice)) => Some(format!("{detail}\n{notice}")),
        (detail, None) => detail,
        (None, notice) => notice,
    }
}

impl AgentProfile {
    /// The visible explanation when the installed build differs from the one profile pin.
    #[must_use]
    pub fn build_notice(&self, installed: &str) -> Option<String> {
        build_detail(self.harness_version.as_deref(), installed, None)
    }
    /// How the harness is reached: spawn unless the profile says daemon.
    #[must_use]
    pub fn attach_mode(&self) -> AttachMode {
        self.attach
            .as_ref()
            .map(|attach| attach.mode)
            .unwrap_or_default()
    }
}

/// A harness found on a machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessFound {
    /// Its kind.
    pub kind: String,
    /// Where it is.
    pub path: String,
    /// Its version.
    pub version: String,
    /// Whether a daemon of it runs.
    pub daemon: bool,
}

/// An adapter a connector can drive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdapterFact {
    /// The harness kind it drives.
    pub kind: String,
    /// The version of its declarations.
    pub adapter_version: u32,
    /// Whether it is built in; otherwise it is a plugin.
    pub builtin: bool,
}

/// How good a machine's link to the gateway is, as the connector measures it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LinkRates {
    /// Bits per second towards the gateway.
    pub up_bps: i64,
    /// Bits per second from the gateway.
    pub down_bps: i64,
    /// The round trip in milliseconds.
    pub round_trip_ms: i64,
}

/// What a connector reports of its machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MachineFacts {
    /// The machine's name.
    pub hostname: String,
    /// The version of its operating system.
    pub os_version: String,
    /// How many processors it has.
    pub cpus: u32,
    /// How much memory it has, in bytes.
    pub memory_bytes: i64,
    /// The harnesses found on it.
    pub harnesses: Vec<HarnessFound>,
    /// The adapters its connector can drive.
    pub adapters: Vec<AdapterFact>,
    /// The external implementations registered on it and how they are doing.
    pub plugins: Vec<PluginStatus>,
    /// Its local policy, as configured on the machine.
    pub policy: Json,
    /// Its link to the gateway.
    pub rates: LinkRates,
    /// How far its clock is from the server's, in milliseconds.
    pub clock_offset_ms: i64,
}

/// The operating systems an environment can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    /// Linux.
    Linux,
    /// macOS.
    Macos,
    /// Windows.
    Windows,
}

/// The processor architectures an environment can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum Arch {
    /// 64-bit x86.
    #[serde(rename = "x86_64")]
    X86_64,
    /// 64-bit ARM.
    #[serde(rename = "aarch64")]
    Aarch64,
}

/// How paths are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PathStyle {
    /// Slashes.
    Posix,
    /// Drive letters and backslashes.
    Windows,
}

/// The shell of an environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShellInfo {
    /// Its name.
    pub name: String,
    /// Where it is.
    pub path: String,
}

/// Whether a manifest of the environment's files is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IndexMode {
    /// A manifest is kept.
    Full,
    /// No manifest is kept for this tree: its files are read live.
    Live,
}

/// An executor as an environment's facts list it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutorFact {
    /// The executor's kind, such as `codex-exec-server` or `orch-exec`.
    pub kind: String,
    /// The build it is.
    pub version: String,
    /// Whether it can be started here now.
    pub available: bool,
    /// Why not, when it cannot.
    pub reason: Option<String>,
    /// What the harness's sandbox can enforce here, probed at start.
    pub sandbox_modes: Vec<SandboxMode>,
    /// For an executor with several: the tool sets it can offer now. `custom` only while an MCP server is
    /// enabled for the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_sets: Option<Vec<String>>,
}

/// What a connector reports of an environment it hosts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentFacts {
    /// When browser and display programs were looked for.
    #[serde(default)]
    pub looked_at: Option<Time>,
    /// Browser support discovered where the environment runs.
    #[serde(default)]
    pub browser: EnvironmentFactsBrowser,
    /// Display support discovered where the environment runs.
    #[serde(default)]
    pub vnc: EnvironmentFactsVnc,

    /// Its operating system.
    pub os: Os,
    /// Its architecture.
    pub arch: Arch,
    /// Its shell.
    pub shell: ShellInfo,
    /// The user the connector runs as.
    pub user: String,
    /// That user's home.
    pub home: String,
    /// Where the environment's workspace is.
    pub workspace_root: String,
    /// How paths are written.
    pub path_style: PathStyle,
    /// Whether file names are case sensitive.
    pub case_sensitive: bool,
    /// The executors it lists, with what each can do here.
    pub executors: Vec<ExecutorFact>,
    /// The MCP servers the harnesses there have configured: their names and never their commands,
    /// addresses or secrets.
    pub mcp_found: Vec<McpFound>,
    /// Whether a manifest is kept.
    pub index: IndexMode,
    /// What became of each mount.
    pub mounts: Vec<MountFact>,
    /// Whether a private view of a mount can be made here.
    pub private_views: bool,
    /// Whether its agents' processes can be held still for a move.
    pub suspend: bool,
    /// Whether git is there.
    pub git: bool,
    /// The generation of the environment's unit.
    pub generation: i64,
}

impl EnvironmentFacts {
    /// The executor of kind `kind`, if the facts list it.
    #[must_use]
    pub fn executor(&self, kind: &str) -> Option<&ExecutorFact> {
        self.executors.iter().find(|executor| executor.kind == kind)
    }

    /// The sandbox modes that can be enforced here by the executor `kind`: those it reports, or none if
    /// it is not listed or not available.
    #[must_use]
    pub fn sandbox_modes(&self, kind: &str) -> &[SandboxMode] {
        self.executor(kind)
            .filter(|executor| executor.available)
            .map_or(&[], |executor| executor.sandbox_modes.as_slice())
    }
}

/// What a provider's sources report of it lives with the provider's contract, written once.
pub use orch_provider_contract::facts::{
    ACCOUNT_BALANCE_ID, AvailableResets, Balance, BalancePart, CredentialFacts, CredentialYield,
    ExtraUsage, FactMember, KEY_SPEND_IDS, KeyCredits, KeyUsage, LimitReached, MemberState,
    PLAN_BALANCE_ID, PlanCredits, ProviderFacts, ResetCredit, ResetRequest, ResetResult, Spend,
    SpendPeriod, UsageWindow, facts_state, member_state, normalize,
};

/// How a harness process is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessState {
    /// It is not running.
    Offline,
    /// It is starting.
    Starting,
    /// It is running and answering.
    Online,
    /// It failed, for the reason given beside it.
    Error,
}

/// The report of a harness process: what its row says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessStatus {
    /// How it is doing.
    pub status: HarnessState,
    /// Why, in a sentence, when that needs saying.
    pub detail: Option<String>,
    /// The version of the harness.
    pub harness_version: Option<String>,
    /// How many sessions it has open.
    pub sessions_open: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn what_a_machine_reports_reads_as_the_spec_writes_it() {
        let facts: MachineFacts = serde_json::from_value(json!({
            "hostname": "build-3", "os_version": "Debian 13", "cpus": 8, "memory_bytes": 16000000000i64,
            "harnesses": [ { "kind": "codex", "path": "/usr/local/bin/codex", "version": "0.161.0", "daemon": true } ],
            "adapters": [ { "kind": "codex", "adapter_version": 1, "builtin": true } ],
            "plugins": [ { "kind": "adapter", "name": "grok", "version": "0.2.0", "state": "running" } ],
            "policy": { "agent.max_sandbox": "workspace" },
            "rates": { "up_bps": 4100000, "down_bps": 9000000, "round_trip_ms": 42 },
            "clock_offset_ms": -12
        }))
        .unwrap();
        assert_eq!(facts.plugins[0].name, "grok");
        assert_eq!(facts.clock_offset_ms, -12);
        assert_eq!(
            serde_json::to_value(&facts).unwrap()["memory_bytes"],
            json!(16_000_000_000i64)
        );
    }

    fn environment() -> EnvironmentFacts {
        serde_json::from_value(json!({
            "os": "linux", "arch": "x86_64",
            "shell": { "name": "bash", "path": "/usr/bin/bash" },
            "user": "agent", "home": "/home/agent", "workspace_root": "/workspace",
            "path_style": "posix", "case_sensitive": true,
            "executors": [
              { "kind": "codex-exec-server", "version": "0.160.1", "available": true, "reason": null, "sandbox_modes": ["open"] },
              { "kind": "orch-exec", "version": "0.1.0", "available": true, "reason": null, "sandbox_modes": ["open"],
                "tool_sets": ["claude-code", "plain", "custom"] } ],
            "mcp_found": [ { "name": "github", "source": "codex", "scope": "user", "directory": null,
                             "transport": "stdio", "digest": "9f2c" } ],
            "index": "full",
            "mounts": [ { "mount_id": "0199abcd-1234-7000-8000-000000000001", "name": "src", "state": "attached",
                          "reason": null, "path": "/data/src", "mode": "private", "asked": "private", "enforced": true } ],
            "private_views": true, "suspend": true, "git": true, "generation": 3
        }))
        .unwrap()
    }

    #[test]
    fn what_an_environment_reports_reads_as_the_spec_writes_it() {
        let facts = environment();
        assert_eq!(facts.arch, Arch::X86_64);
        assert_eq!(
            facts
                .executor("orch-exec")
                .unwrap()
                .tool_sets
                .as_deref()
                .unwrap()
                .len(),
            3
        );
        assert!(
            facts
                .executor("codex-exec-server")
                .unwrap()
                .tool_sets
                .is_none()
        );
        assert_eq!(facts.sandbox_modes("orch-exec"), [SandboxMode::Open]);
        assert!(facts.sandbox_modes("nothing").is_empty());
        let written = serde_json::to_value(&facts).unwrap();
        assert_eq!(written["arch"], json!("x86_64"));
        assert!(written["executors"][0].get("tool_sets").is_none());
    }

    #[test]
    fn an_executor_that_is_not_available_enforces_nothing_here() {
        let mut facts = environment();
        facts.executors[0].available = false;
        facts.executors[0].reason = Some("build missing".into());
        assert!(facts.sandbox_modes("codex-exec-server").is_empty());
    }

    #[test]
    fn a_profile_spawns_unless_it_says_daemon() {
        let profile: AgentProfile = serde_json::from_value(json!({
            "harness_version": "0.160.1",
            "attach": { "mode": "daemon", "binary": null, "home": "/home/me/.codex" },
            "config": {}, "env": { "NAME": "value" }
        }))
        .unwrap();
        assert_eq!(profile.attach_mode(), AttachMode::Daemon);
        assert_eq!(AgentProfile::default().attach_mode(), AttachMode::Spawn);
        let bare: AgentProfile =
            serde_json::from_value(json!({ "attach": { "binary": "/opt/codex" } })).unwrap();
        assert_eq!(bare.attach_mode(), AttachMode::Spawn);
    }

    #[test]
    fn the_pin_names_the_actual_and_required_versions_and_the_way_on_without_hiding_other_notices()
    {
        let profile = AgentProfile {
            harness_version: Some("2.0.0".into()),
            ..Default::default()
        };
        assert!(profile.build_notice("2.0.0").is_none());
        let visible = build_detail(
            profile.harness_version.as_deref(),
            "1.0.0",
            Some("Commands after start are off.".into()),
        )
        .unwrap();
        assert!(visible.contains("Commands after start are off."));
        assert!(
            visible.contains("installed build 1.0.0")
                && visible.contains("required build 2.0.0")
                && visible.contains("Update the runtime")
                && visible.contains("matching profile")
        );
    }

    #[test]
    fn a_pin_and_the_installed_build_of_one_release_agree_whichever_way_each_is_spelled() {
        // A Python index calls the release `0.2.1rc2`; the program prints, and the profile pins, `0.2.1-rc.2`.
        assert!(build_detail(Some("0.2.1rc2"), "0.2.1-rc.2", None).is_none());
        assert!(build_detail(Some("0.2.1-rc.2"), "0.2.1rc2", None).is_none());
        assert!(build_detail(Some("0.2.1-rc.2"), "0.2.1-rc.2", None).is_none());
        let other = build_detail(Some("0.2.1rc3"), "0.2.1-rc.2", None).unwrap();
        assert!(other.contains("required build 0.2.1rc3"), "{other}");
        assert!(build_detail(Some("0.2.1"), "0.2.1-rc.2", None).is_some());
    }

    #[test]
    fn a_harness_home_has_one_choice_and_old_paths_read_as_borrowed_data() {
        for home in [
            json!("/srv/harness"),
            json!({"kind":"existing_directory","path":"/srv/harness"}),
        ] {
            let profile: AgentProfile =
                serde_json::from_value(json!({"attach":{"home":home}})).unwrap();
            assert_eq!(
                profile
                    .attach
                    .as_ref()
                    .unwrap()
                    .home
                    .as_ref()
                    .unwrap()
                    .directory(),
                Some("/srv/harness")
            );
            assert_eq!(
                serde_json::to_value(profile).unwrap()["attach"]["home"],
                json!({"kind":"existing_directory","path":"/srv/harness"})
            );
        }
        let owned: AgentProfile =
            serde_json::from_value(json!({"attach":{"home":{"kind":"made_by_orch"}}})).unwrap();
        assert_eq!(
            owned.attach.unwrap().home,
            Some(EnvironmentDataSource::MadeByOrch)
        );
    }

    #[test]
    fn a_harness_process_reports_its_state_as_its_row_says() {
        let status: HarnessStatus = serde_json::from_value(
            json!({ "status": "online", "detail": null, "harness_version": "0.160.1", "sessions_open": 4 }),
        )
        .unwrap();
        assert_eq!(status.status, HarnessState::Online);
    }
}
