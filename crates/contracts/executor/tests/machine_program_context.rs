//! Confirmed machine paths are executor inputs, separate from an environment's command variables.
#![allow(clippy::unwrap_used, reason = "a contract test fails by panicking")]

use orch_executor_contract::context::EnvironmentContext;
use serde_json::json;

fn context() -> serde_json::Value {
    json!({
        "environment_id":"0199c1a0-0000-7000-8000-00000000000a","generation":1,
        "data_dir":"/controlled/exec","workspace_root":"/controlled/workspace","home":null,
        "tools_dir":null,"tools":[],"mounts":[],"user":"agent",
        "shell":{"name":"sh","path":"/bin/sh"},"os":"linux","arch":"x86_64","path_style":"posix",
        "limits":{},"variables":{"ENVIRONMENT_SECRET":"synthetic"},"mcp_servers_enabled":false
    })
}

#[test]
fn confirmed_machine_paths_survive_the_executor_boundary_without_becoming_command_variables() {
    let mut value = context();
    value["local_programs"] = json!({"codex":"/controlled/bin/codex"});
    let read: EnvironmentContext = serde_json::from_value(value).unwrap();
    assert_eq!(
        read.variables.get("ENVIRONMENT_SECRET").map(String::as_str),
        Some("synthetic")
    );
    let written = serde_json::to_value(read).unwrap();
    assert_eq!(
        written["local_programs"],
        json!({"codex":"/controlled/bin/codex"})
    );
    assert!(written["variables"].get("codex").is_none());
}

#[test]
fn an_older_context_keeps_its_held_tool_cache_and_has_no_machine_path() {
    let mut value = context();
    value["tools_dir"] = json!("/controlled/tool-cache");
    value["tools"] = json!([{"name":"codex","version":"0.165.0"}]);
    let read: EnvironmentContext = serde_json::from_value(value).unwrap();
    assert_eq!(
        read.tool_dir("codex").as_deref(),
        Some("/controlled/tool-cache/codex")
    );
    let written = serde_json::to_value(read).unwrap();
    assert!(written.get("local_programs").is_none());
}
