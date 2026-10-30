//! Harness and executor builds fetched on the server's word where the owner allows it, verified by hash.
//!
//! What is built so far is the finding of a harness's program where a runtime the system made keeps it. The
//! node gives such a runtime a read-only directory of tools (`ORCH_TOOLS_DIR`) with a directory for each
//! tool it was asked for, named as the harness kind is, holding the build as it was released: a program that
//! brings what it needs, either as the one file of a self-contained build, or among the files of a directory
//! that is the build. [`find_program`] looks for the program there, by the names the harness's adapter
//! answers to, at the top of the tool's directory and in its `bin`, and [`installed`] lists the harnesses a
//! runtime holds, each with the version its directory says it is. A machine a person connected keeps its
//! harness wherever its owner installed it and lists it in the local policy; fetching a build there on the
//! server's word, and checking it against its hash, is not built yet.

use crate::harnesses::AdapterRegistry;
use std::path::{Path, PathBuf};

/// The directory of a tool's programs, inside the tool's own directory.
const BIN_DIR: &str = "bin";

/// The file the node leaves in the directory of a tool, saying which version of it this is. A unit sees the
/// directory of a tool by the tool's name alone, so this is the only place in it that the version is.
pub const VERSION_MARKER: &str = ".version";

/// What is said of a build whose directory does not say which version it is.
pub const UNKNOWN_VERSION: &str = "unknown";

/// Whether a name can be one component of a path and nothing else.
fn is_plain(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

#[cfg(unix)]
fn can_run(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn can_run(path: &Path) -> bool {
    path.is_file()
}

/// Whether a name of a program is a path below the tool's directory: names of plain parts, none empty, `.` or `..`.
/// A build that PyPI carries keeps its program in a directory of the wheel.
fn is_below(name: &str) -> bool {
    !name.is_empty() && !name.contains(['\\', '\0']) && name.split('/').all(is_plain)
}

/// The program of the tool `tool` in the tools directory `tools_dir`: the first of `names` that is a file one
/// can run at the top of `<tools_dir>/<tool>/`, or in its `bin`. A name may be a path below that directory
/// (`runtime/program`). `None` when the tool is not there, or has no such program, or a name leaves the
/// directory (a harness kind comes from the system and never reaches outside it).
#[must_use]
pub fn find_program(tools_dir: &Path, tool: &str, names: &[&str]) -> Option<PathBuf> {
    if !is_plain(tool) {
        return None;
    }
    let directory = tools_dir.join(tool);
    names
        .iter()
        .filter(|name| is_below(name))
        .flat_map(|name| [directory.join(name), directory.join(BIN_DIR).join(name)])
        .find(|candidate| can_run(candidate))
}

/// A harness found in the tools directory of a runtime the system made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The kind of harness: the name of the tool's directory.
    pub kind: String,
    /// The program that runs it.
    pub program: PathBuf,
    /// The version its directory says it is, or [`UNKNOWN_VERSION`].
    pub version: String,
}

/// The version a tool's directory says it is: the first line of its [`VERSION_MARKER`] if that is a plain
/// word, and otherwise nothing is known of it.
fn version_of(directory: &Path) -> String {
    std::fs::read_to_string(directory.join(VERSION_MARKER))
        .ok()
        .and_then(|text| text.lines().next().map(|line| line.trim().to_owned()))
        .filter(|version| is_plain(version))
        .unwrap_or_else(|| UNKNOWN_VERSION.to_owned())
}

/// The harnesses the tools directory holds: for each kind of harness an adapter drives with a program of
/// its own, the program [`find_program`] finds in the directory of that kind, with the version the directory
/// says. A kind whose directory is not there, or holds no program that can be run, is not listed.
#[must_use]
pub fn installed(tools_dir: &Path, registry: &AdapterRegistry) -> Vec<Installed> {
    registry
        .kinds_with_a_program()
        .into_iter()
        .filter_map(|kind| {
            let program = find_program(tools_dir, kind, &registry.programs_of(kind))?;
            Some(Installed {
                kind: kind.to_owned(),
                program,
                version: version_of(&tools_dir.join(kind)),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    fn program(path: &Path, mode: u32) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_program_is_found_at_the_top_of_its_tools_directory_or_in_its_bin() {
        let tools = tempfile::tempdir().unwrap();
        program(&tools.path().join("codex/codex"), 0o755);
        program(&tools.path().join("claude-code/bin/claude"), 0o755);
        assert_eq!(
            find_program(tools.path(), "codex", &["codex"]),
            Some(tools.path().join("codex/codex"))
        );
        assert_eq!(
            find_program(tools.path(), "claude-code", &["claude-code", "claude"]),
            Some(tools.path().join("claude-code/bin/claude"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn what_is_not_a_program_one_can_run_is_not_found() {
        let tools = tempfile::tempdir().unwrap();
        program(&tools.path().join("codex/codex"), 0o644);
        assert_eq!(find_program(tools.path(), "codex", &["codex"]), None);
        assert_eq!(find_program(tools.path(), "absent", &["codex"]), None);
        std::fs::create_dir_all(tools.path().join("dir/codex")).unwrap();
        assert_eq!(find_program(tools.path(), "dir", &["codex"]), None);
    }

    /// The adapters a program composes: two that need the program of their harness, one that needs none. The
    /// scripted adapter stands in for each, since only their registrations matter here.
    fn registry() -> AdapterRegistry {
        use crate::harnesses::{AdapterRegistration, HomeRule};
        let mut registry = AdapterRegistry::new();
        for (kind, needs_binary) in [("claude-code", true), ("codex", true), ("fake", false)] {
            registry
                .register(AdapterRegistration {
                    kind,
                    adapter_version: 1,
                    builtin: true,
                    needs_binary,
                    home_rule: HomeRule::Harness,
                    factory: std::sync::Arc::new(|clock| {
                        Box::new(orch_adapter_fake::FakeAdapter::new(clock))
                    }),
                })
                .unwrap();
        }
        registry.program_names("claude-code", &["claude", "claude-code"]);
        registry
    }

    #[cfg(unix)]
    #[test]
    fn the_harnesses_a_tools_directory_holds_are_listed_each_with_the_version_its_directory_says() {
        let tools = tempfile::tempdir().unwrap();
        // A package has its program below `bin`; a single program is kept under the name of its tool.
        program(&tools.path().join("codex/bin/codex"), 0o755);
        std::fs::write(tools.path().join("codex/.version"), "0.162.1\n").unwrap();
        program(&tools.path().join("claude-code/claude-code"), 0o755);
        // An adapter that needs no program of its harness is not looked for.
        program(&tools.path().join("fake/fake"), 0o755);
        assert_eq!(
            installed(tools.path(), &registry()),
            [
                Installed {
                    kind: "claude-code".to_owned(),
                    program: tools.path().join("claude-code/claude-code"),
                    version: UNKNOWN_VERSION.to_owned(),
                },
                Installed {
                    kind: "codex".to_owned(),
                    program: tools.path().join("codex/bin/codex"),
                    version: "0.162.1".to_owned(),
                },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_version_that_is_no_plain_word_is_not_believed_and_a_directory_without_a_program_is_not_listed()
     {
        let tools = tempfile::tempdir().unwrap();
        program(&tools.path().join("codex/bin/codex"), 0o755);
        std::fs::write(tools.path().join("codex/.version"), "../../etc\n").unwrap();
        std::fs::create_dir_all(tools.path().join("claude-code")).unwrap();
        std::fs::write(tools.path().join("claude-code/.version"), "2.1.296\n").unwrap();
        let found = installed(tools.path(), &registry());
        assert_eq!(
            found
                .iter()
                .map(|each| each.kind.as_str())
                .collect::<Vec<_>>(),
            ["codex"],
            "the directory that holds no program one can run is not a harness"
        );
        assert_eq!(found[0].version, UNKNOWN_VERSION);
        assert_eq!(installed(&tools.path().join("absent"), &registry()), []);
    }

    #[cfg(unix)]
    #[test]
    fn a_program_may_be_a_path_below_the_tools_directory_and_never_one_that_leaves_it() {
        let tools = tempfile::tempdir().unwrap();
        program(
            &tools
                .path()
                .join("dsh/deepseek_harness_runtime/runtime/dsh-runtime"),
            0o755,
        );
        assert_eq!(
            find_program(
                tools.path(),
                "dsh",
                &["deepseek_harness_runtime/runtime/dsh-runtime"]
            ),
            Some(
                tools
                    .path()
                    .join("dsh/deepseek_harness_runtime/runtime/dsh-runtime")
            )
        );
        assert_eq!(
            find_program(tools.path(), "dsh", &["deepseek_harness_runtime/../../x"]),
            None
        );
        assert_eq!(
            find_program(
                tools.path(),
                "dsh",
                &["/deepseek_harness_runtime/runtime/dsh-runtime"]
            ),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_would_leave_the_tools_directory_finds_nothing() {
        let tools = tempfile::tempdir().unwrap();
        program(&tools.path().join("outside"), 0o755);
        let inner = tools.path().join("tools");
        program(&inner.join("codex/codex"), 0o755);
        assert_eq!(find_program(&inner, "../codex", &["codex"]), None);
        assert_eq!(find_program(&inner, "codex", &["../../outside"]), None);
        assert_eq!(find_program(&inner, "", &["codex"]), None);
    }
}
