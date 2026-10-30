//! Spawn or attach the harness, started quiet and pinned.
//!
//! What runs the harness is given to the adapter as a [`Launcher`]: it turns a [`LaunchRequest`] (a program,
//! its arguments, the variables it finds, a working directory) into a [`HarnessProcess`] with the three
//! standard streams and a handle to end it. The connector's launcher starts a plain program inside the
//! runtime ([`DirectLauncher`]); a test can start the harness in a container, or play it from a pipe.
//!
//! The configuration a harness is started with is the adapter's own and is the same for every start
//! ([`base_config`]): usage statistics, error reports, feedback and the check for a newer version are off,
//! its memories and the connectors of the vendor's apps are off, and a sign-in is kept in a file of the home.
//! In a home that is Orch's own it is written as the home's `config.toml` ([`write_home_config`]); on a
//! person's machine nothing of theirs is touched and the same settings are passed as `-c key=value`
//! overrides ([`overrides`]).

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use orch_types::Json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};

/// The program that is run when nothing says which: the harness's own name, found on the path.
pub const DEFAULT_PROGRAM: &str = "codex";

/// The variable that names the harness's home.
pub const HOME_VARIABLE: &str = "CODEX_HOME";

/// The variable through which the harness finds the token of the connector's listener for model calls.
pub const LLM_TOKEN_VARIABLE: &str = "ORCH_LLM_TOKEN";

/// The variable through which the harness finds the token of the connector's listener for MCP servers.
pub const MCP_TOKEN_VARIABLE: &str = "ORCH_MCP_TOKEN";

/// How many of the last lines the harness wrote on its error output are kept, to say why it ended: the
/// last few lines of a failing start say what is wrong, and more only costs memory.
pub const STDERR_TAIL_LINES: usize = 12;
/// A missing newline cannot grow a diagnostic forever; keep the latest 64 KiB of the reason.
pub const STDERR_TAIL_BYTES: usize = 64 * 1024;
/// Drain diagnostics in ordinary 8 KiB I/O chunks without buffering an entire native line.
const STDERR_READ_BYTES: usize = 8 * 1024;

/// The file of the home in which a sign-in is kept.
pub const AUTH_FILE: &str = "auth.json";

/// The file of the home that holds the settings.
pub const CONFIG_FILE: &str = "config.toml";

/// The vendor daemon's local state directory, used only to prevent its autonomous updater.
pub const DAEMON_DIRECTORY: &str = "app-server-daemon";
/// The vendor daemon's preference file within its state directory.
pub const DAEMON_SETTINGS_FILE: &str = "settings.json";

// These helpers can act outside an issued turn; Orch supplies their supervised equivalents.
const MANAGED_OFF_FEATURES: &[&str] = &[
    "daemon_auto_start",
    "goals",
    "hooks",
    "codex_hooks",
    "shell_snapshot",
    "skill_mcp_dependency_install",
    "memories",
    "apps",
    "plugins",
    "remote_plugin",
    "tool_suggest",
];

fn isolate_managed_config(config: &mut toml::Table) {
    for section in ["analytics", "feedback"] {
        config.insert(
            section.into(),
            toml::Value::Table(table_of(&[("enabled", toml::Value::Boolean(false))])),
        );
    }
    config.insert(
        "otel".into(),
        toml::Value::Table(table_of(&[
            ("exporter", toml::Value::String("none".into())),
            ("metrics_exporter", toml::Value::String("none".into())),
            ("trace_exporter", toml::Value::String("none".into())),
        ])),
    );
    config.insert(
        "check_for_update_on_startup".into(),
        toml::Value::Boolean(false),
    );
    config.insert("notify".into(), toml::Value::Array(Vec::new()));
    if let Some(providers) = config
        .get_mut("model_providers")
        .and_then(toml::Value::as_table_mut)
    {
        for (_, provider) in providers.iter_mut() {
            if let Some(auth) = provider
                .as_table_mut()
                .and_then(|provider| provider.get_mut("auth"))
                .and_then(toml::Value::as_table_mut)
            {
                // Token helpers may answer an issued request, never run their own idle refresh loop.
                auth.insert("refresh_interval_ms".into(), toml::Value::Integer(0));
            }
        }
    }
    let features = config
        .entry("features")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if !features.is_table() {
        *features = toml::Value::Table(toml::Table::new());
    }
    if let Some(features) = features.as_table_mut() {
        for feature in MANAGED_OFF_FEATURES {
            features.insert((*feature).into(), toml::Value::Boolean(false));
        }
    }
}

/// Immediate native unloading lets a later resume discard an earlier instruction-root override.
const NATIVE_UNLOAD_DELAY_SECONDS: i64 = 0;

/// What to run.
#[derive(Clone, PartialEq, Eq)]
pub struct LaunchRequest {
    /// The program: a path, or a name found on the path.
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// The variables it finds beyond those of the connector's own environment. Some are secrets: the
    /// launcher passes them as the process's environment and never as an argument.
    pub env: BTreeMap<String, String>,
    /// Where it runs; the connector's own directory when none is named.
    pub cwd: Option<PathBuf>,
}

impl fmt::Debug for LaunchRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaunchRequest")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("cwd", &self.cwd)
            .finish()
    }
}

/// How a process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessExit {
    /// The exit code, where it exited by itself.
    pub code: Option<i32>,
    /// What else is known of the end, such as the signal that ended it.
    pub detail: String,
}

impl fmt::Display for ProcessExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            Some(code) => write!(f, "exited with code {code}")?,
            None => f.write_str("ended without an exit code")?,
        }
        if !self.detail.is_empty() {
            write!(f, " ({})", self.detail)?;
        }
        Ok(())
    }
}

/// What ends or waits for a process the adapter started.
#[async_trait]
pub trait ProcessControl: Send {
    /// Waits until the process has ended. Cancel-safe.
    async fn wait(&mut self) -> ProcessExit;

    /// Ends the process now, if it still runs.
    fn kill(&mut self);
}

/// A running harness (or a one-shot command of it) with its streams.
pub struct HarnessProcess {
    /// What the harness reads.
    pub stdin: Box<dyn AsyncWrite + Send + Unpin>,
    /// What the harness writes.
    pub stdout: Box<dyn AsyncRead + Send + Unpin>,
    /// What it writes for the log, if the launcher gives it.
    pub stderr: Option<Box<dyn AsyncRead + Send + Unpin>>,
    /// The handle that waits for it and ends it.
    pub control: Box<dyn ProcessControl>,
}

impl fmt::Debug for HarnessProcess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HarnessProcess").finish_non_exhaustive()
    }
}

/// Why a process could not be started.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    /// The program is not there or could not be run.
    #[error("{program} could not be started: {source}")]
    Spawn {
        /// The program.
        program: String,
        /// Why.
        source: io::Error,
    },
}

/// Starts the programs the adapter needs.
#[async_trait]
pub trait Launcher: Send + Sync + 'static {
    /// Starts what `request` describes.
    async fn launch(&self, request: LaunchRequest) -> Result<HarnessProcess, LaunchError>;
}

/// Starts the program as a process of the machine, in the connector's own environment with the request's
/// variables added.
#[derive(Debug, Clone, Copy, Default)]
pub struct DirectLauncher;

struct ChildControl(tokio::process::Child);

#[async_trait]
impl ProcessControl for ChildControl {
    async fn wait(&mut self) -> ProcessExit {
        match self.0.wait().await {
            Ok(status) => ProcessExit {
                code: status.code(),
                detail: if status.code().is_none() {
                    status.to_string()
                } else {
                    String::new()
                },
            },
            Err(error) => ProcessExit {
                code: None,
                detail: format!("its end could not be awaited: {error}"),
            },
        }
    }

    fn kill(&mut self) {
        let _ = self.0.start_kill();
    }
}

#[async_trait]
impl Launcher for DirectLauncher {
    async fn launch(&self, request: LaunchRequest) -> Result<HarnessProcess, LaunchError> {
        let mut command = tokio::process::Command::new(&request.program);
        command
            .args(&request.args)
            .envs(&request.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &request.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn().map_err(|source| LaunchError::Spawn {
            program: request.program.clone(),
            source,
        })?;
        let broken = |what: &str| LaunchError::Spawn {
            program: request.program.clone(),
            source: io::Error::other(format!("the {what} of the process is not piped")),
        };
        let stdin = child.stdin.take().ok_or_else(|| broken("input"))?;
        let stdout = child.stdout.take().ok_or_else(|| broken("output"))?;
        let stderr = child.stderr.take();
        Ok(HarnessProcess {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            stderr: stderr.map(|stream| Box::new(stream) as Box<dyn AsyncRead + Send + Unpin>),
            control: Box::new(ChildControl(child)),
        })
    }
}

/// The last lines a harness wrote on its error output, shared between the task that reads them and whoever
/// reports why the harness ended.
#[derive(Debug, Clone, Default)]
pub struct StderrTail(Arc<Mutex<VecDeque<u8>>>);

impl StderrTail {
    /// Drains `stream` in bounded chunks, keeping the latest diagnostic even without a final newline.
    pub async fn follow(self, mut stream: Box<dyn AsyncRead + Send + Unpin>) {
        let mut buffer = [0; STDERR_READ_BYTES];
        loop {
            let read = match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            tracing::debug!(target: "orch_adapter_codex::harness", "{}", String::from_utf8_lossy(&buffer[..read]));
            let mut kept = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            kept.extend(&buffer[..read]);
            let excess = kept.len().saturating_sub(STDERR_TAIL_BYTES);
            kept.drain(..excess);
        }
    }

    /// The latest bounded lines, with terminal colour escapes removed.
    #[must_use]
    pub fn text(&self) -> String {
        let bytes: Vec<_> = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .copied()
            .collect();
        let text = String::from_utf8_lossy(&bytes);
        let mut lines: Vec<_> = text
            .lines()
            .rev()
            .take(STDERR_TAIL_LINES)
            .map(strip_escapes)
            .collect();
        lines.reverse();
        lines.join("\n")
    }
}

/// A line without the `ESC [ ... m` sequences a logger colours it with.
fn strip_escapes(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------------------
// The configuration a harness is started with

/// What the adapter is told to configure about the harness as a whole (`providers.config`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HomeOptions {
    /// The workspace of the vendor's account a sign-in must belong to.
    pub workspace_id: Option<String>,
    /// Whether the home is Orch's own, so that the sign-in is kept in a file of it.
    pub managed: bool,
}

/// Reads the options set on the provider: the member `workspace_id`.
#[must_use]
pub fn home_options(provider_config: &Json, managed: bool) -> HomeOptions {
    HomeOptions {
        workspace_id: provider_config
            .get("workspace_id")
            .and_then(Json::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned),
        managed,
    }
}

fn table_of(entries: &[(&str, toml::Value)]) -> toml::Table {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

/// The settings every harness process of the adapter starts with.
///
/// Quiet: no usage statistics, no error reports, no feedback, no check for a newer version, and no export
/// of traces or metrics. Isolated: the memories the harness would keep for the whole of a home are off
/// (one home serves every thread of a provider), and so are the vendor's apps, plugins and suggestions, which
/// reach servers that nobody gave the session and bring tools it was not given.
#[must_use]
pub fn base_config(options: &HomeOptions) -> toml::Table {
    let off = toml::Value::Boolean(false);
    let none = toml::Value::String("none".to_owned());
    let mut config = toml::Table::new();
    config.insert("check_for_update_on_startup".into(), off.clone());
    config.insert(
        "analytics".into(),
        toml::Value::Table(table_of(&[("enabled", off.clone())])),
    );
    config.insert(
        "feedback".into(),
        toml::Value::Table(table_of(&[("enabled", off.clone())])),
    );
    config.insert(
        "otel".into(),
        toml::Value::Table(table_of(&[
            ("exporter", none.clone()),
            ("metrics_exporter", none.clone()),
            ("trace_exporter", none),
        ])),
    );
    config.insert(
        "features".into(),
        toml::Value::Table(table_of(&[
            ("memories", off.clone()),
            // The harness's own goal mode starts turns that Orch did not check: Orch's goals are its own.
            ("goals", off.clone()),
            ("apps", off.clone()),
            ("plugins", off.clone()),
            ("remote_plugin", off.clone()),
            ("tool_suggest", off),
        ])),
    );
    if options.managed {
        isolate_managed_config(&mut config);
        // Orch stores every input; the CLI's prompt-recall file would duplicate it indefinitely.
        config.insert(
            "history".into(),
            toml::Value::Table(table_of(&[(
                "persistence",
                toml::Value::String("none".to_owned()),
            )])),
        );
        config.insert(
            "cli_auth_credentials_store".into(),
            toml::Value::String("file".to_owned()),
        );
    }
    if let Some(workspace) = &options.workspace_id {
        config.insert(
            "forced_chatgpt_workspace_id".into(),
            toml::Value::String(workspace.clone()),
        );
    }
    config
}

/// A JSON value as TOML. A `null` has no TOML form and is left out wherever it stands.
#[must_use]
pub fn toml_of(value: &Json) -> Option<toml::Value> {
    Some(match value {
        Json::Null => return None,
        Json::Bool(flag) => toml::Value::Boolean(*flag),
        Json::Number(number) => match number.as_i64() {
            Some(whole) => toml::Value::Integer(whole),
            None => toml::Value::Float(number.as_f64()?),
        },
        Json::String(text) => toml::Value::String(text.clone()),
        Json::Array(items) => toml::Value::Array(items.iter().filter_map(toml_of).collect()),
        Json::Object(members) => toml::Value::Table(
            members
                .iter()
                .filter_map(|(name, member)| Some((name.clone(), toml_of(member)?)))
                .collect(),
        ),
    })
}

/// Merges `overlay` over `base` as the harness merges its layers: a table meets a table member by member,
/// anything else replaces what it meets.
pub fn merge_tables(base: &mut toml::Table, overlay: &toml::Table) {
    for (name, value) in overlay {
        match (base.get_mut(name), value) {
            (Some(toml::Value::Table(into)), toml::Value::Table(from)) => merge_tables(into, from),
            _ => {
                base.insert(name.clone(), value.clone());
            }
        }
    }
}

/// The profile overlays the base; managed-home isolation remains authoritative over its switches.
#[must_use]
pub fn effective_config(options: &HomeOptions, profile_config: &Json) -> toml::Table {
    let mut config = base_config(options);
    if let Some(toml::Value::Table(overlay)) = toml_of(profile_config) {
        merge_tables(&mut config, &overlay);
    }
    if options.managed {
        isolate_managed_config(&mut config);
        // Session rollouts still persist; only terminal prompt recall is disabled in shared homes.
        config.insert(
            "history".into(),
            toml::Value::Table(table_of(&[(
                "persistence",
                toml::Value::String("none".to_owned()),
            )])),
        );
    }
    config.insert(
        "thread_unload_delay_secs".to_owned(),
        toml::Value::Integer(NATIVE_UNLOAD_DELAY_SECONDS),
    );
    config
}

fn flatten(prefix: &str, table: &toml::Table, out: &mut Vec<(String, toml::Value)>) {
    for (name, value) in table {
        let key = if name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            name.clone()
        } else {
            format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
        };
        let path = if prefix.is_empty() {
            key
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            toml::Value::Table(inner) if !inner.is_empty() => flatten(&path, inner, out),
            other => out.push((path, other.clone())),
        }
    }
}

/// The arguments `-c key=value` that pass `config` to a harness without writing a file.
#[must_use]
pub fn overrides(config: &toml::Table) -> Vec<String> {
    let mut leaves = Vec::new();
    flatten("", config, &mut leaves);
    leaves
        .into_iter()
        .flat_map(|(path, value)| ["-c".to_owned(), format!("{path}={value}")])
        .collect()
}

/// Writes `contents` to `path` through a temporary file beside it, so that a reader never sees half of it,
/// readable by the user of this process alone.
pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)
}

/// Makes the home directory the harness will be told to use, readable by the user of this process alone.
pub fn prepare_home(home: &Path) -> io::Result<()> {
    std::fs::create_dir_all(home)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Writes the configuration into the `config.toml` of a home that is Orch's own, unless it is already
/// what would be written.
pub fn write_home_config(home: &Path, config: &toml::Table) -> io::Result<()> {
    let text = toml::to_string(config).map_err(io::Error::other)?;
    let path = home.join(CONFIG_FILE);
    if std::fs::read_to_string(&path).is_ok_and(|current| current == text) {
        return Ok(());
    }
    write_private(&path, text.as_bytes())
}

/// Publishes the private daemon update preference once, without replacing an existing choice.
pub fn write_daemon_settings(home: &Path) -> io::Result<()> {
    use std::io::Write;
    let directory = home.join(DAEMON_DIRECTORY);
    match std::fs::create_dir(&directory) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(&directory)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the daemon settings directory is not an owned directory",
                ));
            }
        }
        Err(error) => return Err(error),
    }
    let path = directory.join(DAEMON_SETTINGS_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let temporary = directory.join(format!(".settings-{}.tmp", orch_types::new_id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    let result = (|| {
        file.write_all(b"{\"updater\":{\"autoUpdateEnabled\":false}}\n")?;
        file.sync_all()?;
        match std::fs::hard_link(&temporary, &path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error),
        }
    })();
    drop(file);
    let cleaned = std::fs::remove_file(temporary);
    result?;
    cleaned
}

/// The arguments of the harness's server: the standard streams as its transport, and, where no file is
/// written, the configuration as overrides.
#[must_use]
pub fn app_server_args(config_overrides: &[String]) -> Vec<String> {
    let mut args = vec![
        "app-server".to_owned(),
        "--listen".to_owned(),
        "stdio://".to_owned(),
    ];
    args.extend(config_overrides.iter().cloned());
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_managed_home_cannot_enable_an_unattended_daemon_or_helper_through_a_profile() {
        let options = HomeOptions {
            managed: true,
            ..HomeOptions::default()
        };
        let profile = json!({"features":{"daemon_auto_start":true,"goals":true,"hooks":true},"check_for_update_on_startup":true,"notify":["unobserved"],"analytics":{"enabled":true},"feedback":{"enabled":true},"otel":{"exporter":"console"},"model_providers":{"custom":{"auth":{"command":"credential-helper","refresh_interval_ms":300000}}}});
        let config = effective_config(&options, &profile);
        assert_eq!(config["analytics"]["enabled"].as_bool(), Some(false));
        assert_eq!(config["feedback"]["enabled"].as_bool(), Some(false));
        assert_eq!(config["otel"]["exporter"].as_str(), Some("none"));
        assert_eq!(
            config["model_providers"]["custom"]["auth"]["refresh_interval_ms"].as_integer(),
            Some(0)
        );
        assert_eq!(
            config["features"]["daemon_auto_start"].as_bool(),
            Some(false)
        );
        assert_eq!(config["features"]["goals"].as_bool(), Some(false));
        assert_eq!(config["features"]["hooks"].as_bool(), Some(false));
        assert_eq!(config["check_for_update_on_startup"].as_bool(), Some(false));
        assert!(config["notify"].as_array().unwrap().is_empty());
        let personal = effective_config(&HomeOptions::default(), &profile);
        assert_eq!(
            personal["features"]["daemon_auto_start"].as_bool(),
            Some(true)
        );
        assert_eq!(personal["notify"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn the_daemon_update_preference_is_private_and_a_later_choice_is_never_replaced() {
        let home = tempfile::tempdir().unwrap();
        write_daemon_settings(home.path()).unwrap();
        let directory = home.path().join(DAEMON_DIRECTORY);
        let path = directory.join(DAEMON_SETTINGS_FILE);
        let read =
            || serde_json::from_str::<Json>(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read()["updater"]["autoUpdateEnabled"], false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        std::fs::write(
            &path,
            "{\"updater\":{\"autoUpdateEnabled\":true},\"choice\":\"kept\"}",
        )
        .unwrap();
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        write_daemon_settings(home.path()).unwrap();
        assert_eq!(read()["choice"], "kept");
        assert_eq!(read()["updater"]["autoUpdateEnabled"], true);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
        assert_eq!(std::fs::read_dir(directory).unwrap().count(), 1);
    }

    #[test]
    fn managed_homes_skip_duplicate_prompt_recall_without_changing_personal_history_settings() {
        let managed = HomeOptions {
            managed: true,
            ..HomeOptions::default()
        };
        let profile = json!({"history":{"persistence":"save-all", "max_bytes":1024}});
        assert_eq!(
            effective_config(&managed, &profile)["history"]["persistence"].as_str(),
            Some("none")
        );
        assert!(
            base_config(&HomeOptions::default())
                .get("history")
                .is_none()
        );
        assert_eq!(
            effective_config(&HomeOptions::default(), &profile)["history"]["persistence"].as_str(),
            Some("save-all")
        );
    }

    #[test]
    fn every_harness_is_started_quiet_with_its_memories_and_the_vendors_apps_off() {
        let config = base_config(&HomeOptions::default());
        assert_eq!(
            config["check_for_update_on_startup"],
            toml::Value::Boolean(false)
        );
        assert_eq!(config["analytics"]["enabled"], toml::Value::Boolean(false));
        assert_eq!(config["feedback"]["enabled"], toml::Value::Boolean(false));
        for exporter in ["exporter", "metrics_exporter", "trace_exporter"] {
            assert_eq!(
                config["otel"][exporter].as_str(),
                Some("none"),
                "{exporter}"
            );
        }
        assert_eq!(config["features"]["memories"], toml::Value::Boolean(false));
        assert_eq!(config["features"]["apps"], toml::Value::Boolean(false));
        assert_eq!(config["features"]["goals"], toml::Value::Boolean(false));
    }

    #[test]
    fn a_home_of_orchs_own_keeps_its_sign_in_in_a_file_and_a_persons_does_not_say() {
        let managed = base_config(&HomeOptions {
            managed: true,
            ..HomeOptions::default()
        });
        assert_eq!(managed["cli_auth_credentials_store"].as_str(), Some("file"));
        assert!(
            base_config(&HomeOptions::default())
                .get("cli_auth_credentials_store")
                .is_none()
        );
    }

    #[test]
    fn the_workspace_a_sign_in_must_belong_to_is_the_harnesss_own_restriction() {
        let options = home_options(&json!({ "workspace_id": "ws_team_1" }), true);
        assert_eq!(
            base_config(&options)["forced_chatgpt_workspace_id"].as_str(),
            Some("ws_team_1")
        );
        assert!(
            base_config(&home_options(&json!({}), true))
                .get("forced_chatgpt_workspace_id")
                .is_none()
        );
    }

    #[test]
    fn what_a_profiles_owner_sets_is_merged_over_the_base_member_by_member() {
        let config = effective_config(
            &HomeOptions::default(),
            &json!({ "analytics": { "enabled": true }, "model_reasoning_effort": "high", "dropped": null }),
        );
        assert_eq!(config["analytics"]["enabled"], toml::Value::Boolean(true));
        assert_eq!(
            config["feedback"]["enabled"],
            toml::Value::Boolean(false),
            "what is not named stays"
        );
        assert_eq!(config["model_reasoning_effort"].as_str(), Some("high"));
        assert!(config.get("dropped").is_none());
    }

    #[test]
    fn a_closed_thread_is_unloaded_at_once_whatever_the_profile_says() {
        // A thread is resumed with new settings or skills only after it has unloaded; the harness's own default
        // keeps an idle thread a minute, which somebody sending a message would sit through.
        let config = effective_config(
            &HomeOptions::default(),
            &json!({ "thread_unload_delay_secs": 60 }),
        );
        assert_eq!(config["thread_unload_delay_secs"], toml::Value::Integer(0));
        let args = overrides(&config);
        assert!(args.iter().any(|arg| arg == "thread_unload_delay_secs=0"));
    }

    #[test]
    fn on_a_persons_machine_the_same_settings_are_passed_as_overrides_and_no_file_is_needed() {
        let args = overrides(&base_config(&HomeOptions::default()));
        let pairs: Vec<_> = args.chunks(2).collect();
        assert!(pairs.iter().all(|pair| pair[0] == "-c"));
        let settings: Vec<_> = pairs.iter().map(|pair| pair[1].as_str()).collect();
        assert!(settings.contains(&"analytics.enabled=false"));
        assert!(settings.contains(&"otel.exporter=\"none\""));
        assert!(settings.contains(&"features.memories=false"));
        assert_eq!(
            app_server_args(&args[..2])[..3],
            ["app-server", "--listen", "stdio://"]
        );
    }

    #[test]
    fn a_home_config_is_written_once_and_privately() {
        let home = tempfile::tempdir().unwrap();
        let config = base_config(&HomeOptions {
            managed: true,
            ..HomeOptions::default()
        });
        write_home_config(home.path(), &config).unwrap();
        let written = std::fs::read_to_string(home.path().join(CONFIG_FILE)).unwrap();
        assert_eq!(toml::from_str::<toml::Table>(&written).unwrap(), config);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(home.path().join(CONFIG_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let before = std::fs::metadata(home.path().join(CONFIG_FILE))
            .unwrap()
            .modified()
            .unwrap();
        write_home_config(home.path(), &config).unwrap();
        let after = std::fs::metadata(home.path().join(CONFIG_FILE))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after, "an unchanged config is not written again");
    }

    #[test]
    fn the_colour_a_logger_adds_is_taken_out_of_what_is_said_of_a_failure() {
        assert_eq!(
            strip_escapes("\u{1b}[2m2026\u{1b}[0m \u{1b}[31mERROR\u{1b}[0m it broke"),
            "2026 ERROR it broke"
        );
    }

    #[test]
    fn a_launch_request_shows_the_names_of_its_variables_and_never_their_values() {
        let request = LaunchRequest {
            program: "codex".into(),
            args: vec!["app-server".into()],
            env: BTreeMap::from([(LLM_TOKEN_VARIABLE.to_owned(), "very-secret".to_owned())]),
            cwd: None,
        };
        let shown = format!("{request:?}");
        assert!(shown.contains(LLM_TOKEN_VARIABLE));
        assert!(!shown.contains("very-secret"));
    }
    #[tokio::test]
    async fn a_million_unterminated_diagnostic_bytes_keep_the_final_reason_in_bounded_memory() {
        use tokio::io::AsyncWriteExt;
        let tail = StderrTail::default();
        let (input, mut output) = tokio::io::duplex(STDERR_READ_BYTES);
        let following = tokio::spawn(tail.clone().follow(Box::new(input)));
        let block = [b'x'; STDERR_READ_BYTES];
        for _ in 0..128 {
            output.write_all(&block).await.unwrap();
        }
        output.write_all(b" final-native-failure").await.unwrap();
        drop(output);
        following.await.unwrap();
        assert!(tail.0.lock().unwrap().len() <= STDERR_TAIL_BYTES);
        assert!(tail.text().ends_with("final-native-failure"));
    }
}
