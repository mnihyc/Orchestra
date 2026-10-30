//! Copied Git metadata keeps each checkout's files and repository connected.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use orch_types::Hash;

use crate::error::{HostError, Result};
use crate::files::Files;
use crate::manifest::{self, EntryKind, ManifestEntry, Parsed, Resolved};
use crate::paths::{Locate, RelPath};
use crate::platform;

// Git's connection files contain one path, bounded by the workspace's portable path limit.
const POINTER_MAX_BYTES: u64 = crate::paths::PATH_MAX_BYTES as u64 + 32;
// Configuration uses the protected Git reader's existing eight-mebibyte reply bound.
const CONFIG_MAX_BYTES: u64 = 8 * 1024 * 1024;
// core.worktree is relative to the Git directory, so its parent is this independent checkout.
const INDEPENDENT_CONFIG: &[u8] = b"\n\n[core]\n\tworktree = ..\n\tbare = false\n";

/// Virtual names of an independent checkout retain the physical source of every raw byte.
pub(crate) struct Mapping {
    pub entries: Vec<ManifestEntry>,
    pub sources: BTreeMap<String, RelPath>,
    pub inline: BTreeMap<Hash, Vec<u8>>,
    pub additional_files: i64,
    pub additional_bytes: i64,
}

fn metadata_selection(
    directory: &RelPath,
    private: bool,
) -> orch_link_contract::commands::Selection {
    let excluded: &[&str] = if private {
        &[
            "commondir",
            "gitdir",
            "locked",
            "config",
            "objects",
            "packed-refs",
            "shallow",
        ]
    } else {
        &[
            "worktrees",
            "HEAD",
            "index",
            "config.worktree",
            "logs/HEAD",
            "refs/bisect",
            "refs/worktree",
            "refs/rewritten",
            "objects/pack/*.promisor",
            "objects/info/http-alternates",
        ]
    };
    orch_link_contract::commands::Selection {
        include: vec![directory.to_string()],
        exclude: excluded
            .iter()
            .map(|path| format!("{directory}/{path}"))
            .collect(),
    }
}

fn configuration(files: &Files, path: &RelPath) -> Result<Vec<u8>> {
    let file = files.open_regular(path)?;
    let mut bytes = Vec::new();
    (&*file.file)
        .take(CONFIG_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| HostError::io("reading the independent checkout configuration", error))?;
    if bytes.len() as u64 > CONFIG_MAX_BYTES {
        return Err(HostError::Unsupported(
            "the independent checkout configuration exceeds the read bound".into(),
        ));
    }
    bytes.extend_from_slice(INDEPENDENT_CONFIG);
    Ok(bytes)
}

/// Selects the worktree's own HEAD/index and the common objects/refs into an ordinary .git directory.
pub(crate) fn independent(
    files: &Files,
    selection: &orch_link_contract::commands::Selection,
    directory: &str,
) -> Result<Option<Mapping>> {
    let directory = files.parse(directory)?;
    let git = directory.join(".git")?;
    let resolved = manifest::resolve(files.root(), selection)?;
    if !resolved.included(&git) || resolved.excluded(&git) {
        return Ok(None);
    }
    let located = files.root().locate(&git, Locate::ENTRY)?;
    if !located.meta.as_ref().is_some_and(|meta| meta.is_file()) {
        return Ok(None);
    }
    match files.check_visible(&git, &located, false) {
        Ok(()) => {}
        Err(HostError::Ignored(_)) => return Ok(None),
        Err(error) => return Err(error),
    }
    let (bytes, _) = pointer(&located.parent, located.name()?)?;
    let named = text(&bytes)?.strip_prefix("gitdir: ").ok_or_else(|| {
        HostError::Unsupported("the worktree has no repository connection".into())
    })?;
    let metadata = relative(
        files,
        &normalize(&directory.under(files.root().root_path()).join(named))?,
    )?;
    let located = files.root().locate(&metadata, Locate::THROUGH)?;
    if located.real != metadata {
        return Err(HostError::Changed(
            "the worktree's metadata follows another directory".into(),
        ));
    }
    let held = files.root().open_dir(&located)?;
    let common_text = pointer(&held, OsStr::new("commondir"))?.0;
    let common = relative(
        files,
        &normalize(
            &metadata
                .under(files.root().root_path())
                .join(text(&common_text)?),
        )?,
    )?;
    let common_located = files.root().locate(&common, Locate::THROUGH)?;
    if common_located.real != common {
        return Err(HostError::Changed(
            "the common repository follows another directory".into(),
        ));
    }
    let common_dir = files.root().open_dir(&common_located)?;
    common_dir
        .open_child(OsStr::new("objects"))
        .map_err(|error| HostError::io("checking the independent objects", error))?;
    common_dir
        .open_child(OsStr::new("refs"))
        .map_err(|error| HostError::io("checking the independent refs", error))?;
    let alternates = common.append(&files.parse("objects/info/alternates")?)?;
    let alternates = match files.root().locate(&alternates, Locate::ENTRY) {
        Ok(entry) => entry.meta,
        Err(HostError::NotFound(_)) => None,
        Err(error) => return Err(error),
    };
    if alternates.is_some_and(|meta| meta.size > 0 || !meta.is_file()) {
        return Err(HostError::Unsupported("this repository borrows objects outside its own store; copy its complete repository first".into()));
    }
    let mut entries = BTreeMap::new();
    let mut sources = BTreeMap::new();
    let mut inline = BTreeMap::new();
    let mut total_entries = 0i64;
    for (root, private) in [(&common, false), (&metadata, true)] {
        let selected = metadata_selection(root, private);
        let measured = crate::measure::selection(files, &selected)?;
        total_entries = total_entries.saturating_add(measured.entry_count);
        if !measured.complete
            || total_entries
                > i64::try_from(orch_link_contract::commands::SNAPSHOT_MAX_ENTRIES)
                    .unwrap_or(i64::MAX)
        {
            return Err(HostError::Unsupported(
                "the independent repository must fit the bounded snapshot measurement".into(),
            ));
        }
        let resolved = manifest::resolve(files.root(), &selected)?;
        for mut entry in manifest::collect(files, &resolved)? {
            if !matches!(entry.t, EntryKind::Dir | EntryKind::File) {
                return Err(HostError::Unsupported("the independent repository has linked or special metadata that cannot be carried safely".into()));
            }
            let source = root.append(&files.parse(&entry.p)?)?;
            entry.p = format!(".git/{}", entry.p);
            if matches!(entry.p.as_str(), ".git/config" | ".git/config.worktree") {
                let bytes = configuration(files, &source)?;
                entry.h = Some(Hash::of(&bytes));
                entry.s = i64::try_from(bytes.len()).map_err(|_| {
                    HostError::Unsupported("the Git configuration is too large".into())
                })?;
                inline.insert(
                    entry.h.ok_or_else(|| {
                        HostError::Internal("the Git configuration has no content identity".into())
                    })?,
                    bytes,
                );
                sources.remove(&entry.p);
            } else {
                sources.insert(entry.p.clone(), source);
            }
            entries.insert(entry.p.clone(), entry);
        }
    }
    for required in [".git/HEAD", ".git/config"] {
        if !entries
            .get(required)
            .is_some_and(|entry| entry.t == EntryKind::File)
        {
            return Err(HostError::Ignored(
                "current rules leave out required independent repository metadata".into(),
            ));
        }
    }
    entries.insert(
        ".git".into(),
        ManifestEntry {
            p: ".git".into(),
            t: EntryKind::Dir,
            s: 0,
            m: 0,
            x: 0o755,
            h: None,
            l: None,
        },
    );
    let additional_files = i64::try_from(
        entries
            .values()
            .filter(|entry| entry.t == EntryKind::File)
            .count(),
    )
    .map_err(|_| HostError::Unsupported("too many independent files".into()))?
        - 1;
    let additional_bytes = entries
        .values()
        .filter(|entry| entry.t == EntryKind::File)
        .fold(0i64, |total, entry| total.saturating_add(entry.s))
        .saturating_sub(i64::try_from(bytes.len()).unwrap_or(i64::MAX));
    Ok(Some(Mapping {
        entries: entries.into_values().collect(),
        sources,
        inline,
        additional_files,
        additional_bytes,
    }))
}

/// Rebases virtual metadata under the original selected manifest root; source paths stay physical.
pub(crate) type MappedSources = (BTreeMap<String, RelPath>, BTreeMap<Hash, Vec<u8>>);

/// Rebases paths while retaining physical source files and generated metadata bytes.
pub(crate) fn map(
    mapping: Mapping,
    files: &Files,
    resolved: &Resolved,
    directory: &str,
    entries: &mut BTreeMap<String, ManifestEntry>,
) -> Result<MappedSources> {
    let directory = files
        .parse(directory)?
        .strip_prefix(&resolved.root)
        .ok_or_else(|| HostError::Changed("the worktree left the selected snapshot root".into()))?;
    let prefix = |path: &str| {
        if directory.is_root() {
            path.to_owned()
        } else {
            format!("{directory}/{path}")
        }
    };
    for mut entry in mapping.entries {
        entry.p = prefix(&entry.p);
        entries.insert(entry.p.clone(), entry);
    }
    Ok((
        mapping
            .sources
            .into_iter()
            .map(|(path, source)| (prefix(&path), source))
            .collect(),
        mapping.inline,
    ))
}

/// Proves an imported independent repository has every required ref and index object locally.
pub(crate) async fn verify_independent(
    tool: &crate::git::GitTool,
    files: &Files,
    states: &crate::file_states::FileStates,
    directory: &str,
) -> Result<()> {
    files.gates().require_remote_write()?;
    let directory = files.target_of(directory)?;
    let _held = states.action_gate().await;
    let located = files.root().locate(&directory, Locate::THROUGH)?;
    if located.real != directory {
        return Err(HostError::Changed(
            "the independent checkout moved through a link".into(),
        ));
    }
    let git = files
        .root()
        .locate(&directory.join(".git")?, Locate::ENTRY)?;
    if !git.meta.is_some_and(|meta| meta.is_dir()) {
        return Err(HostError::Changed(
            "the copied checkout has no independent repository".into(),
        ));
    }
    let (cwd, root) = files
        .git_repository(tool, &directory.under(files.root().root_path()))
        .await?
        .ok_or_else(|| {
            HostError::Unsupported(
                "the copied checkout cannot read its independent repository".into(),
            )
        })?;
    if root != directory {
        return Err(HostError::Changed(
            "the copied checkout names another repository".into(),
        ));
    }
    let args = ["fsck", "--no-reflogs", "--no-dangling"]
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
    crate::git_actions::run(
        tool,
        files,
        states.store().root(),
        &cwd,
        &args,
        crate::git_actions::RunOptions::default(),
    )
    .await?
    .success()?;
    Ok(())
}

fn normalize(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(HostError::Changed(
            "a copied Git root must be absolute".into(),
        ));
    }
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !result.pop() {
                    return Err(HostError::Changed(
                        "a copied Git path leaves its root".into(),
                    ));
                }
            }
            Component::CurDir => {}
            other => result.push(other.as_os_str()),
        }
    }
    Ok(result)
}

fn pointer(directory: &platform::Dir, name: &OsStr) -> Result<(Vec<u8>, Hash)> {
    let file = directory
        .open_read(name)
        .map_err(|error| HostError::io("reading a copied Git pointer", error))?;
    let mut bytes = Vec::new();
    file.take(POINTER_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| HostError::io("reading a copied Git pointer", error))?;
    if bytes.len() as u64 > POINTER_MAX_BYTES {
        return Err(HostError::Unsupported(
            "a copied Git pointer exceeds the path bound".into(),
        ));
    }
    let hash = Hash::of(&bytes);
    Ok((bytes, hash))
}

fn text(bytes: &[u8]) -> Result<&str> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| HostError::Unsupported("a copied Git pointer is not UTF-8".into()))?
        .trim_end_matches(['\r', '\n']);
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err(HostError::Unsupported(
            "a copied Git pointer has an invalid path".into(),
        ));
    }
    Ok(text)
}

fn relative(files: &Files, path: &Path) -> Result<RelPath> {
    files.parse(
        path.strip_prefix(files.root().root_path())
            .map_err(|_| {
                HostError::RootNotAllowed("the copied Git metadata leaves this workspace".into())
            })?
            .to_str()
            .ok_or_else(|| {
                HostError::Unsupported("the copied Git metadata path is not UTF-8".into())
            })?,
    )
}

struct Rewrite {
    path: RelPath,
    before: Hash,
    after: Vec<u8>,
}

fn rewrite(files: &Files, change: &Rewrite) -> Result<()> {
    let located = files.root().locate(&change.path, Locate::ENTRY)?;
    if located.real != change.path {
        return Err(HostError::Changed(
            "a copied Git pointer changed through a link".into(),
        ));
    }
    let (current, hash) = pointer(&located.parent, located.name()?)?;
    if current == change.after {
        return Ok(());
    }
    if hash != change.before {
        return Err(HostError::Changed(
            "another writer changed a copied Git pointer; it is kept".into(),
        ));
    }
    let name = OsStr::new(&format!(".orch-tmp-git-pointer-{}", orch_types::new_id())).to_owned();
    let mut output = located
        .parent
        .create_new(&name, located.meta.as_ref().map_or(0o600, |meta| meta.mode))
        .map_err(|error| HostError::io("staging the copied Git connection", error))?;
    let written = (|| {
        output
            .write_all(&change.after)
            .and_then(|_| output.sync_all())
            .map_err(|error| HostError::io("writing the copied Git connection", error))?;
        if pointer(&located.parent, located.name()?)?.1 != change.before {
            return Err(HostError::Changed(
                "the copied Git connection changed before publication".into(),
            ));
        }
        located
            .parent
            .rename_to(&name, &located.parent, located.name()?, true)
            .map_err(|error| HostError::io("publishing the copied Git connection", error))?;
        located
            .parent
            .sync()
            .map_err(|error| HostError::io("syncing the copied Git connection", error))
    })();
    if written.is_err() {
        let _ = located.parent.unlink(&name);
    }
    written
}

/// Repairs both ends of copied worktree connections without accessing the previous host's paths.
pub fn repair(
    files: &Files,
    parsed: &Parsed,
    previous_root: &str,
    target_root: &str,
) -> Result<i64> {
    files.gates().require_remote_write()?;
    let old_root = normalize(Path::new(previous_root))?;
    let target = files.target_of(target_root)?;
    let new_root = target.under(files.root().root_path());
    let entries = parsed
        .entries
        .iter()
        .map(|entry| (entry.p.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut rewrites = Vec::new();
    let mut repaired = 0i64;
    for entry in parsed.entries.iter().filter(|entry| {
        entry.t == EntryKind::File && Path::new(&entry.p).file_name() == Some(OsStr::new(".git"))
    }) {
        let local = files.parse(&entry.p)?;
        let path = target.append(&local)?;
        let located = files.root().locate(&path, Locate::ENTRY)?;
        if located.real != path {
            return Err(HostError::Changed(
                "the copied worktree follows another directory".into(),
            ));
        }
        files.check_visible(&path, &located, false)?;
        let (bytes, current_hash) = pointer(&located.parent, located.name()?)?;
        let named = text(&bytes)?.strip_prefix("gitdir: ").ok_or_else(|| {
            HostError::Unsupported("the copied worktree has no Git connection".into())
        })?;
        // Resolve lexically. Opening the old absolute path could act on the still-running
        // source when both environments are on one host.
        let current = normalize(
            &old_root
                .join(local.parent().unwrap_or_else(RelPath::root).as_str())
                .join(named),
        )?;
        let suffix = current
            .strip_prefix(&old_root)
            .or_else(|_| current.strip_prefix(&new_root))
            .map_err(|_| {
                HostError::RootNotAllowed(
                    "the copied worktree names a repository outside its copied root".into(),
                )
            })?;
        let suffix = files.parse(suffix.to_str().ok_or_else(|| {
            HostError::Unsupported("the copied worktree pointer is not UTF-8".into())
        })?)?;
        let git_directory = new_root.join(suffix.as_str());
        let git_relative = relative(files, &git_directory)?;
        let owned = files.root().locate(&git_relative, Locate::THROUGH)?;
        if owned.real != git_relative || !owned.meta.as_ref().is_some_and(|meta| meta.is_dir()) {
            return Err(HostError::Changed(
                "the copied worktree's repository metadata is missing or follows a link".into(),
            ));
        }
        let metadata = files.root().open_dir(&owned)?;
        let common_text = pointer(&metadata, OsStr::new("commondir"))?.0;
        let common = normalize(&git_directory.join(text(&common_text)?))?;
        let common = relative(files, &common)?;
        let common = files.root().locate(&common, Locate::THROUGH)?;
        let common = files.root().open_dir(&common)?;
        for part in ["objects", "refs"] {
            common
                .open_child(OsStr::new(part))
                .map_err(|error| HostError::io("checking the copied common repository", error))?;
        }
        let back_relative = git_relative.join("gitdir")?;
        let back_key = suffix.join("gitdir")?;
        let before = entries
            .get(back_key.as_str())
            .filter(|entry| entry.t == EntryKind::File)
            .and_then(|entry| entry.h)
            .ok_or_else(|| {
                HostError::Changed(
                    "the move did not include both ends of this worktree connection".into(),
                )
            })?;
        let after = format!("gitdir: {}\n", git_directory.to_string_lossy()).into_bytes();
        let original = entry.h.ok_or_else(|| {
            HostError::Changed("the copied worktree connection has no reviewed bytes".into())
        })?;
        if current_hash != original && bytes != after {
            return Err(HostError::Changed(
                "the copied worktree connection changed after copying".into(),
            ));
        }
        let back = files.root().locate(&back_relative, Locate::ENTRY)?;
        let (back_bytes, back_hash) = pointer(&back.parent, back.name()?)?;
        let back_after = format!(
            "{}\n",
            path.under(files.root().root_path()).to_string_lossy()
        )
        .into_bytes();
        let old_back = old_root.join(local.as_str());
        let named_back = normalize(Path::new(text(&back_bytes)?))?;
        if named_back != old_back && named_back != path.under(files.root().root_path()) {
            return Err(HostError::Changed(
                "the repository metadata names another worktree".into(),
            ));
        }
        if back_hash != before && back_bytes != back_after {
            return Err(HostError::Changed(
                "the copied repository connection changed after copying".into(),
            ));
        }
        rewrites.push(Rewrite {
            path,
            before: original,
            after,
        });
        rewrites.push(Rewrite {
            path: back_relative,
            before,
            after: back_after,
        });
        repaired += 1;
    }
    // No pointer changes until every pair is proven to be inside the copied namespace.
    for change in &rewrites {
        rewrite(files, change)?;
    }
    Ok(repaired)
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
    use crate::manifest;
    use crate::paths::{PathStyle, Workspace};
    use std::sync::Arc;

    fn files(root: &Path) -> Files {
        Files::workspace(
            Workspace::open(root, &[], PathStyle::Posix, true).unwrap(),
            Arc::new(IgnoreRules::new(&[], true)),
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        )
    }

    fn git(root: &Path, args: &[&str]) -> Vec<u8> {
        let result = std::process::Command::new("git")
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        result.stdout
    }

    fn copy(source: &Path, target: &Path) {
        std::fs::create_dir_all(target).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let path = target.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &path);
            } else {
                std::fs::copy(entry.path(), path).unwrap();
            }
        }
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, Parsed) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        std::fs::create_dir_all(source.join("repo")).unwrap();
        git(&source.join("repo"), &["init", "-b", "main"]);
        std::fs::write(source.join("repo/code"), "base\n").unwrap();
        git(&source.join("repo"), &["add", "code"]);
        git(
            &source.join("repo"),
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
        let stage = source.join(format!(".orch-tmp-worktree-{}", orch_types::new_id()));
        git(
            &source.join("repo"),
            &["worktree", "add", "-b", "attempt", stage.to_str().unwrap()],
        );
        git(
            &source.join("repo"),
            &[
                "worktree",
                "move",
                stage.to_str().unwrap(),
                source.join("attempt").to_str().unwrap(),
            ],
        );
        std::fs::write(source.join("attempt/code"), "staged\n").unwrap();
        git(&source.join("attempt"), &["add", "code"]);
        std::fs::write(source.join("attempt/code"), "current\n").unwrap();
        let files = files(&source);
        let selection = orch_link_contract::commands::Selection {
            include: vec![String::new()],
            exclude: Vec::new(),
        };
        let resolved = manifest::resolve(files.root(), &selection).unwrap();
        let mut entries = manifest::collect(&files, &resolved).unwrap();
        for entry in entries
            .iter_mut()
            .filter(|entry| entry.t == EntryKind::File)
        {
            entry.h = Some(Hash::of(&std::fs::read(source.join(&entry.p)).unwrap()));
        }
        let header = manifest::header_for(
            files.root(),
            &RelPath::root(),
            1,
            "2026-10-14T21:00:00.000Z"
                .parse::<orch_types::Time>()
                .unwrap()
                .get(),
        );
        let parsed = manifest::parse(&manifest::build(&header, entries).unwrap().bytes).unwrap();
        copy(&source, &target);
        (temp, source, target, parsed)
    }

    #[test]
    fn a_moved_worktree_keeps_its_head_index_dirty_files_and_common_config_at_the_new_root() {
        let (_temp, source, target, parsed) = fixture();
        let original_git = std::fs::read(source.join("attempt/.git")).unwrap();
        let original_index = git(
            &source.join("attempt"),
            &["rev-parse", "--git-path", "index"],
        );
        let original_index = text(&original_index).unwrap();
        let index_bytes = std::fs::read(original_index).unwrap();
        let config = std::fs::read(source.join("repo/.git/config")).unwrap();
        assert_eq!(
            repair(
                &files(&target),
                &parsed,
                source.to_str().unwrap(),
                target.to_str().unwrap()
            )
            .unwrap(),
            1
        );
        assert_eq!(
            git(
                &target.join("attempt"),
                &["symbolic-ref", "--short", "HEAD"]
            ),
            b"attempt\n"
        );
        assert_eq!(
            git(&target.join("attempt"), &["show", ":code"]),
            b"staged\n"
        );
        assert_eq!(
            std::fs::read(target.join("attempt/code")).unwrap(),
            b"current\n"
        );
        let target_index = git(
            &target.join("attempt"),
            &["rev-parse", "--git-path", "index"],
        );
        assert_eq!(
            std::fs::read(text(&target_index).unwrap()).unwrap(),
            index_bytes
        );
        assert_eq!(
            std::fs::read(target.join("repo/.git/config")).unwrap(),
            config
        );
        assert_eq!(
            std::fs::read(source.join("attempt/.git")).unwrap(),
            original_git,
            "the original host's pointer was never opened for writing"
        );
        assert_eq!(
            repair(
                &files(&target),
                &parsed,
                source.to_str().unwrap(),
                target.to_str().unwrap()
            )
            .unwrap(),
            1,
            "a lost reply rechecks the translated pair"
        );
        git(
            &target.join("repo"),
            &["config", "copied.setting", "target-only"],
        );
        assert_eq!(
            git(
                &target.join("attempt"),
                &["config", "--get", "copied.setting"]
            ),
            b"target-only\n"
        );
        assert_eq!(
            std::fs::read(source.join("repo/.git/config")).unwrap(),
            config
        );
    }

    #[test]
    fn missing_other_end_or_changed_pointer_refuses_before_repairing_any_connection() {
        for changed in ["missing", "later"] {
            let (_temp, source, target, mut parsed) = fixture();
            let before = std::fs::read(target.join("attempt/.git")).unwrap();
            if changed == "missing" {
                parsed.entries.retain(|entry| !entry.p.ends_with("/gitdir"));
            } else {
                std::fs::write(
                    target.join("attempt/.git"),
                    "gitdir: /outside/copied/namespace\n",
                )
                .unwrap();
            }
            assert!(
                repair(
                    &files(&target),
                    &parsed,
                    source.to_str().unwrap(),
                    target.to_str().unwrap()
                )
                .is_err()
            );
            if changed == "missing" {
                assert_eq!(std::fs::read(target.join("attempt/.git")).unwrap(), before);
            } else {
                assert_eq!(
                    std::fs::read(target.join("attempt/.git")).unwrap(),
                    b"gitdir: /outside/copied/namespace\n"
                );
            }
        }
    }

    #[tokio::test]
    async fn copying_a_worktree_out_keeps_its_staging_raw_files_and_refs_in_an_independent_repository()
     {
        let (temp, source, _copied, _parsed) = fixture();
        let marker = temp.path().join("hook-or-filter-ran");
        let program = format!("sh -c 'touch {}; cat'", marker.display());
        git(
            &source.join("repo"),
            &["config", "filter.copy-marker.clean", &program],
        );
        git(
            &source.join("repo"),
            &["config", "filter.copy-marker.smudge", &program],
        );
        std::fs::write(
            source.join("attempt/.gitattributes"),
            "code filter=copy-marker\n",
        )
        .unwrap();
        std::fs::write(source.join("attempt/binary"), [0, 255, 17]).unwrap();
        std::fs::create_dir_all(source.join("attempt/src/nested")).unwrap();
        std::fs::write(
            source.join("attempt/src/nested/code.rs"),
            "raw nested files\n",
        )
        .unwrap();
        std::fs::create_dir(source.join("attempt/empty")).unwrap();
        std::os::unix::fs::symlink("code", source.join("attempt/link")).unwrap();
        let index_path = git(
            &source.join("attempt"),
            &["rev-parse", "--git-path", "index"],
        );
        let index = std::fs::read(text(&index_path).unwrap()).unwrap();
        let config = std::fs::read(source.join("repo/.git/config")).unwrap();
        let head = std::fs::read(source.join("repo/.git/HEAD")).unwrap();
        let selected = orch_link_contract::commands::Selection {
            include: vec!["attempt".into()],
            exclude: Vec::new(),
        };
        let original = files(&source);
        let measured = crate::measure::selection(&original, &selected).unwrap();
        let extra = independent(&original, &selected, "attempt")
            .unwrap()
            .unwrap();
        assert!(
            extra.additional_bytes > 0 && extra.additional_files > 0,
            "the plan includes repository data"
        );
        let expected_bytes = measured.total_bytes + extra.additional_bytes;
        let blobs = crate::blobs::MemoryBlobs::new();
        let clock = orch_types::ManualClock::new(
            "2026-10-14T22:00:00.000Z"
                .parse::<orch_types::Time>()
                .unwrap()
                .get(),
        );
        let (report, converted) = original
            .snapshot_independent(
                &selected,
                "attempt",
                1,
                &clock,
                &blobs,
                &crate::snapshot::NoProgress,
            )
            .await
            .unwrap();
        assert!(converted);
        assert_eq!(report.result.missing, 0);
        assert_eq!(report.result.total_bytes, expected_bytes);
        let parsed =
            manifest::parse(&blobs.get_bytes(&report.result.manifest_blob).unwrap()).unwrap();
        assert!(
            parsed
                .entries
                .iter()
                .any(|entry| entry.p == ".git" && entry.t == EntryKind::Dir)
        );
        assert!(
            !parsed
                .entries
                .iter()
                .any(|entry| entry.p.starts_with(".git/worktrees/")
                    || entry.p == ".git/commondir"
                    || entry.p == ".git/gitdir")
        );
        let target = temp.path().join("independent");
        std::fs::create_dir(&target).unwrap();
        let target_files = files(&target);
        target_files
            .materialize(
                orch_link_contract::commands::MaterializePurpose::Move,
                "checkout",
                &parsed,
                &blobs,
                &crate::snapshot::NoProgress,
            )
            .await
            .unwrap();
        let checkout = target.join("checkout");
        let states = crate::file_states::FileStates::new(
            crate::state_store::ObjectStore::open(&temp.path().join("target-states"), &target)
                .unwrap(),
        );
        verify_independent(
            &crate::git::GitTool::discover(),
            &target_files,
            &states,
            "checkout",
        )
        .await
        .unwrap();
        assert!(
            !marker.exists(),
            "copy and verification run no repository filters or hooks"
        );
        assert_eq!(
            git(&checkout, &["symbolic-ref", "--short", "HEAD"]),
            b"attempt\n"
        );
        assert_eq!(git(&checkout, &["show", ":code"]), b"staged\n");
        assert_eq!(std::fs::read(checkout.join("code")).unwrap(), b"current\n");
        assert_eq!(
            std::fs::read(checkout.join("binary")).unwrap(),
            [0, 255, 17]
        );
        assert_eq!(
            std::fs::read(checkout.join("src/nested/code.rs")).unwrap(),
            b"raw nested files\n"
        );
        assert!(checkout.join("empty").is_dir());
        assert_eq!(
            std::fs::read_link(checkout.join("link")).unwrap(),
            Path::new("code")
        );
        assert_eq!(std::fs::read(checkout.join(".git/index")).unwrap(), index);
        assert_eq!(std::fs::read(text(&index_path).unwrap()).unwrap(), index);
        assert_eq!(
            std::fs::read(source.join("repo/.git/config")).unwrap(),
            config
        );
        assert_eq!(std::fs::read(source.join("repo/.git/HEAD")).unwrap(), head);
        git(&checkout, &["config", "independent.setting", "target-only"]);
        assert_eq!(
            std::fs::read(source.join("repo/.git/config")).unwrap(),
            config
        );
        assert_eq!(
            git(&checkout, &["show-ref", "refs/heads/main"]),
            git(&source.join("repo"), &["show-ref", "refs/heads/main"])
        );
    }

    #[tokio::test]
    async fn a_missing_imported_object_never_passes_the_independent_checkout_check() {
        let (temp, source, _target, _parsed) = fixture();
        let files = files(&source);
        let selected = orch_link_contract::commands::Selection {
            include: vec!["attempt".into()],
            exclude: Vec::new(),
        };
        let blobs = crate::blobs::MemoryBlobs::new();
        let clock = orch_types::ManualClock::new(
            "2026-10-14T22:00:00.000Z"
                .parse::<orch_types::Time>()
                .unwrap()
                .get(),
        );
        let (report, _) = files
            .snapshot_independent(
                &selected,
                "attempt",
                1,
                &clock,
                &blobs,
                &crate::snapshot::NoProgress,
            )
            .await
            .unwrap();
        let mut parsed =
            manifest::parse(&blobs.get_bytes(&report.result.manifest_blob).unwrap()).unwrap();
        parsed
            .entries
            .retain(|entry| !entry.p.starts_with(".git/objects/"));
        let target = temp.path().join("missing-objects");
        std::fs::create_dir(&target).unwrap();
        let target_files = self::files(&target);
        target_files
            .materialize(
                orch_link_contract::commands::MaterializePurpose::Move,
                "checkout",
                &parsed,
                &blobs,
                &crate::snapshot::NoProgress,
            )
            .await
            .unwrap();
        let states = crate::file_states::FileStates::new(
            crate::state_store::ObjectStore::open(&temp.path().join("missing-states"), &target)
                .unwrap(),
        );
        assert!(
            verify_independent(
                &crate::git::GitTool::discover(),
                &target_files,
                &states,
                "checkout"
            )
            .await
            .is_err()
        );
        assert_eq!(
            std::fs::read(source.join("attempt/code")).unwrap(),
            b"current\n",
            "the original remains usable"
        );
    }
}
