//! Write files, metadata and links from available blobs: a moved workspace, or files delivered with a message.
//!
//! The list of what to write is a manifest; the content of each file is a blob the store holds.
//! Directories are made first, then the files (a few at a time, each received into a file of its
//! own and renamed into place, so that a file is never half there), then the links. A file that
//! is already there with the content the list names is left alone, which is what makes a job that
//! was cut off cheap to run again; one that is there with other content is replaced.
//!
//! **Nothing is written through a link.** A manifest comes from another machine, and a path of it
//! that goes through a link, whether one that was in the target before or one the manifest itself
//! makes, is refused for that entry: a materialized tree can only hold what its list says, inside
//! the directory it was asked to fill. The paths are checked as every path is (no `..`, within
//! the root, and as the target environment writes names), what the target's ignore rules leave
//! out is not written, and a delivery goes only under a `.orch/` directory.

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use futures::StreamExt;
use orch_link_contract::commands::{FsMaterializeResult, MaterializePurpose};
use orch_types::Hash;

use crate::blobs::{BlobSource, FileWriter, download_checked};
use crate::error::{HostError, Result, blocking};
use crate::files::Files;
use crate::manifest::{EntryKind, ManifestEntry, Parsed};
use crate::paths::{Locate, Located, ORCH_DIR, RelPath, TEMP_PREFIX};
use crate::platform::{Dir, set_file_mode};
use crate::snapshot::{Progress, Tracker};

/// How many files are received at once. The link moves four transfers at a time for a machine,
/// so more would only wait.
pub const MATERIALIZE_PARALLEL: usize = 4;

/// The bits kept on a directory that was made with permissions that would keep the next run of
/// this job from writing into it: the owner may always write.
const DIRECTORY_OWNER_BITS: u32 = 0o700;

/// The permission bits a manifest entry may set; the special bits never come from outside.
const MODE_MASK: u32 = 0o777;

const PHASES: [(&str, u32); 3] = [("directories", 5), ("files", 90), ("links", 5)];

/// Where the phases stand in [`PHASES`].
const PHASE_DIRECTORIES: usize = 0;
const PHASE_FILES: usize = 1;
const PHASE_LINKS: usize = 2;

/// How many of the entries that could not be written an error names.
const FAILURES_SHOWN: usize = 3;

/// What a materialization did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializeReport {
    /// What the command answers with: files written, bytes written, entries already right.
    pub result: FsMaterializeResult,
    /// Files the list names without content, which could not be written.
    pub unavailable: Vec<String>,
    /// Entries the target's ignore rules leave out, which were not written.
    pub left_out: Vec<String>,
}

enum Done {
    Written(i64),
    Skipped,
    Unavailable,
    LeftOut,
}

fn outcome_text(rel: &RelPath, why: impl std::fmt::Display) -> String {
    format!("{rel}: {why}")
}

impl Files {
    /// Where `text` is below the workspace root: a path from the root, or an absolute path that
    /// lies below it.
    pub fn target_of(&self, text: &str) -> Result<RelPath> {
        let path = Path::new(text);
        if path.is_absolute() || text.starts_with('/') {
            let below = self.root().relative_to_root(path).ok_or_else(|| {
                HostError::RootNotAllowed(format!("{text} is not inside the workspace root"))
            })?;
            let joined = below.to_string_lossy().replace('\\', "/");
            return self.parse(&joined);
        }
        self.parse(text)
    }

    /// Where an entry of the manifest goes: below the target, spelled as this environment spells paths.
    fn place(&self, target: &RelPath, entry: &ManifestEntry) -> Result<RelPath> {
        let joined = if target.is_root() {
            entry.p.clone()
        } else {
            format!("{target}/{}", entry.p)
        };
        self.parse(&joined)
    }

    /// Finds where `rel` is, making the directories above it, and refuses a path that goes through a link.
    fn locate_plain(&self, rel: &RelPath) -> Result<Located> {
        let located = self.root().locate(rel, Locate::FOR_CREATE)?;
        if located.real != *rel {
            return Err(HostError::NotFound(format!(
                "{rel} goes through a link, and nothing is written through one"
            )));
        }
        Ok(located)
    }

    /// Writes the entries of `parsed` below `target_root`, taking content from `source`.
    pub async fn materialize(
        &self,
        purpose: MaterializePurpose,
        target_root: &str,
        parsed: &Parsed,
        source: &dyn BlobSource,
        progress: &dyn Progress,
    ) -> Result<MaterializeReport> {
        let gates = self.gates();
        match purpose {
            MaterializePurpose::Move => gates.require_remote_write()?,
            MaterializePurpose::Exact => {
                return Err(HostError::Unsupported(
                    "exact saved files use the durable unused-directory writer".into(),
                ));
            }
            MaterializePurpose::Delivery => gates.require_deliver_files()?,
        }
        if !self.is_writable() {
            return Err(HostError::Unsupported(
                "an attached mount is not written through Orch".to_owned(),
            ));
        }
        let target = self.target_of(target_root)?;
        if purpose == MaterializePurpose::Delivery && !target.has_component(ORCH_DIR) {
            return Err(HostError::Unsupported(
                "a delivery is written under .orch/ and nowhere else".to_owned(),
            ));
        }
        let mut tracker = Tracker::new(progress, &PHASES);
        let bytes_total: i64 = parsed
            .entries
            .iter()
            .filter(|e| e.t == EntryKind::File)
            .map(|e| e.s)
            .sum();
        let mut report = MaterializeReport {
            result: FsMaterializeResult {
                files: 0,
                bytes: 0,
                skipped: 0,
            },
            unavailable: Vec::new(),
            left_out: Vec::new(),
        };
        let mut failures: Vec<String> = Vec::new();

        // directories
        let dirs: Vec<&ManifestEntry> = parsed
            .entries
            .iter()
            .filter(|e| e.t == EntryKind::Dir)
            .collect();
        tracker.begin(PHASE_DIRECTORIES, i64::try_from(dirs.len()).ok(), None);
        let this = self.clone();
        let target_for_dirs = target.clone();
        let made: Vec<(String, Result<bool>)> = {
            let planned: Vec<(String, u32)> = dirs.iter().map(|e| (e.p.clone(), e.x)).collect();
            blocking(move || {
                let mut outcomes = Vec::new();
                for (path, mode) in planned {
                    let joined = if target_for_dirs.is_root() {
                        path.clone()
                    } else {
                        format!("{target_for_dirs}/{path}")
                    };
                    let outcome = this
                        .parse(&joined)
                        .and_then(|rel| this.ensure_dir(&rel, mode));
                    outcomes.push((path, outcome));
                }
                Ok(outcomes)
            })
            .await?
        };
        for (path, outcome) in made {
            match outcome {
                Ok(true) => {}
                Ok(false) => report.left_out.push(path),
                Err(error) => {
                    failures.push(outcome_text(&RelPath::root(), format!("{path}: {error}")))
                }
            }
            tracker.advance(1, 0);
        }

        // files
        let files: Vec<&ManifestEntry> = parsed
            .entries
            .iter()
            .filter(|e| e.t == EntryKind::File)
            .collect();
        tracker.begin(
            PHASE_FILES,
            i64::try_from(files.len()).ok(),
            Some(bytes_total),
        );
        // The entries are handed to the stream by value: a closure over references would leave the future
        // unprovable as `Send` to the compiler, and the connector runs it on a runtime that moves it.
        let owned: Vec<ManifestEntry> = files.iter().map(|entry| (*entry).clone()).collect();
        let mut stream = futures::stream::iter(owned.into_iter().map(|entry| {
            let this = self.clone();
            let target = target.clone();
            async move {
                let outcome = this.write_entry(&target, &entry, source).await;
                (entry, outcome)
            }
        }))
        .buffer_unordered(MATERIALIZE_PARALLEL);
        while let Some((entry, outcome)) = stream.next().await {
            crate::error::check_cancelled()?;
            match outcome {
                Ok(Done::Written(bytes)) => {
                    report.result.files += 1;
                    report.result.bytes += bytes;
                }
                Ok(Done::Skipped) => report.result.skipped += 1,
                Ok(Done::Unavailable) => report.unavailable.push(entry.p.clone()),
                Ok(Done::LeftOut) => report.left_out.push(entry.p.clone()),
                Err(error) => failures.push(outcome_text(
                    &RelPath::root(),
                    format!("{}: {error}", entry.p),
                )),
            }
            tracker.advance(1, entry.s);
        }
        drop(stream);

        // links
        let links: Vec<&ManifestEntry> = parsed
            .entries
            .iter()
            .filter(|e| e.t == EntryKind::Link)
            .collect();
        tracker.begin(PHASE_LINKS, i64::try_from(links.len()).ok(), None);
        for entry in links {
            crate::error::check_cancelled()?;
            let this = self.clone();
            let place = self.place(&target, entry);
            let wanted = entry.l.clone();
            let outcome = blocking(move || {
                let rel = place?;
                let wanted = wanted.ok_or_else(|| {
                    HostError::Unsupported(
                        "a link in the list says nothing of where it points".to_owned(),
                    )
                })?;
                this.ensure_link(&rel, &wanted)
            })
            .await;
            match outcome {
                Ok(Done::Skipped) => report.result.skipped += 1,
                Ok(Done::LeftOut) => report.left_out.push(entry.p.clone()),
                Ok(_) => {}
                Err(error) => failures.push(outcome_text(
                    &RelPath::root(),
                    format!("{}: {error}", entry.p),
                )),
            }
            tracker.advance(1, 0);
        }
        tracker.finish();

        if failures.is_empty() {
            Ok(report)
        } else {
            let shown: Vec<&str> = failures
                .iter()
                .take(FAILURES_SHOWN)
                .map(String::as_str)
                .collect();
            Err(HostError::Internal(format!(
                "{} of the entries could not be written (written so far: {} files); the first: {}",
                failures.len(),
                report.result.files,
                shown.join("; ")
            )))
        }
    }

    /// Makes the directory `rel`; false when the target's rules leave it out.
    fn ensure_dir(&self, rel: &RelPath, mode: u32) -> Result<bool> {
        let located = self.locate_plain(rel)?;
        if located.name.is_none() {
            return Ok(true);
        }
        if self.check_visible(rel, &located, true).is_err() {
            return Ok(false);
        }
        match located.meta {
            Some(meta) if meta.is_dir() => {}
            Some(_) => {
                return Err(HostError::Exists(format!(
                    "{rel} is there and is not a directory"
                )));
            }
            None => {
                let name = located.name()?;
                located
                    .parent
                    .mkdir(name, DIRECTORY_OWNER_BITS | (mode & MODE_MASK))
                    .map_err(|e| HostError::io(format!("making {rel}"), e))?;
                self.note_orch_write(&located.real);
            }
        }
        Ok(true)
    }

    fn ensure_link(&self, rel: &RelPath, wanted: &str) -> Result<Done> {
        let located = self.locate_plain(rel)?;
        let name = located.name()?.to_owned();
        if self.check_visible(rel, &located, false).is_err() {
            return Ok(Done::LeftOut);
        }
        match located.meta {
            Some(meta) if meta.is_symlink() => {
                let current = located
                    .parent
                    .read_link(&name)
                    .map_err(|e| HostError::io(format!("reading the link {rel}"), e))?;
                if current == wanted {
                    return Ok(Done::Skipped);
                }
                located
                    .parent
                    .unlink(&name)
                    .map_err(|e| HostError::io(format!("replacing the link {rel}"), e))?;
            }
            Some(meta) if meta.is_file() => {
                located
                    .parent
                    .unlink(&name)
                    .map_err(|e| HostError::io(format!("replacing {rel}"), e))?;
            }
            Some(_) => {
                return Err(HostError::Exists(format!(
                    "{rel} is there and is a directory"
                )));
            }
            None => {}
        }
        located
            .parent
            .symlink(std::ffi::OsStr::new(wanted), &name)
            .map_err(|e| HostError::io(format!("making the link {rel}"), e))?;
        self.note_orch_write(&located.real);
        Ok(Done::Written(0))
    }

    async fn write_entry(
        &self,
        target: &RelPath,
        entry: &ManifestEntry,
        source: &dyn BlobSource,
    ) -> Result<Done> {
        let Some(hash) = entry.h else {
            return Ok(Done::Unavailable);
        };
        let rel = self.place(target, entry)?;
        let this = self.clone();
        let asked = rel.clone();
        let wanted_size = entry.s;
        let prepared = blocking(move || {
            let located = this.locate_plain(&asked)?;
            let name = located.name()?.to_owned();
            if this.check_visible(&asked, &located, false).is_err() {
                return Ok(None);
            }
            let mut current = None;
            if let Some(meta) = located.meta {
                if meta.is_dir() {
                    return Err(HostError::Exists(format!("{asked} is a directory")));
                }
                if meta.is_file() && i64::try_from(meta.size).ok() == Some(wanted_size) {
                    current = this.hash_in(&located.parent, &name, &located.real)?;
                }
            }
            Ok(Some((located.parent, name, located.real, current)))
        })
        .await?;
        let Some((parent, name, real, current)) = prepared else {
            return Ok(Done::LeftOut);
        };
        if current == Some(hash) {
            return Ok(Done::Skipped);
        }
        let temp = OsString::from(format!("{TEMP_PREFIX}{}", orch_types::new_id().simple()));
        let file = parent
            .create_new(&temp, 0o666)
            .map_err(|e| HostError::io(format!("receiving {rel}"), e))?;
        let _guard = TempFile {
            parent: parent
                .try_clone()
                .map_err(|e| HostError::io("holding a directory", e))?,
            name: temp.clone(),
        };
        let file = Arc::new(file);
        let mut writer = FileWriter::new(Arc::clone(&file));
        let size =
            download_checked(source, hash, &mut writer)
                .await
                .map_err(|error| match error {
                    crate::blobs::BlobError::HashMismatch => HostError::Changed(format!(
                        "the content of {rel} was not what the list names"
                    )),
                    other => HostError::Content(other.to_string()),
                })?;
        let mode = entry.x & MODE_MASK;
        let mtime = entry.m;
        let this = self.clone();
        blocking(move || {
            set_file_mode(&file, mode)
                .map_err(|e| HostError::io("setting the permissions of a received file", e))?;
            let at = std::time::UNIX_EPOCH
                + std::time::Duration::from_nanos(u64::try_from(mtime).unwrap_or(0));
            let _ = file.set_modified(at);
            parent
                .rename_to(&temp, &parent, &name, true)
                .map_err(|e| HostError::io("putting a received file in place", e))?;
            // The guard finds nothing to remove once the name is the file's own.
            if let Ok(meta) = parent.lstat(&name) {
                this.hashes().put(&real, meta.identity(), hash);
            }
            this.note_orch_write(&real);
            Ok(())
        })
        .await?;
        Ok(Done::Written(size))
    }

    /// The hash of the regular file `name` in `parent`, from the cache when this version is known.
    fn hash_in(
        &self,
        parent: &Dir,
        name: &std::ffi::OsStr,
        real: &RelPath,
    ) -> Result<Option<Hash>> {
        let file = match parent.open_read(name) {
            Ok(file) => file,
            Err(_) => return Ok(None),
        };
        let version = crate::platform::file_meta(&file)
            .map_err(|e| HostError::io(format!("looking at {real}"), e))?
            .identity();
        if let Some(hash) = self.hashes().get(real, &version) {
            return Ok(Some(hash));
        }
        let hash = crate::blobs::hash_file(&file)
            .map_err(|e| HostError::io(format!("reading {real}"), e))?;
        self.hashes().put(real, version, hash);
        Ok(Some(hash))
    }
}

/// Removes a received file that was never put in place.
struct TempFile {
    parent: Dir,
    name: OsString,
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = self.parent.unlink(&self.name);
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::blobs::{BlobError, BlobWriter, MemoryBlobs};
    use crate::environment::{GateState, Gates, Limits};
    use crate::ignore::IgnoreRules;
    use crate::index::HashCache;
    use crate::manifest;
    use crate::paths::{PathStyle, Workspace};
    use crate::snapshot::NoProgress;
    use async_trait::async_trait;
    use orch_link_contract::commands::Selection;
    use orch_types::ManualClock;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    fn files_in(root: &std::path::Path, patterns: &[&str], gates: Gates) -> Files {
        let ws = Workspace::open(root, &[], PathStyle::Posix, true).unwrap();
        let rules = Arc::new(IgnoreRules::new(
            &patterns.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>(),
            true,
        ));
        Files::workspace(
            ws,
            rules,
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(gates)),
        )
    }

    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn clock() -> ManualClock {
        ManualClock::new(chrono::DateTime::from_timestamp(1_791_484_919, 0).unwrap())
    }

    fn source_tree(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("app/src/deep")).unwrap();
        std::fs::create_dir_all(root.join("app/empty")).unwrap();
        std::fs::write(root.join("app/src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("app/src/deep/x.txt"), "x").unwrap();
        std::fs::write(root.join("app/run.sh"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(
            root.join("app/run.sh"),
            std::fs::Permissions::from_mode(0o750),
        )
        .unwrap();
        symlink("src/main.rs", root.join("app/main-link")).unwrap();
        std::fs::write(root.join("app/binary"), vec![0u8, 1, 2, 255, 254]).unwrap();
        std::fs::write(root.join("app/zero"), "").unwrap();
    }

    /// Everything about a tree that a move must carry: paths, kinds, content, permissions, times, links.
    fn describe(root: &std::path::Path) -> Vec<String> {
        let mut out = Vec::new();
        fn visit(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
            let mut names: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            names.sort();
            for path in names {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let meta = std::fs::symlink_metadata(&path).unwrap();
                if meta.file_type().is_symlink() {
                    out.push(format!(
                        "l {rel} -> {}",
                        std::fs::read_link(&path).unwrap().display()
                    ));
                } else if meta.is_dir() {
                    out.push(format!("d {rel}"));
                    visit(base, &path, out);
                } else {
                    let content = Hash::of(&std::fs::read(&path).unwrap());
                    out.push(format!(
                        "f {rel} {} mode={:o} mtime={}",
                        content,
                        meta.mode() & 0o777,
                        meta.mtime()
                    ));
                }
            }
        }
        visit(root, root, &mut out);
        out
    }

    #[test]
    fn a_tree_is_snapshotted_and_materialized_into_an_equal_tree() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        source_tree(source.path());
        let from = files_in(source.path(), &[], Gates::OPEN);
        let to = files_in(target.path(), &[], Gates::OPEN);
        let store = MemoryBlobs::new();
        let selection = Selection {
            include: vec!["app".to_owned()],
            exclude: vec![],
        };
        let snap = block_on(from.snapshot(&selection, 1, &clock(), &store, &NoProgress)).unwrap();
        assert_eq!(snap.result.missing, 0);
        let parsed =
            manifest::parse(&store.get_bytes(&snap.result.manifest_blob).unwrap()).unwrap();

        let report = block_on(to.materialize(
            MaterializePurpose::Move,
            "copy/of/app",
            &parsed,
            &store,
            &NoProgress,
        ))
        .unwrap();
        assert_eq!(
            report.result.files, 5,
            "main.rs, x.txt, run.sh, binary and the empty file"
        );
        assert_eq!(report.result.skipped, 0);
        assert_eq!(
            describe(&source.path().join("app")),
            describe(&target.path().join("copy/of/app")),
            "an equal tree"
        );

        // Running it again writes nothing: everything is there with the right content.
        let again = block_on(to.materialize(
            MaterializePurpose::Move,
            "copy/of/app",
            &parsed,
            &store,
            &NoProgress,
        ))
        .unwrap();
        assert_eq!((again.result.files, again.result.bytes), (0, 0));
        assert_eq!(again.result.skipped, 6, "five files and the link");
    }

    #[test]
    fn a_file_that_is_there_with_other_content_is_replaced_and_the_rest_is_left_alone() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        source_tree(source.path());
        let from = files_in(source.path(), &[], Gates::OPEN);
        let to = files_in(target.path(), &[], Gates::OPEN);
        let store = MemoryBlobs::new();
        let snap = block_on(from.snapshot(
            &Selection {
                include: vec!["app".to_owned()],
                exclude: vec![],
            },
            1,
            &clock(),
            &store,
            &NoProgress,
        ))
        .unwrap();
        let parsed =
            manifest::parse(&store.get_bytes(&snap.result.manifest_blob).unwrap()).unwrap();
        std::fs::create_dir_all(target.path().join("src")).unwrap();
        std::fs::write(target.path().join("src/main.rs"), "something else\n").unwrap();
        std::fs::write(target.path().join("unrelated.txt"), "mine").unwrap();
        block_on(to.materialize(MaterializePurpose::Move, "", &parsed, &store, &NoProgress))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(target.path().join("src/main.rs")).unwrap(),
            "fn main() {}\n"
        );
        assert_eq!(
            std::fs::read_to_string(target.path().join("unrelated.txt")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn nothing_is_written_through_a_link_that_was_there_or_that_the_list_itself_makes() {
        let outside = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let to = files_in(target.path(), &[], Gates::OPEN);
        let store = MemoryBlobs::new();
        let blob = store.insert(&b"pwned"[..]);
        let header = manifest::ManifestHeader {
            v: 1,
            root: "/x".into(),
            generated_at: orch_types::Time::new(chrono::DateTime::from_timestamp(0, 0).unwrap()),
            generation: 1,
            case_sensitive: true,
        };
        let entry =
            |p: &str, t: EntryKind, h: Option<Hash>, l: Option<&str>| manifest::ManifestEntry {
                p: p.into(),
                t,
                s: 5,
                m: 0,
                x: 0o644,
                h,
                l: l.map(str::to_owned),
            };

        // A link that is in the target already, pointing out of it and pointing into it.
        symlink(outside.path(), target.path().join("out")).unwrap();
        std::fs::create_dir(target.path().join("inside")).unwrap();
        symlink("inside", target.path().join("alias")).unwrap();
        let entries = vec![
            entry("out/pwn.txt", EntryKind::File, Some(blob), None),
            entry("alias/pwn.txt", EntryKind::File, Some(blob), None),
            entry("fine.txt", EntryKind::File, Some(blob), None),
        ];
        let parsed = manifest::parse(&manifest::build(&header, entries).unwrap().bytes).unwrap();
        let error =
            block_on(to.materialize(MaterializePurpose::Move, "", &parsed, &store, &NoProgress))
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("2 of the entries could not be written"),
            "{error}"
        );
        assert!(!outside.path().join("pwn.txt").exists());
        assert!(
            !target.path().join("inside/pwn.txt").exists(),
            "not even a link that stays inside is written through"
        );
        assert!(
            target.path().join("fine.txt").exists(),
            "the rest of the list is still written"
        );

        // A list that makes a link and then writes through it: the file is refused.
        let target2 = tempfile::tempdir().unwrap();
        let to2 = files_in(target2.path(), &[], Gates::OPEN);
        let entries = vec![
            entry(
                "evil",
                EntryKind::Link,
                None,
                Some(outside.path().to_str().unwrap()),
            ),
            entry("evil/pwn.txt", EntryKind::File, Some(blob), None),
        ];
        let parsed = manifest::parse(&manifest::build(&header, entries).unwrap().bytes).unwrap();
        let _ =
            block_on(to2.materialize(MaterializePurpose::Move, "", &parsed, &store, &NoProgress));
        assert!(!outside.path().join("pwn.txt").exists());
    }

    #[test]
    fn the_target_cannot_be_outside_the_workspace_and_a_delivery_goes_only_under_orch() {
        let root = tempfile::tempdir().unwrap();
        let to = files_in(root.path(), &[], Gates::OPEN);
        let store = MemoryBlobs::new();
        let header = manifest::ManifestHeader {
            v: 1,
            root: "/x".into(),
            generated_at: orch_types::Time::new(chrono::DateTime::from_timestamp(0, 0).unwrap()),
            generation: 1,
            case_sensitive: true,
        };
        let blob = store.insert(&b"hello"[..]);
        let entries = vec![manifest::ManifestEntry {
            p: "note.txt".into(),
            t: EntryKind::File,
            s: 5,
            m: 0,
            x: 0o644,
            h: Some(blob),
            l: None,
        }];
        let parsed = manifest::parse(&manifest::build(&header, entries).unwrap().bytes).unwrap();
        assert!(
            block_on(to.materialize(
                MaterializePurpose::Move,
                "../elsewhere",
                &parsed,
                &store,
                &NoProgress
            ))
            .is_err()
        );
        assert!(matches!(
            block_on(to.materialize(
                MaterializePurpose::Move,
                "/somewhere/else",
                &parsed,
                &store,
                &NoProgress
            )),
            Err(HostError::RootNotAllowed(_))
        ));
        // A delivery under .orch/, by an absolute path inside the root, works; elsewhere it does not.
        let inside = format!(
            "{}/web/.orch/uploads/abc",
            root.path().canonicalize().unwrap().display()
        );
        block_on(to.materialize(
            MaterializePurpose::Delivery,
            &inside,
            &parsed,
            &store,
            &NoProgress,
        ))
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("web/.orch/uploads/abc/note.txt")).unwrap(),
            "hello"
        );
        assert!(matches!(
            block_on(to.materialize(
                MaterializePurpose::Delivery,
                "web/notes",
                &parsed,
                &store,
                &NoProgress
            )),
            Err(HostError::Unsupported(_))
        ));
    }

    #[test]
    fn a_machine_that_accepts_no_outside_writes_or_no_deliveries_accepts_neither_kind() {
        let root = tempfile::tempdir().unwrap();
        let store = MemoryBlobs::new();
        let header = manifest::ManifestHeader {
            v: 1,
            root: "/x".into(),
            generated_at: orch_types::Time::new(chrono::DateTime::from_timestamp(0, 0).unwrap()),
            generation: 1,
            case_sensitive: true,
        };
        let parsed = manifest::parse(&manifest::build(&header, vec![]).unwrap().bytes).unwrap();
        let closed = files_in(root.path(), &[], Gates::CLOSED);
        assert!(matches!(
            block_on(closed.materialize(
                MaterializePurpose::Move,
                "",
                &parsed,
                &store,
                &NoProgress
            )),
            Err(HostError::RemoteWriteDisabled)
        ));
        assert!(matches!(
            block_on(closed.materialize(
                MaterializePurpose::Delivery,
                "a/.orch",
                &parsed,
                &store,
                &NoProgress
            )),
            Err(HostError::DeliverFilesDisabled)
        ));
        // Deliveries and moves are separate permissions.
        let deliveries_only = files_in(
            root.path(),
            &[],
            Gates {
                remote_write: false,
                deliver_files: true,
                terminal: false,
                browser: false,
                vnc: false,
            },
        );
        assert!(matches!(
            block_on(deliveries_only.materialize(
                MaterializePurpose::Move,
                "",
                &parsed,
                &store,
                &NoProgress
            )),
            Err(HostError::RemoteWriteDisabled)
        ));
        assert!(
            block_on(deliveries_only.materialize(
                MaterializePurpose::Delivery,
                "a/.orch",
                &parsed,
                &store,
                &NoProgress
            ))
            .is_ok()
        );
    }

    #[test]
    fn what_the_target_leaves_out_is_not_written_and_what_has_no_content_is_reported() {
        let root = tempfile::tempdir().unwrap();
        let to = files_in(root.path(), &["*.secret"], Gates::OPEN);
        let store = MemoryBlobs::new();
        let blob = store.insert(&b"data"[..]);
        let header = manifest::ManifestHeader {
            v: 1,
            root: "/x".into(),
            generated_at: orch_types::Time::new(chrono::DateTime::from_timestamp(0, 0).unwrap()),
            generation: 1,
            case_sensitive: true,
        };
        let entry = |p: &str, h: Option<Hash>| manifest::ManifestEntry {
            p: p.into(),
            t: EntryKind::File,
            s: 4,
            m: 0,
            x: 0o644,
            h,
            l: None,
        };
        let entries = vec![
            entry("a.txt", Some(blob)),
            entry("b.secret", Some(blob)),
            entry("gone.txt", None),
        ];
        let parsed = manifest::parse(&manifest::build(&header, entries).unwrap().bytes).unwrap();
        let report =
            block_on(to.materialize(MaterializePurpose::Move, "", &parsed, &store, &NoProgress))
                .unwrap();
        assert_eq!(report.result.files, 1);
        assert_eq!(report.left_out, ["b.secret"]);
        assert_eq!(report.unavailable, ["gone.txt"]);
        assert!(!root.path().join("b.secret").exists() && !root.path().join("gone.txt").exists());
    }

    #[test]
    fn content_that_is_not_what_the_list_names_is_never_put_in_place_and_leaves_nothing_behind() {
        /// A store that answers every request with other content.
        struct Liar;
        #[async_trait]
        impl BlobSource for Liar {
            async fn get(
                &self,
                _hash: Hash,
                target: &mut dyn BlobWriter,
            ) -> std::result::Result<i64, BlobError> {
                target
                    .write(0, bytes::Bytes::from_static(b"not it"))
                    .await
                    .map_err(|e| BlobError::Local(e.to_string()))?;
                Ok(6)
            }
        }
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.txt"), "old").unwrap();
        let to = files_in(root.path(), &[], Gates::OPEN);
        let header = manifest::ManifestHeader {
            v: 1,
            root: "/x".into(),
            generated_at: orch_types::Time::new(chrono::DateTime::from_timestamp(0, 0).unwrap()),
            generation: 1,
            case_sensitive: true,
        };
        let entries = vec![manifest::ManifestEntry {
            p: "a.txt".into(),
            t: EntryKind::File,
            s: 4,
            m: 0,
            x: 0o644,
            h: Some(Hash::of(b"data")),
            l: None,
        }];
        let parsed = manifest::parse(&manifest::build(&header, entries).unwrap().bytes).unwrap();
        assert!(
            block_on(to.materialize(MaterializePurpose::Move, "", &parsed, &Liar, &NoProgress))
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "old"
        );
        let names: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            names.len(),
            1,
            "no half-received file stays behind: {names:?}"
        );
    }
}
