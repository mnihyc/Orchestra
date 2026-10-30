//! Executor availability, the build it has to be, and the variables Codex's exec server gives each command for attribution.
//!
//! The Codex executor is the vendor's own program, run by the exec node: `codex exec-server`, of the build the
//! environment holds. The node does not speak the vendor's protocol and chooses nothing about it; it starts
//! the program, hands back the loopback address, and reports which build it is.
//!
//! The build is the one in the environment's tool cache, in the directory of the tool `codex`, or the local
//! program selected by the machine's policy when there is no cache. The cache directory holds either the program itself (`codex`), or the vendor's package layout, which names its program in
//! `codex-package.json` (`entrypoint`, `bin/codex` in the builds seen) and brings what the sandbox needs with
//! it. Which build an adapter needs is decided where the arrangement is chosen, from the version reported here.
//!
//! The commands a harness runs through the server carry `CODEX_THREAD_ID`, which the harness puts there and
//! which is Codex's own id of the thread, the one an adapter keeps as the session's native reference. The
//! vendor documents `CODEX_TOOL_CALL_ID` beside it, for the tool call; the build checked here does not give
//! it. A variable that is absent reads as nothing, so the declaration names both.

use std::path::{Component, Path, PathBuf};

use orch_agent_contract::descriptor::SandboxMode;
use orch_executor_contract::context::EnvironmentContext;
use orch_executor_contract::descriptor::{Attribution, ExecutorAvailability, ExecutorDescriptor};

/// The kind an environment offers this executor under.
pub const KIND: &str = "codex-exec-server";

/// The name of the tool in the environment's tool cache whose build is run.
pub const TOOL: &str = "codex";

/// The variable that holds Codex's id of the thread in every command the server runs.
pub const THREAD_VARIABLE: &str = "CODEX_THREAD_ID";

/// The variable the vendor documents for the id of the tool call. The build checked here does not set it.
pub const TOOL_CALL_VARIABLE: &str = "CODEX_TOOL_CALL_ID";

/// The file in which the vendor's package layout names its program.
const PACKAGE_MANIFEST: &str = "codex-package.json";

/// The places of the program in a directory that holds it without a manifest, in the order tried: the
/// program itself, including Windows executable names, and below `bin`, as the package layout has it.
const PROGRAM_PLACES: [&str; 6] = [
    "codex",
    "codex.exe",
    "codex.cmd",
    "bin/codex",
    "bin/codex.exe",
    "bin/codex.cmd",
];

/// The variables through which a command carries its ids.
#[must_use]
pub fn attribution() -> Attribution {
    Attribution {
        thread_id: THREAD_VARIABLE.to_owned(),
        tool_call_id: TOOL_CALL_VARIABLE.to_owned(),
    }
}

/// What the executor declares about itself: the vendor's program, speaking the vendor's protocol, with no
/// tool sets because the tools are the harness's own.
#[must_use]
pub fn descriptor() -> ExecutorDescriptor {
    ExecutorDescriptor::of_manifest(orch_agent_programs::shipped_executor(KIND))
}

/// What the executor reports of itself where it can be started: the build, and the sandbox modes that work.
#[must_use]
pub fn availability(
    version: &semver::Version,
    sandbox_modes: Vec<SandboxMode>,
) -> ExecutorAvailability {
    ExecutorAvailability::available(&version.to_string(), sandbox_modes)
}

/// The build the environment holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    /// The directory of the tool.
    pub directory: PathBuf,
    /// The program to run.
    pub program: PathBuf,
    /// The version the environment lists for the tool.
    pub listed_version: String,
}

/// Why there is no build to run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Missing {
    /// The environment was made without the tool.
    #[error("the environment holds no build of {TOOL}: it is not among the tools it was made with")]
    NotListed,
    /// The environment has no tool cache.
    #[error("the environment has no tool cache to find a build of {TOOL} in")]
    NoToolsDirectory,
    /// The directory of the tool has no program in it.
    #[error("the build of {TOOL} in {0} has no program in it")]
    NoProgram(String),
    /// The selected local program no longer exists.
    #[error("the locally confirmed program {0} is not a file")]
    LocalProgramMissing(String),
}

/// The build of the tool `codex` that the environment holds, and its program.
pub fn locate(env: &EnvironmentContext) -> Result<Build, Missing> {
    if env.tools_dir.is_none() {
        if let Some(path) = env.local_programs.get(TOOL) {
            let program = PathBuf::from(path);
            if !program.is_file() {
                return Err(Missing::LocalProgramMissing(path.clone()));
            }
            return Ok(Build {
                directory: program.parent().unwrap_or(Path::new(".")).to_path_buf(),
                program,
                listed_version: "unknown".to_owned(),
            });
        }
        return Err(if env.tool(TOOL).is_some() {
            Missing::NoToolsDirectory
        } else {
            Missing::NotListed
        });
    }
    let listed = env.tool(TOOL).ok_or(Missing::NotListed)?;
    let directory = PathBuf::from(env.tool_dir(TOOL).ok_or(Missing::NoToolsDirectory)?);
    let program = program_in(&directory)
        .ok_or_else(|| Missing::NoProgram(directory.display().to_string()))?;
    Ok(Build {
        directory,
        program,
        listed_version: listed.version.clone(),
    })
}

/// The program of a build in `directory`: the one its manifest names, else one of the usual places.
fn program_in(directory: &Path) -> Option<PathBuf> {
    if let Some(named) = std::fs::read_to_string(directory.join(PACKAGE_MANIFEST))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|manifest| manifest.get("entrypoint")?.as_str().map(PathBuf::from))
    {
        // The manifest is data of the build; it names a place inside the build and nowhere else.
        let stays_inside = named
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
        let program = directory.join(&named);
        return (stays_inside && program.is_file()).then_some(program);
    }
    PROGRAM_PLACES
        .iter()
        .map(|place| directory.join(place))
        .find(|program| program.is_file())
}

/// The version a program reports for `--version`: the first word of its output that is a version, as
/// `codex-cli 0.162.1` has it.
#[must_use]
pub fn version_in(output: &str) -> Option<semver::Version> {
    output
        .split_whitespace()
        .find_map(|word| semver::Version::parse(word.trim_start_matches('v')).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_executor_contract::descriptor::ExecutorProtocol;

    fn environment(tools_dir: Option<&Path>, listed: bool) -> EnvironmentContext {
        serde_json::from_value(serde_json::json!({
            "environment_id": "0199abcd-1234-7000-8000-0000000000e1", "generation": 1, "data_dir": "/d",
            "workspace_root": "/w", "home": null,
            "tools_dir": tools_dir.map(|dir| dir.display().to_string()),
            "tools": if listed { serde_json::json!([{ "name": "codex", "version": "0.162.1" }]) } else { serde_json::json!([]) },
            "mounts": [], "user": "agent", "shell": { "name": "sh", "path": "/bin/sh" }, "os": "linux",
            "arch": "x86_64", "path_style": "posix", "limits": {}, "variables": {}, "mcp_servers_enabled": false
        }))
        .expect("an environment")
    }

    fn make(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("directory");
        std::fs::write(path, "#!/bin/sh\n").expect("file");
    }

    #[test]
    fn the_declaration_holds_together_and_has_no_tool_sets_because_the_tools_are_the_harnesss_own()
    {
        let declared = descriptor();
        assert_eq!(declared.check(), Ok(()));
        assert_eq!(declared.protocol, ExecutorProtocol::Harness);
        assert!(declared.tool_sets.is_empty());
        assert_eq!(
            declared.sandbox_modes,
            [
                SandboxMode::ReadOnly,
                SandboxMode::Workspace,
                SandboxMode::Open
            ]
        );
    }

    #[test]
    fn the_declaration_is_the_one_the_contract_gives_as_its_example() {
        let written = serde_json::to_value(descriptor()).expect("JSON");
        assert_eq!(written["kind"], "codex-exec-server");
        assert_eq!(written["protocol"], "harness");
        assert_eq!(
            written["sandbox_modes"],
            serde_json::json!(["read_only", "workspace", "open"])
        );
        assert_eq!(
            written["attribution"],
            serde_json::json!({ "thread_id": "CODEX_THREAD_ID", "tool_call_id": "CODEX_TOOL_CALL_ID" })
        );
    }

    #[test]
    fn a_command_is_attributed_by_the_thread_variable_and_a_build_without_the_tool_call_one_reads_as_nothing()
     {
        let process = std::collections::BTreeMap::from([
            (
                "CODEX_THREAD_ID".to_owned(),
                "01a12adc-c412-76a2-912d-e6331c0808e9".to_owned(),
            ),
            ("CODEX_SESSION_ID".to_owned(), "the same".to_owned()),
        ]);
        let ids = attribution().read(&process);
        assert_eq!(
            ids.thread_id.as_deref(),
            Some("01a12adc-c412-76a2-912d-e6331c0808e9")
        );
        assert_eq!(ids.tool_call_id, None);
    }

    #[test]
    fn the_build_is_found_where_the_tool_cache_has_it_in_each_of_the_ways_it_is_shipped() {
        let dir = tempfile::tempdir().expect("a directory");
        let tools = dir.path();
        let env = environment(Some(tools), true);
        assert_eq!(
            locate(&env),
            Err(Missing::NoProgram(
                tools.join("codex").display().to_string()
            ))
        );
        make(&tools.join("codex/codex"));
        assert_eq!(
            locate(&env).expect("a static build").program,
            tools.join("codex/codex")
        );
        std::fs::remove_file(tools.join("codex/codex")).expect("remove");
        make(&tools.join("codex/bin/codex"));
        assert_eq!(
            locate(&env).expect("below bin").program,
            tools.join("codex/bin/codex")
        );
        make(&tools.join("codex/libexec/main"));
        std::fs::write(
            tools.join("codex/codex-package.json"),
            r#"{ "layoutVersion": 1, "entrypoint": "libexec/main" }"#,
        )
        .expect("manifest");
        let found = locate(&env).expect("by its manifest");
        assert_eq!(found.program, tools.join("codex/libexec/main"));
        assert_eq!(found.listed_version, "0.162.1");
    }

    #[test]
    fn a_manifest_cannot_name_a_program_outside_the_build() {
        let dir = tempfile::tempdir().expect("a directory");
        let tools = dir.path();
        make(&tools.join("elsewhere"));
        make(&tools.join("codex/bin/codex"));
        for entrypoint in ["../elsewhere", "/bin/sh", "bin/../../elsewhere"] {
            std::fs::write(
                tools.join("codex/codex-package.json"),
                format!(r#"{{ "entrypoint": "{entrypoint}" }}"#),
            )
            .expect("manifest");
            assert!(
                matches!(
                    locate(&environment(Some(tools), true)),
                    Err(Missing::NoProgram(_))
                ),
                "{entrypoint}"
            );
        }
    }

    #[test]
    fn an_environment_without_the_tool_or_without_a_tool_cache_says_so() {
        let dir = tempfile::tempdir().expect("a directory");
        assert_eq!(
            locate(&environment(Some(dir.path()), false)),
            Err(Missing::NotListed)
        );
        assert_eq!(
            locate(&environment(None, true)),
            Err(Missing::NoToolsDirectory)
        );
    }

    #[test]
    fn a_connected_environment_can_use_its_confirmed_codex_program() {
        let dir = tempfile::tempdir().expect("a directory");
        let program = dir.path().join("codex");
        make(&program);
        let mut context = serde_json::to_value(environment(None, false)).expect("context");
        context["local_programs"] = serde_json::json!({"codex": program});
        let mut context: EnvironmentContext = serde_json::from_value(context).expect("context");

        assert_eq!(
            locate(&context).expect("confirmed program").program,
            program
        );
        context.local_programs.clear();
        assert_eq!(locate(&context), Err(Missing::NotListed));
    }

    #[test]
    fn a_managed_cache_stays_authoritative_when_a_local_program_is_also_listed() {
        let dir = tempfile::tempdir().expect("a directory");
        let local = dir.path().join("local");
        let cached = dir.path().join("codex/codex");
        make(&local);
        make(&cached);
        let mut context =
            serde_json::to_value(environment(Some(dir.path()), true)).expect("context");
        context["local_programs"] = serde_json::json!({"codex": local});
        let context: EnvironmentContext = serde_json::from_value(context).expect("context");
        assert_eq!(locate(&context).expect("cached program").program, cached);
    }

    #[test]
    fn a_cached_windows_executable_can_be_found_by_its_native_name() {
        let dir = tempfile::tempdir().expect("a directory");
        let exe = dir.path().join("codex/bin/codex.exe");
        make(&exe);
        assert_eq!(
            locate(&environment(Some(dir.path()), true))
                .expect("program")
                .program,
            exe
        );
    }

    #[test]
    fn the_version_is_read_from_what_the_program_prints() {
        assert_eq!(
            version_in("codex-cli 0.162.1\n"),
            semver::Version::parse("0.162.1").ok()
        );
        assert_eq!(
            version_in("codex v1.2.3-rc.1"),
            semver::Version::parse("1.2.3-rc.1").ok()
        );
        assert_eq!(version_in("no version here"), None);
    }
}
