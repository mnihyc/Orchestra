//! Environment execution context.
//!
//! An executor's whole job is to start something inside the environment that speaks the protocol a harness
//! expects, and to hand back a loopback address. What it is given to do that is an [`EnvironmentContext`]:
//! the environment, the one directory it may keep everything under, where the work is, the tools the
//! environment has, and the limits it runs within.
//!
//! **An exec node is simple, and there are many.** It is one process that keeps everything it has under its
//! own data directory, listens on ports the system gives it, and holds no lock on anything outside: several
//! run on one host side by side, one for each environment there. An executor from a package is written the
//! same way, or it cannot be offered by two environments of one host: nothing fixed outside the directory
//! it is given, no fixed port, no state shared between two of its processes.

use std::collections::BTreeMap;

use orch_agent_contract::facts::{Arch, Os, PathStyle, ShellInfo};
use orch_driver_contract::mounts::AttachedMount;
use orch_driver_contract::runtime::{Resources, ToolRef};
use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What an executor is given about the environment it starts in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentContext {
    /// A native launcher before the program and its arguments; empty uses the ordinary process boundary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub process_launcher: Vec<String>,
    /// The environment.
    pub environment_id: Id,
    /// The generation of its unit, which changes when the unit is replaced.
    pub generation: i64,
    /// The directory that is the executor's own. Everything it keeps lives under it, and nothing it keeps
    /// is outside it.
    pub data_dir: String,
    /// Where the environment's workspace is.
    pub workspace_root: String,
    /// The user's home, where the environment has one.
    pub home: Option<String>,
    /// Where the harness builds are found: a directory with one directory for each tool. Absent where the
    /// environment has no tool cache.
    pub tools_dir: Option<String>,
    /// The tools the environment was made with: the builds an executor of a harness's own may run.
    pub tools: Vec<ToolRef>,
    /// The mounts that were attached: further roots, readable, and writable unless read-only.
    pub mounts: Vec<AttachedMount>,
    /// The user the connector, and so the executor, runs as.
    pub user: String,
    /// The environment's shell.
    pub shell: ShellInfo,
    /// Its operating system.
    pub os: Os,
    /// Its architecture.
    pub arch: Arch,
    /// How paths are written there.
    pub path_style: PathStyle,
    /// What the environment may use at most.
    pub limits: Resources,
    /// The variables the agent's commands receive, the environment's own and its secrets among them.
    pub variables: BTreeMap<String, String>,
    /// Whether at least one MCP server is enabled for the environment: a tool set made of those servers
    /// alone is offered only then.
    pub mcp_servers_enabled: bool,
}

impl EnvironmentContext {
    /// The native process prefix and then this program, preserving arguments as separate words.
    #[must_use]
    pub fn launch(&self, program: impl AsRef<std::ffi::OsStr>) -> Vec<std::ffi::OsString> {
        let mut argv: Vec<_> = self
            .process_launcher
            .iter()
            .map(std::ffi::OsString::from)
            .collect();
        argv.push(program.as_ref().to_owned());
        argv
    }
    /// The build of `tool` the environment holds, if it holds one.
    #[must_use]
    pub fn tool(&self, name: &str) -> Option<&ToolRef> {
        self.tools.iter().find(|tool| tool.name == name)
    }

    /// The directory of a build under the tools directory: `<tools dir>/<name>`, where there is one.
    #[must_use]
    pub fn tool_dir(&self, name: &str) -> Option<String> {
        let dir = self.tools_dir.as_deref()?;
        self.tool(name)?;
        Some(format!("{}/{name}", dir.trim_end_matches('/')))
    }

    /// The roots the agent may reach besides the workspace: the mounts.
    pub fn extra_roots(&self) -> impl Iterator<Item = &str> {
        self.mounts.iter().map(|mount| mount.path.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn context() -> EnvironmentContext {
        serde_json::from_value(json!({
            "environment_id": "0199abcd-1234-7000-8000-0000000000e1", "generation": 3,
            "data_dir": "/var/lib/orch/exec/e1", "workspace_root": "/workspace", "home": "/home/agent",
            "tools_dir": "/opt/orch/tools/",
            "tools": [ { "name": "codex", "version": "0.160.1" } ],
            "mounts": [ { "name": "src", "path": "/data/src", "mode": "read_only" } ],
            "user": "agent", "shell": { "name": "bash", "path": "/usr/bin/bash" },
            "os": "linux", "arch": "x86_64", "path_style": "posix",
            "limits": { "cpu_millis": 4000, "memory_mb": 8192 },
            "variables": { "NAME": "value" }, "mcp_servers_enabled": false
        }))
        .unwrap()
    }

    #[test]
    fn what_an_executor_is_given_reads_as_the_node_writes_it() {
        let context = context();
        assert_eq!(context.generation, 3);
        assert_eq!(context.limits.memory_mb, Some(8192));
        assert_eq!(context.extra_roots().collect::<Vec<_>>(), ["/data/src"]);
        assert_eq!(serde_json::to_value(&context).unwrap()["os"], "linux");
    }

    #[test]
    fn a_build_is_found_under_the_tools_directory_only_when_the_environment_holds_it() {
        let context = context();
        assert_eq!(
            context.tool("codex").map(|tool| tool.version.as_str()),
            Some("0.160.1")
        );
        assert_eq!(
            context.tool_dir("codex").as_deref(),
            Some("/opt/orch/tools/codex")
        );
        assert_eq!(context.tool_dir("claude"), None);
        let mut without = context;
        without.tools_dir = None;
        assert_eq!(without.tool_dir("codex"), None);
    }
}
