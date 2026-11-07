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
    programs: BTreeMap<String, Vec<String>>,
    snapshot: RwLock<Snapshot>,
    refresh: tokio::sync::Mutex<()>,
    changes: tokio::sync::watch::Sender<()>,
    #[cfg(test)]
    test_search: Option<(Vec<PathBuf>, Option<OsString>)>,
}

impl Discovery {
    /// Creates a cache for the adapter registry on this connector.
    pub(crate) fn new(registry: Arc<crate::harnesses::AdapterRegistry>) -> Self {
        Self {
            programs: registry
                .kinds_with_a_program()
                .into_iter()
                .map(|kind| {
                    (
                        kind.to_owned(),
                        searched_names(kind, registry.programs_of(kind)),
                    )
                })
                .collect(),
            snapshot: RwLock::new(Snapshot::default()),
            refresh: tokio::sync::Mutex::new(()),
            changes: tokio::sync::watch::channel(()).0,
            #[cfg(test)]
            test_search: None,
        }
    }

    #[cfg(all(test, unix))]
    pub(crate) fn with_test_search(
        registry: Arc<crate::harnesses::AdapterRegistry>,
        directory: PathBuf,
    ) -> Self {
        let mut discovery = Self::new(registry);
        discovery.test_search = Some((vec![directory.clone()], Some(directory.into_os_string())));
        discovery
    }

    pub(crate) fn supports(&self, kind: &str) -> bool {
        self.programs.contains_key(kind)
    }

    /// A completed refresh may change the version at a selected path without changing policy.
    pub(crate) fn changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.changes.subscribe()
    }

    fn search_inputs(&self) -> Search {
        let search = Search::current();
        #[cfg(test)]
        if let Some((_, path)) = &self.test_search {
            return Search {
                path: path.clone(),
                ..search
            };
        }
        search
    }

    pub(crate) fn matches_selected_path(&self, selected: &str, reported: &str) -> bool {
        selected == reported
            || resolve_named(selected, self.search_inputs().path.as_deref())
                .is_some_and(|resolved| resolved == Path::new(reported))
    }

    /// Repeats the local observations under the machine's current selections.
    pub(crate) async fn refresh(&self, policy: &crate::policy::PolicyStore) {
        let _refresh = self.refresh.lock().await;
        let Some(policy) = policy.policy() else {
            return;
        };
        // Search only the adapters this physical connector started with.
        let kinds: Vec<String> = self.programs.keys().cloned().collect();
        let configured: BTreeMap<String, String> = kinds
            .iter()
            .filter_map(|kind| {
                policy
                    .harness(kind)
                    .map(|harness| (kind.clone(), harness.path.clone()))
            })
            .collect();
        let search = self.search_inputs();
        let path = search.path.as_deref();
        #[cfg(test)]
        let directories = if let Some((directories, _)) = &self.test_search {
            directories.clone()
        } else {
            search_directories(&search, configured.len() < kinds.len()).await
        };
        #[cfg(not(test))]
        let directories = search_directories(&search, configured.len() < kinds.len()).await;
        let found = join_all(kinds.iter().map(|kind| {
            let names = self.programs[kind].iter().map(String::as_str).collect();
            discover_in(
                kind,
                configured.get(kind).map(String::as_str),
                names,
                &directories,
                path,
                search.windows,
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
        self.changes.send_replace(());
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
                    snapshot
                        .found
                        .iter()
                        .find(|found| found.kind == *kind && found.path.is_file())
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
                        .filter(|found| found.kind == *kind && found.path.is_file())
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

fn searched_names(kind: &str, declared: Vec<&str>) -> Vec<String> {
    // Discovery uses the shipped program's canonical executable; configured launch aliases remain intact.
    if let Some(program) = orch_agent_programs::Programs::builtin().program(kind)
        && let Some(name) = program.launch.programs.first()
    {
        return vec![name.clone()];
    }
    declared.into_iter().map(str::to_owned).collect()
}

fn command_environment(command: &mut Command, path: Option<&OsStr>, extra: &[(&str, &OsStr)]) {
    command.env_clear();
    if let Some(path) = path {
        command.env("PATH", path);
    }
    // Windows command shims and system DLL loading need these OS bootstrap values.
    // They come from the connector, never from an environment's variables or secrets.
    #[cfg(windows)]
    for name in ["SystemRoot", "WINDIR", "COMSPEC", "PATHEXT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.envs(extra.iter().copied());
}

async fn output(
    program: &Path,
    args: &[&str],
    path: Option<&OsStr>,
    timeout: Duration,
    extra: &[(&str, &OsStr)],
) -> Option<String> {
    let mut command = Command::new(program);
    command_environment(&mut command, path, extra);
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
            let _ = child.kill().await;
            None
        }
    }
}

fn version(text: &str) -> Option<String> {
    text.split_whitespace().find_map(|word| {
        semver::Version::parse(word.strip_prefix('v').unwrap_or(word))
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
        let version = output(&program, &["--version"], path, timeout, &[])
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

/// Connector-owned search inputs; tests supply private directories without changing process globals.
struct Search {
    path: Option<OsString>,
    home: Option<OsString>,
    appdata: Option<OsString>,
    prefix: Option<OsString>,
    user_config: Option<OsString>,
    windows: bool,
}

impl Search {
    fn current() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            home: std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }),
            appdata: std::env::var_os("APPDATA"),
            prefix: std::env::var_os("npm_config_prefix")
                .or_else(|| std::env::var_os("NPM_CONFIG_PREFIX")),
            user_config: std::env::var_os("NPM_CONFIG_USERCONFIG"),
            windows: cfg!(windows),
        }
    }
}

async fn search_directories(search: &Search, discover: bool) -> Vec<PathBuf> {
    let path = search.path.as_deref();
    let mut dirs: Vec<PathBuf> =
        path.map_or_else(Vec::new, |path| std::env::split_paths(path).collect());
    if !discover {
        return dirs;
    }
    if let Some(home) = &search.home {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".npm-global/bin"));
    }
    if search.windows {
        if let Some(appdata) = &search.appdata {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
    } else {
        dirs.extend([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);
    }
    let prefix = if let Some(prefix) = &search.prefix {
        Some(PathBuf::from(prefix))
    } else {
        let npm = if search.windows { "npm.cmd" } else { "npm" };
        let npm = path.and_then(|path| {
            std::env::split_paths(path)
                .map(|dir| dir.join(npm))
                .find(|file| file.is_file())
        });
        let mut extra = Vec::new();
        if let Some(home) = search.home.as_deref() {
            extra.push((
                if search.windows {
                    "USERPROFILE"
                } else {
                    "HOME"
                },
                home,
            ));
        }
        if search.windows
            && let Some(appdata) = search.appdata.as_deref()
        {
            extra.push(("APPDATA", appdata));
        }
        if let Some(config) = search.user_config.as_deref() {
            extra.push(("NPM_CONFIG_USERCONFIG", config));
        }
        match npm {
            Some(npm) => output(
                &npm,
                &["config", "get", "prefix"],
                path,
                PROBE_TIMEOUT,
                &extra,
            )
            .await
            .filter(|prefix| !prefix.trim().is_empty())
            .map(|prefix| PathBuf::from(prefix.trim())),
            None => None,
        }
    };
    if let Some(prefix) = prefix {
        dirs.push(if search.windows {
            prefix
        } else {
            prefix.join("bin")
        });
    }
    dirs
}

#[cfg(test)]
mod parsing_tests {
    use super::{searched_names, version};

    #[test]
    fn discovery_names_do_not_expand_to_harness_launch_aliases() {
        assert_eq!(searched_names("codex", vec!["other-codex"]), ["codex"]);
        assert_eq!(
            searched_names("claude-code", vec!["claude", "claude-code"]),
            ["claude"]
        );
        assert_eq!(
            searched_names("custom", vec!["custom-agent"]),
            ["custom-agent"]
        );
    }

    #[test]
    fn the_first_semantic_word_loses_only_one_leading_v() {
        assert_eq!(version("codex 12 v1.2.3 9.8.7"), Some("1.2.3".into()));
        assert_eq!(version("release vv1.2.3"), None);
        assert_eq!(version("version unknown"), None);
    }
}

#[cfg(test)]
#[cfg(unix)]
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

    fn quoted(path: &Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    fn search_in(root: &Path) -> Search {
        Search {
            path: Some(root.join("bin").into_os_string()),
            home: Some(root.join("home").into_os_string()),
            appdata: Some(root.join("appdata").into_os_string()),
            prefix: None,
            user_config: None,
            windows: false,
        }
    }

    #[tokio::test]
    async fn search_locations_keep_path_home_platform_and_prefix_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut search = search_in(dir.path());
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        search.path = Some(std::env::join_paths([&first, &second]).unwrap());
        let prefix = dir.path().join("prefix");
        search.prefix = Some(prefix.clone().into_os_string());
        let home = dir.path().join("home");
        assert_eq!(
            search_directories(&search, true).await,
            [
                first.clone(),
                second.clone(),
                home.join(".local/bin"),
                home.join(".npm-global/bin"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
                prefix.join("bin")
            ]
        );
        search.windows = true;
        assert_eq!(
            search_directories(&search, true).await,
            [
                first.clone(),
                second.clone(),
                home.join(".local/bin"),
                home.join(".npm-global/bin"),
                dir.path().join("appdata/npm"),
                prefix
            ]
        );
        assert_eq!(search_directories(&search, false).await, [first, second]);
    }

    #[tokio::test]
    async fn an_environment_prefix_does_not_run_npm() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("npm-called");
        program(
            &dir.path().join("bin"),
            "npm",
            &format!("printf called > {}; printf '/unexpected'", quoted(&marker)),
        );
        let mut search = search_in(dir.path());
        let prefix = dir.path().join("chosen-prefix");
        search.prefix = Some(prefix.clone().into_os_string());
        let directories = search_directories(&search, true).await;
        assert_eq!(directories.last(), Some(&prefix.join("bin")));
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn npm_is_asked_config_get_prefix_only_when_no_environment_prefix_exists() {
        let dir = tempfile::tempdir().unwrap();
        let args = dir.path().join("npm-args");
        let prefix = dir.path().join("configured-prefix");
        program(
            &dir.path().join("bin"),
            "npm",
            &format!(
                "printf '%s\\n' \"$@\" > {}; printf '%s\\n' {}",
                quoted(&args),
                quoted(&prefix)
            ),
        );
        let search = search_in(dir.path());
        let directories = search_directories(&search, true).await;
        assert_eq!(
            std::fs::read_to_string(args).unwrap(),
            "config\nget\nprefix\n"
        );
        assert_eq!(directories.last(), Some(&prefix.join("bin")));
    }

    #[test]
    fn version_probes_clear_everything_except_the_connector_path() {
        let mut command = Command::new("controlled-version-probe");
        command
            .env("ENVIRONMENT_SECRET", "synthetic")
            .env("HOME", "/environment-home");
        command_environment(&mut command, Some(OsStr::new("/connector/bin")), &[]);
        let values = command
            .as_std()
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(OsStr::to_owned)))
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            [(
                OsString::from("PATH"),
                Some(OsString::from("/connector/bin"))
            )]
        );
    }

    #[test]
    fn canonical_aliases_are_reported_once_at_their_first_search_location() {
        let dir = tempfile::tempdir().unwrap();
        let real = program(&dir.path().join("real"), "codex", "exit 0");
        let aliases = dir.path().join("aliases");
        std::fs::create_dir_all(&aliases).unwrap();
        let alias = aliases.join("codex");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        assert_eq!(
            candidate_files(&[aliases, dir.path().join("real")], &["codex"], false),
            [alias]
        );
    }

    #[tokio::test]
    async fn missing_or_oversized_version_output_still_reports_the_existing_file_as_unknown() {
        let dir = tempfile::tempdir().unwrap();
        for text in [
            "printf 'no version was reported'",
            "i=0; while [ $i -lt 4097 ]; do printf x; i=$((i+1)); done",
        ] {
            let path = program(dir.path(), "codex", text);
            let found = discover_in(
                "codex",
                None,
                vec!["codex"],
                &[dir.path().into()],
                None,
                false,
                PROBE_TIMEOUT,
            )
            .await;
            assert_eq!(
                found,
                [Found {
                    kind: "codex".into(),
                    path,
                    version: "unknown".into()
                }]
            );
        }
        assert_eq!(
            version("codex v0.165.0 next 0.166.0"),
            Some("0.165.0".into())
        );
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
            PROBE_TIMEOUT,
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
            PROBE_TIMEOUT,
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
            PROBE_TIMEOUT,
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
                PROBE_TIMEOUT
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
            PROBE_TIMEOUT,
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

    #[tokio::test]
    async fn a_configured_path_keeps_its_launch_alias_without_expanding_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let selected = program(dir.path(), "claude-code", "printf '2.1.312\\n'");
        let found = discover_in(
            "claude-code",
            selected.to_str(),
            vec!["claude"],
            &[],
            None,
            false,
            PROBE_TIMEOUT,
        )
        .await;
        assert_eq!(
            found,
            [Found {
                kind: "claude-code".into(),
                path: selected,
                version: "2.1.312".into()
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
