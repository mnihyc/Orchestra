//! Job-owned worktrees keep their own files and index while sharing the source repository.

use orch_api_contract::source::{SourceInfo, SourceKind, SourceState, WorktreeFrom};
use orch_link_contract::source::{FsRecordRead, GitWorktree, GitWorktreeCleanupResult};
use orch_types::{Hash, Time};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;

use crate::error::{HostError, Result};
use crate::file_states::FileStates;
use crate::files::Files;
use crate::git::GitTool;
use crate::git_actions::{RunOptions, run};
use crate::manifest::{EntryKind, ManifestEntry};
use crate::paths::{Locate, RelPath};
use crate::platform;
use crate::state_store::{atomic_write, read_bounded};

// A job journal can retain the same complete namespace as its saved manifest.
const READ_BOUND: u64 = crate::manifest::MANIFEST_MAX_EXPANDED_BYTES;
const EXECUTABLE_BITS: u32 = 0o111;
fn words(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
async fn git(
    tool: &GitTool,
    files: &Files,
    states: &FileStates,
    cwd: &Path,
    args: &[OsString],
) -> Result<Vec<u8>> {
    run(
        tool,
        files,
        states.store().root(),
        cwd,
        args,
        RunOptions::default(),
    )
    .await?
    .success()
}
fn text(bytes: &[u8]) -> Result<String> {
    Ok(std::str::from_utf8(bytes)
        .map_err(|_| HostError::Unsupported("Git returned a non-UTF-8 reference".to_owned()))?
        .trim()
        .to_owned())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    request: Hash,
    generation: i64,
    parent_device: u64,
    parent_inode: u64,
    stage_device: u64,
    stage_inode: u64,
    stage: String,
    base: String,
    created: bool,
    populated: bool,
    installed: bool,
    result: Option<SourceInfo>,
    #[serde(default)]
    indexed: bool,
    #[serde(default)]
    git_file: Option<Hash>,
    #[serde(default)]
    snapshot_id: Option<orch_types::Id>,
    #[serde(default)]
    contents: Option<Vec<ManifestEntry>>,
    #[serde(default)]
    installed_git_file: Option<Hash>,
    #[serde(default)]
    removing: bool,
    #[serde(default)]
    removed: bool,
}

fn save(path: &Path, journal: &Journal) -> Result<()> {
    atomic_write(
        path,
        &serde_json::to_vec(journal).map_err(|error| HostError::Internal(error.to_string()))?,
    )
}

/// The first completed worktree result survives capture expiry and later source-directory changes.
pub(crate) fn completed(
    states: &FileStates,
    request: &GitWorktree,
    generation: i64,
) -> Result<Option<SourceInfo>> {
    let path = states
        .store()
        .root()
        .join("worktree-jobs")
        .join(format!("{}.json", request.job_id));
    if !path.exists() {
        return Ok(None);
    }
    let journal: Journal = serde_json::from_slice(&read_bounded(&path, READ_BOUND)?)
        .map_err(|_| HostError::Unsupported("the worktree journal needs recovery".into()))?;
    let hash = Hash::of(
        &serde_json::to_vec(request).map_err(|error| HostError::Internal(error.to_string()))?,
    );
    if journal.request != hash || journal.generation != generation {
        return Err(HostError::Changed(
            "the worktree job changed its choice or generation".into(),
        ));
    }
    if journal.removed || journal.removing {
        return Err(HostError::Changed(
            "the failed worktree needs a fresh owned attempt".into(),
        ));
    }
    Ok(journal.result)
}

struct Temporary {
    directory: platform::Dir,
    name: OsString,
    device: u64,
    inode: u64,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        if self
            .directory
            .lstat(&self.name)
            .is_ok_and(|meta| (meta.dev, meta.ino) == (self.device, self.inode))
        {
            let _ = self.directory.unlink(&self.name);
        }
    }
}

async fn verify_head(
    tool: &GitTool,
    files: &Files,
    states: &FileStates,
    path: &Path,
    journal: &Journal,
    branch: &str,
) -> Result<()> {
    files.git_repository(tool, path).await?.ok_or_else(|| {
        HostError::Changed("the worktree no longer names an owned repository".to_owned())
    })?;
    let head = text(
        &git(
            tool,
            files,
            states,
            path,
            &words(&["rev-parse", "--verify", "HEAD"]),
        )
        .await?,
    )?;
    let current = text(
        &git(
            tool,
            files,
            states,
            path,
            &words(&["symbolic-ref", "--quiet", "--short", "HEAD"]),
        )
        .await?,
    )?;
    if head != journal.base || current != branch {
        return Err(HostError::Changed(
            "the worktree changed its frozen branch or base".to_owned(),
        ));
    }
    Ok(())
}

fn parent(files: &Files, directory: &RelPath) -> Result<platform::Dir> {
    let path = directory.parent().unwrap_or_else(RelPath::root);
    let located = files.root().locate(&path, Locate::THROUGH)?;
    if located.real != path {
        return Err(HostError::Changed(
            "the worktree parent follows another directory".to_owned(),
        ));
    }
    files.check_visible(&path, &located, true)?;
    files.root().open_dir(&located)
}

fn with_parents(
    files: &Files,
    entries: impl IntoIterator<Item = ManifestEntry>,
    git_file: Option<Hash>,
) -> Result<BTreeMap<String, ManifestEntry>> {
    let mut wanted = BTreeMap::new();
    for entry in entries {
        let relative = files.parse(&entry.p)?;
        if relative.is_root() || relative.has_component(".git") || entry.t == EntryKind::Other {
            return Err(HostError::Changed(
                "the worktree contains unsupported repository metadata or special files".into(),
            ));
        }
        let mut ancestor = relative.parent();
        if wanted
            .insert(entry.p.clone(), entry)
            .is_some_and(|old| old.t != EntryKind::Dir)
        {
            return Err(HostError::Changed("the worktree repeats a path".into()));
        }
        while let Some(path) = ancestor.filter(|path| !path.is_root()) {
            wanted
                .entry(path.to_string())
                .or_insert_with(|| ManifestEntry {
                    p: path.to_string(),
                    t: EntryKind::Dir,
                    s: 0,
                    m: 0,
                    x: 0o700,
                    h: None,
                    l: None,
                });
            ancestor = path.parent();
        }
    }
    wanted.insert(
        ".git".into(),
        ManifestEntry {
            p: ".git".into(),
            t: EntryKind::File,
            s: 0,
            m: 0,
            x: 0,
            h: git_file,
            l: None,
        },
    );
    Ok(wanted)
}

// Compute the project's object identity and the raw recorded identity from the same bounded
// stream. No attributes, filters, Git executable or network are involved in reading bytes.
fn file_identity(file: &std::fs::File, size: u64, sha256: bool) -> Result<(String, Hash)> {
    let header = format!("blob {size}\0");
    let mut sha1 = Sha1::new();
    let mut sha2 = Sha256::new();
    let mut raw = Hash::hasher();
    sha1.update(header.as_bytes());
    sha2.update(header.as_bytes());
    let mut buffer = vec![0; crate::blobs::READ_CHUNK_BYTES];
    let mut offset = 0;
    loop {
        let count = platform::read_at(file, &mut buffer, offset)
            .map_err(|error| HostError::io("checking worktree bytes", error))?;
        if count == 0 {
            break;
        }
        offset += count as u64;
        if offset > size {
            return Err(HostError::Changed(
                "the worktree file grew while it was checked".into(),
            ));
        }
        sha1.update(&buffer[..count]);
        sha2.update(&buffer[..count]);
        raw.update(&buffer[..count]);
    }
    if offset != size {
        return Err(HostError::Changed(
            "the worktree file changed size while it was checked".into(),
        ));
    }
    Ok((
        if sha256 {
            hex::encode(sha2.finalize())
        } else {
            hex::encode(sha1.finalize())
        },
        Hash::finish(&raw),
    ))
}

async fn commit_entries(
    tool: &GitTool,
    files: &Files,
    states: &FileStates,
    path: &Path,
    base: &str,
    partial: bool,
) -> Result<Vec<ManifestEntry>> {
    let tree = git(
        tool,
        files,
        states,
        path,
        &words(&["ls-tree", "-r", "-z", "--full-tree", base]),
    )
    .await?;
    let mut entries = Vec::new();
    for row in tree.split(|byte| *byte == 0).filter(|row| !row.is_empty()) {
        let split = row.iter().position(|byte| *byte == b'\t').ok_or_else(|| {
            HostError::Unsupported("Git returned an invalid worktree tree".into())
        })?;
        let (header, rest) = row.split_at(split);
        let name = &rest[1..];
        let mut fields = std::str::from_utf8(header)
            .map_err(|_| HostError::Unsupported("Git returned an invalid worktree object".into()))?
            .split(' ');
        let mode = fields.next().unwrap_or_default();
        let kind = fields.next().unwrap_or_default();
        let oid = fields.next().unwrap_or_default();
        if kind != "blob"
            || !matches!(mode, "100644" | "100755" | "120000")
            || fields.next().is_some()
            || !matches!(oid.len(), 40 | 64)
            || !oid.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(HostError::Unsupported(
                "a worktree base contains an unsupported object".into(),
            ));
        }
        let relative = files.parse(std::str::from_utf8(name).map_err(|_| {
            HostError::Unsupported("the worktree base has a non-UTF-8 path".into())
        })?)?;
        let directory = files.parse(
            path.strip_prefix(files.root().root_path())
                .map_err(|_| HostError::Changed("the worktree left the workspace".into()))?
                .to_str()
                .ok_or_else(|| {
                    HostError::Unsupported("the worktree directory is not UTF-8".into())
                })?,
        )?;
        let located = files
            .root()
            .locate(&directory.append(&relative)?, Locate::ENTRY)?;
        let Some(meta) = located.meta.as_ref() else {
            if partial {
                continue;
            }
            return Err(HostError::Changed(
                "a committed worktree path disappeared".into(),
            ));
        };
        let (object, hash, link) = if mode == "120000" && meta.is_symlink() {
            let link = located
                .parent
                .read_link(located.name()?)
                .map_err(|error| HostError::io("checking the committed link", error))?
                .into_string()
                .map_err(|_| HostError::Unsupported("the committed link is not UTF-8".into()))?;
            let bytes = format!("blob {}\0{link}", link.len());
            (
                if oid.len() == 64 {
                    hex::encode(Sha256::digest(bytes.as_bytes()))
                } else {
                    hex::encode(Sha1::digest(bytes.as_bytes()))
                },
                Hash::of(link.as_bytes()),
                Some(link),
            )
        } else if mode != "120000"
            && meta.is_file()
            && (meta.mode & 0o100 != 0) == (mode == "100755")
        {
            let file = located
                .parent
                .open_read(located.name()?)
                .map_err(|error| HostError::io("checking the committed worktree file", error))?;
            let size = file
                .metadata()
                .map_err(|error| HostError::io("checking the committed file size", error))?
                .len();
            let (object, hash) = file_identity(&file, size, oid.len() == 64)?;
            (object, hash, None)
        } else {
            return Err(HostError::Changed(
                "the committed worktree path changed its type or executable bits".into(),
            ));
        };
        if object != oid {
            return Err(HostError::Changed(
                "the worktree bytes differ from the chosen commit".into(),
            ));
        }
        entries.push(ManifestEntry {
            p: relative.to_string(),
            t: if link.is_some() {
                EntryKind::Link
            } else {
                EntryKind::File
            },
            s: 0,
            m: 0,
            x: meta.mode & EXECUTABLE_BITS,
            h: Some(hash),
            l: link,
        });
    }
    Ok(entries)
}

struct CheckoutProof<'a> {
    request: &'a GitWorktree,
    journal: &'a Journal,
    path: &'a Path,
    directory: &'a platform::Dir,
    now: Time,
    complete: bool,
}

async fn verify_contents(
    tool: &GitTool,
    files: &Files,
    states: &FileStates,
    proof: CheckoutProof<'_>,
) -> Result<()> {
    let CheckoutProof {
        request,
        journal,
        path,
        directory,
        now,
        complete,
    } = proof;
    verify_head(tool, files, states, path, journal, &request.choice.branch).await?;
    let tree = text(
        &git(
            tool,
            files,
            states,
            path,
            &words(&[
                "rev-parse",
                "--verify",
                &format!("{}^{{tree}}", journal.base),
            ]),
        )
        .await?,
    )?;
    if journal.indexed
        && text(&git(tool, files, states, path, &words(&["write-tree"])).await?)? != tree
    {
        return Err(HostError::Changed(
            "another writer changed the prepared worktree index".into(),
        ));
    }
    let entries = match &request.choice.from {
        WorktreeFrom::Now { .. } => {
            let read = states.read_recorded_recovery(
                files,
                &crate::index::Index::new(orch_link_contract::commands::IndexMode::Live, 0),
                &FsRecordRead {
                    environment_id: request.environment_id,
                    snapshot_id: request.snapshot_id.ok_or_else(|| {
                        HostError::Changed("the worktree has no saved files".into())
                    })?,
                    path: None,
                    export: false,
                },
                request.job_id,
                now,
            )?;
            if read.root.directory != request.choice.repository_dir
                || read.root.generation != journal.generation
                || read.root.repository_head.as_deref() != Some(journal.base.as_str())
            {
                return Err(HostError::Changed(
                    "the prepared worktree no longer names its own saved base".into(),
                ));
            }
            read.entries
        }
        WorktreeFrom::Commit { .. } => {
            commit_entries(tool, files, states, path, &journal.base, !complete).await?
        }
    };
    let wanted = with_parents(
        files,
        journal.contents.clone().unwrap_or(entries),
        journal.git_file,
    )?;
    crate::exact_materialize::inventory(directory, "", &wanted, complete)?;
    Ok(())
}

async fn remove_branch(
    tool: &GitTool,
    files: &Files,
    states: &FileStates,
    repository: &Path,
    branch: &str,
    base: &str,
) -> Result<bool> {
    let worktrees = git(
        tool,
        files,
        states,
        repository,
        &words(&["worktree", "list", "--porcelain", "-z"]),
    )
    .await?;
    let used = format!("branch {branch}");
    if worktrees
        .split(|byte| *byte == 0)
        .any(|field| field == used.as_bytes())
    {
        return Ok(false);
    }
    Ok(run(
        tool,
        files,
        states.store().root(),
        repository,
        &words(&["update-ref", "-d", branch, base]),
        RunOptions::default(),
    )
    .await?
    .code
        == Some(0))
}

/// An owned source checkout has exactly its frozen raw commit files and unchanged HEAD/index.
pub(crate) async fn verify_source_checkout(
    tool: &GitTool,
    files: &Files,
    states: &FileStates,
    path: &Path,
    head: &str,
    branch: &str,
) -> Result<()> {
    files.git_repository(tool, path).await?.ok_or_else(|| {
        HostError::Changed("the prepared source no longer has its repository".into())
    })?;
    if text(
        &git(
            tool,
            files,
            states,
            path,
            &words(&["rev-parse", "--verify", "HEAD"]),
        )
        .await?,
    )? != head
        || text(
            &git(
                tool,
                files,
                states,
                path,
                &words(&["symbolic-ref", "--quiet", "--short", "HEAD"]),
            )
            .await?,
        )? != branch
    {
        return Err(HostError::Changed(
            "another writer changed the prepared source's head or branch".into(),
        ));
    }
    let tree = text(
        &git(
            tool,
            files,
            states,
            path,
            &words(&["rev-parse", "--verify", &format!("{head}^{{tree}}")]),
        )
        .await?,
    )?;
    if text(&git(tool, files, states, path, &words(&["write-tree"])).await?)? != tree {
        return Err(HostError::Changed(
            "another writer staged files in the prepared source".into(),
        ));
    }
    let mut wanted = with_parents(
        files,
        commit_entries(tool, files, states, path, head, false).await?,
        None,
    )?;
    wanted.remove(".git");
    let relative =
        files.target_of(path.to_str().ok_or_else(|| {
            HostError::Unsupported("the prepared source path is not UTF-8".into())
        })?)?;
    let located = files.root().locate(&relative, Locate::THROUGH)?;
    let held = files.root().open_dir(&located)?;
    let mut count = 0usize;
    for name in held
        .names()
        .map_err(|error| HostError::io("checking all prepared source names", error))?
    {
        if name == OsStr::new(".git") {
            if !held.lstat(&name).is_ok_and(|meta| meta.is_dir()) {
                return Err(HostError::Changed(
                    "the prepared source's private repository changed type".into(),
                ));
            }
            continue;
        }
        let name_text = name.to_str().ok_or_else(|| {
            HostError::Changed("another writer added a non-UTF-8 source name".into())
        })?;
        let expected = wanted.get(name_text).ok_or_else(|| {
            HostError::Changed("another writer added a name to the prepared source".into())
        })?;
        if !crate::exact_materialize::same(expected, &held, &name)? {
            return Err(HostError::Changed(
                "another writer changed a prepared source entry".into(),
            ));
        }
        count += 1;
        if expected.t == EntryKind::Dir {
            count += crate::exact_materialize::inventory(
                &held.open_child(&name).map_err(|error| {
                    HostError::io("checking a prepared source directory", error)
                })?,
                name_text,
                &wanted,
                false,
            )?;
        }
    }
    if count != wanted.len() {
        return Err(HostError::Changed(
            "the prepared source no longer has its complete namespace".into(),
        ));
    }
    Ok(())
}

/// Makes an unused branch/directory once; From now copies the verified raw recorded files only.
pub async fn create(
    tool: &GitTool,
    files: &Files,
    states: &FileStates,
    request: &GitWorktree,
    generation: i64,
    now: Time,
) -> Result<SourceInfo> {
    files.gates().require_remote_write()?;
    let _gate = states.action_gate().await;
    if !orch_settings_contract::registry::is_git_branch(&request.choice.branch) {
        return Err(HostError::Unsupported(
            "the new worktree branch is invalid".to_owned(),
        ));
    }
    let source = files.parse(&request.choice.repository_dir)?;
    let directory = files.parse(&request.choice.directory)?;
    if directory.is_root() || directory == source {
        return Err(HostError::Changed(
            "an own attempt needs another unused directory".to_owned(),
        ));
    }
    let located = files.root().locate(&source, Locate::THROUGH)?;
    files.check_visible(&source, &located, true)?;
    let (repository, root) = files
        .git_repository(tool, &located.real.under(files.root().root_path()))
        .await?
        .ok_or_else(|| HostError::Unsupported("an own worktree needs a repository".to_owned()))?;
    if root != source {
        return Err(HostError::Changed(
            "select the source repository shown by status".to_owned(),
        ));
    }
    let journal_path = states
        .store()
        .root()
        .join("worktree-jobs")
        .join(format!("{}.json", request.job_id));
    let hash = Hash::of(
        &serde_json::to_vec(request).map_err(|error| HostError::Internal(error.to_string()))?,
    );
    let mut journal = if journal_path.exists() {
        let journal: Journal = serde_json::from_slice(&read_bounded(&journal_path, READ_BOUND)?)
            .map_err(|_| {
                HostError::Unsupported("the worktree journal needs recovery".to_owned())
            })?;
        if journal.request != hash || journal.generation != generation {
            return Err(HostError::Changed(
                "the worktree job changed its choice or generation".to_owned(),
            ));
        }
        if journal.removing || journal.removed {
            return Err(HostError::Changed(
                "this failed worktree was removed; retry with a new owned attempt".into(),
            ));
        }
        if let Some(result) = &journal.result {
            return Ok(result.clone());
        }
        journal
    } else {
        let branch = format!("refs/heads/{}", request.choice.branch);
        let exists = run(
            tool,
            files,
            states.store().root(),
            &repository,
            &words(&["show-ref", "--verify", "--quiet", &branch]),
            RunOptions::default(),
        )
        .await?;
        if exists.code != Some(1) {
            return Err(HostError::Changed(
                "the branch exists already; choose a new branch".to_owned(),
            ));
        }
        let target = files.root().locate(&directory, Locate::ENTRY)?;
        files.check_visible(&directory, &target, true)?;
        if target.meta.is_some() {
            return Err(HostError::Changed(
                "the worktree directory exists already; its files are kept".to_owned(),
            ));
        }
        let base = match &request.choice.from {
            WorktreeFrom::Now { .. } => {
                let snapshot = request.snapshot_id.ok_or_else(|| {
                    HostError::Unsupported(
                        "From now requires a complete current file state".to_owned(),
                    )
                })?;
                let root = states.store().load(snapshot, now)?.0;
                if root.directory != request.choice.repository_dir {
                    return Err(HostError::Changed(
                        "the current files belong to another repository directory".to_owned(),
                    ));
                }
                root.repository_head.ok_or_else(|| {
                    HostError::Unsupported(
                        "the recorded current state's base commit is unknown; choose a commit"
                            .to_owned(),
                    )
                })?
            }
            WorktreeFrom::Commit { commit }
                if !commit.is_empty() && !commit.chars().any(char::is_control) =>
            {
                format!("{commit}^{{commit}}")
            }
            _ => {
                return Err(HostError::Unsupported(
                    "choose a complete current state or a commit".to_owned(),
                ));
            }
        };
        let base = text(
            &git(
                tool,
                files,
                states,
                &repository,
                &words(&["rev-parse", "--verify", "--end-of-options", &base]),
            )
            .await?,
        )?;
        if !matches!(base.len(), 40 | 64) || !base.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(HostError::Unsupported(
                "the worktree base is not a commit".to_owned(),
            ));
        }
        let parent = parent(files, &directory)?;
        let stage = format!(".orch-tmp-worktree-{}", request.job_id);
        parent
            .mkdir(OsStr::new(&stage), 0o700)
            .map_err(|error| HostError::io("making the owned worktree stage", error))?;
        let meta = parent
            .stat_self()
            .map_err(|error| HostError::io("checking the worktree parent", error))?;
        let staged = parent
            .lstat(OsStr::new(&stage))
            .map_err(|error| HostError::io("checking the worktree stage", error))?;
        let journal = Journal {
            request: hash,
            generation,
            parent_device: meta.dev,
            parent_inode: meta.ino,
            stage_device: staged.dev,
            stage_inode: staged.ino,
            stage,
            base,
            created: false,
            populated: false,
            installed: false,
            result: None,
            indexed: false,
            git_file: None,
            snapshot_id: request.snapshot_id,
            contents: None,
            installed_git_file: None,
            removing: false,
            removed: false,
        };
        save(&journal_path, &journal)?;
        journal
    };
    let parent = parent(files, &directory)?;
    let meta = parent
        .stat_self()
        .map_err(|error| HostError::io("rechecking the worktree parent", error))?;
    if (meta.dev, meta.ino) != (journal.parent_device, journal.parent_inode) {
        return Err(HostError::Changed("the worktree parent changed".to_owned()));
    }
    let staging = directory
        .parent()
        .unwrap_or_else(RelPath::root)
        .join(&journal.stage)?;
    let stage_path = staging.under(files.root().root_path());
    if !journal.installed {
        let staged = parent.lstat(OsStr::new(&journal.stage));
        let at_target = files.root().locate(&directory, Locate::ENTRY)?;
        if staged
            .as_ref()
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            && at_target.meta.as_ref().is_some_and(|meta| {
                (meta.dev, meta.ino) == (journal.stage_device, journal.stage_inode)
            })
        {
            journal.installed = true;
            save(&journal_path, &journal)?;
        } else {
            let staged = staged
                .map_err(|error| HostError::io("rechecking worktree stage ownership", error))?;
            if !staged.is_dir()
                || (staged.dev, staged.ino) != (journal.stage_device, journal.stage_inode)
            {
                return Err(HostError::Changed(
                    "the worktree stage was replaced".to_owned(),
                ));
            }
            if at_target.meta.is_some() {
                return Err(HostError::Changed(
                    "another writer created the worktree destination".to_owned(),
                ));
            }
        }
    }
    if !journal.created && !journal.installed {
        if !stage_path.join(".git").exists() {
            let args = words(&[
                "worktree",
                "add",
                "--no-checkout",
                "-b",
                &request.choice.branch,
                "--",
                &stage_path.to_string_lossy(),
                &journal.base,
            ]);
            git(tool, files, states, &repository, &args).await?;
        }
        verify_head(
            tool,
            files,
            states,
            &stage_path,
            &journal,
            &request.choice.branch,
        )
        .await?;
        journal.created = true;
        save(&journal_path, &journal)?;
    }
    if !journal.installed {
        let stage = parent
            .open_child(OsStr::new(&journal.stage))
            .map_err(|error| HostError::io("holding worktree metadata", error))?;
        let link = stage
            .open_read(OsStr::new(".git"))
            .map_err(|error| HostError::io("checking the owned worktree metadata link", error))?;
        let hash = crate::blobs::hash_file(&link)
            .map_err(|error| HostError::io("checking the owned worktree metadata link", error))?;
        if let Some(expected) = journal.git_file {
            if hash != expected {
                return Err(HostError::Changed(
                    "the worktree's repository metadata link changed".into(),
                ));
            }
        } else {
            journal.git_file = Some(hash);
            save(&journal_path, &journal)?;
        }
    }
    if !journal.populated && !journal.installed {
        let tree = text(
            &git(
                tool,
                files,
                states,
                &stage_path,
                &words(&[
                    "rev-parse",
                    "--verify",
                    &format!("{}^{{tree}}", journal.base),
                ]),
            )
            .await?,
        )?;
        if !journal.indexed {
            let index = text(
                &git(
                    tool,
                    files,
                    states,
                    &stage_path,
                    &words(&["rev-parse", "--git-path", "index"]),
                )
                .await?,
            )?;
            if stage_path.join(index).exists() {
                let current =
                    text(&git(tool, files, states, &stage_path, &words(&["write-tree"])).await?)?;
                if current != tree {
                    return Err(HostError::Changed(
                        "the new worktree index changed before initialization".to_owned(),
                    ));
                }
            } else {
                git(
                    tool,
                    files,
                    states,
                    &stage_path,
                    &words(&["read-tree", &journal.base]),
                )
                .await?;
            }
            journal.indexed = true;
            save(&journal_path, &journal)?;
        } else if text(&git(tool, files, states, &stage_path, &words(&["write-tree"])).await?)?
            != tree
        {
            return Err(HostError::Changed(
                "another writer staged paths while the worktree was being prepared".to_owned(),
            ));
        }
        match &request.choice.from {
            WorktreeFrom::Commit { .. } => {
                let empty = text(
                    &run(
                        tool,
                        files,
                        states.store().root(),
                        &stage_path,
                        &words(&["hash-object", "-w", "-t", "tree", "--stdin"]),
                        RunOptions {
                            input: Some(&[]),
                            ..Default::default()
                        },
                    )
                    .await?
                    .success()?,
                )?;
                if !tool.has_attribute_source() {
                    return Err(HostError::Unsupported(
                        "protected worktree checkout requires Git 2.43".to_owned(),
                    ));
                }
                let args = words(&[
                    &format!("--attr-source={empty}"),
                    "-c",
                    "core.autocrlf=false",
                    "checkout-index",
                    "--all",
                ]);
                let checked = run(
                    tool,
                    files,
                    states.store().root(),
                    &stage_path,
                    &args,
                    RunOptions::default(),
                )
                .await?;
                // Without --force, replay fills missing files but cannot erase a later writer.
                if !matches!(checked.code, Some(0 | 1)) {
                    checked.success()?;
                }
                let changed = git(
                    tool,
                    files,
                    states,
                    &stage_path,
                    &words(&[
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--exit-code",
                        "HEAD",
                        "--",
                    ]),
                )
                .await?;
                if !changed.is_empty() {
                    return Err(HostError::Changed(
                        "the worktree files changed while being prepared".to_owned(),
                    ));
                }
                journal.contents = Some(
                    commit_entries(tool, files, states, &stage_path, &journal.base, false).await?,
                );
            }
            WorktreeFrom::Now { .. } => {
                let snapshot = request.snapshot_id.ok_or_else(|| {
                    HostError::Unsupported("From now needs a complete recorded state".to_owned())
                })?;
                let read = states.read_recorded_recovery(
                    files,
                    &crate::index::Index::new(orch_link_contract::commands::IndexMode::Live, 0),
                    &FsRecordRead {
                        environment_id: request.environment_id,
                        snapshot_id: snapshot,
                        path: None,
                        export: false,
                    },
                    request.job_id,
                    now,
                )?;
                if read.root.directory != request.choice.repository_dir {
                    return Err(HostError::Changed(
                        "the current files belong to another directory".to_owned(),
                    ));
                }
                if read
                    .root
                    .repository_head
                    .as_ref()
                    .is_some_and(|head| head != &journal.base)
                {
                    return Err(HostError::Changed(
                        "the recorded current files and the worktree base name different commits"
                            .to_owned(),
                    ));
                }
                // Each object is verified before the first working-file effect.
                for entry in read
                    .entries
                    .iter()
                    .filter(|entry| matches!(entry.t, EntryKind::File | EntryKind::Link))
                {
                    states.store().read_entry(&read.root, entry)?;
                }
                let mut wanted = std::collections::BTreeMap::new();
                for entry in &read.entries {
                    wanted.insert(entry.p.clone(), entry.clone());
                    let mut ancestor = files.parse(&entry.p)?.parent();
                    while let Some(path) = ancestor.filter(|path| !path.is_root()) {
                        wanted.entry(path.to_string()).or_insert_with(|| {
                            crate::manifest::ManifestEntry {
                                p: path.to_string(),
                                t: EntryKind::Dir,
                                s: 0,
                                m: 0,
                                x: 0o700,
                                h: None,
                                l: None,
                            }
                        });
                        ancestor = path.parent();
                    }
                }
                wanted.insert(
                    ".git".into(),
                    crate::manifest::ManifestEntry {
                        p: ".git".into(),
                        t: EntryKind::File,
                        s: 0,
                        m: 0,
                        x: 0,
                        h: journal.git_file,
                        l: None,
                    },
                );
                let staged = parent
                    .open_child(OsStr::new(&journal.stage))
                    .map_err(|error| HostError::io("checking the entire worktree stage", error))?;
                crate::exact_materialize::inventory(&staged, "", &wanted, false)?;
                for entry in &read.entries {
                    let relative = files.parse(&entry.p)?;
                    if relative.is_root() || relative.has_component(".git") {
                        return Err(HostError::Changed(
                            "the recorded worktree contains repository metadata".to_owned(),
                        ));
                    }
                    let path = staging.append(&relative)?;
                    if let Some(ancestor) = path.parent() {
                        files.make_dir(&ancestor)?;
                    }
                    let target = files.root().locate(&path, Locate::ENTRY)?;
                    if let Some(meta) = &target.meta {
                        let same = match entry.t {
                            EntryKind::Dir => meta.is_dir(),
                            EntryKind::File if meta.is_file() => {
                                let file =
                                    target.parent.open_read(target.name()?).map_err(|error| {
                                        HostError::io("checking an existing worktree file", error)
                                    })?;
                                crate::blobs::hash_file(&file).map_err(|error| {
                                    HostError::io("checking worktree bytes", error)
                                })? == entry.h.ok_or_else(|| {
                                    HostError::Unsupported("recorded bytes have no hash".to_owned())
                                })? && (meta.mode & 0o111) == (entry.x & 0o111)
                            }
                            EntryKind::Link if meta.is_symlink() => {
                                target
                                    .parent
                                    .read_link(target.name()?)
                                    .map_err(|error| {
                                        HostError::io("checking worktree link text", error)
                                    })?
                                    .to_str()
                                    == entry.l.as_deref()
                            }
                            _ => false,
                        };
                        if same {
                            continue;
                        }
                        return Err(HostError::Changed(
                            "another writer changed a worktree path during preparation".to_owned(),
                        ));
                    }
                    match entry.t {
                        EntryKind::Dir => {
                            target
                                .parent
                                .mkdir(target.name()?, 0o777)
                                .map_err(|error| {
                                    HostError::io("making a recorded worktree directory", error)
                                })?;
                        }
                        EntryKind::Link => {
                            target
                                .parent
                                .symlink(
                                    OsStr::new(entry.l.as_deref().ok_or_else(|| {
                                        HostError::Unsupported(
                                            "recorded link text is unavailable".to_owned(),
                                        )
                                    })?),
                                    target.name()?,
                                )
                                .map_err(|error| {
                                    HostError::io("making a recorded worktree link", error)
                                })?;
                        }
                        EntryKind::File => {
                            let mut raw = states.store().read_entry(&read.root, entry)?;
                            std::io::Seek::rewind(raw.as_file_mut()).map_err(|error| {
                                HostError::io("reading verified worktree bytes", error)
                            })?;
                            let name = target.name()?.to_owned();
                            let temporary = OsString::from(format!(
                                ".orch-tmp-worktree-bytes-{}",
                                orch_types::new_id()
                            ));
                            let mut file = target
                                .parent
                                .create_new(&temporary, 0o666 | (entry.x & 0o111))
                                .map_err(|error| {
                                    HostError::io("staging a recorded worktree file", error)
                                })?;
                            let meta = platform::file_meta(&file).map_err(|error| {
                                HostError::io("checking staged worktree bytes", error)
                            })?;
                            let held = Temporary {
                                directory: target.parent,
                                name: temporary,
                                device: meta.dev,
                                inode: meta.ino,
                            };
                            std::io::copy(raw.as_file_mut(), &mut file)
                                .and_then(|_| file.sync_all())
                                .map_err(|error| {
                                    HostError::io("copying recorded worktree bytes", error)
                                })?;
                            held.directory
                                .rename_to(&held.name, &held.directory, &name, false)
                                .map_err(|error| {
                                    HostError::io("publishing one recorded worktree file", error)
                                })?;
                        }
                        EntryKind::Other => {
                            return Err(HostError::Unsupported(
                                "a special file cannot be carried as current files".to_owned(),
                            ));
                        }
                    }
                }
                crate::exact_materialize::inventory(&staged, "", &wanted, true)?;
            }
        }
        journal.populated = true;
        save(&journal_path, &journal)?;
    }
    if !journal.installed {
        let staged = parent
            .open_child(OsStr::new(&journal.stage))
            .map_err(|error| HostError::io("holding the prepared worktree", error))?;
        verify_contents(
            tool,
            files,
            states,
            CheckoutProof {
                request,
                journal: &journal,
                path: &stage_path,
                directory: &staged,
                now,
                complete: true,
            },
        )
        .await?;
        git(
            tool,
            files,
            states,
            &repository,
            &words(&[
                "worktree",
                "move",
                "--",
                &stage_path.to_string_lossy(),
                &directory.under(files.root().root_path()).to_string_lossy(),
            ]),
        )
        .await?;
        journal.installed = true;
        save(&journal_path, &journal)?;
    }
    let destination = directory.under(files.root().root_path());
    verify_head(
        tool,
        files,
        states,
        &destination,
        &journal,
        &request.choice.branch,
    )
    .await?;
    if journal.installed_git_file.is_none() {
        let located = files.root().locate(&directory, Locate::THROUGH)?;
        let held = files.root().open_dir(&located)?;
        let link = held
            .open_read(OsStr::new(".git"))
            .map_err(|error| HostError::io("keeping the installed worktree identity", error))?;
        journal.installed_git_file =
            Some(crate::blobs::hash_file(&link).map_err(|error| {
                HostError::io("keeping the installed worktree identity", error)
            })?);
        save(&journal_path, &journal)?;
    }
    let remote = run(
        tool,
        files,
        states.store().root(),
        &destination,
        &words(&["config", "--get", "remote.origin.url"]),
        RunOptions::default(),
    )
    .await?;
    let remote = if remote.code == Some(0) {
        crate::git::safe_remote_url(&text(&remote.stdout)?)
    } else {
        None
    };
    let result = SourceInfo {
        kind: SourceKind::Worktree,
        directory: request.choice.directory.clone(),
        repository_url: remote,
        branch: Some(request.choice.branch.clone()),
        head: Some(journal.base.clone()),
        state: SourceState::Ready,
        job: None,
        reason: None,
    };
    journal.result = Some(result.clone());
    save(&journal_path, &journal)?;
    states.invalidate("not_ready");
    Ok(result)
}

/// Cleans up the original job's worktree only while its base, index and raw files are unchanged.
pub async fn cleanup(
    tool: &GitTool,
    files: &Files,
    states: &FileStates,
    request: &GitWorktree,
    generation: i64,
    now: Time,
) -> Result<GitWorktreeCleanupResult> {
    files.gates().require_remote_write()?;
    let _held = states.action_gate().await;
    let kept = |reason: &str| GitWorktreeCleanupResult {
        removed: false,
        reason: Some(reason.to_owned()),
    };
    let journal_path = states
        .store()
        .root()
        .join("worktree-jobs")
        .join(format!("{}.json", request.job_id));
    if !journal_path.exists() {
        return Ok(GitWorktreeCleanupResult {
            removed: true,
            reason: None,
        });
    }
    let mut journal: Journal = serde_json::from_slice(&read_bounded(&journal_path, READ_BOUND)?)
        .map_err(|_| {
            HostError::Unsupported("the worktree cleanup journal is unavailable".into())
        })?;
    let request_hash = Hash::of(
        &serde_json::to_vec(request).map_err(|error| HostError::Internal(error.to_string()))?,
    );
    if journal.request != request_hash || journal.generation != generation {
        return Ok(kept("the original worktree request or generation changed"));
    }
    if journal.removed {
        return Ok(GitWorktreeCleanupResult {
            removed: true,
            reason: None,
        });
    }
    let directory = files.parse(&request.choice.directory)?;
    let parent = parent(files, &directory)?;
    let meta = parent
        .stat_self()
        .map_err(|error| HostError::io("holding the cleanup parent", error))?;
    if (meta.dev, meta.ino) != (journal.parent_device, journal.parent_inode) {
        return Ok(kept("the worktree parent changed; its files are kept"));
    }
    let source = files
        .parse(&request.choice.repository_dir)?
        .under(files.root().root_path());
    let (repository, _) = files.git_repository(tool, &source).await?.ok_or_else(|| {
        HostError::Changed("the source repository is unavailable for cleanup".into())
    })?;
    let branch = format!("refs/heads/{}", request.choice.branch);
    let stage = directory
        .parent()
        .unwrap_or_else(RelPath::root)
        .join(&journal.stage)?;
    let found = [stage, directory.clone()].into_iter().find(|path| {
        files.root().locate(path, Locate::ENTRY).is_ok_and(|entry| {
            entry.meta.is_some_and(|meta| {
                meta.is_dir() && (meta.dev, meta.ino) == (journal.stage_device, journal.stage_inode)
            })
        })
    });
    let Some(path) = found else {
        if !journal.removing {
            return Ok(kept(
                "the job's owned directory is gone or replaced; other files are kept",
            ));
        }
        let reference = run(
            tool,
            files,
            states.store().root(),
            &repository,
            &words(&["show-ref", "--verify", "--quiet", &branch]),
            RunOptions::default(),
        )
        .await?;
        if reference.code == Some(1) {
            journal.removed = true;
            save(&journal_path, &journal)?;
            return Ok(GitWorktreeCleanupResult {
                removed: true,
                reason: None,
            });
        }
        // The working directory was removed before the last answer. Only the exact old
        // branch may be removed; compare-and-delete cannot overwrite a later commit.
        if !remove_branch(tool, files, states, &repository, &branch, &journal.base).await? {
            return Ok(kept("the worktree branch changed or is in use and is kept"));
        }
        journal.removed = true;
        save(&journal_path, &journal)?;
        return Ok(GitWorktreeCleanupResult {
            removed: true,
            reason: None,
        });
    };
    let located = files.root().locate(&path, Locate::THROUGH)?;
    let held = files.root().open_dir(&located)?;
    let working = path.under(files.root().root_path());
    if path == directory {
        let Some(expected) = journal.installed_git_file else {
            return Ok(kept(
                "the installed worktree's original metadata is unknown and is kept",
            ));
        };
        journal.git_file = Some(expected);
    }
    if !journal.created
        && held
            .names()
            .map_err(|error| HostError::io("checking a never-created stage", error))?
            .is_empty()
    {
        let branch_exists = run(
            tool,
            files,
            states.store().root(),
            &repository,
            &words(&["show-ref", "--verify", "--quiet", &branch]),
            RunOptions::default(),
        )
        .await?;
        if branch_exists.code != Some(1) {
            return Ok(kept(
                "a branch exists without this job's worktree proof and is kept",
            ));
        }
        parent
            .rmdir(OsStr::new(&journal.stage))
            .map_err(|error| HostError::io("removing the job's empty stage", error))?;
        journal.removed = true;
        save(&journal_path, &journal)?;
        return Ok(GitWorktreeCleanupResult {
            removed: true,
            reason: None,
        });
    }
    if let Err(error) = verify_contents(
        tool,
        files,
        states,
        CheckoutProof {
            request,
            journal: &journal,
            path: &working,
            directory: &held,
            now,
            complete: journal.populated,
        },
    )
    .await
    {
        if matches!(
            error,
            HostError::Changed(_) | HostError::Ignored(_) | HostError::NotFound(_)
        ) {
            return Ok(kept(
                "another writer changed the worktree; its branch and files are kept",
            ));
        }
        return Err(error);
    }
    let current = files.root().locate(&path, Locate::ENTRY)?;
    if !current.meta.is_some_and(|meta| {
        meta.is_dir() && (meta.dev, meta.ino) == (journal.stage_device, journal.stage_inode)
    }) {
        return Ok(kept(
            "the worktree directory was replaced during cleanup and is kept",
        ));
    }
    journal.removing = true;
    save(&journal_path, &journal)?;
    // The normal remove refuses Now's intentional uncommitted bytes. Force is confined to
    // the proven job-owned directory after the complete raw namespace check above.
    git(
        tool,
        files,
        states,
        &repository,
        &words(&[
            "worktree",
            "remove",
            "--force",
            "--",
            &working.to_string_lossy(),
        ]),
    )
    .await?;
    if !remove_branch(tool, files, states, &repository, &branch, &journal.base).await? {
        return Ok(kept("the worktree branch changed or is in use and is kept"));
    }
    journal.removed = true;
    save(&journal_path, &journal)?;
    states.invalidate("not_ready");
    Ok(GitWorktreeCleanupResult {
        removed: true,
        reason: None,
    })
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
    use crate::index::{HashCache, Index};
    use crate::paths::{PathStyle, Workspace};
    use crate::state_store::ObjectStore;
    use orch_api_contract::source::{WorktreeChoice, WorktreeNow};
    use orch_link_contract::source::{FsRecord, RecordKind};
    use orch_types::new_id;
    use std::sync::Arc;

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

    fn fixture() -> (tempfile::TempDir, Files, FileStates, GitTool) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let repository = workspace.join("repo");
        std::fs::create_dir_all(&repository).unwrap();
        let files = Files::workspace(
            Workspace::open(&workspace, &[], PathStyle::Posix, true).unwrap(),
            Arc::new(IgnoreRules::new(&[], true)),
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        );
        let states =
            FileStates::new(ObjectStore::open(&dir.path().join("kept"), &workspace).unwrap());
        let tool = GitTool::discover();
        native(&tool, &repository, &["init", "-b", "work"]);
        std::fs::write(repository.join("code"), "committed\n").unwrap();
        std::fs::create_dir_all(repository.join("src/nested")).unwrap();
        std::fs::write(
            repository.join("src/nested/code.rs"),
            "nested committed bytes\n",
        )
        .unwrap();
        std::fs::write(repository.join(".gitignore"), "secret\n").unwrap();
        native(&tool, &repository, &["add", "."]);
        native(
            &tool,
            &repository,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@orch.invalid",
                "commit",
                "-m",
                "base",
            ],
        );
        std::fs::write(repository.join("code"), "staged\n").unwrap();
        native(&tool, &repository, &["add", "code"]);
        std::fs::write(repository.join("code"), "current dirty bytes\n").unwrap();
        std::fs::write(repository.join("binary"), [0, 255, 1]).unwrap();
        std::fs::write(repository.join("secret"), "IGNORED_SECRET").unwrap();
        std::fs::create_dir(repository.join("empty")).unwrap();
        std::os::unix::fs::symlink("code", repository.join("raw-link")).unwrap();
        (dir, files, states, tool)
    }

    fn now() -> Time {
        "2026-10-14T18:00:00.000Z".parse().unwrap()
    }

    #[tokio::test]
    async fn current_attempt_has_own_files_head_and_index_while_original_staging_and_config_are_kept()
     {
        let (_dir, files, states, tool) = fixture();
        let original = files.root().root_path().join("repo");
        let index_before = std::fs::read(original.join(".git/index")).unwrap();
        let head_before = std::fs::read(original.join(".git/HEAD")).unwrap();
        let config_before = std::fs::read(original.join(".git/config")).unwrap();
        let index = Index::new(
            orch_link_contract::commands::IndexMode::Full,
            Limits::default().index_max_entries,
        );
        states.request(
            &files.parse("repo").unwrap(),
            crate::file_states::DEFAULT_RECORD_MAX_BYTES,
        );
        index.scan(files.root(), files.ignore().unwrap()).unwrap();
        states.refresh(&files, &index, None, true).unwrap();
        let environment = new_id();
        let snapshot = new_id();
        let recorded = states
            .record(
                &files,
                &FsRecord {
                    environment_id: environment,
                    snapshot_id: snapshot,
                    directory: "repo".to_owned(),
                    kind: RecordKind::Turn,
                    record_days: 7,
                    max_bytes: crate::file_states::DEFAULT_RECORD_MAX_BYTES,
                },
                1,
                now(),
            )
            .unwrap();
        assert_eq!(
            recorded.state,
            orch_link_contract::source::RecordState::Complete
        );
        assert!(
            recorded.repository_head.is_some(),
            "raw local metadata supplies the exact base without a Git observation"
        );
        let request = GitWorktree {
            environment_id: environment,
            job_id: new_id(),
            choice: WorktreeChoice {
                repository_dir: "repo".to_owned(),
                branch: "attempt/second".to_owned(),
                from: WorktreeFrom::Now {
                    state: WorktreeNow::Now,
                },
                directory: "attempt".to_owned(),
            },
            snapshot_id: Some(snapshot),
        };
        let result = create(&tool, &files, &states, &request, 1, now())
            .await
            .unwrap();
        let attempt = files.root().root_path().join("attempt");
        assert_eq!(
            std::fs::read(attempt.join("code")).unwrap(),
            b"current dirty bytes\n"
        );
        assert_eq!(std::fs::read(attempt.join("binary")).unwrap(), [0, 255, 1]);
        assert_eq!(
            std::fs::read(attempt.join("src/nested/code.rs")).unwrap(),
            b"nested committed bytes\n"
        );
        assert_eq!(
            std::fs::read_link(attempt.join("raw-link")).unwrap(),
            std::path::PathBuf::from("code")
        );
        assert!(attempt.join("empty").is_dir());
        assert!(!attempt.join("secret").exists());
        assert_eq!(
            std::fs::read(original.join(".git/index")).unwrap(),
            index_before
        );
        assert_eq!(
            std::fs::read(original.join(".git/HEAD")).unwrap(),
            head_before
        );
        assert_eq!(
            std::fs::read(original.join(".git/config")).unwrap(),
            config_before
        );
        assert_eq!(
            std::fs::read(original.join("code")).unwrap(),
            b"current dirty bytes\n"
        );
        assert_eq!(
            text(&native(
                &tool,
                &attempt,
                &["symbolic-ref", "--short", "HEAD"]
            ))
            .unwrap(),
            "attempt/second"
        );
        assert_eq!(native(&tool, &attempt, &["show", ":code"]), b"committed\n");
        assert_eq!(native(&tool, &original, &["show", ":code"]), b"staged\n");
        assert_eq!(
            create(&tool, &files, &states, &request, 1, now())
                .await
                .unwrap(),
            result
        );
        std::fs::write(attempt.join("code"), "later attempt").unwrap();
        assert_eq!(
            std::fs::read(original.join("code")).unwrap(),
            b"current dirty bytes\n"
        );
        native(&tool, &original, &["config", "shared.setting", "kept"]);
        assert_eq!(
            text(&native(
                &tool,
                &attempt,
                &["config", "--get", "shared.setting"]
            ))
            .unwrap(),
            "kept"
        );
    }

    #[tokio::test]
    async fn a_commit_attempt_does_not_carry_dirty_files_or_reuse_an_existing_branch_or_directory()
    {
        let (_dir, files, states, tool) = fixture();
        let original = files.root().root_path().join("repo");
        let commit = text(&native(&tool, &original, &["rev-parse", "HEAD"])).unwrap();
        let request = GitWorktree {
            environment_id: new_id(),
            job_id: new_id(),
            choice: WorktreeChoice {
                repository_dir: "repo".to_owned(),
                branch: "attempt/commit".to_owned(),
                from: WorktreeFrom::Commit { commit },
                directory: "from-commit".to_owned(),
            },
            snapshot_id: None,
        };
        create(&tool, &files, &states, &request, 1, now())
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(files.root().root_path().join("from-commit/code")).unwrap(),
            b"committed\n"
        );
        assert_eq!(
            std::fs::read(
                files
                    .root()
                    .root_path()
                    .join("from-commit/src/nested/code.rs")
            )
            .unwrap(),
            b"nested committed bytes\n"
        );
        assert!(!files.root().root_path().join("from-commit/binary").exists());
        assert_eq!(
            std::fs::read(original.join("code")).unwrap(),
            b"current dirty bytes\n"
        );
        let existing = GitWorktree {
            job_id: new_id(),
            ..request.clone()
        };
        assert!(matches!(
            create(&tool, &files, &states, &existing, 1, now()).await,
            Err(HostError::Changed(_))
        ));
        let owned = files.root().root_path().join("person");
        std::fs::create_dir(&owned).unwrap();
        std::fs::write(owned.join("keep"), "person's bytes").unwrap();
        let other = GitWorktree {
            job_id: new_id(),
            choice: WorktreeChoice {
                directory: "person".to_owned(),
                branch: "another".to_owned(),
                ..request.choice
            },
            ..request
        };
        assert!(matches!(
            create(&tool, &files, &states, &other, 1, now()).await,
            Err(HostError::Changed(_))
        ));
        assert_eq!(
            std::fs::read(owned.join("keep")).unwrap(),
            b"person's bytes"
        );
    }

    #[tokio::test]
    async fn a_retry_after_population_keeps_a_later_ignored_name_and_never_publishes_the_stage() {
        let (_dir, files, states, tool) = fixture();
        let original = files.root().root_path().join("repo");
        let request = GitWorktree {
            environment_id: new_id(),
            job_id: new_id(),
            choice: WorktreeChoice {
                repository_dir: "repo".into(),
                branch: "attempt/replay".into(),
                from: WorktreeFrom::Commit {
                    commit: text(&native(&tool, &original, &["rev-parse", "HEAD"])).unwrap(),
                },
                directory: "from-commit".into(),
            },
            snapshot_id: None,
        };
        create(&tool, &files, &states, &request, 1, now())
            .await
            .unwrap();
        let journal_path = states
            .store()
            .root()
            .join("worktree-jobs")
            .join(format!("{}.json", request.job_id));
        let mut journal: Journal =
            serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
        let stage = files.root().root_path().join(&journal.stage);
        let destination = files.root().root_path().join(&request.choice.directory);
        native(
            &tool,
            &original,
            &[
                "worktree",
                "move",
                destination.to_str().unwrap(),
                stage.to_str().unwrap(),
            ],
        );
        journal.installed = false;
        journal.result = None;
        journal.git_file = Some(Hash::of(&std::fs::read(stage.join(".git")).unwrap()));
        save(&journal_path, &journal).unwrap();
        std::fs::write(stage.join("secret"), "a later writer's ignored bytes").unwrap();
        assert!(matches!(
            create(&tool, &files, &states, &request, 1, now()).await,
            Err(HostError::Changed(_))
        ));
        assert!(
            !destination.exists(),
            "no worktree is published after a namespace change"
        );
        assert_eq!(
            std::fs::read(stage.join("secret")).unwrap(),
            b"a later writer's ignored bytes"
        );
        assert_eq!(
            std::fs::read(original.join("code")).unwrap(),
            b"current dirty bytes\n"
        );
    }

    #[tokio::test]
    async fn a_retry_after_population_refuses_changed_bytes_index_and_executable_bits() {
        for changed in ["bytes", "index", "mode"] {
            let (_dir, files, states, tool) = fixture();
            let original = files.root().root_path().join("repo");
            let request = GitWorktree {
                environment_id: new_id(),
                job_id: new_id(),
                choice: WorktreeChoice {
                    repository_dir: "repo".into(),
                    branch: "attempt/replay".into(),
                    from: WorktreeFrom::Commit {
                        commit: text(&native(&tool, &original, &["rev-parse", "HEAD"])).unwrap(),
                    },
                    directory: "from-commit".into(),
                },
                snapshot_id: None,
            };
            create(&tool, &files, &states, &request, 1, now())
                .await
                .unwrap();
            let journal_path = states
                .store()
                .root()
                .join("worktree-jobs")
                .join(format!("{}.json", request.job_id));
            let mut journal: Journal =
                serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
            let stage = files.root().root_path().join(&journal.stage);
            let destination = files.root().root_path().join(&request.choice.directory);
            native(
                &tool,
                &original,
                &[
                    "worktree",
                    "move",
                    destination.to_str().unwrap(),
                    stage.to_str().unwrap(),
                ],
            );
            journal.installed = false;
            journal.result = None;
            journal.git_file = Some(Hash::of(&std::fs::read(stage.join(".git")).unwrap()));
            save(&journal_path, &journal).unwrap();
            match changed {
                "mode" => {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        stage.join("code"),
                        std::fs::Permissions::from_mode(0o755),
                    )
                    .unwrap();
                }
                "index" => {
                    std::fs::write(stage.join("later"), "later staging").unwrap();
                    native(&tool, &stage, &["add", "later"]);
                }
                _ => std::fs::write(stage.join("code"), "later bytes").unwrap(),
            }
            assert!(
                matches!(
                    create(&tool, &files, &states, &request, 1, now()).await,
                    Err(HostError::Changed(_))
                ),
                "{changed}"
            );
            assert!(!destination.exists(), "{changed}");
            assert!(stage.join("code").exists(), "later work is left in place");
        }
    }

    #[tokio::test]
    async fn failed_attempt_cleanup_removes_its_unchanged_worktree_and_branch_but_keeps_the_original_staging()
     {
        let (_dir, files, states, tool) = fixture();
        let original = files.root().root_path().join("repo");
        let index = std::fs::read(original.join(".git/index")).unwrap();
        let config = std::fs::read(original.join(".git/config")).unwrap();
        let request = GitWorktree {
            environment_id: new_id(),
            job_id: new_id(),
            choice: WorktreeChoice {
                repository_dir: "repo".into(),
                branch: "attempt/cleanup".into(),
                from: WorktreeFrom::Commit {
                    commit: text(&native(&tool, &original, &["rev-parse", "HEAD"])).unwrap(),
                },
                directory: "attempt".into(),
            },
            snapshot_id: None,
        };
        create(&tool, &files, &states, &request, 1, now())
            .await
            .unwrap();
        assert!(
            cleanup(&tool, &files, &states, &request, 1, now())
                .await
                .unwrap()
                .removed
        );
        assert!(!files.root().root_path().join("attempt").exists());
        let reference = std::process::Command::new(tool.program().unwrap())
            .current_dir(&original)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                "refs/heads/attempt/cleanup",
            ])
            .status()
            .unwrap();
        assert_eq!(
            reference.code(),
            Some(1),
            "only this job's new branch was removed"
        );
        assert_eq!(std::fs::read(original.join(".git/index")).unwrap(), index);
        assert_eq!(std::fs::read(original.join(".git/config")).unwrap(), config);
        assert_eq!(
            std::fs::read(original.join("code")).unwrap(),
            b"current dirty bytes\n"
        );
        assert!(
            cleanup(&tool, &files, &states, &request, 1, now())
                .await
                .unwrap()
                .removed,
            "cleanup replay returns its first outcome"
        );
    }

    #[tokio::test]
    async fn cleanup_keeps_later_worktree_bytes_new_commits_and_a_replaced_directory() {
        for changed in ["bytes", "commit", "directory"] {
            let (_dir, files, states, tool) = fixture();
            let original = files.root().root_path().join("repo");
            let request = GitWorktree {
                environment_id: new_id(),
                job_id: new_id(),
                choice: WorktreeChoice {
                    repository_dir: "repo".into(),
                    branch: "attempt/keep".into(),
                    from: WorktreeFrom::Commit {
                        commit: text(&native(&tool, &original, &["rev-parse", "HEAD"])).unwrap(),
                    },
                    directory: "attempt".into(),
                },
                snapshot_id: None,
            };
            create(&tool, &files, &states, &request, 1, now())
                .await
                .unwrap();
            let attempt = files.root().root_path().join("attempt");
            match changed {
                "commit" => {
                    std::fs::write(attempt.join("code"), "later commit").unwrap();
                    native(&tool, &attempt, &["add", "code"]);
                    native(
                        &tool,
                        &attempt,
                        &[
                            "-c",
                            "user.name=Fixture",
                            "-c",
                            "user.email=fixture@orch.invalid",
                            "commit",
                            "-m",
                            "later",
                        ],
                    );
                }
                "directory" => {
                    std::fs::rename(
                        &attempt,
                        files.root().root_path().join("person-kept-attempt"),
                    )
                    .unwrap();
                    std::fs::create_dir(&attempt).unwrap();
                    std::fs::write(attempt.join("code"), "replacement bytes").unwrap();
                }
                _ => std::fs::write(attempt.join("code"), "later bytes").unwrap(),
            }
            let before = std::fs::read(attempt.join("code")).unwrap();
            let result = cleanup(&tool, &files, &states, &request, 1, now())
                .await
                .unwrap();
            assert!(!result.removed, "{changed}");
            assert!(result.reason.is_some(), "the job explains what was kept");
            assert_eq!(
                std::fs::read(attempt.join("code")).unwrap(),
                before,
                "{changed}"
            );
            native(
                &tool,
                &original,
                &["show-ref", "--verify", "refs/heads/attempt/keep"],
            );
        }
    }
}
