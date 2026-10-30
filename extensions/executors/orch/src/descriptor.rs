//! Executor identity, its three tool sets and attribution.
//!
//! What `orch-exec` says of itself to the environment that offers it: its kind, that it speaks MCP, the
//! three tool sets it can offer, what its sandbox can enforce, and the two variables every command it starts
//! carries so that the environment can say which thread a process belongs to.
//!
//! It declares the sandbox mode `open` only. The harness's own sandbox cannot reach commands that this
//! executor carries out, and a shell command is held in by the environment, not by the executor; the file
//! tools do refuse paths outside the thread's directory in the other two modes (see [`crate::sandbox`]), but
//! that is not a promise about a command.

use orch_agent_contract::descriptor::SandboxMode;
use orch_executor_contract::descriptor::{Attribution, ExecutorAvailability, ExecutorDescriptor};

use crate::tools::ToolSet;

/// The kind an environment offers this executor under.
pub const KIND: &str = "orch-exec";

/// The variable that holds the thread's id in every command this executor starts.
pub const THREAD_VARIABLE: &str = "ORCH_THREAD_ID";

/// The variable that holds the tool call's id in every command this executor starts.
pub const ITEM_VARIABLE: &str = "ORCH_ITEM_ID";

/// What it is called on the screens: its manifest's title.
#[must_use]
pub fn title() -> &'static str {
    orch_agent_programs::shipped_executor(KIND).title.en()
}

/// What the sandbox of a harness can enforce through this executor. A command is held in by the
/// environment and not by the executor, so `open` is all it can claim; its manifest says so.
#[must_use]
pub fn sandbox_modes() -> Vec<SandboxMode> {
    descriptor().sandbox_modes
}

/// The build this executor is: the version of the program it is part of.
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The variables through which a command carries its ids.
#[must_use]
pub fn attribution() -> Attribution {
    Attribution {
        thread_id: THREAD_VARIABLE.to_owned(),
        tool_call_id: ITEM_VARIABLE.to_owned(),
    }
}

/// What the executor declares about itself.
#[must_use]
pub fn descriptor() -> ExecutorDescriptor {
    ExecutorDescriptor::of_manifest(orch_agent_programs::shipped_executor(KIND))
}

/// What the executor reports of itself in an environment where it can be started: the tool sets it can
/// offer now (`custom` only while an MCP server is enabled for the environment) and the sandbox modes it
/// claims.
#[must_use]
pub fn availability(mcp_servers_enabled: bool) -> ExecutorAvailability {
    let mut report = ExecutorAvailability::available(version(), sandbox_modes());
    report.tool_sets = Some(
        ToolSet::offered_now(mcp_servers_enabled)
            .iter()
            .map(|set| set.name().to_owned())
            .collect(),
    );
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_executor_contract::descriptor::ExecutorProtocol;

    #[test]
    fn the_declaration_holds_together_and_names_three_tool_sets() {
        let declared = descriptor();
        assert_eq!(declared.check(), Ok(()));
        let names: Vec<&str> = declared
            .tool_sets
            .iter()
            .map(|set| set.name.as_str())
            .collect();
        assert_eq!(names, ["claude-code", "plain", "custom"]);
        assert_eq!(declared.protocol, ExecutorProtocol::Mcp);
    }

    #[test]
    fn the_declaration_reads_and_writes_as_the_contracts_own_example_does() {
        let written = serde_json::to_value(descriptor()).expect("a descriptor is JSON");
        assert_eq!(written["kind"], "orch-exec");
        assert_eq!(written["sandbox_modes"], serde_json::json!(["open"]));
        assert_eq!(
            written["attribution"],
            serde_json::json!({ "thread_id": "ORCH_THREAD_ID", "tool_call_id": "ORCH_ITEM_ID" })
        );
    }

    #[test]
    fn a_tool_set_made_of_the_environments_servers_is_offered_only_while_one_is_enabled() {
        let without = availability(false);
        assert_eq!(
            without.tool_sets.as_deref(),
            Some(["claude-code".to_owned(), "plain".to_owned()].as_slice())
        );
        let with = availability(true);
        assert_eq!(with.tool_sets.map(|sets| sets.len()), Some(3));
        assert!(with.available && with.version.as_deref() == Some(version()));
    }
}
