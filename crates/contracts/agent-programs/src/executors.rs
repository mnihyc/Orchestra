//! The manifest of an executor: the program that carries out a program's tool calls inside an environment.
//!
//! An executor is either one of the program's own (the vendor's exec server, which speaks the vendor's protocol
//! and is the program's build run in another mode) or Orch's, which offers its tools as an MCP server in tool
//! sets. The manifest is what the screens and the engine need without starting one: its title, its tool sets with
//! a name and a sentence each, the build it must be, the endpoint it is reached at, and the variables that carry a
//! command's ids. The executor's own descriptor is built from it, so nothing is written twice.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::localized::Localized;

/// What protocol an executor speaks to the program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorWire {
    /// One of the program's own: the executor is the vendor's program, and carries its protocol.
    Program,
    /// MCP: the executor offers its tools as an MCP server.
    Mcp,
}

/// How an executor of a program's own is reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutorEndpoint {
    /// The transport: `websocket` or `http`.
    pub transport: String,
    /// The path the executor takes its connection at.
    pub path: String,
}

/// One set of tools an executor can offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolSetManifest {
    /// Its name, which an environment's offers and a program's entries use after the executor's: `orch-exec:plain`.
    pub name: String,
    /// What it is called on the screens.
    pub title: Localized,
    /// One sentence for the screens.
    pub summary: Localized,
}

/// The variables through which the commands an executor starts carry their ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutorAttribution {
    /// The variable that holds the thread's id.
    pub thread_id: String,
    /// The variable that holds the tool call's id.
    pub tool_call_id: String,
}

/// The manifest of an executor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutorManifest {
    /// Its kind, such as `orch-exec`: the name an environment offers it under.
    pub kind: String,
    /// What it is called on the screens.
    pub title: Localized,
    /// Whether it speaks a protocol of its program's own or offers its tools as an MCP server.
    pub wire: ExecutorWire,
    /// The program whose build it runs, for an executor that is a program's own; the unit that offers it holds
    /// that build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// The tool in the environment's tool cache whose build is run; the program's kind when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Which build of it will do for the program it serves: `same`, `*` or a semver range.
    #[serde(default = "same")]
    pub build: String,
    /// Where an executor of a program's own is reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<ExecutorEndpoint>,
    /// Its tool sets, when it has more than one.
    #[serde(default)]
    pub tool_sets: Vec<ToolSetManifest>,
    /// What the program's own sandbox can enforce through it at best, as `read_only`, `workspace` and `open`.
    pub sandbox_modes: Vec<String>,
    /// The variables that carry a command's ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<ExecutorAttribution>,
}

fn same() -> String {
    "same".to_owned()
}

impl ExecutorManifest {
    /// Whether it has a tool set named `name`.
    #[must_use]
    pub fn has_tool_set(&self, name: &str) -> bool {
        self.tool_sets.iter().any(|set| set.name == name)
    }

    /// The tool set named `name`.
    #[must_use]
    pub fn tool_set(&self, name: &str) -> Option<&ToolSetManifest> {
        self.tool_sets.iter().find(|set| set.name == name)
    }

    /// The tool in the tool cache whose build the executor runs, for an executor that is a program's own.
    #[must_use]
    pub fn tool_name(&self) -> Option<&str> {
        self.tool.as_deref().or(self.program.as_deref())
    }

    /// What is wrong with the manifest, in words for its author.
    #[must_use]
    pub fn faults(&self) -> Vec<String> {
        let mut faults = Vec::new();
        if self.kind.trim().is_empty() {
            faults.push("an executor has a kind".to_owned());
        }
        if !self.title.is_complete() {
            faults.push(format!("executor {} has no English title", self.kind));
        }
        let mut seen: Vec<&str> = Vec::new();
        for set in &self.tool_sets {
            if set.name.is_empty() || seen.contains(&set.name.as_str()) {
                faults.push(format!(
                    "executor {}: tool set {:?} is unnamed or declared twice",
                    self.kind, set.name
                ));
            }
            seen.push(&set.name);
            if !set.title.is_complete() || !set.summary.is_complete() {
                faults.push(format!(
                    "executor {}: tool set {} has no title or summary",
                    self.kind, set.name
                ));
            }
        }
        if self.wire == ExecutorWire::Program && !self.tool_sets.is_empty() {
            faults.push(format!(
                "executor {} speaks its program's protocol and so has no tool sets of its own",
                self.kind
            ));
        }
        if self.wire == ExecutorWire::Program && self.program.is_none() {
            faults.push(format!("executor {} names no program", self.kind));
        }
        if self.sandbox_modes.is_empty() {
            faults.push(format!("executor {} lists no sandbox mode", self.kind));
        }
        faults
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_executor_of_a_programs_own_has_no_tool_sets_and_names_its_program() {
        let own: ExecutorManifest = serde_json::from_value(json!({
            "kind": "exec-own", "title": { "en": "Own" }, "wire": "program", "program": "tiny",
            "sandbox_modes": ["open"]
        }))
        .unwrap();
        assert!(own.faults().is_empty());
        assert_eq!(own.build, "same");
        assert_eq!(own.tool_name(), Some("tiny"));
        let mut broken = own.clone();
        broken.program = None;
        broken.tool_sets = vec![ToolSetManifest {
            name: "x".to_owned(),
            title: Localized::english("X"),
            summary: Localized::english("An X."),
        }];
        assert_eq!(broken.faults().len(), 2);
    }
}
