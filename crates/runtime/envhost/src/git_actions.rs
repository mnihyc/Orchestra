//! Controlled Git actions and repository-scoped command credentials.
//!
//! Only explicit source/publication commands reach these actions. Git gets a cleared environment,
//! disabled project hooks, no signing/filter/monitor programs and no interactive prompt. A reading
//! credential is held in one child's environment, checked against the exact HTTPS
//! repository protocol, host and path, and is never written into a remote URL or shared Git config.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use orch_api_contract::source::{SourceDestination, SourceInfo, SourceKind, SourceState};
use orch_link_contract::source::GitPrepare;
use orch_types::Hash;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{HostError, Result};
use crate::file_states::FileStates;
use crate::files::Files;
use crate::git::GitTool;
use crate::paths::{Locate, RelPath};
use crate::platform::{self, Sig};

/// The protected Git reader's existing eight-mebibyte bound also bounds action replies.
const OUTPUT_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Error classification keeps the reader's existing 64-KiB prefix; raw text never leaves this module.
const ERROR_KEEP_BYTES: u64 = 64 * 1024;

/// A command-only credential, whose temporary helper contains no secret value.
pub struct Credentials {
    directory: tempfile::TempDir,
    helper: String,
    username: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsernamePassword {
    username: String,
    password: String,
}

fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn decoded_path(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut position = 0;
    while position < bytes.len() {
        if bytes[position] == b'%' {
            let pair = bytes.get(position + 1..position + 3).ok_or_else(|| {
                HostError::Unsupported("the credential repository path is invalid".to_owned())
            })?;
            let digits = std::str::from_utf8(pair).map_err(|_| {
                HostError::Unsupported("the credential repository path is invalid".to_owned())
            })?;
            decoded.push(u8::from_str_radix(digits, 16).map_err(|_| {
                HostError::Unsupported("the credential repository path is invalid".to_owned())
            })?);
            position += 3;
        } else {
            decoded.push(bytes[position]);
            position += 1;
        }
    }
    let path = String::from_utf8(decoded).map_err(|_| {
        HostError::Unsupported("the credential repository path is not UTF-8".to_owned())
    })?;
    if path.chars().any(char::is_control) {
        return Err(HostError::Unsupported(
            "the credential repository path has control characters".to_owned(),
        ));
    }
    Ok(path.trim_matches('/').to_owned())
}

impl Credentials {
    /// Holds username/password JSON for one exact HTTPS repository, outside every project directory.
    pub fn for_repository(data: &Path, repository: &str, value: &str) -> Result<Self> {
        if !cfg!(unix) {
            return Err(HostError::Unsupported(
                "scoped Git credentials require the connector's Unix credential helper".to_owned(),
            ));
        }
        let repository = reqwest::Url::parse(repository).map_err(|_| {
            HostError::Unsupported("the credential repository is invalid".to_owned())
        })?;
        if repository.scheme() != "https"
            || repository.host_str().is_none()
            || !repository.username().is_empty()
            || repository.password().is_some()
            || repository.query().is_some()
            || repository.fragment().is_some()
        {
            return Err(HostError::Unsupported("a source credential is scoped to an HTTPS repository without embedded authentication".to_owned()));
        }
        let secret: UsernamePassword = serde_json::from_str(value).map_err(|_| {
            HostError::Unsupported(
                "the source credential must have username and password".to_owned(),
            )
        })?;
        if secret.username.is_empty()
            || secret.username.chars().any(char::is_control)
            || secret.password.chars().any(char::is_control)
        {
            return Err(HostError::Unsupported(
                "the source credential has an empty username or control characters".to_owned(),
            ));
        }
        let path = decoded_path(repository.path())?;
        if path.is_empty() {
            return Err(HostError::Unsupported(
                "the source credential must name a repository path".to_owned(),
            ));
        }
        let host = match repository.port() {
            Some(port) => format!("{}:{port}", repository.host_str().unwrap_or_default()),
            None => repository.host_str().unwrap_or_default().to_owned(),
        };
        let directory = tempfile::Builder::new()
            .prefix("source-credential-")
            .tempdir_in(data)
            .map_err(|error| HostError::io("making a command-only Git credential", error))?;
        for (name, value) in [
            ("protocol", "https"),
            ("host", host.as_str()),
            ("path", path.as_str()),
        ] {
            let location = directory.path().join(name);
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&location)
                .map_err(|error| HostError::io("keeping a command-only Git credential", error))?;
            #[cfg(unix)]
            platform::set_file_mode(&file, 0o600).map_err(|error| {
                HostError::io("protecting a command-only Git credential", error)
            })?;
            std::io::Write::write_all(&mut file, format!("{value}\n").as_bytes())
                .map_err(|error| HostError::io("keeping a command-only Git credential", error))?;
        }
        let script = directory.path().join("helper");
        std::fs::write(&script, CREDENTIAL_HELPER)
            .map_err(|error| HostError::io("keeping the scoped Git credential helper", error))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).map_err(
                |error| HostError::io("protecting the scoped Git credential helper", error),
            )?;
        }
        let helper = format!("!exec {} \"$@\"", shell_word(&script.to_string_lossy()));
        Ok(Self {
            directory,
            helper,
            username: secret.username,
            password: secret.password,
        })
    }
}

// Git supplies protocol/host/path as credential-protocol fields, rather than a prompt whose
// wording might change. The helper refuses another authority or repository. Store/erase do
// nothing: this operation never installs the credential into a user's credential store.
const CREDENTIAL_HELPER: &str = r#"#!/bin/sh
[ "$1" = get ] || exit 0
protocol= host= path=
while IFS='=' read -r key value; do
    case "$key" in
        '') break ;;
        protocol) protocol=$value ;;
        host) host=$value ;;
        path) path=$value ;;
    esac
done
IFS= read -r expected_protocol < "$ORCH_SOURCE_CREDENTIAL_DIR/protocol" || exit 1
IFS= read -r expected_host < "$ORCH_SOURCE_CREDENTIAL_DIR/host" || exit 1
IFS= read -r expected_path < "$ORCH_SOURCE_CREDENTIAL_DIR/path" || exit 1
[ "$protocol" = "$expected_protocol" ] || exit 0
if [ "$host" != "$expected_host" ]; then
    # URL parsing removes HTTPS's default port; spelling :443 is the same authority.
    [ "$host" = "$expected_host:443" ] || exit 0
    case "$expected_host" in *']') ;; *:*) exit 0 ;; esac
fi
case "/$path/" in
    *"/../"*|*"/./"*|*//* ) exit 0 ;;
esac
case "$path" in
    "$expected_path"|"$expected_path"/*) ;;
    *) exit 0 ;;
esac
[ -n "$ORCH_SOURCE_CREDENTIAL_USERNAME" ] || exit 1
printf 'username=%s\npassword=%s\n' "$ORCH_SOURCE_CREDENTIAL_USERNAME" "$ORCH_SOURCE_CREDENTIAL_PASSWORD"
"#;

struct Group(Option<u32>);
impl Drop for Group {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let _ = platform::send_group(pid, Sig::Kill);
        }
    }
}

/// One controlled Git reply; error output is reduced to a safe category.
pub struct Run {
    /// Its exit code, where the operating system supplied one.
    pub code: Option<i32>,
    /// Bounded ordinary output, never the raw error output.
    pub stdout: Vec<u8>,
    /// The safe reason to show if the command failed.
    pub reason: &'static str,
}

impl Run {
    /// Requires success without exposing a URL, credential or arbitrary server response.
    pub fn success(self) -> Result<Vec<u8>> {
        if self.code == Some(0) {
            Ok(self.stdout)
        } else {
            Err(HostError::Unsupported(self.reason.to_owned()))
        }
    }
}

/// Input, explicit environment and repository credential for one controlled Git action.
#[derive(Default, Clone, Copy)]
pub struct RunOptions<'a> {
    /// Optional bytes supplied to this command's standard input.
    pub input: Option<&'a [u8]>,
    /// Explicit environment entries; Git still clears the inherited environment.
    pub extra_env: &'a [(OsString, OsString)],
    /// One repository-scoped credential, held only for this command.
    pub credentials: Option<&'a Credentials>,
}

/// Runs a fixed action with its input, environment and scoped credential kept together.
pub async fn run(
    tool: &GitTool,
    files: &Files,
    data: &Path,
    cwd: &Path,
    arguments: &[OsString],
    options: RunOptions<'_>,
) -> Result<Run> {
    let RunOptions {
        input,
        extra_env,
        credentials,
    } = options;
    files.gates().require_remote_write()?;
    let program = tool.program()?;
    let hooks = data.join("disabled-git-hooks");
    std::fs::create_dir_all(&hooks)
        .map_err(|error| HostError::io("making the disabled Git hooks directory", error))?;
    let filters = crate::git::filter_overrides(program, cwd, files).await?;
    let mut arguments_before_command = arguments.iter();
    let mut network = false;
    while let Some(argument) = arguments_before_command.next() {
        if argument == "-c" {
            arguments_before_command.next();
            continue;
        }
        if argument.to_string_lossy().starts_with('-') {
            continue;
        }
        network = matches!(argument.to_str(), Some("clone" | "push" | "ls-remote"));
        break;
    }
    // A repository cannot supply executable authentication helpers. An explicit transport
    // without a source secret may reuse only the environment's protected global credentials.
    // Values stay in the child environment, even when a trusted helper contains a password.
    let mut trusted = Vec::<(String, String)>::new();
    if network && credentials.is_none() {
        let config = words(&[
            "config",
            "--global",
            "--includes",
            "--null",
            "--get-regexp",
            "^credential(\\..*)?\\.(helper|username|usehttppath)$",
        ]);
        // The nested fixed config read is local-only; boxing breaks the async type cycle.
        let reply = Box::pin(run(
            tool,
            files,
            data,
            cwd,
            &config,
            RunOptions {
                extra_env,
                ..Default::default()
            },
        ))
        .await?;
        if reply.code == Some(0) {
            for record in reply
                .stdout
                .split(|byte| *byte == 0)
                .filter(|record| !record.is_empty())
            {
                let record = std::str::from_utf8(record).map_err(|_| {
                    HostError::Unsupported(
                        "the environment's Git authentication is not UTF-8".into(),
                    )
                })?;
                let (key, value) = record.split_once('\n').ok_or_else(|| {
                    HostError::Unsupported(
                        "the environment's Git authentication is malformed".into(),
                    )
                })?;
                trusted.push((key.to_owned(), value.to_owned()));
            }
        } else if reply.code != Some(1) {
            return Err(HostError::Unsupported(
                "the environment's Git authentication could not be read".into(),
            ));
        }
    }
    let mut command = tokio::process::Command::new(program);
    command
        .args(["--no-optional-locks", "--literal-pathspecs"])
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.pager=cat",
            "-c",
            "color.ui=never",
            "-c",
            "core.attributesFile=",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "tag.gpgSign=false",
            "-c",
            "gc.auto=0",
            "-c",
            "maintenance.auto=false",
            "-c",
            "http.followRedirects=false",
            "-c",
            "credential.interactive=false",
        ])
        .arg("-c")
        .arg(format!("core.hooksPath={}", hooks.to_string_lossy()))
        .arg("-c")
        .arg(format!("init.templateDir={}", hooks.to_string_lossy()))
        .args(filters)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(home) = std::env::var_os("HOME") {
        command.env("HOME", home);
    }
    if let Some(socket) = std::env::var_os("SSH_AUTH_SOCK") {
        command.env("SSH_AUTH_SOCK", socket);
    }
    // These are the driver's configured network routes, including restricted egress. They
    // stay in the process environment and never enter a URL, argument, plan or log.
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.envs(extra_env.iter().map(|(key, value)| (key, value)));
    command.env("GIT_NO_LAZY_FETCH", "1");
    // Older Git versions enforce the same local-only boundary through protocol denial.
    // Only the fixed network subcommands of an explicit source/push act may use transport.
    if !network {
        command.env("GIT_ALLOW_PROTOCOL", "");
    }
    if trusted.is_empty() {
        command.args(["-c", "credential.helper="]);
    } else {
        command
            .env("GIT_CONFIG_COUNT", (trusted.len() + 1).to_string())
            .env("GIT_CONFIG_KEY_0", "credential.helper")
            .env("GIT_CONFIG_VALUE_0", "");
        for (index, (key, value)) in trusted.iter().enumerate() {
            command
                .env(format!("GIT_CONFIG_KEY_{}", index + 1), key)
                .env(format!("GIT_CONFIG_VALUE_{}", index + 1), value);
        }
    }
    if let Some(credentials) = credentials {
        command
            .args([
                "-c",
                "credential.helper=",
                "-c",
                "credential.useHttpPath=true",
            ])
            .arg("-c")
            .arg(format!("credential.helper={}", credentials.helper))
            .env("ORCH_SOURCE_CREDENTIAL_DIR", credentials.directory.path())
            .env("ORCH_SOURCE_CREDENTIAL_USERNAME", &credentials.username)
            .env("ORCH_SOURCE_CREDENTIAL_PASSWORD", &credentials.password);
    }
    // Global -c options precede the subcommand. Passing them after clone would save them
    // into the new repository's config instead of keeping them command-only.
    command.args(arguments);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| HostError::io("starting the controlled Git action", error))?;
    let pid = child.id();
    let mut group = Group(pid);
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| HostError::Internal("the Git action has no output".to_owned()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| HostError::Internal("the Git action has no error stream".to_owned()))?;
    let stdin = child.stdin.take();
    let operation = async {
        let read_output = async {
            let mut bytes = Vec::new();
            (&mut stdout)
                .take(OUTPUT_MAX_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|error| HostError::io("reading the Git action's reply", error))?;
            if bytes.len() > OUTPUT_MAX_BYTES {
                if let Some(pid) = pid {
                    let _ = platform::send_group(pid, Sig::Kill);
                }
                return Err(HostError::Unsupported(
                    "the Git action's reply is too large".to_owned(),
                ));
            }
            Ok::<_, HostError>(bytes)
        };
        let read_error = async {
            let mut bytes = Vec::new();
            let _ = (&mut stderr)
                .take(ERROR_KEEP_BYTES)
                .read_to_end(&mut bytes)
                .await;
            let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
            bytes
        };
        let write_input = async {
            if let (Some(mut stdin), Some(input)) = (stdin, input) {
                stdin
                    .write_all(input)
                    .await
                    .map_err(|error| HostError::io("giving the Git action its input", error))?;
                stdin
                    .shutdown()
                    .await
                    .map_err(|error| HostError::io("ending the Git action's input", error))?;
            }
            Ok::<_, HostError>(())
        };
        let (output, errors, wrote) = tokio::join!(read_output, read_error, write_input);
        wrote?;
        let code = child
            .wait()
            .await
            .map_err(|error| HostError::io("waiting for the Git action", error))?
            .code();
        let text = String::from_utf8_lossy(&errors).to_ascii_lowercase();
        let reason = if text.contains("authentication failed")
            || text.contains("could not read username")
            || text.contains("permission denied (publickey)")
        {
            "Git refused the repository credential"
        } else if text.contains("remote branch") && text.contains("not found") {
            "the requested source branch does not exist"
        } else if text.contains("non-fast-forward") || text.contains("[rejected]") {
            "the remote refused a fast-forward push"
        } else {
            "Git refused the requested operation"
        };
        Ok::<_, HostError>(Run {
            code,
            stdout: output?,
            reason,
        })
    };
    let deadline = Duration::from_secs(u64::from(
        orch_link_contract::commands::CommandKind::GitPrepare.deadline_secs(),
    ));
    let result = tokio::time::timeout(deadline, operation)
        .await
        .map_err(|_| {
            HostError::Timeout("the Git action exceeded its operation deadline".to_owned())
        })?;
    if result.is_ok() {
        group.0 = None;
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    device: u64,
    inode: u64,
}

impl Identity {
    fn of(meta: &platform::Meta) -> Self {
        Self {
            device: meta.dev,
            inode: meta.ino,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PreparePhase {
    Empty,
    Fetching,
    Staged,
    Installing,
    Installed,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedEntry {
    name: String,
    identity: Identity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareJournal {
    request_hash: Hash,
    generation: i64,
    directory: String,
    staging: String,
    parent: Identity,
    stage: Identity,
    phase: PreparePhase,
    entries: Vec<PreparedEntry>,
    next: usize,
    result: Option<SourceInfo>,
    #[serde(default)]
    retained: Vec<PreparedEntry>,
    #[serde(default)]
    prepared: Option<SourceInfo>,
    #[serde(default)]
    cleanup_complete: bool,
}

fn save_prepare(states: &FileStates, request: &GitPrepare, journal: &PrepareJournal) -> Result<()> {
    let bytes =
        serde_json::to_vec(journal).map_err(|error| HostError::Internal(error.to_string()))?;
    crate::state_store::atomic_write(
        &states
            .store()
            .root()
            .join("source-jobs")
            .join(format!("{}.json", request.job_id)),
        &bytes,
    )
}

/// A finished preparation needs no replacement secret or fetch to return its first result.
pub(crate) fn completed(
    states: &FileStates,
    request: &GitPrepare,
    generation: i64,
) -> Result<Option<SourceInfo>> {
    let path = states
        .store()
        .root()
        .join("source-jobs")
        .join(format!("{}.json", request.job_id));
    if !path.exists() {
        return Ok(None);
    }
    let journal: PrepareJournal = serde_json::from_slice(&crate::state_store::read_bounded(
        &path,
        crate::manifest::MANIFEST_MAX_EXPANDED_BYTES,
    )?)
    .map_err(|_| HostError::Unsupported("the source journal needs recovery".into()))?;
    let mut frozen = request.clone();
    frozen.credential = None;
    let hash = Hash::of(
        &serde_json::to_vec(&frozen).map_err(|error| HostError::Internal(error.to_string()))?,
    );
    if journal.request_hash != hash || journal.generation != generation {
        return Err(HostError::Changed(
            "the source job changed its choice or generation".into(),
        ));
    }
    if journal.cleanup_complete {
        return Err(HostError::Changed(
            "the empty source stage was removed; retry with a new effect identity".into(),
        ));
    }
    Ok(journal.result)
}

/// Cleans an empty stage under its original directory handle, never a checkout or retained bytes.
pub async fn cleanup(
    files: &Files,
    states: &FileStates,
    request: &GitPrepare,
    generation: i64,
) -> Result<orch_link_contract::source::GitPrepareCleanupResult> {
    use orch_link_contract::source::GitPrepareCleanupResult;
    files.gates().require_remote_write()?;
    let _gate = states.action_gate().await;
    let path = states
        .store()
        .root()
        .join("source-jobs")
        .join(format!("{}.json", request.job_id));
    if !path.exists() {
        return Ok(GitPrepareCleanupResult {
            removed: true,
            reason: None,
        });
    }
    let mut journal: PrepareJournal = serde_json::from_slice(&crate::state_store::read_bounded(
        &path,
        crate::manifest::MANIFEST_MAX_EXPANDED_BYTES,
    )?)
    .map_err(|_| HostError::Unsupported("the source journal needs recovery".into()))?;
    let mut frozen = request.clone();
    frozen.credential = None;
    if journal.request_hash
        != Hash::of(
            &serde_json::to_vec(&frozen).map_err(|error| HostError::Internal(error.to_string()))?,
        )
        || journal.generation != generation
    {
        return Err(HostError::Changed(
            "the source cleanup changed its choice or generation".into(),
        ));
    }
    if journal.cleanup_complete {
        return Ok(GitPrepareCleanupResult {
            removed: true,
            reason: None,
        });
    }
    if !matches!(journal.phase, PreparePhase::Empty | PreparePhase::Fetching) {
        return Ok(GitPrepareCleanupResult {
            removed: false,
            reason: Some("prepared_or_installed_checkout_kept".into()),
        });
    }
    let directory = files.parse(&journal.directory)?;
    let parent = verify_parent(files, &directory, &journal)?;
    let mut stages = journal.retained.clone();
    stages.push(PreparedEntry {
        name: journal.staging.clone(),
        identity: journal.stage,
    });
    for stage in &stages {
        let name = std::ffi::OsStr::new(&stage.name);
        let meta = match parent.lstat(name) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(HostError::io("checking the source cleanup stage", error)),
        };
        if Identity::of(&meta) != stage.identity || !meta.is_dir() {
            return Ok(GitPrepareCleanupResult {
                removed: false,
                reason: Some("replaced_stage_kept".into()),
            });
        }
        let held = parent
            .open_child(name)
            .map_err(|error| HostError::io("holding the source cleanup stage", error))?;
        if !held
            .names()
            .map_err(|error| HostError::io("checking the source cleanup names", error))?
            .is_empty()
        {
            return Ok(GitPrepareCleanupResult {
                removed: false,
                reason: Some("nonempty_stage_kept".into()),
            });
        }
    }
    for stage in &stages {
        let name = std::ffi::OsStr::new(&stage.name);
        match parent.lstat(name) {
            Ok(meta) if Identity::of(&meta) == stage.identity && meta.is_dir() => {
                // rmdir also refuses a name added since the empty check; no recursive unlink.
                parent.rmdir(name).map_err(|error| {
                    HostError::io("removing an empty owned source stage", error)
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => {
                return Err(HostError::Changed(
                    "the empty source stage was replaced before cleanup".into(),
                ));
            }
        }
    }
    journal.cleanup_complete = true;
    save_prepare(states, request, &journal)?;
    Ok(GitPrepareCleanupResult {
        removed: true,
        reason: None,
    })
}

fn parent_for(files: &Files, directory: &RelPath) -> Result<platform::Dir> {
    if directory.is_root() {
        return files.root().root_dir();
    }
    let parent = directory.parent().unwrap_or_else(RelPath::root);
    let located = files.root().locate(&parent, Locate::THROUGH)?;
    if located.real != parent {
        return Err(HostError::Changed(
            "the source parent became another directory".to_owned(),
        ));
    }
    files.check_visible(&parent, &located, true)?;
    files.root().open_dir(&located)
}

fn verify_parent(
    files: &Files,
    directory: &RelPath,
    journal: &PrepareJournal,
) -> Result<platform::Dir> {
    let parent = parent_for(files, directory)?;
    let meta = parent
        .stat_self()
        .map_err(|error| HostError::io("checking the source destination's parent", error))?;
    if Identity::of(&meta) != journal.parent {
        return Err(HostError::Changed(
            "the source destination's parent changed while source was fetched".to_owned(),
        ));
    }
    Ok(parent)
}

fn words(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

async fn verified_source(
    tool: &GitTool,
    files: &Files,
    states: &FileStates,
    cwd: &Path,
    request: &GitPrepare,
) -> Result<SourceInfo> {
    let head = run(
        tool,
        files,
        states.store().root(),
        cwd,
        &words(&["rev-parse", "--verify", "HEAD"]),
        RunOptions::default(),
    )
    .await?
    .success()?;
    let head = std::str::from_utf8(&head)
        .map_err(|_| HostError::Unsupported("the source commit is not UTF-8".to_owned()))?
        .trim()
        .to_owned();
    if !matches!(head.len(), 40 | 64) || !head.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(HostError::Unsupported(
            "the source has no advertised default commit".to_owned(),
        ));
    }
    let branch = run(
        tool,
        files,
        states.store().root(),
        cwd,
        &words(&["symbolic-ref", "--quiet", "--short", "HEAD"]),
        RunOptions::default(),
    )
    .await?
    .success()?;
    let branch = std::str::from_utf8(&branch)
        .map_err(|_| HostError::Unsupported("the source branch is not UTF-8".to_owned()))?
        .trim()
        .to_owned();
    if request
        .branch
        .as_ref()
        .is_some_and(|expected| expected != &branch)
    {
        return Err(HostError::Changed(
            "the fetched source branch differs from the frozen choice".to_owned(),
        ));
    }
    Ok(SourceInfo {
        kind: SourceKind::Repository,
        directory: request.directory.clone(),
        repository_url: crate::git::safe_remote_url(&request.repository_url),
        branch: Some(branch),
        head: Some(head),
        state: SourceState::Ready,
        job: None,
        reason: None,
    })
}

/// Fetches into a job-owned staging directory and installs only into the frozen missing or owned root.
pub async fn prepare(
    tool: &GitTool,
    files: &Files,
    states: &FileStates,
    request: &GitPrepare,
    generation: i64,
    managed: bool,
    credential: Option<&str>,
) -> Result<SourceInfo> {
    files.gates().require_remote_write()?;
    let _operation = states.action_gate().await;
    if !orch_settings_contract::registry::is_repository_address(&request.repository_url)
        || request
            .branch
            .as_deref()
            .is_some_and(|branch| !orch_settings_contract::registry::is_git_branch(branch))
    {
        return Err(HostError::Unsupported(
            "the source address or branch is invalid".to_owned(),
        ));
    }
    if request.credential.is_some() != credential.is_some() {
        return Err(HostError::Unsupported(
            "the configured source credential could not be opened".to_owned(),
        ));
    }
    let directory = files.parse(&request.directory)?;
    let owned_root = match &request.destination {
        SourceDestination::MissingDirectory if !directory.is_root() => false,
        SourceDestination::OwnedEmptyRoot {
            generation: expected,
            workspace_volume,
            ..
        } if managed
            && directory.is_root()
            && generation == *expected
            && !workspace_volume.is_empty() =>
        {
            true
        }
        _ => {
            return Err(HostError::Changed(
                "the source destination is not the frozen owned root or a missing child".to_owned(),
            ));
        }
    };
    let mut frozen = request.clone();
    frozen.credential = None;
    let asked =
        serde_json::to_vec(&frozen).map_err(|error| HostError::Internal(error.to_string()))?;
    let request_hash = Hash::of(&asked);
    let journal_path = states
        .store()
        .root()
        .join("source-jobs")
        .join(format!("{}.json", request.job_id));
    let mut journal = if journal_path.exists() {
        let bytes = crate::state_store::read_bounded(
            &journal_path,
            crate::manifest::MANIFEST_MAX_EXPANDED_BYTES,
        )?;
        let journal: PrepareJournal = serde_json::from_slice(&bytes)
            .map_err(|_| HostError::Unsupported("the source journal needs recovery".to_owned()))?;
        if journal.request_hash != request_hash
            || journal.generation != generation
            || journal.directory != request.directory
        {
            return Err(HostError::Changed(
                "the source job was reused with another frozen choice".to_owned(),
            ));
        }
        journal
    } else {
        if let Some(parent) = directory.parent().filter(|parent| !parent.is_root()) {
            files.make_dir(&parent)?;
        }
        let located = files.root().locate(&directory, Locate::ENTRY)?;
        files.check_visible(&directory, &located, true)?;
        let parent = parent_for(files, &directory)?;
        if owned_root {
            if !parent
                .names()
                .map_err(|error| {
                    HostError::io("checking the newly owned empty source root", error)
                })?
                .is_empty()
            {
                return Err(HostError::Changed(
                    "the newly owned source root contains unexpected files".to_owned(),
                ));
            }
        } else if located.meta.is_some() {
            return Err(HostError::Changed(
                "the source directory already exists; its files are kept".to_owned(),
            ));
        }
        let staging = format!(".orch-tmp-source-{}", request.job_id);
        parent
            .mkdir(std::ffi::OsStr::new(&staging), 0o700)
            .map_err(|error| HostError::io("making the owned source staging directory", error))?;
        let stage = parent
            .lstat(std::ffi::OsStr::new(&staging))
            .map_err(|error| HostError::io("checking the owned source staging directory", error))?;
        let meta = parent
            .stat_self()
            .map_err(|error| HostError::io("checking the source parent", error))?;
        let journal = PrepareJournal {
            request_hash,
            generation,
            directory: request.directory.clone(),
            staging,
            parent: Identity::of(&meta),
            stage: Identity::of(&stage),
            phase: PreparePhase::Empty,
            entries: Vec::new(),
            next: 0,
            result: None,
            retained: Vec::new(),
            prepared: None,
            cleanup_complete: false,
        };
        save_prepare(states, request, &journal)?;
        journal
    };
    if journal.cleanup_complete {
        return Err(HostError::Changed(
            "the empty source stage was removed; retry with a new effect identity".into(),
        ));
    }
    if let Some(result) = journal.result.clone() {
        return Ok(result);
    }
    let parent = verify_parent(files, &directory, &journal)?;
    let mut stage_name = OsString::from(journal.staging.as_str());
    if matches!(journal.phase, PreparePhase::Empty | PreparePhase::Fetching) {
        let meta = parent
            .lstat(&stage_name)
            .map_err(|error| HostError::io("checking source staging before retry", error))?;
        if !meta.is_dir() || Identity::of(&meta) != journal.stage {
            return Err(HostError::Changed(
                "the source staging directory is not owned by this job".to_owned(),
            ));
        }
        let mut stage = parent
            .open_child(&stage_name)
            .map_err(|error| HostError::io("opening the owned source staging directory", error))?;
        if !stage
            .names()
            .map_err(|error| HostError::io("checking unfinished source names", error))?
            .is_empty()
        {
            // A killed fetch has no complete inventory proving that every byte still belongs
            // to it. Keep that stage instead of deleting later writes during Retry.
            if owned_root {
                return Err(HostError::Changed(
                    "the unfinished source root contains unverified data; its files are kept"
                        .into(),
                ));
            }
            let retained = PreparedEntry {
                name: journal.staging.clone(),
                identity: journal.stage,
            };
            let next = format!(
                ".orch-tmp-source-{}-{}",
                request.job_id,
                orch_types::new_id()
            );
            parent
                .mkdir(std::ffi::OsStr::new(&next), 0o700)
                .map_err(|error| HostError::io("making a fresh source stage", error))?;
            let meta = parent
                .lstat(std::ffi::OsStr::new(&next))
                .map_err(|error| HostError::io("holding the fresh source stage", error))?;
            journal.retained.push(retained);
            journal.staging = next;
            journal.stage = Identity::of(&meta);
            stage_name = OsString::from(journal.staging.as_str());
            save_prepare(states, request, &journal)?;
            stage = parent
                .open_child(&stage_name)
                .map_err(|error| HostError::io("opening the fresh source stage", error))?;
        }
        journal.phase = PreparePhase::Fetching;
        save_prepare(states, request, &journal)?;
        let credentials = credential
            .map(|value| {
                Credentials::for_repository(states.store().root(), &request.repository_url, value)
            })
            .transpose()?;
        let repository = reqwest::Url::parse(&request.repository_url)
            .map_or_else(|_| request.repository_url.clone(), |url| url.to_string());
        let mut args = words(&[
            "-c",
            "protocol.file.allow=never",
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "protocol.git.allow=never",
            "-c",
            "core.sshCommand=ssh -o BatchMode=yes",
            "clone",
            "--no-checkout",
            "--origin",
            "origin",
        ]);
        if let Some(branch) = &request.branch {
            args.push(OsString::from("--branch"));
            args.push(OsString::from(branch));
        }
        args.extend(words(&["--", &repository, "."]));
        let fetched = run(
            tool,
            files,
            states.store().root(),
            stage.path(),
            &args,
            RunOptions {
                credentials: credentials.as_ref(),
                ..Default::default()
            },
        )
        .await?;
        if fetched.code != Some(0) {
            journal.phase = PreparePhase::Empty;
            save_prepare(states, request, &journal)?;
            return Err(HostError::Unsupported(fetched.reason.to_owned()));
        }
        // An owned empty tree is written in this repository's own object format; the
        // checkout then reads no project attributes, even for a SHA-256 repository.
        let empty = run(
            tool,
            files,
            states.store().root(),
            stage.path(),
            &words(&["hash-object", "-w", "-t", "tree", "--stdin"]),
            RunOptions {
                input: Some(&[]),
                ..Default::default()
            },
        )
        .await?
        .success()?;
        let empty = std::str::from_utf8(&empty)
            .map_err(|_| HostError::Unsupported("the empty source tree is invalid".to_owned()))?
            .trim();
        let mut checkout = Vec::new();
        if tool.has_attribute_source() {
            checkout.push(OsString::from(format!("--attr-source={empty}")));
        }
        checkout.extend(words(&["read-tree", "--reset", "-u", "HEAD"]));
        run(
            tool,
            files,
            states.store().root(),
            stage.path(),
            &checkout,
            RunOptions::default(),
        )
        .await?
        .success()?;
        journal.prepared = Some(verified_source(tool, files, states, stage.path(), request).await?);
        journal.entries = stage
            .names()
            .map_err(|error| HostError::io("listing prepared source entries", error))?
            .into_iter()
            .map(|name| {
                let meta = stage
                    .lstat(&name)
                    .map_err(|error| HostError::io("checking prepared source ownership", error))?;
                let name = name.into_string().map_err(|_| {
                    HostError::Unsupported("the source has a non-UTF-8 top-level name".to_owned())
                })?;
                Ok(PreparedEntry {
                    name,
                    identity: Identity::of(&meta),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        journal.phase = PreparePhase::Staged;
        save_prepare(states, request, &journal)?;
    }
    let stage_name = stage_name.as_os_str();
    let parent = verify_parent(files, &directory, &journal)?;
    if journal.phase == PreparePhase::Staged {
        let prepared = journal.prepared.as_ref().ok_or_else(|| {
            HostError::Unsupported("the prepared source needs its original frozen head".into())
        })?;
        let stage = parent.open_child(stage_name).map_err(|error| {
            HostError::io("holding the prepared source before publication", error)
        })?;
        crate::git_worktrees::verify_source_checkout(
            tool,
            files,
            states,
            stage.path(),
            prepared
                .head
                .as_deref()
                .ok_or_else(|| HostError::Unsupported("the source has no frozen head".into()))?,
            prepared
                .branch
                .as_deref()
                .ok_or_else(|| HostError::Unsupported("the source has no frozen branch".into()))?,
        )
        .await?;
    }
    if owned_root
        && journal.phase == PreparePhase::Installing
        && journal.next == journal.entries.len()
        && parent
            .lstat(stage_name)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        // A restart may follow removal of the now-empty stage before the Installed journal
        // record reached disk. Reconcile every installed name and ownership, then continue.
        let names = parent
            .names()
            .map_err(|error| HostError::io("recovering the installed source root", error))?;
        if names.len() != journal.entries.len() {
            return Err(HostError::Changed(
                "the installed source root gained other entries".to_owned(),
            ));
        }
        for entry in &journal.entries {
            let meta = parent
                .lstat(std::ffi::OsStr::new(&entry.name))
                .map_err(|error| HostError::io("recovering an installed source entry", error))?;
            if Identity::of(&meta) != entry.identity {
                return Err(HostError::Changed(
                    "an installed source entry was replaced".to_owned(),
                ));
            }
        }
        journal.phase = PreparePhase::Installed;
        save_prepare(states, request, &journal)?;
    }
    if matches!(
        journal.phase,
        PreparePhase::Staged | PreparePhase::Installing
    ) {
        if owned_root {
            let names = parent
                .names()
                .map_err(|error| HostError::io("rechecking the owned source root", error))?;
            if journal.phase == PreparePhase::Staged
                && names.iter().any(|name| name.as_os_str() != stage_name)
            {
                return Err(HostError::Changed(
                    "the newly owned source root was filled while source was fetched".to_owned(),
                ));
            }
            if names.iter().any(|name| {
                name.as_os_str() != stage_name
                    && !journal
                        .entries
                        .iter()
                        .any(|entry| std::ffi::OsStr::new(&entry.name) == name.as_os_str())
            }) {
                return Err(HostError::Changed(
                    "unexpected files appeared in the owned source root".to_owned(),
                ));
            }
            let stage = parent
                .open_child(stage_name)
                .map_err(|error| HostError::io("opening staged source for installation", error))?;
            if Identity::of(
                &stage
                    .stat_self()
                    .map_err(|error| HostError::io("checking staged source ownership", error))?,
            ) != journal.stage
            {
                return Err(HostError::Changed(
                    "the source stage changed before installation".to_owned(),
                ));
            }
            // Check every name before the first effect, including one whose same name raced
            // the plan. A name already installed by this journal must have the recorded inode.
            for entry in &journal.entries {
                let name = std::ffi::OsStr::new(&entry.name);
                match parent.lstat(name) {
                    Ok(meta) if Identity::of(&meta) == entry.identity => {}
                    Ok(_) => {
                        return Err(HostError::Changed(
                            "a source root name was filled by another writer".to_owned(),
                        ));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        let meta = stage.lstat(name).map_err(|error| {
                            HostError::io("checking staged source before installation", error)
                        })?;
                        if Identity::of(&meta) != entry.identity {
                            return Err(HostError::Changed(
                                "a staged source entry changed".to_owned(),
                            ));
                        }
                    }
                    Err(error) => {
                        return Err(HostError::io(
                            "checking the source root before installation",
                            error,
                        ));
                    }
                }
            }
            journal.phase = PreparePhase::Installing;
            save_prepare(states, request, &journal)?;
            while journal.next < journal.entries.len() {
                let entry = &journal.entries[journal.next];
                let name = std::ffi::OsStr::new(&entry.name);
                if !parent
                    .lstat(name)
                    .is_ok_and(|meta| Identity::of(&meta) == entry.identity)
                {
                    stage
                        .rename_to(name, &parent, name, false)
                        .map_err(|error| {
                            HostError::io("installing one owned source entry", error)
                        })?;
                    parent.sync().map_err(|error| {
                        HostError::io("syncing installed source entries", error)
                    })?;
                }
                journal.next += 1;
                save_prepare(states, request, &journal)?;
            }
            parent
                .rmdir(stage_name)
                .map_err(|error| HostError::io("removing the empty source stage", error))?;
        } else {
            let destination = files.root().locate(&directory, Locate::ENTRY)?;
            if let Some(meta) = destination.meta {
                if Identity::of(&meta) != journal.stage {
                    return Err(HostError::Changed(
                        "another writer created the source destination".to_owned(),
                    ));
                }
            } else {
                let meta = parent
                    .lstat(stage_name)
                    .map_err(|error| HostError::io("checking source staging ownership", error))?;
                if Identity::of(&meta) != journal.stage {
                    return Err(HostError::Changed(
                        "the source staging directory changed".to_owned(),
                    ));
                }
                parent
                    .rename_to(stage_name, &parent, destination.name()?, false)
                    .map_err(|error| {
                        HostError::io("installing the owned source checkout", error)
                    })?;
                parent
                    .sync()
                    .map_err(|error| HostError::io("syncing the source checkout", error))?;
            }
        }
        journal.phase = PreparePhase::Installed;
        save_prepare(states, request, &journal)?;
    }
    let destination = files.root().locate(&directory, Locate::THROUGH)?;
    if destination.real != directory {
        return Err(HostError::Changed(
            "the source destination became another place".to_owned(),
        ));
    }
    let mut result = verified_source(
        tool,
        files,
        states,
        &directory.under(files.root().root_path()),
        request,
    )
    .await?;
    if let Some(prepared) = &journal.prepared {
        crate::git_worktrees::verify_source_checkout(
            tool,
            files,
            states,
            &directory.under(files.root().root_path()),
            prepared
                .head
                .as_deref()
                .ok_or_else(|| HostError::Unsupported("the source has no frozen head".into()))?,
            prepared
                .branch
                .as_deref()
                .ok_or_else(|| HostError::Unsupported("the source has no frozen branch".into()))?,
        )
        .await?;
    }
    if !journal.retained.is_empty() {
        result.reason = Some("stage_retained".into());
    }
    journal.result = Some(result.clone());
    journal.phase = PreparePhase::Complete;
    save_prepare(states, request, &journal)?;
    states.invalidate("not_ready");
    Ok(result)
}

#[cfg(all(test, unix))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod tests {
    use super::*;
    use crate::environment::{GateState, Gates, Limits};
    use crate::ignore::IgnoreRules;
    use crate::index::HashCache;
    use crate::paths::{PathStyle, Workspace};
    use crate::state_store::ObjectStore;
    use orch_types::new_id;
    use std::io::Write;
    use std::sync::Arc;

    fn setup() -> (tempfile::TempDir, Files, FileStates) {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let files = Files::workspace(
            Workspace::open(&workspace, &[], PathStyle::Posix, true).unwrap(),
            Arc::new(IgnoreRules::new(&[], true)),
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        );
        let states =
            FileStates::new(ObjectStore::open(&directory.path().join("kept"), &workspace).unwrap());
        (directory, files, states)
    }

    fn source(directory: &str) -> GitPrepare {
        GitPrepare {
            environment_id: new_id(),
            job_id: new_id(),
            directory: directory.to_owned(),
            repository_url: "https://example.test/team/repository.git".to_owned(),
            branch: None,
            destination: SourceDestination::MissingDirectory,
            credential: None,
        }
    }

    /// Actual Git prepares the fixture and performs clone/read-tree. This owned executable only
    /// substitutes a local bare repository for HTTPS transport; it makes no authentication claim.
    fn local_transport(directory: &Path, after_clone: &str) -> GitTool {
        use std::os::unix::fs::PermissionsExt;
        let real = GitTool::discover().program().unwrap().to_owned();
        let git = |cwd: &Path, arguments: &[&str]| {
            let output = std::process::Command::new(&real)
                .current_dir(cwd)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "Git fixture failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        let original = directory.join("original");
        std::fs::create_dir(&original).unwrap();
        git(&original, &["init", "-b", "advertised"]);
        std::fs::write(original.join("code"), "from source\n").unwrap();
        std::fs::write(original.join("binary"), [0, 255, 1]).unwrap();
        git(&original, &["add", "."]);
        git(
            &original,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@orch.invalid",
                "commit",
                "-m",
                "fixture",
            ],
        );
        let bare = directory.join("remote.git");
        git(
            directory,
            &[
                "clone",
                "--bare",
                original.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        let bin = directory.join("bin");
        std::fs::create_dir(&bin).unwrap();
        let script = bin.join("git");
        let script_body = format!(
            "#!/bin/sh\nis_clone=no\nfor argument in \"$@\"; do\n [ \"$argument\" = clone ] && is_clone=yes\ndone\n\
            if [ \"$is_clone\" = yes ]; then\n {real} -c core.hooksPath=/dev/null clone --no-checkout --origin origin {bare} . || exit $?\n\
            {real} config remote.origin.url https://example.test/team/repository.git || exit $?\n {after_clone}\n exit 0\nfi\nexec {real} \"$@\"\n",
            real = shell_word(&real.to_string_lossy()),
            bare = shell_word(&bare.to_string_lossy())
        );
        std::fs::write(&script, script_body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        GitTool::find_in(&bin.into_os_string())
    }

    #[tokio::test]
    async fn a_real_checkout_keeps_the_advertised_branch_and_retry_keeps_the_first_result() {
        let (directory, files, states) = setup();
        let tool = local_transport(directory.path(), ":");
        let request = source("attempt");
        let prepared = prepare(&tool, &files, &states, &request, 1, false, None)
            .await
            .unwrap();
        assert_eq!(
            prepared.branch.as_deref(),
            Some("advertised"),
            "no main/master default was guessed or detached"
        );
        assert_eq!(prepared.state, SourceState::Ready);
        assert_eq!(
            std::fs::read(files.root().root_path().join("attempt/code")).unwrap(),
            b"from source\n"
        );
        assert_eq!(
            std::fs::read(files.root().root_path().join("attempt/binary")).unwrap(),
            [0, 255, 1]
        );
        std::fs::write(files.root().root_path().join("attempt/code"), "later work").unwrap();
        let kept = cleanup(&files, &states, &request, 1).await.unwrap();
        assert!(
            !kept.removed,
            "an installed checkout is never a staging cleanup target"
        );
        assert_eq!(
            prepare(&tool, &files, &states, &request, 1, false, None)
                .await
                .unwrap(),
            prepared
        );
        assert_eq!(
            std::fs::read(files.root().root_path().join("attempt/code")).unwrap(),
            b"later work",
            "retry does not clone over later files"
        );
        let changed = GitPrepare {
            branch: Some("different".to_owned()),
            ..request
        };
        assert!(matches!(
            prepare(&tool, &files, &states, &changed, 1, false, None).await,
            Err(HostError::Changed(_))
        ));
    }

    #[tokio::test]
    async fn cancellation_removes_only_an_empty_stage_and_keeps_a_later_file_or_replacement() {
        for mode in ["empty", "later", "replacement"] {
            let (_directory, files, states) = setup();
            let request = source("attempt");
            let directory = files.parse(&request.directory).unwrap();
            let parent = parent_for(&files, &directory).unwrap();
            let staging = format!(".orch-tmp-source-{}", request.job_id);
            let name = std::ffi::OsStr::new(&staging);
            parent.mkdir(name, 0o700).unwrap();
            let mut frozen = request.clone();
            frozen.credential = None;
            let journal = PrepareJournal {
                request_hash: Hash::of(&serde_json::to_vec(&frozen).unwrap()),
                generation: 1,
                directory: request.directory.clone(),
                staging: staging.clone(),
                parent: Identity::of(&parent.stat_self().unwrap()),
                stage: Identity::of(&parent.lstat(name).unwrap()),
                phase: PreparePhase::Empty,
                entries: Vec::new(),
                next: 0,
                result: None,
                retained: Vec::new(),
                prepared: None,
                cleanup_complete: false,
            };
            save_prepare(&states, &request, &journal).unwrap();
            if mode == "later" {
                std::fs::write(parent.path().join(name).join("keep"), b"later work").unwrap();
            }
            if mode == "replacement" {
                std::fs::rename(parent.path().join(name), parent.path().join("old-stage")).unwrap();
                parent.mkdir(name, 0o700).unwrap();
            }
            let result = cleanup(&files, &states, &request, 1).await.unwrap();
            assert_eq!(result.removed, mode == "empty");
            if mode == "empty" {
                assert!(!parent.path().join(name).exists());
                assert!(
                    cleanup(&files, &states, &request, 1).await.unwrap().removed,
                    "a lost cleanup answer is idempotent"
                );
            } else {
                assert!(parent.path().join(name).is_dir());
            }
            if mode == "later" {
                assert_eq!(
                    std::fs::read(parent.path().join(name).join("keep")).unwrap(),
                    b"later work"
                );
            }
        }
    }

    #[tokio::test]
    async fn retry_of_an_interrupted_fetch_keeps_a_later_stage_file_and_the_original_destination() {
        let (directory, files, states) = setup();
        let first = shell_word(&directory.path().join("first-fetch").to_string_lossy());
        let tool = local_transport(
            directory.path(),
            &format!("if [ ! -f {first} ]; then touch {first}; exit 9; fi"),
        );
        let request = source("attempt");
        assert!(
            prepare(&tool, &files, &states, &request, 1, false, None)
                .await
                .is_err()
        );
        let journal_path = states
            .store()
            .root()
            .join("source-jobs")
            .join(format!("{}.json", request.job_id));
        let before: PrepareJournal =
            serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
        let stage = files.root().root_path().join(&before.staging);
        std::fs::write(stage.join("later"), "another writer's bytes").unwrap();
        let result = prepare(&tool, &files, &states, &request, 1, false, None)
            .await
            .unwrap();
        assert_eq!(result.directory, "attempt");
        assert_eq!(result.repository_url, Some(request.repository_url.clone()));
        assert_eq!(result.reason.as_deref(), Some("stage_retained"));
        assert_eq!(
            std::fs::read(stage.join("later")).unwrap(),
            b"another writer's bytes"
        );
        assert_eq!(
            std::fs::read(files.root().root_path().join("attempt/code")).unwrap(),
            b"from source\n"
        );
        let after: PrepareJournal =
            serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
        assert!(
            after
                .retained
                .iter()
                .any(|retained| retained.name == before.staging)
        );
    }

    #[tokio::test]
    async fn source_publication_refuses_a_name_added_after_the_prepared_head_was_saved() {
        let (directory, files, states) = setup();
        let tool = local_transport(directory.path(), ":");
        let request = source("attempt");
        prepare(&tool, &files, &states, &request, 1, false, None)
            .await
            .unwrap();
        let journal_path = states
            .store()
            .root()
            .join("source-jobs")
            .join(format!("{}.json", request.job_id));
        let mut journal: PrepareJournal =
            serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
        let stage = files.root().root_path().join(&journal.staging);
        std::fs::rename(files.root().root_path().join("attempt"), &stage).unwrap();
        journal.phase = PreparePhase::Staged;
        journal.result = None;
        save_prepare(&states, &request, &journal).unwrap();
        std::fs::write(stage.join("later"), "later data").unwrap();
        assert!(matches!(
            prepare(&tool, &files, &states, &request, 1, false, None).await,
            Err(HostError::Changed(_))
        ));
        assert!(!files.root().root_path().join("attempt").exists());
        assert_eq!(std::fs::read(stage.join("later")).unwrap(), b"later data");
    }

    #[tokio::test]
    async fn an_interrupted_owned_root_keeps_unknown_bytes_instead_of_cleaning_them_for_retry() {
        let (directory, files, states) = setup();
        let tool = local_transport(directory.path(), "exit 9");
        let mut request = source("");
        request.destination = SourceDestination::OwnedEmptyRoot {
            provisioning_job_id: request.job_id,
            generation: 1,
            workspace_volume: "orch-workspace-owned".into(),
        };
        assert!(
            prepare(&tool, &files, &states, &request, 1, true, None)
                .await
                .is_err()
        );
        let journal_path = states
            .store()
            .root()
            .join("source-jobs")
            .join(format!("{}.json", request.job_id));
        let journal: PrepareJournal =
            serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
        let stage = files.root().root_path().join(&journal.staging);
        std::fs::write(stage.join("later"), "kept").unwrap();
        assert!(matches!(
            prepare(&tool, &files, &states, &request, 1, true, None).await,
            Err(HostError::Changed(_))
        ));
        assert_eq!(std::fs::read(stage.join("later")).unwrap(), b"kept");
        assert!(!files.root().root_path().join(".git").exists());
    }

    #[tokio::test]
    async fn a_preexisting_or_racing_destination_is_preserved_and_an_owned_root_is_filled_without_replacement()
     {
        let (directory, files, states) = setup();
        let tool = local_transport(directory.path(), ":");
        std::fs::create_dir(files.root().root_path().join("existing")).unwrap();
        std::fs::write(
            files.root().root_path().join("existing/keep"),
            "person's files",
        )
        .unwrap();
        assert!(matches!(
            prepare(&tool, &files, &states, &source("existing"), 1, false, None).await,
            Err(HostError::Changed(_))
        ));
        assert_eq!(
            std::fs::read(files.root().root_path().join("existing/keep")).unwrap(),
            b"person's files"
        );

        let (directory, files, states) = setup();
        let race = format!(
            "mkdir {destination}; printf '%s' 'racing files' > {file}",
            destination = shell_word(&files.root().root_path().join("racing").to_string_lossy()),
            file = shell_word(
                &files
                    .root()
                    .root_path()
                    .join("racing/keep")
                    .to_string_lossy()
            )
        );
        let tool = local_transport(directory.path(), &race);
        assert!(matches!(
            prepare(&tool, &files, &states, &source("racing"), 1, false, None).await,
            Err(HostError::Changed(_))
        ));
        assert_eq!(
            std::fs::read(files.root().root_path().join("racing/keep")).unwrap(),
            b"racing files"
        );

        let (directory, files, states) = setup();
        let tool = local_transport(directory.path(), ":");
        let request = GitPrepare {
            destination: SourceDestination::OwnedEmptyRoot {
                provisioning_job_id: new_id(),
                workspace_volume: "job-owned-workspace".to_owned(),
                generation: 3,
            },
            ..source("")
        };
        assert!(
            prepare(&tool, &files, &states, &request, 3, false, None)
                .await
                .is_err(),
            "a connected root cannot assert provisioning ownership"
        );
        let prepared = prepare(&tool, &files, &states, &request, 3, true, None)
            .await
            .unwrap();
        assert_eq!(prepared.branch.as_deref(), Some("advertised"));
        assert_eq!(
            std::fs::read(files.root().root_path().join("code")).unwrap(),
            b"from source\n"
        );
        assert!(files.root().root_path().join(".git").is_dir());
        assert!(
            !files
                .root()
                .root_path()
                .join(format!(".orch-tmp-source-{}", request.job_id))
                .exists()
        );
    }

    fn ask(credentials: &Credentials, input: &str) -> Vec<u8> {
        let mut child = std::process::Command::new("/bin/sh")
            .arg(credentials.directory.path().join("helper"))
            .arg("get")
            .env_clear()
            .env("ORCH_SOURCE_CREDENTIAL_DIR", credentials.directory.path())
            .env("ORCH_SOURCE_CREDENTIAL_USERNAME", &credentials.username)
            .env("ORCH_SOURCE_CREDENTIAL_PASSWORD", &credentials.password)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        output.stdout
    }

    #[test]
    fn a_command_credential_answers_only_for_its_exact_https_repository() {
        let directory = tempfile::tempdir().unwrap();
        let credentials = Credentials::for_repository(
            directory.path(),
            "https://example.test/team/a%20b.git",
            r#"{"username":"fixture-user","password":"fixture-token"}"#,
        )
        .unwrap();
        assert_eq!(
            ask(
                &credentials,
                "protocol=https\nhost=example.test\npath=team/a b.git\n\n"
            ),
            b"username=fixture-user\npassword=fixture-token\n"
        );
        assert_eq!(
            ask(
                &credentials,
                "protocol=https\nhost=example.test\npath=team/a b.git/info/refs\n\n"
            ),
            b"username=fixture-user\npassword=fixture-token\n"
        );
        assert_eq!(
            ask(
                &credentials,
                "protocol=https\nhost=example.test:443\npath=team/a b.git\n\n"
            ),
            b"username=fixture-user\npassword=fixture-token\n"
        );
        for input in [
            "protocol=http\nhost=example.test\npath=team/a b.git\n\n",
            "protocol=https\nhost=other.test\npath=team/a b.git\n\n",
            "protocol=https\nhost=example.test:444\npath=team/a b.git\n\n",
            "protocol=https\nhost=example.test\npath=team/other.git\n\n",
            "protocol=https\nhost=example.test\npath=team/a b.git-sibling\n\n",
            "protocol=https\nhost=example.test\npath=team/a b.git/../other.git\n\n",
        ] {
            // Canonical Git paths contain no traversal; helper scope must also refuse a
            // forged credential-protocol request rather than depending on that convention.
            assert!(ask(&credentials, input).is_empty(), "{input}");
        }
        let secret_path = credentials.directory.path().to_owned();
        assert!(!credentials.helper.contains("fixture-token"));
        for entry in std::fs::read_dir(&secret_path).unwrap() {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap();
            assert!(
                !String::from_utf8_lossy(&bytes).contains("fixture-token"),
                "even an interrupted command leaves no credential value on disk"
            );
        }
        drop(credentials);
        assert!(
            !secret_path.exists(),
            "the token is not retained after the command"
        );
    }

    #[test]
    fn credential_shapes_urls_and_control_characters_are_refused_without_values_in_errors() {
        let directory = tempfile::tempdir().unwrap();
        for url in [
            "http://example.test/repo.git",
            "ssh://git@example.test/repo.git",
            "https://person@example.test/repo.git",
            "https://example.test/repo.git?secret=value",
            "https://example.test/repo.git#ref",
            "https://example.test/",
            "https://example.test/a%0Ab.git",
        ] {
            assert!(
                Credentials::for_repository(
                    directory.path(),
                    url,
                    r#"{"username":"u","password":"fixture-token"}"#
                )
                .is_err(),
                "{url}"
            );
        }
        for value in [
            r#"{"username":"u","password":"fixture-token","unknown":true}"#,
            r#"{"username":"u","password":"fixture-token\n"}"#,
            r#"{"username":"","password":"fixture-token"}"#,
        ] {
            let error = Credentials::for_repository(
                directory.path(),
                "https://example.test/repo.git",
                value,
            )
            .err()
            .unwrap();
            assert!(!error.to_string().contains("fixture-token"));
        }
    }
}
