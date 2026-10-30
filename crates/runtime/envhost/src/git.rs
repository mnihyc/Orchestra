//! Read-only Git status and diffs.
//!
//! Both views run the `git` installed on the machine with a fixed command line, in a selected
//! repository inside the workspace,
//! and read its output; they change nothing. The command line is chosen so that looking at a
//! repository cannot run anything the repository brings and cannot write to it: no optional locks
//! (so the index is not refreshed and rewritten), no file-system monitor hook, no external diff
//! program, no text conversion filters, no pager, no prompt. A path that is asked for is a path of
//! the workspace like any other and reaches `git` only after `--`, as a literal path.
//!
//! The output is bounded in bytes and the run in time; a difference that is longer than the bound
//! is cut and says so.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;

use orch_api_contract::workspace::{GitChange, GitStatus, GitUnavailable};
use orch_link_contract::commands::GitDiffResult;
use orch_types::Hash;
use tokio::io::AsyncReadExt;

use crate::error::{HostError, Result};
use crate::files::Files;
use crate::paths::{Locate, RelPath};
use crate::platform::{self, Sig};

/// The most bytes of the answer to a status that are read. A repository that lists more than this
/// is not one a person reads in a panel; the rest is not read and `git` is ended.
const STATUS_MAX_BYTES: usize = 8 * 1024 * 1024;

/// The most changes a status lists.
const STATUS_MAX_CHANGES: usize = 20_000;

/// How much of what `git` writes to its error output is kept: enough for a reason, never a flood.
const ERROR_TEXT_KEEP_BYTES: usize = 64 * 1024;

/// How many bytes one read of the output of `git` asks for.
const READ_BUFFER_BYTES: usize = 64 * 1024;

/// How many space-separated fields the records of the status end in, counting the path: an
/// ordinary change has nine, a renamed or copied one ten, an unmerged one eleven.
const ORDINARY_FIELDS: usize = 9;
pub(crate) const RENAMED_FIELDS: usize = 10;
const UNMERGED_FIELDS: usize = 11;

/// The language a run of `git` is made in, so that what it prints can be told from what it says.
const GIT_LANGUAGE: &str = "C";

/// The object that is the empty tree in every repository that hashes with SHA-1: attributes read
/// from it are none, which is how the attributes of the work tree are kept from being read.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// The first version of `git` that can read its attributes from a tree.
const ATTR_SOURCE_SINCE: (u32, u32) = (2, 43);

/// Where `git` is on this machine.
#[derive(Debug, Clone)]
pub struct GitTool {
    program: Option<PathBuf>,
    attr_source: bool,
}

/// The version a `git --version` says: "git version 2.47.3" is (2, 47).
fn parse_version(text: &str) -> Option<(u32, u32)> {
    let number = text.split_whitespace().nth(2)?;
    let mut parts = number.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

impl GitTool {
    /// The `git` that `path` (a list of directories, as `PATH` is written) holds.
    #[must_use]
    pub fn find_in(path: &OsString) -> Self {
        let name = if cfg!(windows) { "git.exe" } else { "git" };
        let program = std::env::split_paths(path)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file());
        let attr_source = program
            .as_ref()
            .and_then(|program| {
                std::process::Command::new(program)
                    .arg("--version")
                    .env_clear()
                    .output()
                    .ok()
            })
            .and_then(|output| parse_version(&String::from_utf8_lossy(&output.stdout)))
            .is_some_and(|version| version >= ATTR_SOURCE_SINCE);
        Self {
            program,
            attr_source,
        }
    }

    /// The `git` that this process's own `PATH` finds.
    #[must_use]
    pub fn discover() -> Self {
        Self::find_in(&std::env::var_os("PATH").unwrap_or_default())
    }

    /// Whether there is a `git`.
    #[must_use]
    pub fn available(&self) -> bool {
        self.program.is_some()
    }

    /// The controlled installed executable, shared with explicit source/Git actions.
    pub(crate) fn program(&self) -> Result<&std::path::Path> {
        self.program
            .as_deref()
            .ok_or_else(|| HostError::Unsupported("this machine has no git".to_owned()))
    }

    /// Whether Git can keep working-tree attributes out of a data-only operation.
    pub(crate) const fn has_attribute_source(&self) -> bool {
        self.attr_source
    }
}

/// What a run of `git` produced.
struct Run {
    status: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
    cut: bool,
}

/// The settings that keep `git` from running a filter the repository names: every driver the
/// configuration declares gets empty commands. The attributes that would pick a driver for a file
/// are read from a repository's own files, which a repository chooses, so the drivers are what is
/// turned off.
pub(crate) async fn filter_overrides(
    program: &std::path::Path,
    cwd: &std::path::Path,
    files: &Files,
) -> Result<Vec<String>> {
    let mut child = tokio::process::Command::new(program)
        .args([
            "config",
            "-z",
            "--name-only",
            "--get-regexp",
            r"^filter\..*\.(clean|smudge|process)$",
        ])
        .current_dir(cwd)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("HOME", std::env::var_os("HOME").unwrap_or_default())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| HostError::io("reading git filter configuration", error))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| HostError::Internal("git has no output".to_owned()))?;
    let read = async {
        let mut output = Vec::new();
        (&mut stdout)
            .take(STATUS_MAX_BYTES as u64 + 1)
            .read_to_end(&mut output)
            .await
            .map_err(|error| HostError::io("reading git filter configuration", error))?;
        if output.len() > STATUS_MAX_BYTES {
            return Err(HostError::Unsupported(
                "git filter configuration is too large to read safely".to_owned(),
            ));
        }
        child
            .wait()
            .await
            .map_err(|error| HostError::io("reading git filter configuration", error))?;
        Ok(output)
    };
    let output = tokio::time::timeout(files.limits().git_time, read)
        .await
        .map_err(|_| {
            HostError::Timeout("git filter configuration took longer than it may".to_owned())
        })??;
    Ok(String::from_utf8_lossy(&output)
        .split('\0')
        .filter(|name| name.starts_with("filter."))
        .flat_map(|name| [String::from("-c"), format!("{name}=")])
        .collect())
}

async fn run(
    tool: &GitTool,
    files: &Files,
    cwd: &std::path::Path,
    arguments: &[&str],
    literal: &[OsString],
    max_bytes: usize,
) -> Result<Run> {
    let program = tool
        .program
        .as_ref()
        .ok_or_else(|| HostError::Unsupported("this machine has no git".to_owned()))?;
    let root = files.root().root_path();
    let ceiling = root
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let filters = filter_overrides(program, cwd, files).await?;
    let mut command = tokio::process::Command::new(program);
    command
        .arg("--no-optional-locks")
        .arg("--literal-pathspecs");
    if tool.attr_source {
        command.arg(format!("--attr-source={EMPTY_TREE}"));
    }
    command
        .args(["-c", "core.fsmonitor=false"])
        .args(["-c", "core.pager=cat"])
        .args(["-c", "core.quotepath=false"])
        .args(["-c", "color.ui=never"])
        .args(["-c", "core.attributesFile="])
        .args(&filters)
        .args(arguments);
    if !matches!(arguments.first().copied(), Some("config" | "rev-parse")) {
        command.arg("--");
    }
    command
        .args(literal)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("LC_ALL", GIT_LANGUAGE)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CEILING_DIRECTORIES", ceiling)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(home) = std::env::var_os("HOME") {
        command.env("HOME", home);
    }
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| HostError::io("starting git", error))?;
    let pid = child.id();
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| HostError::Internal("git has no output".to_owned()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| HostError::Internal("git has no error output".to_owned()))?;

    let read_stdout = async {
        let mut out = Vec::new();
        let mut buffer = vec![0u8; READ_BUFFER_BYTES];
        let mut cut = false;
        loop {
            let count = stdout
                .read(&mut buffer)
                .await
                .map_err(|error| HostError::io("reading the output of git", error))?;
            if count == 0 {
                break;
            }
            out.extend_from_slice(&buffer[..count]);
            if out.len() > max_bytes {
                out.truncate(max_bytes);
                cut = true;
                kill(pid);
                break;
            }
        }
        Ok::<_, HostError>((out, cut))
    };
    let read_stderr = async {
        let mut error_text = Vec::new();
        let _ = (&mut stderr)
            .take(ERROR_TEXT_KEEP_BYTES as u64)
            .read_to_end(&mut error_text)
            .await;
        let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
        String::from_utf8_lossy(&error_text).into_owned()
    };
    let ((out, cut), error_text) = match tokio::time::timeout(files.limits().git_time, async {
        let (out, error_text) = tokio::join!(read_stdout, read_stderr);
        Ok::<_, HostError>((out?, error_text))
    })
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            kill(pid);
            return Err(HostError::Timeout("git took longer than it may".to_owned()));
        }
    };
    if cut {
        // The rest is not wanted; end git and what it started.
        kill(pid);
    }
    let status = match tokio::time::timeout(files.limits().git_time, child.wait()).await {
        Ok(Ok(status)) => status.code(),
        _ => {
            kill(pid);
            None
        }
    };
    Ok(Run {
        status,
        stdout: out,
        stderr: error_text,
        cut,
    })
}

fn kill(pid: Option<u32>) {
    if let Some(pid) = pid {
        let _ = platform::send_group(pid, Sig::Kill);
    }
}

fn unavailable(reason: GitUnavailable) -> GitStatus {
    GitStatus {
        available: false,
        reason: Some(reason),
        repository_dir: None,
        status_hash: None,
        remotes: Vec::new(),
        upstream: None,
        branch: None,
        head: None,
        ahead: None,
        behind: None,
        changes: Vec::new(),
    }
}

/// A letter of `git status --porcelain`: version 2 writes `.` for nothing, version 1 a space.
fn letter(code: char) -> String {
    if code == '.' {
        " ".to_owned()
    } else {
        code.to_string()
    }
}

/// Reads the answer of `git status --porcelain=v2 --branch -z`.
fn parse_status(output: &[u8]) -> GitStatus {
    let text = String::from_utf8_lossy(output);
    let mut status = GitStatus {
        available: true,
        reason: None,
        repository_dir: None,
        status_hash: None,
        remotes: Vec::new(),
        upstream: None,
        branch: None,
        head: None,
        ahead: None,
        behind: None,
        changes: Vec::new(),
    };
    let mut parts = text.split('\0');
    while let Some(entry) = parts.next() {
        if entry.is_empty() {
            continue;
        }
        if let Some(header) = entry.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => status.head = Some(value.to_owned()),
                "branch.head" if value != "(detached)" => status.branch = Some(value.to_owned()),
                "branch.upstream" => status.upstream = Some(value.to_owned()),
                "branch.ab" => {
                    for word in value.split_whitespace() {
                        if let Some(ahead) = word.strip_prefix('+') {
                            status.ahead = ahead.parse().ok();
                        } else if let Some(behind) = word.strip_prefix('-') {
                            status.behind = behind.parse().ok();
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        if status.changes.len() >= STATUS_MAX_CHANGES {
            continue;
        }
        let mut kind = entry.chars();
        match kind.next() {
            Some('1') => {
                let fields: Vec<&str> = entry.splitn(ORDINARY_FIELDS, ' ').collect();
                if let (Some(codes), Some(path)) = (fields.get(1), fields.get(ORDINARY_FIELDS - 1))
                {
                    let mut letters = codes.chars();
                    status.changes.push(GitChange {
                        path: (*path).to_owned(),
                        index: letter(letters.next().unwrap_or('.')),
                        worktree: letter(letters.next().unwrap_or('.')),
                    });
                }
            }
            Some('2') => {
                let fields: Vec<&str> = entry.splitn(RENAMED_FIELDS, ' ').collect();
                // The name it had before follows as a part of its own.
                let _original = parts.next();
                if let (Some(codes), Some(path)) = (fields.get(1), fields.get(RENAMED_FIELDS - 1)) {
                    let mut letters = codes.chars();
                    status.changes.push(GitChange {
                        path: (*path).to_owned(),
                        index: letter(letters.next().unwrap_or('.')),
                        worktree: letter(letters.next().unwrap_or('.')),
                    });
                }
            }
            Some('u') => {
                let fields: Vec<&str> = entry.splitn(UNMERGED_FIELDS, ' ').collect();
                if let (Some(codes), Some(path)) = (fields.get(1), fields.get(UNMERGED_FIELDS - 1))
                {
                    let mut letters = codes.chars();
                    status.changes.push(GitChange {
                        path: (*path).to_owned(),
                        index: letter(letters.next().unwrap_or('U')),
                        worktree: letter(letters.next().unwrap_or('U')),
                    });
                }
            }
            Some('?') => {
                if let Some(path) = entry.strip_prefix("? ") {
                    status.changes.push(GitChange {
                        path: path.to_owned(),
                        index: "?".to_owned(),
                        worktree: "?".to_owned(),
                    });
                }
            }
            _ => {}
        }
    }
    status
}

impl Files {
    /// The state of the repository at the workspace root: its branch, its commit, how far it is
    /// ahead of and behind its upstream, and the paths that differ. A directory in no repository,
    /// and a machine with no `git`, are answers and not failures.
    pub async fn git_status(&self, tool: &GitTool) -> Result<GitStatus> {
        self.git_status_at(tool, None).await
    }

    /// Reads the repository at `directory`, refusing repository metadata outside the workspace.
    pub async fn git_status_at(
        &self,
        tool: &GitTool,
        directory: Option<&RelPath>,
    ) -> Result<GitStatus> {
        tokio::time::timeout(
            self.limits().git_time,
            self.git_status_inner(tool, directory),
        )
        .await
        .map_err(|_| {
            HostError::Timeout(
                "git status is too slow here; use the environment's shell".to_owned(),
            )
        })?
    }

    async fn git_status_inner(
        &self,
        tool: &GitTool,
        directory: Option<&RelPath>,
    ) -> Result<GitStatus> {
        if !tool.available() {
            return Ok(unavailable(GitUnavailable::GitMissing));
        }
        let asked = directory.cloned().unwrap_or_else(RelPath::root);
        let located = self.root().locate(&asked, Locate::THROUGH)?;
        let meta = located
            .meta
            .ok_or_else(|| HostError::NotFound(format!("{asked} is not there")))?;
        self.check_visible(&asked, &located, meta.is_dir())?;
        if !meta.is_dir() {
            return Err(HostError::NotFound(format!(
                "{asked} is not a repository directory"
            )));
        }
        let cwd = located.real.under(self.root().root_path());
        let Some((cwd, repository)) = self.git_repository(tool, &cwd).await? else {
            return Ok(unavailable(GitUnavailable::NotARepository));
        };
        let output = run(
            tool,
            self,
            &cwd,
            &[
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=all",
                "--no-renames",
            ],
            &[],
            STATUS_MAX_BYTES,
        )
        .await?;
        match output.status {
            Some(0) if !output.cut => {
                let mut status = parse_status(&output.stdout);
                status.repository_dir = Some(repository.to_string());
                if status.changes.len() >= STATUS_MAX_CHANGES {
                    return Err(HostError::Unsupported(
                        "git status has too many changes; use the environment's shell".to_owned(),
                    ));
                }
                status.changes.retain(|change| {
                    self.parse(&change.path)
                        .and_then(|path| {
                            self.parse(&if repository.is_root() {
                                path.to_string()
                            } else {
                                format!("{repository}/{path}")
                            })
                        })
                        .and_then(|path| {
                            let located = self.root().locate(&path, Locate::ENTRY)?;
                            self.check_visible(
                                &path,
                                &located,
                                located.meta.is_some_and(|meta| meta.is_dir()),
                            )
                        })
                        .is_ok()
                });
                status.remotes = self.git_remotes(tool, &cwd).await?;
                status.status_hash = Some(
                    self.git_status_hash(tool, &cwd, &repository, &status)
                        .await?,
                );
                // The reviewed hash still covers the full repository so selected-path commit
                // rechecks the same HEAD/index/work state. Chips count only this asked directory.
                if asked != repository {
                    let relative = asked.strip_prefix(&repository).ok_or_else(|| {
                        HostError::Changed(
                            "the requested directory left the discovered repository".to_owned(),
                        )
                    })?;
                    status.changes.retain(|change| {
                        self.parse(&change.path)
                            .is_ok_and(|path| path.starts_with(&relative))
                    });
                }
                Ok(status)
            }
            _ if output.cut => Err(HostError::Unsupported(
                "git status is too large; use the environment's shell".to_owned(),
            )),
            _ if output.stderr.contains("not a git repository")
                || output.stderr.contains("dubious ownership") =>
            {
                Ok(unavailable(GitUnavailable::NotARepository))
            }
            _ => Err(HostError::Internal(format!(
                "git failed: {}",
                output.stderr.trim()
            ))),
        }
    }

    async fn git_remotes(
        &self,
        tool: &GitTool,
        cwd: &std::path::Path,
    ) -> Result<Vec<orch_api_contract::workspace::GitStatusRemote>> {
        let output = run(
            tool,
            self,
            cwd,
            &["config", "--null", "--get-regexp", r"^remote\..*\.url$"],
            &[],
            STATUS_MAX_BYTES,
        )
        .await?;
        if output.cut {
            return Err(HostError::Unsupported(
                "git remote configuration is too large".to_owned(),
            ));
        }
        if !matches!(output.status, Some(0 | 1)) {
            return Err(HostError::Unsupported(
                "git remote configuration cannot be read".to_owned(),
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut remotes = Vec::new();
        for record in text.split('\0') {
            let Some((key, url)) = record.split_once('\n') else {
                continue;
            };
            let Some(name) = key
                .strip_prefix("remote.")
                .and_then(|key| key.strip_suffix(".url"))
            else {
                continue;
            };
            if let Some(url) = safe_remote_url(url) {
                remotes.push(orch_api_contract::workspace::GitStatusRemote {
                    name: name.to_owned(),
                    url,
                });
            }
        }
        remotes.sort_by(|left, right| (&left.name, &left.url).cmp(&(&right.name, &right.url)));
        Ok(remotes)
    }

    async fn git_status_hash(
        &self,
        tool: &GitTool,
        cwd: &std::path::Path,
        repository: &RelPath,
        status: &GitStatus,
    ) -> Result<Hash> {
        let output = run(
            tool,
            self,
            cwd,
            &["ls-files", "--stage", "-z"],
            &[],
            STATUS_MAX_BYTES,
        )
        .await?;
        if output.status != Some(0) || output.cut {
            return Err(HostError::Unsupported(
                "the Git index is too large or unavailable for a reviewed status".to_owned(),
            ));
        }
        let text = std::str::from_utf8(&output.stdout)
            .map_err(|_| HostError::Unsupported("the Git index has a non-UTF-8 path".to_owned()))?;
        let mut index: Vec<String> = text
            .split('\0')
            .filter(|record| !record.is_empty())
            .map(str::to_owned)
            .collect();
        index.sort();
        let this = self.clone();
        let repository = repository.clone();
        let changes = status.changes.clone();
        let head = status.head.clone();
        let branch = status.branch.clone();
        crate::error::blocking(move || {
            let started = std::time::Instant::now();
            let mut working = std::collections::BTreeMap::new();
            let mut buffer = vec![0u8; READ_BUFFER_BYTES];
            for change in changes {
                let path = this.parse(&if repository.is_root() { change.path.clone() } else { format!("{repository}/{}", change.path) })?;
                let located = this.root().locate(&path, Locate::ENTRY)?;
                let value = match located.meta {
                    None => serde_json::json!({ "absent": true }),
                    Some(meta) if meta.is_symlink() => {
                        let link = located.parent.read_link(located.name()?).map_err(|error| HostError::io("reading a Git link", error))?;
                        serde_json::json!({ "link": link.to_string_lossy(), "mode": meta.mode })
                    }
                    Some(meta) if meta.is_file() => {
                        let mut file = located.parent.open_read(located.name()?).map_err(|error| HostError::io("reading changed Git bytes", error))?;
                        let before = platform::file_meta(&file).map_err(|error| HostError::io("checking changed Git bytes", error))?;
                        if before.identity() != meta.identity() || before.mode != meta.mode {
                            return Err(HostError::Changed("a Git file changed before its status was read".to_owned()));
                        }
                        let mut hash = Hash::hasher();
                        loop {
                            if started.elapsed() >= this.limits().git_time { return Err(HostError::Timeout("changed Git files take too long to review; use the environment's shell".to_owned())); }
                            let count = std::io::Read::read(&mut file, &mut buffer).map_err(|error| HostError::io("hashing changed Git bytes", error))?;
                            if count == 0 { break; }
                            hash.update(&buffer[..count]);
                        }
                        let after = platform::file_meta(&file).map_err(|error| HostError::io("checking changed Git bytes", error))?;
                        if after.identity() != before.identity() || after.mode != before.mode {
                            return Err(HostError::Changed("a Git file changed while its status was read".to_owned()));
                        }
                        serde_json::json!({ "hash": Hash::finish(&hash), "mode": meta.mode })
                    }
                    Some(meta) => serde_json::json!({ "directory": meta.is_dir(), "mode": meta.mode }),
                };
                working.insert(change.path, value);
            }
            let record = serde_json::to_vec(&serde_json::json!({ "head": head, "branch": branch, "index": index, "working": working }))
                .map_err(|error| HostError::Internal(error.to_string()))?;
            Ok(Hash::of(&record))
        }).await
    }

    pub(crate) async fn git_repository(
        &self,
        tool: &GitTool,
        cwd: &std::path::Path,
    ) -> Result<Option<(PathBuf, RelPath)>> {
        let output = run(
            tool,
            self,
            cwd,
            &[
                "rev-parse",
                "--show-toplevel",
                "--absolute-git-dir",
                "--git-common-dir",
            ],
            &[],
            STATUS_MAX_BYTES,
        )
        .await?;
        if output.status != Some(0) || output.cut {
            if output.stderr.contains("not a git repository")
                || output.stderr.contains("dubious ownership")
            {
                return Ok(None);
            }
            return Err(HostError::Unsupported(
                "the repository directory cannot be read safely".to_owned(),
            ));
        }
        let text = std::str::from_utf8(&output.stdout).map_err(|_| {
            HostError::Unsupported("the repository directory is not UTF-8".to_owned())
        })?;
        let mut paths = text.lines();
        let top = paths
            .next()
            .ok_or_else(|| HostError::Internal("git named no repository directory".to_owned()))?;
        let top = std::fs::canonicalize(cwd.join(top))
            .map_err(|error| HostError::io("locating the repository", error))?;
        for _ in 0..2 {
            let path = paths.next().ok_or_else(|| {
                HostError::Internal("git named no repository metadata".to_owned())
            })?;
            let path = std::fs::canonicalize(cwd.join(path))
                .map_err(|error| HostError::io("locating repository metadata", error))?;
            if !path.starts_with(self.root().root_path()) {
                return Err(HostError::NotFound(
                    "repository metadata leaves the workspace".to_owned(),
                ));
            }
        }
        let relative = top
            .strip_prefix(self.root().root_path())
            .map_err(|_| HostError::NotFound("the repository leaves the workspace".to_owned()))?;
        let repository = self.parse(
            &relative
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/"),
        )?;
        let located = self.root().locate(&repository, Locate::THROUGH)?;
        self.check_visible(&repository, &located, true)?;
        Ok(Some((top, repository)))
    }

    /// The unified difference of the repository: of the working tree against the index, or with
    /// `staged` of the index against the last commit, kept to `path` when one is named. Longer than
    /// the bound it is cut there and says so.
    pub async fn git_diff(
        &self,
        tool: &GitTool,
        path: Option<&RelPath>,
        staged: bool,
    ) -> Result<GitDiffResult> {
        tokio::time::timeout(
            self.limits().git_time,
            self.git_diff_inner(tool, path, staged),
        )
        .await
        .map_err(|_| {
            HostError::Timeout(
                "git difference is too slow here; use the environment's shell".to_owned(),
            )
        })?
    }

    async fn git_diff_inner(
        &self,
        tool: &GitTool,
        path: Option<&RelPath>,
        staged: bool,
    ) -> Result<GitDiffResult> {
        if !tool.available() {
            return Err(HostError::Unsupported("this machine has no git".to_owned()));
        }
        // Git receives only visible changed paths, including a deleted file that status still
        // names. Running from the repository directory keeps every returned path unambiguous.
        let (cwd, scope) = match path {
            None => (self.root().root_path().to_path_buf(), RelPath::root()),
            Some(asked) => {
                let located = self.root().locate(asked, Locate::ENTRY)?;
                let is_dir = located.meta.is_some_and(|meta| meta.is_dir());
                self.check_visible(asked, &located, is_dir)?;
                let base = self.root().root_path();
                if is_dir {
                    (located.real.under(base), located.real)
                } else {
                    let parent = located.real.parent().unwrap_or_else(RelPath::root);
                    (parent.under(base), located.real)
                }
            }
        };
        let Some((cwd, repository)) = self.git_repository(tool, &cwd).await? else {
            return Err(HostError::NotFound(
                "there is no repository in that directory".to_owned(),
            ));
        };
        let status = self.git_status_at(tool, Some(&repository)).await?;
        let mut literal = Vec::new();
        for change in status.changes {
            let relative = self.parse(&change.path)?;
            let full = if repository.is_root() {
                relative.clone()
            } else {
                self.parse(&format!("{repository}/{relative}"))?
            };
            if full.starts_with(&scope) {
                literal.push(OsString::from(relative.as_str()));
            }
        }
        if literal.is_empty() {
            if let Some(path) = path
                && self.root().locate(path, Locate::ENTRY)?.meta.is_none()
            {
                return Err(HostError::NotFound(format!(
                    "{path} is not a changed tracked path"
                )));
            }
            return Ok(GitDiffResult {
                text: String::new(),
                truncated: false,
            });
        }
        let limit = self.limits().git_diff_max_bytes;
        let mut arguments = vec![
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
        ];
        if staged {
            arguments.push("--cached");
        }
        let output = run(tool, self, &cwd, &arguments, &literal, limit).await?;
        match output.status {
            Some(0) | Some(1) => {}
            _ if output.cut => {}
            _ => {
                return Err(HostError::Internal(format!(
                    "git failed: {}",
                    output.stderr.trim()
                )));
            }
        }
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        let truncated = output.cut || text.len() > limit;
        if text.len() > limit {
            let mut end = limit;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        Ok(GitDiffResult { text, truncated })
    }
}

/// Removes embedded authentication and query material from a locally configured remote URL.
#[must_use]
pub fn safe_remote_url(raw: &str) -> Option<String> {
    if raw.chars().any(char::is_control) {
        return None;
    }
    if let Ok(mut url) = reqwest::Url::parse(raw) {
        if !matches!(url.scheme(), "http" | "https" | "ssh" | "git") {
            return None;
        }
        if matches!(url.scheme(), "http" | "https") {
            url.set_username("").ok()?;
        }
        url.set_password(None).ok()?;
        url.set_query(None);
        url.set_fragment(None);
        return Some(url.to_string());
    }
    // scp-style SSH addresses do not contain a password. Local filesystem remotes are kept
    // local, but names with authentication-like syntax that is not an SSH address are hidden.
    let prefix = raw.split_once(':').map(|(host, _)| host).unwrap_or(raw);
    if prefix.contains('@') && prefix.contains('/') {
        return None;
    }
    if raw
        .split_once('@')
        .is_some_and(|(user, _)| user.contains(':'))
    {
        return None;
    }
    Some(raw.to_owned())
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::environment::{GateState, Gates, Limits};
    use crate::ignore::IgnoreRules;
    use crate::index::HashCache;
    use crate::paths::{PathStyle, Workspace};
    use std::sync::Arc;

    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn files_in(root: &std::path::Path, limits: Limits) -> Files {
        let ws = Workspace::open(root, &[], PathStyle::Posix, true).unwrap();
        Files::workspace(
            ws,
            Arc::new(IgnoreRules::new(&[], true)),
            Arc::new(HashCache::new()),
            limits,
            Arc::new(GateState::new(Gates::OPEN)),
        )
    }

    fn path(text: &str) -> RelPath {
        RelPath::parse(text, PathStyle::Posix).unwrap()
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("README.md"), "# hello\n").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        dir
    }

    #[tokio::test]
    async fn two_nested_repositories_return_their_own_changes_and_bounded_differences() {
        let workspace = tempfile::tempdir().unwrap();
        for name in ["first", "second"] {
            let root = workspace.path().join(name);
            std::fs::create_dir(&root).unwrap();
            git(&root, &["init", "-q"]);
            std::fs::write(root.join("code.txt"), "original\n").unwrap();
            git(&root, &["add", "code.txt"]);
            git(&root, &["commit", "-q", "-m", "first"]);
            std::fs::write(root.join("code.txt"), format!("{name}\n").repeat(1000)).unwrap();
        }
        let files = files_in(
            workspace.path(),
            Limits {
                git_diff_max_bytes: 1000,
                ..Limits::default()
            },
        );
        let tool = GitTool::discover();
        assert_eq!(
            files.git_status(&tool).await.unwrap().reason,
            Some(GitUnavailable::NotARepository)
        );
        for name in ["first", "second"] {
            let status = files.git_status_at(&tool, Some(&path(name))).await.unwrap();
            assert!(status.available);
            assert_eq!(status.repository_dir.as_deref(), Some(name));
            assert_eq!(status.changes[0].path, "code.txt");
            let diff = files
                .git_diff(&tool, Some(&path(&format!("{name}/code.txt"))), false)
                .await
                .unwrap();
            assert!(diff.text.contains(&format!("+{name}")), "{}", diff.text);
            assert!(
                !diff
                    .text
                    .contains(if name == "first" { "+second" } else { "+first" })
            );
            assert!(diff.truncated && diff.text.len() <= 1000);
        }
    }

    #[tokio::test]
    async fn an_ignored_changed_file_is_absent_from_status_and_difference() {
        let dir = repo();
        std::fs::write(dir.path().join("secret.txt"), "before\n").unwrap();
        git(dir.path(), &["add", "secret.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "tracked secret"]);
        std::fs::write(dir.path().join("secret.txt"), "HIDDEN_SECRET\n").unwrap();
        std::fs::write(dir.path().join("README.md"), "visible\n").unwrap();
        let ws = Workspace::open(dir.path(), &[], PathStyle::Posix, true).unwrap();
        let files = Files::workspace(
            ws,
            Arc::new(IgnoreRules::new(&["secret.txt".to_owned()], true)),
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        );
        let tool = GitTool::discover();
        assert!(
            files
                .git_status(&tool)
                .await
                .unwrap()
                .changes
                .iter()
                .all(|change| change.path != "secret.txt")
        );
        let diff = files.git_diff(&tool, None, false).await.unwrap();
        assert!(diff.text.contains("+visible"));
        assert!(!diff.text.contains("HIDDEN_SECRET") && !diff.text.contains("secret.txt"));
    }

    #[tokio::test]
    async fn a_worktree_whose_shared_repository_leaves_the_workspace_is_refused() {
        let repo = repo();
        let destination = tempfile::tempdir().unwrap();
        let worktree = destination.path().join("attempt");
        git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "attempt",
                worktree.to_str().unwrap(),
            ],
        );
        let files = files_in(destination.path(), Limits::default());
        assert!(
            files
                .git_status_at(&GitTool::discover(), Some(&path("attempt")))
                .await
                .is_err()
        );
        assert!(
            files
                .git_diff(&GitTool::discover(), Some(&path("attempt")), false)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn the_reviewed_status_detects_working_and_staged_changes_without_rewriting_the_index() {
        let dir = repo();
        let files = files_in(dir.path(), Limits::default());
        let tool = GitTool::discover();
        let first = files.git_status(&tool).await.unwrap().status_hash.unwrap();
        let index = std::fs::read(dir.path().join(".git/index")).unwrap();
        assert_eq!(
            files.git_status(&tool).await.unwrap().status_hash,
            Some(first)
        );
        assert_eq!(std::fs::read(dir.path().join(".git/index")).unwrap(), index);
        std::fs::write(dir.path().join("README.md"), "changed\n").unwrap();
        let unstaged = files.git_status(&tool).await.unwrap().status_hash.unwrap();
        assert_ne!(first, unstaged);
        git(dir.path(), &["add", "README.md"]);
        let staged = files.git_status(&tool).await.unwrap().status_hash.unwrap();
        assert_ne!(unstaged, staged);
        git(
            dir.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://token:password@example.invalid/team/repo.git?secret=hidden",
            ],
        );
        let status = files.git_status(&tool).await.unwrap();
        assert_eq!(
            status.remotes[0].url,
            "https://example.invalid/team/repo.git"
        );
        let shown = serde_json::to_string(&status).unwrap();
        assert!(
            !shown.contains("password") && !shown.contains("token") && !shown.contains("hidden")
        );
    }

    #[tokio::test]
    async fn status_names_the_branch_the_commit_and_every_path_that_differs() {
        let dir = repo();
        std::fs::write(
            dir.path().join("src/main.rs"),
            "fn main() { println!(); }\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("new file.txt"), "untracked\n").unwrap();
        std::fs::write(dir.path().join("README.md"), "# changed\n").unwrap();
        git(dir.path(), &["add", "README.md"]);
        std::fs::write(dir.path().join("README.md"), "# changed again\n").unwrap();
        let files = files_in(dir.path(), Limits::default());
        let status = files.git_status(&GitTool::discover()).await.unwrap();
        assert!(status.available && status.reason.is_none());
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(
            status.head.as_deref(),
            Some(git(dir.path(), &["rev-parse", "HEAD"]).trim())
        );
        assert_eq!((status.ahead, status.behind), (None, None), "no upstream");
        let by_path: std::collections::BTreeMap<&str, (&str, &str)> = status
            .changes
            .iter()
            .map(|c| (c.path.as_str(), (c.index.as_str(), c.worktree.as_str())))
            .collect();
        assert_eq!(by_path["src/main.rs"], (" ", "M"));
        assert_eq!(
            by_path["README.md"],
            ("M", "M"),
            "changed in the index and again in the work tree"
        );
        assert_eq!(
            by_path["new file.txt"],
            ("?", "?"),
            "a name with a space survives"
        );
    }

    #[tokio::test]
    async fn status_says_so_when_there_is_no_repository_or_no_git() {
        let dir = tempfile::tempdir().unwrap();
        let files = files_in(dir.path(), Limits::default());
        let none = files.git_status(&GitTool::discover()).await.unwrap();
        assert!(!none.available);
        assert_eq!(none.reason, Some(GitUnavailable::NotARepository));
        let missing = GitTool::find_in(&OsString::from("/nonexistent-directory"));
        assert!(!missing.available());
        let gone = files.git_status(&missing).await.unwrap();
        assert_eq!(gone.reason, Some(GitUnavailable::GitMissing));
        assert!(files.git_diff(&missing, None, false).await.is_err());
    }

    #[tokio::test]
    async fn a_repository_above_the_workspace_is_not_read() {
        let outer = repo();
        let inner = outer.path().join("workspace");
        std::fs::create_dir(&inner).unwrap();
        let files = files_in(&inner, Limits::default());
        let status = files.git_status(&GitTool::discover()).await.unwrap();
        assert_eq!(
            status.reason,
            Some(GitUnavailable::NotARepository),
            "git looks for a repository in the workspace and not above it"
        );
    }

    #[tokio::test]
    async fn the_difference_is_the_unified_one_kept_to_a_path_and_to_what_is_staged() {
        let dir = repo();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() { 1; }\n").unwrap();
        std::fs::write(dir.path().join("README.md"), "# staged\n").unwrap();
        git(dir.path(), &["add", "README.md"]);
        let files = files_in(dir.path(), Limits::default());
        let tool = GitTool::discover();
        let all = files.git_diff(&tool, None, false).await.unwrap();
        assert!(
            all.text.contains("+fn main() { 1; }") && !all.text.contains("staged"),
            "{}",
            all.text
        );
        assert!(!all.truncated);
        let staged = files.git_diff(&tool, None, true).await.unwrap();
        assert!(
            staged.text.contains("+# staged") && !staged.text.contains("main"),
            "{}",
            staged.text
        );
        let only_dir = files
            .git_diff(&tool, Some(&path("src")), false)
            .await
            .unwrap();
        assert!(only_dir.text.contains("main.rs"));
        let only_file = files
            .git_diff(&tool, Some(&path("src/main.rs")), false)
            .await
            .unwrap();
        assert!(only_file.text.contains("+fn main() { 1; }"));
        let elsewhere = files
            .git_diff(&tool, Some(&path("README.md")), false)
            .await
            .unwrap();
        assert_eq!(elsewhere.text, "", "README.md has nothing unstaged");
        assert!(
            files
                .git_diff(&tool, Some(&path("nowhere")), false)
                .await
                .is_err()
        );
        assert!(
            RelPath::parse("../x", PathStyle::Posix).is_err(),
            "a path out of the workspace never reaches git"
        );
    }

    #[tokio::test]
    async fn a_difference_longer_than_the_bound_is_cut_and_says_so() {
        // The command deadline guards hanging; this test proves byte truncation.
        const GIT_PATIENCE: std::time::Duration = std::time::Duration::from_secs(300);
        let dir = repo();
        std::fs::write(dir.path().join("src/main.rs"), "line\n".repeat(5000)).unwrap();
        let limits = Limits {
            git_diff_max_bytes: 1000,
            git_time: GIT_PATIENCE,
            ..Limits::default()
        };
        let files = files_in(dir.path(), limits);
        let diff = files
            .git_diff(&GitTool::discover(), None, false)
            .await
            .unwrap();
        assert!(diff.truncated);
        assert!(diff.text.len() <= 1000);
    }

    #[tokio::test]
    async fn looking_at_a_repository_runs_nothing_it_brings_and_writes_nothing_to_it() {
        let dir = repo();
        let marker = dir.path().join("marker-ran");
        let hook = format!("touch {}", marker.display());
        // A repository whose own configuration would run programs when it is looked at.
        git(
            dir.path(),
            &["config", "core.fsmonitor", &format!("{hook}; echo")],
        );
        git(dir.path(), &["config", "diff.external", &hook]);
        git(dir.path(), &["config", "diff.evil.textconv", &hook]);
        git(dir.path(), &["config", "filter.evil.clean", &hook]);
        std::fs::write(
            dir.path().join(".gitattributes"),
            "*.rs diff=evil filter=evil\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() { changed }\n").unwrap();
        let index_before = std::fs::read(dir.path().join(".git/index")).unwrap();
        let files = files_in(dir.path(), Limits::default());
        let tool = GitTool::discover();
        let status = files.git_status(&tool).await.unwrap();
        assert!(status.available);
        let diff = files.git_diff(&tool, None, false).await.unwrap();
        assert!(diff.text.contains("changed"));
        assert!(!marker.exists(), "nothing the repository names was run");
        assert_eq!(
            std::fs::read(dir.path().join(".git/index")).unwrap(),
            index_before,
            "the index was not written"
        );
        assert!(!dir.path().join(".git/index.lock").exists());
    }

    #[tokio::test]
    async fn a_missing_promisor_object_never_invokes_a_remote_helper_during_a_read() {
        let dir = repo();
        let marker = dir.path().join("remote-helper-ran");
        let blob = git(dir.path(), &["rev-parse", "HEAD:src/main.rs"])
            .trim()
            .to_owned();
        let object = dir
            .path()
            .join(".git/objects")
            .join(&blob[..2])
            .join(&blob[2..]);
        assert!(object.is_file());
        std::fs::remove_file(&object).unwrap();
        git(
            dir.path(),
            &[
                "config",
                "remote.origin.url",
                &format!("ext::touch {}", marker.display()),
            ],
        );
        git(dir.path(), &["config", "remote.origin.promisor", "true"]);
        git(dir.path(), &["config", "protocol.ext.allow", "always"]);
        std::fs::write(
            dir.path().join("src/main.rs"),
            "changed without the old object\n",
        )
        .unwrap();
        let index = std::fs::read(dir.path().join(".git/index")).unwrap();
        let files = files_in(dir.path(), Limits::default());
        let tool = GitTool::discover();
        let _ = files.git_status(&tool).await;
        let _ = files.git_diff(&tool, None, false).await;
        assert!(
            !marker.exists(),
            "an explicit local read cannot fetch or run a promisor helper"
        );
        assert_eq!(std::fs::read(dir.path().join(".git/index")).unwrap(), index);
    }
}
