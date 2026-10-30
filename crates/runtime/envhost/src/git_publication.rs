//! Selected-path commits and fast-forward pushes with durable effect reconciliation.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use orch_api_contract::source::GitCommitResult;
use orch_link_contract::source::{GitCommit, GitPush, GitPushResult};
use orch_types::Hash;
use serde::{Deserialize, Serialize};

use crate::error::{HostError, Result};
use crate::file_states::FileStates;
use crate::files::Files;
use crate::git::GitTool;
use crate::git_actions::{Credentials, RunOptions, run};
use crate::paths::{Locate, RelPath};
use crate::state_store::{atomic_write, read_bounded};

/// The protected reader's output bound also bounds indexes and saved replies.
const READ_BOUND: u64 = 8 * 1024 * 1024;

fn words(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

async fn git(
    tool: &GitTool,
    files: &Files,
    states: &FileStates,
    cwd: &Path,
    args: &[OsString],
    input: Option<&[u8]>,
    env: &[(OsString, OsString)],
) -> Result<Vec<u8>> {
    run(
        tool,
        files,
        states.store().root(),
        cwd,
        args,
        RunOptions {
            input,
            extra_env: env,
            ..Default::default()
        },
    )
    .await?
    .success()
}

fn text(bytes: &[u8]) -> Result<String> {
    Ok(std::str::from_utf8(bytes)
        .map_err(|_| HostError::Unsupported("Git returned a non-UTF-8 identifier".to_owned()))?
        .trim()
        .to_owned())
}

fn oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn same_remote(actual: &str, reviewed: &str) -> bool {
    orch_settings_contract::registry::is_repository_address(actual)
        && (actual == reviewed
            || crate::git::safe_remote_url(actual)
                .zip(crate::git::safe_remote_url(reviewed))
                .is_some_and(|(actual, reviewed)| actual == reviewed))
}

async fn repository(tool: &GitTool, files: &Files, asked: &str) -> Result<(PathBuf, RelPath)> {
    let path = files.parse(asked)?;
    let located = files.root().locate(&path, Locate::THROUGH)?;
    files.check_visible(&path, &located, true)?;
    let found = files
        .git_repository(tool, &located.real.under(files.root().root_path()))
        .await?
        .ok_or_else(|| {
            HostError::Unsupported("the selected directory is not a repository".to_owned())
        })?;
    if found.1 != path {
        return Err(HostError::Changed(
            "select the repository directory shown by Git status".to_owned(),
        ));
    }
    Ok(found)
}

async fn index_path(
    tool: &GitTool,
    files: &Files,
    states: &FileStates,
    cwd: &Path,
) -> Result<PathBuf> {
    let value = text(
        &git(
            tool,
            files,
            states,
            cwd,
            &words(&["rev-parse", "--git-path", "index"]),
            None,
            &[],
        )
        .await?,
    )?;
    let path = cwd.join(value);
    let parent = std::fs::canonicalize(
        path.parent()
            .ok_or_else(|| HostError::Unsupported("Git named no index directory".to_owned()))?,
    )
    .map_err(|error| HostError::io("locating the repository index", error))?;
    if !parent.starts_with(files.root().root_path()) {
        return Err(HostError::Changed(
            "the repository index leaves the workspace".to_owned(),
        ));
    }
    let path = parent.join("index");
    if std::fs::symlink_metadata(&path)
        .is_ok_and(|meta| !meta.is_file() || meta.file_type().is_symlink())
    {
        return Err(HostError::Changed(
            "the repository index is not an ordinary file".to_owned(),
        ));
    }
    Ok(path)
}

fn index_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
            read_bounded(path, READ_BOUND).map(Some)
        }
        Ok(_) => Err(HostError::Changed(
            "the repository index became another kind of entry".to_owned(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(HostError::io("reading the repository index", error)),
    }
}

/// The native index lock is held through the compare-and-swap and index publication. Its
/// marker identifies an interrupted owner; a replaced or edited lock is never removed.
struct IndexLock {
    path: PathBuf,
    content: Hash,
}
impl IndexLock {
    fn take(index: &Path, marker: &[u8], prepared: Option<Hash>) -> Result<Self> {
        let path = index.with_file_name("index.lock");
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(marker)
                    .and_then(|()| file.sync_all())
                    .map_err(|error| HostError::io("holding the Git index lock", error))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let held = index_bytes(&path)?
                    .ok_or_else(|| HostError::Changed("the index lock disappeared".to_owned()))?;
                let hash = Hash::of(&held);
                if held != marker && Some(hash) != prepared {
                    return Err(HostError::Busy(
                        "another Git writer holds the index".to_owned(),
                    ));
                }
                return Ok(Self {
                    path,
                    content: hash,
                });
            }
            Err(error) => return Err(HostError::io("holding the Git index lock", error)),
        }
        Ok(Self {
            path,
            content: Hash::of(marker),
        })
    }

    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        if index_bytes(&self.path)?.is_none_or(|held| Hash::of(&held) != self.content) {
            return Err(HostError::Changed("the Git index lock changed".to_owned()));
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&self.path)
            .map_err(|error| HostError::io("writing the reviewed Git index", error))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| HostError::io("syncing the reviewed Git index", error))?;
        self.content = Hash::of(bytes);
        Ok(())
    }
}
impl Drop for IndexLock {
    fn drop(&mut self) {
        if index_bytes(&self.path)
            .is_ok_and(|bytes| bytes.is_some_and(|bytes| Hash::of(&bytes) == self.content))
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitJournal {
    request: Hash,
    reference: String,
    original_index: Option<Hash>,
    index_hash: Hash,
    head: String,
    result: Option<GitCommitResult>,
}

async fn reference(
    tool: &GitTool,
    files: &Files,
    states: &FileStates,
    cwd: &Path,
) -> Result<String> {
    let reply = run(
        tool,
        files,
        states.store().root(),
        cwd,
        &words(&["symbolic-ref", "--quiet", "HEAD"]),
        RunOptions::default(),
    )
    .await?;
    if reply.code == Some(1) {
        return Ok("HEAD".to_owned());
    }
    let value = text(&reply.success()?)?;
    if !value.starts_with("refs/heads/") {
        return Err(HostError::Unsupported(
            "HEAD does not name a local branch".to_owned(),
        ));
    }
    Ok(value)
}

fn save_commit(path: &Path, journal: &CommitJournal) -> Result<()> {
    atomic_write(
        path,
        &serde_json::to_vec(journal).map_err(|error| HostError::Internal(error.to_string()))?,
    )
}

/// Creates the reviewed commit once, preserving all other staged paths and every working file.
pub async fn commit(
    tool: &GitTool,
    files: &Files,
    states: &FileStates,
    request: &GitCommit,
) -> Result<GitCommitResult> {
    files.gates().require_remote_write()?;
    let _gate = states.action_gate().await;
    if request.message.trim().is_empty()
        || request.message.contains('\0')
        || request.paths.is_empty()
        || request
            .expected_head
            .as_deref()
            .is_some_and(|head| !oid(head))
        || request.author.name.trim().is_empty()
        || request.author.email.trim().is_empty()
        || request.author.name.chars().any(char::is_control)
        || request.author.email.chars().any(char::is_control)
    {
        return Err(HostError::Unsupported(
            "a commit needs reviewed paths, a message and the server's identity".to_owned(),
        ));
    }
    let (cwd, directory) = repository(tool, files, &request.path).await?;
    let root = states
        .store()
        .root()
        .join("git-jobs")
        .join(request.job_id.to_string());
    std::fs::create_dir_all(&root)
        .map_err(|error| HostError::io("keeping a publication journal", error))?;
    let journal_path = root.join("commit.json");
    let request_hash = Hash::of(
        &serde_json::to_vec(request).map_err(|error| HostError::Internal(error.to_string()))?,
    );
    let marker = format!("orch-commit:{}\n", request.job_id);
    let index = index_path(tool, files, states, &cwd).await?;
    let mut journal = if journal_path.exists() {
        let journal: CommitJournal =
            serde_json::from_slice(&read_bounded(&journal_path, READ_BOUND)?).map_err(|_| {
                HostError::Unsupported("the commit journal needs recovery".to_owned())
            })?;
        if journal.request != request_hash {
            return Err(HostError::Changed(
                "the commit job was reused for different reviewed files".to_owned(),
            ));
        }
        if let Some(result) = journal.result.clone() {
            return Ok(result);
        }
        journal
    } else {
        let _lock = IndexLock::take(&index, marker.as_bytes(), None)?;
        let status = files.git_status_at(tool, Some(&directory)).await?;
        if !status.available
            || status.head != request.expected_head
            || status.status_hash != Some(request.expected_status)
        {
            return Err(HostError::Changed(
                "HEAD, staging or working files changed after review".to_owned(),
            ));
        }
        let mut paths = Vec::new();
        for asked in &request.paths {
            let path = files.parse(asked)?;
            if path.is_root()
                || path.as_str() != asked
                || path
                    .as_str()
                    .split('/')
                    .any(|part| part.eq_ignore_ascii_case(".git"))
                || !status.changes.iter().any(|change| change.path == *asked)
            {
                return Err(HostError::Changed(
                    "select only the changed paths shown by the reviewed status".to_owned(),
                ));
            }
            if !paths.contains(asked) {
                paths.push(asked.clone());
            }
        }
        // Porcelain names a staged rename by its new path. Selecting that change includes
        // its old endpoint as well, while every unrelated staged path remains untouched.
        let renamed = git(
            tool,
            files,
            states,
            &cwd,
            &words(&[
                "status",
                "--porcelain=v2",
                "-z",
                "--untracked-files=no",
                "--renames",
            ]),
            None,
            &[],
        )
        .await?;
        let mut records = renamed.split(|byte| *byte == 0);
        while let Some(record) = records.next() {
            if !record.starts_with(b"2 ") {
                continue;
            }
            let original = records
                .next()
                .ok_or_else(|| HostError::Changed("the rename observation is incomplete".into()))?;
            let record = std::str::from_utf8(record).map_err(|_| {
                HostError::Unsupported("the rename cannot be represented by the file panel".into())
            })?;
            let fields = record
                .splitn(crate::git::RENAMED_FIELDS, ' ')
                .collect::<Vec<_>>();
            if fields
                .get(crate::git::RENAMED_FIELDS - 1)
                .is_some_and(|path| request.paths.iter().any(|asked| asked.as_str() == *path))
            {
                let old = std::str::from_utf8(original).map_err(|_| {
                    HostError::Unsupported(
                        "the rename's old name cannot be represented by the file panel".into(),
                    )
                })?;
                let old_path = files.parse(old)?;
                let old_location = crate::paths::Located {
                    parent: files.root().root_dir()?,
                    real: directory.join(old_path.as_str())?,
                    name: old_path.file_name().map(OsString::from),
                    meta: None,
                };
                files.check_visible(&directory.join(old_path.as_str())?, &old_location, false)?;
                if old_path.is_root() || old_path.has_component(".git") {
                    return Err(HostError::Changed(
                        "the rename leaves the reviewed repository paths".into(),
                    ));
                }
                if !paths.iter().any(|path| path == old) {
                    paths.push(old.to_owned());
                }
            }
        }
        let original = index_bytes(&index)?;
        let original_hash = original.as_deref().map(Hash::of);
        let commit_index = root.join("commit-index");
        let env = vec![(
            OsString::from("GIT_INDEX_FILE"),
            commit_index.into_os_string(),
        )];
        let base = request.expected_head.as_deref().unwrap_or("--empty");
        git(
            tool,
            files,
            states,
            &cwd,
            &words(&["read-tree", base]),
            None,
            &env,
        )
        .await?;
        let empty = text(
            &git(
                tool,
                files,
                states,
                &cwd,
                &words(&["hash-object", "-w", "-t", "tree", "--stdin"]),
                Some(&[]),
                &[],
            )
            .await?,
        )?;
        if !tool.has_attribute_source() {
            return Err(HostError::Unsupported(
                "selected-path publication requires Git 2.43's protected attribute source"
                    .to_owned(),
            ));
        }
        let mut add = words(&[
            &format!("--attr-source={empty}"),
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.safecrlf=false",
            "add",
            "--all",
            "--",
        ]);
        add.extend(paths.iter().map(OsString::from));
        git(tool, files, states, &cwd, &add, None, &env).await?;
        let tree = text(
            &git(
                tool,
                files,
                states,
                &cwd,
                &words(&["write-tree"]),
                None,
                &env,
            )
            .await?,
        )?;
        let final_index = root.join("result-index");
        if let Some(original) = &original {
            atomic_write(&final_index, original)?;
        }
        let index_env = vec![(
            OsString::from("GIT_INDEX_FILE"),
            final_index.clone().into_os_string(),
        )];
        if original.is_none() {
            git(
                tool,
                files,
                states,
                &cwd,
                &words(&["read-tree", "--empty"]),
                None,
                &index_env,
            )
            .await?;
        }
        let mut info = Vec::new();
        let zero = "0".repeat(empty.len());
        for path in &paths {
            info.extend_from_slice(format!("0 {zero}\t{path}\0").as_bytes());
        }
        let mut selected = words(&["ls-files", "--stage", "-z", "--"]);
        selected.extend(paths.iter().map(OsString::from));
        info.extend(git(tool, files, states, &cwd, &selected, None, &env).await?);
        git(
            tool,
            files,
            states,
            &cwd,
            &words(&["update-index", "--no-split-index", "-z", "--index-info"]),
            Some(&info),
            &index_env,
        )
        .await?;
        let saved = read_bounded(&final_index, READ_BOUND)?;
        let date = format!("@{} +0000", chrono::Utc::now().timestamp());
        let author_env = [
            (
                OsString::from("GIT_AUTHOR_NAME"),
                OsString::from(&request.author.name),
            ),
            (
                OsString::from("GIT_COMMITTER_NAME"),
                OsString::from(&request.author.name),
            ),
            (
                OsString::from("GIT_AUTHOR_EMAIL"),
                OsString::from(&request.author.email),
            ),
            (
                OsString::from("GIT_COMMITTER_EMAIL"),
                OsString::from(&request.author.email),
            ),
            (OsString::from("GIT_AUTHOR_DATE"), OsString::from(&date)),
            (OsString::from("GIT_COMMITTER_DATE"), OsString::from(&date)),
        ];
        let mut args = words(&["-c", "i18n.commitEncoding=UTF-8", "commit-tree", &tree]);
        if let Some(parent) = &request.expected_head {
            args.extend(words(&["-p", parent]));
        }
        let head = text(
            &git(
                tool,
                files,
                states,
                &cwd,
                &args,
                Some(request.message.as_bytes()),
                &author_env,
            )
            .await?,
        )?;
        if !oid(&head) {
            return Err(HostError::Unsupported(
                "Git returned no commit identifier".to_owned(),
            ));
        }
        let journal = CommitJournal {
            request: request_hash,
            reference: reference(tool, files, states, &cwd).await?,
            original_index: original_hash,
            index_hash: Hash::of(&saved),
            head,
            result: None,
        };
        save_commit(&journal_path, &journal)?;
        journal
    };
    let mut lock = IndexLock::take(&index, marker.as_bytes(), Some(journal.index_hash))?;
    if reference(tool, files, states, &cwd).await? != journal.reference {
        return Err(HostError::Changed(
            "the checked-out branch changed before commit".to_owned(),
        ));
    }
    let current = files.git_status_at(tool, Some(&directory)).await?;
    let final_index = read_bounded(&root.join("result-index"), READ_BOUND)?;
    if Hash::of(&final_index) != journal.index_hash {
        return Err(HostError::Changed(
            "the prepared index failed its hash check".to_owned(),
        ));
    }
    let current_index = index_bytes(&index)?.as_deref().map(Hash::of);
    if current.head == request.expected_head {
        if current.status_hash != Some(request.expected_status)
            || current_index != journal.original_index
        {
            return Err(HostError::Changed(
                "the reviewed state changed before commit".to_owned(),
            ));
        }
        lock.write(&final_index)?;
        let previous = request
            .expected_head
            .clone()
            .unwrap_or_else(|| "0".repeat(journal.head.len()));
        git(
            tool,
            files,
            states,
            &cwd,
            &words(&[
                "update-ref",
                "-m",
                &format!("Orch commit {}", request.job_id),
                &journal.reference,
                &journal.head,
                &previous,
            ]),
            None,
            &[],
        )
        .await?;
    } else if current.head.as_deref() != Some(journal.head.as_str()) {
        return Err(HostError::Changed(
            "HEAD changed after the prepared commit; recovery will not make a second commit"
                .to_owned(),
        ));
    }
    let current_index = index_bytes(&index)?.as_deref().map(Hash::of);
    if current_index == journal.original_index {
        lock.write(&final_index)?;
        std::fs::rename(&lock.path, &index)
            .map_err(|error| HostError::io("publishing the selected-path index", error))?;
        if let Some(parent) = index.parent() {
            std::fs::File::open(parent)
                .and_then(|file| file.sync_all())
                .map_err(|error| HostError::io("syncing the selected-path index", error))?;
        }
    } else if current_index != Some(journal.index_hash) {
        return Err(HostError::Changed(
            "the commit exists but staging changed; recovery keeps the later staging".to_owned(),
        ));
    }
    let result = GitCommitResult {
        head: journal.head.clone(),
        status: files.git_status_at(tool, Some(&directory)).await?,
    };
    journal.result = Some(result.clone());
    save_commit(&journal_path, &journal)?;
    states.invalidate("not_ready");
    Ok(result)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PushJournal {
    request: Hash,
    attempted: bool,
    accepted: bool,
    result: Option<GitPushResult>,
    #[serde(default)]
    tracking_before: Option<Option<String>>,
}

/// Pushes an explicit commit to the exact reviewed destination and reconciles a lost answer.
pub async fn push(
    tool: &GitTool,
    files: &Files,
    states: &FileStates,
    request: &GitPush,
    credential: Option<&str>,
) -> Result<GitPushResult> {
    files.gates().require_remote_write()?;
    let _gate = states.action_gate().await;
    if !oid(&request.expected_head)
        || !orch_settings_contract::registry::is_git_branch(&request.branch)
        || request.remote.is_empty()
        || request.remote.starts_with('-')
        || request.remote.chars().any(char::is_control)
        || !orch_settings_contract::registry::is_git_branch(&format!(
            "{}/{}",
            request.remote, request.branch
        ))
        || !orch_settings_contract::registry::is_repository_address(&request.expected_remote_url)
    {
        return Err(HostError::Unsupported(
            "a push needs the reviewed branch, HEAD and safe remote".to_owned(),
        ));
    }
    let (cwd, directory) = repository(tool, files, &request.path).await?;
    let root = states
        .store()
        .root()
        .join("git-jobs")
        .join(request.job_id.to_string());
    let path = root.join("push.json");
    let mut frozen = request.clone();
    frozen.credential = None;
    let hash = Hash::of(
        &serde_json::to_vec(&frozen).map_err(|error| HostError::Internal(error.to_string()))?,
    );
    let mut journal = if path.exists() {
        let value: PushJournal = serde_json::from_slice(&read_bounded(&path, READ_BOUND)?)
            .map_err(|_| HostError::Unsupported("the push journal needs recovery".to_owned()))?;
        if value.request != hash {
            return Err(HostError::Changed(
                "the push job was reused for another destination".to_owned(),
            ));
        }
        if let Some(result) = value.result.clone() {
            return Ok(result);
        }
        value
    } else {
        PushJournal {
            request: hash,
            attempted: false,
            accepted: false,
            result: None,
            tracking_before: None,
        }
    };
    let status = files.git_status_at(tool, Some(&directory)).await?;
    if status.head.as_deref() != Some(request.expected_head.as_str())
        || status.branch.as_deref() != Some(request.branch.as_str())
    {
        return Err(HostError::Changed(
            "the checked-out branch or HEAD changed after push review".to_owned(),
        ));
    }
    // A named remote may have pushurl, multiple destinations or insteadOf rewrites. Resolve
    // every effective push URL, and pass the reviewed URL itself to push, never an alias.
    let effective = git(
        tool,
        files,
        states,
        &cwd,
        &words(&["remote", "get-url", "--push", "--all", &request.remote]),
        None,
        &[],
    )
    .await?;
    let urls = std::str::from_utf8(&effective)
        .map_err(|_| HostError::Unsupported("the remote URL is not UTF-8".to_owned()))?
        .lines()
        .collect::<Vec<_>>();
    if urls.len() != 1 || !same_remote(urls[0], &request.expected_remote_url) {
        return Err(HostError::Changed(
            "the effective remote URL changed after review".to_owned(),
        ));
    }
    let direct = text(
        &git(
            tool,
            files,
            states,
            &cwd,
            &words(&["ls-remote", "--get-url", "--", &request.expected_remote_url]),
            None,
            &[],
        )
        .await?,
    )?;
    if !same_remote(&direct, &request.expected_remote_url) {
        return Err(HostError::Changed(
            "the reviewed URL is rewritten by Git configuration".to_owned(),
        ));
    }
    let credential_matches = credential.is_some()
        && request.credential.is_some()
        && request
            .credential_repository_url
            .as_deref()
            .is_some_and(|original| same_remote(original, &request.expected_remote_url));
    if (credential.is_some() || request.credential.is_some()) && !credential_matches {
        return Err(HostError::Changed(
            "the source credential is not scoped to this exact remote".to_owned(),
        ));
    }
    let credentials = credential
        .map(|value| {
            Credentials::for_repository(states.store().root(), &request.expected_remote_url, value)
        })
        .transpose()?;
    let branch_ref = format!("refs/heads/{}", request.branch);
    let tracking_ref = format!("refs/remotes/{}/{}", request.remote, request.branch);
    if journal.tracking_before.is_none() {
        let before = run(
            tool,
            files,
            states.store().root(),
            &cwd,
            &words(&["rev-parse", "--verify", "--quiet", &tracking_ref]),
            RunOptions::default(),
        )
        .await?;
        journal.tracking_before = Some(if before.code == Some(0) {
            Some(text(&before.stdout)?)
        } else if before.code == Some(1) {
            None
        } else {
            return Err(HostError::Unsupported(
                "the remote-tracking ref could not be read safely".into(),
            ));
        });
        atomic_write(
            &path,
            &serde_json::to_vec(&journal)
                .map_err(|error| HostError::Internal(error.to_string()))?,
        )?;
    }
    let network = words(&[
        "-c",
        "protocol.file.allow=never",
        "-c",
        "protocol.ext.allow=never",
        "-c",
        "protocol.git.allow=never",
        "-c",
        "core.sshCommand=ssh -o BatchMode=yes",
    ]);
    if journal.attempted && !journal.accepted {
        let mut args = network.clone();
        args.extend(words(&[
            "ls-remote",
            "--refs",
            "--",
            &request.expected_remote_url,
            &branch_ref,
        ]));
        let reply = run(
            tool,
            files,
            states.store().root(),
            &cwd,
            &args,
            RunOptions {
                credentials: credentials.as_ref(),
                ..Default::default()
            },
        )
        .await?
        .success()?;
        let remote_head = std::str::from_utf8(&reply).ok().and_then(|text| {
            text.lines().find_map(|line| {
                let (head, name) = line.split_once('\t')?;
                (name == branch_ref).then_some(head)
            })
        });
        if remote_head == Some(request.expected_head.as_str()) {
            journal.accepted = true;
        } else {
            return Err(HostError::Changed("the push answer was lost and the remote no longer proves this exact commit; inspect it before retrying".to_owned()));
        }
    }
    if !journal.accepted {
        journal.attempted = true;
        atomic_write(
            &path,
            &serde_json::to_vec(&journal)
                .map_err(|error| HostError::Internal(error.to_string()))?,
        )?;
        let mut args = network;
        args.extend(words(&[
            "-c",
            "push.followTags=false",
            "push",
            "--porcelain",
            "--no-verify",
            "--no-follow-tags",
            "--recurse-submodules=no",
            "--",
            &request.expected_remote_url,
            &format!("{}:{branch_ref}", request.expected_head),
        ]));
        run(
            tool,
            files,
            states.store().root(),
            &cwd,
            &args,
            RunOptions {
                credentials: credentials.as_ref(),
                ..Default::default()
            },
        )
        .await?
        .success()?;
        journal.accepted = true;
        atomic_write(
            &path,
            &serde_json::to_vec(&journal)
                .map_err(|error| HostError::Internal(error.to_string()))?,
        )?;
    }
    if request.set_upstream {
        git(
            tool,
            files,
            states,
            &cwd,
            &words(&[
                "config",
                &format!("branch.{}.remote", request.branch),
                &request.remote,
            ]),
            None,
            &[],
        )
        .await?;
        git(
            tool,
            files,
            states,
            &cwd,
            &words(&[
                "config",
                &format!("branch.{}.merge", request.branch),
                &branch_ref,
            ]),
            None,
            &[],
        )
        .await?;
    }
    let current = run(
        tool,
        files,
        states.store().root(),
        &cwd,
        &words(&["rev-parse", "--verify", "--quiet", &tracking_ref]),
        RunOptions::default(),
    )
    .await?;
    let current = if current.code == Some(0) {
        Some(text(&current.stdout)?)
    } else if current.code == Some(1) {
        None
    } else {
        return Err(HostError::Unsupported(
            "the remote-tracking ref changed into an unreadable ref".into(),
        ));
    };
    if current.as_deref() != Some(request.expected_head.as_str())
        && journal.tracking_before.as_ref() == Some(&current)
    {
        // Sending by the reviewed URL cannot let Git refresh a named remote automatically.
        // Publish just this accepted branch with a CAS, preserving a later external fetch/push.
        let old = current.unwrap_or_else(|| "0".repeat(request.expected_head.len()));
        git(
            tool,
            files,
            states,
            &cwd,
            &words(&["update-ref", &tracking_ref, &request.expected_head, &old]),
            None,
            &[],
        )
        .await?;
    }
    let base = run(
        tool,
        files,
        states.store().root(),
        &cwd,
        &words(&[
            "symbolic-ref",
            "--quiet",
            &format!("refs/remotes/{}/HEAD", request.remote),
        ]),
        RunOptions::default(),
    )
    .await?;
    let base_branch = if base.code == Some(0) {
        text(&base.stdout)?
            .strip_prefix(&format!("refs/remotes/{}/", request.remote))
            .filter(|branch| orch_settings_contract::registry::is_git_branch(branch))
            .map(str::to_owned)
    } else {
        None
    };
    let result = GitPushResult {
        status: files.git_status_at(tool, Some(&directory)).await?,
        base_branch,
    };
    journal.result = Some(result.clone());
    atomic_write(
        &path,
        &serde_json::to_vec(&journal).map_err(|error| HostError::Internal(error.to_string()))?,
    )?;
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
    #[test]
    fn a_reviewed_remote_accepts_default_port_spelling_but_no_other_repository_or_hidden_credential()
     {
        assert!(same_remote(
            "https://code.example.test:443/team/repo.git",
            "https://code.example.test/team/repo.git"
        ));
        assert!(!same_remote(
            "https://code.example.test/team/other.git",
            "https://code.example.test/team/repo.git"
        ));
        assert!(!same_remote(
            "https://token@code.example.test/team/repo.git",
            "https://code.example.test/team/repo.git"
        ));
        assert!(!same_remote(
            "https://code.example.test/team/repo.git?secret=hidden",
            "https://code.example.test/team/repo.git"
        ));
        assert!(same_remote(
            "one.example.test:team/repo.git",
            "one.example.test:team/repo.git"
        ));
        assert!(
            !same_remote(
                "one.example.test:team/repo.git",
                "other.example.test:team/other.git"
            ),
            "two unavailable sanitized URLs are not equal evidence"
        );
    }
    use crate::environment::{GateState, Gates, Limits};
    use crate::ignore::IgnoreRules;
    use crate::index::HashCache;
    use crate::paths::{PathStyle, Workspace};
    use crate::state_store::ObjectStore;
    use orch_link_contract::source::GitAuthor;
    use orch_types::new_id;
    use std::sync::Arc;

    fn fixture() -> (tempfile::TempDir, Files, FileStates, GitTool) {
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
        let tool = GitTool::discover();
        native(&tool, &workspace, &["init", "-b", "work"]);
        std::fs::write(workspace.join("selected"), "original selected\n").unwrap();
        std::fs::write(workspace.join("other"), "original other\n").unwrap();
        native(&tool, &workspace, &["add", "."]);
        native(
            &tool,
            &workspace,
            &[
                "-c",
                "user.name=Original",
                "-c",
                "user.email=original@orch.invalid",
                "commit",
                "-m",
                "base",
            ],
        );
        (directory, files, states, tool)
    }

    fn native(tool: &GitTool, cwd: &Path, args: &[&str]) -> Vec<u8> {
        let output = std::process::Command::new(tool.program().unwrap())
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Git fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    async fn reviewed(tool: &GitTool, files: &Files) -> GitCommit {
        let status = files.git_status(tool).await.unwrap();
        GitCommit {
            environment_id: new_id(),
            job_id: new_id(),
            path: String::new(),
            paths: vec!["selected".to_owned()],
            message: "Publish selected work".to_owned(),
            expected_head: status.head,
            expected_status: status.status_hash.unwrap(),
            author: GitAuthor {
                name: "Confirmed caller".to_owned(),
                email: "confirmed@orch.invalid".to_owned(),
            },
        }
    }

    #[tokio::test]
    async fn selected_commit_has_the_callers_identity_and_preserves_other_staging_and_files() {
        let (_directory, files, states, tool) = fixture();
        let cwd = files.root().root_path();
        std::fs::write(cwd.join("other"), "staged other\n").unwrap();
        native(&tool, cwd, &["add", "other"]);
        std::fs::write(cwd.join("other"), "later unstaged other\n").unwrap();
        std::fs::write(cwd.join("selected"), [0, 255, 1, 7]).unwrap();
        let staged_other = native(&tool, cwd, &["show", ":other"]);
        let config = std::fs::read(cwd.join(".git/config")).unwrap();
        let asked = reviewed(&tool, &files).await;
        let result = commit(&tool, &files, &states, &asked).await.unwrap();
        assert_eq!(
            native(&tool, cwd, &["show", "HEAD:selected"]),
            [0, 255, 1, 7]
        );
        assert_eq!(
            native(&tool, cwd, &["show", "HEAD:other"]),
            b"original other\n"
        );
        assert_eq!(native(&tool, cwd, &["show", ":other"]), staged_other);
        assert_eq!(
            std::fs::read(cwd.join("other")).unwrap(),
            b"later unstaged other\n"
        );
        assert_eq!(std::fs::read(cwd.join(".git/config")).unwrap(), config);
        let identity = native(
            &tool,
            cwd,
            &["show", "-s", "--format=%an|%ae|%cn|%ce", "HEAD"],
        );
        assert_eq!(
            text(&identity).unwrap(),
            "Confirmed caller|confirmed@orch.invalid|Confirmed caller|confirmed@orch.invalid"
        );
        assert_eq!(
            commit(&tool, &files, &states, &asked).await.unwrap(),
            result
        );
        assert_eq!(
            text(&native(&tool, cwd, &["rev-list", "--count", "HEAD"])).unwrap(),
            "2"
        );
    }

    #[tokio::test]
    async fn selecting_a_rename_commits_both_ends_and_keeps_unrelated_staging() {
        let (_directory, files, states, tool) = fixture();
        let cwd = files.root().root_path();
        std::fs::write(cwd.join("other"), "other staging\n").unwrap();
        native(&tool, cwd, &["add", "other"]);
        native(&tool, cwd, &["mv", "selected", "renamed"]);
        let status = files.git_status(&tool).await.unwrap();
        let request = GitCommit {
            environment_id: new_id(),
            job_id: new_id(),
            path: String::new(),
            paths: vec!["renamed".into()],
            message: "Rename one selected file".into(),
            expected_head: status.head,
            expected_status: status.status_hash.unwrap(),
            author: GitAuthor {
                name: "Caller".into(),
                email: "caller@orch.invalid".into(),
            },
        };
        commit(&tool, &files, &states, &request).await.unwrap();
        assert_eq!(
            native(&tool, cwd, &["show", "HEAD:renamed"]),
            b"original selected\n"
        );
        assert!(
            !text(&native(&tool, cwd, &["ls-tree", "--name-only", "HEAD"]))
                .unwrap()
                .lines()
                .any(|path| path == "selected")
        );
        assert_eq!(native(&tool, cwd, &["show", ":other"]), b"other staging\n");
        assert_eq!(
            native(&tool, cwd, &["show", "HEAD:other"]),
            b"original other\n"
        );
    }

    #[tokio::test]
    async fn changed_review_is_refused_and_a_ref_applied_before_index_publication_is_recovered_once()
     {
        let (_directory, files, states, tool) = fixture();
        let cwd = files.root().root_path();
        std::fs::write(cwd.join("selected"), "reviewed\n").unwrap();
        let stale = reviewed(&tool, &files).await;
        std::fs::write(cwd.join("selected"), "later bytes\n").unwrap();
        assert!(matches!(
            commit(&tool, &files, &states, &stale).await,
            Err(HostError::Changed(_))
        ));
        assert_eq!(
            text(&native(&tool, cwd, &["rev-list", "--count", "HEAD"])).unwrap(),
            "1"
        );
        let asked = reviewed(&tool, &files).await;
        let old_index = std::fs::read(cwd.join(".git/index")).unwrap();
        let result = commit(&tool, &files, &states, &asked).await.unwrap();
        let path = states
            .store()
            .root()
            .join("git-jobs")
            .join(asked.job_id.to_string())
            .join("commit.json");
        let mut journal: CommitJournal =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        journal.result = None;
        save_commit(&path, &journal).unwrap();
        // Reproduce the durable boundary after the ref changed and before index publication.
        std::fs::write(cwd.join(".git/index"), old_index).unwrap();
        let recovered = commit(&tool, &files, &states, &asked).await.unwrap();
        assert_eq!(recovered.head, result.head);
        assert_eq!(native(&tool, cwd, &["show", ":selected"]), b"later bytes\n");
        assert_eq!(
            text(&native(&tool, cwd, &["rev-list", "--count", "HEAD"])).unwrap(),
            "2"
        );
    }

    fn shell_word(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    // Network transport alone is substituted; acceptance/rejection/object transfer/ref CAS
    // run through real Git and a local bare repository, with no external service or token.
    fn local_remote(
        directory: &Path,
        files: &Files,
        real: &GitTool,
    ) -> (GitTool, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let bare = directory.join("remote.git");
        native(
            real,
            directory,
            &["init", "--bare", &bare.to_string_lossy()],
        );
        native(
            real,
            files.root().root_path(),
            &[
                "remote",
                "add",
                "origin",
                "https://example.test/team/repo.git",
            ],
        );
        native(
            real,
            files.root().root_path(),
            &["push", &bare.to_string_lossy(), "HEAD:refs/heads/work"],
        );
        let bin = directory.join("transport");
        std::fs::create_dir(&bin).unwrap();
        let program = bin.join("git");
        let script = format!(
            "#!/bin/sh\nnetwork=no\nget_url=no\nfor argument in \"$@\"; do\n case \"$argument\" in push|ls-remote) network=yes;; --get-url) get_url=yes;; esac\ndone\n\
            count=$#\nwhile [ \"$count\" -gt 0 ]; do\n argument=$1\n shift\n if [ \"$network\" = yes ] && [ \"$get_url\" = no ]; then\n\
            case \"$argument\" in https://example.test/team/repo.git) argument={bare};; protocol.file.allow=never) argument=protocol.file.allow=always;; esac\n fi\n\
            set -- \"$@\" \"$argument\"\n count=$((count - 1))\ndone\nexec {real} \"$@\"\n",
            bare = shell_word(&bare.to_string_lossy()),
            real = shell_word(&real.program().unwrap().to_string_lossy())
        );
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        (GitTool::find_in(&bin.into_os_string()), bare)
    }

    async fn push_request(tool: &GitTool, files: &Files) -> GitPush {
        let status = files.git_status(tool).await.unwrap();
        GitPush {
            environment_id: new_id(),
            job_id: new_id(),
            path: String::new(),
            remote: "origin".into(),
            expected_remote_url: "https://example.test/team/repo.git".into(),
            branch: "work".into(),
            expected_head: status.head.unwrap(),
            set_upstream: true,
            credential: None,
            credential_repository_url: None,
        }
    }

    #[tokio::test]
    async fn real_bare_push_sets_explicit_upstream_and_reconciles_a_lost_acceptance_without_a_second_push()
     {
        let (directory, files, states, real) = fixture();
        let (tool, bare) = local_remote(directory.path(), &files, &real);
        std::fs::write(
            files.root().root_path().join("selected"),
            "next local commit\n",
        )
        .unwrap();
        let selected = reviewed(&tool, &files).await;
        commit(&tool, &files, &states, &selected).await.unwrap();
        let asked = push_request(&tool, &files).await;
        native(
            &real,
            files.root().root_path(),
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/work",
            ],
        );
        let result = push(&tool, &files, &states, &asked, None).await.unwrap();
        assert_eq!(
            result.base_branch.as_deref(),
            Some("work"),
            "the forge base comes only from the recorded remote HEAD"
        );
        assert_eq!(
            text(&native(&real, &bare, &["rev-parse", "refs/heads/work"])).unwrap(),
            asked.expected_head
        );
        assert_eq!(result.status.upstream.as_deref(), Some("origin/work"));
        assert_eq!(result.status.ahead, Some(0));
        let journal_path = states
            .store()
            .root()
            .join("git-jobs")
            .join(asked.job_id.to_string())
            .join("push.json");
        let mut journal: PushJournal =
            serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
        journal.result = None;
        journal.accepted = false;
        atomic_write(&journal_path, &serde_json::to_vec(&journal).unwrap()).unwrap();
        assert_eq!(
            push(&tool, &files, &states, &asked, None).await.unwrap(),
            result
        );
        assert_eq!(
            text(&native(
                &real,
                &bare,
                &["rev-list", "--count", "refs/heads/work"]
            ))
            .unwrap(),
            "2"
        );
    }

    #[tokio::test]
    async fn a_changed_remote_and_non_fast_forward_leave_the_commit_local_and_preserve_the_remote_branch()
     {
        let (directory, files, states, real) = fixture();
        let (tool, bare) = local_remote(directory.path(), &files, &real);
        std::fs::write(
            files.root().root_path().join("selected"),
            "local second commit\n",
        )
        .unwrap();
        commit(&tool, &files, &states, &reviewed(&tool, &files).await)
            .await
            .unwrap();
        let asked = push_request(&tool, &files).await;
        native(
            &real,
            files.root().root_path(),
            &[
                "config",
                "remote.origin.pushurl",
                "https://other.example.test/team/repo.git",
            ],
        );
        assert!(matches!(
            push(&tool, &files, &states, &asked, None).await,
            Err(HostError::Changed(_))
        ));
        native(
            &real,
            files.root().root_path(),
            &["config", "--unset", "remote.origin.pushurl"],
        );
        let other = directory.path().join("other-writer");
        native(
            &real,
            directory.path(),
            &[
                "clone",
                "--branch",
                "work",
                &bare.to_string_lossy(),
                &other.to_string_lossy(),
            ],
        );
        std::fs::write(other.join("other"), "remote second commit\n").unwrap();
        native(&real, &other, &["add", "other"]);
        native(
            &real,
            &other,
            &[
                "-c",
                "user.name=Other",
                "-c",
                "user.email=other@orch.invalid",
                "commit",
                "-m",
                "other",
            ],
        );
        native(&real, &other, &["push", "origin", "work"]);
        let remote_head = text(&native(&real, &bare, &["rev-parse", "refs/heads/work"])).unwrap();
        let retry = GitPush {
            job_id: new_id(),
            ..asked
        };
        assert!(push(&tool, &files, &states, &retry, None).await.is_err());
        assert_eq!(
            text(&native(&real, &bare, &["rev-parse", "refs/heads/work"])).unwrap(),
            remote_head
        );
        assert_eq!(
            text(&native(
                &real,
                files.root().root_path(),
                &["rev-parse", "HEAD"]
            ))
            .unwrap(),
            retry.expected_head
        );
        assert_eq!(
            native(&real, files.root().root_path(), &["show", "HEAD:selected"]),
            b"local second commit\n"
        );
    }
}
