//! Discovery of local harness programs, without treating discovery as permission to run them.

use std::collections::{BTreeMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use futures::future::join_all;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// A probe should finish quickly enough for an interactive machine refresh.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// A version or npm prefix is one short line; bound output from an unexpected program.
const OUTPUT_LIMIT: u64 = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    kind: String,
    path: PathBuf,
    version: String,
}

#[derive(Debug, Default)]
struct Snapshot {
    observed: bool,
    kinds: Vec<String>,
    configured: BTreeMap<String, String>,
    found: Vec<Found>,
}

/// The latest local observations, kept separate from the local authorization policy.
#[derive(Debug)]
pub(crate) struct Discovery {
    registry: Arc<crate::harnesses::AdapterRegistry>,
    snapshot: RwLock<Snapshot>,
    refresh: tokio::sync::Mutex<()>,
    changed: tokio::sync::watch::Sender<u64>,
}

impl Discovery {
    /// Creates a cache for the adapter registry on this connector.
    pub(crate) fn new(registry: Arc<crate::harnesses::AdapterRegistry>) -> Self {
        Self {
            registry,
            snapshot: RwLock::new(Snapshot::default()),
            refresh: tokio::sync::Mutex::new(()),
            changed: tokio::sync::watch::channel(0).0,
        }
    }

    /// Repeats the local observations under the machine's current selections.
    pub(crate) async fn refresh(&self, policy: &crate::policy::PolicyStore) {
        let _refresh = self.refresh.lock().await;
        let Some(policy) = policy.policy() else {
            return;
        };
        let kinds: Vec<String> = self
            .registry
            .kinds_with_a_program()
            .iter()
            .map(|kind| (*kind).to_owned())
            .collect();
        let configured: BTreeMap<String, String> = kinds
            .iter()
            .filter_map(|kind| {
                policy
                    .harness(kind)
                    .map(|harness| (kind.clone(), harness.path.clone()))
            })
            .collect();
        let path = std::env::var_os("PATH");
        let directories = search_directories(path.as_deref(), configured.len() < kinds.len()).await;
        let found = join_all(kinds.iter().map(|kind| {
            let names = self.registry.programs_of(kind);
            discover_in(
                kind,
                configured.get(kind).map(String::as_str),
                names,
                &directories,
                path.as_deref(),
                cfg!(windows),
                PROBE_TIMEOUT,
            )
        }))
        .await
        .into_iter()
        .flatten()
        .collect();
        *self
            .snapshot
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Snapshot {
            observed: true,
            kinds,
            configured,
            found,
        };
        self.changed.send_modify(|generation| *generation += 1);
    }

    /// Notifies environment hosts when observations have been refreshed.
    pub(crate) fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Checks whether this exact path is an unconfirmed result of the latest search.
    pub(crate) fn found(&self, kind: &str, path: &str) -> bool {
        let snapshot = self.snapshot.read().unwrap_or_else(PoisonError::into_inner);
        snapshot.observed
            && !snapshot.configured.contains_key(kind)
            && snapshot.found.iter().any(|found| {
                found.kind == kind && found.path == Path::new(path) && found.path.is_file()
            })
    }

    /// Adds observations without turning an unconfirmed result into an authorized harness.
    pub(crate) fn apply(&self, facts: &mut orch_link_contract::session::MachineFacts) {
        let Some(policy) = facts.policy.as_ref() else {
            return;
        };
        let snapshot = self.snapshot.read().unwrap_or_else(PoisonError::into_inner);
        if !snapshot.observed {
            return;
        }
        facts.searched_harness_kinds = Some(snapshot.kinds.clone());
        facts
            .harnesses
            .retain(|harness| !snapshot.kinds.contains(&harness.kind));
        let mut discovered = Vec::new();
        for kind in &snapshot.kinds {
            let configured = policy.harness(kind).map(|harness| harness.path.as_str());
            let observed = snapshot.configured.get(kind).map(String::as_str);
            if let Some(path) = configured {
                let selected = if observed == configured {
                    snapshot.found.iter().find(|found| found.kind == *kind)
                } else {
                    None
                };
                let actual = selected
                    .map(|found| found.path.clone())
                    .or_else(|| resolve_named(path, std::env::var_os("PATH").as_deref()));
                if let Some(actual) = actual {
                    facts
                        .harnesses
                        .push(orch_link_contract::session::MachineFactsHarness {
                            kind: kind.clone(),
                            path: actual.to_string_lossy().into_owned(),
                            version: selected
                                .map_or("unknown", |found| found.version.as_str())
                                .into(),
                            daemon: false,
                        });
                }
            } else if observed.is_none() {
                discovered.extend(
                    snapshot
                        .found
                        .iter()
                        .filter(|found| found.kind == *kind)
                        .map(|found| orch_link_contract::session::MachineFactsHarness {
                            kind: kind.clone(),
                            path: found.path.to_string_lossy().into_owned(),
                            version: found.version.clone(),
                            daemon: false,
                        }),
                );
            }
        }
        facts.discovered_harnesses = Some(discovered);
    }
}

fn command_environment(command: &mut Command, path: Option<&OsStr>, npm_config: bool) {
    command.env_clear();
    if let Some(path) = path {
        command.env("PATH", path);
    }
    // Windows needs these to launch command files and find its system libraries.
    for name in ["SystemRoot", "WINDIR", "COMSPEC", "PATHEXT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    if npm_config {
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "npm_config_prefix",
            "NPM_CONFIG_PREFIX",
            "NPM_CONFIG_USERCONFIG",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
    }
}

async fn output(
    program: &Path,
    args: &[&str],
    path: Option<&OsStr>,
    timeout: Duration,
    npm_config: bool,
) -> Option<String> {
    let mut command = Command::new(program);
    command_environment(&mut command, path, npm_config);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let result = tokio::time::timeout(timeout, async {
        let mut bytes = Vec::new();
        stdout
            .take(OUTPUT_LIMIT + 1)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        let status = child.wait().await.ok()?;
        if status.success() && bytes.len() <= OUTPUT_LIMIT as usize {
            String::from_utf8(bytes).ok()
        } else {
            None
        }
    })
    .await;
    match result {
        Ok(value) => value,
        Err(_) => {
            let _ = child.start_kill();
            None
        }
    }
}

fn version(text: &str) -> Option<String> {
    text.split_whitespace().find_map(|word| {
        semver::Version::parse(word.trim_start_matches('v'))
            .ok()
            .map(|v| v.to_string())
    })
}

/// Resolve a locally configured bare program through the connector's PATH before passing it on.
pub(crate) fn resolve_named(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    resolve_named_for(name, path, cfg!(windows))
}

fn resolve_named_for(name: &str, path: Option<&OsStr>, windows: bool) -> Option<PathBuf> {
    let program = Path::new(name);
    if program.components().count() == 1 && !program.is_absolute() {
        let names = if windows && program.extension().is_none() {
            vec![format!("{name}.exe"), format!("{name}.cmd")]
        } else {
            vec![name.to_owned()]
        };
        return std::env::split_paths(path?)
            .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
            .find(|candidate| candidate.is_file())
            .and_then(absolute);
    }
    program
        .is_file()
        .then(|| program.to_path_buf())
        .and_then(absolute)
}

fn absolute(path: PathBuf) -> Option<PathBuf> {
    if path.is_absolute() {
        Some(path)
    } else {
        Some(std::env::current_dir().ok()?.join(path))
    }
}

fn candidate_files(directories: &[PathBuf], names: &[&str], windows: bool) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let names: Vec<String> = names
        .iter()
        .filter(|name| {
            !name.is_empty()
                && Path::new(name)
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
        })
        .flat_map(|name| {
            if windows && !name.ends_with(".exe") && !name.ends_with(".cmd") {
                vec![format!("{name}.exe"), format!("{name}.cmd")]
            } else {
                vec![(*name).to_owned()]
            }
        })
        .collect();
    directories
        .iter()
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .filter(|path| path.is_file())
        .filter_map(absolute)
        .filter(|path| seen.insert(std::fs::canonicalize(path).unwrap_or_else(|_| path.clone())))
        .collect()
}

async fn discover_in(
    kind: &str,
    configured: Option<&str>,
    names: Vec<&str>,
    directories: &[PathBuf],
    path: Option<&OsStr>,
    windows: bool,
    timeout: Duration,
) -> Vec<Found> {
    let programs = match configured {
        Some(program) => resolve_named(program, path).into_iter().collect(),
        None => candidate_files(directories, &names, windows),
    };
    join_all(programs.into_iter().map(|program| async move {
        let version = output(&program, &["--version"], path, timeout, false)
            .await
            .as_deref()
            .and_then(version)
            .unwrap_or_else(|| "unknown".to_owned());
        Found {
            kind: kind.to_owned(),
            path: program,
            version,
        }
    }))
    .await
}

async fn search_directories(path: Option<&OsStr>, search: bool) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> =
        path.map_or_else(Vec::new, |path| std::env::split_paths(path).collect());
    if !search {
        return dirs;
    }
    let windows = cfg!(windows);
    if let Some(home) = std::env::var_os(if windows { "USERPROFILE" } else { "HOME" }) {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".npm-global/bin"));
    }
    if windows {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
    } else {
        dirs.extend([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);
    }
    let prefix: Option<OsString> =
        std::env::var_os("npm_config_prefix").or_else(|| std::env::var_os("NPM_CONFIG_PREFIX"));
    if let Some(prefix) = prefix {
        let prefix = PathBuf::from(prefix);
        dirs.push(if windows { prefix } else { prefix.join("bin") });
    }
    // npm's global prefix may have been chosen in configuration instead of an environment variable.
    let npm = if windows { "npm.cmd" } else { "npm" };
    let npm_path = path.and_then(|path| {
        std::env::split_paths(path)
            .map(|dir| dir.join(npm))
            .find(|file| file.is_file())
    });
    if let Some(npm) = npm_path
        && let Some(prefix) = output(&npm, &["prefix", "-g"], path, PROBE_TIMEOUT, true).await
        && !prefix.trim().is_empty()
    {
        let prefix = PathBuf::from(prefix.trim());
        dirs.push(if windows { prefix } else { prefix.join("bin") });
    }
    dirs
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn program(directory: &Path, name: &str, text: &str) -> PathBuf {
        std::fs::create_dir_all(directory).expect("directory");
        let path = directory.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{text}\n")).expect("program");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("executable");
        path
    }

    #[tokio::test]
    async fn discovery_reports_each_program_and_the_version_it_answers() {
        let dir = tempfile::tempdir().expect("directory");
        let first = program(
            &dir.path().join("one"),
            "codex",
            "printf 'codex-cli 0.164.0\\n'",
        );
        let second = program(
            &dir.path().join("two"),
            "codex",
            "printf 'codex-cli 0.165.0\\n'",
        );
        let dirs = vec![dir.path().join("one"), dir.path().join("two")];
        let found = discover_in(
            "codex",
            None,
            vec!["codex"],
            &dirs,
            None,
            false,
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(
            found,
            vec![
                Found {
                    kind: "codex".into(),
                    path: first,
                    version: "0.164.0".into()
                },
                Found {
                    kind: "codex".into(),
                    path: second,
                    version: "0.165.0".into()
                }
            ]
        );
    }

    #[tokio::test]
    async fn the_adapters_program_names_discover_claude_independently_of_codex() {
        let dir = tempfile::tempdir().expect("directory");
        let claude = program(dir.path(), "claude", "printf '2.1.309 (Claude Code)\\n'");
        let found = discover_in(
            "claude-code",
            None,
            vec!["claude", "claude-code"],
            &[dir.path().into()],
            None,
            false,
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(
            found,
            vec![Found {
                kind: "claude-code".into(),
                path: claude,
                version: "2.1.309".into()
            }]
        );
    }

    #[tokio::test]
    async fn a_program_that_does_not_answer_is_found_with_an_unknown_version() {
        let dir = tempfile::tempdir().expect("directory");
        let path = program(dir.path(), "codex", "exec /bin/sleep 5");
        let found = discover_in(
            "codex",
            None,
            vec!["codex"],
            &[dir.path().into()],
            None,
            false,
            Duration::from_millis(100),
        )
        .await;
        assert_eq!(
            found,
            vec![Found {
                kind: "codex".into(),
                path,
                version: "unknown".into()
            }]
        );
    }

    #[tokio::test]
    async fn an_empty_search_reports_no_program() {
        let dir = tempfile::tempdir().expect("directory");
        assert!(
            discover_in(
                "codex",
                None,
                vec!["codex"],
                &[dir.path().into()],
                None,
                false,
                Duration::from_secs(1)
            )
            .await
            .is_empty()
        );
    }

    #[tokio::test]
    async fn a_configured_program_takes_precedence_over_other_discoveries() {
        let dir = tempfile::tempdir().expect("directory");
        let configured = program(
            &dir.path().join("selected"),
            "codex",
            "printf 'codex-cli 0.164.0\\n'",
        );
        program(
            &dir.path().join("search"),
            "codex",
            "printf 'codex-cli 0.165.0\\n'",
        );
        let found = discover_in(
            "codex",
            configured.to_str(),
            vec!["codex"],
            &[dir.path().join("search")],
            None,
            false,
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(
            found,
            vec![Found {
                kind: "codex".into(),
                path: configured,
                version: "0.164.0".into()
            }]
        );
    }

    #[test]
    fn windows_candidates_include_both_executable_and_command_files() {
        let dir = tempfile::tempdir().expect("directory");
        let exe = program(dir.path(), "claude.exe", "exit 0");
        let cmd = program(dir.path(), "claude.cmd", "exit 0");
        assert_eq!(
            candidate_files(&[dir.path().into()], &["claude"], true),
            [exe.clone(), cmd]
        );
        assert_eq!(
            resolve_named_for("claude", Some(dir.path().as_os_str()), true),
            Some(exe)
        );
    }

    #[test]
    fn found_programs_are_not_reported_as_confirmed_and_removal_withdraws_authorization() {
        let dir = tempfile::tempdir().unwrap();
        let codex = program(dir.path(), "codex", "exit 0");
        let claude = program(dir.path(), "claude", "exit 0");
        let discovery = Discovery::new(Arc::new(crate::harnesses::AdapterRegistry::new()));
        let selected = codex.display().to_string();
        *discovery.snapshot.write().unwrap() = Snapshot {
            observed: true,
            kinds: vec!["codex".into(), "claude-code".into()],
            configured: BTreeMap::from([("codex".into(), selected.clone())]),
            found: vec![
                Found {
                    kind: "codex".into(),
                    path: codex,
                    version: "0.164.0".into(),
                },
                Found {
                    kind: "claude-code".into(),
                    path: claude,
                    version: "2.1.309".into(),
                },
            ],
        };
        assert!(!discovery.found("codex", &selected));
        assert!(discovery.found(
            "claude-code",
            &dir.path().join("claude").display().to_string()
        ));
        assert!(!discovery.found("claude-code", "/not-discovered"));
        let mut policy = orch_link_contract::policy::LocalPolicy::default();
        policy
            .agent_harnesses
            .push(orch_link_contract::policy::PolicyHarness {
                kind: "codex".into(),
                path: selected,
            });
        let machine = |policy| {
            crate::facts::machine_facts(
                &crate::facts::Host {
                    hostname: "test".into(),
                    os_version: "test".into(),
                    cpus: 1,
                    memory_bytes: 1,
                },
                crate::facts::FactsDraft::default(),
                Some(policy),
                orch_link_contract::session::MachineFactsRates {
                    up_bps: 0,
                    down_bps: 0,
                    round_trip_ms: 0,
                },
                0,
            )
        };
        let mut facts = machine(policy.clone());
        discovery.apply(&mut facts);
        assert_eq!(
            facts
                .harnesses
                .iter()
                .map(|row| row.kind.as_str())
                .collect::<Vec<_>>(),
            ["codex"]
        );
        assert_eq!(
            facts
                .discovered_harnesses
                .as_ref()
                .unwrap()
                .iter()
                .map(|row| row.kind.as_str())
                .collect::<Vec<_>>(),
            ["claude-code"]
        );
        policy.agent_harnesses.clear();
        let mut after_removal = machine(policy);
        discovery.apply(&mut after_removal);
        assert!(after_removal.harnesses.is_empty());
        assert_eq!(after_removal.discovered_harnesses.unwrap().len(), 1);
    }
}
