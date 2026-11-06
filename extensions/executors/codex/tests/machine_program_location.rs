//! A confirmed path supplies tools only when the environment has no managed tool cache.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use orch_executor_codex::descriptor;
use orch_executor_contract::context::EnvironmentContext;
use serde_json::json;
use std::path::Path;

fn context(local: &Path, cache: Option<&Path>) -> EnvironmentContext {
    serde_json::from_value(json!({
        "environment_id":"0199c1a0-0000-7000-8000-00000000000a","generation":1,
        "data_dir":"/controlled/exec","workspace_root":"/controlled/workspace","home":null,
        "tools_dir":cache.map(|path|path.to_string_lossy()),
        "tools":if cache.is_some() {json!([{"name":"codex","version":"0.165.0"}])} else {json!([])},
        "local_programs":{"codex":local.to_string_lossy()},"mounts":[],"user":"agent",
        "shell":{"name":"sh","path":"/bin/sh"},"os":"linux","arch":"x86_64","path_style":"posix",
        "limits":{},"variables":{"ENVIRONMENT_SECRET":"synthetic"},"mcp_servers_enabled":false
    }))
    .unwrap()
}

#[test]
fn a_confirmed_program_is_located_without_a_tool_cache_or_build_artifact() {
    let directory = tempfile::tempdir().unwrap();
    let program = directory.path().join("confirmed-codex");
    std::fs::write(&program, "controlled program").unwrap();
    let found = descriptor::locate(&context(&program, None)).unwrap();
    assert_eq!(found.program, program);
    assert_eq!(
        found.listed_version, "unknown",
        "the actual version is probed rather than invented from an artifact"
    );
}

#[test]
fn a_held_cache_build_precedes_the_confirmed_machine_program() {
    let directory = tempfile::tempdir().unwrap();
    let program = directory.path().join("confirmed-codex");
    std::fs::write(&program, "controlled program").unwrap();
    let cache = directory.path().join("cache");
    let build = cache.join("codex");
    std::fs::create_dir_all(&build).unwrap();
    let cached = build.join("codex");
    std::fs::write(&cached, "controlled cached program").unwrap();
    let found = descriptor::locate(&context(&program, Some(&cache))).unwrap();
    assert_eq!(found.program, cached);
    assert_eq!(found.listed_version, "0.165.0");
}

#[test]
fn an_incomplete_held_cache_does_not_silently_fall_back_to_the_machine_program() {
    let directory = tempfile::tempdir().unwrap();
    let program = directory.path().join("confirmed-codex");
    std::fs::write(&program, "controlled program").unwrap();
    let cache = directory.path().join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    assert!(descriptor::locate(&context(&program, Some(&cache))).is_err());
}
