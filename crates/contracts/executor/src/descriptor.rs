//! Availability, the protocol it speaks, its tool sets, sandbox support and attribution variables.
//!
//! An executor's descriptor says what it is: whether the protocol it speaks is one of its harness's own or
//! MCP; the tool sets it has, when it has more than one, each with a name and one sentence for the screens;
//! what the harness's own sandbox can enforce through it; and which environment variables carry a command's
//! thread and tool-call ids, so that the environment can attribute processes without knowing the executor.
//!
//! Two sides state facts, and neither chooses for a thread. An adapter names the executors its harness can
//! work through; an environment says what its exec node offers, in the order to try them. What an executor
//! reports of itself in an environment is [`ExecutorAvailability`]: whether it can start there now, which
//! tool sets it can offer now, and which sandbox modes its harness can enforce there, probed at start.

use std::collections::BTreeMap;

use orch_agent_contract::descriptor::SandboxMode;
use orch_agent_contract::facts::ExecutorFact;
use orch_plugin_contract::Extensions;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What protocol an executor speaks to the harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorProtocol {
    /// One of the harness's own: the executor is the vendor's program, and carries its protocol.
    Harness,
    /// MCP: the executor offers its tools as an MCP server.
    Mcp,
}

/// One set of tools an executor can offer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ToolSetInfo {
    /// Its name, which an environment's offers and an adapter's entries use.
    pub name: String,
    /// Declared English display title; empty in older reports, never inferred from the stable name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// Display titles in other languages, by language tag.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub titles: BTreeMap<String, String>,
    /// One sentence for the screens, in English.
    pub summary: String,
    /// Summaries in other languages, by language tag.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub summaries: BTreeMap<String, String>,
}

/// The environment variables through which the commands an executor starts carry their ids.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Attribution {
    /// The variable that holds the thread's id.
    pub thread_id: String,
    /// The variable that holds the tool call's id.
    pub tool_call_id: String,
}

/// The ids found in the environment of a process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attributed {
    /// What the thread variable held.
    pub thread_id: Option<String>,
    /// What the tool-call variable held.
    pub tool_call_id: Option<String>,
}

impl Attribution {
    /// The ids a process carries, read from its environment `process_env` by the variable names this
    /// executor declares. A variable that is absent or empty gives nothing.
    #[must_use]
    pub fn read(&self, process_env: &BTreeMap<String, String>) -> Attributed {
        let held = |name: &str| {
            process_env
                .get(name)
                .filter(|value| !value.is_empty())
                .cloned()
        };
        Attributed {
            thread_id: held(&self.thread_id),
            tool_call_id: held(&self.tool_call_id),
        }
    }
}

/// Where an executor of a program's own takes its connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutorEndpoint {
    /// The transport: `websocket` or `http`.
    pub transport: String,
    /// The path it takes its connection at.
    pub path: String,
}

/// What an executor declares about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutorDescriptor {
    /// Its kind, such as `codex-exec-server` or `orch-exec`; the name a package's executor is offered under.
    pub kind: String,
    /// What it is called, in English.
    pub title: String,
    /// What it is called in the other languages, by language tag.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub titles: BTreeMap<String, String>,
    /// The program whose build it runs, for an executor that is a program's own: the unit that offers it holds
    /// that build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// Where an executor of a program's own takes its connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<ExecutorEndpoint>,
    /// Whether it speaks a protocol of its harness's own or offers its tools as an MCP server.
    pub protocol: ExecutorProtocol,
    /// Its tool sets, when it has more than one.
    #[serde(default)]
    pub tool_sets: Vec<ToolSetInfo>,
    /// What the harness's own sandbox can enforce through it, at best; what works in an environment is
    /// probed there.
    pub sandbox_modes: Vec<SandboxMode>,
    /// The variables that carry a command's ids; `null` for an executor that carries none.
    #[serde(default)]
    pub attribution: Option<Attribution>,
    /// Members that belong to the implementation, kept and never interpreted.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// What is wrong with an executor's declaration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DescriptorFault {
    /// It has no kind.
    #[error("an executor has a kind")]
    NoKind,
    /// A tool set has no name, or two share one.
    #[error("tool set {0:?} is unnamed or declared twice")]
    ToolSetName(String),
    /// A tool set has no sentence for the screens.
    #[error("tool set {0:?} has no summary")]
    ToolSetSummary(String),
    /// An executor of a harness's own protocol has no tool sets: the tools are the harness's.
    #[error("an executor that speaks a protocol of its harness's own has no tool sets of its own")]
    ToolSetsWithHarnessProtocol,
    /// It lists no sandbox mode; an executor whose harness has no sandbox lists `open`.
    #[error("an executor lists the sandbox modes it can enforce; none is open")]
    NoSandboxMode,
    /// An attribution names the same variable twice.
    #[error("the thread and the tool call are carried by one variable")]
    AttributionShared,
}

impl ExecutorDescriptor {
    /// What an executor declares of itself as its manifest says it: its title, its tool sets with a sentence each, what
    /// the program's sandbox can enforce through it, and the variables that carry its commands' ids. The manifest is
    /// the one source, so the executor and the screens that name it say the same.
    #[must_use]
    pub fn of_manifest(manifest: &orch_agent_programs::ExecutorManifest) -> Self {
        Self {
            kind: manifest.kind.clone(),
            title: manifest.title.en().to_owned(),
            titles: manifest.title.others(),
            program: manifest.program.clone(),
            endpoint: manifest.endpoint.as_ref().map(|endpoint| ExecutorEndpoint {
                transport: endpoint.transport.clone(),
                path: endpoint.path.clone(),
            }),
            protocol: match manifest.wire {
                orch_agent_programs::executors::ExecutorWire::Program => ExecutorProtocol::Harness,
                orch_agent_programs::executors::ExecutorWire::Mcp => ExecutorProtocol::Mcp,
            },
            tool_sets: manifest
                .tool_sets
                .iter()
                .map(|set| ToolSetInfo {
                    name: set.name.clone(),
                    title: set.title.en().to_owned(),
                    titles: set.title.others(),
                    summary: set.summary.en().to_owned(),
                    summaries: set.summary.others(),
                })
                .collect(),
            sandbox_modes: manifest
                .sandbox_modes
                .iter()
                .filter_map(|mode| {
                    serde_json::from_value(serde_json::Value::String(mode.clone())).ok()
                })
                .collect(),
            attribution: manifest
                .attribution
                .as_ref()
                .map(|attribution| Attribution {
                    thread_id: attribution.thread_id.clone(),
                    tool_call_id: attribution.tool_call_id.clone(),
                }),
            extensions: Extensions::new(),
        }
    }

    /// Whether it has a tool set named `name`.
    #[must_use]
    pub fn has_tool_set(&self, name: &str) -> bool {
        self.tool_sets.iter().any(|set| set.name == name)
    }

    /// Checks that the declaration is consistent with itself. Every fault is returned.
    pub fn check(&self) -> Result<(), Vec<DescriptorFault>> {
        let mut faults = Vec::new();
        if self.kind.is_empty() {
            faults.push(DescriptorFault::NoKind);
        }
        for (position, set) in self.tool_sets.iter().enumerate() {
            if set.name.is_empty()
                || self.tool_sets[..position]
                    .iter()
                    .any(|before| before.name == set.name)
            {
                faults.push(DescriptorFault::ToolSetName(set.name.clone()));
            }
            if set.summary.trim().is_empty() {
                faults.push(DescriptorFault::ToolSetSummary(set.name.clone()));
            }
        }
        if self.protocol == ExecutorProtocol::Harness && !self.tool_sets.is_empty() {
            faults.push(DescriptorFault::ToolSetsWithHarnessProtocol);
        }
        if self.sandbox_modes.is_empty() {
            faults.push(DescriptorFault::NoSandboxMode);
        }
        if self
            .attribution
            .as_ref()
            .is_some_and(|attribution| attribution.thread_id == attribution.tool_call_id)
        {
            faults.push(DescriptorFault::AttributionShared);
        }
        if faults.is_empty() {
            Ok(())
        } else {
            Err(faults)
        }
    }
}

/// What an executor reports of itself in one environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutorAvailability {
    /// Whether it can be started there now.
    pub available: bool,
    /// The build it is; `null` when that is not known because it cannot be started.
    pub version: Option<String>,
    /// Why it cannot, when it cannot.
    pub reason: Option<String>,
    /// What the harness's sandbox can enforce there, probed. An executor lists only what works.
    pub sandbox_modes: Vec<SandboxMode>,
    /// For an executor with several: the tool sets it can offer now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_sets: Option<Vec<String>>,
}

impl ExecutorAvailability {
    /// An executor that cannot be started in the environment, for `reason`.
    #[must_use]
    pub fn unavailable(reason: &str) -> Self {
        Self {
            available: false,
            version: None,
            reason: Some(reason.to_owned()),
            sandbox_modes: Vec::new(),
            tool_sets: None,
        }
    }

    /// An executor of `version` that can be started, with the sandbox modes that work there.
    #[must_use]
    pub fn available(version: &str, sandbox_modes: Vec<SandboxMode>) -> Self {
        Self {
            available: true,
            version: Some(version.to_owned()),
            reason: None,
            sandbox_modes,
            tool_sets: None,
        }
    }

    /// This report as a line of the environment's facts.
    #[must_use]
    pub fn into_fact(self, kind: &str) -> ExecutorFact {
        ExecutorFact {
            kind: kind.to_owned(),
            version: self.version.unwrap_or_default(),
            available: self.available,
            reason: self.reason,
            sandbox_modes: self.sandbox_modes,
            tool_sets: self.tool_sets,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The executor that carries a harness's own protocol.
    fn codex_exec_server() -> serde_json::Value {
        json!({
            "kind": "codex-exec-server", "title": "Codex's own exec server", "protocol": "harness",
            "sandbox_modes": ["read_only", "workspace", "open"],
            "attribution": { "thread_id": "CODEX_THREAD_ID", "tool_call_id": "CODEX_TOOL_CALL_ID" }
        })
    }

    /// Orch's own, which offers its tools as an MCP server in three tool sets.
    fn orch_exec() -> serde_json::Value {
        json!({
            "kind": "orch-exec", "title": "Orch's own tools", "protocol": "mcp",
            "tool_sets": [
                { "name": "claude-code", "summary": "Tools with the names and inputs of Claude Code's own shell and file tools." },
                { "name": "plain", "summary": "Eight plain tools for any harness and any model." },
                { "name": "custom", "summary": "No tools of its own: the agent acts through the environment's MCP servers." } ],
            "sandbox_modes": ["open"],
            "attribution": { "thread_id": "ORCH_THREAD_ID", "tool_call_id": "ORCH_ITEM_ID" }
        })
    }

    #[test]
    fn the_two_built_in_executors_read_and_write_as_their_declarations_say() {
        for written in [codex_exec_server(), orch_exec()] {
            let read: ExecutorDescriptor = serde_json::from_value(written.clone()).unwrap();
            assert_eq!(read.check(), Ok(()));
            let mut back = serde_json::to_value(&read).unwrap();
            // Left out, a list of tool sets is empty: both read the same.
            back.as_object_mut()
                .unwrap()
                .entry("tool_sets")
                .or_insert(json!([]));
            let mut want = written;
            want.as_object_mut()
                .unwrap()
                .entry("tool_sets")
                .or_insert(json!([]));
            assert_eq!(back, want);
        }
        let orch: ExecutorDescriptor = serde_json::from_value(orch_exec()).unwrap();
        assert!(orch.has_tool_set("plain") && !orch.has_tool_set("other"));
        assert_eq!(orch.protocol, ExecutorProtocol::Mcp);
    }

    #[test]
    fn a_processs_ids_are_read_from_the_variables_the_executor_declares() {
        let codex: ExecutorDescriptor = serde_json::from_value(codex_exec_server()).unwrap();
        let process = BTreeMap::from([
            ("CODEX_THREAD_ID".to_owned(), "thr_1".to_owned()),
            ("CODEX_TOOL_CALL_ID".to_owned(), String::new()),
            ("ORCH_THREAD_ID".to_owned(), "someone else's".to_owned()),
        ]);
        let ids = codex.attribution.unwrap().read(&process);
        assert_eq!(ids.thread_id.as_deref(), Some("thr_1"));
        assert_eq!(ids.tool_call_id, None, "an empty variable gives nothing");
    }

    #[test]
    fn a_declaration_that_cannot_hold_together_is_refused() {
        let mut broken: ExecutorDescriptor = serde_json::from_value(codex_exec_server()).unwrap();
        broken.kind.clear();
        broken.tool_sets = vec![
            ToolSetInfo {
                name: "a".into(),
                title: String::new(),
                titles: BTreeMap::new(),
                summaries: BTreeMap::new(),
                summary: "x".into(),
            },
            ToolSetInfo {
                name: "a".into(),
                title: String::new(),
                titles: BTreeMap::new(),
                summaries: BTreeMap::new(),
                summary: " ".into(),
            },
        ];
        broken.sandbox_modes.clear();
        broken.attribution = Some(Attribution {
            thread_id: "X".into(),
            tool_call_id: "X".into(),
        });
        let faults = broken.check().unwrap_err();
        for expected in [
            DescriptorFault::NoKind,
            DescriptorFault::ToolSetName("a".into()),
            DescriptorFault::ToolSetSummary("a".into()),
            DescriptorFault::ToolSetsWithHarnessProtocol,
            DescriptorFault::NoSandboxMode,
            DescriptorFault::AttributionShared,
        ] {
            assert!(faults.contains(&expected), "{expected:?} in {faults:?}");
        }
    }

    #[test]
    fn what_an_executor_reports_of_itself_becomes_a_line_of_the_environments_facts() {
        let mut orch = ExecutorAvailability::available("0.1.0", vec![SandboxMode::Open]);
        orch.tool_sets = Some(vec!["claude-code".into(), "plain".into()]);
        let fact = orch.into_fact("orch-exec");
        assert_eq!(
            (fact.kind.as_str(), fact.version.as_str(), fact.available),
            ("orch-exec", "0.1.0", true)
        );
        assert_eq!(fact.tool_sets.as_deref().map(<[String]>::len), Some(2));
        let missing = ExecutorAvailability::unavailable("the build is not in the tool cache")
            .into_fact("codex-exec-server");
        assert!(!missing.available);
        assert_eq!(
            missing.reason.as_deref(),
            Some("the build is not in the tool cache")
        );
        assert!(
            missing.sandbox_modes.is_empty(),
            "an executor that cannot start enforces nothing"
        );
    }
}
